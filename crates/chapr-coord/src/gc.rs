// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! Blob garbage collection + retention (concept §12).
//!
//! Reference-counted mark-and-sweep over the content-addressed blob store, with
//! the version log as the reference set. A blob's **bytes** are kept iff:
//!
//! - it is among a file's newest `per_file_floor` versions (kept regardless of
//!   age — the floor), OR
//! - its newest version-log timestamp is within `retention_days`.
//!
//! Everything else is evicted: orphans (no version-log row) and old
//! beyond-floor pre-images. A global `ceiling_bytes` valve then evicts the
//! oldest still-kept-by-age (never floor) blobs until under the ceiling.
//!
//! Two additions guard the reference set against the fact that a blob is stored
//! *before* the row that references it exists:
//!
//! - **In-flight pre-images.** `journal.pre_image_version` names the last
//!   known-good bytes of a write that is still open. Nothing in the version log
//!   references it yet, so a plain orphan sweep would delete exactly the blob
//!   crash recovery needs — turning a recoverable torn write into
//!   `RecoveryFailed`. The journal is therefore part of the keep-set.
//! - **A write grace period.** `PUT /blobs` and `POST /version-log` are separate
//!   round-trips, so every write has a window where its blob is on disk with no
//!   row anywhere. Blobs whose file mtime is inside `write_grace` are kept
//!   regardless, which closes that window without needing a distributed
//!   transaction.
//!
//! Crucially this GCs **bytes only** — version-log *metadata* is untouched
//! (kept for audit retention), so "who changed this and when" stays answerable
//! long after the old bytes are gone; restoring a GC'd version fails cleanly
//! with `VersionNotFound`.
//!
//! The numbers (90 d / last-10 / ~50 GB) are runaway-protection defaults and an
//! explicitly-open item (§18 #2 — validate against real write volume); all are
//! tunable here without touching the mechanism.

use crate::state::AppState;
use chapr_proto::ChaprError;
use chrono::Utc;
use sqlx::Row;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use tokio::fs;

const DAY_MS: i64 = 86_400_000;

/// GC tuning (concept §12, §16). Defaults are the documented runaway-protection
/// values — tunable pending real-write-volume validation (§18 #2).
#[derive(Clone, Debug)]
pub struct GcConfig {
    pub retention_days: i64,
    pub per_file_floor: usize,
    pub ceiling_bytes: u64,
    /// Blobs written this recently are never evicted, whatever the reference set
    /// says. Covers the gap between `PUT /blobs` and `POST /version-log`.
    pub write_grace: std::time::Duration,
}

impl Default for GcConfig {
    fn default() -> Self {
        GcConfig {
            retention_days: 90,
            per_file_floor: 10,
            ceiling_bytes: 50 * 1024 * 1024 * 1024,
            // Generous relative to the milliseconds a commit tail actually takes;
            // the cost of being wrong here is destroying a recovery pre-image.
            write_grace: std::time::Duration::from_secs(3600),
        }
    }
}

/// What a sweep did — always logged, never a silent cap.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct GcReport {
    pub blobs_kept: u64,
    pub blobs_evicted: u64,
    pub bytes_freed: u64,
    /// Blobs the sweep wanted to evict but could not delete. Reported rather
    /// than aborting the sweep; they are retried on the next tick.
    pub evict_failures: u64,
}

/// Run one GC sweep.
pub async fn sweep(st: &AppState, cfg: &GcConfig) -> Result<GcReport, ChaprError> {
    let now_ms = Utc::now().timestamp_millis();
    let cutoff = now_ms - cfg.retention_days.saturating_mul(DAY_MS);

    // Floor: the newest `per_file_floor` blob hashes per path.
    let floor: HashSet<String> = sqlx::query(
        "SELECT blob_hash FROM (
             SELECT blob_hash, ROW_NUMBER() OVER (PARTITION BY path ORDER BY id DESC) AS rn
             FROM version_log
         ) WHERE rn <= ?1",
    )
    .bind(cfg.per_file_floor as i64)
    .fetch_all(&st.pool)
    .await
    .map_err(db)?
    .into_iter()
    .map(|r| r.get::<String, _>("blob_hash"))
    .collect();

    // Age: hashes whose newest version is within the retention window.
    let within_age: HashSet<String> = sqlx::query(
        "SELECT DISTINCT blob_hash FROM version_log WHERE timestamp_ms >= ?1",
    )
    .bind(cutoff)
    .fetch_all(&st.pool)
    .await
    .map_err(db)?
    .into_iter()
    .map(|r| r.get::<String, _>("blob_hash"))
    .collect();

    // In-flight pre-images: a write that is still open has a journal row naming
    // the last known-good bytes, and NO version-log row referencing them yet.
    // These are the blobs crash recovery reads, so they are never evictable.
    let in_flight: HashSet<String> = sqlx::query("SELECT pre_image_version FROM journal")
        .fetch_all(&st.pool)
        .await
        .map_err(db)?
        .into_iter()
        .map(|r| r.get::<String, _>("pre_image_version"))
        .collect();

    let mut keep: HashSet<String> = floor.union(&within_age).cloned().collect();
    keep.extend(in_flight.iter().cloned());

    // Ceiling valve: if the kept set still exceeds the ceiling, evict the oldest
    // age-only (non-floor) blobs until under. Never evict floor blobs.
    let blobs = list_blobs(&st.blob_root).await?;
    let kept_bytes: u64 = blobs
        .iter()
        .filter(|b| keep.contains(&b.hash))
        .map(|b| b.size)
        .sum();
    if kept_bytes > cfg.ceiling_bytes {
        let ages = blob_newest_ts(st).await?;
        let mut evictable: Vec<&Blob> = blobs
            .iter()
            // Never the floor, and never an in-flight pre-image — the ceiling is
            // a storage guard, not a licence to break crash recovery.
            .filter(|b| {
                keep.contains(&b.hash)
                    && !floor.contains(&b.hash)
                    && !in_flight.contains(&b.hash)
            })
            .collect();
        // Oldest first.
        evictable.sort_by_key(|b| ages.get(&b.hash).copied().unwrap_or(i64::MAX));
        let mut over = kept_bytes.saturating_sub(cfg.ceiling_bytes);
        for b in evictable {
            if over == 0 {
                break;
            }
            keep.remove(&b.hash);
            over = over.saturating_sub(b.size);
        }
        tracing::warn!(ceiling = cfg.ceiling_bytes, kept_bytes, "blob store over ceiling — evicting oldest beyond floor");
    }

    // Sweep: delete every blob file not in the keep set, except ones written so
    // recently that their referencing row may still be in flight.
    let now = std::time::SystemTime::now();
    let mut report = GcReport::default();
    for b in &blobs {
        if keep.contains(&b.hash) {
            report.blobs_kept += 1;
            continue;
        }
        let within_grace = b
            .modified
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age < cfg.write_grace);
        if within_grace {
            report.blobs_kept += 1;
            continue;
        }
        // One un-deletable blob (a concurrent read holding a handle on Windows,
        // a permissions oddity) must not abort the rest of the sweep and discard
        // the whole report.
        match fs::remove_file(&b.path).await {
            Ok(()) => {
                report.blobs_evicted += 1;
                report.bytes_freed += b.size;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                report.evict_failures += 1;
                tracing::warn!(blob = %b.hash, error = %e, "blob eviction failed; will retry next sweep");
            }
        }
    }
    Ok(report)
}

/// Spawn a periodic GC sweep (background job, like the reaper).
pub fn spawn(st: AppState, period: std::time::Duration, cfg: GcConfig) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(period);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            match sweep(&st, &cfg).await {
                Ok(r) if r.blobs_evicted > 0 => {
                    tracing::info!(evicted = r.blobs_evicted, bytes = r.bytes_freed, kept = r.blobs_kept, "blob GC sweep")
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "blob GC sweep failed"),
            }
        }
    })
}

struct Blob {
    hash: String,
    path: PathBuf,
    size: u64,
    /// File mtime, for the write-grace check. `None` if the platform or
    /// filesystem would not report one — treated as "old" so it stays evictable.
    modified: Option<std::time::SystemTime>,
}

/// The newest version-log timestamp per blob hash (for ceiling ordering).
async fn blob_newest_ts(st: &AppState) -> Result<HashMap<String, i64>, ChaprError> {
    Ok(sqlx::query("SELECT blob_hash, MAX(timestamp_ms) AS ts FROM version_log GROUP BY blob_hash")
        .fetch_all(&st.pool)
        .await
        .map_err(db)?
        .into_iter()
        .map(|r| (r.get::<String, _>("blob_hash"), r.get::<i64, _>("ts")))
        .collect())
}

/// Enumerate every blob in the two-level sharded store.
async fn list_blobs(root: &std::path::Path) -> Result<Vec<Blob>, ChaprError> {
    let mut out = Vec::new();
    let mut l1 = match fs::read_dir(root).await {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(io(e)),
    };
    while let Some(a) = l1.next_entry().await.map_err(io)? {
        if !a.file_type().await.map_err(io)?.is_dir() {
            continue;
        }
        let mut l2 = fs::read_dir(a.path()).await.map_err(io)?;
        while let Some(b) = l2.next_entry().await.map_err(io)? {
            if !b.file_type().await.map_err(io)?.is_dir() {
                continue;
            }
            let mut files = fs::read_dir(b.path()).await.map_err(io)?;
            while let Some(f) = files.next_entry().await.map_err(io)? {
                if f.file_type().await.map_err(io)?.is_file() {
                    let meta = f.metadata().await.map_err(io)?;
                    out.push(Blob {
                        hash: f.file_name().to_string_lossy().into_owned(),
                        size: meta.len(),
                        modified: meta.modified().ok(),
                        path: f.path(),
                    });
                }
            }
        }
    }
    Ok(out)
}

fn db(e: sqlx::Error) -> ChaprError {
    ChaprError::Internal {
        message: format!("coord db error: {e}"),
    }
}
fn io(e: std::io::Error) -> ChaprError {
    ChaprError::Internal {
        message: format!("coord blob GC I/O error: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, history};
    use chapr_proto::{CanonicalPath, Principal, VersionToken};

    fn st_with_blobs(tmp: &tempfile::TempDir, pool: sqlx::SqlitePool) -> AppState {
        AppState::new(pool).with_blob_root(tmp.path().to_path_buf())
    }
    fn path() -> CanonicalPath {
        CanonicalPath::new_unchecked("\\\\srv\\share\\a.md")
    }
    fn who() -> Principal {
        Principal::new_unchecked("CONTOSO\\demo")
    }

    /// Test blobs are written milliseconds ago, so the real write-grace would
    /// keep every one of them. Eviction tests opt out of it explicitly; the
    /// grace itself is covered by `write_grace_protects_a_fresh_orphan`.
    fn cfg(retention_days: i64, per_file_floor: usize, ceiling_bytes: u64) -> GcConfig {
        GcConfig {
            retention_days,
            per_file_floor,
            ceiling_bytes,
            write_grace: std::time::Duration::ZERO,
        }
    }

    /// Store a blob and record a version-log entry for it at `age_days` old.
    async fn seed(st: &AppState, content: &[u8], age_days: i64) -> VersionToken {
        let stored = history::put_blob(&st.blob_root, content).await.unwrap();
        let ts = Utc::now().timestamp_millis() - age_days * DAY_MS;
        sqlx::query(
            "INSERT INTO version_log (path, timestamp_ms, blob_hash, writer_principal, prev_hash, size, event)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5, 'write')",
        )
        .bind(path().as_str())
        .bind(ts)
        .bind(stored.version.as_str())
        .bind(who().as_str())
        .bind(content.len() as i64)
        .execute(&st.pool)
        .await
        .unwrap();
        stored.version
    }

    async fn exists(st: &AppState, v: &VersionToken) -> bool {
        history::get_blob(&st.blob_root, v).await.is_ok()
    }

    #[tokio::test]
    async fn evicts_old_beyond_floor_keeps_floor_and_recent() {
        let tmp = tempfile::tempdir().unwrap();
        let st = st_with_blobs(&tmp, db::test_pool().await);
        // An old blob (200d) that will be beyond a floor of 1.
        let old = seed(&st, b"old-content", 200).await;
        // A recent blob (1d).
        let recent = seed(&st, b"recent-content", 1).await;

        let cfg = cfg(90, 1, u64::MAX);
        let report = sweep(&st, &cfg).await.unwrap();

        // floor=1 keeps the newest (recent); old is beyond floor AND past 90d → evicted.
        assert!(exists(&st, &recent).await, "recent kept (floor + within age)");
        assert!(!exists(&st, &old).await, "old-beyond-floor-and-retention evicted");
        assert_eq!(report.blobs_evicted, 1);
    }

    #[tokio::test]
    async fn age_retention_keeps_recent_beyond_floor() {
        let tmp = tempfile::tempdir().unwrap();
        let st = st_with_blobs(&tmp, db::test_pool().await);
        let recent_old_position = seed(&st, b"c1", 5).await; // recent, but will be beyond floor=1
        let newest = seed(&st, b"c2", 1).await;

        let cfg = cfg(90, 1, u64::MAX);
        sweep(&st, &cfg).await.unwrap();
        // Both within 90d → both kept, even though only one is within the floor.
        assert!(exists(&st, &recent_old_position).await);
        assert!(exists(&st, &newest).await);
    }

    #[tokio::test]
    async fn orphan_blob_is_evicted_once_past_the_write_grace() {
        let tmp = tempfile::tempdir().unwrap();
        let st = st_with_blobs(&tmp, db::test_pool().await);
        // A blob with no version-log row and no journal row: genuine garbage.
        let orphan = history::put_blob(&st.blob_root, b"orphan").await.unwrap().version;
        let report = sweep(&st, &cfg(90, 10, u64::MAX)).await.unwrap();
        assert!(!exists(&st, &orphan).await);
        assert_eq!(report.blobs_evicted, 1);
    }

    /// The data-loss regression that motivated [`VersionEvent::Baseline`].
    ///
    /// A write snapshots the bytes it **replaces** and logs the version it
    /// **produces**. For a file Chaperone itself authored those line up one write
    /// apart, so every blob ends up referenced. For a file it did not — anything a
    /// human wrote — the pre-image matched no version-log row, GC saw a plain
    /// orphan, and once past the write grace it deleted the only copy of the
    /// file's pre-agent contents. On the pilot's central workflow (agents writing
    /// into human-authored tenders) that is the common path, not an edge case.
    #[tokio::test]
    async fn a_writes_pre_image_survives_gc_and_stays_restorable() {
        let tmp = tempfile::tempdir().unwrap();
        let st = st_with_blobs(&tmp, db::test_pool().await);

        // A human wrote this. Chaperone never saw it produced, so nothing in the
        // version log names it.
        let human = b"the tender a human wrote";
        let pre = history::put_blob(&st.blob_root, human).await.unwrap();

        // An agent writes over it, exactly as `commit_tail` does.
        let produced = VersionToken::hash(b"what the agent wrote");
        history::append_version_log(
            &st,
            &path(),
            &produced,
            &who(),
            20,
            chapr_proto::VersionEvent::Write,
            Some(&chapr_proto::PreImage {
                version: pre.version.clone(),
                size: human.len() as u64,
            }),
        )
        .await
        .unwrap();

        // Zero grace: nothing survives merely for being freshly written.
        sweep(&st, &cfg(90, 10, u64::MAX)).await.unwrap();

        assert!(
            exists(&st, &pre.version).await,
            "GC deleted the only copy of the file's pre-agent contents"
        );
        // Surviving is half of it — history must name the version, or nothing can
        // restore to it.
        let hist = history::history(&st.pool, &path()).await.unwrap();
        assert!(
            hist.entries
                .iter()
                .any(|e| e.version == pre.version
                    && e.event == chapr_proto::VersionEvent::Baseline),
            "the pre-agent version is not offered by chapr.history"
        );
    }

    #[tokio::test]
    async fn write_grace_protects_a_fresh_orphan() {
        let tmp = tempfile::tempdir().unwrap();
        let st = st_with_blobs(&tmp, db::test_pool().await);
        // Same blob as above, but swept with the real default grace: it was just
        // written, so its `POST /version-log` may still be in flight.
        let fresh = history::put_blob(&st.blob_root, b"just-written").await.unwrap().version;
        let report = sweep(&st, &GcConfig::default()).await.unwrap();
        assert!(exists(&st, &fresh).await, "a blob written seconds ago must survive");
        assert_eq!(report.blobs_evicted, 0);
    }

    #[tokio::test]
    async fn in_flight_journal_pre_image_is_never_evicted() {
        let tmp = tempfile::tempdir().unwrap();
        let st = st_with_blobs(&tmp, db::test_pool().await);
        // The pre-image of an open write: on disk, referenced only by `journal`.
        // Evicting this turns a recoverable torn write into RecoveryFailed.
        let pre = history::put_blob(&st.blob_root, b"pre-image").await.unwrap().version;
        sqlx::query(
            "INSERT INTO journal (path, lease_id, principal, pre_image_version, intended_version, opened_at_ms)
             VALUES (?1, 'lease-x', ?2, ?3, NULL, ?4)",
        )
        .bind(path().as_str())
        .bind(who().as_str())
        .bind(pre.as_str())
        .bind(Utc::now().timestamp_millis())
        .execute(&st.pool)
        .await
        .unwrap();

        // Zero grace and a retention window that would otherwise evict it.
        let report = sweep(&st, &cfg(0, 0, u64::MAX)).await.unwrap();
        assert!(exists(&st, &pre).await, "journal pre-image kept");
        assert_eq!(report.blobs_evicted, 0);
    }

    #[tokio::test]
    async fn ceiling_evicts_oldest_beyond_floor() {
        let tmp = tempfile::tempdir().unwrap();
        let st = st_with_blobs(&tmp, db::test_pool().await);
        // Three recent (within age) blobs, floor=1. Ceiling forces eviction of
        // the oldest age-kept (non-floor) ones.
        let oldest = seed(&st, b"AAAAAAAAAA", 3).await; // 10 bytes
        let middle = seed(&st, b"BBBBBBBBBB", 2).await;
        let newest = seed(&st, b"CCCCCCCCCC", 1).await;

        // Ceiling 15 bytes: floor keeps newest (10); 5 bytes of headroom < the
        // next blob, so both older age-kept blobs get evicted.
        let cfg = cfg(90, 1, 15);
        sweep(&st, &cfg).await.unwrap();

        assert!(exists(&st, &newest).await, "floor always kept");
        assert!(!exists(&st, &oldest).await, "oldest beyond floor evicted by ceiling");
        assert!(!exists(&st, &middle).await, "middle beyond floor evicted by ceiling");
    }
}
