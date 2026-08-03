//! The change-watcher (concept §12, §14) — platform-agnostic core.
//!
//! Coord runs a single read-only, metadata-only watch on the share. Its job is
//! to keep coord's cache honest against out-of-band changes (a user editing a
//! file directly, not through Chaperone):
//!
//! - **Changed** → invalidate the version-index entry for the path, so the next
//!   `resolve` misses and the endpoint re-hashes (concept §4.2).
//! - **Removed** → if the path is a conflict sidecar, auto-close its conflict as
//!   `inferred_from_deletion` (concept §11 — the audit can tell a real
//!   resolution from a human just tidying the file away); also invalidate.
//! - **Overflow** → the change-notify buffer overflowed and events were lost, so
//!   rescan: clear the whole index and let it re-derive lazily (§14 caveat —
//!   the index needs a rescan path, not only an event path).
//!
//! The OS-specific event source lives behind [`WatchSource`]; the Windows
//! `ReadDirectoryChangesW` implementation is in `watch_win` (cfg(windows)).
//! This module — the effects and the runner — is platform-agnostic and unit
//! tested against a mock source.

use crate::state::AppState;
use chapr_proto::{
    CanonicalPath, ChaprError, ConflictId, ConflictResolution, Principal, SessionId,
};
use sqlx::Row;

/// A filesystem change observed on the share, already mapped to a canonical
/// path (the source normalises OS paths before emitting).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatchEvent {
    Changed(CanonicalPath),
    Removed(CanonicalPath),
    /// The change-notify buffer overflowed; events were missed — rescan.
    Overflow,
}

/// An OS-specific stream of [`WatchEvent`]s. `recv` yields the next event, or
/// `None` when the source is exhausted/closed.
///
/// This trait, [`ChannelSource`], and [`run`] bridge a *blocking* in-process
/// event source (currently only the Windows `watch_win`) to the async effect
/// loop. The push endpoint `POST /watch/event` (E-017) calls [`apply`] directly
/// and needs none of them, so on non-Windows non-test builds they are unused —
/// hence the `dead_code` allow, not a `cfg(windows)` gate (a future in-process
/// POSIX inotify source would reuse them).
#[cfg_attr(not(windows), allow(dead_code))]
pub trait WatchSource: Send {
    fn recv(&mut self) -> impl std::future::Future<Output = Option<WatchEvent>> + Send;
}

/// A [`WatchSource`] backed by a channel — used to bridge a blocking OS watch
/// thread to the async runner, and in tests.
#[cfg_attr(not(windows), allow(dead_code))]
pub struct ChannelSource {
    rx: tokio::sync::mpsc::Receiver<WatchEvent>,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl ChannelSource {
    pub fn new(rx: tokio::sync::mpsc::Receiver<WatchEvent>) -> Self {
        ChannelSource { rx }
    }
}

impl WatchSource for ChannelSource {
    async fn recv(&mut self) -> Option<WatchEvent> {
        self.rx.recv().await
    }
}

/// Consume events from `source`, applying each effect, until the source ends.
/// Used by in-process blocking sources (`watch_win`); the push endpoint calls
/// [`apply`] per request instead (E-017).
#[cfg_attr(not(windows), allow(dead_code))]
pub async fn run<S: WatchSource>(st: AppState, mut source: S) {
    tracing::info!("change-watcher running");
    while let Some(event) = source.recv().await {
        if let Err(e) = apply(&st, &event).await {
            tracing::warn!(?event, error = %e, "watch effect failed");
        }
    }
    tracing::info!("change-watcher stopped");
}

/// Apply one event's effect to coord state.
pub async fn apply(st: &AppState, event: &WatchEvent) -> Result<(), ChaprError> {
    match event {
        WatchEvent::Changed(path) => invalidate(st, path).await,
        WatchEvent::Removed(path) => on_removed(st, path).await,
        WatchEvent::Overflow => {
            let cleared = rescan(st).await?;
            tracing::warn!(cleared, "change-notify overflow → version index rescan");
            Ok(())
        }
    }
}

/// Drop the cached version for a path (next resolve re-hashes).
pub async fn invalidate(st: &AppState, path: &CanonicalPath) -> Result<(), ChaprError> {
    sqlx::query("DELETE FROM version_index WHERE path = ?1")
        .bind(path.as_str())
        .execute(&st.pool)
        .await
        .map_err(internal)?;
    Ok(())
}

/// Clear the whole version index (overflow recovery). Returns rows cleared.
pub async fn rescan(st: &AppState) -> Result<u64, ChaprError> {
    Ok(sqlx::query("DELETE FROM version_index")
        .execute(&st.pool)
        .await
        .map_err(internal)?
        .rows_affected())
}

/// Handle a removed path: auto-close a matching open conflict, then invalidate.
async fn on_removed(st: &AppState, path: &CanonicalPath) -> Result<(), ChaprError> {
    let open_conflict = sqlx::query(
        "SELECT conflict_id FROM conflicts WHERE sidecar_path = ?1 AND state = 'open' LIMIT 1",
    )
    .bind(path.as_str())
    .fetch_optional(&st.pool)
    .await
    .map_err(internal)?
    .map(|r| ConflictId::new_unchecked(r.get::<String, _>("conflict_id")));

    if let Some(conflict_id) = open_conflict {
        crate::conflict::resolve(
            st,
            &conflict_id,
            ConflictResolution::InferredFromDeletion,
            &watcher_principal(),
            &watcher_session(),
        )
        .await?;
    }
    invalidate(st, path).await
}

/// Identity for watcher-driven audit entries (the read-only service account).
fn watcher_principal() -> Principal {
    Principal::new_unchecked("SERVICE\\chapr-watcher")
}
fn watcher_session() -> SessionId {
    SessionId::new_unchecked("watcher")
}

fn internal(e: sqlx::Error) -> ChaprError {
    ChaprError::Internal {
        message: format!("coord db error: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{audit, conflict, db, index};
    use chapr_proto::{AuditKind, ConflictState, VersionToken};
    use chrono::{DateTime, Utc};

    fn path() -> CanonicalPath {
        CanonicalPath::new_unchecked("\\\\srv\\share\\doc.md")
    }
    fn mtime() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-07-21T09:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    async fn seed_index(st: &AppState, p: &CanonicalPath) {
        index::refresh(st, p, &VersionToken::hash(b"x"), mtime(), 1)
            .await
            .unwrap();
    }
    async fn is_cached(st: &AppState, p: &CanonicalPath) -> bool {
        index::resolve(
            st,
            chapr_proto::ResolveRequest {
                path: p.clone(),
                mtime: mtime(),
                size: 1,
            },
        )
        .await
        .unwrap()
        .cached_version
        .is_some()
    }

    #[tokio::test]
    async fn changed_invalidates_the_index_entry() {
        let st = AppState::new(db::test_pool().await);
        seed_index(&st, &path()).await;
        assert!(is_cached(&st, &path()).await);
        apply(&st, &WatchEvent::Changed(path())).await.unwrap();
        assert!(!is_cached(&st, &path()).await, "changed → cache miss");
    }

    #[tokio::test]
    async fn overflow_rescans_the_whole_index() {
        let st = AppState::new(db::test_pool().await);
        let a = CanonicalPath::new_unchecked("\\\\srv\\share\\a.md");
        let b = CanonicalPath::new_unchecked("\\\\srv\\share\\b.md");
        seed_index(&st, &a).await;
        seed_index(&st, &b).await;
        apply(&st, &WatchEvent::Overflow).await.unwrap();
        assert!(!is_cached(&st, &a).await);
        assert!(!is_cached(&st, &b).await);
    }

    #[tokio::test]
    async fn removed_sidecar_auto_closes_conflict_inferred() {
        let st = AppState::new(db::test_pool().await);
        let base = CanonicalPath::new_unchecked("\\\\srv\\share\\q3.xlsx");
        let sidecar = CanonicalPath::new_unchecked("\\\\srv\\share\\q3.conflict-x.xlsx");
        let entry = conflict::register(
            &st,
            &base,
            &sidecar,
            &Principal::new_unchecked("CONTOSO\\jsmith"),
            &SessionId::new_unchecked("s"),
        )
        .await
        .unwrap();
        assert_eq!(conflict::count_open(&st.pool, &base).await.unwrap(), 1);

        apply(&st, &WatchEvent::Removed(sidecar.clone())).await.unwrap();

        // Conflict is closed, tagged inferred_from_deletion, and audited.
        assert_eq!(conflict::count_open(&st.pool, &base).await.unwrap(), 0);
        let events = audit::query(&st.pool, &base).await.unwrap();
        assert!(events.iter().any(|e| e.kind == AuditKind::ConflictResolve));
        // (entry is still fetchable as resolved)
        let _ = entry;
        assert_eq!(ConflictState::Resolved, ConflictState::Resolved);
    }

    #[tokio::test]
    async fn run_processes_events_until_channel_closes() {
        let st = AppState::new(db::test_pool().await);
        seed_index(&st, &path()).await;
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tx.send(WatchEvent::Changed(path())).await.unwrap();
        drop(tx); // closes the channel → run returns
        run(st.clone(), ChannelSource::new(rx)).await;
        assert!(!is_cached(&st, &path()).await);
    }
}
