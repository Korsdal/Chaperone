// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! The mutation verbs beyond `chapr.write`: `create`, `delete` (soft),
//! `restore` (concept §6.2, §6.5), and `move` (§6.3).
//!
//! Each is an async wrapper that brackets a backend call: acquire the lease and
//! assert read-before-write, run the boring §-specific core on a blocking thread
//! via [`crate::backend::SmbBackend`], then do the uniform post-close tail
//! (version log + audit + record-read) built from the returned `CommitReceipt`.
//! The steps genuinely differ per verb (create has no pre-image; delete removes
//! the file; restore reinstates old bytes; move re-keys coord state), so each is
//! its own explicit sequence — the contended `write_cas` path is left untouched.
//! Move's version-log + audit are emitted coord-side by `move_paths`, so it has
//! no tool-layer tail.

use crate::backend::{Backend, DeleteCasArgs, MoveCasArgs, RestoreInPlaceArgs, WriteCtx};
use crate::canon::canonicalize;
use crate::coord_client::CoordClient;
use crate::lease_manager::LeaseManager;
use crate::pathgrammar::grammar_for;
use chapr_proto::{
    AcquireLeaseRequest, AppendVersionLogRequest, AuditKind, ChaprError, CreateResponse,
    DeleteResponse, LeasePurpose, MkdirResponse, MoveResponse, PreImage, Principal, ReadReceipt,
    RecordAuditRequest, RestoreBase, RestoreMode, RestoreResponse, SessionId, VersionEvent,
    VersionToken,
};
use std::sync::Arc;
use tokio::runtime::Handle;

// ===========================================================================
// create (concept §6.2) — no prior version; atomic CREATE_NEW.
// ===========================================================================

pub async fn create(
    coord: &CoordClient,
    leases: &LeaseManager,
    backend: Arc<dyn Backend>,
    principal: &Principal,
    session_id: &SessionId,
    raw_uri: &str,
    content: Vec<u8>,
) -> Result<CreateResponse, ChaprError> {
    let path = canonicalize(raw_uri, grammar_for(backend.kind()))?;
    let lease = leases
        .acquire(&AcquireLeaseRequest {
            principal: principal.clone(),
            session_id: session_id.clone(),
            purpose: LeasePurpose::Create,
            paths: vec![path.clone()],
        })
        .await?;

    // Inner async block so every `?` exits the block, not the function — see the
    // note in `write::write`. Without it, a failing tail skipped the release
    // below and held the path for the full 20-minute hard lease lifetime.
    let resp: Result<CreateResponse, ChaprError> = async {
        let rt = Handle::current();
        let (coord2, p2, s2, path2) =
            (coord.clone(), principal.clone(), session_id.clone(), path.clone());
        let receipt = tokio::task::spawn_blocking(move || {
            let ctx = WriteCtx {
                rt: &rt,
                coord: &coord2,
                principal: &p2,
                session_id: &s2,
            };
            backend.create(&ctx, &path2, &content)
        })
        .await
        .map_err(join_err)??;

        let version = receipt.to_version.clone().ok_or_else(|| ChaprError::Internal {
            message: format!("backend bug: successful create receipt for {path} has no to_version"),
        })?;
        // The file exists on the share from here on, so a tail failure is
        // committed-but-unrecorded, not a failed create.
        let committed = |e: ChaprError| ChaprError::CommittedButUnrecorded {
            path: path.clone(),
            version: version.clone(),
            message: e.to_string(),
        };
        coord
            .append_version_log(&AppendVersionLogRequest {
                path: path.clone(),
                blob_hash: version.clone(),
                writer_principal: principal.clone(),
                size: receipt.size,
                event: VersionEvent::Create,
                pre_image: None, // a create replaces nothing
            })
            .await
            .map_err(&committed)?;
        coord
            .record_audit(&RecordAuditRequest {
                principal: principal.clone(),
                session_id: session_id.clone(),
                path: path.clone(),
                kind: AuditKind::WriteCommit,
                from_version: None,
                to_version: Some(version.clone()),
                detail: "create".to_string(),
            })
            .await
            .map_err(&committed)?;
        // Record the new version so the session can immediately write to it (§6.2).
        let _ = coord
            .record_read(&ReadReceipt {
                session_id: session_id.clone(),
                path: path.clone(),
                version: version.clone(),
            })
            .await;
        Ok(CreateResponse { version })
    }
    .await;
    let _ = leases.release(&lease.lease_id).await;
    resp
}

// ===========================================================================
// mkdir — create one directory, guarded against near-duplicate names.
// ===========================================================================

/// `chapr.mkdir`. No lease, no journal, no version log: a directory has no
/// content, so there is nothing to serialise against, recover, or version. It is
/// audited (`DirCreate`) because *who added a folder, and did they override the
/// guard* is a question worth being able to answer.
pub async fn mkdir(
    coord: &CoordClient,
    backend: Arc<dyn Backend>,
    principal: &Principal,
    session_id: &SessionId,
    raw_uri: &str,
    confirm_new: bool,
) -> Result<MkdirResponse, ChaprError> {
    let grammar = grammar_for(backend.kind());
    let path = canonicalize(raw_uri, grammar)?;

    // The parent must exist. Checked here rather than left to the OS so the
    // caller gets `ParentMissing` naming the parent, instead of the OS's
    // `NotFound` naming the directory it asked for.
    let parent = crate::backend::parent_of(backend.kind(), &path).ok_or_else(|| {
        ChaprError::InvalidPath {
            raw: raw_uri.to_string(),
            reason: "a share root cannot be created".into(),
        }
    })?;

    // The near-name guard. Listing the parent is what makes it deterministic:
    // the comparison is against what is actually there, not against a model's
    // memory of what it saw.
    let siblings = match backend.as_file_source().list(&parent) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ChaprError::ParentMissing {
                path: path.clone(),
                parent,
            })
        }
        Err(e) => return Err(crate::backend::map_os_err(&parent, e)),
    };
    let leaf = path.as_str().rsplit(grammar.sep()).next().unwrap_or_default();
    let dir_names: Vec<&str> = siblings
        .iter()
        // Only directories. A file called `Reports` is not the mistake this
        // guards against, and flagging it would refuse a legitimate folder for
        // sharing a name with a document.
        .filter(|e| e.entry_type == chapr_proto::EntryType::Dir)
        .map(|e| e.name.as_str())
        .collect();
    let similar = crate::nearname::similar_names(leaf, dir_names);
    if !similar.is_empty() && !confirm_new {
        return Err(ChaprError::NearDuplicateName {
            path: path.clone(),
            similar,
        });
    }

    backend.mkdir(&path)?;

    // Best-effort, and after the fact: the directory exists either way, and
    // failing the call because the trail could not be written would tell the
    // caller its mkdir failed when it succeeded — the lie `CommittedButUnrecorded`
    // exists to prevent, which is not worth introducing for a folder.
    let detail = if similar.is_empty() {
        "mkdir".to_string()
    } else {
        // The override is the part worth reading later.
        format!("mkdir with confirm_new past similar: {}", similar.join(", "))
    };
    if let Err(e) = coord
        .record_audit(&RecordAuditRequest {
            principal: principal.clone(),
            session_id: session_id.clone(),
            path: path.clone(),
            kind: AuditKind::DirCreate,
            from_version: None,
            to_version: None,
            detail,
        })
        .await
    {
        tracing::warn!(path = %path, error = %e, "created directory but could not audit it");
    }

    Ok(MkdirResponse {
        canonical_path: path,
        similar_existing: similar,
    })
}

// ===========================================================================
// delete (concept §6.2) — soft: snapshot pre-image, then remove.
// ===========================================================================

pub async fn delete(
    coord: &CoordClient,
    leases: &LeaseManager,
    backend: Arc<dyn Backend>,
    principal: &Principal,
    session_id: &SessionId,
    raw_uri: &str,
    base_version: VersionToken,
) -> Result<DeleteResponse, ChaprError> {
    let path = canonicalize(raw_uri, grammar_for(backend.kind()))?;
    // Read-before-write (§6.2): the delete's base_version must have been read.
    coord
        .assert_read(&ReadReceipt {
            session_id: session_id.clone(),
            path: path.clone(),
            version: base_version.clone(),
        })
        .await?;
    let lease = leases
        .acquire(&AcquireLeaseRequest {
            principal: principal.clone(),
            session_id: session_id.clone(),
            purpose: LeasePurpose::Delete,
            paths: vec![path.clone()],
        })
        .await?;

    // Inner async block: see the note in `write::write`. This verb is the sharpest
    // case — by the time the tail runs the file is already GONE from the share, so
    // a tail failure that also leaked the lease left the path both deleted and
    // locked, while telling the caller the delete failed.
    let resp: Result<DeleteResponse, ChaprError> = async {
        let rt = Handle::current();
        let (coord2, p2, s2) = (coord.clone(), principal.clone(), session_id.clone());
        let args = DeleteCasArgs {
            path: path.clone(),
            lease_id: lease.lease_id.clone(),
            base_version,
        };
        let receipt = tokio::task::spawn_blocking(move || {
            let ctx = WriteCtx {
                rt: &rt,
                coord: &coord2,
                principal: &p2,
                session_id: &s2,
            };
            backend.delete_cas(&ctx, &args)
        })
        .await
        .map_err(join_err)??;

        let deleted = receipt.from_version.clone().ok_or_else(|| ChaprError::Internal {
            message: format!("backend bug: successful delete receipt for {path} has no from_version"),
        })?;
        let committed = |e: ChaprError| ChaprError::CommittedButUnrecorded {
            path: path.clone(),
            version: deleted.clone(),
            message: e.to_string(),
        };
        coord
            .append_version_log(&AppendVersionLogRequest {
                path: path.clone(),
                blob_hash: deleted.clone(),
                writer_principal: principal.clone(),
                size: receipt.size,
                event: VersionEvent::Delete,
                // This entry is itself keyed by the pre-image hash, so the
                // snapshot is already referenced — no baseline needed.
                pre_image: None,
            })
            .await
            .map_err(&committed)?;
        coord
            .record_audit(&RecordAuditRequest {
                principal: principal.clone(),
                session_id: session_id.clone(),
                path: path.clone(),
                kind: AuditKind::WriteCommit,
                from_version: Some(deleted.clone()),
                to_version: None,
                detail: "soft delete".to_string(),
            })
            .await
            .map_err(&committed)?;
        Ok(DeleteResponse {})
    }
    .await;
    let _ = leases.release(&lease.lease_id).await;
    resp
}

// ===========================================================================
// restore (concept §6.5) — default copy; in_place opt-in.
// ===========================================================================

#[allow(clippy::too_many_arguments)]
pub async fn restore(
    coord: &CoordClient,
    leases: &LeaseManager,
    backend: Arc<dyn Backend>,
    principal: &Principal,
    session_id: &SessionId,
    raw_uri: &str,
    version: VersionToken,
    mode: RestoreMode,
    base: Option<RestoreBase>,
) -> Result<RestoreResponse, ChaprError> {
    let path = canonicalize(raw_uri, grammar_for(backend.kind()))?;
    // Fetch the old bytes from the history store first (fails fast if gone).
    let bytes = coord.get_blob(&version).await?;
    let size = bytes.len() as u64;

    // `in_place` cannot proceed without knowing what the caller saw (Q13(a)).
    //
    // Refused here rather than defaulted, because every default is wrong: assume
    // a version and the restore clobbers unseen content, assume absent and it
    // refuses a legitimate overwrite. `Copy` needs no base — it writes a fresh,
    // uniquely-named sibling and destroys nothing.
    let base_seen = match (mode, base) {
        (RestoreMode::InPlace, None) => {
            return Err(ChaprError::BaseVersionRequired { path });
        }
        (RestoreMode::InPlace, Some(b)) => b,
        // Ignored for Copy, and not worth a separate arg shape.
        (RestoreMode::Copy, _) => RestoreBase::Absent(chapr_proto::AbsentMarker::Absent),
    };

    match mode {
        RestoreMode::Copy => {
            // Write a fresh, uniquely-named copy — no lease needed (new file).
            let rt = Handle::current();
            let (coord2, p2, s2, path2, v2) = (
                coord.clone(),
                principal.clone(),
                session_id.clone(),
                path.clone(),
                version.clone(),
            );
            let result = tokio::task::spawn_blocking(move || {
                let ctx = WriteCtx {
                    rt: &rt,
                    coord: &coord2,
                    principal: &p2,
                    session_id: &s2,
                };
                backend.restore_copy(&ctx, &path2, &bytes, &v2)
            })
            .await
            .map_err(join_err)?;
            let restored = result?;
            coord
                .append_version_log(&AppendVersionLogRequest {
                    path: restored.clone(),
                    blob_hash: version.clone(),
                    writer_principal: principal.clone(),
                    size,
                    event: VersionEvent::Restore,
                    // Restore-to-copy writes a new sibling; nothing is replaced.
                    pre_image: None,
                })
                .await?;
            coord
                .record_audit(&RecordAuditRequest {
                    principal: principal.clone(),
                    session_id: session_id.clone(),
                    path: restored.clone(),
                    kind: AuditKind::Restore,
                    from_version: None,
                    to_version: Some(version.clone()),
                    detail: "restore copy".to_string(),
                })
                .await?;
            Ok(RestoreResponse {
                restored_path: Some(restored),
                version,
            })
        }
        RestoreMode::InPlace => {
            // The full contended path: lease, exclusive open, snapshot current,
            // write the old bytes.
            //
            // What that does and does not protect, stated exactly, because the
            // comment here used to claim it "cannot clobber a concurrent writer"
            // — narrowly true and broadly false. The lease plus the exclusive
            // handle do exclude a write that is *in flight* right now. They do
            // not make this a compare-and-swap: a restore performs no CAS by
            // design (D-012, D-027 — "a restore is a deliberate overwrite"), so
            // a version committed and closed between the caller's
            // `chapr_history` and this call is overwritten with no conflict and
            // no sidecar. The pre-image snapshot below is what makes that
            // recoverable rather than lost.
            let lease = leases
                .acquire(&AcquireLeaseRequest {
                    principal: principal.clone(),
                    session_id: session_id.clone(),
                    purpose: LeasePurpose::Restore,
                    paths: vec![path.clone()],
                })
                .await?;
            // Inner async block: see the note in `write::write`.
            let resp: Result<RestoreResponse, ChaprError> = async {
                let rt = Handle::current();
                let (coord2, p2, s2) = (coord.clone(), principal.clone(), session_id.clone());
                let args = RestoreInPlaceArgs {
                    path: path.clone(),
                    lease_id: lease.lease_id.clone(),
                    bytes,
                    version: version.clone(),
                    base: base_seen,
                };
                let receipt = tokio::task::spawn_blocking(move || {
                    let ctx = WriteCtx {
                        rt: &rt,
                        coord: &coord2,
                        principal: &p2,
                        session_id: &s2,
                    };
                    backend.restore_in_place(&ctx, &args)
                })
                .await
                .map_err(join_err)??;

                // The old bytes are live on the share from here on.
                let committed = |e: ChaprError| ChaprError::CommittedButUnrecorded {
                    path: path.clone(),
                    version: version.clone(),
                    message: e.to_string(),
                };
                coord
                    .append_version_log(&AppendVersionLogRequest {
                        path: path.clone(),
                        blob_hash: version.clone(),
                        writer_principal: principal.clone(),
                        size: receipt.size,
                        event: VersionEvent::Restore,
                        // An in-place restore overwrites live bytes and snapshots
                        // them first; this entry names the version being
                        // reinstated, not the one replaced, so the snapshot needs
                        // its own baseline reference or GC reclaims it.
                        pre_image: receipt.from_version.as_ref().zip(receipt.from_size).map(
                            |(version, size)| PreImage {
                                version: version.clone(),
                                size,
                            },
                        ),
                    })
                    .await
                    .map_err(&committed)?;
                coord
                    .record_audit(&RecordAuditRequest {
                        principal: principal.clone(),
                        session_id: session_id.clone(),
                        path: path.clone(),
                        kind: AuditKind::Restore,
                        from_version: receipt.from_version.clone(),
                        to_version: Some(version.clone()),
                        detail: "restore in_place".to_string(),
                    })
                    .await
                    .map_err(&committed)?;
                Ok(RestoreResponse {
                    restored_path: None,
                    version: version.clone(),
                })
            }
            .await;
            let _ = leases.release(&lease.lease_id).await;
            resp
        }
    }
}

// ===========================================================================
// move (concept §6.3) — atomic dual-lease rename.
// ===========================================================================

#[allow(clippy::too_many_arguments)]
pub async fn mv(
    coord: &CoordClient,
    leases: &LeaseManager,
    backend: Arc<dyn Backend>,
    principal: &Principal,
    session_id: &SessionId,
    src_uri: &str,
    dst_uri: &str,
    src_base_version: VersionToken,
    dst_base_version: Option<VersionToken>,
) -> Result<MoveResponse, ChaprError> {
    let src = canonicalize(src_uri, grammar_for(backend.kind()))?;
    let dst = canonicalize(dst_uri, grammar_for(backend.kind()))?;
    if src == dst {
        return Err(ChaprError::InvalidPath {
            raw: dst_uri.to_string(),
            reason: "move source and destination are the same path".into(),
        });
    }

    // Read-before-write on the source (§6.2); the destination is checked inside
    // the blocking section once we know it's an overwrite.
    coord
        .assert_read(&ReadReceipt {
            session_id: session_id.clone(),
            path: src.clone(),
            version: src_base_version.clone(),
        })
        .await?;

    // All-or-none lease over {src, dst} — coord acquires in canonical order,
    // which kills the A-holds-1-wants-2 deadlock (concept §9).
    let lease = leases
        .acquire(&AcquireLeaseRequest {
            principal: principal.clone(),
            session_id: session_id.clone(),
            purpose: LeasePurpose::Move,
            paths: vec![src.clone(), dst.clone()],
        })
        .await?;

    let rt = Handle::current();
    let (coord2, p2, s2) = (coord.clone(), principal.clone(), session_id.clone());
    let args = MoveCasArgs {
        src: src.clone(),
        dst: dst.clone(),
        src_base_version,
        dst_base_version,
        // The move journal records this (B3): the lease's liveness is what tells
        // a later sweep whether a move is in flight or died owing a migration.
        lease_id: lease.lease_id.clone(),
    };
    let result = tokio::task::spawn_blocking(move || {
        let ctx = WriteCtx {
            rt: &rt,
            coord: &coord2,
            principal: &p2,
            session_id: &s2,
        };
        backend.move_cas(&ctx, &args)
    })
    .await
    .map_err(join_err)?;

    let _ = leases.release(&lease.lease_id).await;
    // Hand the destination's version back, so a caller can chain a CAS write
    // without reading the file it just moved.
    result.map(|r| MoveResponse { version: r.version })
}

// ---- helpers --------------------------------------------------------------

fn join_err(e: tokio::task::JoinError) -> ChaprError {
    ChaprError::Internal {
        message: format!("task join failed: {e}"),
    }
}
