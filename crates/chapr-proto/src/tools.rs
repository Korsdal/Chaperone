// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

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
//! [`ReadResponse`]/[`WriteRequest`] carry file bytes over the endpoint↔model
//! stdio channel. The heavy 200 MiB PDF a model *reads* flows endpoint → SMB →
//! model and never touches coord.
//!
//! Coord does see bytes, in exactly one direction and for exactly one purpose:
//! every write ships the file's **previous** contents to the history store as a
//! pre-image (`PUT /blobs`), because that snapshot is what history and crash
//! recovery are made of. That channel is bounded by `chapr_coord::http::
//! MAX_BLOB_BYTES` (256 MiB), which is also the largest file that can be written
//! at all — a write whose pre-image will not fit is refused up front by
//! `chapr_endpoint::backend::MAX_PRE_IMAGE_BYTES`.
//!
//! **Do not add a byte-carrying field to any coord-facing type in this module,
//! and do not add a byte-carrying coord *route* either.** This used to say only
//! the first half, and the gap was load-bearing: `PUT /blobs` sends its bytes as
//! a raw `application/octet-stream` body rather than as a type defined here, so
//! it never tripped the rule as written. Nobody sized that channel, it inherited
//! axum's 2 MiB `DefaultBodyLimit`, and every write to a file already larger than
//! 2 MiB failed — on the *pre-image*, which is why the ceiling looked unrelated to
//! the content being written and went undiagnosed until the pilot-readiness pass.
//! An invariant enforced on type shape alone does not hold; state the channel too.

use crate::enums::{
    AuditKind, ConflictResolution, EntryType, Integrity, JournalState, LeasePurpose, RestoreBase,
    RestoreMode, VersionEvent, WriteMode,
};
use crate::ids::{CanonicalPath, ConflictId, LeaseId, Principal, SessionId};
use crate::records::{ConflictEntry, LeaseRef, MoveJournalEntry, RecoveredFrom};
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
    /// File or directory — see [`EntryType`]. Without it a directory was
    /// `size: 0` and indistinguishable from an empty file.
    pub entry_type: EntryType,
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

/// `chapr.mkdir(uri, confirm_new)` — create one directory.
///
/// Directories were **first-class in the output and absent from the API**:
/// `chapr.list` returns them, and nothing could bring one into existence, so an
/// agent could see the shape of the tree and not extend it. That friction pushed
/// work outside Chaperone, which is the opposite of the point.
///
/// No CAS, no journal, no history — a directory has no content to version. It is
/// **non-recursive**: a missing parent is [`ChaprError::ParentMissing`], the same
/// answer `create` gives, so the tree is only ever extended one deliberate level
/// at a time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MkdirRequest {
    pub uri: String,
    /// Proceed even though the near-name guard found similar siblings.
    ///
    /// The guard exists because implicit or careless directory creation is how a
    /// share ends up with `Reports`, `reports` and `Repotrs` — and invariant 5
    /// keys coordination by canonical path, so each of those is a distinct,
    /// permanent key. It compares the requested name against existing siblings
    /// **deterministically** (normalised form plus edit distance), never by asking
    /// a model to judge similarity, and refuses with the candidates named.
    ///
    /// Setting this is the caller asserting the new name is deliberate. It is
    /// audited, so the assertion is attributable.
    #[serde(default)]
    pub confirm_new: bool,
}

/// `chapr.mkdir` success.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MkdirResponse {
    pub canonical_path: CanonicalPath,
    /// Similar sibling names that were present and overridden via
    /// `confirm_new`. Empty on an unambiguous create. Returned so the model can
    /// tell the human what it decided past, rather than the fact living only in
    /// the audit log.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub similar_existing: Vec<String>,
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

/// `chapr.move` success. The version-log chain and any open journal/conflict
/// entries are migrated to the new canonical path internally.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoveResponse {
    /// The moved content's version, at its new path.
    ///
    /// This response used to be empty, which made `move` the only verb that
    /// dropped something the caller demonstrably needs: `create` and `write`
    /// both return the new version, so a caller can chain a CAS write with no
    /// extra round trip, while after a move it had to read the destination
    /// first. The value is already in hand — a move CAS-verifies the source and
    /// the content is unchanged by a rename, so this is the source's verified
    /// version.
    pub version: VersionToken,
}

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

/// `chapr.restore(uri, version, mode)`. Takes the lease and the exclusive open,
/// snapshots the current bytes, and is itself versioned and audited — so a
/// restore can be undone. It performs **no CAS**: a restore is a deliberate
/// overwrite (D-012, D-027), so a version committed between the `chapr.history`
/// call and the restore is overwritten without a conflict, recoverable only
/// through the snapshot. `mode` defaults to `Copy` (concept §6.5).
///
/// **Concept §6.5's "full write path" now holds for `in_place`** — the
/// disagreement between spec and code is resolved in the spec's favour (Q13,
/// option (a)): an in-place restore CAS-checks the target against `base`.
/// `Copy` writes a fresh sibling and needs no base.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreRequest {
    pub uri: String,
    pub version: VersionToken,
    #[serde(default)]
    pub mode: RestoreMode,
    /// What the caller observed at the target: a version hex, or `"absent"` for
    /// a soft-deleted path. **Required for `in_place`** and ignored for `Copy`.
    /// See [`RestoreBase`] for why it is not optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<RestoreBase>,
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
    /// The pre-image this operation snapshotted into the blob store, when there
    /// was one. Coord records a [`VersionEvent::Baseline`] entry for it if the
    /// chain does not already name it; see [`PreImage`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_image: Option<PreImage>,
}

/// A snapshotted pre-image, named so coord can reference its blob from the
/// version log.
///
/// Carries the size explicitly because the version log records it and coord does
/// no file I/O of its own (invariant 1) — it cannot derive it, and the blob is
/// the endpoint's upload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreImage {
    pub version: VersionToken,
    pub size: u64,
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
    /// The destination's pre-image, present only on an overwrite-move. Those are
    /// bytes the rename destroys, so the endpoint snapshots them — and that blob
    /// needs the same [`VersionEvent::Baseline`] reference a write's pre-image
    /// does, or GC reclaims the only copy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dst_pre_image: Option<PreImage>,
}

/// Record the intent to rename, before the rename happens (B3, D-013).
///
/// D-013 accepted that the gap between the rename and the coord migration cannot
/// be one transaction, and called what it leaves "stale-but-recoverable". Until
/// this existed there was nothing to recover *from*: the rename committed, the
/// migration did not, and no record said the two paths belonged to one another —
/// so `dst` silently lost its lineage and `src`'s rows described a file that was
/// no longer there.
///
/// Every field [`MovePathsRequest`] needs is carried here, deliberately: whoever
/// completes the move afterwards is often **not** the session that opened it (a
/// crashed endpoint's successor), and must not have to reconstruct the payload
/// from a file it can only hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenMoveJournalRequest {
    pub src: CanonicalPath,
    pub dst: CanonicalPath,
    /// The move's all-or-none dual lease over `{src, dst}`. Its liveness is what
    /// separates a move still in flight from one that died — the same signal the
    /// write journal uses (concept §8.1).
    pub lease_id: LeaseId,
    pub principal: Principal,
    pub session_id: SessionId,
    /// The source's CAS-verified version: what `dst` must hash to for a
    /// recovering session to conclude the rename went through.
    pub version: VersionToken,
    pub size: u64,
    pub overwrite: bool,
    /// The destination's pre-image, present only on an overwrite-move — carried
    /// so a recovering session can name the blob the original snapshotted, which
    /// GC would otherwise reclaim as unreferenced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dst_pre_image: Option<PreImage>,
}

/// Drop a move-intent entry without migrating anything: the rename did not
/// happen, so there is nothing to complete.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClearMoveJournalRequest {
    pub src: CanonicalPath,
}

/// Move-intent entries whose lease is dead — a rename that may have committed
/// with its coord migration still owed.
///
/// *Dangling* means the same thing it means for a write: entry present, owning
/// lease gone. It does **not** mean the file is damaged. A move's bytes are
/// intact under one name or the other; what is stale is coord's bookkeeping,
/// which is why this is swept rather than served from the read path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DanglingMovesResponse {
    pub entries: Vec<MoveJournalEntry>,
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
        // `MoveResponse` is no longer empty: it carries the version that landed
        // at the destination, so a caller can chain a CAS write without reading
        // the file it just moved. It was the only verb that dropped that.
        let moved = MoveResponse {
            version: VersionToken::hash(b"moved"),
        };
        assert!(serde_json::to_string(&moved).unwrap().contains("version"));
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
