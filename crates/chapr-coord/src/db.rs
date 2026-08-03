//! SQLite persistence for coord.
//!
//! SQLite is the confirmed engine (concept §18 open item, resolved — logbook
//! D-003): single on-prem instance, ~20 users, embedded, one file plus a
//! separate blob mount, trivial backup. `sqlx` is used with **runtime** queries
//! (`sqlx::query`, not the `query!` macro) so `cargo build` needs no live
//! database and no `DATABASE_URL` at compile time.
//!
//! Timestamps are stored as **Unix epoch milliseconds (`INTEGER`)**, not RFC
//! 3339 text: integer comparison in SQL is exact and index-friendly, which the
//! lazy-expiry sweep (`WHERE expiry_ms <= ?`) depends on. Conversion to/from
//! [`chrono::DateTime<Utc>`] happens at the Rust boundary.

use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};
use std::str::FromStr;
use std::time::Duration;

/// The schema through E-003: the lease table and the version index.
///
/// `leases`: one row per `(lease_id, path)` — a single lease covering N paths is
/// N rows sharing a `lease_id`. Makes the per-path conflict check a trivial
/// indexed lookup, the hot operation on `lease_acquire`.
///
/// `version_index`: the BLAKE3 hash cache (concept §4.2), one current entry per
/// canonical path. The resolve lookup matches on the composite
/// `(path, mtime_ms, size)` key — a changed file (different mtime/size) is a
/// miss even though the `path` row still exists, which is exactly the
/// stale-cache signal the endpoint needs (concept §8.1). Populated lazily via
/// the refresh call until the change-watcher exists (deferred; §14).
///
/// Later subsystems (intent journal E-004, history/conflict/audit) add tables.
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS leases (
    lease_id       TEXT    NOT NULL,
    path           TEXT    NOT NULL,   -- canonical (concept §5.1); keyed per invariant 5
    principal      TEXT    NOT NULL,   -- AD principal that holds the lease
    session_id     TEXT    NOT NULL,   -- acquiring session (for lease_* audit)
    purpose        TEXT    NOT NULL,   -- LeasePurpose, serde snake_case
    granted_at_ms  INTEGER NOT NULL,
    ttl_s          INTEGER NOT NULL,
    renewed_at_ms  INTEGER NOT NULL,
    hard_expiry_ms INTEGER NOT NULL,   -- granted_at + max lifetime (concept §9)
    expiry_ms      INTEGER NOT NULL,   -- renewed_at + ttl; the heartbeat expiry
    PRIMARY KEY (lease_id, path)
);
CREATE INDEX IF NOT EXISTS idx_leases_path ON leases(path);

CREATE TABLE IF NOT EXISTS version_index (
    path          TEXT    PRIMARY KEY,  -- canonical
    version       TEXT    NOT NULL,     -- BLAKE3 hex (invariant 2)
    mtime_ms      INTEGER NOT NULL,     -- observed modification time
    size          INTEGER NOT NULL,     -- observed size in bytes
    updated_at_ms INTEGER NOT NULL      -- when coord last refreshed this entry
);

CREATE TABLE IF NOT EXISTS journal (
    path              TEXT    PRIMARY KEY,  -- canonical; at most one in-flight write per path
    lease_id          TEXT    NOT NULL,     -- the write's owning lease (liveness ⇒ live vs dangling)
    principal         TEXT    NOT NULL,
    pre_image_version TEXT    NOT NULL,     -- last known-good; points into history (blob store)
    intended_version  TEXT,                 -- NULL if crashed before hashing new content (concept §5.2)
    opened_at_ms      INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS version_log (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,  -- append order within a file
    path             TEXT    NOT NULL,   -- canonical
    timestamp_ms     INTEGER NOT NULL,
    blob_hash        TEXT    NOT NULL,   -- this version's content address = blob-store key
    writer_principal TEXT    NOT NULL,
    prev_hash        TEXT,               -- previous version in the chain; NULL for the first
    size             INTEGER NOT NULL,
    event            TEXT    NOT NULL    -- VersionEvent, serde snake_case
);
CREATE INDEX IF NOT EXISTS idx_version_log_path ON version_log(path, id);

CREATE TABLE IF NOT EXISTS audit_log (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,  -- append order
    event_id       TEXT    NOT NULL UNIQUE,
    timestamp_ms   INTEGER NOT NULL,
    principal      TEXT    NOT NULL,   -- the AD principal that acted (concept §13.1)
    session_id     TEXT    NOT NULL,
    canonical_path TEXT    NOT NULL,
    kind           TEXT    NOT NULL,   -- AuditKind, serde snake_case
    from_version   TEXT,
    to_version     TEXT,
    detail         TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_audit_path ON audit_log(canonical_path, id);

CREATE TABLE IF NOT EXISTS conflicts (
    conflict_id      TEXT    PRIMARY KEY,
    base_path        TEXT    NOT NULL,   -- the live file the conflict is against (canonical)
    sidecar_path     TEXT    NOT NULL,   -- F.conflict-{user}-{ts} holding the losing bytes
    losing_principal TEXT    NOT NULL,
    created_at_ms    INTEGER NOT NULL,
    state            TEXT    NOT NULL,   -- ConflictState: open | resolved
    resolution       TEXT                -- ConflictResolution snake_case; NULL while open
);
CREATE INDEX IF NOT EXISTS idx_conflicts_base ON conflicts(base_path, state);

CREATE TABLE IF NOT EXISTS session_reads (
    session_id TEXT    NOT NULL,
    path       TEXT    NOT NULL,   -- canonical
    version    TEXT    NOT NULL,   -- a version this session has read (concept §6.2)
    seen_at_ms INTEGER NOT NULL,
    PRIMARY KEY (session_id, path, version)
);
";

/// Open (creating if absent) a SQLite pool at `url`, e.g.
/// `sqlite:chapr-coord.db` or `sqlite::memory:`.
/// WAL, because coord has several concurrent writers by design: HTTP handlers,
/// the lease reaper, the blob GC, and the change-watcher. sqlx deliberately
/// leaves `journal_mode` alone, so without this a created database keeps
/// SQLite's `delete` rollback journal and those writers serialise into
/// `SQLITE_BUSY` — which every module collapses into `Internal` → HTTP 500, and
/// a 500 on a write's commit tail is exactly the case that loses an audit record.
pub async fn connect(url: &str, max_connections: u32) -> Result<SqlitePool, sqlx::Error> {
    let opts = SqliteConnectOptions::from_str(url)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        // sqlx defaults this to 5s; a slow blob volume plus four users deserves
        // more headroom before a handler gives up and 500s.
        .busy_timeout(Duration::from_secs(10))
        // NORMAL is the standard companion to WAL: durable across process crash
        // (which is what the journal protects against), fsync only on checkpoint.
        .synchronous(SqliteSynchronous::Normal);
    SqlitePoolOptions::new()
        .max_connections(max_connections)
        .connect_with(opts)
        .await
}

/// Apply the schema. Idempotent (`CREATE ... IF NOT EXISTS`), so it is safe to
/// run on every startup — the E-002 stand-in for real migrations.
pub async fn migrate(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::raw_sql(SCHEMA).execute(pool).await?;
    Ok(())
}

/// A pool suitable for tests: a single-connection in-memory database (a shared
/// in-memory DB needs `max_connections = 1`, or each pooled connection would
/// get its own empty database), already migrated.
#[cfg(test)]
pub async fn test_pool() -> SqlitePool {
    let pool = connect("sqlite::memory:", 1).await.expect("open in-memory db");
    migrate(&pool).await.expect("migrate");
    pool
}
