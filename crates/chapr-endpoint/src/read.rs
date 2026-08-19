// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! The read state machine (concept §8) — `chapr.read`'s core logic, independent
//! of both MCP transport and Win32.
//!
//! Two-phase (§8.2): a cheap `coord.resolve` metadata call, then bytes straight
//! from the share. The heavy bytes never touch coord.
//!
//! The three journal states (§8.1) plus the availability asymmetry (§8.3):
//! - **Clean** — serve bytes. Composite-key cache hit → trust the cached
//!   version without re-hashing (the hot path); miss → hash and refresh the
//!   index. `integrity = verified`.
//! - **Live** — a write holds the file under an exclusive handle. Bounded brief
//!   wait, re-resolving until it clears; then serve (or `RetryBudgetExhausted`).
//! - **Dangling** — a crashed write. Recover-then-serve: coord clears the entry
//!   and audits `crash_recover`; we fetch the **pre-image** from the blob store
//!   and serve *that*, so the reader never sees torn bytes. `integrity = recovered`.
//! - **coord unreachable** — reads degrade-open (§8.3): serve bytes with
//!   `integrity = unverified` and no version. (Writes fail-closed; reads don't.)
//!
//! The untrusted-data envelope (§13.3) is applied by the MCP tool layer, not
//! here — this module returns the raw bytes in a [`ReadResponse`].
//!
//! [`FileSource`] abstracts the share so every branch is unit-testable without a
//! real file or a concurrent writer; `StdFs` is the real backing.

use crate::backend::{map_os_err, select_backend};
use crate::canon::canonicalize;
use crate::coord_client::CoordClient;
use crate::pathgrammar::grammar_for;
use chapr_proto::{
    AuditKind, BackendKind, CanonicalPath, ChaprError, ClearJournalRequest, ConflictsQuery,
    Integrity, JournalState, ListEntry, ListResponse, Principal, ReadContent, ReadReceipt,
    ReadResponse, RecordAuditRequest, RecoverJournalRequest, RefreshIndexRequest, ResolveRequest,
    SessionId, StatResponse, VersionToken,
};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::io;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The stat the read path needs to key the version index (concept §4.2).
pub struct FileStat {
    pub mtime: DateTime<Utc>,
    pub size: u64,
}

/// One raw directory entry from [`FileSource::list`] — backend-native metadata
/// before coord annotation (the canonical path + open-conflict counts that the
/// tool-layer [`list`] adds).
pub struct RawDirEntry {
    pub name: String,
    pub size: u64,
    pub mtime: DateTime<Utc>,
}

/// The file backend (SMB share / local NTFS / …). The narrow **read sub-seam**:
/// kept separate from the full `Backend` trait so the read state machine stays
/// testable against a tiny mock. The real SMB backing lives on
/// [`crate::backend::SmbBackend`]; `MockFs` (below) backs the tests.
pub trait FileSource: Send + Sync {
    fn stat(&self, path: &CanonicalPath) -> io::Result<FileStat>;
    fn read(&self, path: &CanonicalPath) -> io::Result<Vec<u8>>;
    fn list(&self, dir: &CanonicalPath) -> io::Result<Vec<RawDirEntry>>;
}

/// Tuning for the Live-state bounded wait (concept §8.1, §16: ≈3 × 200 ms).
#[derive(Clone, Debug)]
pub struct ReadConfig {
    pub live_wait_attempts: u32,
    pub live_wait_interval: Duration,
}

impl Default for ReadConfig {
    fn default() -> Self {
        ReadConfig {
            live_wait_attempts: 3,
            live_wait_interval: Duration::from_millis(200),
        }
    }
}

/// Execute `chapr.read` for `raw_uri`. See the module docs for the branches.
#[allow(clippy::too_many_arguments)]
pub async fn read(
    coord: &CoordClient,
    fs: &dyn FileSource,
    local_backend: BackendKind,
    cfg: &ReadConfig,
    principal: &Principal,
    session_id: &SessionId,
    raw_uri: &str,
) -> Result<ReadResponse, ChaprError> {
    let path = canonicalize(raw_uri, grammar_for(local_backend))?;
    let stat = fs.stat(&path).map_err(|e| map_os_err(&path, e))?;

    // Phase one: cheap metadata. Coord unreachable → degrade-open (§8.3).
    let resolved = match coord
        .resolve(&ResolveRequest {
            path: path.clone(),
            mtime: stat.mtime,
            size: stat.size,
        })
        .await
    {
        Ok(r) => r,
        Err(ChaprError::CoordUnreachable) => return degrade_open(fs, &path),
        Err(e) => return Err(e),
    };

    // Cross-check coord's advisory backend announcement against the backend we
    // actually drive (D-A: local-authoritative, coord-advisory). A mismatch is a
    // misconfiguration to surface — never a reason to switch mid-read; we serve
    // with the local backend regardless. With one backend this never fires.
    let announced = select_backend(resolved.backend.as_ref());
    if announced != local_backend {
        tracing::warn!(
            path = %path, %announced, %local_backend,
            "coord announced a different backend than this endpoint drives; using local"
        );
    }

    let resp = match resolved.journal_state {
        JournalState::Clean => {
            serve_clean(
                coord,
                fs,
                &path,
                &stat,
                resolved.cached_version,
                resolved.open_conflicts,
            )
            .await?
        }
        JournalState::Live => serve_live(coord, fs, cfg, principal, session_id, &path).await?,
        JournalState::Dangling => {
            serve_dangling(
                coord,
                fs,
                principal,
                session_id,
                &path,
                resolved.open_conflicts,
            )
            .await?
        }
    };

    // Record the read so a subsequent write can present this version as its
    // base_version (structural read-before-write, §6.2). Best-effort — a
    // recording failure must not fail the read.
    if let Some(version) = &resp.version {
        let _ = coord
            .record_read(&ReadReceipt {
                session_id: session_id.clone(),
                path: path.clone(),
                version: version.clone(),
            })
            .await;
    }
    Ok(resp)
}

/// Clean: serve bytes with a verified version (cache hit avoids the re-hash).
async fn serve_clean(
    coord: &CoordClient,
    fs: &dyn FileSource,
    path: &CanonicalPath,
    stat: &FileStat,
    cached_version: Option<VersionToken>,
    open_conflicts: Option<u32>,
) -> Result<ReadResponse, ChaprError> {
    let bytes = fs.read(path).map_err(|e| map_os_err(path, e))?;
    let version = match cached_version {
        Some(v) => v, // composite key matched → trust it, no re-hash (hot path)
        None => {
            // Miss: hash and lazily refresh the index. Refresh is best-effort —
            // a failure must not fail the read.
            let v = VersionToken::hash(&bytes);
            let _ = coord
                .refresh_index(&RefreshIndexRequest {
                    path: path.clone(),
                    version: v.clone(),
                    mtime: stat.mtime,
                    size: stat.size,
                })
                .await;
            v
        }
    };
    Ok(ReadResponse {
        content: ReadContent::Inline { bytes },
        version: Some(version),
        integrity: Integrity::Verified,
        recovered_from: None,
        open_conflicts, // surface-on-touch (§11)
    })
}

/// Live: bounded brief wait, re-resolving until the write clears.
async fn serve_live(
    coord: &CoordClient,
    fs: &dyn FileSource,
    cfg: &ReadConfig,
    principal: &Principal,
    session_id: &SessionId,
    path: &CanonicalPath,
) -> Result<ReadResponse, ChaprError> {
    for _ in 0..cfg.live_wait_attempts {
        tokio::time::sleep(cfg.live_wait_interval).await;
        let stat = fs.stat(path).map_err(|e| map_os_err(path, e))?;
        let resolved = match coord
            .resolve(&ResolveRequest {
                path: path.clone(),
                mtime: stat.mtime,
                size: stat.size,
            })
            .await
        {
            Ok(r) => r,
            Err(ChaprError::CoordUnreachable) => return degrade_open(fs, path),
            Err(e) => return Err(e),
        };
        match resolved.journal_state {
            JournalState::Clean => {
                return serve_clean(
                    coord,
                    fs,
                    path,
                    &stat,
                    resolved.cached_version,
                    resolved.open_conflicts,
                )
                .await
            }
            JournalState::Dangling => {
                return serve_dangling(
                    coord,
                    fs,
                    principal,
                    session_id,
                    path,
                    resolved.open_conflicts,
                )
                .await
            }
            JournalState::Live => continue,
        }
    }
    Err(ChaprError::RetryBudgetExhausted {
        path: path.clone(),
        attempts: cfg.live_wait_attempts,
    })
}

/// Dangling: a journal entry is open but its owning lease is dead. Two very
/// different situations produce that, and `intended_version` — recorded at
/// journal-open and, until now, never read back anywhere — is what tells them
/// apart.
///
/// - **The write committed and only failed to clear.** Its bytes are durable on
///   the share; step 11 (`journal_clear`) failed afterwards, e.g. coord blipped.
///   The file hashes to `intended_version`. Nothing is torn, and serving the
///   pre-image here would hand the reader *older* content than the file
///   genuinely holds — so clear the stale entry and serve the file.
/// - **The write is genuinely torn.** Serve the pre-image, and **leave the entry
///   in place.** Clearing it would protect only this reader: the next read would
///   see a Clean path, hash the still-torn bytes and return them as
///   [`Integrity::Verified`]. Nothing repairs the file, so the marker has to
///   outlive the read. It is superseded by the next real write (`journal::open`
///   is `INSERT OR REPLACE`), which is what actually re-establishes ground truth.
///
/// The blob is fetched only after that decision, and the entry is never cleared
/// before the bytes are in hand — previously a failed `get_blob` destroyed the
/// marker and served nothing, leaving the file torn and unflagged.
async fn serve_dangling(
    coord: &CoordClient,
    fs: &dyn FileSource,
    principal: &Principal,
    session_id: &SessionId,
    path: &CanonicalPath,
    open_conflicts: Option<u32>,
) -> Result<ReadResponse, ChaprError> {
    let recovered = coord
        .recover_journal(&RecoverJournalRequest {
            path: path.clone(),
            principal: principal.clone(),
            session_id: session_id.clone(),
        })
        .await?;

    // One read serves both purposes: the completion check, and — in the committed
    // case — the bytes to return.
    let on_disk = fs.read(path).map_err(|e| map_os_err(path, e))?;
    let on_disk_version = VersionToken::hash(&on_disk);

    if recovered.intended_version.as_ref() == Some(&on_disk_version) {
        // Committed, not torn. Clearing is best-effort: if it fails, the next
        // reader simply repeats this check and reaches the same conclusion.
        let _ = coord
            .journal_clear(&ClearJournalRequest { path: path.clone() })
            .await;
        return Ok(ReadResponse {
            content: ReadContent::Inline { bytes: on_disk },
            version: Some(on_disk_version),
            integrity: Integrity::Verified,
            recovered_from: None,
            open_conflicts,
        });
    }

    let bytes = coord.get_blob(&recovered.version).await?;
    // Audited here rather than coord-side, because only the endpoint can hash the
    // file and therefore only the endpoint knows a recovery actually happened.
    // Best-effort — a failed audit write must not fail the read.
    let _ = coord
        .record_audit(&RecordAuditRequest {
            principal: principal.clone(),
            session_id: session_id.clone(),
            path: path.clone(),
            kind: AuditKind::CrashRecover,
            from_version: Some(recovered.version.clone()),
            to_version: None,
            detail: format!(
                "served pre-image after interrupted write by {}",
                recovered.interrupted_writer
            ),
        })
        .await;
    Ok(ReadResponse {
        content: ReadContent::Inline { bytes },
        version: Some(recovered.version.clone()),
        integrity: Integrity::Recovered,
        recovered_from: Some(recovered),
        open_conflicts,
    })
}

/// Degrade-open (§8.3): coord is unreachable, so serve bytes unverified.
fn degrade_open(fs: &dyn FileSource, path: &CanonicalPath) -> Result<ReadResponse, ChaprError> {
    let bytes = fs.read(path).map_err(|e| map_os_err(path, e))?;
    Ok(ReadResponse {
        content: ReadContent::Inline { bytes },
        version: None,
        integrity: Integrity::Unverified,
        recovered_from: None,
        open_conflicts: None,
    })
}

pub(crate) fn system_time_to_utc(t: SystemTime) -> DateTime<Utc> {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    DateTime::<Utc>::from_timestamp(d.as_secs() as i64, d.subsec_nanos()).unwrap_or_default()
}

/// `chapr.stat` (concept §6.1): metadata for one file. Version comes from the
/// index (cache hit) or is computed on a miss; journal/lease state from resolve.
pub async fn stat(
    coord: &CoordClient,
    fs: &dyn FileSource,
    kind: BackendKind,
    raw_uri: &str,
) -> Result<StatResponse, ChaprError> {
    let path = canonicalize(raw_uri, grammar_for(kind))?;
    let st = fs.stat(&path).map_err(|e| map_os_err(&path, e))?;
    let mtime = st.mtime;
    let size = st.size;

    match coord
        .resolve(&ResolveRequest {
            path: path.clone(),
            mtime,
            size,
        })
        .await
    {
        Ok(r) => {
            let version = match r.cached_version {
                Some(v) => v,
                None => hash_and_refresh(coord, fs, &path, mtime, size).await?,
            };
            Ok(StatResponse {
                canonical_path: path,
                size,
                mtime,
                version,
                lease: r.lease_state,
                journal_state: r.journal_state,
            })
        }
        // Coord down: still answer with a locally-computed version (best-effort).
        Err(ChaprError::CoordUnreachable) => {
            let bytes = fs.read(&path).map_err(|e| map_os_err(&path, e))?;
            Ok(StatResponse {
                canonical_path: path,
                size,
                mtime,
                version: VersionToken::hash(&bytes),
                lease: None,
                journal_state: JournalState::Clean,
            })
        }
        Err(e) => Err(e),
    }
}

/// `chapr.list` (concept §6.1): directory entries, annotated with open-conflict
/// counts (surface-on-touch, §11). Version is omitted per entry to avoid
/// hashing every file.
pub async fn list(
    coord: &CoordClient,
    fs: &dyn FileSource,
    kind: BackendKind,
    raw_uri: &str,
) -> Result<ListResponse, ChaprError> {
    let grammar = grammar_for(kind);
    let dir = canonicalize(raw_uri, grammar)?;
    let raw = fs.list(&dir).map_err(|e| map_os_err(&dir, e))?;

    // One scoped conflict query annotates the whole listing (degrade to empty
    // if coord is unreachable — the listing itself still succeeds).
    let mut conflict_counts: HashMap<String, u32> = HashMap::new();
    if let Ok(resp) = coord.list_conflicts(&ConflictsQuery { scope: dir.clone() }).await {
        for c in resp.conflicts {
            *conflict_counts.entry(c.base_path.into_inner()).or_insert(0) += 1;
        }
    }

    let mut entries = Vec::new();
    for e in raw {
        let canonical_path = canonicalize(&grammar.join(dir.as_str(), &e.name), grammar)?;
        let open_conflicts = conflict_counts.get(canonical_path.as_str()).copied();
        entries.push(ListEntry {
            name: e.name,
            size: e.size,
            mtime: e.mtime,
            version: None,
            open_conflicts,
            canonical_path,
        });
    }
    Ok(ListResponse { entries })
}

/// Hash a file and lazily refresh the index (shared by the read miss path and
/// stat).
async fn hash_and_refresh(
    coord: &CoordClient,
    fs: &dyn FileSource,
    path: &CanonicalPath,
    mtime: DateTime<Utc>,
    size: u64,
) -> Result<VersionToken, ChaprError> {
    let bytes = fs.read(path).map_err(|e| map_os_err(path, e))?;
    let v = VersionToken::hash(&bytes);
    let _ = coord
        .refresh_index(&RefreshIndexRequest {
            path: path.clone(),
            version: v.clone(),
            mtime,
            size,
        })
        .await;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path as wpath};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    struct MockFs {
        mtime: DateTime<Utc>,
        size: u64,
        bytes: Vec<u8>,
        stat_err: Option<io::ErrorKind>,
    }
    impl MockFs {
        fn with(bytes: &[u8]) -> Self {
            MockFs {
                mtime: DateTime::parse_from_rfc3339("2026-07-21T09:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
                size: bytes.len() as u64,
                bytes: bytes.to_vec(),
                stat_err: None,
            }
        }
    }
    impl FileSource for MockFs {
        fn stat(&self, _p: &CanonicalPath) -> io::Result<FileStat> {
            match self.stat_err {
                Some(k) => Err(io::Error::from(k)),
                None => Ok(FileStat {
                    mtime: self.mtime,
                    size: self.size,
                }),
            }
        }
        fn read(&self, _p: &CanonicalPath) -> io::Result<Vec<u8>> {
            Ok(self.bytes.clone())
        }
        fn list(&self, _dir: &CanonicalPath) -> io::Result<Vec<RawDirEntry>> {
            Ok(Vec::new())
        }
    }

    fn who() -> Principal {
        Principal::new_unchecked("CONTOSO\\reader")
    }
    fn sess() -> SessionId {
        SessionId::new_unchecked("sess-1")
    }
    fn fast_cfg() -> ReadConfig {
        ReadConfig {
            live_wait_attempts: 3,
            live_wait_interval: Duration::from_millis(1),
        }
    }
    fn inline(resp: &ReadResponse) -> &[u8] {
        match &resp.content {
            ReadContent::Inline { bytes } => bytes,
            _ => panic!("expected inline content"),
        }
    }

    #[tokio::test]
    async fn clean_cache_hit_serves_verified_without_rehash() {
        let server = MockServer::start().await;
        let cached = VersionToken::hash(b"whatever"); // deliberately NOT hash(bytes)
        Mock::given(method("POST"))
            .and(wpath("/resolve"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "cached_version": cached.as_str(),
                "journal_state": "clean"
            })))
            .mount(&server)
            .await;

        let client = CoordClient::new(server.uri());
        let fs = MockFs::with(b"file bytes");
        let resp = read(&client, &fs, BackendKind::Smb, &fast_cfg(), &who(), &sess(), "//srv/share/a.md")
            .await
            .unwrap();
        assert_eq!(resp.integrity, Integrity::Verified);
        // Trusts the cached version verbatim — proof it did not re-hash the bytes.
        assert_eq!(resp.version, Some(cached));
        assert_eq!(inline(&resp), b"file bytes");
    }

    #[tokio::test]
    async fn clean_cache_miss_hashes_and_refreshes() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(wpath("/resolve"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "journal_state": "clean" // no cached_version → miss
            })))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(wpath("/index"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1) // the lazy refresh must fire on a miss
            .mount(&server)
            .await;

        let client = CoordClient::new(server.uri());
        let fs = MockFs::with(b"content");
        let resp = read(&client, &fs, BackendKind::Smb, &fast_cfg(), &who(), &sess(), "//srv/share/a.md")
            .await
            .unwrap();
        assert_eq!(resp.integrity, Integrity::Verified);
        assert_eq!(resp.version, Some(VersionToken::hash(b"content")));
    }

    #[tokio::test]
    async fn coord_unreachable_degrades_open() {
        // No server: point at a closed port so resolve → CoordUnreachable.
        let client = CoordClient::new("http://127.0.0.1:1");
        let fs = MockFs::with(b"served anyway");
        let resp = read(&client, &fs, BackendKind::Smb, &fast_cfg(), &who(), &sess(), "//srv/share/a.md")
            .await
            .unwrap();
        assert_eq!(resp.integrity, Integrity::Unverified);
        assert_eq!(resp.version, None);
        assert_eq!(inline(&resp), b"served anyway");
    }

    #[tokio::test]
    async fn dangling_serves_recovered_pre_image_not_torn_bytes() {
        let server = MockServer::start().await;
        let pre = VersionToken::hash(b"good pre-image");
        Mock::given(method("POST"))
            .and(wpath("/resolve"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "journal_state": "dangling"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(wpath("/journal/recover"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "version": pre.as_str(),
                "interrupted_writer": "CONTOSO\\crashed",
                "at": "2026-07-21T09:00:00Z"
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(wpath(format!("/blobs/{pre}")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"good pre-image".to_vec()))
            .mount(&server)
            .await;

        let client = CoordClient::new(server.uri());
        // The file on disk is "torn" — must NOT be served.
        let fs = MockFs::with(b"TORN GARBAGE");
        let resp = read(&client, &fs, BackendKind::Smb, &fast_cfg(), &who(), &sess(), "//srv/share/a.md")
            .await
            .unwrap();
        assert_eq!(resp.integrity, Integrity::Recovered);
        assert_eq!(resp.version, Some(pre));
        assert_eq!(inline(&resp), b"good pre-image");
        assert_eq!(
            resp.recovered_from.unwrap().interrupted_writer,
            Principal::new_unchecked("CONTOSO\\crashed")
        );
    }

    #[tokio::test]
    async fn live_then_clear_serves_once_the_write_clears() {
        let server = MockServer::start().await;
        // First resolve → Live (consumed once), thereafter → Clean.
        Mock::given(method("POST"))
            .and(wpath("/resolve"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "journal_state": "live"
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(wpath("/resolve"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "cached_version": VersionToken::hash(b"cleared").as_str(),
                "journal_state": "clean"
            })))
            .mount(&server)
            .await;

        let client = CoordClient::new(server.uri());
        let fs = MockFs::with(b"cleared");
        let resp = read(&client, &fs, BackendKind::Smb, &fast_cfg(), &who(), &sess(), "//srv/share/a.md")
            .await
            .unwrap();
        assert_eq!(resp.integrity, Integrity::Verified);
        assert_eq!(resp.version, Some(VersionToken::hash(b"cleared")));
    }

    #[tokio::test]
    async fn live_forever_exhausts_the_retry_budget() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(wpath("/resolve"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "journal_state": "live"
            })))
            .mount(&server)
            .await;

        let client = CoordClient::new(server.uri());
        let fs = MockFs::with(b"unreachable-under-lock");
        let err = read(&client, &fs, BackendKind::Smb, &fast_cfg(), &who(), &sess(), "//srv/share/a.md")
            .await
            .unwrap_err();
        assert!(matches!(err, ChaprError::RetryBudgetExhausted { .. }));
    }

    #[tokio::test]
    async fn open_conflicts_surface_on_read() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(wpath("/resolve"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "cached_version": VersionToken::hash(b"c").as_str(),
                "journal_state": "clean",
                "open_conflicts": 2
            })))
            .mount(&server)
            .await;
        let client = CoordClient::new(server.uri());
        let fs = MockFs::with(b"c");
        let resp = read(&client, &fs, BackendKind::Smb, &fast_cfg(), &who(), &sess(), "//srv/share/a.md")
            .await
            .unwrap();
        assert_eq!(resp.open_conflicts, Some(2));
    }

    #[tokio::test]
    async fn missing_file_is_not_found() {
        let client = CoordClient::new("http://127.0.0.1:1");
        let fs = MockFs {
            stat_err: Some(io::ErrorKind::NotFound),
            ..MockFs::with(b"")
        };
        let err = read(&client, &fs, BackendKind::Smb, &fast_cfg(), &who(), &sess(), "//srv/share/gone.md")
            .await
            .unwrap_err();
        assert!(matches!(err, ChaprError::NotFound { .. }));
    }

    #[tokio::test]
    async fn read_consumes_coord_backend_announcement() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(wpath("/resolve"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "cached_version": VersionToken::hash(b"body").as_str(),
                "journal_state": "clean",
                "backend": { "kind": "smb" }
            })))
            .mount(&server)
            .await;
        let client = CoordClient::new(server.uri());
        let fs = MockFs::with(b"body");
        // Announced (smb) matches the local backend → serves normally, no degrade.
        let resp = read(&client, &fs, BackendKind::Smb, &fast_cfg(), &who(), &sess(), "//srv/share/a.md")
            .await
            .unwrap();
        assert_eq!(resp.integrity, Integrity::Verified);
        assert_eq!(resp.version, Some(VersionToken::hash(b"body")));
    }
}
