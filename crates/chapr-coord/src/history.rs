// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! Content-addressed history: the blob store and the per-file version log
//! (concept §12, §5.2, §6.5).
//!
//! ## Blob store — file-per-blob (logbook D-006)
//!
//! Each pre-image is a file named by its BLAKE3 hash — which *is* its
//! [`VersionToken`] (invariant 2) — sharded two levels deep
//! (`<root>/ab/cd/abcd…`) to keep directories small. Content addressing makes
//! dedup free: identical content hashes to the same name and is stored once.
//! Blobs live on coord's own volume, a separate mount from the operational DB
//! (§12), so bytes never bloat the SQLite file. Writes are temp-then-rename so
//! a crash mid-write cannot leave a half-written blob under its final name.
//!
//! ## Version log — the durable history (concept §5.2)
//!
//! A per-file append-only chain in SQLite: tiny metadata rows (`blob_hash`,
//! `prev_hash`, writer, size, event). Kept for the long audit-retention window
//! **even after the blob bytes are GC'd** — so "who changed this and when" is
//! answerable long after the old bytes are gone. Coord owns the chain: it fills
//! `prev_hash` from the current head on append, never trusting the caller.
//!
//! ## Scope (E-006)
//!
//! Store / fetch / append / read. **No GC or retention** — deferred (the §12
//! numbers are the unvalidated open item #2). The version log is itself the
//! reference set, so reference-counted GC can be added later with no schema
//! change.

use crate::state::AppState;
use chapr_proto::{
    CanonicalPath, ChaprError, HistoryEntry, HistoryResponse, Principal, VersionEvent,
    VersionLogEntry, VersionToken,
};
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use std::path::{Path, PathBuf};
use tokio::fs;
use uuid::Uuid;

/// Outcome of storing a blob.
pub struct BlobStored {
    pub version: VersionToken,
    pub size: u64,
    /// True if the blob already existed (dedup hit) — no bytes were written.
    pub deduplicated: bool,
}

/// Store `bytes` in the blob store under their BLAKE3 hash and return the
/// derived version. Coord always re-hashes — it never trusts a caller-supplied
/// key. Idempotent: a content-address collision *is* a dedup hit, not an error.
pub async fn put_blob(root: &Path, bytes: &[u8]) -> Result<BlobStored, ChaprError> {
    let version = VersionToken::hash(bytes);
    let (dir, file) = shard_path(root, &version);

    if fs::try_exists(&file).await.map_err(io)? {
        return Ok(BlobStored {
            version,
            size: bytes.len() as u64,
            deduplicated: true,
        });
    }

    fs::create_dir_all(&dir).await.map_err(io)?;
    // Temp-then-rename so a reader never sees a partially written blob under its
    // final content-addressed name. The temp name is unique per attempt.
    let tmp = dir.join(format!("{}.{}.tmp", version, Uuid::new_v4()));
    fs::write(&tmp, bytes).await.map_err(io)?;
    fs::rename(&tmp, &file).await.map_err(io)?;

    Ok(BlobStored {
        version,
        size: bytes.len() as u64,
        deduplicated: false,
    })
}

/// Fetch a blob's bytes by version. `VersionNotFound` if the blob is absent
/// (GC'd or never stored).
pub async fn get_blob(root: &Path, version: &VersionToken) -> Result<Vec<u8>, ChaprError> {
    let (_, file) = shard_path(root, version);
    match fs::read(&file).await {
        Ok(bytes) => Ok(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(ChaprError::VersionNotFound {
            // A raw blob fetch has no file path context; the version is the key.
            path: CanonicalPath::new_unchecked(String::new()),
            version: version.clone(),
        }),
        Err(e) => Err(io(e)),
    }
}

/// The `writer_principal` recorded on a [`VersionEvent::Baseline`] entry.
///
/// Deliberately not the principal of the write that discovered the bytes: it did
/// not author them, and the audit trail is a primary deliverable, so guessing
/// would put a name against content that name did not produce. Whoever did write
/// them did it outside Chaperone, where there is nothing to attribute.
pub(crate) const BASELINE_PRINCIPAL: &str = "(pre-existing)";

/// Append a version to a file's log (concept §7 step 11). Coord stamps the
/// timestamp and derives `prev_hash` from the current head, so the chain is
/// coord-owned. Returns the entry it wrote.
///
/// When `pre_image` is set and the chain does not already name it, a
/// [`VersionEvent::Baseline`] entry is recorded for it first, inside the same
/// transaction. That is what keeps the snapshotted blob referenced: this call
/// records the version the write *produced*, while the blob the endpoint just
/// uploaded is the one it *replaced*. For a file Chaperone itself authored the
/// two line up one write apart and the pre-image is already in the chain, so no
/// baseline is written; for a file it did not, the pre-image would otherwise be
/// named by nothing and blob GC would reclaim the only copy of the file's
/// pre-agent contents.
///
/// Ordering matters: the baseline goes in first so the produced version's
/// `prev_hash` links to it, leaving the chain in true order.
pub async fn append_version_log(
    st: &AppState,
    path: &CanonicalPath,
    blob_hash: &VersionToken,
    writer_principal: &Principal,
    size: u64,
    event: VersionEvent,
    pre_image: Option<&chapr_proto::PreImage>,
) -> Result<VersionLogEntry, ChaprError> {
    let _guard = st.acquire_lock.lock().await;
    let now = Utc::now();
    let mut tx = st.pool.begin().await.map_err(db)?;

    if let Some(pre) = pre_image {
        let already_named =
            sqlx::query("SELECT 1 FROM version_log WHERE path = ?1 AND blob_hash = ?2 LIMIT 1")
                .bind(path.as_str())
                .bind(pre.version.as_str())
                .fetch_optional(&mut *tx)
                .await
                .map_err(db)?
                .is_some();
        if !already_named {
            let head: Option<String> = sqlx::query(
                "SELECT blob_hash FROM version_log WHERE path = ?1 ORDER BY id DESC LIMIT 1",
            )
            .bind(path.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?
            .map(|r| r.get::<String, _>("blob_hash"));
            sqlx::query(
                "INSERT INTO version_log
                   (path, timestamp_ms, blob_hash, writer_principal, prev_hash, size, event)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )
            .bind(path.as_str())
            .bind(now.timestamp_millis())
            .bind(pre.version.as_str())
            .bind(BASELINE_PRINCIPAL)
            .bind(head)
            .bind(pre.size as i64)
            .bind(event_str(VersionEvent::Baseline))
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        }
    }

    let prev_hash: Option<VersionToken> =
        sqlx::query("SELECT blob_hash FROM version_log WHERE path = ?1 ORDER BY id DESC LIMIT 1")
            .bind(path.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?
            .and_then(|r| VersionToken::from_hex(r.get::<String, _>("blob_hash")));

    sqlx::query(
        "INSERT INTO version_log
           (path, timestamp_ms, blob_hash, writer_principal, prev_hash, size, event)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(path.as_str())
    .bind(now.timestamp_millis())
    .bind(blob_hash.as_str())
    .bind(writer_principal.as_str())
    .bind(prev_hash.as_ref().map(|v| v.as_str()))
    .bind(size as i64)
    .bind(event_str(event))
    .execute(&mut *tx)
    .await
    .map_err(db)?;

    tx.commit().await.map_err(db)?;

    Ok(VersionLogEntry {
        path: path.clone(),
        timestamp: now,
        blob_hash: blob_hash.clone(),
        writer_principal: writer_principal.clone(),
        prev_hash,
        size,
        event,
    })
}

/// Read a file's history, newest first (concept §6.5, backing `chapr.history`).
pub async fn history(pool: &SqlitePool, path: &CanonicalPath) -> Result<HistoryResponse, ChaprError> {
    let rows = sqlx::query(
        "SELECT timestamp_ms, blob_hash, writer_principal, size, event
         FROM version_log WHERE path = ?1 ORDER BY id DESC",
    )
    .bind(path.as_str())
    .fetch_all(pool)
    .await
    .map_err(db)?;

    let mut entries = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(version) = VersionToken::from_hex(row.get::<String, _>("blob_hash")) else {
            continue; // corrupt row; skip rather than surface a bad token
        };
        entries.push(HistoryEntry {
            version,
            timestamp: DateTime::<Utc>::from_timestamp_millis(row.get::<i64, _>("timestamp_ms"))
                .unwrap_or_default(),
            writer: Principal::new_unchecked(row.get::<String, _>("writer_principal")),
            size: row.get::<i64, _>("size") as u64,
            event: event_from_str(&row.get::<String, _>("event")),
        });
    }
    Ok(HistoryResponse { entries })
}

/// `<root>/<hex[0..2]>/<hex[2..4]>/<hex>` — two-level sharding.
fn shard_path(root: &Path, version: &VersionToken) -> (PathBuf, PathBuf) {
    let h = version.as_str();
    // A VersionToken is always 64 hex chars, so slicing is safe.
    let dir = root.join(&h[0..2]).join(&h[2..4]);
    let file = dir.join(h);
    (dir, file)
}

fn event_str(e: VersionEvent) -> &'static str {
    match e {
        VersionEvent::Baseline => "baseline",
        VersionEvent::Create => "create",
        VersionEvent::Write => "write",
        VersionEvent::Delete => "delete",
        VersionEvent::Restore => "restore",
        VersionEvent::Move => "move",
        VersionEvent::Recover => "recover",
    }
}

fn event_from_str(s: &str) -> VersionEvent {
    match s {
        "baseline" => VersionEvent::Baseline,
        "create" => VersionEvent::Create,
        "delete" => VersionEvent::Delete,
        "restore" => VersionEvent::Restore,
        "move" => VersionEvent::Move,
        "recover" => VersionEvent::Recover,
        // "write" and any unexpected value fall back to the ordinary write.
        _ => VersionEvent::Write,
    }
}

/// Blob-store filesystem faults are coord's own operational faults, not a
/// protocol branch — surface them as internal errors.
fn io(e: std::io::Error) -> ChaprError {
    ChaprError::Internal {
        message: format!("coord blob store I/O error: {e}"),
    }
}

fn db(e: sqlx::Error) -> ChaprError {
    ChaprError::Internal {
        message: format!("coord db error: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn path() -> CanonicalPath {
        CanonicalPath::new_unchecked("\\\\srv\\share\\doc.md")
    }
    fn who() -> Principal {
        Principal::new_unchecked("CONTOSO\\jsmith")
    }

    async fn state_with_blobs(tmp: &tempfile::TempDir) -> AppState {
        AppState::new(db::test_pool().await).with_blob_root(tmp.path().to_path_buf())
    }

    #[tokio::test]
    async fn put_then_get_roundtrips() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let stored = put_blob(root, b"the pre-image bytes").await.unwrap();
        assert!(!stored.deduplicated);
        assert_eq!(stored.version, VersionToken::hash(b"the pre-image bytes"));

        let got = get_blob(root, &stored.version).await.unwrap();
        assert_eq!(got, b"the pre-image bytes");
    }

    #[tokio::test]
    async fn identical_content_is_deduplicated() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let first = put_blob(root, b"same").await.unwrap();
        let second = put_blob(root, b"same").await.unwrap();
        assert!(!first.deduplicated);
        assert!(second.deduplicated, "second store of identical bytes is a dedup hit");
        assert_eq!(first.version, second.version);
    }

    #[tokio::test]
    async fn get_missing_blob_is_version_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let err = get_blob(tmp.path(), &VersionToken::hash(b"never stored"))
            .await
            .unwrap_err();
        assert!(matches!(err, ChaprError::VersionNotFound { .. }));
    }

    #[tokio::test]
    async fn version_log_chains_and_reads_newest_first() {
        let tmp = tempfile::tempdir().unwrap();
        let st = state_with_blobs(&tmp).await;

        let v1 = VersionToken::hash(b"v1");
        let v2 = VersionToken::hash(b"v2");
        let e1 = append_version_log(&st, &path(), &v1, &who(), 2, VersionEvent::Create, None)
            .await
            .unwrap();
        let e2 = append_version_log(&st, &path(), &v2, &who(), 3, VersionEvent::Write, None)
            .await
            .unwrap();

        // The chain links backwards.
        assert_eq!(e1.prev_hash, None);
        assert_eq!(e2.prev_hash, Some(v1.clone()));

        // history is newest-first and preserves version + event.
        let hist = history(&st.pool, &path()).await.unwrap();
        assert_eq!(hist.entries.len(), 2);
        assert_eq!(hist.entries[0].version, v2);
        assert_eq!(hist.entries[0].event, VersionEvent::Write);
        assert_eq!(hist.entries[1].version, v1);
        assert_eq!(hist.entries[1].event, VersionEvent::Create);
    }

    /// A pre-image Chaperone never authored enters the chain as a baseline, ahead
    /// of the version the write produced, and the produced version chains to it.
    #[tokio::test]
    async fn an_unknown_pre_image_is_recorded_as_a_baseline_first() {
        let tmp = tempfile::tempdir().unwrap();
        let st = state_with_blobs(&tmp).await;

        let human = VersionToken::hash(b"what a human wrote");
        let produced = VersionToken::hash(b"what the agent wrote");
        let entry = append_version_log(
            &st,
            &path(),
            &produced,
            &who(),
            20,
            VersionEvent::Write,
            Some(&chapr_proto::PreImage {
                version: human.clone(),
                size: 18,
            }),
        )
        .await
        .unwrap();

        // The write chains to the baseline, not to nothing.
        assert_eq!(entry.prev_hash, Some(human.clone()));

        let hist = history(&st.pool, &path()).await.unwrap();
        assert_eq!(hist.entries.len(), 2);
        assert_eq!(hist.entries[0].version, produced);
        assert_eq!(hist.entries[1].version, human);
        assert_eq!(hist.entries[1].event, VersionEvent::Baseline);
        assert_eq!(hist.entries[1].size, 18);
        // Authorship is not guessed: the agent did not write these bytes.
        assert_eq!(hist.entries[1].writer.as_str(), BASELINE_PRINCIPAL);
    }

    /// The steady state: once Chaperone has authored a version, the next write's
    /// pre-image is already in the chain and must not be duplicated.
    #[tokio::test]
    async fn a_pre_image_already_in_the_chain_adds_no_baseline() {
        let tmp = tempfile::tempdir().unwrap();
        let st = state_with_blobs(&tmp).await;

        let v1 = VersionToken::hash(b"v1");
        append_version_log(&st, &path(), &v1, &who(), 2, VersionEvent::Create, None)
            .await
            .unwrap();

        // Second write: its pre-image is v1, which the create entry already names.
        let v2 = VersionToken::hash(b"v2");
        append_version_log(
            &st,
            &path(),
            &v2,
            &who(),
            3,
            VersionEvent::Write,
            Some(&chapr_proto::PreImage {
                version: v1.clone(),
                size: 2,
            }),
        )
        .await
        .unwrap();

        let hist = history(&st.pool, &path()).await.unwrap();
        assert_eq!(hist.entries.len(), 2, "no duplicate entry for known bytes");
        assert!(hist
            .entries
            .iter()
            .all(|e| e.event != VersionEvent::Baseline));
    }

    #[tokio::test]
    async fn history_of_unknown_file_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let st = state_with_blobs(&tmp).await;
        let hist = history(&st.pool, &path()).await.unwrap();
        assert!(hist.entries.is_empty());
    }
}
