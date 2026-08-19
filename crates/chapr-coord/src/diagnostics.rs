// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! The diagnostics store (E-026) — grouped operational failures.
//!
//! The rationale for keeping this separate from the audit log lives on the proto
//! types ([`chapr_proto::diagnostics`]). In short: audit answers *who did what*
//! and is a primary deliverable; this answers *why something broke*, for a
//! different reader and a different retention. Only unexpected failures arrive
//! here — a CAS conflict and an Office lock are designed outcomes and surface as
//! conflict and lease status instead.
//!
//! Grouping by `(code, path)` is the load-bearing choice. The failure worth
//! finding is usually the one repeating, and without grouping one agent looping on
//! a misconfigured drive letter buries every other entry.
//!
//! Lock-free: [`record`] is a single upsert plus an insert, and the group key is a
//! unique index, so two endpoints reporting the same failure at the same instant
//! converge on one row rather than racing to create two.

use crate::state::AppState;
use chapr_proto::{
    ChaprError, DiagnosticGroup, DiagnosticOccurrence, DiagnosticReport, DiagnosticState,
    DiagnosticsQuery, DiagnosticsResponse, Principal, Severity,
};
use chrono::{DateTime, TimeZone, Utc};
use sqlx::{Row, SqlitePool};
use std::collections::BTreeMap;
use uuid::Uuid;

/// How many recent occurrences to keep per group.
///
/// The group's `count` is the real history; this is the evidence an administrator
/// looks at ("is it always the same user?"). Trimmed so a persistent fault cannot
/// grow the table without bound.
const MAX_OCCURRENCES_PER_GROUP: i64 = 20;

/// Default cap on a query, when the caller does not give one.
const DEFAULT_QUERY_LIMIT: u32 = 100;

/// Record one occurrence, folding it into its `(code, path)` group.
///
/// Later reports refresh `title`/`detail`/`remedy`/`severity` rather than being
/// dropped: the newest sighting carries the most relevant OS message, and an
/// endpoint that has been upgraded to describe a fault better should win over the
/// row an older build wrote.
pub async fn record(
    st: &AppState,
    report: &DiagnosticReport,
) -> Result<DiagnosticGroup, ChaprError> {
    let now = Utc::now();
    let now_ms = now.timestamp_millis();
    // SQLite cannot key a unique index across NULL, so the group key stores '' for
    // "no path" while the column itself keeps NULL for a faithful read-back.
    let path_key = report.path.as_ref().map(|p| p.as_str()).unwrap_or("");
    let facts_json = serde_json::to_string(&report.facts).map_err(|e| ChaprError::Internal {
        message: format!("cannot serialise diagnostic facts: {e}"),
    })?;

    let mut tx = st.pool.begin().await.map_err(internal)?;

    let existing: Option<String> =
        sqlx::query("SELECT id FROM diagnostics WHERE code = ?1 AND IFNULL(path, '') = ?2")
            .bind(&report.code)
            .bind(path_key)
            .fetch_optional(&mut *tx)
            .await
            .map_err(internal)?
            .map(|r| r.get("id"));

    let group_id = match existing {
        Some(id) => {
            sqlx::query(
                "UPDATE diagnostics
                    SET last_seen_ms = ?1, count = count + 1,
                        title = ?2, severity = ?3, detail = ?4, remedy = ?5, facts_json = ?6
                  WHERE id = ?7",
            )
            .bind(now_ms)
            .bind(&report.title)
            .bind(severity_str(report.severity))
            .bind(&report.detail)
            .bind(&report.remedy)
            .bind(&facts_json)
            .bind(&id)
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
            id
        }
        None => {
            let id = format!("diag-{}", Uuid::new_v4());
            sqlx::query(
                "INSERT INTO diagnostics
                   (id, code, path, title, severity, detail, remedy, facts_json,
                    state, first_seen_ms, last_seen_ms, count)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'open', ?9, ?9, 1)",
            )
            .bind(&id)
            .bind(&report.code)
            .bind(report.path.as_ref().map(|p| p.as_str()))
            .bind(&report.title)
            .bind(severity_str(report.severity))
            .bind(&report.detail)
            .bind(&report.remedy)
            .bind(&facts_json)
            .bind(now_ms)
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
            id
        }
    };

    sqlx::query(
        "INSERT INTO diagnostic_occurrences (group_id, at_ms, principal, host)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(&group_id)
    .bind(now_ms)
    .bind(report.principal.as_str())
    .bind(report.host.as_deref())
    .execute(&mut *tx)
    .await
    .map_err(internal)?;

    // Trim in the same transaction, so the table never sits over the cap even
    // briefly. `rowid` breaks ties for occurrences recorded in the same
    // millisecond, which a retry loop produces routinely.
    sqlx::query(
        "DELETE FROM diagnostic_occurrences
          WHERE group_id = ?1
            AND rowid NOT IN (
                SELECT rowid FROM diagnostic_occurrences
                 WHERE group_id = ?1
                 ORDER BY at_ms DESC, rowid DESC
                 LIMIT ?2
            )",
    )
    .bind(&group_id)
    .bind(MAX_OCCURRENCES_PER_GROUP)
    .execute(&mut *tx)
    .await
    .map_err(internal)?;

    tx.commit().await.map_err(internal)?;

    load_group(&st.pool, &group_id).await
}

/// Query groups, most recently active first.
pub async fn query(
    pool: &SqlitePool,
    q: &DiagnosticsQuery,
) -> Result<DiagnosticsResponse, ChaprError> {
    let limit = q.limit.unwrap_or(DEFAULT_QUERY_LIMIT) as i64;

    // States are a closed enum, so mapping them to literals is safe and avoids
    // building a dynamic IN-list with bind placeholders.
    let state_filter = if q.states.is_empty() {
        String::new()
    } else {
        let list = q
            .states
            .iter()
            .map(|s| format!("'{}'", state_str(*s)))
            .collect::<Vec<_>>()
            .join(",");
        format!(" AND state IN ({list})")
    };
    let sql = format!(
        "SELECT id FROM diagnostics
          WHERE (?1 IS NULL OR code = ?1){state_filter}
          ORDER BY last_seen_ms DESC
          LIMIT ?2"
    );

    let ids: Vec<String> = sqlx::query(&sql)
        .bind(q.code.as_deref())
        .bind(limit)
        .fetch_all(pool)
        .await
        .map_err(internal)?
        .into_iter()
        .map(|r| r.get("id"))
        .collect();

    let mut groups = Vec::with_capacity(ids.len());
    for id in ids {
        groups.push(load_group(pool, &id).await?);
    }
    Ok(DiagnosticsResponse { groups })
}

/// Count open groups by severity, for the admin overview's stat tiles.
///
/// A dedicated count rather than reusing [`query`]: that loads each group's
/// occurrences with a query per group, which is the wrong cost for a number
/// rendered on every page load.
pub async fn count_open(pool: &SqlitePool) -> Result<(i64, i64), ChaprError> {
    let row = sqlx::query(
        "SELECT
           SUM(CASE WHEN severity = 'error'   THEN 1 ELSE 0 END) AS errors,
           SUM(CASE WHEN severity = 'warning' THEN 1 ELSE 0 END) AS warnings
         FROM diagnostics WHERE state = 'open'",
    )
    .fetch_one(pool)
    .await
    .map_err(internal)?;
    // SUM over no rows is NULL, not 0.
    Ok((
        row.get::<Option<i64>, _>("errors").unwrap_or(0),
        row.get::<Option<i64>, _>("warnings").unwrap_or(0),
    ))
}

/// Read one group plus its retained occurrences.
async fn load_group(pool: &SqlitePool, id: &str) -> Result<DiagnosticGroup, ChaprError> {
    let row = sqlx::query(
        "SELECT id, code, path, title, severity, detail, remedy, facts_json,
                state, first_seen_ms, last_seen_ms, count
           FROM diagnostics WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(internal)?
    .ok_or_else(|| ChaprError::Internal {
        message: format!("diagnostic group {id} vanished mid-read"),
    })?;

    let facts_json: String = row.get("facts_json");
    let facts: BTreeMap<String, String> = serde_json::from_str(&facts_json).unwrap_or_default();

    let occurrences = sqlx::query(
        "SELECT at_ms, principal, host FROM diagnostic_occurrences
          WHERE group_id = ?1 ORDER BY at_ms DESC, rowid DESC",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .map_err(internal)?
    .into_iter()
    .map(|r| DiagnosticOccurrence {
        at: ms_to_dt(r.get::<i64, _>("at_ms")),
        principal: Principal::new_unchecked(r.get::<String, _>("principal")),
        host: r.get::<Option<String>, _>("host"),
    })
    .collect();

    Ok(DiagnosticGroup {
        id: row.get("id"),
        code: row.get("code"),
        title: row.get("title"),
        severity: severity_from(&row.get::<String, _>("severity")),
        path: row
            .get::<Option<String>, _>("path")
            .map(chapr_proto::CanonicalPath::new_unchecked),
        detail: row.get("detail"),
        remedy: row.get("remedy"),
        facts,
        state: state_from(&row.get::<String, _>("state")),
        first_seen: ms_to_dt(row.get::<i64, _>("first_seen_ms")),
        last_seen: ms_to_dt(row.get::<i64, _>("last_seen_ms")),
        count: row.get("count"),
        occurrences,
    })
}

fn severity_str(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
    }
}

fn severity_from(s: &str) -> Severity {
    match s {
        "warning" => Severity::Warning,
        // An unknown value is more likely a newer peer than corruption, and the
        // safe direction is to over-report rather than hide it.
        _ => Severity::Error,
    }
}

fn state_str(s: DiagnosticState) -> &'static str {
    match s {
        DiagnosticState::Open => "open",
        DiagnosticState::Acknowledged => "acknowledged",
        DiagnosticState::Resolved => "resolved",
    }
}

fn state_from(s: &str) -> DiagnosticState {
    match s {
        "acknowledged" => DiagnosticState::Acknowledged,
        "resolved" => DiagnosticState::Resolved,
        _ => DiagnosticState::Open,
    }
}

fn ms_to_dt(ms: i64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(ms).single().unwrap_or_else(Utc::now)
}

fn internal(e: sqlx::Error) -> ChaprError {
    ChaprError::Internal {
        message: e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use chapr_proto::CanonicalPath;

    fn report(code: &str, path: Option<&str>, who: &str) -> DiagnosticReport {
        let mut facts = BTreeMap::new();
        facts.insert("os_error".to_string(), "32".to_string());
        DiagnosticReport {
            code: code.to_string(),
            title: "Could not open the file exclusively".into(),
            severity: Severity::Error,
            path: path.map(CanonicalPath::new_unchecked),
            principal: Principal::new_unchecked(who),
            host: Some("LAPTOP-04".into()),
            detail: "sharing violation".into(),
            remedy: "Check for antivirus scanning the share.".into(),
            facts,
        }
    }

    async fn state() -> AppState {
        AppState::new(db::test_pool().await)
    }

    #[tokio::test]
    async fn a_first_report_creates_an_open_group() {
        let st = state().await;
        let g = record(&st, &report("IO", Some("\\\\srv\\share\\a.md"), "CONTOSO\\a"))
            .await
            .unwrap();
        assert_eq!(g.count, 1);
        assert_eq!(g.state, DiagnosticState::Open);
        assert_eq!(g.occurrences.len(), 1);
        assert_eq!(g.facts.get("os_error").map(String::as_str), Some("32"));
        assert_eq!(g.first_seen, g.last_seen);
    }

    #[tokio::test]
    async fn repeats_of_one_failure_group_instead_of_multiplying_rows() {
        // The property the schema exists for: a looping agent must not bury
        // everything else under identical rows.
        let st = state().await;
        for _ in 0..5 {
            record(&st, &report("IO", Some("\\\\srv\\share\\a.md"), "CONTOSO\\a"))
                .await
                .unwrap();
        }
        let out = query(&st.pool, &DiagnosticsQuery::default()).await.unwrap();
        assert_eq!(out.groups.len(), 1, "five reports must be one group");
        assert_eq!(out.groups[0].count, 5);
        assert_eq!(out.groups[0].occurrences.len(), 5);
    }

    #[tokio::test]
    async fn the_same_code_on_a_different_path_is_a_different_group() {
        let st = state().await;
        record(&st, &report("IO", Some("\\\\srv\\share\\a.md"), "CONTOSO\\a"))
            .await
            .unwrap();
        record(&st, &report("IO", Some("\\\\srv\\share\\b.md"), "CONTOSO\\a"))
            .await
            .unwrap();
        let out = query(&st.pool, &DiagnosticsQuery::default()).await.unwrap();
        assert_eq!(out.groups.len(), 2);
    }

    #[tokio::test]
    async fn an_endpoint_wide_failure_groups_without_a_path() {
        // `CoordUnreachable` has no path. A unique index cannot span NULL in
        // SQLite, so this is the case the '' group key exists for — two reports
        // must still converge on one group, and read back with `path: None`.
        let st = state().await;
        record(&st, &report("COORD_UNREACHABLE", None, "CONTOSO\\a"))
            .await
            .unwrap();
        let g = record(&st, &report("COORD_UNREACHABLE", None, "CONTOSO\\b"))
            .await
            .unwrap();
        assert_eq!(g.count, 2, "pathless reports must group");
        assert_eq!(g.path, None, "'' must not leak back as a path");
    }

    #[tokio::test]
    async fn occurrences_are_capped_but_the_count_keeps_counting() {
        let st = state().await;
        let n = MAX_OCCURRENCES_PER_GROUP + 7;
        for _ in 0..n {
            record(&st, &report("IO", Some("\\\\srv\\share\\a.md"), "CONTOSO\\a"))
                .await
                .unwrap();
        }
        let out = query(&st.pool, &DiagnosticsQuery::default()).await.unwrap();
        let g = &out.groups[0];
        assert_eq!(g.count, n, "the count is the real history");
        assert_eq!(
            g.occurrences.len() as i64,
            MAX_OCCURRENCES_PER_GROUP,
            "retained evidence must stay bounded"
        );
    }

    #[tokio::test]
    async fn occurrences_record_who_and_where_so_one_bad_laptop_is_visible() {
        let st = state().await;
        record(&st, &report("IO", Some("\\\\srv\\share\\a.md"), "CONTOSO\\a"))
            .await
            .unwrap();
        let g = record(&st, &report("IO", Some("\\\\srv\\share\\a.md"), "CONTOSO\\b"))
            .await
            .unwrap();
        let who: Vec<&str> = g.occurrences.iter().map(|o| o.principal.as_str()).collect();
        assert!(who.contains(&"CONTOSO\\a") && who.contains(&"CONTOSO\\b"));
        assert_eq!(g.occurrences[0].host.as_deref(), Some("LAPTOP-04"));
    }

    #[tokio::test]
    async fn a_later_report_refreshes_the_description() {
        // An endpoint upgraded to describe a fault better should win over the row
        // an older build wrote.
        let st = state().await;
        record(&st, &report("IO", Some("\\\\srv\\share\\a.md"), "CONTOSO\\a"))
            .await
            .unwrap();
        let mut better = report("IO", Some("\\\\srv\\share\\a.md"), "CONTOSO\\a");
        better.remedy = "Exclude the share from Defender's real-time scanning.".into();
        better.severity = Severity::Warning;
        let g = record(&st, &better).await.unwrap();
        assert!(g.remedy.contains("Defender"));
        assert_eq!(g.severity, Severity::Warning);
    }

    #[tokio::test]
    async fn query_filters_by_code_and_respects_the_limit() {
        let st = state().await;
        record(&st, &report("IO", Some("\\\\srv\\share\\a.md"), "CONTOSO\\a"))
            .await
            .unwrap();
        record(
            &st,
            &report("PERMISSION_DENIED", Some("\\\\srv\\share\\b.md"), "CONTOSO\\a"),
        )
        .await
        .unwrap();

        let only_io = query(
            &st.pool,
            &DiagnosticsQuery {
                code: Some("IO".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(only_io.groups.len(), 1);
        assert_eq!(only_io.groups[0].code, "IO");

        let capped = query(
            &st.pool,
            &DiagnosticsQuery {
                limit: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(capped.groups.len(), 1);
    }

    #[tokio::test]
    async fn query_filters_by_state() {
        let st = state().await;
        record(&st, &report("IO", Some("\\\\srv\\share\\a.md"), "CONTOSO\\a"))
            .await
            .unwrap();
        let open = query(
            &st.pool,
            &DiagnosticsQuery {
                states: vec![DiagnosticState::Open],
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(open.groups.len(), 1);
        let resolved = query(
            &st.pool,
            &DiagnosticsQuery {
                states: vec![DiagnosticState::Resolved],
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(resolved.groups.is_empty());
    }
}
