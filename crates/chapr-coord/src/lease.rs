// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! The lease table: all-or-none acquire, release, and lazy expiry.
//!
//! Leases are an **optimisation**, not the correctness core (invariant 3) — the
//! system stays correct under exclusive-open + CAS even if every lease here is
//! wrong. That framing licenses the deliberately simple implementation: a
//! coarse lock, a single SQLite table, and lazy expiry instead of a background
//! reaper (the reaper and renewal arrive in E-003).
//!
//! ## All-or-none in canonical order (concept §9)
//!
//! A `lease_acquire` over a set grants every path or none, and paths are
//! processed in sorted (canonical) order. Ordered set acquisition is what kills
//! the classic A-holds-1-wants-2 / B-holds-2-wants-1 deadlock. Under the single
//! coarse lock the ordering cannot actually deadlock today, but the invariant
//! is honoured now so it survives any future move to finer locking.

use crate::audit;
use crate::state::AppState;
use chapr_proto::{
    AuditKind, CanonicalPath, ChaprError, LeaseAcquireResponse, LeaseId, LeasePurpose, LeaseRef,
    LeaseRenewResponse, Principal, SessionId,
};
use chrono::{DateTime, Duration, Utc};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

/// Heartbeat TTL (concept §9, §16). Short so an orphaned lease from a slept or
/// crashed laptop clears fast.
pub const HEARTBEAT_TTL_S: u32 = 90;

/// Hard lease-lifetime ceiling in seconds (concept §9): 20 minutes. Defends
/// against an agent stuck in a loop renewing forever. Enforced here as the
/// `hard_expiry_ms` column even though renewal itself lands in E-003.
pub const MAX_LEASE_LIFETIME_S: i64 = 20 * 60;

/// Acquire an all-or-none lease set over `paths` for `principal`.
///
/// Returns [`ChaprError::LeaseHeld`] naming the holder and the specific
/// conflicting paths if any requested path is already under a live lease —
/// nothing is granted in that case. Expired leases are swept lazily inside the
/// same transaction before the check, so a lease whose TTL has elapsed never
/// blocks a new acquisition.
pub async fn acquire(
    st: &AppState,
    principal: Principal,
    session_id: SessionId,
    purpose: LeasePurpose,
    paths: Vec<CanonicalPath>,
) -> Result<LeaseAcquireResponse, ChaprError> {
    if paths.is_empty() {
        return Err(ChaprError::InvalidPath {
            raw: String::new(),
            reason: "lease_acquire requires at least one path".into(),
        });
    }

    // Canonical order + de-dup. Paths arrive already canonicalised (the
    // endpoint's job, §5.1); here we only sort and de-duplicate so the set is
    // acquired deterministically and a repeated path is not double-inserted.
    let mut paths = paths;
    paths.sort();
    paths.dedup();

    let now = Utc::now();
    let now_ms = now.timestamp_millis();
    let hard_expiry_ms = (now + Duration::seconds(MAX_LEASE_LIFETIME_S)).timestamp_millis();
    let expiry_ms = (now + Duration::seconds(HEARTBEAT_TTL_S as i64)).timestamp_millis();

    // The one coarse critical section: sweep-expired -> conflict-check -> insert,
    // all atomic under the lock and one transaction.
    let _guard = st.acquire_lock.lock().await;

    let mut tx = st.pool.begin().await.map_err(internal)?;

    // Lazy reaper: drop anything already past its heartbeat or hard ceiling, so
    // the conflict check below only ever sees live leases.
    sqlx::query("DELETE FROM leases WHERE expiry_ms <= ?1 OR hard_expiry_ms <= ?1")
        .bind(now_ms)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;

    // Conflict check, in canonical order.
    let mut conflicts: Vec<CanonicalPath> = Vec::new();
    let mut holder: Option<Principal> = None;
    for path in &paths {
        let row = sqlx::query("SELECT principal FROM leases WHERE path = ?1 LIMIT 1")
            .bind(path.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(internal)?;
        if let Some(row) = row {
            let p: String = row.get("principal");
            holder.get_or_insert_with(|| Principal::new_unchecked(p));
            conflicts.push(path.clone());
        }
    }
    if !conflicts.is_empty() {
        // Nothing was inserted; rolling the (read-only) tx back is tidy.
        tx.rollback().await.map_err(internal)?;
        return Err(ChaprError::LeaseHeld {
            holder: holder.expect("a conflict implies a holder"),
            paths: conflicts,
        });
    }

    // Grant: one row per path, sharing the lease id.
    let lease_id = LeaseId::new_unchecked(format!("lease-{}", Uuid::new_v4()));
    let purpose_str = purpose_to_str(purpose);
    for path in &paths {
        sqlx::query(
            "INSERT INTO leases
               (lease_id, path, principal, session_id, purpose,
                granted_at_ms, ttl_s, renewed_at_ms, hard_expiry_ms, expiry_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?6, ?8, ?9)",
        )
        .bind(lease_id.as_str())
        .bind(path.as_str())
        .bind(principal.as_str())
        .bind(session_id.as_str())
        .bind(purpose_str)
        .bind(now_ms)
        .bind(HEARTBEAT_TTL_S)
        .bind(hard_expiry_ms)
        .bind(expiry_ms)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
    }

    tx.commit().await.map_err(internal)?;

    // lease_grant audit (concept §13.1). After commit, still under the lock.
    audit::record(
        st,
        &principal,
        &session_id,
        &paths[0],
        AuditKind::LeaseGrant,
        None,
        None,
        &format!("lease {lease_id} granted over {} path(s)", paths.len()),
    )
    .await?;

    Ok(LeaseAcquireResponse {
        lease_id,
        ttl_s: HEARTBEAT_TTL_S,
    })
}

/// Release a lease by id, removing every path row it covers. Returns
/// [`ChaprError::LeaseNotFound`] if no rows matched — the lease was never
/// granted, already released, or already reaped after expiry.
pub async fn release(st: &AppState, lease_id: &LeaseId) -> Result<(), ChaprError> {
    let _guard = st.acquire_lock.lock().await;

    let affected = delete_lease(&st.pool, lease_id).await?;

    if affected == 0 {
        Err(ChaprError::LeaseNotFound {
            lease_id: lease_id.clone(),
        })
    } else {
        Ok(())
    }
}

/// Renew a lease's heartbeat (concept §6.4, §9).
///
/// Pushes `expiry` out to `now + ttl`, capped at the lease's immovable
/// `hard_expiry` (the 20-minute ceiling that defends against an agent stuck
/// renewing forever). Failure modes, each of which also removes the dead lease
/// so it stops blocking others:
/// - unknown / already-reaped id → [`ChaprError::LeaseNotFound`];
/// - already past the hard ceiling → force-expire, [`ChaprError::MaxLeaseLifetimeExceeded`];
/// - already past its heartbeat expiry → [`ChaprError::LeaseExpired`] (the
///   holder must re-read before retrying, concept §15).
pub async fn renew(st: &AppState, lease_id: &LeaseId) -> Result<LeaseRenewResponse, ChaprError> {
    let _guard = st.acquire_lock.lock().await;

    let now = Utc::now();
    let now_ms = now.timestamp_millis();

    let row = sqlx::query(
        "SELECT hard_expiry_ms, expiry_ms, ttl_s, principal, session_id, path
         FROM leases WHERE lease_id = ?1 LIMIT 1",
    )
    .bind(lease_id.as_str())
    .fetch_optional(&st.pool)
    .await
    .map_err(internal)?;

    let Some(row) = row else {
        return Err(ChaprError::LeaseNotFound {
            lease_id: lease_id.clone(),
        });
    };
    let hard_expiry_ms: i64 = row.get("hard_expiry_ms");
    let expiry_ms: i64 = row.get("expiry_ms");
    let ttl_s: i64 = row.get("ttl_s");
    let principal = Principal::new_unchecked(row.get::<String, _>("principal"));
    let session_id = SessionId::new_unchecked(row.get::<String, _>("session_id"));
    let path = CanonicalPath::new_unchecked(row.get::<String, _>("path"));

    if now_ms >= hard_expiry_ms {
        delete_lease(&st.pool, lease_id).await?;
        audit_expire(st, &principal, &session_id, &path, lease_id, "hard lifetime ceiling").await?;
        return Err(ChaprError::MaxLeaseLifetimeExceeded {
            lease_id: lease_id.clone(),
            hard_expiry: ms_to_dt(hard_expiry_ms),
        });
    }
    if now_ms >= expiry_ms {
        delete_lease(&st.pool, lease_id).await?;
        audit_expire(st, &principal, &session_id, &path, lease_id, "heartbeat lapsed").await?;
        return Err(ChaprError::LeaseExpired {
            lease_id: lease_id.clone(),
        });
    }

    // Push expiry out, but never past the hard ceiling.
    let new_expiry_ms = (now_ms + ttl_s * 1000).min(hard_expiry_ms);
    sqlx::query("UPDATE leases SET renewed_at_ms = ?2, expiry_ms = ?3 WHERE lease_id = ?1")
        .bind(lease_id.as_str())
        .bind(now_ms)
        .bind(new_expiry_ms)
        .execute(&st.pool)
        .await
        .map_err(internal)?;

    audit::record(
        st,
        &principal,
        &session_id,
        &path,
        AuditKind::LeaseRenew,
        None,
        None,
        &format!("lease {lease_id} renewed"),
    )
    .await?;

    Ok(LeaseRenewResponse {
        renewed_until: ms_to_dt(new_expiry_ms),
    })
}

/// Emit a `lease_expire` audit event (shared by renew's force-expiry branches
/// and the reaper).
pub(crate) async fn audit_expire(
    st: &AppState,
    principal: &Principal,
    session_id: &SessionId,
    path: &CanonicalPath,
    lease_id: &LeaseId,
    reason: &str,
) -> Result<(), ChaprError> {
    audit::record(
        st,
        principal,
        session_id,
        path,
        AuditKind::LeaseExpire,
        None,
        None,
        &format!("lease {lease_id} expired: {reason}"),
    )
    .await
    .map(|_| ())
}

/// Return a [`LeaseRef`] for the live lease over `path`, or `None` if the path
/// is unleased. Used by `coord.resolve` to fill `lease_state` (concept §8.1).
/// A pure read — no coarse lock, since it does not mutate coord state.
pub async fn lease_ref_for_path(
    pool: &SqlitePool,
    path: &CanonicalPath,
    now_ms: i64,
) -> Result<Option<LeaseRef>, ChaprError> {
    let row = sqlx::query(
        "SELECT lease_id, principal, hard_expiry_ms FROM leases
         WHERE path = ?1 AND expiry_ms > ?2 AND hard_expiry_ms > ?2 LIMIT 1",
    )
    .bind(path.as_str())
    .bind(now_ms)
    .fetch_optional(pool)
    .await
    .map_err(internal)?;

    Ok(row.map(|r| LeaseRef {
        lease_id: LeaseId::new_unchecked(r.get::<String, _>("lease_id")),
        principal: Principal::new_unchecked(r.get::<String, _>("principal")),
        hard_expiry: ms_to_dt(r.get::<i64, _>("hard_expiry_ms")),
    }))
}

/// Delete every path row of a lease. Shared by `release` and the force-expiry
/// paths of `renew`.
async fn delete_lease(pool: &SqlitePool, lease_id: &LeaseId) -> Result<u64, ChaprError> {
    Ok(sqlx::query("DELETE FROM leases WHERE lease_id = ?1")
        .bind(lease_id.as_str())
        .execute(pool)
        .await
        .map_err(internal)?
        .rows_affected())
}

/// Epoch-millis → UTC datetime. Values in the DB were all produced from
/// `DateTime<Utc>::timestamp_millis`, so the conversion back cannot realistically
/// fail; fall back to the epoch rather than panic if a row is somehow corrupt.
fn ms_to_dt(ms: i64) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp_millis(ms).unwrap_or_default()
}

/// Serialise a [`LeasePurpose`] to its stored string, matching the proto
/// serde `snake_case` form so the column is grep-able against audit output.
fn purpose_to_str(p: LeasePurpose) -> &'static str {
    match p {
        LeasePurpose::Write => "write",
        LeasePurpose::Create => "create",
        LeasePurpose::Delete => "delete",
        LeasePurpose::Move => "move",
        LeasePurpose::Restore => "restore",
    }
}

/// Collapse an unexpected `sqlx` error into the protocol's internal-error
/// variant. A DB failure here is a bug or an operational fault, not an expected
/// branch of the lease state machine.
/// A held lease as the admin view shows it (E-024 read side).
///
/// Coord-local rather than a proto type: the consumer is the admin page's
/// JavaScript, and the precedent (D-022) is to defer proto promotion until a
/// second *Rust* consumer exists. It is deliberately **not** proto's
/// [`chapr_proto::LeaseRecord`] either — that has no `session_id`, which is
/// exactly the column an administrator needs when several agents share one user's
/// identity, and it is already in use on the endpoint's path.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct LeaseView {
    pub lease_id: String,
    /// Every path this lease covers. One lease over an all-or-none set is one
    /// row here, not N — the set is the unit that was granted.
    pub paths: Vec<String>,
    pub principal: String,
    pub session_id: String,
    pub purpose: String,
    pub granted_at: DateTime<Utc>,
    pub renewed_at: DateTime<Utc>,
    pub expiry: DateTime<Utc>,
    pub hard_expiry: DateTime<Utc>,
    /// Seconds until the heartbeat expiry; negative once past it.
    pub renews_in_s: i64,
    /// How long it has been held, in seconds.
    pub held_for_s: i64,
    pub state: LeaseHealth,
}

/// How healthy a held lease looks, for the admin view's status chip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseHealth {
    /// Renewing normally.
    Held,
    /// Approaching its heartbeat expiry — one more renewal is due imminently.
    Expiring,
    /// **Renewal has stopped.** Past a full TTL since the last renewal but not
    /// yet reaped, which means nobody is heartbeating it any more. Before the
    /// I-008 fix this was the visible symptom of a single failed renewal
    /// permanently ending renewal for that lease.
    Stale,
}

/// Classify a lease from its timestamps. Pure, so the thresholds are testable
/// without a database.
pub fn classify(now_ms: i64, renewed_at_ms: i64, expiry_ms: i64, ttl_s: u32) -> LeaseHealth {
    let ttl_ms = ttl_s as i64 * 1000;
    // Renewal runs at TTL/3, so missing a whole TTL means it is not running.
    if now_ms - renewed_at_ms > ttl_ms {
        return LeaseHealth::Stale;
    }
    // Inside the last third of the heartbeat: a renewal is due about now.
    if expiry_ms - now_ms < ttl_ms / 3 {
        return LeaseHealth::Expiring;
    }
    LeaseHealth::Held
}

/// Every currently-held lease, newest first, grouped by `lease_id`.
///
/// Expired rows are filtered by time rather than deleted: this is a read, and a
/// read must not mutate coordination state. The lazy sweep in [`acquire`] and the
/// background reaper are what remove them.
pub async fn list_held(pool: &SqlitePool, limit: i64) -> Result<Vec<LeaseView>, ChaprError> {
    let now = Utc::now();
    let now_ms = now.timestamp_millis();

    let rows = sqlx::query(
        "SELECT lease_id, path, principal, session_id, purpose,
                granted_at_ms, ttl_s, renewed_at_ms, hard_expiry_ms, expiry_ms
           FROM leases
          WHERE expiry_ms > ?1 AND hard_expiry_ms > ?1
          ORDER BY granted_at_ms DESC, lease_id, path",
    )
    .bind(now_ms)
    .fetch_all(pool)
    .await
    .map_err(internal)?;

    // Fold the per-path rows back into one entry per lease, preserving the
    // ordering the query established.
    let mut out: Vec<LeaseView> = Vec::new();
    for row in rows {
        let lease_id: String = row.get("lease_id");
        let path: String = row.get("path");
        if let Some(existing) = out.iter_mut().find(|v| v.lease_id == lease_id) {
            existing.paths.push(path);
            continue;
        }
        if out.len() >= limit.max(0) as usize {
            continue;
        }
        let granted_at_ms: i64 = row.get("granted_at_ms");
        let renewed_at_ms: i64 = row.get("renewed_at_ms");
        let expiry_ms: i64 = row.get("expiry_ms");
        let hard_expiry_ms: i64 = row.get("hard_expiry_ms");
        let ttl_s: u32 = row.get::<i64, _>("ttl_s") as u32;
        out.push(LeaseView {
            lease_id,
            paths: vec![path],
            principal: row.get("principal"),
            session_id: row.get("session_id"),
            purpose: row.get("purpose"),
            granted_at: ms_to_dt(granted_at_ms),
            renewed_at: ms_to_dt(renewed_at_ms),
            expiry: ms_to_dt(expiry_ms),
            hard_expiry: ms_to_dt(hard_expiry_ms),
            renews_in_s: (expiry_ms - now_ms) / 1000,
            held_for_s: (now_ms - granted_at_ms) / 1000,
            state: classify(now_ms, renewed_at_ms, expiry_ms, ttl_s),
        });
    }
    Ok(out)
}

fn internal(e: sqlx::Error) -> ChaprError {
    ChaprError::Internal {
        message: format!("coord db error: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn p(s: &str) -> CanonicalPath {
        CanonicalPath::new_unchecked(s)
    }
    fn who(s: &str) -> Principal {
        Principal::new_unchecked(s)
    }
    fn sess() -> SessionId {
        SessionId::new_unchecked("sess-t")
    }

    #[test]
    fn classify_separates_healthy_from_stopped_renewal() {
        let ttl = HEARTBEAT_TTL_S; // 90 s
        let now = 1_000_000_000_i64;
        let s = |renewed_ago_s: i64, expires_in_s: i64| {
            classify(now, now - renewed_ago_s * 1000, now + expires_in_s * 1000, ttl)
        };
        // Renewed recently, plenty of heartbeat left.
        assert_eq!(s(5, 85), LeaseHealth::Held);
        // Inside the last third of the TTL: a renewal is due about now. Normal.
        assert_eq!(s(70, 20), LeaseHealth::Expiring);
        // Past a whole TTL since the last renewal — the renewer is not running.
        // This is I-008's visible symptom, and it must not be reported as merely
        // "expiring", because the two call for different responses.
        assert_eq!(s(120, 40), LeaseHealth::Stale);
    }

    #[tokio::test]
    async fn list_held_groups_a_set_into_one_entry() {
        // A lease over an all-or-none set is one grant, so it is one row in the
        // admin view rather than N rows that look like N leases.
        let st = AppState::new(db::test_pool().await);
        acquire(
            &st,
            who("CONTOSO\\a"),
            sess(),
            LeasePurpose::Move,
            vec![p("\\\\srv\\share\\b.md"), p("\\\\srv\\share\\a.md")],
        )
        .await
        .unwrap();

        let held = list_held(&st.pool, 100).await.unwrap();
        assert_eq!(held.len(), 1, "one lease, not one per path");
        assert_eq!(held[0].paths.len(), 2);
        assert_eq!(held[0].principal, "CONTOSO\\a");
        assert_eq!(held[0].session_id, "sess-t");
        assert_eq!(held[0].state, LeaseHealth::Held);
        assert!(held[0].renews_in_s > 0, "a fresh lease has heartbeat left");
        assert!(held[0].held_for_s >= 0);
    }

    #[tokio::test]
    async fn list_held_hides_an_expired_lease_without_deleting_it() {
        // A read must not mutate coordination state; the lazy sweep in `acquire`
        // and the background reaper own removal.
        let st = AppState::new(db::test_pool().await);
        let lease = acquire(
            &st,
            who("CONTOSO\\a"),
            sess(),
            LeasePurpose::Write,
            vec![p("\\\\srv\\share\\a.md")],
        )
        .await
        .unwrap();
        sqlx::query("UPDATE leases SET expiry_ms = 1 WHERE lease_id = ?1")
            .bind(lease.lease_id.as_str())
            .execute(&st.pool)
            .await
            .unwrap();

        assert!(list_held(&st.pool, 100).await.unwrap().is_empty());
        let still_there: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM leases WHERE lease_id = ?1")
                .bind(lease.lease_id.as_str())
                .fetch_one(&st.pool)
                .await
                .unwrap();
        assert_eq!(still_there, 1, "the read must not have swept the row");
    }

    #[tokio::test]
    async fn acquire_then_release_roundtrips() {
        let st = AppState::new(db::test_pool().await);
        let resp = acquire(&st, who("A"), sess(), LeasePurpose::Write, vec![p("\\\\srv\\share\\a.md")])
            .await
            .unwrap();
        assert_eq!(resp.ttl_s, HEARTBEAT_TTL_S);
        release(&st, &resp.lease_id).await.unwrap();
        // Released — a fresh acquire on the same path now succeeds.
        acquire(&st, who("B"), sess(), LeasePurpose::Write, vec![p("\\\\srv\\share\\a.md")])
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn second_holder_is_refused_with_lease_held() {
        let st = AppState::new(db::test_pool().await);
        acquire(&st, who("A"), sess(), LeasePurpose::Write, vec![p("\\\\srv\\share\\a.md")])
            .await
            .unwrap();
        let err = acquire(&st, who("B"), sess(), LeasePurpose::Write, vec![p("\\\\srv\\share\\a.md")])
            .await
            .unwrap_err();
        match err {
            ChaprError::LeaseHeld { holder, paths } => {
                assert_eq!(holder, who("A"));
                assert_eq!(paths, vec![p("\\\\srv\\share\\a.md")]);
            }
            other => panic!("expected LeaseHeld, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn acquisition_is_all_or_none() {
        let st = AppState::new(db::test_pool().await);
        // A holds b.md.
        acquire(&st, who("A"), sess(), LeasePurpose::Write, vec![p("\\\\srv\\share\\b.md")])
            .await
            .unwrap();
        // B wants {a.md, b.md} — must get neither because b.md is held.
        let err = acquire(
            &st,
            who("B"),
            sess(),
            LeasePurpose::Write,
            vec![p("\\\\srv\\share\\a.md"), p("\\\\srv\\share\\b.md")],
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ChaprError::LeaseHeld { .. }));
        // a.md must be free (nothing was granted to B), so A can take it.
        acquire(&st, who("A"), sess(), LeasePurpose::Write, vec![p("\\\\srv\\share\\a.md")])
            .await
            .expect("a.md was never granted to B");
    }

    #[tokio::test]
    async fn release_unknown_lease_is_not_found() {
        let st = AppState::new(db::test_pool().await);
        let err = release(&st, &LeaseId::new_unchecked("lease-does-not-exist"))
            .await
            .unwrap_err();
        assert!(matches!(err, ChaprError::LeaseNotFound { .. }));
    }

    #[tokio::test]
    async fn empty_path_set_is_rejected() {
        let st = AppState::new(db::test_pool().await);
        let err = acquire(&st, who("A"), sess(), LeasePurpose::Write, vec![])
            .await
            .unwrap_err();
        assert!(matches!(err, ChaprError::InvalidPath { .. }));
    }

    #[tokio::test]
    async fn duplicate_paths_in_one_request_are_deduped() {
        let st = AppState::new(db::test_pool().await);
        // Same path twice must not violate the (lease_id, path) primary key.
        acquire(
            &st,
            who("A"),
            sess(),
            LeasePurpose::Write,
            vec![p("\\\\srv\\share\\a.md"), p("\\\\srv\\share\\a.md")],
        )
        .await
        .expect("duplicate paths should dedupe, not error");
    }

    #[tokio::test]
    async fn renew_extends_expiry_and_keeps_the_lease_alive() {
        let st = AppState::new(db::test_pool().await);
        let acq = acquire(&st, who("A"), sess(), LeasePurpose::Write, vec![p("\\\\srv\\share\\a.md")])
            .await
            .unwrap();
        let renewed = renew(&st, &acq.lease_id).await.unwrap();
        // New expiry is in the future, roughly now + TTL.
        assert!(renewed.renewed_until > Utc::now());
        // Still held: B cannot take the path.
        let err = acquire(&st, who("B"), sess(), LeasePurpose::Write, vec![p("\\\\srv\\share\\a.md")])
            .await
            .unwrap_err();
        assert!(matches!(err, ChaprError::LeaseHeld { .. }));
    }

    #[tokio::test]
    async fn renew_unknown_lease_is_not_found() {
        let st = AppState::new(db::test_pool().await);
        let err = renew(&st, &LeaseId::new_unchecked("lease-nope"))
            .await
            .unwrap_err();
        assert!(matches!(err, ChaprError::LeaseNotFound { .. }));
    }

    #[tokio::test]
    async fn renew_past_hard_ceiling_force_expires() {
        let st = AppState::new(db::test_pool().await);
        // A lease still within its heartbeat but already past its hard ceiling.
        let now = Utc::now().timestamp_millis();
        let past = (Utc::now() - Duration::seconds(1)).timestamp_millis();
        let future = (Utc::now() + Duration::seconds(90)).timestamp_millis();
        sqlx::query(
            "INSERT INTO leases
               (lease_id, path, principal, session_id, purpose,
                granted_at_ms, ttl_s, renewed_at_ms, hard_expiry_ms, expiry_ms)
             VALUES ('capped', ?1, 'A', 's', 'write', ?2, 90, ?2, ?3, ?4)",
        )
        .bind("\\\\srv\\share\\a.md")
        .bind(now)
        .bind(past) // hard_expiry in the past
        .bind(future) // heartbeat still valid
        .execute(&st.pool)
        .await
        .unwrap();

        let err = renew(&st, &LeaseId::new_unchecked("capped")).await.unwrap_err();
        assert!(matches!(err, ChaprError::MaxLeaseLifetimeExceeded { .. }));
        // Force-expired: the path is now free.
        acquire(&st, who("B"), sess(), LeasePurpose::Write, vec![p("\\\\srv\\share\\a.md")])
            .await
            .expect("force-expired lease must free the path");
    }

    #[tokio::test]
    async fn renew_of_lapsed_heartbeat_reports_expired() {
        let st = AppState::new(db::test_pool().await);
        let now = Utc::now().timestamp_millis();
        let past = (Utc::now() - Duration::seconds(1)).timestamp_millis();
        let future = (Utc::now() + Duration::seconds(1200)).timestamp_millis();
        sqlx::query(
            "INSERT INTO leases
               (lease_id, path, principal, session_id, purpose,
                granted_at_ms, ttl_s, renewed_at_ms, hard_expiry_ms, expiry_ms)
             VALUES ('lapsed', ?1, 'A', 's', 'write', ?2, 90, ?2, ?3, ?4)",
        )
        .bind("\\\\srv\\share\\a.md")
        .bind(now)
        .bind(future) // hard ceiling still ahead
        .bind(past) // but heartbeat already lapsed
        .execute(&st.pool)
        .await
        .unwrap();

        let err = renew(&st, &LeaseId::new_unchecked("lapsed")).await.unwrap_err();
        assert!(matches!(err, ChaprError::LeaseExpired { .. }));
    }

    #[tokio::test]
    async fn expired_lease_is_swept_and_does_not_block() {
        let st = AppState::new(db::test_pool().await);
        // Insert a lease that is already expired (expiry_ms in the past).
        let past = (Utc::now() - Duration::seconds(1)).timestamp_millis();
        sqlx::query(
            "INSERT INTO leases
               (lease_id, path, principal, session_id, purpose,
                granted_at_ms, ttl_s, renewed_at_ms, hard_expiry_ms, expiry_ms)
             VALUES ('stale', ?1, 'A', 's', 'write', ?2, 90, ?2, ?2, ?3)",
        )
        .bind("\\\\srv\\share\\a.md")
        .bind(past)
        .bind(past)
        .execute(&st.pool)
        .await
        .unwrap();

        // B should acquire cleanly — the stale lease is swept first.
        acquire(&st, who("B"), sess(), LeasePurpose::Write, vec![p("\\\\srv\\share\\a.md")])
            .await
            .expect("expired lease must not block");
    }
}
