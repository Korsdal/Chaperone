// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! # chapr-proto — the Chaperone wire contract
//!
//! This crate is the single, shared definition of everything that crosses the
//! control channel between the local MCP server (`chapr-endpoint`) and the
//! coordination service (`chapr-coord`): the data model, the `chapr.*`
//! request/response types, and the exhaustive error enum. Both binaries import
//! it, so a change to the protocol that one side does not honour is a compile
//! error rather than a runtime surprise (implementation notes §3–4).
//!
//! ## What lives here — and what deliberately does not
//!
//! This crate is **pure data**. It has no async runtime, no I/O, no Win32, and
//! no networking. It defines the *shapes* that travel on the wire and the one
//! piece of logic inseparable from a shape — deriving a [`VersionToken`] from
//! bytes (invariant 2: `version = BLAKE3(file_bytes)`).
//!
//! Path canonicalisation (concept §5.1: DFS resolve, NFC, casefold, UNC) needs
//! the platform and belongs in `chapr-endpoint`. This crate carries the result
//! as an opaque [`CanonicalPath`] newtype and trusts that it was constructed
//! correctly — see that type's docs.
//!
//! ## The load-bearing invariants (concept §3)
//!
//! These types are designed around assertions the rest of the system relies on:
//!
//! 1. Ground truth is the file on the share; coord only caches. Every write
//!    re-derives the version under the lock.
//! 2. The version token *is* the content hash — one value is CAS change
//!    detector, history-store key, and audit chain link.
//! 3. Exclusive-open + CAS is the correctness core; leases are an optimisation.
//! 4. Version-check and write share one file handle (a TOCTOU race otherwise).
//! 5. All coordination state is keyed by canonical path.
//! 6. The write path carries bytes; the control channel carries metadata. These
//!    never cross — nothing in this crate's control-channel types should ever be
//!    made to carry a large file body.
//!
//! ## Module map
//!
//! - [`error`] — [`ChaprError`], the exhaustive failure enum. The crown jewel.
//! - [`version`] — [`VersionToken`], the BLAKE3 content hash.
//! - [`ids`] — typed newtypes: [`CanonicalPath`], [`Principal`], [`LeaseId`], …
//! - [`enums`] — closed enumerations shared across records and tools.
//! - [`records`] — the persisted/cached record shapes (concept §5.2).
//! - [`tools`] — the `chapr.*` request/response types (concept §6) plus the
//!   internal `coord.resolve` control-channel call (concept §8.1).

pub mod backend;
pub mod diagnostics;
pub mod enums;
pub mod error;
pub mod ids;
pub mod records;
pub mod tools;
pub mod version;

// Flat re-export of the whole contract. Downstream crates write
// `use chapr_proto::*` (or name a single type) without tracking module paths.
pub use backend::{BackendDescriptor, BackendKind};
pub use diagnostics::{
    DiagnosticGroup, DiagnosticOccurrence, DiagnosticReport, DiagnosticState, DiagnosticsQuery,
    DiagnosticsResponse, Severity,
};
pub use enums::{
    AbsentMarker, AuditKind, ConflictResolution, ConflictState, EntryType, Integrity,
    JournalState, LeasePurpose, RestoreBase, RestoreMode, VersionEvent, WriteMode,
};
pub use error::ChaprError;
pub use ids::{CanonicalPath, ConflictId, EventId, LeaseId, Principal, SessionId};
pub use records::{
    AuditEvent, ConflictEntry, JournalEntry, LeaseRecord, LeaseRef, MoveJournalEntry,
    RecoveredFrom, VersionLogEntry,
};
pub use tools::{
    AcquireLeaseRequest, AppendVersionLogRequest, ClearJournalRequest, ClearMoveJournalRequest,
    ConflictsQuery,
    ConflictsRequest, ConflictsResponse, CreateRequest, CreateResponse,
    DanglingMovesResponse, DeleteRequest, DeleteResponse, HistoryEntry, HistoryQuery,
    HistoryRequest, HistoryResponse,
    LeaseAcquireRequest, LeaseAcquireResponse, LeaseRenewRequest, LeaseRenewResponse,
    LeaseReleaseRequest, LeaseReleaseResponse, ListEntry, ListRequest, ListResponse,
    MkdirRequest, MkdirResponse,
    MovePathsRequest, MoveRequest, MoveResponse, OpenJournalRequest, OpenMoveJournalRequest,
    PreImage, PutBlobResponse,
    ReadContent, ReadReceipt, ReadRequest, ReadResponse,
    RecordAuditRequest, RecoverJournalRequest, RefreshIndexRequest, RegisterConflictRequest,
    ResolveConflictControl, ResolveConflictRequest, ResolveConflictResponse, ResolveRequest,
    ResolveResponse, RestoreRequest, RestoreResponse, StatRequest, StatResponse, WriteRequest,
    WriteResponse,
};
pub use version::VersionToken;

/// The `chapr.*` tool namespace prefix. Naming is settled (see the README); the
/// spec's historical `fs.*` / `FMCP` names must never appear in new code,
/// audit event names, or identifiers.
pub const TOOL_NAMESPACE: &str = "chapr";
