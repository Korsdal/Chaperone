//! Session read-tracking — the basis for structural read-before-write
//! (concept §6.2).
//!
//! Coord records `(session, path, version)` whenever a session reads a version
//! ([`record`], posted by `chapr.read` and by a write on commit so the writer
//! can chain), and rejects a `base_version` on write/delete/move that it has
//! not recorded this session as having read ([`assert`]). The model cannot
//! fabricate a token it never saw — read-before-write is enforced structurally,
//! not just by schema.
//!
//! Defence-in-depth over CAS (which already requires `base_version` to equal
//! the file's current content hash); both are lock-free blind insert/lookup.

use crate::state::AppState;
use chapr_proto::{CanonicalPath, ChaprError, SessionId, VersionToken};
use chrono::Utc;
use sqlx::SqlitePool;

/// Record that `session` has read `version` of `path`. Idempotent.
pub async fn record(
    st: &AppState,
    session_id: &SessionId,
    path: &CanonicalPath,
    version: &VersionToken,
) -> Result<(), ChaprError> {
    sqlx::query(
        "INSERT OR IGNORE INTO session_reads (session_id, path, version, seen_at_ms)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(session_id.as_str())
    .bind(path.as_str())
    .bind(version.as_str())
    .bind(Utc::now().timestamp_millis())
    .execute(&st.pool)
    .await
    .map_err(internal)?;
    Ok(())
}

/// Assert that `session` has read `version` of `path`; otherwise
/// [`ChaprError::BaseVersionNotRecorded`].
pub async fn assert(
    pool: &SqlitePool,
    session_id: &SessionId,
    path: &CanonicalPath,
    version: &VersionToken,
) -> Result<(), ChaprError> {
    let found = sqlx::query(
        "SELECT 1 FROM session_reads WHERE session_id = ?1 AND path = ?2 AND version = ?3 LIMIT 1",
    )
    .bind(session_id.as_str())
    .bind(path.as_str())
    .bind(version.as_str())
    .fetch_optional(pool)
    .await
    .map_err(internal)?;

    if found.is_some() {
        Ok(())
    } else {
        Err(ChaprError::BaseVersionNotRecorded {
            path: path.clone(),
            provided: version.clone(),
        })
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

    fn path() -> CanonicalPath {
        CanonicalPath::new_unchecked("\\\\srv\\share\\a.md")
    }
    fn sess() -> SessionId {
        SessionId::new_unchecked("sess-1")
    }

    #[tokio::test]
    async fn record_then_assert_passes() {
        let st = AppState::new(db::test_pool().await);
        let v = VersionToken::hash(b"x");
        record(&st, &sess(), &path(), &v).await.unwrap();
        assert!(reads_ok(&st, &v).await);
    }

    #[tokio::test]
    async fn unrecorded_version_is_rejected() {
        let st = AppState::new(db::test_pool().await);
        record(&st, &sess(), &path(), &VersionToken::hash(b"x")).await.unwrap();
        // A different (fabricated) version this session never read.
        assert!(!reads_ok(&st, &VersionToken::hash(b"fabricated")).await);
    }

    #[tokio::test]
    async fn a_different_session_does_not_inherit_reads() {
        let st = AppState::new(db::test_pool().await);
        let v = VersionToken::hash(b"x");
        record(&st, &sess(), &path(), &v).await.unwrap();
        let other = SessionId::new_unchecked("sess-2");
        let err = assert(&st.pool, &other, &path(), &v).await.unwrap_err();
        assert!(matches!(err, ChaprError::BaseVersionNotRecorded { .. }));
    }

    async fn reads_ok(st: &AppState, v: &VersionToken) -> bool {
        assert(&st.pool, &sess(), &path(), v).await.is_ok()
    }
}
