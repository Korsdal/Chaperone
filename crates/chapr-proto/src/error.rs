// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! [`ChaprError`] — the exhaustive failure enum.
//!
//! This is the reason the project is in Rust (implementation notes §1): the
//! failure modes this system is built around — CAS races, dangling journals,
//! stale tokens, Office locks, retry storms — are exactly the class that
//! `Result` + an exhaustive `match` force a caller to handle rather than
//! forget. Every variant here corresponds to a named branch in the concept's
//! state machines and failure table (§7, §8, §10).
//!
//! **When you add a failure mode to the system, add a variant here first.** A
//! new failure that is not representable in this enum is a failure that some
//! caller will handle by ignoring it.
//!
//! ## Wire form
//!
//! Internally tagged on a `code` field in `SCREAMING_SNAKE_CASE`, matching the
//! concept's error names (`LEASE_HELD`, `CONFLICT`, …):
//!
//! ```json
//! { "code": "LEASE_HELD", "holder": "CONTOSO\\jsmith", "paths": ["…"] }
//! { "code": "CONFLICT", "current_version": "…", "sidecar_path": "…", … }
//! { "code": "COORD_UNREACHABLE" }
//! ```

use crate::ids::{CanonicalPath, ConflictId, LeaseId, Principal};
use crate::version::VersionToken;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Every way a `chapr.*` operation can fail. Serialisable so it travels the
/// control channel intact, and `std::error::Error` via `thiserror` so it
/// composes with `?` inside each binary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Error)]
#[serde(tag = "code", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ChaprError {
    // ---- Leases (concept §9) ---------------------------------------------
    /// The requested lease (or one path of an all-or-none set) is held by
    /// another principal. The agent backs off or asks the human (§7 step 2).
    #[error("lease held by {holder} on {paths:?}")]
    LeaseHeld {
        holder: Principal,
        /// The specific paths that were unavailable. For a set acquisition this
        /// may be a subset of what was requested — the acquisition is still
        /// all-or-none, so nothing was granted.
        paths: Vec<CanonicalPath>,
    },

    /// A renew/release named a lease coord does not know (never granted, or
    /// already reaped).
    #[error("no such lease: {lease_id}")]
    LeaseNotFound { lease_id: LeaseId },

    /// The lease's heartbeat TTL elapsed before renewal. The write it covered
    /// must be treated as failed and the file re-read before retrying (§15).
    #[error("lease {lease_id} expired")]
    LeaseExpired { lease_id: LeaseId },

    /// The renewal thread failed to reach coord and marked held leases lost —
    /// e.g. a laptop returning from sleep (§15). Distinct from `LeaseExpired`:
    /// the endpoint noticed first, before coord reaped it.
    #[error("lease {lease_id} lost (renewal failed)")]
    LeaseLost { lease_id: LeaseId },

    /// The lease hit its 20-minute hard lifetime ceiling and coord
    /// force-expired it, defending against an agent stuck renewing forever
    /// (§9). Re-acquisition requires a fresh read (new base version).
    #[error("lease {lease_id} hit its hard lifetime ceiling at {hard_expiry}")]
    MaxLeaseLifetimeExceeded {
        lease_id: LeaseId,
        hard_expiry: DateTime<Utc>,
    },

    // ---- The CAS / write core (concept §7) -------------------------------
    /// CAS mismatch: the file changed since the agent read it. The agent's
    /// bytes were written to `sidecar_path` and a conflict registered; neither
    /// party's bytes are lost. The human reconciles (§7 step 6, §11).
    /// The message names `base_path` — the file the conflict is *on* — and
    /// mentions the sidecar only when one exists.
    ///
    /// It used to open `"write conflict on {sidecar_path}"`, which read as though
    /// the conflict had happened on the sidecar; the sidecar is the *outcome*.
    /// That was not a wording slip. This variant carried **only** `sidecar_path`,
    /// so `delete` and `move` — which have no losing content to park — set it to
    /// the file itself to make the message read correctly, and created no sidecar.
    /// Two verbs therefore returned a `sidecar_path` pointing at a live file. The
    /// two paths are now distinct fields, so no verb has to lie about either.
    #[error("{}", conflict_message(base_path, current_version, last_writer, sidecar_path.as_ref()))]
    Conflict {
        /// The live file the conflict is on. Always set.
        base_path: CanonicalPath,
        /// The version found under the lock (`V_now`) — what the file actually
        /// is now, not what the agent expected.
        current_version: VersionToken,
        last_writer: Principal,
        /// When the file was last written.
        when: DateTime<Utc>,
        /// Where the losing agent's bytes were parked, when there were any.
        /// `None` for verbs that produce no losing content (`delete`, `move`,
        /// and a refused `restore`): the meaning is "re-read and decide", not
        /// "your bytes are over here".
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sidecar_path: Option<CanonicalPath>,
    },

    /// The path resolved to somewhere outside this endpoint's coordinated
    /// root(s) (E-025).
    ///
    /// Its own variant rather than an `InvalidPath` with a telling `reason`,
    /// because two different readers need to tell it apart from a malformed path:
    /// the audit trail, which records `outside_root` as the one refusal that
    /// answers "did an agent probe outside the share", and whoever changes the
    /// message later — deriving a machine code by matching on prose is how the
    /// code silently stops matching.
    ///
    /// The message carries all three parts a user needs to act (what they asked
    /// for, what it resolved to, what the root is) plus where the root came from
    /// and when it was read, since a stale root and a wrong root look identical
    /// otherwise.
    /// All three paths are formatted **plainly**, with literal quotes rather than
    /// `{x:?}`. Debug-formatting doubles every backslash, so the input showed
    /// `Z:\\payroll` beside a root shown as `\\srv\share` — and the whole point
    /// of this message is that a person compares the three by eye. Reported from
    /// a cowork session as an escaping inconsistency, withdrawn as
    /// unreproducible, and in fact present on every refusal.
    #[error("invalid path \"{raw}\": resolves to \"{resolved}\", which is outside this endpoint's coordinated root(s): {}{provenance}", roots.join(", "))]
    OutsideRoot {
        /// What the caller asked for, before resolution.
        raw: String,
        /// The canonical form it resolved to.
        resolved: String,
        /// The configured roots it was compared against.
        roots: Vec<String>,
        /// Which setting the roots came from, and when — empty when unknown.
        provenance: String,
    },

    /// `chapr.mkdir` was asked for a directory whose name closely resembles a
    /// sibling that already exists.
    ///
    /// Refused rather than warned. A warning lands in a model's context where it
    /// may be paraphrased away, and by then the near-duplicate directory exists —
    /// which is the state worth preventing, because invariant 5 keys coordination
    /// by canonical path and `Reports`, `reports` and `Repotrs` are three
    /// permanent keys. `confirm_new` proceeds, and is audited.
    #[error("{path} closely resembles {} that already exist{} here: {}. If the new name is deliberate, set confirm_new; otherwise use the existing one", if similar.len() == 1 { "a directory" } else { "directories" }, if similar.len() == 1 { "s" } else { "" }, similar.join(", "))]
    NearDuplicateName {
        path: CanonicalPath,
        /// The sibling names that matched, in listing order.
        similar: Vec<String>,
    },

    /// A create-shaped operation named a path whose **parent directory** does not
    /// exist. Distinct from `NotFound`, and the distinction is the whole point:
    /// reporting `not found` for the file a caller asked to *create* sends them
    /// to inspect the one path they got right, and the create's own premise is
    /// that the file is absent.
    #[error("cannot create {path}: its parent directory {parent} does not exist{}", mkdir_hint())]
    ParentMissing {
        path: CanonicalPath,
        parent: CanonicalPath,
    },

    /// `base_version` was supplied but coord has no record of this session
    /// having read it. Read-before-write is structurally enforced: the model
    /// cannot fabricate a token it never saw (concept §6.2). Distinct from
    /// `Conflict` — this is rejected *before* the write path even starts.
    #[error("base_version for {path} was never read by this session")]
    BaseVersionNotRecorded {
        path: CanonicalPath,
        provided: VersionToken,
    },

    /// A `write` arrived with no `base_version`. Required, not optional — a
    /// model omits optional fields under pressure (concept §6.2).
    #[error("write to {path} is missing the required base_version")]
    BaseVersionRequired { path: CanonicalPath },

    /// `mode = "force"` was requested without the mandatory `reason` string
    /// (concept §6.2). Normally unrepresentable given [`crate::enums::WriteMode`]
    /// carries the reason, but kept for adapters that build the request from
    /// looser input.
    #[error("force write to {path} requires a reason")]
    ForceRequiresReason { path: CanonicalPath },

    // ---- SMB / OS-level (concept §7 steps 3–4) ---------------------------
    /// An Office lock file (`~$F`) is present. The write is refused — humans
    /// always win; leases are advisory with respect to Excel (§7 step 3, §10).
    #[error("{path} is open in Office (lock file {lock_file} present)")]
    OfficeLockPresent {
        path: CanonicalPath,
        /// The `~$F` lock file that was detected.
        lock_file: CanonicalPath,
    },

    /// The exclusive open (`share = NONE`) failed because another handle is
    /// open — someone got there between the pre-flight and the open (§7 step 4).
    /// The caller backs off and retries.
    #[error("sharing violation opening {path} exclusively")]
    SharingViolation { path: CanonicalPath },

    // ---- Recovery (concept §8.1) -----------------------------------------
    /// A dangling journal entry was found but the pre-image it points at is
    /// missing from the history store, so recover-then-serve cannot complete.
    /// A genuine data-integrity event, not an expected branch — surfaces to a
    /// human.
    #[error("cannot recover {path}: pre-image {missing_version} is gone from history")]
    RecoveryFailed {
        path: CanonicalPath,
        missing_version: VersionToken,
    },

    // ---- Availability (concept §10) --------------------------------------
    /// The control channel to coord is unreachable. On a **write** this is
    /// terminal and fail-closed: no journal entry, no write (§7, §10). On a
    /// read the endpoint degrades open instead and never raises this.
    #[error("coordination service unreachable")]
    CoordUnreachable,

    /// Bounded retries against repeated BUSY/`LEASE_HELD` were exhausted. This
    /// is the terminal "ask the human" state and is part of the tool contract —
    /// it exists because an LLM will otherwise retry forever (§10).
    #[error("retry budget exhausted for {path} after {attempts} attempts — ask the human")]
    RetryBudgetExhausted { path: CanonicalPath, attempts: u32 },

    // ---- Lookup / existence ----------------------------------------------
    /// The path does not exist on the share.
    #[error("not found: {path}")]
    NotFound { path: CanonicalPath },

    /// A `create` targeted a path that already exists.
    #[error("already exists: {path}")]
    AlreadyExists { path: CanonicalPath },

    /// The user's own Kerberos'd open was denied by the file's ACL. Content
    /// access stays safe precisely because this can happen (concept §13.1) —
    /// coord never reads bytes on the user's behalf.
    #[error("permission denied: {path}")]
    PermissionDenied { path: CanonicalPath },

    /// `history`/`restore` named a version not in the file's version log.
    #[error("version {version} not found for {path}")]
    VersionNotFound {
        path: CanonicalPath,
        version: VersionToken,
    },

    /// `resolve_conflict` named an unknown conflict id.
    #[error("no such conflict: {conflict_id}")]
    ConflictNotFound { conflict_id: ConflictId },

    // ---- Malformed input --------------------------------------------------
    /// A supplied path could not be canonicalised (concept §5.1). Carries the
    /// raw input and why it failed. Never let an un-canonicalised path key
    /// coordination state (invariant 5).
    #[error("invalid path {raw:?}: {reason}")]
    InvalidPath { raw: String, reason: String },

    // ---- Committed-but-unrecorded ----------------------------------------
    /// The mutation reached the share (committed, handle closed) but coord could
    /// not record it afterwards — the version-log or audit append failed. The
    /// share **is** the current state; history and audit are missing this entry,
    /// and no read receipt was recorded either.
    ///
    /// Distinct from [`Self::Internal`] on purpose: the caller must not retry
    /// the operation (that would act against its own committed state) and must
    /// re-read before writing again. Reporting a bare failure here would tell
    /// the caller the opposite of what happened.
    ///
    /// **Not write-only.** `create`, `delete`, `restore` and `move` all reach for
    /// this variant for their post-mutation tails, so the wording stays
    /// verb-neutral: it once said "write to {path} committed on disk", which was
    /// already false for a delete and for a move (the renamed file is at `path`,
    /// nothing was written there). `version` names the version involved — the new
    /// head for a write or create, the pre-image for a delete, the source's
    /// version for a move — not necessarily what the path now hashes to.
    #[error(
        "the change to {path} completed on the share ({version}), but recording it failed: \
         {message}; the share holds the result — re-read before writing again"
    )]
    CommittedButUnrecorded {
        path: CanonicalPath,
        version: VersionToken,
        message: String,
    },

    // ---- Catch-alls -------------------------------------------------------
    /// An SMB/OS I/O error with no more specific variant above. Prefer a
    /// specific variant where one exists; this is the honest fallback, not a
    /// dumping ground.
    #[error("I/O error on {path}: {message}")]
    Io { path: CanonicalPath, message: String },

    /// An unexpected internal error inside coord or the endpoint. Indicates a
    /// bug, not an expected failure branch.
    #[error("internal error: {message}")]
    Internal { message: String },
}

/// Convenience alias for fallible `chapr.*` operations.
pub type Result<T> = std::result::Result<T, ChaprError>;

/// The `Conflict` message: names the file the conflict is on, and the sidecar
/// only when one was written.
///
/// A free function rather than a format string because the sidecar clause is
/// conditional, and `thiserror`'s attribute cannot branch. Keeping the two cases
/// here — instead of at each raise site — is what stops a verb inventing its own
/// phrasing for the same condition.
fn conflict_message(
    base_path: &CanonicalPath,
    current_version: &VersionToken,
    last_writer: &Principal,
    sidecar_path: Option<&CanonicalPath>,
) -> String {
    let head =
        format!("conflict on {base_path}: it is now {current_version}, last written by {last_writer}");
    match sidecar_path {
        Some(sidecar) => format!("{head}. Your bytes were parked at {sidecar} — neither version is lost, and a human reconciles them"),
        None => format!("{head}. Nothing was changed and nothing was parked: re-read the file and decide"),
    }
}

/// The remedy clause on [`ChaprError::ParentMissing`].
///
/// Split out so the advice is written once. It names `chapr_mkdir` because that
/// is the only way to create a directory through Chaperone — parents are never
/// created implicitly, since implicit creation is how a share acquires `Reports`,
/// `reports` and `Repotrs` side by side, and invariant 5 makes each of those a
/// distinct coordination key permanently.
fn mkdir_hint() -> &'static str {
    ". Chaperone never creates parent directories implicitly — create it with \
     chapr_mkdir first, or write to a directory that already exists"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tagged_by_screaming_snake_code() {
        let e = ChaprError::CoordUnreachable;
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"code":"COORD_UNREACHABLE"}"#
        );
    }

    #[test]
    fn conflict_round_trips_with_all_fields() {
        let e = ChaprError::Conflict {
            base_path: CanonicalPath::new_unchecked("\\\\srv\\share\\q3.xlsx"),
            current_version: VersionToken::hash(b"now"),
            last_writer: Principal::new_unchecked("CONTOSO\\bthomas"),
            when: DateTime::parse_from_rfc3339("2026-07-21T09:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            sidecar_path: Some(CanonicalPath::new_unchecked(
                "\\\\srv\\share\\q3.xlsx.conflict-jsmith-20260721.xlsx",
            )),
        };
        // The subject of the message is the file, not the sidecar — the sidecar
        // is where the losing bytes went. Getting this backwards read as though
        // the conflict had happened on a file the caller had never heard of.
        let msg = e.to_string();
        assert!(
            msg.starts_with("conflict on \\\\srv\\share\\q3.xlsx"),
            "the message must open with the file: {msg}"
        );
        assert!(msg.contains("parked at"), "and still name the sidecar: {msg}");
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains(r#""code":"CONFLICT""#));
        let back: ChaprError = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn lease_held_carries_holder_and_paths() {
        let e = ChaprError::LeaseHeld {
            holder: Principal::new_unchecked("CONTOSO\\bthomas"),
            paths: vec![CanonicalPath::new_unchecked("\\\\srv\\share\\a.md")],
        };
        let back: ChaprError = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn display_messages_are_human_readable() {
        let e = ChaprError::RetryBudgetExhausted {
            path: CanonicalPath::new_unchecked("\\\\srv\\share\\a.md"),
            attempts: 5,
        };
        assert!(e.to_string().contains("ask the human"));
    }

    /// A `delete` or `move` conflict has no sidecar, and must not imply one. The
    /// old shape could not express that: the field was required, so those verbs
    /// passed the file itself and every caller was told a sidecar existed.
    #[test]
    fn a_conflict_without_a_sidecar_does_not_claim_one() {
        let e = ChaprError::Conflict {
            base_path: CanonicalPath::new_unchecked("\\\\srv\\share\\a.md"),
            current_version: VersionToken::hash(b"now"),
            last_writer: Principal::new_unchecked("CONTOSO\\bthomas"),
            when: Utc::now(),
            sidecar_path: None,
        };
        let msg = e.to_string();
        assert!(msg.contains("nothing was parked"), "{msg}");
        assert!(!msg.contains("parked at"), "must not name a sidecar: {msg}");
        // And the absent field is omitted on the wire, not serialised as null.
        let json = serde_json::to_string(&e).unwrap();
        assert!(!json.contains("sidecar_path"), "{json}");
    }

    /// The three-part boundary refusal is the one message a cowork session could
    /// actually act on, so its parts are pinned: what was asked, what it resolved
    /// to, and what the root is. Plus the provenance that separates a wrong root
    /// from a stale one.
    #[test]
    fn the_boundary_refusal_names_all_three_parts() {
        let e = ChaprError::OutsideRoot {
            raw: "Z:\\payroll\\x.md".into(),
            resolved: "\\\\filesrv\\payroll\\x.md".into(),
            roots: vec!["\\\\filesrv\\aicollab".into()],
            provenance: " (root read from CHAPR_ROOT when this endpoint started, \
                          2026-09-09T11:42:03Z; if you changed it since, restart the extension)"
                .into(),
        };
        let msg = e.to_string();
        for part in [
            "Z:\\payroll\\x.md",
            "\\\\filesrv\\payroll\\x.md",
            "\\\\filesrv\\aicollab",
            "restart the extension",
        ] {
            assert!(msg.contains(part), "missing {part:?} in: {msg}");
        }
        // The resolved path must not arrive Debug-escaped: `\\\\` where the root
        // shows `\\` made the two look mismatched and read as a resolver bug.
        assert!(
            !msg.contains("\\\\\\\\filesrv"),
            "resolved path was double-escaped: {msg}"
        );
    }
}
