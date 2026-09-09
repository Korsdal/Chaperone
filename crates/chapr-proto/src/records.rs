// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! The persisted / cached record shapes (concept §5.2).
//!
//! These are the rows coord owns: the lease table, intent journal, per-file
//! version log, conflict registry, and audit log. They are defined here, in the
//! shared crate, because the endpoint constructs and reads them across the
//! control channel — the same types on both sides is what keeps the two
//! binaries in lockstep.
//!
//! Timestamps are [`chrono::DateTime<chrono::Utc>`], serialised as RFC 3339.
//! Durations that are genuinely a count of seconds (lease TTL) are `u32`
//! seconds, matching the concept's parameter table (§16).

use crate::enums::{AuditKind, ConflictResolution, ConflictState, LeasePurpose, VersionEvent};
use crate::ids::{CanonicalPath, ConflictId, EventId, LeaseId, Principal, SessionId};
use crate::tools::PreImage;
use crate::version::VersionToken;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A granted lease over one or more paths (concept §5.2, §9).
///
/// A single record can cover a *set* of paths — an all-or-none acquisition in
/// canonical order, which is what kills the classic A-holds-1-wants-2 deadlock.
/// TTL is decoupled from task length: renewal covers duration, `ttl_s` covers
/// only the gap between heartbeats.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseRecord {
    pub lease_id: LeaseId,
    /// Every path this lease covers, stored in canonical order.
    pub paths: Vec<CanonicalPath>,
    pub principal: Principal,
    pub granted_at: DateTime<Utc>,
    /// Heartbeat TTL in seconds (default 90; concept §16).
    pub ttl_s: u32,
    /// Updated on each successful renewal.
    pub renewed_at: DateTime<Utc>,
    /// `granted_at + max_lifetime`. On reaching this, coord force-expires the
    /// lease and re-acquisition requires a fresh read (default 20 min; §9).
    pub hard_expiry: DateTime<Utc>,
    pub purpose: LeasePurpose,
}

/// A compact view of the lease currently held over a file, as returned by
/// `chapr.stat` (concept §6.1). Not a full [`LeaseRecord`] — a stat caller only
/// needs to know who holds it and until when.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseRef {
    pub lease_id: LeaseId,
    pub principal: Principal,
    pub hard_expiry: DateTime<Utc>,
}

/// A record of an in-flight write (concept §5.2). The presence of a journal
/// entry is what the read state machine keys on; its owning `lease_id`
/// distinguishes a live write from a dangling (crashed) one (concept §8.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    pub path: CanonicalPath,
    pub lease_id: LeaseId,
    pub principal: Principal,
    /// The last known-good version, pointing into the history store — the
    /// pre-image to reinstate if this write turns out to have crashed.
    pub pre_image_version: VersionToken,
    /// The version the write intends to produce. `None` if the process crashed
    /// after opening the journal entry but before hashing the new content.
    pub intended_version: Option<VersionToken>,
    pub opened_at: DateTime<Utc>,
}

/// A record of an in-flight **move** (B3, D-013's "stale-but-recoverable").
///
/// Deliberately a separate record from [`JournalEntry`] rather than a nullable
/// field on it, for two reasons. A write journal answers "what bytes do I
/// reinstate at this path"; a move journal answers "which two paths belong to
/// each other, and what migration is still owed" — different questions with
/// different fields. And the entry is keyed by `src` while the *file* ends up at
/// `dst`, so it is not one path's state at all.
///
/// Recovery from it is decidable by hashing, and has exactly three outcomes:
/// `dst` hashes to `version` and `src` is gone (the rename committed → complete
/// the migration); `src` is still there (it did not → drop the entry, nothing
/// happened); neither (unresolvable → leave it and tell a human, per the
/// bounded-retry rule).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoveJournalEntry {
    pub src: CanonicalPath,
    pub dst: CanonicalPath,
    /// The dual lease over `{src, dst}`; its liveness is the live-vs-dangling
    /// signal, exactly as for a write.
    pub lease_id: LeaseId,
    pub principal: Principal,
    pub session_id: SessionId,
    /// The source's CAS-verified version — what `dst` must hash to.
    pub version: VersionToken,
    pub size: u64,
    pub overwrite: bool,
    /// The destination's snapshotted pre-image, on an overwrite-move only.
    pub dst_pre_image: Option<PreImage>,
    pub opened_at: DateTime<Utc>,
}

/// One append-only entry in a file's version log (concept §5.2, §12).
///
/// The log is per-file, append-only, and kept for the long audit-retention
/// window (1–3 years) — crucially, **even after the pre-image blob bytes are
/// GC'd at 90 days**, so "who changed this and when" is always answerable long
/// after the old bytes are gone. `prev_hash` chains entries; `blob_hash` is both
/// this version's content address and its identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionLogEntry {
    pub path: CanonicalPath,
    pub timestamp: DateTime<Utc>,
    /// Content address of this version — the blob-store key (invariant 2).
    pub blob_hash: VersionToken,
    pub writer_principal: Principal,
    /// The previous version in the chain, or `None` for the first version of a
    /// file.
    pub prev_hash: Option<VersionToken>,
    pub size: u64,
    pub event: VersionEvent,
}

/// A registered conflict sidecar (concept §5.2, §11).
///
/// Created when a CAS check fails: the losing agent's bytes are written to a
/// freshly-named `F.conflict-{user}-{ts}.ext` and registered here. Neither
/// party's bytes are ever lost, and Office binaries are never fake-merged.
/// The registry is the source of truth; conflicts surface on next touch of the
/// file or its directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictEntry {
    pub conflict_id: ConflictId,
    /// The live file the conflict is against.
    pub base_path: CanonicalPath,
    /// The `F.conflict-{user}-{ts}.ext` sidecar holding the losing bytes.
    pub sidecar_path: CanonicalPath,
    pub losing_principal: Principal,
    pub created_at: DateTime<Utc>,
    pub state: ConflictState,
    /// Set once the conflict is closed; `None` while `state == Open`.
    pub resolution: Option<ConflictResolution>,
}

/// One append-only audit event (concept §5.2, §13.1). The audit trail is a
/// primary deliverable: every lease, write, restore, conflict, and recovery
/// lands here stamped with the AD principal and session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub event_id: EventId,
    pub timestamp: DateTime<Utc>,
    pub principal: Principal,
    pub session_id: SessionId,
    pub canonical_path: CanonicalPath,
    pub kind: AuditKind,
    /// Version before the event, where meaningful (e.g. write, restore).
    pub from_version: Option<VersionToken>,
    /// Version after the event, where meaningful.
    pub to_version: Option<VersionToken>,
    /// Free-form human-readable context (e.g. the `force` reason, the conflict
    /// sidecar path, the recovery source).
    pub detail: String,
}

/// Provenance attached to a `chapr.read` response when a dangling write was
/// recovered before serving (concept §6.1, §8.1). Present only when
/// `integrity == Recovered`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveredFrom {
    /// The pre-image version that was reinstated.
    pub version: VersionToken,
    /// The principal whose write crashed and left the file torn.
    pub interrupted_writer: Principal,
    /// When the recovery happened.
    pub at: DateTime<Utc>,
    /// The version the interrupted write was trying to produce, as recorded at
    /// journal-open. The reader hashes the file and compares: an equal hash means
    /// the write actually committed and only failed to clear its journal entry,
    /// so the file is **not** torn and its own bytes are the newest content.
    /// Unequal — or `None`, when the writer died before hashing its content —
    /// means genuinely torn, and the pre-image is what may be served.
    #[serde(default)]
    pub intended_version: Option<VersionToken>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enums::VersionEvent;

    fn ts() -> DateTime<Utc> {
        // A fixed instant — records carry timestamps but constructing them is
        // the runtime's job, so tests use a literal rather than "now".
        DateTime::parse_from_rfc3339("2026-07-21T09:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn version_log_entry_round_trips() {
        let entry = VersionLogEntry {
            path: CanonicalPath::new_unchecked("\\\\srv\\share\\tenders\\eu-2026.md"),
            timestamp: ts(),
            blob_hash: VersionToken::hash(b"v2"),
            writer_principal: Principal::new_unchecked("CONTOSO\\jsmith"),
            prev_hash: Some(VersionToken::hash(b"v1")),
            size: 4096,
            event: VersionEvent::Write,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: VersionLogEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, back);
    }

    #[test]
    fn journal_entry_intended_version_is_optional() {
        let crashed = JournalEntry {
            path: CanonicalPath::new_unchecked("\\\\srv\\share\\a.txt"),
            lease_id: LeaseId::new_unchecked("L-7"),
            principal: Principal::new_unchecked("CONTOSO\\jsmith"),
            pre_image_version: VersionToken::hash(b"good"),
            intended_version: None, // crashed before hashing new content
            opened_at: ts(),
        };
        let json = serde_json::to_string(&crashed).unwrap();
        assert!(json.contains("\"intended_version\":null"));
        let back: JournalEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(crashed, back);
    }
}
