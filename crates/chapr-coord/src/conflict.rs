// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! The conflict registry (concept §11) — the source of truth for open conflict
//! sidecars.
//!
//! On a CAS conflict the write path parks the losing agent's bytes in a
//! uniquely-named `F.conflict-{user}-{ts}` sidecar (never losing either party's
//! bytes, never fake-merging Office binaries) and [`register`]s it here.
//! Conflicts are then surfaced two ways: on-touch, via `open_conflicts` on
//! `coord.resolve` ([`count_open`]), and on demand via [`list`]
//! (`chapr.conflicts`). Closing is either explicit ([`resolve`], audited
//! `conflict_resolve`) or — once the change-watcher exists — inferred from the
//! sidecar being deleted (`inferred_from_deletion`, deferred with the watcher).
//!
//! Append/update are lock-free: `register` is a blind insert with a unique id;
//! `resolve` guards on `state = 'open'` and checks `rows_affected`, so a
//! double-resolve cannot double-fire.

use crate::state::AppState;
use crate::audit;
use chapr_proto::{
    AuditKind, CanonicalPath, ChaprError, ConflictEntry, ConflictId, ConflictResolution,
    ConflictState, Principal, SessionId,
};
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

/// Register a new open conflict and audit `conflict_open`.
pub async fn register(
    st: &AppState,
    base_path: &CanonicalPath,
    sidecar_path: &CanonicalPath,
    losing_principal: &Principal,
    session_id: &SessionId,
) -> Result<ConflictEntry, ChaprError> {
    let conflict_id = ConflictId::new_unchecked(format!("conflict-{}", Uuid::new_v4()));
    let now = Utc::now();

    sqlx::query(
        "INSERT INTO conflicts
           (conflict_id, base_path, sidecar_path, losing_principal, created_at_ms, state, resolution)
         VALUES (?1, ?2, ?3, ?4, ?5, 'open', NULL)",
    )
    .bind(conflict_id.as_str())
    .bind(base_path.as_str())
    .bind(sidecar_path.as_str())
    .bind(losing_principal.as_str())
    .bind(now.timestamp_millis())
    .execute(&st.pool)
    .await
    .map_err(internal)?;

    audit::record(
        st,
        losing_principal,
        session_id,
        base_path,
        AuditKind::ConflictOpen,
        None,
        None,
        &format!("conflict sidecar {sidecar_path}"),
    )
    .await?;

    Ok(ConflictEntry {
        conflict_id,
        base_path: base_path.clone(),
        sidecar_path: sidecar_path.clone(),
        losing_principal: losing_principal.clone(),
        created_at: now,
        state: ConflictState::Open,
        resolution: None,
    })
}

/// Count open conflicts against a specific file (surface-on-touch for
/// `coord.resolve`).
pub async fn count_open(pool: &SqlitePool, base_path: &CanonicalPath) -> Result<u32, ChaprError> {
    let n: i64 = sqlx::query("SELECT COUNT(*) AS n FROM conflicts WHERE base_path = ?1 AND state = 'open'")
        .bind(base_path.as_str())
        .fetch_one(pool)
        .await
        .map_err(internal)?
        .get("n");
    Ok(n as u32)
}

/// Count every open conflict, for the admin overview.
///
/// Distinct from [`count_open`], which answers "does *this file* have conflicts"
/// on every `coord.resolve` — the hot path. This one is fleet-wide.
pub async fn count_open_all(pool: &SqlitePool) -> Result<i64, ChaprError> {
    let n: i64 = sqlx::query("SELECT COUNT(*) AS n FROM conflicts WHERE state = 'open'")
        .fetch_one(pool)
        .await
        .map_err(internal)?
        .get("n");
    Ok(n)
}

/// List open conflicts under a canonical path prefix (the governance artifact,
/// `chapr.conflicts`). Newest first.
pub async fn list(pool: &SqlitePool, scope: &CanonicalPath) -> Result<Vec<ConflictEntry>, ChaprError> {
    let prefix = format!("{}%", scope.as_str());
    let rows = sqlx::query(
        "SELECT conflict_id, base_path, sidecar_path, losing_principal, created_at_ms, state, resolution
         FROM conflicts WHERE base_path LIKE ?1 AND state = 'open' ORDER BY created_at_ms DESC",
    )
    .bind(prefix)
    .fetch_all(pool)
    .await
    .map_err(internal)?;

    rows.into_iter().map(row_to_entry).collect()
}

/// Explicitly resolve a conflict and audit `conflict_resolve`. Errors with
/// [`ChaprError::ConflictNotFound`] if the id is unknown or already resolved.
pub async fn resolve(
    st: &AppState,
    conflict_id: &ConflictId,
    resolution: ConflictResolution,
    principal: &Principal,
    session_id: &SessionId,
) -> Result<ConflictEntry, ChaprError> {
    let affected = sqlx::query(
        "UPDATE conflicts SET state = 'resolved', resolution = ?2
         WHERE conflict_id = ?1 AND state = 'open'",
    )
    .bind(conflict_id.as_str())
    .bind(resolution_str(resolution))
    .execute(&st.pool)
    .await
    .map_err(internal)?
    .rows_affected();

    if affected == 0 {
        return Err(ChaprError::ConflictNotFound {
            conflict_id: conflict_id.clone(),
        });
    }

    let entry = fetch(&st.pool, conflict_id).await?;
    audit::record(
        st,
        principal,
        session_id,
        &entry.base_path,
        AuditKind::ConflictResolve,
        None,
        None,
        &format!("resolved {} as {:?}", conflict_id, resolution),
    )
    .await?;
    Ok(entry)
}

async fn fetch(pool: &SqlitePool, conflict_id: &ConflictId) -> Result<ConflictEntry, ChaprError> {
    let row = sqlx::query(
        "SELECT conflict_id, base_path, sidecar_path, losing_principal, created_at_ms, state, resolution
         FROM conflicts WHERE conflict_id = ?1",
    )
    .bind(conflict_id.as_str())
    .fetch_optional(pool)
    .await
    .map_err(internal)?
    .ok_or_else(|| ChaprError::ConflictNotFound {
        conflict_id: conflict_id.clone(),
    })?;
    row_to_entry(row)
}

fn row_to_entry(row: sqlx::sqlite::SqliteRow) -> Result<ConflictEntry, ChaprError> {
    let resolution = match row.get::<Option<String>, _>("resolution") {
        Some(s) => Some(resolution_from_str(&s).ok_or_else(|| ChaprError::Internal {
            message: format!("corrupt conflict resolution: {s}"),
        })?),
        None => None,
    };
    Ok(ConflictEntry {
        conflict_id: ConflictId::new_unchecked(row.get::<String, _>("conflict_id")),
        base_path: CanonicalPath::new_unchecked(row.get::<String, _>("base_path")),
        sidecar_path: CanonicalPath::new_unchecked(row.get::<String, _>("sidecar_path")),
        losing_principal: Principal::new_unchecked(row.get::<String, _>("losing_principal")),
        created_at: DateTime::<Utc>::from_timestamp_millis(row.get::<i64, _>("created_at_ms"))
            .unwrap_or_default(),
        state: state_from_str(&row.get::<String, _>("state")),
        resolution,
    })
}

fn resolution_str(r: ConflictResolution) -> &'static str {
    match r {
        ConflictResolution::KeptMine => "kept_mine",
        ConflictResolution::KeptTheirs => "kept_theirs",
        ConflictResolution::Merged => "merged",
        ConflictResolution::Discarded => "discarded",
        ConflictResolution::InferredFromDeletion => "inferred_from_deletion",
    }
}

fn resolution_from_str(s: &str) -> Option<ConflictResolution> {
    Some(match s {
        "kept_mine" => ConflictResolution::KeptMine,
        "kept_theirs" => ConflictResolution::KeptTheirs,
        "merged" => ConflictResolution::Merged,
        "discarded" => ConflictResolution::Discarded,
        "inferred_from_deletion" => ConflictResolution::InferredFromDeletion,
        _ => return None,
    })
}

fn state_from_str(s: &str) -> ConflictState {
    match s {
        "resolved" => ConflictState::Resolved,
        _ => ConflictState::Open,
    }
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

    fn base() -> CanonicalPath {
        CanonicalPath::new_unchecked("\\\\srv\\share\\q3.xlsx")
    }
    fn sidecar() -> CanonicalPath {
        CanonicalPath::new_unchecked("\\\\srv\\share\\q3.conflict-jsmith-20260721.xlsx")
    }
    fn who() -> Principal {
        Principal::new_unchecked("CONTOSO\\jsmith")
    }
    fn sess() -> SessionId {
        SessionId::new_unchecked("sess-1")
    }

    #[tokio::test]
    async fn register_surfaces_and_lists() {
        let st = AppState::new(db::test_pool().await);
        let entry = register(&st, &base(), &sidecar(), &who(), &sess()).await.unwrap();
        assert_eq!(entry.state, ConflictState::Open);
        assert!(entry.conflict_id.as_str().starts_with("conflict-"));

        assert_eq!(count_open(&st.pool, &base()).await.unwrap(), 1);
        let under_dir = list(&st.pool, &CanonicalPath::new_unchecked("\\\\srv\\share"))
            .await
            .unwrap();
        assert_eq!(under_dir.len(), 1);
        assert_eq!(under_dir[0].sidecar_path, sidecar());

        // conflict_open was audited.
        let events = audit::query(&st.pool, &base()).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, AuditKind::ConflictOpen);
    }

    #[tokio::test]
    async fn resolve_closes_and_audits() {
        let st = AppState::new(db::test_pool().await);
        let entry = register(&st, &base(), &sidecar(), &who(), &sess()).await.unwrap();
        let resolved = resolve(
            &st,
            &entry.conflict_id,
            ConflictResolution::KeptTheirs,
            &who(),
            &sess(),
        )
        .await
        .unwrap();
        assert_eq!(resolved.state, ConflictState::Resolved);
        assert_eq!(resolved.resolution, Some(ConflictResolution::KeptTheirs));

        // No longer open: count drops, list empty.
        assert_eq!(count_open(&st.pool, &base()).await.unwrap(), 0);
        assert!(list(&st.pool, &base()).await.unwrap().is_empty());

        // conflict_open + conflict_resolve both audited.
        let events = audit::query(&st.pool, &base()).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, AuditKind::ConflictResolve);
    }

    #[tokio::test]
    async fn resolving_twice_is_not_found() {
        let st = AppState::new(db::test_pool().await);
        let entry = register(&st, &base(), &sidecar(), &who(), &sess()).await.unwrap();
        resolve(&st, &entry.conflict_id, ConflictResolution::Discarded, &who(), &sess())
            .await
            .unwrap();
        let err = resolve(&st, &entry.conflict_id, ConflictResolution::Discarded, &who(), &sess())
            .await
            .unwrap_err();
        assert!(matches!(err, ChaprError::ConflictNotFound { .. }));
    }

    #[tokio::test]
    async fn unknown_conflict_resolve_is_not_found() {
        let st = AppState::new(db::test_pool().await);
        let err = resolve(
            &st,
            &ConflictId::new_unchecked("conflict-nope"),
            ConflictResolution::Merged,
            &who(),
            &sess(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ChaprError::ConflictNotFound { .. }));
    }
}
