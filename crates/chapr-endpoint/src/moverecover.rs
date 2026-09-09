// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! Finishing a move that was interrupted between the rename and the migration
//! (B3, D-013).
//!
//! ## The window, and what is actually at stake in it
//!
//! `chapr.move` renames the file first — the file is ground truth (invariant 1)
//! — and then asks coord to re-key its state from `src` to `dst`. Those two
//! cannot share a transaction, which D-013 accepted explicitly, calling what the
//! gap leaves "stale-but-recoverable".
//!
//! Nothing is torn here and no bytes are at risk: the file is intact, under one
//! name or the other. What is at risk is everything coord knows *about* it. Left
//! unfinished, `dst` loses its version chain and its open conflicts, `src`'s rows
//! describe a file that is no longer there, and — on an overwrite-move — the
//! snapshot of the bytes the rename destroyed is referenced by nothing, so GC
//! reclaims the only copy. That last one is the same failure I-016 describes,
//! reached by a different route.
//!
//! ## Why the endpoint does this and coord cannot
//!
//! Deciding whether the rename happened means hashing a file, and coord does no
//! file I/O at all — E-004's rule is "detect and flag, never restore", and this
//! is the same division of labour as a dangling write: coord reports
//! ([`CoordClient::dangling_moves`]), the endpoint acts.
//!
//! ## The decision is decidable, which is the point
//!
//! Every recorded intent carries `src`, `dst` and the source's CAS-verified
//! version, so the filesystem answers the question with no guessing:
//!
//! | On disk | Conclusion | Action |
//! |---|---|---|
//! | `src` gone, `dst` hashes to `version` | the rename committed | complete the migration |
//! | `src` present | it did not | drop the intent; nothing happened |
//! | neither, or `dst` differs | something else has been here | leave it, and say so |
//!
//! The third row is the one worth being careful about. A `dst` whose content is
//! *not* the recorded version means someone wrote to it after the move, so the
//! rename did commit but the file has moved on; completing the migration would
//! append a version-log entry naming a version the file no longer has. Refusing
//! is the bounded-retry rule applied to bookkeeping: reach a terminal state and
//! surface it, rather than retry forever or improvise.
//!
//! Ordering inside the completing call is deliberate too: coord deletes the
//! intent row *inside* the migration's own transaction, so completing is
//! exactly-once by construction. A row that still exists proves the migration
//! did not commit.

use crate::backend::Backend;
use crate::canon::{coordinated_roots, is_within_roots};
use crate::pathgrammar::grammar_for;
use crate::CoordClient;
use chapr_proto::{
    ChaprError, ClearMoveJournalRequest, MoveJournalEntry, MovePathsRequest, Principal, SessionId,
    VersionToken,
};

/// What resolving one interrupted move concluded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The rename had committed; coord's state now matches the share.
    Completed,
    /// The rename had not happened. The intent was dropped and nothing moved.
    Abandoned,
    /// Neither conclusion is safe. The intent is left in place, deliberately, so
    /// it stays visible in coord's startup scan instead of being resolved wrongly
    /// and silently.
    NeedsHuman(String),
    /// Outside this endpoint's coordinated roots (E-025), so not ours to touch —
    /// another endpoint's, or a misconfiguration. Left alone.
    NotOurs,
}

/// A summary of one sweep, for the caller to log.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub completed: usize,
    pub abandoned: usize,
    pub needs_human: Vec<(MoveJournalEntry, String)>,
    pub not_ours: usize,
    /// Entries whose resolution failed with an error (coord unreachable
    /// mid-sweep, an unreadable file). Left for the next sweep — a failure here
    /// is never terminal, because the intent record survives it.
    pub failed: Vec<(MoveJournalEntry, ChaprError)>,
}

impl Report {
    /// Did the sweep find anything at all? Used to keep the quiet case quiet.
    pub fn is_empty(&self) -> bool {
        self.completed == 0
            && self.abandoned == 0
            && self.needs_human.is_empty()
            && self.not_ours == 0
            && self.failed.is_empty()
    }
}

/// Resolve every interrupted move coord is holding, and report what happened.
///
/// Safe to run concurrently with live moves: coord only reports intents whose
/// lease is **dead**, so a move still in flight is never touched. Safe to run
/// repeatedly: completing removes the intent in the migration's transaction, and
/// a failure leaves the intent for the next attempt.
pub async fn sweep(
    coord: &CoordClient,
    backend: &dyn Backend,
    principal: &Principal,
    session_id: &SessionId,
) -> Result<Report, ChaprError> {
    let stale = coord.dangling_moves().await?.entries;
    let mut report = Report::default();
    for entry in stale {
        match resolve_one(coord, backend, principal, session_id, &entry).await {
            Ok(Outcome::Completed) => report.completed += 1,
            Ok(Outcome::Abandoned) => report.abandoned += 1,
            Ok(Outcome::NotOurs) => report.not_ours += 1,
            Ok(Outcome::NeedsHuman(why)) => report.needs_human.push((entry, why)),
            Err(e) => report.failed.push((entry, e)),
        }
    }
    Ok(report)
}

/// Resolve one interrupted move. See the module table for the three conclusions.
pub async fn resolve_one(
    coord: &CoordClient,
    backend: &dyn Backend,
    principal: &Principal,
    session_id: &SessionId,
    entry: &MoveJournalEntry,
) -> Result<Outcome, ChaprError> {
    let sep = grammar_for(backend.kind()).sep();
    let roots = coordinated_roots();
    // Both paths must be ours. Checking both rather than either: completing a
    // move whose destination lies outside the coordinated root would write coord
    // state about a file this endpoint has no business with.
    if !is_within_roots(entry.src.as_str(), roots, sep)
        || !is_within_roots(entry.dst.as_str(), roots, sep)
    {
        return Ok(Outcome::NotOurs);
    }

    let fs = backend.as_file_source();
    let src_exists = fs.stat(&entry.src).is_ok();
    if src_exists {
        // The rename never went through: the source is still where it was. Drop
        // the intent — there is no migration owed, and leaving the row would keep
        // coord reporting a pending move indefinitely.
        coord
            .clear_move(&ClearMoveJournalRequest {
                src: entry.src.clone(),
            })
            .await?;
        return Ok(Outcome::Abandoned);
    }

    // `src` is gone. Either the rename committed, or something else removed it.
    // Reading `dst` answers which, and the recorded version is what makes the
    // answer trustworthy — the file's *name* being right is not evidence.
    let dst_bytes = match fs.read(&entry.dst) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Outcome::NeedsHuman(format!(
                "neither {} nor {} exists; the file is not where either name says",
                entry.src, entry.dst
            )));
        }
        Err(e) => return Err(crate::backend::map_os_err(&entry.dst, e)),
    };
    let dst_now = VersionToken::hash(&dst_bytes);
    if dst_now != entry.version {
        return Ok(Outcome::NeedsHuman(format!(
            "{} holds {dst_now} but the interrupted move recorded {}; it has been written since, \
             so completing the migration would record a version the file no longer has",
            entry.dst, entry.version
        )));
    }

    // The rename committed and the destination still holds exactly the verified
    // bytes. Finish the job with the payload the intent carried — the original
    // session's, not a reconstruction, which is why `dst_pre_image` survives an
    // endpoint restart and the destination's replaced bytes stay in history.
    //
    // Stamped with the *recovering* principal and session, matching how a
    // dangling write's `crash_recover` is attributed: the audit trail records who
    // actually acted. The interrupted party is named in the entry.
    coord
        .move_paths(&MovePathsRequest {
            src: entry.src.clone(),
            dst: entry.dst.clone(),
            version: entry.version.clone(),
            size: entry.size,
            overwrite: entry.overwrite,
            principal: principal.clone(),
            session_id: session_id.clone(),
            dst_pre_image: entry.dst_pre_image.clone(),
        })
        .await?;
    Ok(Outcome::Completed)
}

/// Run a sweep at start-up and log it, swallowing every error.
///
/// Called from `main` after the coord health check. Nothing here may prevent the
/// endpoint from serving: an unreachable coordinator, an unreadable share, a
/// coordinator too old to have the route — all of them mean "not now", and the
/// intent records survive for the next start. A stale migration is a bookkeeping
/// debt, never a reason to refuse an agent its files.
pub async fn sweep_at_startup(
    coord: &CoordClient,
    backend: &dyn Backend,
    principal: &Principal,
    session_id: &SessionId,
) {
    match sweep(coord, backend, principal, session_id).await {
        Ok(report) if report.is_empty() => {
            tracing::debug!("no interrupted moves to finish")
        }
        Ok(report) => {
            tracing::info!(
                completed = report.completed,
                abandoned = report.abandoned,
                not_ours = report.not_ours,
                "finished interrupted moves left by an earlier run"
            );
            for (entry, why) in &report.needs_human {
                tracing::warn!(
                    src = %entry.src, dst = %entry.dst, principal = %entry.principal,
                    "an interrupted move cannot be resolved automatically: {why}"
                );
            }
            for (entry, e) in &report.failed {
                tracing::warn!(
                    src = %entry.src, dst = %entry.dst, error = %e,
                    "an interrupted move could not be finished this time; it stays recorded"
                );
            }
        }
        Err(e) => tracing::info!(
            error = %e,
            "could not ask the coordinator about interrupted moves; they stay recorded"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::PosixBackend;
    use chapr_proto::{CanonicalPath, LeaseId, PreImage};
    use wiremock::matchers::{method as wmethod, path as wpath};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn who() -> Principal {
        Principal::new_unchecked("CONTOSO\\recoverer")
    }
    fn sess() -> SessionId {
        SessionId::new_unchecked("sess-sweep")
    }
    fn canon(p: &std::path::Path) -> CanonicalPath {
        CanonicalPath::new_unchecked(p.to_string_lossy().to_string())
    }

    fn entry(
        src: &std::path::Path,
        dst: &std::path::Path,
        version: VersionToken,
        pre: Option<PreImage>,
    ) -> MoveJournalEntry {
        MoveJournalEntry {
            src: canon(src),
            dst: canon(dst),
            lease_id: LeaseId::new_unchecked("lease-dead"),
            principal: Principal::new_unchecked("CONTOSO\\interrupted"),
            session_id: SessionId::new_unchecked("sess-crashed"),
            version,
            size: 0,
            overwrite: pre.is_some(),
            dst_pre_image: pre,
            opened_at: chrono::Utc::now(),
        }
    }

    /// A coord that accepts the two calls a resolution can make, and records
    /// which one it got — the assertion that matters is *which*, since completing
    /// and abandoning are the two ways to get this wrong.
    async fn coord_server() -> MockServer {
        let s = MockServer::start().await;
        for p in ["/move", "/move/clear"] {
            Mock::given(wmethod("POST"))
                .and(wpath(p))
                .respond_with(ResponseTemplate::new(204))
                .mount(&s)
                .await;
        }
        s
    }

    async fn paths_called(s: &MockServer) -> Vec<String> {
        s.received_requests()
            .await
            .unwrap()
            .iter()
            .map(|r| r.url.path().to_string())
            .collect()
    }

    #[tokio::test]
    async fn a_committed_rename_gets_its_migration_finished() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.txt");
        let dst = dir.path().join("dst.txt");
        std::fs::write(&dst, b"the moved bytes").unwrap();

        let s = coord_server().await;
        let client = CoordClient::new(s.uri());
        let e = entry(&src, &dst, VersionToken::hash(b"the moved bytes"), None);
        assert_eq!(
            resolve_one(&client, &PosixBackend, &who(), &sess(), &e)
                .await
                .unwrap(),
            Outcome::Completed
        );
        assert_eq!(paths_called(&s).await, vec!["/move".to_string()]);
    }

    #[tokio::test]
    async fn a_rename_that_never_happened_is_abandoned_not_completed() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.txt");
        let dst = dir.path().join("dst.txt");
        // The source is still there, so the rename cannot have gone through.
        std::fs::write(&src, b"never moved").unwrap();

        let s = coord_server().await;
        let client = CoordClient::new(s.uri());
        let e = entry(&src, &dst, VersionToken::hash(b"never moved"), None);
        assert_eq!(
            resolve_one(&client, &PosixBackend, &who(), &sess(), &e)
                .await
                .unwrap(),
            Outcome::Abandoned
        );
        assert_eq!(paths_called(&s).await, vec!["/move/clear".to_string()]);
        assert!(src.exists(), "abandoning must not touch the file");
    }

    /// The source is gone *and* the destination was written after the move. The
    /// rename did commit, but completing now would record a version the file no
    /// longer has — so this stops and says so.
    #[tokio::test]
    async fn a_destination_written_since_the_move_is_left_for_a_human() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.txt");
        let dst = dir.path().join("dst.txt");
        std::fs::write(&dst, b"someone wrote this later").unwrap();

        let s = coord_server().await;
        let client = CoordClient::new(s.uri());
        let e = entry(&src, &dst, VersionToken::hash(b"the moved bytes"), None);
        match resolve_one(&client, &PosixBackend, &who(), &sess(), &e)
            .await
            .unwrap()
        {
            Outcome::NeedsHuman(why) => assert!(
                why.contains("written since"),
                "the reason must say what is wrong: {why}"
            ),
            other => panic!("expected NeedsHuman, got {other:?}"),
        }
        assert!(
            paths_called(&s).await.is_empty(),
            "an unresolvable move must change nothing"
        );
    }

    #[tokio::test]
    async fn neither_path_existing_is_left_for_a_human() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("gone.txt");
        let dst = dir.path().join("also-gone.txt");

        let s = coord_server().await;
        let client = CoordClient::new(s.uri());
        let e = entry(&src, &dst, VersionToken::hash(b"whatever"), None);
        assert!(matches!(
            resolve_one(&client, &PosixBackend, &who(), &sess(), &e)
                .await
                .unwrap(),
            Outcome::NeedsHuman(_)
        ));
        assert!(paths_called(&s).await.is_empty());
    }

    /// The pre-image has to reach coord from the *entry*, because the session
    /// that snapshotted those bytes is gone. Without it, the migration names no
    /// baseline and GC reclaims the destination's replaced content.
    #[tokio::test]
    async fn an_overwrite_moves_pre_image_survives_into_the_completion() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.txt");
        let dst = dir.path().join("dst.txt");
        std::fs::write(&dst, b"the moved bytes").unwrap();
        let pre = PreImage {
            version: VersionToken::hash(b"replaced"),
            size: 8,
        };

        let s = coord_server().await;
        let client = CoordClient::new(s.uri());
        let e = entry(
            &src,
            &dst,
            VersionToken::hash(b"the moved bytes"),
            Some(pre.clone()),
        );
        assert_eq!(
            resolve_one(&client, &PosixBackend, &who(), &sess(), &e)
                .await
                .unwrap(),
            Outcome::Completed
        );

        let body: MovePathsRequest = s.received_requests().await.unwrap()[0]
            .body_json()
            .expect("a move request body");
        assert_eq!(body.dst_pre_image, Some(pre));
        assert!(body.overwrite);
        assert_eq!(
            body.principal,
            who(),
            "the recovering principal acted, and the trail should say so"
        );
    }

    #[tokio::test]
    async fn a_sweep_reports_every_conclusion_it_reached() {
        let dir = tempfile::tempdir().unwrap();
        let done_src = dir.path().join("a.txt");
        let done_dst = dir.path().join("a-moved.txt");
        std::fs::write(&done_dst, b"moved").unwrap();
        let undone_src = dir.path().join("b.txt");
        let undone_dst = dir.path().join("b-moved.txt");
        std::fs::write(&undone_src, b"still here").unwrap();
        let lost_src = dir.path().join("c.txt");
        let lost_dst = dir.path().join("c-moved.txt");

        let entries = vec![
            entry(&done_src, &done_dst, VersionToken::hash(b"moved"), None),
            entry(
                &undone_src,
                &undone_dst,
                VersionToken::hash(b"still here"),
                None,
            ),
            entry(&lost_src, &lost_dst, VersionToken::hash(b"gone"), None),
        ];

        let s = coord_server().await;
        Mock::given(wmethod("GET"))
            .and(wpath("/move/dangling"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "entries": entries })),
            )
            .mount(&s)
            .await;

        let report = sweep(&CoordClient::new(s.uri()), &PosixBackend, &who(), &sess())
            .await
            .unwrap();
        assert_eq!(report.completed, 1);
        assert_eq!(report.abandoned, 1);
        assert_eq!(report.needs_human.len(), 1);
        assert!(report.failed.is_empty());
        assert!(!report.is_empty());
    }

    /// A coordinator that cannot answer must not stop the endpoint, and must not
    /// look like "nothing to do" either — the intents stay recorded.
    #[tokio::test]
    async fn a_coordinator_that_cannot_answer_fails_the_sweep_not_the_endpoint() {
        let client = CoordClient::new("http://127.0.0.1:1");
        assert!(sweep(&client, &PosixBackend, &who(), &sess())
            .await
            .is_err());
        // The startup wrapper turns that into a log line and returns.
        sweep_at_startup(&client, &PosixBackend, &who(), &sess()).await;
    }
}
