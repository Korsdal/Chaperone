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
use chapr_proto::{AcquireLeaseRequest, ChaprError, LeaseAcquireResponse, LeaseId};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// Renewal interval: TTL ÷ 3 (concept §9, §16). One dropped renewal never drops
/// the lease.
pub const RENEWAL_INTERVAL: Duration = Duration::from_secs(30);

struct Held {
    lost: bool,
}

/// Owns the set of leases this endpoint holds and renews them in the background.
pub struct LeaseManager {
    coord: CoordClient,
    held: Mutex<HashMap<LeaseId, Held>>,
    interval: Duration,
}

impl LeaseManager {
    pub fn new(coord: CoordClient) -> Self {
        LeaseManager {
            coord,
            held: Mutex::new(HashMap::new()),
            interval: RENEWAL_INTERVAL,
        }
    }

    /// Override the renewal interval (tests use a tiny one).
    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Acquire a lease via coord and start renewing it.
    pub async fn acquire(
        &self,
        req: &AcquireLeaseRequest,
    ) -> Result<LeaseAcquireResponse, ChaprError> {
        let resp = self.coord.lease_acquire(req).await?;
        self.held
            .lock()
            .await
            .insert(resp.lease_id.clone(), Held { lost: false });
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
        self.held.lock().await.remove(lease_id);
        let result = self.coord.lease_release(lease_id).await;
        if let Err(e) = &result {
            tracing::warn!(
                lease = %lease_id, error = %e,
                "lease release failed; it will lapse at the heartbeat TTL"
            );
        }
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
            if let Err(e) = self.coord.lease_renew(&id).await {
                // Expired / reaped / coord unreachable → mark lost, stop renewing.
                if let Some(h) = self.held.lock().await.get_mut(&id) {
                    h.lost = true;
                }
                tracing::warn!(lease = %id, error = %e, "lease renewal failed; lease marked lost");
            }
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

    #[tokio::test]
    async fn coord_unreachable_renewal_marks_lost() {
        // Acquire against a live mock, then point renewals at a dead port.
        let server = MockServer::start().await;
        mount_acquire(&server, "lease-x").await;
        let mgr = LeaseManager::new(CoordClient::new(server.uri()));
        let lease = mgr.acquire(&acquire_req()).await.unwrap();

        // Swap in an unreachable coord by building a fresh manager that shares
        // nothing — instead, simulate by renewing against a closed port.
        let dead = LeaseManager::new(CoordClient::new("http://127.0.0.1:1"));
        dead.held
            .lock()
            .await
            .insert(lease.lease_id.clone(), Held { lost: false });
        dead.renew_all_once().await;
        assert!(!dead.is_held(&lease.lease_id).await);
    }
}
