// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! The lease reaper — the proactive half of expiry (concept §9).
//!
//! `lease_acquire` already sweeps expired leases lazily, so the reaper is not
//! required for correctness; it exists so an orphaned lease from a slept or
//! crashed laptop clears on a timer instead of lingering until the next
//! acquisition happens to touch its path. It also becomes the natural home for
//! the `lease_expire` audit event once the audit log lands.
//!
//! Runs under the same coarse lock as acquire/renew, so a reap can never race a
//! grant.

use crate::lease;
use crate::state::AppState;
use chapr_proto::{CanonicalPath, ChaprError, LeaseId, Principal, SessionId};
use chrono::Utc;
use sqlx::Row;
use std::time::Duration;
use tokio::task::JoinHandle;

/// Delete every lease whose heartbeat or hard ceiling has passed. Returns the
/// number of path-rows removed. Idempotent; safe to call as often as you like.
pub async fn reap_once(st: &AppState) -> Result<u64, ChaprError> {
    let _guard = st.acquire_lock.lock().await;
    let now_ms = Utc::now().timestamp_millis();
    let db_err = |e: sqlx::Error| ChaprError::Internal {
        message: format!("coord db error: {e}"),
    };

    // Audit each expiring lease-path row before removing it (concept §13.1).
    let expiring = sqlx::query(
        "SELECT lease_id, principal, session_id, path FROM leases
         WHERE expiry_ms <= ?1 OR hard_expiry_ms <= ?1",
    )
    .bind(now_ms)
    .fetch_all(&st.pool)
    .await
    .map_err(db_err)?;
    for row in &expiring {
        lease::audit_expire(
            st,
            &Principal::new_unchecked(row.get::<String, _>("principal")),
            &SessionId::new_unchecked(row.get::<String, _>("session_id")),
            &CanonicalPath::new_unchecked(row.get::<String, _>("path")),
            &LeaseId::new_unchecked(row.get::<String, _>("lease_id")),
            "reaped",
        )
        .await?;
    }

    let removed = sqlx::query("DELETE FROM leases WHERE expiry_ms <= ?1 OR hard_expiry_ms <= ?1")
        .bind(now_ms)
        .execute(&st.pool)
        .await
        .map_err(db_err)?
        .rows_affected();
    Ok(removed)
}

/// Spawn the background reaper. Fires every `period`; a slow tick is delayed
/// rather than fired in a burst. The task runs for the life of the process.
pub fn spawn(st: AppState, period: Duration) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(period);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            match reap_once(&st).await {
                Ok(n) if n > 0 => tracing::debug!(reaped = n, "reaped expired lease rows"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "lease reaper sweep failed"),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use chapr_proto::{CanonicalPath, LeasePurpose, Principal};
    use chrono::Duration as ChronoDuration;

    #[tokio::test]
    async fn reaps_expired_but_spares_live() {
        let st = AppState::new(db::test_pool().await);

        // A live lease (real acquire).
        crate::lease::acquire(
            &st,
            Principal::new_unchecked("A"),
            SessionId::new_unchecked("s"),
            LeasePurpose::Write,
            vec![CanonicalPath::new_unchecked("\\\\srv\\share\\live.md")],
        )
        .await
        .unwrap();

        // A hand-inserted expired lease.
        let past = (Utc::now() - ChronoDuration::seconds(1)).timestamp_millis();
        sqlx::query(
            "INSERT INTO leases
               (lease_id, path, principal, session_id, purpose,
                granted_at_ms, ttl_s, renewed_at_ms, hard_expiry_ms, expiry_ms)
             VALUES ('dead', '\\\\srv\\share\\dead.md', 'A', 's', 'write', ?1, 90, ?1, ?1, ?1)",
        )
        .bind(past)
        .execute(&st.pool)
        .await
        .unwrap();

        let removed = reap_once(&st).await.unwrap();
        assert_eq!(removed, 1, "only the expired lease should be reaped");

        // The live lease is untouched — its path is still blocked.
        let err = crate::lease::acquire(
            &st,
            Principal::new_unchecked("B"),
            SessionId::new_unchecked("s"),
            LeasePurpose::Write,
            vec![CanonicalPath::new_unchecked("\\\\srv\\share\\live.md")],
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ChaprError::LeaseHeld { .. }));
    }

    #[tokio::test]
    async fn reaping_nothing_returns_zero() {
        let st = AppState::new(db::test_pool().await);
        assert_eq!(reap_once(&st).await.unwrap(), 0);
    }
}
