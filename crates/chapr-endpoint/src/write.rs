//! The contended write path (concept §7) — the crown jewel.
//!
//! Deliberately boring and linear (implementation notes §6): everything from
//! version-check to write-close happens under **one** held exclusive handle
//! (invariant 4), inside a single `spawn_blocking` so it is straight-line
//! synchronous code. That §7 core now lives in [`crate::backend::SmbBackend::write_cas`]
//! (the backend seam); this module owns the async bracket around it: the
//! read-before-write assert, the lease, and the uniform post-close tail
//! (version log + audit + record-read) built from the returned `CommitReceipt`.
//!
//! Coord's mid-handle durability calls (journal, snapshot) are driven with
//! `block_on` on the blocking thread from inside `write_cas`, so the `HANDLE`
//! never crosses an `.await`. The post-close tail is plain async — it runs only
//! after the handle is dropped, so hoisting it here is behaviour-identical and
//! keeps the scaffolding uniform across backends.
//!
//! Fail-closed (concept §10): if coord is unreachable at the journal step,
//! `write_cas` returns before any bytes are written and the handle drops closed
//! — no journal entry, no write. Reads degrade-open; writes do not.

use crate::backend::{Backend, WriteCasArgs, WriteCtx};
use crate::canon::canonicalize;
use crate::coord_client::CoordClient;
use crate::lease_manager::LeaseManager;
use crate::pathgrammar::grammar_for;
use chapr_proto::{
    AcquireLeaseRequest, AppendVersionLogRequest, AuditKind, ChaprError, LeasePurpose, Principal,
    ReadReceipt, RecordAuditRequest, SessionId, VersionEvent, VersionToken, WriteMode,
    WriteResponse,
};
use std::sync::Arc;
use tokio::runtime::Handle;

/// Perform a `chapr.write`. Acquires the lease, runs the §7 core on a blocking
/// thread via the backend, commits the version log + audit, and always releases
/// the lease afterwards.
#[allow(clippy::too_many_arguments)]
pub async fn write(
    coord: &CoordClient,
    leases: &LeaseManager,
    backend: Arc<dyn Backend>,
    principal: &Principal,
    session_id: &SessionId,
    raw_uri: &str,
    content: Vec<u8>,
    base_version: VersionToken,
    mode: WriteMode,
) -> Result<WriteResponse, ChaprError> {
    let path = canonicalize(raw_uri, grammar_for(backend.kind()))?;

    // Structural read-before-write (§6.2): reject a base_version this session
    // never read, before taking any lease.
    if !matches!(mode, WriteMode::Force { .. }) {
        coord
            .assert_read(&ReadReceipt {
                session_id: session_id.clone(),
                path: path.clone(),
                version: base_version.clone(),
            })
            .await?;
    }

    // Step 2: acquire the write lease. The manager renews it in the background,
    // so even a slow write_cas (blocking) keeps its lease alive past the TTL.
    let lease = leases
        .acquire(&AcquireLeaseRequest {
            principal: principal.clone(),
            session_id: session_id.clone(),
            purpose: LeasePurpose::Write,
            paths: vec![path.clone()],
        })
        .await?;

    // Steps 3–11: the §7 core, on a blocking thread. Coord's mid-handle calls
    // use this runtime handle; the exclusive handle never leaves the thread.
    let rt = Handle::current();
    let coord2 = coord.clone();
    let p2 = principal.clone();
    let s2 = session_id.clone();
    let args = WriteCasArgs {
        path: path.clone(),
        lease_id: lease.lease_id.clone(),
        content,
        base_version,
        mode: mode.clone(),
    };
    let result = tokio::task::spawn_blocking(move || {
        let ctx = WriteCtx {
            rt: &rt,
            coord: &coord2,
            principal: &p2,
            session_id: &s2,
        };
        backend.write_cas(&ctx, &args)
    })
    .await
    .map_err(|e| ChaprError::Internal {
        message: format!("write task join failed: {e}"),
    })?;

    // Post-close tail (uniform across backends): commit the version log + audit
    // from the receipt, then record the new version so the session can chain
    // another write without re-reading (best-effort — §6.2). All strictly after
    // the handle closed, so this runs as plain async.
    let resp = match result {
        Ok(receipt) => {
            let v_new = receipt
                .to_version
                .clone()
                .expect("a successful write produces a new version");
            commit_tail(coord, principal, session_id, &path, &receipt, &v_new, &mode).await?;
            let _ = coord
                .record_read(&ReadReceipt {
                    session_id: session_id.clone(),
                    path: path.clone(),
                    version: v_new.clone(),
                })
                .await;
            Ok(WriteResponse { version: v_new })
        }
        Err(e) => Err(e),
    };

    // Step 13: stop renewing and release the lease regardless of outcome.
    let _ = leases.release(&lease.lease_id).await;
    resp
}

/// Step 11 (hoisted): append the version-log entry and audit the write commit.
async fn commit_tail(
    coord: &CoordClient,
    principal: &Principal,
    session_id: &SessionId,
    path: &chapr_proto::CanonicalPath,
    receipt: &crate::backend::CommitReceipt,
    v_new: &VersionToken,
    mode: &WriteMode,
) -> Result<(), ChaprError> {
    coord
        .append_version_log(&AppendVersionLogRequest {
            path: path.clone(),
            blob_hash: v_new.clone(),
            writer_principal: principal.clone(),
            size: receipt.size,
            event: VersionEvent::Write,
        })
        .await?;
    let detail = match mode {
        WriteMode::Force { reason } => format!("forced write: {reason}"),
        WriteMode::Cas => "write".to_string(),
    };
    coord
        .record_audit(&RecordAuditRequest {
            principal: principal.clone(),
            session_id: session_id.clone(),
            path: path.clone(),
            kind: AuditKind::WriteCommit,
            from_version: receipt.from_version.clone(),
            to_version: Some(v_new.clone()),
            detail,
        })
        .await?;
    Ok(())
}
