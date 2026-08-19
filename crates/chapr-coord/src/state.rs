// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! Shared application state.
//!
//! Coarse locking, exactly as the implementation notes prescribe for this stage
//! (§4): **one** lock around the coordination state. The `acquire_lock`
//! serialises the read-check-then-write critical section of `lease_acquire` so
//! the all-or-none conflict check and the insert are atomic as a unit. It is a
//! `tokio::sync::Mutex` because the guarded section spans `.await` points (the
//! SQLite calls). This is "slow" in a way completely invisible at ~20 users and
//! lets us defer lock-granularity work we do not need and would not feel the
//! absence of.

use crate::auth::{Authenticator, DisabledAuth};
use crate::config::{BackendRoute, Config};
use chapr_proto::BackendKind;
use chrono::{DateTime, Utc};
use sqlx::sqlite::SqlitePool;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use tokio::sync::Mutex;

/// How many recent requests the cutover panel looks at.
const AUTH_WINDOW: usize = 100;

/// Rolling record of which auth mode is admitting requests.
///
/// **Rolling, not cumulative, on purpose.** If a fallback is used heavily right
/// after a cutover and never again, a counter since start-up never returns to
/// zero, so a "remove the fallback when this is zero" gate would never open.
///
/// Failures are counted alongside successes because the two together are the only
/// honest signal. A client that cannot authenticate under the new mode does not
/// appear as fallback use — it appears as a rejection. Judging a cutover on
/// fallback use alone confuses "everyone moved over" with "everyone is failing".
pub struct AuthUsage {
    /// The last [`AUTH_WINDOW`] admitting modes. A `std::sync::Mutex`, not tokio's:
    /// the critical section is a push into a fixed vec and is never held across an
    /// `await`.
    recent: std::sync::Mutex<Ring>,
    /// Monotonic totals, for the log rather than the gate.
    successes: AtomicU64,
    failures: AtomicU64,
    /// Epoch millis of the most recent rejection, 0 if there has never been one.
    last_failure_ms: AtomicU64,
}

/// A fixed-size ring of recent modes. Cursor lives with the data so the two cannot
/// drift apart.
struct Ring {
    slots: Vec<&'static str>,
    next: usize,
}

impl Default for AuthUsage {
    fn default() -> Self {
        AuthUsage {
            recent: std::sync::Mutex::new(Ring {
                slots: Vec::with_capacity(AUTH_WINDOW),
                next: 0,
            }),
            successes: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            last_failure_ms: AtomicU64::new(0),
        }
    }
}

impl AuthUsage {
    /// Note that `mode` admitted a request.
    ///
    /// Drops the sample if the lock is momentarily contended: this sits on the hot
    /// path of every request, and blocking a request to record a statistic would be
    /// the wrong trade.
    pub fn record_success(&self, mode: &'static str) {
        self.successes.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut ring) = self.recent.try_lock() {
            if ring.slots.len() < AUTH_WINDOW {
                ring.slots.push(mode);
            } else {
                let slot = ring.next % AUTH_WINDOW;
                ring.slots[slot] = mode;
            }
            ring.next = ring.next.wrapping_add(1);
        }
    }

    /// Note a rejected request.
    pub fn record_failure(&self) {
        self.failures.fetch_add(1, Ordering::Relaxed);
        self.last_failure_ms.store(
            Utc::now().timestamp_millis().max(0) as u64,
            Ordering::Relaxed,
        );
    }

    /// How many of the recent admitted requests each mode accounted for.
    pub fn recent_by_mode(&self) -> Vec<(String, usize)> {
        let recent = match self.recent.try_lock() {
            Ok(r) => r.slots.clone(),
            Err(_) => return Vec::new(),
        };
        let mut out: Vec<(String, usize)> = Vec::new();
        for mode in recent {
            match out.iter_mut().find(|(m, _)| m == mode) {
                Some((_, n)) => *n += 1,
                None => out.push((mode.to_string(), 1)),
            }
        }
        out.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        out
    }

    pub fn totals(&self) -> (u64, u64) {
        (
            self.successes.load(Ordering::Relaxed),
            self.failures.load(Ordering::Relaxed),
        )
    }

    /// When a request was last rejected, if ever.
    pub fn last_failure(&self) -> Option<DateTime<Utc>> {
        let ms = self.last_failure_ms.load(Ordering::Relaxed);
        if ms == 0 {
            return None;
        }
        DateTime::<Utc>::from_timestamp_millis(ms as i64)
    }
}

/// Cloneable handle to coord's state. Cloning shares the same pool and lock
/// (both `Arc`-backed), so every axum handler contends on the one lock.
#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    /// The one coarse lock. Held across the acquire/release critical sections.
    pub acquire_lock: Arc<Mutex<()>>,
    /// Root directory of the content-addressed blob store (concept §12): the
    /// coord service's own volume, a separate mount from this operational DB.
    pub blob_root: PathBuf,
    /// Connection authenticator (concept §13.1). Default: no connection auth
    /// (handlers fall back to the body principal).
    ///
    /// Behind an `RwLock` so the settings surface can change the auth mode without
    /// a restart. That is only safe because the admin token is independent of the
    /// auth mode — you cannot lock yourself out — which is exactly what makes an
    /// auth cutover something you can attempt rather than commit to blind.
    /// The lock is never held across an `await`: `authenticate` is synchronous.
    auth: Arc<RwLock<Arc<dyn Authenticator>>>,
    /// Which auth mode has been letting requests in lately, and how many have been
    /// rejected. Read by the cutover panel; see [`AuthUsage`].
    pub auth_usage: Arc<AuthUsage>,
    /// The admin token, or `None` if one could not be established. `None` makes the
    /// administrative surface fail closed rather than open. Behind a lock because
    /// rotation replaces it on a running coordinator.
    admin_token: Arc<RwLock<Option<String>>>,
    /// The configuration this coordinator is running. Held so the settings surface
    /// can render and rewrite it, and so the overview can answer "what is this
    /// coordinator set up for" — the first question in any support call.
    config: Arc<RwLock<Config>>,
    /// Default backend kind announced on `resolve` when no route matches (§14).
    pub backend_default: BackendKind,
    /// Longest-prefix backend routes (empty by default). Admin-declared
    /// deployment topology, not per-resource ground truth — the swap-point for a
    /// future SQLite registry sits behind `index::backend_for`.
    pub backend_routes: Arc<Vec<BackendRoute>>,
    /// When this process started serving, for the admin overview's uptime.
    ///
    /// A wall-clock instant rather than an `Instant`: it is rendered for a person
    /// and has to survive being serialised, and a monotonic clock cannot be.
    pub started_at: DateTime<Utc>,
}

impl AppState {
    /// Construct state with a default blob root (`chapr-blobs`) and no
    /// connection auth. Callers override via the builders.
    pub fn new(pool: SqlitePool) -> Self {
        AppState {
            pool,
            acquire_lock: Arc::new(Mutex::new(())),
            blob_root: PathBuf::from("chapr-blobs"),
            auth: Arc::new(RwLock::new(Arc::new(DisabledAuth) as Arc<dyn Authenticator>)),
            auth_usage: Arc::new(AuthUsage::default()),
            admin_token: Arc::new(RwLock::new(None)),
            config: Arc::new(RwLock::new(Config::default())),
            backend_default: BackendKind::default(),
            backend_routes: Arc::new(Vec::new()),
            started_at: Utc::now(),
        }
    }

    /// Record the configuration this coordinator is running.
    ///
    /// Replaces the narrower `with_deployment` from the read-only slice: holding the
    /// whole config means the overview and the settings surface read one value
    /// rather than two copies that can disagree.
    pub fn with_config(self, cfg: Config) -> Self {
        self.set_config(cfg);
        self
    }

    /// Swap the live configuration (after a validated save).
    pub fn set_config(&self, cfg: Config) {
        match self.config.write() {
            Ok(mut slot) => *slot = cfg,
            Err(poisoned) => *poisoned.into_inner() = cfg,
        }
    }

    /// The configuration this coordinator is running.
    pub fn config(&self) -> Config {
        match self.config.read() {
            Ok(slot) => slot.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// The admin token, if one was established.
    pub fn admin_token(&self) -> Option<String> {
        match self.admin_token.read() {
            Ok(slot) => slot.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Replace the admin token (rotation).
    pub fn set_admin_token(&self, token: impl Into<String>) {
        let token = Some(token.into());
        match self.admin_token.write() {
            Ok(mut slot) => *slot = token,
            Err(poisoned) => *poisoned.into_inner() = token,
        }
    }

    /// Point the blob store at `root`.
    pub fn with_blob_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.blob_root = root.into();
        self
    }

    /// Set the connection authenticator (concept §13.1).
    pub fn with_auth(self, auth: Arc<dyn Authenticator>) -> Self {
        self.set_auth(auth);
        self
    }

    /// Swap the authenticator on a running coordinator (the settings surface).
    pub fn set_auth(&self, auth: Arc<dyn Authenticator>) {
        match self.auth.write() {
            Ok(mut slot) => *slot = auth,
            // A poisoned lock means a previous holder panicked while swapping.
            // Recovering is right: the alternative is a coordinator that can never
            // authenticate again.
            Err(poisoned) => *poisoned.into_inner() = auth,
        }
    }

    /// The authenticator to use for this request.
    pub fn authenticator(&self) -> Arc<dyn Authenticator> {
        match self.auth.read() {
            Ok(slot) => slot.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Give this coordinator its admin token.
    pub fn with_admin_token(self, token: impl Into<String>) -> Self {
        self.set_admin_token(token);
        self
    }

    /// Set the backend topology announced on `resolve` (§14): a global default
    /// kind plus optional longest-prefix routes.
    pub fn with_backends(mut self, default: BackendKind, routes: Vec<BackendRoute>) -> Self {
        self.backend_default = default;
        self.backend_routes = Arc::new(routes);
        self
    }
}
