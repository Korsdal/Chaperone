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
use crate::config::BackendRoute;
use chapr_proto::BackendKind;
use chrono::{DateTime, Utc};
use sqlx::sqlite::SqlitePool;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

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
    pub auth: Arc<dyn Authenticator>,
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
    /// The config file this coord was started from, if any, and the auth mode
    /// name — both reported by the overview so an administrator can see *what
    /// this coordinator is configured for* without opening a shell. Also the
    /// hook the future Settings tab reads before it can offer to edit anything.
    pub config_path: Option<PathBuf>,
    pub auth_mode: String,
}

impl AppState {
    /// Construct state with a default blob root (`chapr-blobs`) and no
    /// connection auth. Callers override via the builders.
    pub fn new(pool: SqlitePool) -> Self {
        AppState {
            pool,
            acquire_lock: Arc::new(Mutex::new(())),
            blob_root: PathBuf::from("chapr-blobs"),
            auth: Arc::new(DisabledAuth),
            backend_default: BackendKind::default(),
            backend_routes: Arc::new(Vec::new()),
            started_at: Utc::now(),
            config_path: None,
            auth_mode: "disabled".to_string(),
        }
    }

    /// Record what this coord was started from, for the admin overview.
    pub fn with_deployment(mut self, config_path: Option<PathBuf>, auth_mode: &str) -> Self {
        self.config_path = config_path;
        self.auth_mode = auth_mode.to_string();
        self
    }

    /// Point the blob store at `root`.
    pub fn with_blob_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.blob_root = root.into();
        self
    }

    /// Set the connection authenticator (concept §13.1).
    pub fn with_auth(mut self, auth: Arc<dyn Authenticator>) -> Self {
        self.auth = auth;
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
