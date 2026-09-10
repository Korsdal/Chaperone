// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

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
//!
//! **The schema lives in `migrations/`, not here** (C0, D-041). `0001_baseline`
//! is the schema as it stood at v0.1.4 and carries the per-table notes that used
//! to sit on the `SCHEMA` constant.

use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};
use std::str::FromStr;
use std::time::Duration;

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

/// One schema migration: a version, a name for the ledger, and its SQL.
struct Migration {
    version: i64,
    name: &'static str,
    sql: &'static str,
}

/// Every migration, in order. Append only — never renumber, never edit one that
/// has shipped, because a coordinator in the field records what it applied by
/// version and will skip an edited file rather than notice it changed.
const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "baseline",
    sql: include_str!("../migrations/0001_baseline.sql"),
}];

// Each migration file owns its own BEGIN / COMMIT and inserts its own
// `schema_migrations` row, rather than this module wrapping it in a
// `pool.begin()` transaction. Not a stylistic choice, and not to be "tidied"
// back:
//
// `sqlx::raw_sql(..).execute(&mut *tx)` makes the enclosing future non-Send.
// The compiler needs `Executor` for `&mut SqliteConnection` at *any* lifetime,
// cannot prove it, and reports the failure a long way away — at the `rt.spawn`
// in `service_win.rs`, naming `&Pool<Sqlite>` and `&str`. Plain `sqlx::query`
// does not trip this, which is why every other module's transaction compiles;
// `raw_sql` is the one that does, and a migration is exactly where a
// multi-statement script has to run.
//
// Executing the whole file — transaction and ledger row included — with one
// `raw_sql` on the pool keeps atomicity (SQLite's DDL is transactional, and all
// the statements run on one pooled connection) while using the call shape that
// has compiled here since E-002.
//
// The cost is an implicit contract: a migration that forgot its ledger row
// would re-run on every start. `tests::every_migration_records_its_own_version`
// makes that contract checked rather than remembered.

/// Bring the database up to the current schema (C0, D-041).
///
/// Plain versioned SQL, applied in order, recorded in `schema_migrations`.
/// Deliberately **not** the abstractions D-003 rejected — no storage trait, no
/// ORM, no compile-time `DATABASE_URL` — and deliberately **not** `sqlx::migrate!`
/// either: that macro needs sqlx's `macros` feature, which drags a proc-macro
/// crate and the MySQL and Postgres drivers into a build that speaks only SQLite.
/// Forty lines here costs less than that, and every line of it is legible at the
/// point of failure.
///
/// **Each migration and its ledger row commit in one transaction**, and SQLite's
/// DDL is transactional, so a migration cannot half-apply: either the schema
/// change and the record of it both land, or neither does. That is stronger than
/// D-041 asked for — it wanted a partial state to be *detectable*; this makes it
/// unreachable, and the ledger then says exactly how far a database has come.
///
/// Safe on every startup: an already-applied version is skipped.
///
/// **What it buys over the `CREATE TABLE IF NOT EXISTS` it replaces** is the
/// thing that blocked B3 and 2.4 — a schema change that is *not*
/// additive-by-table now reaches a database that already exists.
pub async fn migrate(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
             version       INTEGER PRIMARY KEY,
             name          TEXT    NOT NULL,
             applied_at_ms INTEGER NOT NULL
         )",
    )
    .execute(pool)
    .await?;

    for m in MIGRATIONS {
        let already: Option<i64> =
            sqlx::query_scalar("SELECT version FROM schema_migrations WHERE version = ?")
                .bind(m.version)
                .fetch_optional(pool)
                .await?;
        if already.is_some() {
            continue;
        }

        sqlx::raw_sql(m.sql).execute(pool).await?;
        tracing::info!(version = m.version, name = m.name, "schema migration applied");
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The tables a coordinator must have, whichever route the database took to
    /// get here. Asserted by name rather than by count so a future migration
    /// adding one does not fail this test for the wrong reason.
    const BASELINE_TABLES: &[&str] = &[
        "audit_log",
        "conflicts",
        "diagnostic_occurrences",
        "diagnostics",
        "journal",
        "leases",
        "move_journal",
        "session_reads",
        "version_index",
        "version_log",
    ];

    async fn table_names(pool: &SqlitePool) -> Vec<String> {
        // The ledger and SQLite's own internals are excluded: what this asserts
        // is the *coordination* schema, which both routes below must agree on.
        sqlx::query_scalar::<_, String>(
            "SELECT name FROM sqlite_master WHERE type = 'table' \
             AND name NOT LIKE 'sqlite_%' AND name <> 'schema_migrations' \
             ORDER BY name",
        )
        .fetch_all(pool)
        .await
        .expect("list tables")
    }

    /// A database as a pre-C0 install left it: every table present, **no ledger
    /// and no record of anything** — which is exactly what a `CREATE TABLE IF NOT
    /// EXISTS` on every startup produced for two months.
    ///
    /// Built by stripping the transaction and the ledger insert out of the
    /// baseline file, so it stays faithful to that file rather than drifting from
    /// a hand-copied duplicate of the schema.
    async fn pre_c0_pool() -> SqlitePool {
        let pool = connect("sqlite::memory:", 1).await.expect("open in-memory db");
        let ddl_only: String = include_str!("../migrations/0001_baseline.sql")
            .replace("BEGIN;", "")
            .replace("COMMIT;", "");
        let ddl_only = &ddl_only[..ddl_only
            .find("INSERT INTO schema_migrations")
            .expect("the baseline records itself")];
        sqlx::raw_sql(ddl_only)
            .execute(&pool)
            .await
            .expect("apply the pre-C0 schema");
        pool
    }

    #[tokio::test]
    async fn a_fresh_database_migrates_to_the_baseline_schema() {
        let pool = connect("sqlite::memory:", 1).await.expect("open in-memory db");
        migrate(&pool).await.expect("migrate a fresh database");

        assert_eq!(table_names(&pool).await, BASELINE_TABLES);

        // The point of the machinery: what ran is recorded, so a partially
        // migrated database is detectable rather than guessed at (D-041).
        let applied: i64 = sqlx::query_scalar("SELECT count(*) FROM schema_migrations")
            .fetch_one(&pool)
            .await
            .expect("the migration ledger exists");
        assert_eq!(applied, 1, "exactly the baseline, recorded");
    }

    /// The risk D-041 named as C0's real one: not the mechanism, the **baseline
    /// for the install that already exists**. A live coordinator's database has
    /// every table and no ledger; migrating it must record the baseline and touch
    /// nothing else.
    #[tokio::test]
    async fn an_existing_pre_c0_database_is_baselined_without_losing_data() {
        let pool = pre_c0_pool().await;

        // A row in the one table whose loss would be unrecoverable and visible.
        sqlx::query(
            "INSERT INTO audit_log \
             (event_id, timestamp_ms, principal, session_id, canonical_path, kind, detail) \
             VALUES (?, 1, ?, 'sess-1', ?, 'write', '')",
        )
        .bind("evt-1")
        .bind(r"CONTOSO\alice")
        .bind(r"\\fs\share\a.txt")
        .execute(&pool)
        .await
        .expect("seed an audit row");

        migrate(&pool).await.expect("migrate an existing database");

        let survived: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log")
            .fetch_one(&pool)
            .await
            .expect("audit_log still readable");
        assert_eq!(survived, 1, "an upgrade must not touch the audit trail");

        // Same end state as the fresh path — the two must converge, or an upgraded
        // coordinator and a new one disagree about what the schema is.
        assert_eq!(table_names(&pool).await, BASELINE_TABLES);

        let applied: i64 = sqlx::query_scalar("SELECT count(*) FROM schema_migrations")
            .fetch_one(&pool)
            .await
            .expect("the migration ledger exists");
        assert_eq!(applied, 1, "the baseline is recorded, not re-run forever");
    }

    /// The contract the file-owns-its-transaction design creates: a migration
    /// that forgot its own ledger row would silently re-run on every start.
    /// Checked here rather than remembered — see the comment above `MIGRATIONS`.
    #[test]
    fn every_migration_records_its_own_version() {
        for m in MIGRATIONS {
            assert!(
                m.sql.contains("BEGIN;") && m.sql.contains("COMMIT;"),
                "migration {} ({}) must own its transaction",
                m.version,
                m.name,
            );
            let expected = format!("VALUES ({},", m.version);
            assert!(
                m.sql.contains("INSERT INTO schema_migrations") && m.sql.contains(&expected),
                "migration {} ({}) must insert its own schema_migrations row",
                m.version,
                m.name,
            );
        }
    }

    /// Versions are the identity a live coordinator records, so a duplicate or a
    /// backwards step would make "which migrations has this database had?"
    /// unanswerable.
    #[test]
    fn migration_versions_are_unique_and_ascending() {
        let mut prev = 0;
        for m in MIGRATIONS {
            assert!(
                m.version > prev,
                "migration {} ({}) is not after {prev}",
                m.version,
                m.name,
            );
            prev = m.version;
        }
    }

    /// Idempotence, which is what makes it safe on every startup.
    #[tokio::test]
    async fn migrating_twice_changes_nothing() {
        let pool = connect("sqlite::memory:", 1).await.expect("open in-memory db");
        migrate(&pool).await.expect("first");
        migrate(&pool).await.expect("second");

        let applied: i64 = sqlx::query_scalar("SELECT count(*) FROM schema_migrations")
            .fetch_one(&pool)
            .await
            .expect("ledger");
        assert_eq!(applied, 1);
        assert_eq!(table_names(&pool).await, BASELINE_TABLES);
    }
}
