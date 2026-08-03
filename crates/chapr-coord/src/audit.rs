//! The audit log (concept §4.2, §5.2, §13.1) — an append-only, principal-stamped
//! record of every coordination-significant action.
//!
//! The spec calls the audit trail **a primary deliverable, not a byproduct**.
//! This subsystem is the durable home for that trail. Each event is stamped with
//! the acting AD principal and session, keyed by canonical path, and carries the
//! version transition where meaningful.
//!
//! ## Scope this slice
//!
//! The table, the append ([`record`]), and a governance/test query ([`query`]).
//! Wiring the full set of event kinds (`lease_grant`, `write_commit`, …) into
//! every coord operation is a follow-up; the one kind wired now is
//! `crash_recover`, emitted by [`crate::journal::recover`] so the read path's
//! Dangling branch is spec-complete (concept §8.1).
//!
//! Append is lock-free — a blind insert with a unique `event_id`, no
//! read-modify-write to race.

use crate::state::AppState;
use chapr_proto::{
    AuditEvent, AuditKind, CanonicalPath, ChaprError, EventId, Principal, SessionId, VersionToken,
};
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

/// Append an audit event. Coord assigns the `event_id` and timestamp. Returns
/// the recorded event.
#[allow(clippy::too_many_arguments)]
pub async fn record(
    st: &AppState,
    principal: &Principal,
    session_id: &SessionId,
    path: &CanonicalPath,
    kind: AuditKind,
    from_version: Option<&VersionToken>,
    to_version: Option<&VersionToken>,
    detail: &str,
) -> Result<AuditEvent, ChaprError> {
    let event_id = EventId::new_unchecked(format!("evt-{}", Uuid::new_v4()));
    let now = Utc::now();

    sqlx::query(
        "INSERT INTO audit_log
           (event_id, timestamp_ms, principal, session_id, canonical_path,
            kind, from_version, to_version, detail)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )
    .bind(event_id.as_str())
    .bind(now.timestamp_millis())
    .bind(principal.as_str())
    .bind(session_id.as_str())
    .bind(path.as_str())
    .bind(kind_str(kind))
    .bind(from_version.map(|v| v.as_str()))
    .bind(to_version.map(|v| v.as_str()))
    .bind(detail)
    .execute(&st.pool)
    .await
    .map_err(internal)?;

    Ok(AuditEvent {
        event_id,
        timestamp: now,
        principal: principal.clone(),
        session_id: session_id.clone(),
        canonical_path: path.clone(),
        kind,
        from_version: from_version.cloned(),
        to_version: to_version.cloned(),
        detail: detail.to_string(),
    })
}

/// Read the audit events for a path, newest first. The governance read
/// ("who changed this and when") and the test hook.
pub async fn query(pool: &SqlitePool, path: &CanonicalPath) -> Result<Vec<AuditEvent>, ChaprError> {
    let rows = sqlx::query(
        "SELECT event_id, timestamp_ms, principal, session_id, canonical_path,
                kind, from_version, to_version, detail
         FROM audit_log WHERE canonical_path = ?1 ORDER BY id DESC",
    )
    .bind(path.as_str())
    .fetch_all(pool)
    .await
    .map_err(internal)?;

    let mut events = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(kind) = kind_from_str(&row.get::<String, _>("kind")) else {
            continue; // unknown kind ⇒ corrupt row; skip
        };
        let from_version = match row.get::<Option<String>, _>("from_version") {
            Some(h) => VersionToken::from_hex(h),
            None => None,
        };
        let to_version = match row.get::<Option<String>, _>("to_version") {
            Some(h) => VersionToken::from_hex(h),
            None => None,
        };
        events.push(AuditEvent {
            event_id: EventId::new_unchecked(row.get::<String, _>("event_id")),
            timestamp: DateTime::<Utc>::from_timestamp_millis(row.get::<i64, _>("timestamp_ms"))
                .unwrap_or_default(),
            principal: Principal::new_unchecked(row.get::<String, _>("principal")),
            session_id: SessionId::new_unchecked(row.get::<String, _>("session_id")),
            canonical_path: CanonicalPath::new_unchecked(row.get::<String, _>("canonical_path")),
            kind,
            from_version,
            to_version,
            detail: row.get::<String, _>("detail"),
        });
    }
    Ok(events)
}

fn kind_str(k: AuditKind) -> &'static str {
    match k {
        AuditKind::LeaseGrant => "lease_grant",
        AuditKind::LeaseRenew => "lease_renew",
        AuditKind::LeaseExpire => "lease_expire",
        AuditKind::WriteCommit => "write_commit",
        AuditKind::Restore => "restore",
        AuditKind::ConflictOpen => "conflict_open",
        AuditKind::ConflictResolve => "conflict_resolve",
        AuditKind::CrashRecover => "crash_recover",
    }
}

fn kind_from_str(s: &str) -> Option<AuditKind> {
    Some(match s {
        "lease_grant" => AuditKind::LeaseGrant,
        "lease_renew" => AuditKind::LeaseRenew,
        "lease_expire" => AuditKind::LeaseExpire,
        "write_commit" => AuditKind::WriteCommit,
        "restore" => AuditKind::Restore,
        "conflict_open" => AuditKind::ConflictOpen,
        "conflict_resolve" => AuditKind::ConflictResolve,
        "crash_recover" => AuditKind::CrashRecover,
        _ => return None,
    })
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

    fn path() -> CanonicalPath {
        CanonicalPath::new_unchecked("\\\\srv\\share\\a.md")
    }

    #[tokio::test]
    async fn record_then_query_roundtrips() {
        let st = AppState::new(db::test_pool().await);
        let from = VersionToken::hash(b"old");
        let ev = record(
            &st,
            &Principal::new_unchecked("CONTOSO\\jsmith"),
            &SessionId::new_unchecked("sess-1"),
            &path(),
            AuditKind::CrashRecover,
            Some(&from),
            None,
            "recovered pre-image",
        )
        .await
        .unwrap();
        assert!(ev.event_id.as_str().starts_with("evt-"));

        let events = query(&st.pool, &path()).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, AuditKind::CrashRecover);
        assert_eq!(events[0].from_version, Some(from));
        assert_eq!(events[0].detail, "recovered pre-image");
    }

    #[tokio::test]
    async fn query_is_newest_first() {
        let st = AppState::new(db::test_pool().await);
        let who = Principal::new_unchecked("A");
        let sess = SessionId::new_unchecked("s");
        record(&st, &who, &sess, &path(), AuditKind::LeaseGrant, None, None, "one")
            .await
            .unwrap();
        record(&st, &who, &sess, &path(), AuditKind::LeaseExpire, None, None, "two")
            .await
            .unwrap();
        let events = query(&st.pool, &path()).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].detail, "two");
        assert_eq!(events[1].detail, "one");
    }
}
