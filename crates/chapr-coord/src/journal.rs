//! The intent journal — the record of in-flight writes that drives crash
//! recovery (concept §5.2, §7, §8.1, §15).
//!
//! ## What it is
//!
//! One entry per canonical path, opened at write-path step 7 (`{F, pre_image,
//! intended}`) and cleared at step 11 once the write commits. Its whole job is
//! to make a *crashed* write detectable: if the process dies between open and
//! clear, the entry is left behind, and the owning lease — whose liveness the
//! entry records — will expire. That combination is the "dangling" signal.
//!
//! ## The three read states (concept §8.1)
//!
//! [`state_for_path`] resolves a path into exactly one:
//! - **Clean** — no journal entry. The hot path.
//! - **Live** — entry present, owning lease still alive. A write is in flight
//!   under an exclusive handle; the reader briefly waits.
//! - **Dangling** — entry present, owning lease dead. The file may be torn from
//!   a crashed write; the endpoint will recover-then-serve (E-005 — coord
//!   cannot write bytes to the share).
//!
//! ## E-004 scope (logbook D-005)
//!
//! Journal + detection only. Coord records entries, reports the three states,
//! and flags dangling entries on startup ([`scan_dangling`]). The pre-image
//! *bytes* live in the content-addressed history store (its own next item), and
//! writing them back into the file is the endpoint's job (E-005). So here
//! recovery means **detect and flag**, never restore.

use crate::state::AppState;
use chapr_proto::{
    AuditKind, CanonicalPath, ChaprError, JournalEntry, JournalState, LeaseId, Principal,
    RecoveredFrom, SessionId, VersionToken,
};
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};

/// Open a journal entry for an in-flight write (concept §7 step 7). Coord
/// stamps `opened_at`. Keyed by path, so a pre-existing entry (a prior dangling
/// write on the same path) is superseded — safe because the caller holds the
/// path's exclusive lease, so no *live* entry can be present to clobber.
pub async fn open(
    st: &AppState,
    path: &CanonicalPath,
    lease_id: &LeaseId,
    principal: &Principal,
    pre_image_version: &VersionToken,
    intended_version: Option<&VersionToken>,
) -> Result<(), ChaprError> {
    let _guard = st.acquire_lock.lock().await;
    let now_ms = Utc::now().timestamp_millis();
    sqlx::query(
        "INSERT OR REPLACE INTO journal
           (path, lease_id, principal, pre_image_version, intended_version, opened_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(path.as_str())
    .bind(lease_id.as_str())
    .bind(principal.as_str())
    .bind(pre_image_version.as_str())
    .bind(intended_version.map(|v| v.as_str()))
    .bind(now_ms)
    .execute(&st.pool)
    .await
    .map_err(internal)?;
    Ok(())
}

/// Clear a path's journal entry (concept §7 step 11, on a clean commit).
/// Idempotent: clearing an absent entry is a no-op success.
pub async fn clear(st: &AppState, path: &CanonicalPath) -> Result<(), ChaprError> {
    let _guard = st.acquire_lock.lock().await;
    sqlx::query("DELETE FROM journal WHERE path = ?1")
        .bind(path.as_str())
        .execute(&st.pool)
        .await
        .map_err(internal)?;
    Ok(())
}

/// Resolve a path into its [`JournalState`] (concept §8.1). A pure read — no
/// coarse lock — so it stays on the fast resolve path.
pub async fn state_for_path(
    pool: &SqlitePool,
    path: &CanonicalPath,
    now_ms: i64,
) -> Result<JournalState, ChaprError> {
    let entry = sqlx::query("SELECT lease_id FROM journal WHERE path = ?1")
        .bind(path.as_str())
        .fetch_optional(pool)
        .await
        .map_err(internal)?;

    let Some(entry) = entry else {
        return Ok(JournalState::Clean);
    };
    let lease_id: String = entry.get("lease_id");

    let owning_lease_alive =
        sqlx::query("SELECT 1 FROM leases WHERE lease_id = ?1 AND expiry_ms > ?2 AND hard_expiry_ms > ?2 LIMIT 1")
            .bind(&lease_id)
            .bind(now_ms)
            .fetch_optional(pool)
            .await
            .map_err(internal)?
            .is_some();

    Ok(if owning_lease_alive {
        JournalState::Live
    } else {
        JournalState::Dangling
    })
}

/// Find every dangling entry — a journal entry whose owning lease is no longer
/// live. Run proactively at coord startup (concept §15) to flag files that may
/// be torn from a crash before serving. Returns the entries so the caller can
/// log them; actual recovery is deferred to the endpoint (E-005).
pub async fn scan_dangling(
    pool: &SqlitePool,
    now_ms: i64,
) -> Result<Vec<JournalEntry>, ChaprError> {
    let rows = sqlx::query(
        "SELECT path, lease_id, principal, pre_image_version, intended_version, opened_at_ms
         FROM journal j
         WHERE NOT EXISTS (
             SELECT 1 FROM leases l
             WHERE l.lease_id = j.lease_id AND l.expiry_ms > ?1 AND l.hard_expiry_ms > ?1
         )",
    )
    .bind(now_ms)
    .fetch_all(pool)
    .await
    .map_err(internal)?;

    let mut dangling = Vec::with_capacity(rows.len());
    for row in rows {
        let path: String = row.get("path");
        let pre_hex: String = row.get("pre_image_version");
        let Some(pre_image_version) = VersionToken::from_hex(pre_hex) else {
            // A stored token is always valid; a bad one means a corrupt row.
            // Skip it rather than fabricate an entry.
            tracing::warn!(%path, "skipping journal row with unparseable pre_image_version");
            continue;
        };
        let intended_version = match row.get::<Option<String>, _>("intended_version") {
            Some(hex) => VersionToken::from_hex(hex),
            None => None,
        };

        dangling.push(JournalEntry {
            path: CanonicalPath::new_unchecked(path),
            lease_id: LeaseId::new_unchecked(row.get::<String, _>("lease_id")),
            principal: Principal::new_unchecked(row.get::<String, _>("principal")),
            pre_image_version,
            intended_version,
            opened_at: DateTime::<Utc>::from_timestamp_millis(row.get::<i64, _>("opened_at_ms"))
                .unwrap_or_default(),
        });
    }
    Ok(dangling)
}

/// Recover a dangling in-flight write (concept §8.1). Confirms the entry is
/// truly dangling (present, owning lease dead), clears it, records a
/// `crash_recover` audit event, and returns the [`RecoveredFrom`] the reader
/// uses to fetch and serve the pre-image. Coord does not touch the file bytes —
/// serving the recovered pre-image is the endpoint's job.
///
/// Errors: [`ChaprError::NotFound`] if there is no journal entry for the path
/// (nothing to recover, or already recovered); [`ChaprError::Internal`] if the
/// owning lease is in fact still alive (the write is `Live`, not dangling —
/// the caller should re-read).
pub async fn recover(
    st: &AppState,
    path: &CanonicalPath,
    principal: &Principal,
    session_id: &SessionId,
) -> Result<RecoveredFrom, ChaprError> {
    let _guard = st.acquire_lock.lock().await;
    let now = Utc::now();
    let now_ms = now.timestamp_millis();

    let row = sqlx::query("SELECT lease_id, principal, pre_image_version FROM journal WHERE path = ?1")
        .bind(path.as_str())
        .fetch_optional(&st.pool)
        .await
        .map_err(internal)?;
    let Some(row) = row else {
        return Err(ChaprError::NotFound { path: path.clone() });
    };
    let lease_id: String = row.get("lease_id");
    let interrupted_writer = Principal::new_unchecked(row.get::<String, _>("principal"));
    let Some(pre_image) = VersionToken::from_hex(row.get::<String, _>("pre_image_version")) else {
        return Err(ChaprError::Internal {
            message: "corrupt journal pre_image_version".into(),
        });
    };

    // Confirm dangling: the owning lease must be dead. If it is alive, this is a
    // live write, not a crash — refuse and let the caller re-read.
    let owning_lease_alive =
        sqlx::query("SELECT 1 FROM leases WHERE lease_id = ?1 AND expiry_ms > ?2 AND hard_expiry_ms > ?2 LIMIT 1")
            .bind(&lease_id)
            .bind(now_ms)
            .fetch_optional(&st.pool)
            .await
            .map_err(internal)?
            .is_some();
    if owning_lease_alive {
        return Err(ChaprError::Internal {
            message: "journal entry is live, not dangling; retry read".into(),
        });
    }

    sqlx::query("DELETE FROM journal WHERE path = ?1")
        .bind(path.as_str())
        .execute(&st.pool)
        .await
        .map_err(internal)?;

    crate::audit::record(
        st,
        principal,
        session_id,
        path,
        AuditKind::CrashRecover,
        Some(&pre_image),
        None,
        &format!("recovered pre-image after interrupted write by {interrupted_writer}"),
    )
    .await?;

    Ok(RecoveredFrom {
        version: pre_image,
        interrupted_writer,
        at: now,
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
    use crate::lease;
    use chapr_proto::LeasePurpose;

    fn path() -> CanonicalPath {
        CanonicalPath::new_unchecked("\\\\srv\\share\\wip.md")
    }
    fn who() -> Principal {
        Principal::new_unchecked("CONTOSO\\jsmith")
    }
    fn now_ms() -> i64 {
        Utc::now().timestamp_millis()
    }

    /// Open a journal entry backed by a real, live lease over the path.
    async fn open_live(st: &AppState) -> LeaseId {
        let acq = lease::acquire(
            st,
            who(),
            SessionId::new_unchecked("sess-j"),
            LeasePurpose::Write,
            vec![path()],
        )
        .await
        .unwrap();
        open(
            st,
            &path(),
            &acq.lease_id,
            &who(),
            &VersionToken::hash(b"pre-image"),
            Some(&VersionToken::hash(b"intended")),
        )
        .await
        .unwrap();
        acq.lease_id
    }

    #[tokio::test]
    async fn no_entry_is_clean() {
        let st = AppState::new(db::test_pool().await);
        assert_eq!(
            state_for_path(&st.pool, &path(), now_ms()).await.unwrap(),
            JournalState::Clean
        );
    }

    #[tokio::test]
    async fn entry_with_live_lease_is_live() {
        let st = AppState::new(db::test_pool().await);
        open_live(&st).await;
        assert_eq!(
            state_for_path(&st.pool, &path(), now_ms()).await.unwrap(),
            JournalState::Live
        );
    }

    #[tokio::test]
    async fn entry_with_dead_lease_is_dangling() {
        let st = AppState::new(db::test_pool().await);
        let lease_id = open_live(&st).await;
        // Release the lease out from under the journal entry — simulates a crash
        // where the writer died and its lease then expired/was reaped.
        lease::release(&st, &lease_id).await.unwrap();
        assert_eq!(
            state_for_path(&st.pool, &path(), now_ms()).await.unwrap(),
            JournalState::Dangling
        );
    }

    #[tokio::test]
    async fn clear_returns_to_clean() {
        let st = AppState::new(db::test_pool().await);
        open_live(&st).await;
        clear(&st, &path()).await.unwrap();
        assert_eq!(
            state_for_path(&st.pool, &path(), now_ms()).await.unwrap(),
            JournalState::Clean
        );
    }

    #[tokio::test]
    async fn scan_finds_only_dangling_entries() {
        let st = AppState::new(db::test_pool().await);
        // One live in-flight write.
        open_live(&st).await;
        assert!(scan_dangling(&st.pool, now_ms()).await.unwrap().is_empty());

        // Kill its lease → now dangling.
        let dangling = {
            let lease_id: String =
                sqlx::query("SELECT lease_id FROM journal WHERE path = ?1")
                    .bind(path().as_str())
                    .fetch_one(&st.pool)
                    .await
                    .unwrap()
                    .get("lease_id");
            lease::release(&st, &LeaseId::new_unchecked(lease_id))
                .await
                .unwrap();
            scan_dangling(&st.pool, now_ms()).await.unwrap()
        };
        assert_eq!(dangling.len(), 1);
        assert_eq!(dangling[0].path, path());
        assert_eq!(dangling[0].pre_image_version, VersionToken::hash(b"pre-image"));
        assert_eq!(
            dangling[0].intended_version,
            Some(VersionToken::hash(b"intended"))
        );
    }

    #[tokio::test]
    async fn recover_clears_dangling_and_audits_crash_recover() {
        let st = AppState::new(db::test_pool().await);
        let lease_id = open_live(&st).await;
        // Writer "crashes": its lease goes away → the entry is now dangling.
        crate::lease::release(&st, &lease_id).await.unwrap();
        assert_eq!(
            state_for_path(&st.pool, &path(), now_ms()).await.unwrap(),
            JournalState::Dangling
        );

        let recovered = recover(
            &st,
            &path(),
            &Principal::new_unchecked("CONTOSO\\reader"),
            &SessionId::new_unchecked("sess-r"),
        )
        .await
        .unwrap();
        assert_eq!(recovered.version, VersionToken::hash(b"pre-image"));
        assert_eq!(recovered.interrupted_writer, who());

        // Journal is now clean, and a crash_recover event was audited.
        assert_eq!(
            state_for_path(&st.pool, &path(), now_ms()).await.unwrap(),
            JournalState::Clean
        );
        // Newest-first: crash_recover, then the earlier lease_grant from open_live.
        let events = crate::audit::query(&st.pool, &path()).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, AuditKind::CrashRecover);
        assert_eq!(events[0].from_version, Some(VersionToken::hash(b"pre-image")));
        assert_eq!(events[1].kind, AuditKind::LeaseGrant);
    }

    #[tokio::test]
    async fn recover_with_no_entry_is_not_found() {
        let st = AppState::new(db::test_pool().await);
        let err = recover(
            &st,
            &path(),
            &Principal::new_unchecked("CONTOSO\\reader"),
            &SessionId::new_unchecked("sess-r"),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ChaprError::NotFound { .. }));
    }

    #[tokio::test]
    async fn recover_refuses_a_live_entry() {
        let st = AppState::new(db::test_pool().await);
        open_live(&st).await; // lease still alive → Live, not dangling
        let err = recover(
            &st,
            &path(),
            &Principal::new_unchecked("CONTOSO\\reader"),
            &SessionId::new_unchecked("sess-r"),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ChaprError::Internal { .. }));
    }

    #[tokio::test]
    async fn open_supersedes_a_prior_entry_on_the_same_path() {
        let st = AppState::new(db::test_pool().await);
        open_live(&st).await;
        // Re-open with a different pre-image (a fresh write superseding a
        // dangling one). PK on path means REPLACE, not a duplicate-row error.
        open(
            &st,
            &path(),
            &LeaseId::new_unchecked("lease-new"),
            &who(),
            &VersionToken::hash(b"pre-image-2"),
            None,
        )
        .await
        .unwrap();
        let entries = scan_dangling(&st.pool, now_ms()).await.unwrap();
        // lease-new does not exist → dangling, and there is exactly one row.
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].pre_image_version, VersionToken::hash(b"pre-image-2"));
        assert_eq!(entries[0].intended_version, None);
    }
}
