//! The `chapr.*` tool surface (concept §6) and the internal `coord.resolve`
//! control-channel call (concept §8.1), as request/response types.
//!
//! ## Two audiences, one crate
//!
//! Most of these are the MCP tool schema the model calls (endpoint ↔ Claude
//! Desktop over stdio). [`ResolveRequest`]/[`ResolveResponse`] are different:
//! they are the endpoint ↔ coord control-channel call. Both belong here so the
//! whole surface is defined once.
//!
//! ## `uri` vs [`CanonicalPath`]
//!
//! Request types that originate from the model carry a raw `uri: String` — what
//! the user or model typed, *not yet* canonicalised. The endpoint runs the
//! §5.1 canonicaliser and only then keys coordination state. Response and
//! record types carry the already-canonical [`CanonicalPath`]. `ResolveRequest`
//! is the exception: it is coord-facing and its path is already canonical.
//!
//! ## Invariant 6 (bytes vs metadata)
//!
//! [`ReadResponse`]/[`WriteRequest`] carry file bytes, but only over the
//! endpoint↔model stdio channel — never over the control channel. The heavy
//! 200 MB PDF flows endpoint → SMB → model; it never touches coord. Do not add
//! a byte-carrying field to any coord-facing type in this module.

use crate::enums::{
    AuditKind, ConflictResolution, Integrity, JournalState, LeasePurpose, RestoreMode,
    VersionEvent, WriteMode,
};
use crate::ids::{CanonicalPath, ConflictId, LeaseId, Principal, SessionId};
use crate::records::{ConflictEntry, LeaseRef, RecoveredFrom};
use crate::version::VersionToken;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ===========================================================================
// Read / list / stat (concept §6.1)
// ===========================================================================

/// `chapr.read(uri)`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadRequest {
    pub uri: String,
}

/// The body of a read result. Small artifacts come back inline; a large file
/// may instead be handed back by reference to keep it out of the model's
/// context until it asks. Bytes still travel endpoint→model, never through
/// coord (invariant 6).
///
/// Wire form is internally tagged on `kind`:
/// `{"kind":"inline","bytes":[…]}` or `{"kind":"ref","content_ref":"…"}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReadContent {
    /// The bytes themselves. The endpoint wraps these in the untrusted-data
    /// envelope (concept §13.3) before handing them to the model.
    Inline { bytes: Vec<u8> },
    /// An opaque handle the model can pass back to fetch the bytes on demand.
    Ref { content_ref: String },
}

/// `chapr.read` result (concept §6.1). `version` is present unless
/// `integrity == Unverified` (coord was unreachable and the read degraded open,
/// §8.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadResponse {
    pub content: ReadContent,
    /// Omitted only when `integrity == Unverified`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<VersionToken>,
    pub integrity: Integrity,
    /// Present only when `integrity == Recovered` (a dangling write was healed
    /// before serving).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovered_from: Option<RecoveredFrom>,
    /// Surface-on-touch: how many conflicts stand open against this file
    /// (concept §11). Omitted when zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_conflicts: Option<u32>,
}

/// `chapr.list(uri)`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListRequest {
    pub uri: String,
}

/// One entry in a directory listing (concept §6.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListEntry {
    pub name: String,
    pub canonical_path: CanonicalPath,
    pub size: u64,
    pub mtime: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<VersionToken>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_conflicts: Option<u32>,
}

/// `chapr.list` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListResponse {
    pub entries: Vec<ListEntry>,
}

/// `chapr.stat(uri)`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatRequest {
    pub uri: String,
}

/// `chapr.stat` result (concept §6.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatResponse {
    pub canonical_path: CanonicalPath,
    pub size: u64,
    pub mtime: DateTime<Utc>,
    pub version: VersionToken,
    /// The lease currently held over this file, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<LeaseRef>,
    pub journal_state: JournalState,
}

// ===========================================================================
// Write / create / delete (concept §6.2)
// ===========================================================================

/// `chapr.write(uri, content, base_version, mode)` (concept §6.2).
///
/// `base_version` is **required** — never optional (a model omits optional
/// fields under pressure). `mode` defaults to CAS when omitted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteRequest {
    pub uri: String,
    pub content: Vec<u8>,
    /// The version the agent read. Coord additionally rejects any value it did
    /// not record this session as having read (concept §6.2), so this cannot be
    /// fabricated.
    pub base_version: VersionToken,
    #[serde(default)]
    pub mode: WriteMode,
}

/// `chapr.write` success — the new version now on the share.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteResponse {
    pub version: VersionToken,
}

/// `chapr.create(uri, content)` — no prior version; CAS with base = null
/// (concept §6.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateRequest {
    pub uri: String,
    pub content: Vec<u8>,
}

/// `chapr.create` success — the first version of the new file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateResponse {
    pub version: VersionToken,
}

/// `chapr.delete(uri, base_version)` — **soft**: the pre-image is snapshotted,
/// then the file removed; recoverable via `chapr.restore` (concept §6.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteRequest {
    pub uri: String,
    pub base_version: VersionToken,
}

/// `chapr.delete` success. Empty — the soft delete is recorded in the version
/// log and audit trail, not returned here.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteResponse {}

// ===========================================================================
// Move / rename (concept §6.3)
// ===========================================================================

/// `chapr.move(src_uri, dst_uri, src_base_version, dst_base_version?)`.
///
/// Acquires an all-or-none lease set on `{src, dst}` in canonical order.
/// Rename-onto-existing-target is a move *and* an overwrite — in that case
/// `dst_base_version` is required and the destination goes through CAS
/// (concept §6.3). This is the one v1 verb with real hidden depth.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoveRequest {
    pub src_uri: String,
    pub dst_uri: String,
    pub src_base_version: VersionToken,
    /// Required only when the destination already exists (the overwrite case);
    /// `None` for a move to a fresh path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dst_base_version: Option<VersionToken>,
}

/// `chapr.move` success. Empty — the version-log chain and any open
/// journal/conflict entries are migrated to the new canonical path internally.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoveResponse {}

// ===========================================================================
// Leases (concept §6.4, §9)
// ===========================================================================

/// `chapr.lease_acquire(paths[], purpose)`. All-or-none over the set, acquired
/// in canonical order. Acquired only for write intent — reads never lease.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseAcquireRequest {
    pub paths: Vec<String>,
    pub purpose: LeasePurpose,
}

/// `chapr.lease_acquire` success. Failure is [`crate::ChaprError::LeaseHeld`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseAcquireResponse {
    pub lease_id: LeaseId,
    pub ttl_s: u32,
}

/// `chapr.lease_renew(lease_id)`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseRenewRequest {
    pub lease_id: LeaseId,
}

/// `chapr.lease_renew` success.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseRenewResponse {
    pub renewed_until: DateTime<Utc>,
}

/// `chapr.lease_release(lease_id)`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseReleaseRequest {
    pub lease_id: LeaseId,
}

/// `chapr.lease_release` success. Empty.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseReleaseResponse {}

// ===========================================================================
// History / restore / conflicts (concept §6.5, §11, §12)
// ===========================================================================

/// `chapr.history(uri)` — reads the version log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryRequest {
    pub uri: String,
}

/// One row of a file's history (concept §6.5). A projection of
/// [`crate::records::VersionLogEntry`] for the model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub version: VersionToken,
    pub timestamp: DateTime<Utc>,
    pub writer: Principal,
    pub size: u64,
    pub event: VersionEvent,
}

/// `chapr.history` result, newest-first by convention.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryResponse {
    pub entries: Vec<HistoryEntry>,
}

/// `chapr.restore(uri, version, mode)`. Runs the full contended-write path, so
/// a restore is itself versioned and audited and cannot clobber a concurrent
/// writer. `mode` defaults to `Copy` (concept §6.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreRequest {
    pub uri: String,
    pub version: VersionToken,
    #[serde(default)]
    pub mode: RestoreMode,
}

/// `chapr.restore` success.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreResponse {
    /// For `mode = Copy`, the `F.restored-{ts}.ext` path written. `None` for
    /// `in_place`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restored_path: Option<CanonicalPath>,
    /// The new version produced by the restore write.
    pub version: VersionToken,
}

/// `chapr.conflicts(scope)` — standing open conflicts under a path (the
/// governance artifact: "what's unreconciled under `/tenders/`", concept §11).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictsRequest {
    /// A path prefix to scope the query.
    pub scope: String,
}

/// `chapr.conflicts` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictsResponse {
    pub conflicts: Vec<ConflictEntry>,
}

/// `chapr.resolve_conflict(conflict_id, resolution)` — the explicit, audited
/// "how it was resolved" (concept §11).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolveConflictRequest {
    pub conflict_id: ConflictId,
    pub resolution: ConflictResolution,
}

/// `chapr.resolve_conflict` success. Empty.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolveConflictResponse {}

// ===========================================================================
// Internal control channel: coord.resolve (concept §8.1)
// ===========================================================================

/// `coord.resolve(path)` — the cheap phase-one metadata call the read state
/// machine runs before touching bytes (concept §8.1, §8.2). Endpoint ↔ coord
/// only; the path is already canonical.
///
/// NOTE the documented v1 existence-leak (concept §13.2): this returns
/// version/size/mtime regardless of the caller's ACL, because the watcher
/// indexes as a service account. Content access stays safe (bytes come through
/// the user's own open); metadata does leak. Accepted for a flat-permission
/// deployment; v2 is an ACL-aware index.
///
/// The version index is keyed by `(canonical_path, mtime, size)` (concept
/// §4.2), so the caller sends the stat it observed and coord returns
/// [`ResolveResponse::cached_version`] `= Some` only on an exact key match —
/// a changed file (different mtime/size) is a cache miss, and the endpoint
/// re-hashes and refreshes the entry (concept §8.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolveRequest {
    pub path: CanonicalPath,
    /// The modification time the endpoint observed when it stat'd the file.
    pub mtime: DateTime<Utc>,
    /// The size in bytes the endpoint observed.
    pub size: u64,
}

/// `coord.resolve` result — the three signals the read state machine needs
/// (concept §8.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolveResponse {
    /// The cached BLAKE3 version, if coord has indexed this path. `None` means
    /// not-yet-indexed (lazy population, concept §15) — the endpoint hashes on
    /// first access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_version: Option<VersionToken>,
    pub journal_state: JournalState,
    /// The lease currently held over the path, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_state: Option<LeaseRef>,
    /// Surface-on-touch (concept §11): how many conflicts stand open against
    /// this file. Omitted when zero, so the common path carries nothing extra.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_conflicts: Option<u32>,
    /// Which backend owns this resource (§14). `None` from an older coord, or
    /// when coord cannot classify the path — the endpoint then falls back to its
    /// local default selection (invariant 3). Advisory: authoritative only on
    /// the read path; a cross-check on writes, never a "proceed" signal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<crate::backend::BackendDescriptor>,
}

// ===========================================================================
// Coord control-channel DTOs (endpoint ↔ coord)
// ===========================================================================
//
// These back the tool surface above but carry what the control channel needs:
// already-canonical [`CanonicalPath`]s (the endpoint canonicalised them, §5.1)
// and the caller's identity. They were first written coord-local and promoted
// here once the endpoint became the second consumer, so a single contract keeps
// both binaries in lockstep.
//
// NOTE the auth shim: `principal`/`writer_principal` are carried in the body
// only until Negotiate/Kerberos auth derives identity from the connection
// (concept §13.1); see the endpoint work and logbook I-001.

/// Backs `chapr.lease_acquire` on the control channel. Distinct from the
/// tool-facing [`LeaseAcquireRequest`] (which carries raw `uri` strings): here
/// the paths are already canonical and the holder's principal is explicit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcquireLeaseRequest {
    pub principal: Principal,
    /// The acquiring session — stored on the lease so the `lease_grant`,
    /// `lease_renew`, and `lease_expire` audit events can be attributed
    /// (concept §13.1) without re-supplying it on renew/expire.
    pub session_id: SessionId,
    pub purpose: LeasePurpose,
    pub paths: Vec<CanonicalPath>,
}

/// Lazy version-index refresh (concept §4.2, §8.1): the endpoint hashed the
/// bytes it read and posts the result so subsequent resolves hit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefreshIndexRequest {
    pub path: CanonicalPath,
    pub version: VersionToken,
    pub mtime: DateTime<Utc>,
    pub size: u64,
}

/// Open an intent-journal entry for an in-flight write (concept §7 step 7).
/// Coord stamps `opened_at`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenJournalRequest {
    pub path: CanonicalPath,
    pub lease_id: LeaseId,
    pub principal: Principal,
    pub pre_image_version: VersionToken,
    /// `None` if the writer had not yet hashed the new content (concept §5.2).
    #[serde(default)]
    pub intended_version: Option<VersionToken>,
}

/// Clear a path's intent-journal entry on a clean commit (concept §7 step 11).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClearJournalRequest {
    pub path: CanonicalPath,
}

/// Result of storing a pre-image blob in the history store (concept §7 step 8,
/// §12).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutBlobResponse {
    pub version: VersionToken,
    pub size: u64,
    /// True if the blob already existed — a dedup hit; no bytes were written.
    pub deduplicated: bool,
}

/// Append a committed version to a file's version log (concept §7 step 11).
/// Coord fills `timestamp` and `prev_hash`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppendVersionLogRequest {
    pub path: CanonicalPath,
    pub blob_hash: VersionToken,
    pub writer_principal: Principal,
    pub size: u64,
    pub event: VersionEvent,
}

/// Query a file's version log (concept §6.5, backing `chapr.history`). Carries
/// the canonical path in the body — it cannot be a URL segment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryQuery {
    pub path: CanonicalPath,
}

/// Record an audit event driven by the endpoint (concept §5.2, §13.1) — e.g.
/// `write_commit` from the write path, which coord cannot observe on its own
/// (bytes never cross the control channel). Coord stamps `event_id`/timestamp.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordAuditRequest {
    pub principal: Principal,
    pub session_id: SessionId,
    pub path: CanonicalPath,
    pub kind: AuditKind,
    #[serde(default)]
    pub from_version: Option<VersionToken>,
    #[serde(default)]
    pub to_version: Option<VersionToken>,
    #[serde(default)]
    pub detail: String,
}

/// Register a CAS conflict (concept §11). The write path calls this after
/// parking the losing bytes in `sidecar_path`; coord records the open
/// `ConflictEntry` and audits `conflict_open`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterConflictRequest {
    pub base_path: CanonicalPath,
    pub sidecar_path: CanonicalPath,
    pub losing_principal: Principal,
    pub session_id: SessionId,
}

/// Query open conflicts under a canonical path prefix (concept §11, backing
/// `chapr.conflicts`). The endpoint canonicalises the tool's raw `scope` first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictsQuery {
    pub scope: CanonicalPath,
}

/// Explicitly resolve a conflict (concept §11, backing `chapr.resolve_conflict`).
/// Carries the acting principal/session so coord can audit `conflict_resolve`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolveConflictControl {
    pub conflict_id: ConflictId,
    pub resolution: ConflictResolution,
    pub principal: Principal,
    pub session_id: SessionId,
}

/// A record that a session has read a specific version of a path — the basis
/// for structural read-before-write (concept §6.2). Posted by `chapr.read`
/// (and by a write on commit, so the writer can chain) via `record`; checked
/// before a write/delete/move via `assert`. The model cannot fabricate a
/// `base_version` it never saw, because coord only honours ones it recorded
/// this session as having read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadReceipt {
    pub session_id: SessionId,
    pub path: CanonicalPath,
    pub version: VersionToken,
}

/// Migrate coord state for a completed rename (concept §6.3). The endpoint has
/// already done the SMB rename; coord then, in one transaction, re-keys the
/// version-log chain + open journal/conflict entries from `src` to `dst` (or,
/// on overwrite, discards `src`'s and continues `dst`'s), appends a `Move`
/// version-log entry, and audits. `version`/`size` describe the moved content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovePathsRequest {
    pub src: CanonicalPath,
    pub dst: CanonicalPath,
    pub version: VersionToken,
    pub size: u64,
    /// True when the destination already existed (move + overwrite).
    pub overwrite: bool,
    pub principal: Principal,
    pub session_id: SessionId,
}

/// Recover a dangling in-flight write (concept §8.1). Called by the read path
/// when `resolve` reported `journal_state = Dangling`: coord clears the journal
/// entry, records a `crash_recover` audit event stamped with this
/// `principal`/`session_id`, and returns the [`crate::RecoveredFrom`] the reader
/// needs to fetch and serve the pre-image. Coord never writes bytes to the
/// share, so the *serving* is the endpoint's job.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoverJournalRequest {
    pub path: CanonicalPath,
    /// The principal of the session performing the recovery (the reader).
    pub principal: Principal,
    pub session_id: SessionId,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_request_mode_defaults_to_cas_when_omitted() {
        let json = r#"{"uri":"x.md","content":[104,105],"base_version":"aa"}"#;
        // base_version must still be a valid token length to parse via from_hex?
        // WriteRequest carries VersionToken which is transparent (no validation
        // on deserialize), so a short string parses — validation is a coord-side
        // concern. We only assert the mode defaulting here.
        let req: WriteRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.mode, WriteMode::Cas);
        assert_eq!(req.content, b"hi");
    }

    #[test]
    fn read_response_omits_absent_optionals() {
        let resp = ReadResponse {
            content: ReadContent::Inline { bytes: b"hi".to_vec() },
            version: None,
            integrity: Integrity::Unverified,
            recovered_from: None,
            open_conflicts: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(!json.contains("version"));
        assert!(!json.contains("recovered_from"));
        assert!(!json.contains("open_conflicts"));
        assert!(json.contains(r#""integrity":"unverified""#));
    }

    #[test]
    fn read_content_is_tagged_on_kind() {
        let inline = serde_json::to_string(&ReadContent::Inline { bytes: vec![1, 2] }).unwrap();
        assert_eq!(inline, r#"{"kind":"inline","bytes":[1,2]}"#);
        let by_ref = serde_json::to_string(&ReadContent::Ref {
            content_ref: "blob://x".into(),
        })
        .unwrap();
        assert_eq!(by_ref, r#"{"kind":"ref","content_ref":"blob://x"}"#);
    }

    #[test]
    fn restore_mode_defaults_to_copy() {
        let req: RestoreRequest =
            serde_json::from_str(r#"{"uri":"x.md","version":"aa"}"#).unwrap();
        assert_eq!(req.mode, RestoreMode::Copy);
    }

    #[test]
    fn move_omits_dst_base_version_for_fresh_target() {
        let req = MoveRequest {
            src_uri: "a.md".into(),
            dst_uri: "b.md".into(),
            src_base_version: VersionToken::hash(b"a"),
            dst_base_version: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("dst_base_version"));
    }

    #[test]
    fn empty_responses_serialize_as_empty_objects() {
        assert_eq!(serde_json::to_string(&DeleteResponse {}).unwrap(), "{}");
        assert_eq!(serde_json::to_string(&MoveResponse {}).unwrap(), "{}");
    }

    #[test]
    fn resolve_request_carries_the_composite_key_stat() {
        let req = ResolveRequest {
            path: CanonicalPath::new_unchecked("\\\\srv\\share\\a.md"),
            mtime: DateTime::parse_from_rfc3339("2026-07-21T09:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            size: 4096,
        };
        let back: ResolveRequest =
            serde_json::from_str(&serde_json::to_string(&req).unwrap()).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn resolve_response_backend_is_wire_backward_compatible() {
        // An older coord's payload has no `backend` key → deserialises to None,
        // and the endpoint falls back to its local default (invariant 3).
        let old = r#"{"journal_state":"clean"}"#;
        let r: ResolveResponse = serde_json::from_str(old).unwrap();
        assert_eq!(r.backend, None);
        // A coord that classified nothing emits the exact same bytes (the field
        // is skipped when None), so old endpoints keep parsing it.
        assert_eq!(serde_json::to_string(&r).unwrap(), r#"{"journal_state":"clean"}"#);
        // And a classified response round-trips as Smb.
        let classified = ResolveResponse {
            backend: Some(crate::BackendDescriptor {
                kind: crate::BackendKind::Smb,
            }),
            ..r.clone()
        };
        let json = serde_json::to_string(&classified).unwrap();
        assert!(json.contains(r#""backend":{"kind":"smb"}"#));
        assert_eq!(serde_json::from_str::<ResolveResponse>(&json).unwrap(), classified);
    }
}
