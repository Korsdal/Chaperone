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
    DeleteResponse, LeasePurpose, MoveResponse, Principal, ReadReceipt, RecordAuditRequest,
    RestoreMode, RestoreResponse, SessionId, VersionEvent, VersionToken,
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

    let rt = Handle::current();
    let (coord2, p2, s2, path2) = (coord.clone(), principal.clone(), session_id.clone(), path.clone());
    let result = tokio::task::spawn_blocking(move || {
        let ctx = WriteCtx {
            rt: &rt,
            coord: &coord2,
            principal: &p2,
            session_id: &s2,
        };
        backend.create(&ctx, &path2, &content)
    })
    .await
    .map_err(join_err)?;

    let resp = match result {
        Ok(receipt) => {
            let version = receipt
                .to_version
                .clone()
                .expect("create produces a new version");
            coord
                .append_version_log(&AppendVersionLogRequest {
                    path: path.clone(),
                    blob_hash: version.clone(),
                    writer_principal: principal.clone(),
                    size: receipt.size,
                    event: VersionEvent::Create,
                })
                .await?;
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
                .await?;
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
        Err(e) => Err(e),
    };
    let _ = leases.release(&lease.lease_id).await;
    resp
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

    let rt = Handle::current();
    let (coord2, p2, s2) = (coord.clone(), principal.clone(), session_id.clone());
    let args = DeleteCasArgs {
        path: path.clone(),
        lease_id: lease.lease_id.clone(),
        base_version,
    };
    let result = tokio::task::spawn_blocking(move || {
        let ctx = WriteCtx {
            rt: &rt,
            coord: &coord2,
            principal: &p2,
            session_id: &s2,
        };
        backend.delete_cas(&ctx, &args)
    })
    .await
    .map_err(join_err)?;

    let resp = match result {
        Ok(receipt) => {
            let deleted = receipt
                .from_version
                .clone()
                .expect("delete has a pre-image version");
            coord
                .append_version_log(&AppendVersionLogRequest {
                    path: path.clone(),
                    blob_hash: deleted.clone(),
                    writer_principal: principal.clone(),
                    size: receipt.size,
                    event: VersionEvent::Delete,
                })
                .await?;
            coord
                .record_audit(&RecordAuditRequest {
                    principal: principal.clone(),
                    session_id: session_id.clone(),
                    path: path.clone(),
                    kind: AuditKind::WriteCommit,
                    from_version: Some(deleted),
                    to_version: None,
                    detail: "soft delete".to_string(),
                })
                .await?;
            Ok(DeleteResponse {})
        }
        Err(e) => Err(e),
    };
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
) -> Result<RestoreResponse, ChaprError> {
    let path = canonicalize(raw_uri, grammar_for(backend.kind()))?;
    // Fetch the old bytes from the history store first (fails fast if gone).
    let bytes = coord.get_blob(&version).await?;
    let size = bytes.len() as u64;

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
            // write the old bytes. Cannot clobber a concurrent writer.
            let lease = leases
                .acquire(&AcquireLeaseRequest {
                    principal: principal.clone(),
                    session_id: session_id.clone(),
                    purpose: LeasePurpose::Restore,
                    paths: vec![path.clone()],
                })
                .await?;
            let rt = Handle::current();
            let (coord2, p2, s2) = (coord.clone(), principal.clone(), session_id.clone());
            let args = RestoreInPlaceArgs {
                path: path.clone(),
                lease_id: lease.lease_id.clone(),
                bytes,
                version: version.clone(),
            };
            let result = tokio::task::spawn_blocking(move || {
                let ctx = WriteCtx {
                    rt: &rt,
                    coord: &coord2,
                    principal: &p2,
                    session_id: &s2,
                };
                backend.restore_in_place(&ctx, &args)
            })
            .await
            .map_err(join_err)?;

            let resp = match result {
                Ok(receipt) => {
                    coord
                        .append_version_log(&AppendVersionLogRequest {
                            path: path.clone(),
                            blob_hash: version.clone(),
                            writer_principal: principal.clone(),
                            size: receipt.size,
                            event: VersionEvent::Restore,
                        })
                        .await?;
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
                        .await?;
                    Ok(RestoreResponse {
                        restored_path: None,
                        version: version.clone(),
                    })
                }
                Err(e) => Err(e),
            };
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
    result.map(|_| MoveResponse {})
}

// ---- helpers --------------------------------------------------------------

fn join_err(e: tokio::task::JoinError) -> ChaprError {
    ChaprError::Internal {
        message: format!("task join failed: {e}"),
    }
}
