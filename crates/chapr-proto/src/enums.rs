// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! Closed enumerations shared across records ([`crate::records`]) and the tool
//! surface ([`crate::tools`]).
//!
//! Wire representation is lowercase `snake_case` strings for the simple
//! discriminators, chosen to match the concept doc's spelled-out states and to
//! read cleanly in the audit log and SQLite. Enums that carry data document
//! their tagging individually.

use crate::version::VersionToken;
use serde::{Deserialize, Serialize};

/// Trust level attached to bytes returned by `chapr.read` (concept §6.1, §8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Integrity {
    /// Bytes were served with a coord-confirmed version. The clean hot path.
    Verified,
    /// A dangling (crashed) write was detected and the pre-image restored
    /// before serving; the reader never saw torn bytes (concept §8.1). Pairs
    /// with [`crate::records::RecoveredFrom`].
    Recovered,
    /// Coord was unreachable, so phase-one metadata could not complete. Bytes
    /// were served straight from SMB and the version is omitted. Reads
    /// degrade-open; writes do not (concept §8.3, §10).
    Unverified,
}

/// The three states the read state machine resolves a file into, from the cheap
/// `coord.resolve` call (concept §8.1). The journal entry carries its owning
/// lease id, which is what distinguishes `Live` from `Dangling`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalState {
    /// No journal entry — serve bytes and version directly. Hot path.
    Clean,
    /// Journal entry present, owning lease still alive — a write is in flight
    /// under an exclusive handle. Bounded brief wait, then serve.
    Live,
    /// Journal entry present, owning lease dead — possibly torn from a crashed
    /// write. Triggers recover-then-serve.
    Dangling,
}

/// Whether a registered conflict is still open (concept §11).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictState {
    Open,
    Resolved,
}

/// How a conflict was closed. Recorded on the [`crate::records::ConflictEntry`]
/// and audited (concept §11). `InferredFromDeletion` is kept distinct so audit
/// can tell a real human resolution from someone merely tidying the sidecar off
/// the share.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictResolution {
    /// Kept the losing (sidecar) version.
    KeptMine,
    /// Kept the winning (base) version; discarded the sidecar's changes.
    KeptTheirs,
    /// A human merged the two by hand.
    Merged,
    /// The sidecar's changes were thrown away.
    Discarded,
    /// The watcher saw the sidecar deleted off the share and auto-closed the
    /// entry. Distinct from an explicit `chapr.resolve_conflict` call.
    InferredFromDeletion,
}

/// The kind of an [`crate::records::AuditEvent`] (concept §5.2). Append-only;
/// every coordination-significant action lands here stamped with a principal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditKind {
    LeaseGrant,
    LeaseRenew,
    LeaseExpire,
    WriteCommit,
    Restore,
    ConflictOpen,
    ConflictResolve,
    CrashRecover,
    /// A directory created through `chapr.mkdir`. Its own kind rather than
    /// `WriteCommit` because no content was versioned — there is no blob, no
    /// pre-image and no version to record, so calling it a write commit would
    /// put a row in the trail that no `chapr.history` can corroborate.
    DirCreate,
    /// An operation Chaperone **refused**, including reads.
    ///
    /// The trail recorded only committed changes, so an agent blocked by the
    /// coordinated-root boundary left no trace at all — a hole in something sold
    /// as accountability, and it makes "did an agent probe outside the root"
    /// unanswerable.
    ///
    /// One kind for every refusal, with the reason as a machine-greppable prefix
    /// in `detail` (`refused[outside_root] chapr_write \\srv\share\f.md: …`).
    /// A structured reason column would query better and needs a schema change
    /// `CREATE TABLE IF NOT EXISTS` cannot deliver until C0 lands; a prefix is
    /// searchable by substring today and can become a column later.
    ///
    /// Only refusals that are **decisions** land here — the boundary, a binary
    /// container, non-UTF-8 text, a CAS conflict, an Office lock, a missing base
    /// version. A coordinator outage is not a decision, and auditing it would
    /// write one row per read in a read-heavy workload.
    Refused,
}

/// The event that produced a [`crate::records::VersionLogEntry`]. Narrower than
/// [`AuditKind`]: the version log records only events that mint a new content
/// version for a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionEvent {
    /// Content Chaperone did not author, recorded the first time a write
    /// snapshots it. A file that existed before any agent touched it — or one a
    /// human edited out of band between two agent writes — enters the chain
    /// here.
    ///
    /// Without this entry the snapshotted pre-image blob is named by nothing:
    /// the version log records the version a write *produced*, while the blob
    /// store holds the one it *replaced*, so an unreferenced pre-image is
    /// reclaimed by blob GC and the file's pre-agent state becomes
    /// unrecoverable — the one thing history exists to prevent.
    Baseline,
    /// First version of a newly created file (`chapr.create`).
    Create,
    /// An ordinary in-place overwrite (`chapr.write`).
    Write,
    /// A write that **forced past a CAS mismatch**, deliberately discarding a
    /// concurrent edit (`mode = "force"` with a reason).
    ///
    /// Its own event because the alternative was worse than it looks: a forced
    /// write was recorded as `Write`, shape-identical to the two legitimate
    /// writes around it, so the most governance-relevant fact about that version
    /// was invisible in `chapr.history`. The reason string stays in the audit
    /// log — **agents assess provenance from history, humans read the panel**,
    /// and only the first of those needed fixing here.
    WriteForced,
    /// A soft delete — the pre-image is snapshotted before removal.
    Delete,
    /// A restore, itself versioned so you can undo an undo (concept §6.5).
    Restore,
    /// The destination side of a move/rename that overwrote an existing target.
    Move,
    /// A pre-image reinstated by crash recovery (concept §8.1 dangling path).
    Recover,
}

/// Why a lease set is being acquired. Leases are taken **only for write intent**
/// — pure reads take no lease (concept §6.4), which is critical for the
/// read-heavy workload. This enum enumerates the mutating verbs rather than
/// carrying a free-form string, so the audit log's `purpose` is a closed set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeasePurpose {
    Write,
    Create,
    Delete,
    /// Move/rename — the one verb that acquires an all-or-none set over
    /// `{src, dst}` (concept §6.3).
    Move,
    /// Restore runs the full contended-write path and so takes a lease too
    /// (concept §6.5).
    Restore,
}

/// Write concurrency mode (concept §6.2).
///
/// Internally tagged on `mode` so `Force` structurally *carries* its mandatory `reason`:
/// there is no way to construct a force write without a justification, and the
/// reason lands in the audit log. Models are measurably reluctant to invoke
/// tools that demand a justification — that reluctance is the point.
///
/// Wire form: `{"mode":"cas"}` or `{"mode":"force","reason":"…"}`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum WriteMode {
    /// Compare-and-swap against `base_version`. The default and correct path.
    #[default]
    Cas,
    /// Bypass the CAS check. Requires a human-meaningful reason, audited.
    Force { reason: String },
}

/// Restore target mode (concept §6.5). Defaults to `Copy` so a restore never
/// silently clobbers the current file — `InPlace` is an explicit opt-in that
/// still runs the full contended-write path.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreMode {
    /// Write the old version as `F.restored-{ts}.ext` for human comparison.
    #[default]
    Copy,
    /// Overwrite the live file in place. Requires a [`crate::RestoreBase`]
    /// describing the state the caller observed — see that type for why.
    InPlace,
}

/// What a caller observed at the target before asking for an in-place restore.
///
/// ## Why this exists
///
/// `restore(in_place)` performed **no CAS at all** — it reinstated old bytes over
/// whatever was there. Data-loss prevention is the whole point of Chaperone, and
/// that made restore the one verb handed a bazooka: an agent could destroy current
/// content it had never read, with no conflict, no sidecar and nothing parked.
///
/// Requiring this field makes the rule structural rather than advisory: **an agent
/// cannot restore over content it has not looked at**, because it has to say what
/// it saw. It also retires a lie — an in-place restore of a soft-deleted file used
/// to report `not found` for a path whose history resolved and whose copy-restore
/// worked. Absence is now a state a caller can describe, not an error.
///
/// Required, never `Option`: a model omits optional fields under pressure
/// (the same reasoning as `write`'s `base_version`, concept §6.2).
///
/// Wire form is a string: `"absent"`, or the version hex.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RestoreBase {
    /// The caller expects **no file** at the path — it was soft-deleted, and
    /// restoring recreates it at its original name. A file found there instead is
    /// a conflict: something was created since the caller looked.
    Absent(AbsentMarker),
    /// The caller read this version at the path. A different version there now is
    /// a conflict; the same version is safe to overwrite.
    Version(VersionToken),
}

/// The literal `"absent"` in [`RestoreBase`], as its own type so `serde`'s
/// untagged representation cannot confuse it with a version hex.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbsentMarker {
    #[default]
    Absent,
}

/// Whether a directory entry is a file or a directory (`chapr.list`).
///
/// Listings carried `name`, `size` and `mtime` and nothing else, so a directory
/// appeared with `size: 0` — indistinguishable from an empty file. A model reading
/// that listing has to guess, in the one surface whose job is telling an agent what
/// is there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryType {
    File,
    Dir,
    /// Neither a regular file nor a directory — a symlink, socket, device, or a
    /// reparse point Windows reports as none of the above. Its own variant rather
    /// than being folded into `File`, because Chaperone cannot coordinate it and
    /// an agent should not be told it can.
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_enums_are_snake_case_strings() {
        assert_eq!(
            serde_json::to_string(&Integrity::Unverified).unwrap(),
            "\"unverified\""
        );
        assert_eq!(
            serde_json::to_string(&AuditKind::WriteCommit).unwrap(),
            "\"write_commit\""
        );
        assert_eq!(
            serde_json::to_string(&JournalState::Dangling).unwrap(),
            "\"dangling\""
        );
    }

    #[test]
    fn write_mode_force_carries_reason_on_the_wire() {
        let json = serde_json::to_string(&WriteMode::Force {
            reason: "manual override, ticket OPS-42".into(),
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"mode":"force","reason":"manual override, ticket OPS-42"}"#
        );

        let cas = serde_json::to_string(&WriteMode::Cas).unwrap();
        assert_eq!(cas, r#"{"mode":"cas"}"#);

        let back: WriteMode = serde_json::from_str(&cas).unwrap();
        assert_eq!(back, WriteMode::Cas);
    }

    #[test]
    fn defaults_are_the_safe_directions() {
        assert_eq!(WriteMode::default(), WriteMode::Cas);
        assert_eq!(RestoreMode::default(), RestoreMode::Copy);
    }
}
