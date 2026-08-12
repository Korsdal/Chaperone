//! Lease lifecycle + background renewal (concept §9, §15).
//!
//! A lease's heartbeat TTL (90 s) is decoupled from how long an operation takes:
//! renewal covers duration, TTL covers only the gap between heartbeats. So the
//! endpoint runs a **background timer task**, independent of tool calls, that
//! renews every held lease on an interval (30 s = TTL ÷ 3, tolerating one
//! dropped renewal). It keeps firing while the model is mid-generation and the
//! server is otherwise idle — the whole point of a persistent endpoint process.
//!
//! This is the async-ownership tax the implementation notes (§5) flag: a
//! long-lived task sharing the held-lease set with the task servicing tool
//! calls. The rule that keeps it correct: **never hold the lock across an
//! `.await`.** `renew_all_once` snapshots the ids under the lock, drops it,
//! renews over the network, then re-locks briefly to record the outcome.
//!
//! On a failed renewal (expired, reaped, or coord unreachable) the lease is
//! marked **lost** and no longer renewed; a holder learns via [`is_held`]
//! rather than wrongly assuming it still holds (concept §15). Correctness does
//! not depend on any of this — the exclusive open + CAS is the core, and a lost
//! lease degrades to the conflict path, never to data loss (invariant 3).

use crate::coord_client::CoordClient;
use crate::pathlock::{PathGuards, PathLocks};
use chapr_proto::{AcquireLeaseRequest, ChaprError, LeaseAcquireResponse, LeaseId};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// Renewal interval: TTL ÷ 3 (concept §9, §16). One dropped renewal never drops
/// the lease.
pub const RENEWAL_INTERVAL: Duration = Duration::from_secs(30);

/// How long to keep waiting for a lease another *process* holds before handing
/// the caller a "still busy" answer (E-027).
///
/// Bounded on purpose (CLAUDE.md failure directions: "bounded retries, exp
/// backoff + jitter, per-file budget, terminal ask-the-human state" — an LLM will
/// otherwise retry forever). Generous on purpose too: data integrity over speed,
/// and a salesperson running a workflow already expects it to take time.
pub const DEFAULT_ACQUIRE_BUDGET: Duration = Duration::from_secs(30);

/// First backoff step; doubles up to [`MAX_BACKOFF`].
const FIRST_BACKOFF: Duration = Duration::from_millis(250);
const MAX_BACKOFF: Duration = Duration::from_secs(4);

struct Held {
    lost: bool,
    /// Consecutive failed renewals (I-008). Reset by every success.
    ///
    /// One failed renewal is not a lost lease — the whole reason the renewal
    /// interval is TTL ÷ 3 is that a dropped heartbeat should be survivable.
    /// Declaring the lease lost on the first failure, and then never renewing it
    /// again, made a momentary network blip permanent.
    failures: u32,
    /// The heartbeat TTL coord granted, in seconds. Taken from the grant rather
    /// than hardcoded, so the tolerance below stays correct if coord's TTL changes.
    ttl_s: u32,
    /// The in-process path locks this lease was granted under (E-027). Held for
    /// exactly the lease's lifetime, so they are dropped by `release` — which is
    /// what lets the next waiter through in the right order.
    _guards: PathGuards,
}

/// Owns the set of leases this endpoint holds and renews them in the background.
pub struct LeaseManager {
    coord: CoordClient,
    held: Mutex<HashMap<LeaseId, Held>>,
    interval: Duration,
    /// Local per-path queueing, applied *before* coord is asked (E-027).
    locks: PathLocks,
    budget: Duration,
    /// Where to report a lease we have given up on (E-026). Optional because a
    /// renewal failure is background work with no tool call to attach to, so this
    /// is the only path by which it becomes visible to anyone.
    diagnostics: Option<(Arc<crate::diag::Diagnostics>, chapr_proto::Principal)>,
}

impl LeaseManager {
    pub fn new(coord: CoordClient) -> Self {
        LeaseManager {
            coord,
            held: Mutex::new(HashMap::new()),
            interval: RENEWAL_INTERVAL,
            locks: PathLocks::new(),
            budget: DEFAULT_ACQUIRE_BUDGET,
            diagnostics: None,
        }
    }

    /// Report a given-up lease to the diagnostics channel (E-026).
    pub fn with_diagnostics(
        mut self,
        diagnostics: Arc<crate::diag::Diagnostics>,
        principal: chapr_proto::Principal,
    ) -> Self {
        self.diagnostics = Some((diagnostics, principal));
        self
    }

    /// Override the renewal interval (tests use a tiny one).
    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Override how long `acquire` waits out a lease held elsewhere.
    pub fn with_acquire_budget(mut self, budget: Duration) -> Self {
        self.budget = budget;
        self
    }

    /// Acquire a lease via coord and start renewing it.
    ///
    /// Two layers of contention handling sit here, and they address different
    /// problems:
    ///
    /// 1. **Local queueing** ([`PathLocks`]) — subagents of *this* process take
    ///    the path in turn, so a session cannot collide with itself. This is the
    ///    common case in a parallel fan-out and it is resolved without a single
    ///    round-trip to coord.
    /// 2. **Bounded retry** — a lease held by another *process* (the genuine
    ///    cross-user contention Chaperone exists for) is waited out with
    ///    exponential backoff plus jitter, up to the configured budget.
    ///
    /// **Only `LeaseHeld` is retried.** That response proves nothing was granted
    /// (coord rolls its transaction back before returning it), so a retry has no
    /// side effect. A transport failure is *not* retried: coord may have granted
    /// a lease whose response was lost, and retrying would spin against our own
    /// invisible lease until the budget expired. Writes fail closed on an
    /// unreachable coord anyway (concept §10), which is the safe direction.
    pub async fn acquire(
        &self,
        req: &AcquireLeaseRequest,
    ) -> Result<LeaseAcquireResponse, ChaprError> {
        // Layer 1. Waits as long as necessary — a sibling subagent holds this for
        // one write, not indefinitely, and queueing is the whole point.
        let guards = self.locks.lock_all(&req.paths).await;

        // Layer 2.
        let started = Instant::now();
        let mut backoff = FIRST_BACKOFF;
        let mut attempts: u32 = 0;
        let resp = loop {
            attempts += 1;
            match self.coord.lease_acquire(req).await {
                Ok(resp) => break resp,
                Err(ChaprError::LeaseHeld { holder, paths }) => {
                    let spent = started.elapsed();
                    if spent + backoff > self.budget {
                        tracing::warn!(
                            waited_ms = spent.as_millis(),
                            attempts,
                            %holder,
                            "giving up waiting for a lease held elsewhere"
                        );
                        // The terminal "ask the human" state, not a bare failure —
                        // its whole reason for existing is that an LLM handed a
                        // retryable error will retry forever (§10). Names the path
                        // that was actually unavailable, which for an all-or-none
                        // set is more useful than the set.
                        return Err(ChaprError::RetryBudgetExhausted {
                            path: paths.into_iter().next().unwrap_or_else(|| {
                                req.paths.first().cloned().expect("acquire requires a path")
                            }),
                            attempts,
                        });
                    }
                    tracing::info!(
                        waited_ms = spent.as_millis(),
                        retry_in_ms = backoff.as_millis(),
                        attempts,
                        "path is being written by another session; waiting"
                    );
                    tokio::time::sleep(jittered(backoff)).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
                Err(e) => return Err(e),
            }
        };

        self.held.lock().await.insert(
            resp.lease_id.clone(),
            Held {
                lost: false,
                failures: 0,
                ttl_s: resp.ttl_s,
                _guards: guards,
            },
        );
        Ok(resp)
    }

    /// Stop renewing and release via coord.
    ///
    /// Local removal first is deliberate: the renewer must never resurrect a
    /// lease the endpoint has decided to drop. If coord's DELETE then fails the
    /// lease simply lapses at the 90 s heartbeat TTL, which is the safe
    /// direction — but every caller discards this `Result` (a release failure
    /// must not turn a completed write into an error), so log it here or the
    /// failure is invisible everywhere.
    pub async fn release(&self, lease_id: &LeaseId) -> Result<(), ChaprError> {
        // Taken out of the map (so the renewer stops seeing it) but deliberately
        // kept alive across the await: the entry owns this lease's path locks, and
        // dropping them before coord has released the lease would let the next
        // waiter through only to be told `LeaseHeld` by our own expiring lease.
        // Holding them until after the DELETE makes the handover ordered.
        let entry = self.held.lock().await.remove(lease_id);
        let result = self.coord.lease_release(lease_id).await;
        if let Err(e) = &result {
            tracing::warn!(
                lease = %lease_id, error = %e,
                "lease release failed; it will lapse at the heartbeat TTL"
            );
        }
        drop(entry);
        result
    }

    /// Whether the lease is still tracked and not marked lost.
    pub async fn is_held(&self, lease_id: &LeaseId) -> bool {
        self.held
            .lock()
            .await
            .get(lease_id)
            .map(|h| !h.lost)
            .unwrap_or(false)
    }

    /// One renewal pass over every live held lease. The lock is never held
    /// across the network calls (invariant of this module).
    pub async fn renew_all_once(&self) {
        // Snapshot the live lease ids, then drop the lock before any await.
        let ids: Vec<LeaseId> = {
            let held = self.held.lock().await;
            held.iter()
                .filter(|(_, h)| !h.lost)
                .map(|(id, _)| id.clone())
                .collect()
        };

        for id in ids {
            match self.coord.lease_renew(&id).await {
                Ok(_) => {
                    // A success clears the history: what matters is *consecutive*
                    // failures, not failures ever.
                    if let Some(h) = self.held.lock().await.get_mut(&id) {
                        h.failures = 0;
                    }
                }
                Err(e) => self.on_renew_failure(&id, e).await,
            }
        }
    }

    /// Decide what a failed renewal means (I-008).
    ///
    /// The defect this replaces marked a lease lost after **one** failure and
    /// never renewed it again, so a momentary blip permanently ended renewal and
    /// coord reaped the lease mid-write. Two kinds of failure need telling apart:
    ///
    /// - **Definitive** — coord says this lease is gone (reaped, expired, past its
    ///   hard ceiling, unknown id). Retrying cannot bring it back, and continuing
    ///   to believe we hold it is the wrong belief. Lost immediately.
    /// - **Transient** — coord could not be reached, or answered with an internal
    ///   error. The lease is probably still there. Keep it and try on the next
    ///   tick; the renewal interval is TTL ÷ 3 precisely so a dropped heartbeat is
    ///   survivable.
    ///
    /// The tolerance is derived rather than picked: after `ttl_s / interval`
    /// consecutive failures we have spent a full TTL without a heartbeat, so the
    /// lease has certainly lapsed on coord's side and holding the belief any
    /// longer would be false. Correctness never depended on any of this — the
    /// exclusive open plus CAS is the core (invariant 3) — but availability and an
    /// honest Leases view both do.
    async fn on_renew_failure(&self, id: &LeaseId, e: ChaprError) {
        let definitive = matches!(
            e,
            ChaprError::LeaseNotFound { .. }
                | ChaprError::LeaseExpired { .. }
                | ChaprError::MaxLeaseLifetimeExceeded { .. }
        );

        let (now_lost, failures) = {
            let mut held = self.held.lock().await;
            let Some(h) = held.get_mut(id) else {
                return; // released while we were on the network
            };
            h.failures += 1;
            let tolerance = (h.ttl_s / self.interval.as_secs().max(1) as u32).max(1);
            h.lost = definitive || h.failures >= tolerance;
            (h.lost, h.failures)
        };

        if now_lost {
            tracing::warn!(
                lease = %id, error = %e, failures,
                definitive,
                "lease given up as lost; renewal stopped"
            );
            // Surface it where an administrator will actually look. Without this
            // the symptom lives only in the endpoint's stderr, which goes nowhere
            // as a stdio child of Claude Desktop.
            if let Some((diagnostics, principal)) = &self.diagnostics {
                diagnostics
                    .report(
                        &self.coord,
                        principal,
                        &ChaprError::LeaseLost {
                            lease_id: id.clone(),
                        },
                    )
                    .await;
            }
        } else {
            tracing::info!(
                lease = %id, error = %e, failures,
                "lease renewal failed; keeping the lease and retrying on the next tick"
            );
        }
    }

    /// Spawn the background renewal task. Runs for the life of the process.
    pub fn spawn_renewer(self: Arc<Self>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(self.interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                self.renew_all_once().await;
            }
        })
    }
}

/// Spread `base` over `[base, 1.5 × base)`.
///
/// Jitter matters more than its quality here: several subagents released at the
/// same instant would otherwise retry in lockstep and keep colliding. Derived
/// from the clock rather than pulling in an RNG dependency — nothing about this
/// needs to be unpredictable, only uncorrelated.
fn jittered(base: Duration) -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    base + base.mul_f64(0.5 * (nanos % 1_000) as f64 / 1_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chapr_proto::{CanonicalPath, LeasePurpose, Principal, SessionId};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn acquire_req() -> AcquireLeaseRequest {
        AcquireLeaseRequest {
            principal: Principal::new_unchecked("CONTOSO\\demo"),
            session_id: SessionId::new_unchecked("sess-1"),
            purpose: LeasePurpose::Write,
            paths: vec![CanonicalPath::new_unchecked("\\\\srv\\share\\a.md")],
        }
    }

    async fn mount_acquire(server: &MockServer, lease_id: &str) {
        Mock::given(method("POST"))
            .and(path("/leases"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "lease_id": lease_id, "ttl_s": 90
            })))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn acquire_tracks_and_successful_renewal_keeps_it_held() {
        let server = MockServer::start().await;
        mount_acquire(&server, "lease-ok").await;
        Mock::given(method("POST"))
            .and(path("/leases/lease-ok/renew"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "renewed_until": "2026-07-21T09:30:00Z"
            })))
            .mount(&server)
            .await;

        let mgr = LeaseManager::new(CoordClient::new(server.uri()));
        let lease = mgr.acquire(&acquire_req()).await.unwrap();
        assert!(mgr.is_held(&lease.lease_id).await);

        mgr.renew_all_once().await;
        assert!(mgr.is_held(&lease.lease_id).await, "still held after a good renewal");
    }

    #[tokio::test]
    async fn failed_renewal_marks_the_lease_lost() {
        let server = MockServer::start().await;
        mount_acquire(&server, "lease-gone").await;
        Mock::given(method("POST"))
            .and(path("/leases/lease-gone/renew"))
            .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
                "code": "LEASE_NOT_FOUND", "lease_id": "lease-gone"
            })))
            .mount(&server)
            .await;

        let mgr = LeaseManager::new(CoordClient::new(server.uri()));
        let lease = mgr.acquire(&acquire_req()).await.unwrap();
        assert!(mgr.is_held(&lease.lease_id).await);

        mgr.renew_all_once().await;
        assert!(!mgr.is_held(&lease.lease_id).await, "renewal failure ⇒ lost");
    }

    /// A `LEASE_HELD` that clears is waited out, not surfaced.
    ///
    /// This is the cross-process case: another person's laptop holds the file.
    /// Before E-027 the first refusal came straight back to the model as a
    /// protocol-level internal error.
    #[tokio::test]
    async fn a_lease_held_by_another_session_is_waited_out() {
        let server = MockServer::start().await;
        // Higher priority + a single use, so it wins once and then stops matching.
        Mock::given(method("POST"))
            .and(path("/leases"))
            .respond_with(ResponseTemplate::new(409).set_body_json(serde_json::json!({
                "code": "LEASE_HELD",
                "holder": "CONTOSO\\someone-else",
                "paths": ["\\\\srv\\share\\a.md"]
            })))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        mount_acquire(&server, "lease-after-wait").await;

        let mgr = LeaseManager::new(CoordClient::new(server.uri()));
        let lease = mgr.acquire(&acquire_req()).await.expect("should wait, not fail");
        assert_eq!(lease.lease_id.as_str(), "lease-after-wait");
    }

    /// A lease that never clears becomes the terminal ask-the-human state.
    #[tokio::test]
    async fn an_unavailable_lease_ends_as_retry_budget_exhausted() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/leases"))
            .respond_with(ResponseTemplate::new(409).set_body_json(serde_json::json!({
                "code": "LEASE_HELD",
                "holder": "CONTOSO\\someone-else",
                "paths": ["\\\\srv\\share\\a.md"]
            })))
            .mount(&server)
            .await;

        let mgr = LeaseManager::new(CoordClient::new(server.uri()))
            .with_acquire_budget(Duration::from_millis(300));
        let err = mgr.acquire(&acquire_req()).await.unwrap_err();
        match err {
            // Not `LeaseHeld`: that reads as retryable, and an LLM handed a
            // retryable error retries forever (§10). This variant is the contract's
            // terminal state and names the path that was actually unavailable.
            ChaprError::RetryBudgetExhausted { path, attempts } => {
                assert_eq!(path.as_str(), "\\\\srv\\share\\a.md");
                assert!(attempts >= 1, "attempts should be counted, got {attempts}");
            }
            other => panic!("wrong error: {other:?}"),
        }
    }

    /// A transport failure must NOT be retried.
    #[tokio::test]
    async fn an_unreachable_coord_fails_immediately_rather_than_retrying() {
        // Coord may have granted a lease whose response was lost; retrying would
        // spin against our own invisible lease until the budget expired. Writes
        // fail closed on an unreachable coord anyway (concept §10).
        let mgr = LeaseManager::new(CoordClient::new("http://127.0.0.1:1"))
            .with_acquire_budget(Duration::from_secs(30));
        let started = Instant::now();
        let err = mgr.acquire(&acquire_req()).await.unwrap_err();
        assert!(
            matches!(err, ChaprError::CoordUnreachable),
            "wrong error: {err:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a transport error was retried: took {:?}",
            started.elapsed()
        );
    }

    /// Two concurrent acquires for one path are serialised locally, so the second
    /// never reaches coord while the first holds the lease.
    #[tokio::test]
    async fn same_path_acquires_are_serialised_within_the_process() {
        let server = MockServer::start().await;
        mount_acquire(&server, "lease-shared").await;
        let mgr = Arc::new(LeaseManager::new(CoordClient::new(server.uri())));

        let first = mgr.acquire(&acquire_req()).await.unwrap();

        // A second acquire for the same path must block on the local lock. It
        // would otherwise sail through, because this mock always grants.
        let mgr2 = mgr.clone();
        let blocked = tokio::spawn(async move { mgr2.acquire(&acquire_req()).await });
        let raced = tokio::time::timeout(Duration::from_millis(300), async {
            // `blocked` cannot finish while the first lease is held.
        })
        .await;
        assert!(raced.is_ok());
        assert!(!blocked.is_finished(), "the second acquire was not serialised");

        // Releasing the first hands the path over.
        Mock::given(method("DELETE"))
            .and(path("/leases/lease-shared"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;
        let _ = mgr.release(&first.lease_id).await;
        let second = tokio::time::timeout(Duration::from_secs(5), blocked)
            .await
            .expect("second acquire should proceed once the first released")
            .unwrap();
        assert!(second.is_ok(), "second acquire failed: {second:?}");
    }

    #[tokio::test]
    async fn release_stops_tracking() {
        let server = MockServer::start().await;
        mount_acquire(&server, "lease-rel").await;
        Mock::given(method("DELETE"))
            .and(path("/leases/lease-rel"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;

        let mgr = LeaseManager::new(CoordClient::new(server.uri()));
        let lease = mgr.acquire(&acquire_req()).await.unwrap();
        mgr.release(&lease.lease_id).await.unwrap();
        assert!(!mgr.is_held(&lease.lease_id).await);
    }

    /// A manager whose renewals go to a closed port, tracking one lease.
    ///
    /// `ttl_s` 90 against the default 30 s interval gives a tolerance of three
    /// consecutive failures — a full TTL without a heartbeat.
    async fn manager_with_unreachable_coord(id: &str) -> (LeaseManager, LeaseId) {
        let mgr = LeaseManager::new(CoordClient::new("http://127.0.0.1:1"));
        let lease_id = LeaseId::new_unchecked(id);
        mgr.held.lock().await.insert(
            lease_id.clone(),
            Held {
                lost: false,
                failures: 0,
                ttl_s: 90,
                _guards: Vec::new(),
            },
        );
        (mgr, lease_id)
    }

    #[tokio::test]
    async fn a_single_transient_renewal_failure_keeps_the_lease() {
        // I-008. One unreachable-coord blip used to end renewal for that lease
        // permanently, so coord reaped it mid-write. The renewal interval is
        // TTL ÷ 3 precisely so a dropped heartbeat is survivable — the manager has
        // to actually survive it.
        let (mgr, id) = manager_with_unreachable_coord("lease-blip").await;
        mgr.renew_all_once().await;
        assert!(
            mgr.is_held(&id).await,
            "one transient failure must not give up the lease"
        );
        assert_eq!(mgr.held.lock().await.get(&id).unwrap().failures, 1);
    }

    #[tokio::test]
    async fn a_success_clears_the_failure_history() {
        // What matters is *consecutive* failures. Two blips an hour apart are not
        // a lost lease.
        let server = MockServer::start().await;
        mount_acquire(&server, "lease-mix").await;
        Mock::given(method("POST"))
            .and(path("/leases/lease-mix/renew"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "renewed_until": "2026-07-21T09:30:00Z"
            })))
            .mount(&server)
            .await;
        let mgr = LeaseManager::new(CoordClient::new(server.uri()));
        let lease = mgr.acquire(&acquire_req()).await.unwrap();

        // Pretend two failures already happened, then let a real renewal land.
        mgr.held.lock().await.get_mut(&lease.lease_id).unwrap().failures = 2;
        mgr.renew_all_once().await;
        assert!(mgr.is_held(&lease.lease_id).await);
        assert_eq!(
            mgr.held.lock().await.get(&lease.lease_id).unwrap().failures,
            0,
            "a success must reset the counter, not leave the lease one blip from death"
        );
    }

    #[tokio::test]
    async fn transient_failures_give_up_once_a_whole_ttl_has_passed() {
        // The other side of the fix: we must not believe we hold a lease coord has
        // certainly reaped. Three failures at a 30 s interval is 90 s — the TTL.
        let (mgr, id) = manager_with_unreachable_coord("lease-gone-quietly").await;
        mgr.renew_all_once().await;
        mgr.renew_all_once().await;
        assert!(mgr.is_held(&id).await, "still inside the TTL");
        mgr.renew_all_once().await;
        assert!(
            !mgr.is_held(&id).await,
            "past a full TTL without a heartbeat the lease is gone"
        );
    }

    #[tokio::test]
    async fn a_definitive_failure_gives_up_immediately() {
        // Coord saying the lease does not exist is not a blip. Retrying cannot
        // bring it back, and continuing to believe we hold it is the wrong belief.
        let server = MockServer::start().await;
        mount_acquire(&server, "lease-reaped").await;
        Mock::given(method("POST"))
            .and(path("/leases/lease-reaped/renew"))
            .respond_with(ResponseTemplate::new(410).set_body_json(serde_json::json!({
                "code": "LEASE_EXPIRED", "lease_id": "lease-reaped"
            })))
            .mount(&server)
            .await;
        let mgr = LeaseManager::new(CoordClient::new(server.uri()));
        let lease = mgr.acquire(&acquire_req()).await.unwrap();

        mgr.renew_all_once().await;
        assert!(
            !mgr.is_held(&lease.lease_id).await,
            "an expired lease must be given up on the first answer"
        );
    }

    #[tokio::test]
    async fn giving_up_a_lease_is_reported_as_a_diagnostic() {
        // A renewal failure is background work with no tool call to attach to, so
        // this is the only path by which it becomes visible to an administrator.
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("diagnostics.jsonl");
        let (mut mgr, id) = manager_with_unreachable_coord("lease-report").await;
        mgr = mgr.with_diagnostics(
            Arc::new(crate::diag::Diagnostics::new(Some(log.clone()))),
            Principal::new_unchecked("CONTOSO\\jsmith"),
        );

        for _ in 0..3 {
            mgr.renew_all_once().await;
        }
        assert!(!mgr.is_held(&id).await);
        let text = std::fs::read_to_string(&log).expect("a given-up lease must be recorded");
        assert!(text.contains("LEASE_LOST"));
        // A warning, not an error: availability suffered, correctness did not.
        assert!(text.contains("\"warning\""));
    }
}
