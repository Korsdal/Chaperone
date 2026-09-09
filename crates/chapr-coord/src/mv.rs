// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! Atomic coord-state migration for a rename (concept §6.3).
//!
//! The endpoint has already performed the SMB rename (the file is ground truth,
//! invariant 1); this re-keys coord's view to match, in **one transaction**:
//!
//! - **plain move** (destination was absent): re-key the version-log chain and
//!   any open journal/conflict entries from `src` to `dst`;
//! - **overwrite move** (destination existed, went through CAS): `dst` keeps its
//!   own lineage, so `src`'s coord state is discarded.
//!
//! Then a `Move` version-log entry is appended on `dst` and the move is audited
//! (`write_commit` — `AuditKind` has no dedicated move; the detail records it).
//! All-or-nothing: a failure rolls back and coord is unchanged.
//!
//! ## The move window, and why the intent record makes it recoverable (B3)
//!
//! D-013 accepted that the rename and this migration cannot share a transaction:
//! the file is ground truth (invariant 1), so it moves first, and a failure
//! afterwards leaves coord "stale-but-recoverable". *Recoverable* was aspiration
//! until [`open_move`] existed — nothing recorded that the two paths belonged to
//! one another, so a migration that never ran left `dst` without its lineage and
//! `src`'s rows describing a file that had gone.
//!
//! The record is written before the rename and deleted **inside this function's
//! transaction**. That ordering is what makes the whole thing exactly-once
//! without any idempotency logic: a surviving row *proves* the migration did not
//! commit, so a recovering session can run it without checking whether it
//! already ran. There is no state in which the row and the migration both exist.
//!
//! Coord cannot resolve one of these itself — deciding whether the rename
//! actually happened means hashing the file, and coord does no file I/O
//! (invariant 1, and E-004's "detect and flag, never restore"). So it reports
//! them ([`dangling_moves`]) and the endpoint decides.
//!
//! Named `mv` because `move` is a Rust keyword.

use crate::state::AppState;
use chapr_proto::{
    CanonicalPath, ChaprError, LeaseId, MoveJournalEntry, PreImage, Principal, SessionId,
    VersionToken,
};
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

/// Record the intent to rename `src` → `dst`, before the rename (B3).
///
/// Keyed by `src`, `INSERT OR REPLACE` like [`crate::journal::open`] and for the
/// same reason: the caller holds the all-or-none `{src, dst}` lease, so no *live*
/// entry for either path can be present to clobber. A *dangling* one can be —
/// the endpoint resolves those before opening a new intent on the same paths.
#[allow(clippy::too_many_arguments)]
pub async fn open_move(
    st: &AppState,
    src: &CanonicalPath,
    dst: &CanonicalPath,
    lease_id: &LeaseId,
    principal: &Principal,
    session_id: &SessionId,
    version: &VersionToken,
    size: u64,
    overwrite: bool,
    dst_pre_image: Option<&PreImage>,
) -> Result<(), ChaprError> {
    let _guard = st.acquire_lock.lock().await;
    sqlx::query(
        "INSERT OR REPLACE INTO move_journal
           (src, dst, lease_id, principal, session_id, version, size, overwrite,
            dst_pre_image_version, dst_pre_image_size, opened_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )
    .bind(src.as_str())
    .bind(dst.as_str())
    .bind(lease_id.as_str())
    .bind(principal.as_str())
    .bind(session_id.as_str())
    .bind(version.as_str())
    .bind(size as i64)
    .bind(overwrite as i64)
    .bind(dst_pre_image.map(|p| p.version.as_str().to_string()))
    .bind(dst_pre_image.map(|p| p.size as i64))
    .bind(Utc::now().timestamp_millis())
    .execute(&st.pool)
    .await
    .map_err(|e| ChaprError::Internal {
        message: format!("coord db error: {e}"),
    })?;
    Ok(())
}

/// Drop a move intent without migrating anything — the rename did not happen.
///
/// Distinct from [`move_paths`], which also removes the row: that one removes it
/// *because the migration committed*. This one removes it because there is
/// nothing to migrate, and calling the wrong one of the two would either lose a
/// file's lineage or invent a move that never occurred.
pub async fn clear_move(st: &AppState, src: &CanonicalPath) -> Result<(), ChaprError> {
    let _guard = st.acquire_lock.lock().await;
    sqlx::query("DELETE FROM move_journal WHERE src = ?1")
        .bind(src.as_str())
        .execute(&st.pool)
        .await
        .map_err(|e| ChaprError::Internal {
            message: format!("coord db error: {e}"),
        })?;
    Ok(())
}

/// Every move intent whose owning lease is no longer live — a rename that may
/// have committed with its migration still owed.
///
/// Same shape and same meaning of "dangling" as [`crate::journal::scan_dangling`]:
/// entry present, lease dead. Unlike a write's, a dangling move does **not**
/// imply damaged bytes — the file is intact under one name or the other — which
/// is why this is swept rather than consulted on the read path.
pub async fn dangling_moves(
    pool: &SqlitePool,
    now_ms: i64,
) -> Result<Vec<MoveJournalEntry>, ChaprError> {
    let rows = sqlx::query(
        "SELECT src, dst, lease_id, principal, session_id, version, size, overwrite,
                dst_pre_image_version, dst_pre_image_size, opened_at_ms
         FROM move_journal m
         WHERE NOT EXISTS (
             SELECT 1 FROM leases l
             WHERE l.lease_id = m.lease_id AND l.expiry_ms > ?1 AND l.hard_expiry_ms > ?1
         )",
    )
    .bind(now_ms)
    .fetch_all(pool)
    .await
    .map_err(|e| ChaprError::Internal {
        message: format!("coord db error: {e}"),
    })?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let src: String = row.get("src");
        let Some(version) = VersionToken::from_hex(row.get::<String, _>("version")) else {
            // A stored token is always valid, so an unparseable one means a
            // corrupt row. Skipping is the only safe move: a fabricated version
            // would make recovery conclude the rename never happened and drop a
            // migration that is genuinely owed.
            tracing::warn!(%src, "skipping move_journal row with unparseable version");
            continue;
        };
        // The two pre-image columns are written together or not at all; a row
        // with one of them is corrupt, and treating it as absent would let GC
        // reclaim the snapshot. Skip, so a human sees it in the dangling report.
        let dst_pre_image = match (
            row.get::<Option<String>, _>("dst_pre_image_version"),
            row.get::<Option<i64>, _>("dst_pre_image_size"),
        ) {
            (Some(hex), Some(size)) => match VersionToken::from_hex(hex) {
                Some(version) => Some(PreImage {
                    version,
                    size: size as u64,
                }),
                None => {
                    tracing::warn!(%src, "skipping move_journal row with unparseable pre-image");
                    continue;
                }
            },
            (None, None) => None,
            _ => {
                tracing::warn!(%src, "skipping move_journal row with a half-written pre-image");
                continue;
            }
        };

        out.push(MoveJournalEntry {
            src: CanonicalPath::new_unchecked(src),
            dst: CanonicalPath::new_unchecked(row.get::<String, _>("dst")),
            lease_id: LeaseId::new_unchecked(row.get::<String, _>("lease_id")),
            principal: Principal::new_unchecked(row.get::<String, _>("principal")),
            session_id: SessionId::new_unchecked(row.get::<String, _>("session_id")),
            version,
            size: row.get::<i64, _>("size") as u64,
            overwrite: row.get::<i64, _>("overwrite") != 0,
            dst_pre_image,
            opened_at: DateTime::<Utc>::from_timestamp_millis(row.get::<i64, _>("opened_at_ms"))
                .unwrap_or_default(),
        });
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
pub async fn move_paths(
    st: &AppState,
    src: &CanonicalPath,
    dst: &CanonicalPath,
    version: &VersionToken,
    size: u64,
    overwrite: bool,
    principal: &Principal,
    session_id: &SessionId,
    dst_pre_image: Option<&chapr_proto::PreImage>,
) -> Result<(), ChaprError> {
    let _guard = st.acquire_lock.lock().await;
    let now_ms = Utc::now().timestamp_millis();
    let db = |e: sqlx::Error| ChaprError::Internal {
        message: format!("coord db error: {e}"),
    };

    let mut tx = st.pool.begin().await.map_err(db)?;

    if overwrite {
        // Destination keeps its own history/journal/conflicts; src is consumed.
        for sql in [
            "DELETE FROM version_log WHERE path = ?1",
            "DELETE FROM journal WHERE path = ?1",
            "DELETE FROM conflicts WHERE base_path = ?1",
        ] {
            sqlx::query(sql).bind(src.as_str()).execute(&mut *tx).await.map_err(db)?;
        }
    } else {
        // Plain move: re-key src's coord state onto dst.
        sqlx::query("UPDATE version_log SET path = ?2 WHERE path = ?1")
            .bind(src.as_str()).bind(dst.as_str()).execute(&mut *tx).await.map_err(db)?;
        sqlx::query("UPDATE journal SET path = ?2 WHERE path = ?1")
            .bind(src.as_str()).bind(dst.as_str()).execute(&mut *tx).await.map_err(db)?;
        sqlx::query("UPDATE conflicts SET base_path = ?2 WHERE base_path = ?1")
            .bind(src.as_str()).bind(dst.as_str()).execute(&mut *tx).await.map_err(db)?;
    }

    // An overwrite-move destroys the destination's bytes, and the endpoint
    // snapshotted them before renaming. The `move` entry below names the *source*
    // version as dst's new head, so without a baseline entry that blob is
    // referenced by nothing and GC reclaims the only copy of what was replaced.
    // Recorded before the move entry so the chain reads in true order.
    if let Some(pre) = dst_pre_image {
        let already_named =
            sqlx::query("SELECT 1 FROM version_log WHERE path = ?1 AND blob_hash = ?2 LIMIT 1")
                .bind(dst.as_str())
                .bind(pre.version.as_str())
                .fetch_optional(&mut *tx)
                .await
                .map_err(db)?
                .is_some();
        if !already_named {
            let head: Option<String> = sqlx::query(
                "SELECT blob_hash FROM version_log WHERE path = ?1 ORDER BY id DESC LIMIT 1",
            )
            .bind(dst.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?
            .map(|r| r.get::<String, _>("blob_hash"));
            sqlx::query(
                "INSERT INTO version_log
                   (path, timestamp_ms, blob_hash, writer_principal, prev_hash, size, event)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'baseline')",
            )
            .bind(dst.as_str())
            .bind(now_ms)
            .bind(pre.version.as_str())
            .bind(crate::history::BASELINE_PRINCIPAL)
            .bind(head)
            .bind(pre.size as i64)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        }
    }

    // Append the Move entry, chained to dst's current head.
    let prev: Option<String> =
        sqlx::query("SELECT blob_hash FROM version_log WHERE path = ?1 ORDER BY id DESC LIMIT 1")
            .bind(dst.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?
            .map(|r| r.get::<String, _>("blob_hash"));
    sqlx::query(
        "INSERT INTO version_log
           (path, timestamp_ms, blob_hash, writer_principal, prev_hash, size, event)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'move')",
    )
    .bind(dst.as_str())
    .bind(now_ms)
    .bind(version.as_str())
    .bind(principal.as_str())
    .bind(prev)
    .bind(size as i64)
    .execute(&mut *tx)
    .await
    .map_err(db)?;

    // Audit the move.
    sqlx::query(
        "INSERT INTO audit_log
           (event_id, timestamp_ms, principal, session_id, canonical_path,
            kind, from_version, to_version, detail)
         VALUES (?1, ?2, ?3, ?4, ?5, 'write_commit', NULL, ?6, ?7)",
    )
    .bind(format!("evt-{}", Uuid::new_v4()))
    .bind(now_ms)
    .bind(principal.as_str())
    .bind(session_id.as_str())
    .bind(dst.as_str())
    .bind(version.as_str())
    .bind(format!("move from {src}"))
    .execute(&mut *tx)
    .await
    .map_err(db)?;

    // The move intent is discharged here, in the same transaction as the
    // migration it describes (B3). Deleting it anywhere else — a second call
    // after the commit, say — would create a window where both the row and the
    // migration exist, and a recovering session that saw that row would run the
    // migration a second time. Inside the transaction there is no such window:
    // the row survives if and only if the migration did not.
    //
    // No-op for a caller that never opened one (an older endpoint against this
    // coordinator), which is why nothing here checks that a row was removed.
    sqlx::query("DELETE FROM move_journal WHERE src = ?1")
        .bind(src.as_str())
        .execute(&mut *tx)
        .await
        .map_err(db)?;

    tx.commit().await.map_err(db)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{audit, conflict, db, history, lease};
    use chapr_proto::{AuditKind, LeasePurpose, VersionEvent};

    fn src() -> CanonicalPath {
        CanonicalPath::new_unchecked("\\\\srv\\share\\old.md")
    }
    fn dst() -> CanonicalPath {
        CanonicalPath::new_unchecked("\\\\srv\\share\\new.md")
    }
    fn who() -> Principal {
        Principal::new_unchecked("CONTOSO\\demo")
    }
    fn sess() -> SessionId {
        SessionId::new_unchecked("sess-1")
    }

    fn now_ms() -> i64 {
        Utc::now().timestamp_millis()
    }

    /// Open a move intent under a real, live dual lease — the arrangement the
    /// endpoint actually produces.
    async fn open_intent(st: &AppState, overwrite: bool, pre: Option<PreImage>) -> LeaseId {
        let granted = lease::acquire(
            st,
            who(),
            sess(),
            LeasePurpose::Move,
            vec![src(), dst()],
        )
        .await
        .unwrap();
        open_move(
            st,
            &src(),
            &dst(),
            &granted.lease_id,
            &who(),
            &sess(),
            &VersionToken::hash(b"moved"),
            5,
            overwrite,
            pre.as_ref(),
        )
        .await
        .unwrap();
        granted.lease_id
    }

    async fn intent_rows(st: &AppState) -> i64 {
        sqlx::query("SELECT COUNT(*) AS n FROM move_journal")
            .fetch_one(&st.pool)
            .await
            .unwrap()
            .get::<i64, _>("n")
    }

    /// The property the whole design rests on: a surviving intent row proves the
    /// migration did not commit, because the migration deletes it in its own
    /// transaction. If these two could both be true, a recovering session would
    /// re-run a migration that had already happened.
    #[tokio::test]
    async fn the_migration_discharges_the_intent_it_describes() {
        let st = AppState::new(db::test_pool().await);
        open_intent(&st, false, None).await;
        assert_eq!(intent_rows(&st).await, 1);

        move_paths(
            &st,
            &src(),
            &dst(),
            &VersionToken::hash(b"moved"),
            5,
            false,
            &who(),
            &sess(),
            None,
        )
        .await
        .unwrap();

        assert_eq!(intent_rows(&st).await, 0, "the migration left its intent behind");
    }

    /// A move still in flight must not be reported: its lease is alive, so some
    /// session is mid-rename and completing it from elsewhere would race.
    #[tokio::test]
    async fn an_intent_is_dangling_only_once_its_lease_dies() {
        let st = AppState::new(db::test_pool().await);
        let lease_id = open_intent(&st, false, None).await;
        assert!(
            dangling_moves(&st.pool, now_ms()).await.unwrap().is_empty(),
            "a live move was reported as needing recovery"
        );

        lease::release(&st, &lease_id).await.unwrap();
        let stale = dangling_moves(&st.pool, now_ms()).await.unwrap();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].src, src());
        assert_eq!(stale[0].dst, dst());
        assert_eq!(stale[0].version, VersionToken::hash(b"moved"));
        assert_eq!(stale[0].size, 5);
        assert!(!stale[0].overwrite);
        assert!(stale[0].dst_pre_image.is_none());
    }

    /// An overwrite intent has to carry the snapshot forward, because the blob is
    /// referenced by nothing until a `baseline` entry names it and GC would
    /// otherwise reclaim the only copy of what the rename destroyed.
    #[tokio::test]
    async fn an_overwrite_intent_carries_the_destinations_pre_image() {
        let st = AppState::new(db::test_pool().await);
        let pre = PreImage {
            version: VersionToken::hash(b"dst-old"),
            size: 7,
        };
        let lease_id = open_intent(&st, true, Some(pre.clone())).await;
        lease::release(&st, &lease_id).await.unwrap();

        let stale = dangling_moves(&st.pool, now_ms()).await.unwrap();
        assert_eq!(stale.len(), 1);
        assert!(stale[0].overwrite);
        assert_eq!(stale[0].dst_pre_image, Some(pre));
    }

    /// The other half of the two-outcome split: clearing must not look like a
    /// move that happened. Nothing is appended, nothing is re-keyed.
    #[tokio::test]
    async fn clearing_an_intent_migrates_nothing() {
        let st = AppState::new(db::test_pool().await);
        let v1 = VersionToken::hash(b"v1");
        history::append_version_log(&st, &src(), &v1, &who(), 2, VersionEvent::Create, None)
            .await
            .unwrap();
        open_intent(&st, false, None).await;

        clear_move(&st, &src()).await.unwrap();

        assert_eq!(intent_rows(&st).await, 0);
        // src keeps its own history: the file never went anywhere.
        assert_eq!(history::history(&st.pool, &src()).await.unwrap().entries.len(), 1);
        assert!(history::history(&st.pool, &dst()).await.unwrap().entries.is_empty());
        // Not "no audit events on dst" — acquiring the dual lease legitimately
        // audits one on each path. What must be absent is a *move*.
        assert!(
            !audit::query(&st.pool, &dst())
                .await
                .unwrap()
                .iter()
                .any(|e| e.detail.contains("move from")),
            "clearing an intent audited a move that never happened"
        );
    }

    /// A half-written pre-image is corruption, and reporting it as "no
    /// pre-image" would invite recovery to complete the move without naming the
    /// snapshot — after which GC reclaims the destination's last copy. Skipping
    /// leaves the row for a human instead.
    #[tokio::test]
    async fn a_row_with_half_a_pre_image_is_skipped_not_guessed() {
        let st = AppState::new(db::test_pool().await);
        let lease_id = open_intent(
            &st,
            true,
            Some(PreImage {
                version: VersionToken::hash(b"dst-old"),
                size: 7,
            }),
        )
        .await;
        lease::release(&st, &lease_id).await.unwrap();
        sqlx::query("UPDATE move_journal SET dst_pre_image_size = NULL WHERE src = ?1")
            .bind(src().as_str())
            .execute(&st.pool)
            .await
            .unwrap();

        assert!(dangling_moves(&st.pool, now_ms()).await.unwrap().is_empty());
        assert_eq!(intent_rows(&st).await, 1, "the corrupt row must still be there");
    }

    #[tokio::test]
    async fn plain_move_rekeys_history_and_conflicts() {
        let st = AppState::new(db::test_pool().await);
        let v1 = VersionToken::hash(b"v1");
        history::append_version_log(&st, &src(), &v1, &who(), 2, VersionEvent::Create, None)
            .await
            .unwrap();
        conflict::register(
            &st,
            &src(),
            &CanonicalPath::new_unchecked("\\\\srv\\share\\old.conflict-x.md"),
            &who(),
            &sess(),
        )
        .await
        .unwrap();

        move_paths(&st, &src(), &dst(), &v1, 2, false, &who(), &sess(), None)
            .await
            .unwrap();

        // src has nothing; dst has the migrated chain + a Move entry.
        assert!(history::history(&st.pool, &src()).await.unwrap().entries.is_empty());
        let dst_hist = history::history(&st.pool, &dst()).await.unwrap();
        assert_eq!(dst_hist.entries.len(), 2); // create (migrated) + move
        assert_eq!(dst_hist.entries[0].event, VersionEvent::Move);
        assert_eq!(dst_hist.entries[1].event, VersionEvent::Create);

        // The open conflict moved to dst.
        assert_eq!(conflict::count_open(&st.pool, &src()).await.unwrap(), 0);
        assert_eq!(conflict::count_open(&st.pool, &dst()).await.unwrap(), 1);

        // Move was audited on dst.
        let events = audit::query(&st.pool, &dst()).await.unwrap();
        assert!(events.iter().any(|e| e.kind == AuditKind::WriteCommit
            && e.detail.contains("move from")));
    }

    #[tokio::test]
    async fn overwrite_move_discards_src_and_keeps_dst_lineage() {
        let st = AppState::new(db::test_pool().await);
        let sv = VersionToken::hash(b"src");
        let dv = VersionToken::hash(b"dst-old");
        history::append_version_log(&st, &src(), &sv, &who(), 3, VersionEvent::Create, None)
            .await
            .unwrap();
        history::append_version_log(&st, &dst(), &dv, &who(), 4, VersionEvent::Create, None)
            .await
            .unwrap();

        move_paths(&st, &src(), &dst(), &sv, 3, true, &who(), &sess(), None)
            .await
            .unwrap();

        assert!(history::history(&st.pool, &src()).await.unwrap().entries.is_empty());
        // dst keeps its own create + the Move entry (src's chain discarded).
        let dst_hist = history::history(&st.pool, &dst()).await.unwrap();
        assert_eq!(dst_hist.entries.len(), 2);
        assert_eq!(dst_hist.entries[0].event, VersionEvent::Move);
        assert_eq!(dst_hist.entries[1].event, VersionEvent::Create);
        assert_eq!(dst_hist.entries[1].version, dv); // dst's own lineage, not src's
    }
}
