// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! The version index — coord's BLAKE3 hash cache (concept §4.2) and the
//! `coord.resolve` read used by the endpoint's read state machine (§8.1).
//!
//! The cache is keyed by the composite `(canonical_path, mtime, size)`
//! (confirmed E-003 contract; logbook D-004). [`resolve`] returns
//! `cached_version = Some` only on an exact key match, so a changed file — same
//! path, different mtime/size — reads as a miss and the endpoint knows to
//! re-hash. [`refresh`] is how the freshly-hashed value gets back in.
//!
//! Until the change-watcher exists (deferred; §14), the index is populated
//! **lazily**: first read misses, the endpoint hashes the bytes, then calls
//! refresh. There is no watcher-driven invalidation yet, which is why the
//! composite key matters — it is the only staleness signal available.
//!
//! Both calls are lock-free. `resolve` is a pure read on the hot path (§8.1
//! says keep it fast) and `refresh` is a blind idempotent upsert with no
//! read-modify-write to race, so neither needs the coarse lock.

use crate::config::BackendRoute;
use crate::state::AppState;
use crate::{conflict, journal, lease};
use chapr_proto::{
    BackendDescriptor, BackendKind, CanonicalPath, ChaprError, ResolveRequest, ResolveResponse,
    VersionToken,
};
use chrono::{DateTime, Utc};
use sqlx::Row;

/// `coord.resolve` (concept §8.1): the cheap phase-one metadata call. Returns
/// the cached version (composite-key hit only), the journal state, and the
/// current lease over the path.
pub async fn resolve(st: &AppState, req: ResolveRequest) -> Result<ResolveResponse, ChaprError> {
    let now_ms = Utc::now().timestamp_millis();

    // lease_state is path-keyed (independent of the stat).
    let lease_state = lease::lease_ref_for_path(&st.pool, &req.path, now_ms).await?;

    // cached_version matches on the full composite key.
    let cached_version = lookup_version(st, &req.path, req.mtime, req.size).await?;

    // journal_state: Clean / Live / Dangling from the intent journal (§8.1).
    let journal_state = journal::state_for_path(&st.pool, &req.path, now_ms).await?;

    // Surface-on-touch: open conflicts against this file (§11). Omitted if zero.
    let count = conflict::count_open(&st.pool, &req.path).await?;
    let open_conflicts = (count > 0).then_some(count);

    // Announce which backend owns this resource (§14). Advisory — the endpoint
    // selects and verifies locally (invariant 3); coord never does backend I/O.
    let backend = Some(backend_for(st.backend_default, &st.backend_routes, &req.path));

    Ok(ResolveResponse {
        cached_version,
        journal_state,
        lease_state,
        open_conflicts,
        backend,
    })
}

/// Classify a path to its backend: longest-prefix match over the configured
/// routes, falling back to the global default. **The one path-shaped function**
/// in the discovery seam and the swap-point for a future SQLite registry — when
/// resource identity generalizes to an opaque locator (invariant 5), only the
/// `starts_with` here becomes a `scope.contains(locator)`.
pub(crate) fn backend_for(
    default: BackendKind,
    routes: &[BackendRoute],
    path: &CanonicalPath,
) -> BackendDescriptor {
    let kind = routes
        .iter()
        .filter(|r| path.as_str().starts_with(&r.prefix))
        .max_by_key(|r| r.prefix.len())
        .map(|r| r.kind)
        .unwrap_or(default);
    BackendDescriptor { kind }
}

/// Composite-key lookup: `Some(version)` iff a row exists for `path` whose
/// stored `(mtime, size)` matches exactly.
async fn lookup_version(
    st: &AppState,
    path: &CanonicalPath,
    mtime: DateTime<Utc>,
    size: u64,
) -> Result<Option<VersionToken>, ChaprError> {
    let row = sqlx::query(
        "SELECT version FROM version_index WHERE path = ?1 AND mtime_ms = ?2 AND size = ?3",
    )
    .bind(path.as_str())
    .bind(mtime.timestamp_millis())
    .bind(size as i64)
    .fetch_optional(&st.pool)
    .await
    .map_err(internal)?;

    // A stored value is always a valid token (refresh only ever writes
    // `VersionToken::as_str`), so a parse failure would mean a corrupt row —
    // treat it as a miss rather than trusting it.
    Ok(row.and_then(|r| VersionToken::from_hex(r.get::<String, _>("version"))))
}

/// Insert or update the cache entry for `path` (the lazy-population write, and
/// later the watcher's refresh path). Idempotent upsert on the `path` key.
pub async fn refresh(
    st: &AppState,
    path: &CanonicalPath,
    version: &VersionToken,
    mtime: DateTime<Utc>,
    size: u64,
) -> Result<(), ChaprError> {
    let now_ms = Utc::now().timestamp_millis();
    sqlx::query(
        "INSERT INTO version_index (path, version, mtime_ms, size, updated_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(path) DO UPDATE SET
             version = ?2, mtime_ms = ?3, size = ?4, updated_at_ms = ?5",
    )
    .bind(path.as_str())
    .bind(version.as_str())
    .bind(mtime.timestamp_millis())
    .bind(size as i64)
    .bind(now_ms)
    .execute(&st.pool)
    .await
    .map_err(internal)?;
    Ok(())
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
    use chapr_proto::{JournalState, LeasePurpose, Principal};

    fn path() -> CanonicalPath {
        CanonicalPath::new_unchecked("\\\\srv\\share\\doc.md")
    }
    fn mtime() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-07-21T09:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }
    fn req(mtime: DateTime<Utc>, size: u64) -> ResolveRequest {
        ResolveRequest {
            path: path(),
            mtime,
            size,
        }
    }

    #[tokio::test]
    async fn refresh_then_resolve_is_a_cache_hit() {
        let st = AppState::new(db::test_pool().await);
        let v = VersionToken::hash(b"content");
        refresh(&st, &path(), &v, mtime(), 7).await.unwrap();

        let resp = resolve(&st, req(mtime(), 7)).await.unwrap();
        assert_eq!(resp.cached_version, Some(v));
        assert_eq!(resp.journal_state, JournalState::Clean);
        assert!(resp.lease_state.is_none());
    }

    #[tokio::test]
    async fn changed_size_or_mtime_is_a_miss() {
        let st = AppState::new(db::test_pool().await);
        let v = VersionToken::hash(b"content");
        refresh(&st, &path(), &v, mtime(), 7).await.unwrap();

        // Same path, different size → miss.
        assert!(resolve(&st, req(mtime(), 8)).await.unwrap().cached_version.is_none());
        // Same path, different mtime → miss.
        let later = mtime() + chrono::Duration::seconds(1);
        assert!(resolve(&st, req(later, 7)).await.unwrap().cached_version.is_none());
    }

    #[tokio::test]
    async fn unknown_path_resolves_clean_with_no_version() {
        let st = AppState::new(db::test_pool().await);
        let resp = resolve(&st, req(mtime(), 7)).await.unwrap();
        assert!(resp.cached_version.is_none());
        assert_eq!(resp.journal_state, JournalState::Clean);
        assert!(resp.lease_state.is_none());
    }

    #[tokio::test]
    async fn refresh_upserts_the_new_version() {
        let st = AppState::new(db::test_pool().await);
        refresh(&st, &path(), &VersionToken::hash(b"v1"), mtime(), 2).await.unwrap();
        let v2 = VersionToken::hash(b"v2");
        let m2 = mtime() + chrono::Duration::seconds(5);
        refresh(&st, &path(), &v2, m2, 3).await.unwrap();

        // Old key is gone; new key hits.
        assert!(resolve(&st, req(mtime(), 2)).await.unwrap().cached_version.is_none());
        assert_eq!(resolve(&st, req(m2, 3)).await.unwrap().cached_version, Some(v2));
    }

    #[tokio::test]
    async fn resolve_surfaces_a_held_lease() {
        let st = AppState::new(db::test_pool().await);
        lease::acquire(
            &st,
            Principal::new_unchecked("CONTOSO\\jsmith"),
            chapr_proto::SessionId::new_unchecked("sess-i"),
            LeasePurpose::Write,
            vec![path()],
        )
        .await
        .unwrap();

        let resp = resolve(&st, req(mtime(), 7)).await.unwrap();
        let lease = resp.lease_state.expect("path is leased");
        assert_eq!(lease.principal, Principal::new_unchecked("CONTOSO\\jsmith"));
    }

    #[tokio::test]
    async fn resolve_announces_the_default_backend() {
        let st = AppState::new(db::test_pool().await); // default: Smb, no routes
        let resp = resolve(&st, req(mtime(), 7)).await.unwrap();
        assert_eq!(
            resp.backend,
            Some(BackendDescriptor {
                kind: BackendKind::Smb
            })
        );
    }

    #[tokio::test]
    async fn resolve_announces_a_posix_default_backend() {
        // A POSIX-configured coord announces posix on the wire (E-019 S0).
        let st = AppState::new(db::test_pool().await).with_backends(BackendKind::Posix, vec![]);
        let resp = resolve(&st, req(mtime(), 7)).await.unwrap();
        assert_eq!(
            resp.backend,
            Some(BackendDescriptor {
                kind: BackendKind::Posix
            })
        );
    }

    #[test]
    fn backend_for_selects_a_posix_route() {
        let routes = vec![crate::config::BackendRoute {
            prefix: "/mnt/share".into(),
            kind: BackendKind::Posix,
        }];
        let p = CanonicalPath::new_unchecked("/mnt/share/a.md");
        assert_eq!(
            backend_for(BackendKind::Smb, &routes, &p).kind,
            BackendKind::Posix
        );
    }

    #[test]
    fn backend_for_longest_prefix_wins_else_default() {
        let routes = vec![
            BackendRoute {
                prefix: "\\\\srv\\share".into(),
                kind: BackendKind::Smb,
            },
            BackendRoute {
                prefix: "\\\\srv\\share\\cloud".into(),
                kind: BackendKind::Smb, // only Smb exists; longest-prefix still selected
            },
        ];
        // Longest matching prefix chosen.
        let deep = CanonicalPath::new_unchecked("\\\\srv\\share\\cloud\\a.md");
        assert_eq!(
            backend_for(BackendKind::Smb, &routes, &deep).kind,
            BackendKind::Smb
        );
        // No route matches → global default.
        let other = CanonicalPath::new_unchecked("\\\\other\\x");
        assert_eq!(
            backend_for(BackendKind::Smb, &[], &other).kind,
            BackendKind::Smb
        );
    }
}
