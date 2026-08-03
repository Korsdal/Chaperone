//! Atomic coord-state migration for a rename (concept §6.3).
//!
//! The endpoint has already performed the SMB rename (the file is ground truth,
//! invariant 1); this re-keys coord's view to match, in **one transaction**:
//!
//! - **plain move** (destination was absent): re-key the version-log chain and
//!   any open journal/conflict entries from `src` to `dst`;
//! - **overwrite move** (destination existed, went through CAS): `dst` keeps its
//!   own lineage, so `src`'s coord state is discarded.
//!
//! Then a `Move` version-log entry is appended on `dst` and the move is audited
//! (`write_commit` — `AuditKind` has no dedicated move; the detail records it).
//! All-or-nothing: a failure rolls back and coord is unchanged.
//!
//! Named `mv` because `move` is a Rust keyword.

use crate::state::AppState;
use chapr_proto::{CanonicalPath, ChaprError, Principal, SessionId, VersionToken};
use chrono::Utc;
use sqlx::Row;
use uuid::Uuid;

#[allow(clippy::too_many_arguments)]
pub async fn move_paths(
    st: &AppState,
    src: &CanonicalPath,
    dst: &CanonicalPath,
    version: &VersionToken,
    size: u64,
    overwrite: bool,
    principal: &Principal,
    session_id: &SessionId,
) -> Result<(), ChaprError> {
    let _guard = st.acquire_lock.lock().await;
    let now_ms = Utc::now().timestamp_millis();
    let db = |e: sqlx::Error| ChaprError::Internal {
        message: format!("coord db error: {e}"),
    };

    let mut tx = st.pool.begin().await.map_err(db)?;

    if overwrite {
        // Destination keeps its own history/journal/conflicts; src is consumed.
        for sql in [
            "DELETE FROM version_log WHERE path = ?1",
            "DELETE FROM journal WHERE path = ?1",
            "DELETE FROM conflicts WHERE base_path = ?1",
        ] {
            sqlx::query(sql).bind(src.as_str()).execute(&mut *tx).await.map_err(db)?;
        }
    } else {
        // Plain move: re-key src's coord state onto dst.
        sqlx::query("UPDATE version_log SET path = ?2 WHERE path = ?1")
            .bind(src.as_str()).bind(dst.as_str()).execute(&mut *tx).await.map_err(db)?;
        sqlx::query("UPDATE journal SET path = ?2 WHERE path = ?1")
            .bind(src.as_str()).bind(dst.as_str()).execute(&mut *tx).await.map_err(db)?;
        sqlx::query("UPDATE conflicts SET base_path = ?2 WHERE base_path = ?1")
            .bind(src.as_str()).bind(dst.as_str()).execute(&mut *tx).await.map_err(db)?;
    }

    // Append the Move entry, chained to dst's current head.
    let prev: Option<String> =
        sqlx::query("SELECT blob_hash FROM version_log WHERE path = ?1 ORDER BY id DESC LIMIT 1")
            .bind(dst.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?
            .map(|r| r.get::<String, _>("blob_hash"));
    sqlx::query(
        "INSERT INTO version_log
           (path, timestamp_ms, blob_hash, writer_principal, prev_hash, size, event)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'move')",
    )
    .bind(dst.as_str())
    .bind(now_ms)
    .bind(version.as_str())
    .bind(principal.as_str())
    .bind(prev)
    .bind(size as i64)
    .execute(&mut *tx)
    .await
    .map_err(db)?;

    // Audit the move.
    sqlx::query(
        "INSERT INTO audit_log
           (event_id, timestamp_ms, principal, session_id, canonical_path,
            kind, from_version, to_version, detail)
         VALUES (?1, ?2, ?3, ?4, ?5, 'write_commit', NULL, ?6, ?7)",
    )
    .bind(format!("evt-{}", Uuid::new_v4()))
    .bind(now_ms)
    .bind(principal.as_str())
    .bind(session_id.as_str())
    .bind(dst.as_str())
    .bind(version.as_str())
    .bind(format!("move from {src}"))
    .execute(&mut *tx)
    .await
    .map_err(db)?;

    tx.commit().await.map_err(db)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{audit, conflict, db, history};
    use chapr_proto::{AuditKind, VersionEvent};

    fn src() -> CanonicalPath {
        CanonicalPath::new_unchecked("\\\\srv\\share\\old.md")
    }
    fn dst() -> CanonicalPath {
        CanonicalPath::new_unchecked("\\\\srv\\share\\new.md")
    }
    fn who() -> Principal {
        Principal::new_unchecked("CONTOSO\\demo")
    }
    fn sess() -> SessionId {
        SessionId::new_unchecked("sess-1")
    }

    #[tokio::test]
    async fn plain_move_rekeys_history_and_conflicts() {
        let st = AppState::new(db::test_pool().await);
        let v1 = VersionToken::hash(b"v1");
        history::append_version_log(&st, &src(), &v1, &who(), 2, VersionEvent::Create)
            .await
            .unwrap();
        conflict::register(
            &st,
            &src(),
            &CanonicalPath::new_unchecked("\\\\srv\\share\\old.conflict-x.md"),
            &who(),
            &sess(),
        )
        .await
        .unwrap();

        move_paths(&st, &src(), &dst(), &v1, 2, false, &who(), &sess())
            .await
            .unwrap();

        // src has nothing; dst has the migrated chain + a Move entry.
        assert!(history::history(&st.pool, &src()).await.unwrap().entries.is_empty());
        let dst_hist = history::history(&st.pool, &dst()).await.unwrap();
        assert_eq!(dst_hist.entries.len(), 2); // create (migrated) + move
        assert_eq!(dst_hist.entries[0].event, VersionEvent::Move);
        assert_eq!(dst_hist.entries[1].event, VersionEvent::Create);

        // The open conflict moved to dst.
        assert_eq!(conflict::count_open(&st.pool, &src()).await.unwrap(), 0);
        assert_eq!(conflict::count_open(&st.pool, &dst()).await.unwrap(), 1);

        // Move was audited on dst.
        let events = audit::query(&st.pool, &dst()).await.unwrap();
        assert!(events.iter().any(|e| e.kind == AuditKind::WriteCommit
            && e.detail.contains("move from")));
    }

    #[tokio::test]
    async fn overwrite_move_discards_src_and_keeps_dst_lineage() {
        let st = AppState::new(db::test_pool().await);
        let sv = VersionToken::hash(b"src");
        let dv = VersionToken::hash(b"dst-old");
        history::append_version_log(&st, &src(), &sv, &who(), 3, VersionEvent::Create)
            .await
            .unwrap();
        history::append_version_log(&st, &dst(), &dv, &who(), 4, VersionEvent::Create)
            .await
            .unwrap();

        move_paths(&st, &src(), &dst(), &sv, 3, true, &who(), &sess())
            .await
            .unwrap();

        assert!(history::history(&st.pool, &src()).await.unwrap().entries.is_empty());
        // dst keeps its own create + the Move entry (src's chain discarded).
        let dst_hist = history::history(&st.pool, &dst()).await.unwrap();
        assert_eq!(dst_hist.entries.len(), 2);
        assert_eq!(dst_hist.entries[0].event, VersionEvent::Move);
        assert_eq!(dst_hist.entries[1].event, VersionEvent::Create);
        assert_eq!(dst_hist.entries[1].version, dv); // dst's own lineage, not src's
    }
}
