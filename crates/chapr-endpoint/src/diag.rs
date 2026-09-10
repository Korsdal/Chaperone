// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! Reporting unexpected failures (E-026, D-030).
//!
//! ## The principle
//!
//! *We cannot anticipate every environment failure, but we can make failures
//! discoverable.* That is why this module exists instead of a longer list of
//! questions for the customer's IT administrator. An FSRM file screen rejecting a
//! conflict sidecar, a path over the length limit, antivirus holding a handle open,
//! a Mac client writing NFD filenames — each shows up here at runtime rather than
//! being something somebody had to think to ask about.
//!
//! ## Two sinks, and why both are needed
//!
//! 1. **Coord**, so an administrator can see every laptop's failures in one place
//!    without visiting any of them.
//! 2. **A local file**, because a failure that happens *before* coord is reachable
//!    — wrong URL, TLS mismatch, blocked port — cannot phone home. The endpoint is
//!    a stdio child of Claude Desktop and its stderr goes nowhere, which is
//!    precisely the gap that made such failures unexplainable.
//!
//! ## Only the unexpected
//!
//! A CAS conflict and a document a human has open in Word are **designed
//! outcomes**. They belong in conflict and lease status, with their own vocabulary,
//! and recording them here would bury the entry an administrator actually needs.
//! [`classify`] is where that line is drawn, and returning `None` is a deliberate
//! answer rather than a gap.
//!
//! Every classified failure carries a **remedy**. A diagnostic that only says what
//! broke sends the reader back to the source code.

use crate::coord_client::CoordClient;
use chapr_proto::{ChaprError, DiagnosticReport, Principal, Severity};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;

/// Rotate the local log at this size, keeping one previous file. Bounded at twice
/// this on disk — a laptop log that grows forever is its own incident.
const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

/// Reports unexpected failures to coord and to a local file.
pub struct Diagnostics {
    host: Option<String>,
    log_path: Option<PathBuf>,
}

impl Diagnostics {
    /// `log_path` of `None` disables the local sink (used by tests).
    pub fn new(log_path: Option<PathBuf>) -> Self {
        Diagnostics {
            host: hostname(),
            log_path,
        }
    }

    /// The default local log location — discoverable without configuration, which
    /// is the whole point: someone supporting a laptop should not need to be told
    /// where to look.
    pub fn default_log_path() -> Option<PathBuf> {
        let base = if cfg!(windows) {
            std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
        } else {
            std::env::var_os("XDG_STATE_HOME")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state"))
                })
        }?;
        Some(base.join("Chaperone").join("diagnostics.jsonl"))
    }

    /// Record a failure, if it is one worth recording.
    ///
    /// Best-effort by construction: a diagnostic that fails to be delivered must
    /// never change what the caller reports. Awaited rather than detached because
    /// these are rare — an unreachable coord classifies as local-only, so the one
    /// case that would block does not reach the network at all.
    pub async fn report(&self, coord: &CoordClient, principal: &Principal, e: &ChaprError) {
        let Some(report) = classify(e) else {
            return;
        };
        // An unreachable coordinator cannot be told that it is unreachable.
        let local_only = matches!(e, ChaprError::CoordUnreachable);
        self.deliver(coord, principal, report, local_only).await;
    }

    /// Record a condition that is not a [`ChaprError`] at all.
    ///
    /// [`classify`] is the right door for anything the error enum can express, and
    /// most callers want it. This one exists for findings the endpoint makes
    /// *while succeeding*: `chapr_read` refusing a text file for being in a legacy
    /// encoding is not an error — the read worked, the file is intact, and the
    /// caller is told so — but it is exactly the kind of environment fact this
    /// module was built to make discoverable, and it is invisible to `classify`
    /// because it never becomes a `ChaprError` (see `server::NotAnalysable`, which
    /// is deliberately outside coord's wire contract).
    pub async fn record(
        &self,
        coord: &CoordClient,
        principal: &Principal,
        report: DiagnosticReport,
    ) {
        self.deliver(coord, principal, report, false).await;
    }

    /// Stamp identity onto a report and put it in both sinks.
    async fn deliver(
        &self,
        coord: &CoordClient,
        principal: &Principal,
        mut report: DiagnosticReport,
        local_only: bool,
    ) {
        report.principal = principal.clone();
        report.host = self.host.clone();

        // Local first: it is the sink that still works when the other does not.
        self.append_local(&report);

        if local_only {
            return;
        }
        if let Err(err) = coord.report_diagnostic(&report).await {
            tracing::debug!(%err, "could not report diagnostic to coord (kept locally)");
        }
    }

    /// Append one JSON line. Line-delimited so it can be grepped, tailed and
    /// shipped without a parser, and so a truncated final line costs one record
    /// rather than the file.
    fn append_local(&self, report: &DiagnosticReport) {
        let Some(path) = &self.log_path else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) > MAX_LOG_BYTES {
            let _ = std::fs::rename(path, path.with_extension("jsonl.1"));
        }
        let line = serde_json::json!({
            "at": chrono::Utc::now().to_rfc3339(),
            "code": report.code,
            "severity": report.severity,
            "title": report.title,
            "path": report.path.as_ref().map(|p| p.as_str()),
            "principal": report.principal.as_str(),
            "host": report.host,
            "detail": report.detail,
            "remedy": report.remedy,
            "facts": report.facts,
        });
        // Nothing here may fail the caller's operation — a laptop with a full disk
        // still has to be able to refuse a write for the right reason.
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{line}");
        }
    }
}

/// Decide whether a failure is diagnostic-worthy, and describe it if so.
///
/// `None` means **this is a designed outcome**: it surfaces to the user through
/// the tool result and, where it is a coordination event, through conflict or
/// lease status. Recording those here would drown the ones that need action.
pub fn classify(e: &ChaprError) -> Option<DiagnosticReport> {
    use ChaprError as E;

    let mut facts: BTreeMap<String, String> = BTreeMap::new();
    let (code, title, severity, remedy) = match e {
        // ---- designed outcomes: not diagnostics ---------------------------
        // Contention, resolved by waiting or by a person. Belongs in lease status.
        E::LeaseHeld { .. } | E::RetryBudgetExhausted { .. } => return None,
        // A CAS loss. Both parties' bytes are kept; belongs in conflict status.
        E::Conflict { .. } => return None,
        // Humans always win, by design.
        E::OfficeLockPresent { .. } => return None,
        // Ordinary lookup answers.
        E::NotFound { .. }
        | E::AlreadyExists { .. }
        | E::VersionNotFound { .. }
        | E::ConflictNotFound { .. }
        // A missing parent and a near-duplicate directory name are both answers
        // to the caller, fully explained in the tool result. Neither indicates
        // anything wrong with the share or the deployment.
        | E::ParentMissing { .. }
        | E::NearDuplicateName { .. }
        // A file tool pointed at a folder. An answer to the caller, and nothing
        // wrong with the share - which is the whole reason it stopped being
        // reported as a permissions failure.
        | E::IsADirectory { .. } => return None,
        // Caller mistakes, already answered inline by the tool result.
        E::BaseVersionNotRecorded { .. }
        | E::BaseVersionRequired { .. }
        | E::ForceRequiresReason { .. } => return None,

        // ---- environment faults ------------------------------------------
        E::SharingViolation { path } => {
            facts.insert("path".into(), path.to_string());
            (
                "SHARING_VIOLATION",
                "A file could not be opened exclusively",
                Severity::Error,
                "Something else is holding the file open. On a fileserver this is most often \
                 antivirus or a backup agent scanning the share — exclude the share from \
                 real-time scanning. It can also be a document someone left open.",
            )
        }
        E::PermissionDenied { path } => {
            facts.insert("path".into(), path.to_string());
            (
                "PERMISSION_DENIED",
                "Access was denied on the share",
                Severity::Error,
                "Chaperone acts as the signed-in user and never impersonates, so this is that \
                 user's own permissions. Check their NTFS and share rights on this path — \
                 including create and delete on the containing folder, which conflict copies \
                 need.",
            )
        }
        E::Io { path, message } => {
            facts.insert("path".into(), path.to_string());
            facts.insert("os_message".into(), message.clone());
            (
                "IO",
                "A file operation on the share failed",
                Severity::Error,
                "The operating system message below names the cause. Common ones on a share: \
                 the path exceeds the length limit, a quota or file-screening rule rejected \
                 the name, or the connection dropped mid-operation.",
            )
        }
        E::InvalidPath { raw, reason } => {
            facts.insert("raw".into(), raw.clone());
            facts.insert("reason".into(), reason.clone());
            (
                "INVALID_PATH",
                "A path could not be used",
                Severity::Error,
                "Use a UNC path, or a full path on a mapped drive. If it names a drive letter, \
                 check that mapping exists for this user.",
            )
        }
        // Its own diagnostic, not folded into INVALID_PATH: a path outside the
        // root is almost always a *configuration* fact rather than a bad path,
        // and it is the one an administrator can act on. The facts carry what the
        // agent was told, so the diagnostic and the refusal agree.
        E::OutsideRoot {
            raw,
            resolved,
            roots,
            provenance,
        } => {
            facts.insert("raw".into(), raw.clone());
            facts.insert("resolved".into(), resolved.clone());
            facts.insert("roots".into(), roots.join(", "));
            facts.insert("root_provenance".into(), provenance.clone());
            (
                "OUTSIDE_ROOT",
                "A path resolved outside the coordinated root",
                Severity::Warning,
                "Either the caller asked for the wrong place, or CHAPR_ROOT is not what this \
                 deployment intended. Compare the resolved path against the roots in the facts \
                 — and check the recorded boot time: a root edited after the endpoint started \
                 has not been loaded, which looks identical to a wrong one.",
            )
        }

        // ---- recoverability faults: the serious ones ---------------------
        E::RecoveryFailed { path, missing_version } => {
            facts.insert("path".into(), path.to_string());
            facts.insert("missing_version".into(), missing_version.to_string());
            (
                "RECOVERY_FAILED",
                "A torn file could not be repaired from history",
                Severity::Error,
                "The pre-image is gone from the coordinator's history store, so this file \
                 cannot be repaired automatically — restore it from backup. Then check the \
                 history retention settings: blob GC may be evicting versions too early.",
            )
        }
        E::CommittedButUnrecorded { path, version, message } => {
            facts.insert("path".into(), path.to_string());
            facts.insert("version".into(), version.to_string());
            facts.insert("cause".into(), message.clone());
            (
                "COMMITTED_BUT_UNRECORDED",
                "A write landed on the share but was not recorded in history",
                Severity::Error,
                "The file itself is correct. What is missing is its history entry, so this \
                 version cannot be restored later and the audit trail is incomplete for it. \
                 Check the coordinator's database is writable and its volume has free space.",
            )
        }

        // ---- availability ------------------------------------------------
        E::CoordUnreachable => (
            "COORD_UNREACHABLE",
            "The coordination service could not be reached",
            Severity::Error,
            "Writes deliberately fail closed while the coordinator is unreachable; reading \
             keeps working. Check the coordinator service is running, that the URL configured \
             on this laptop is right, and that nothing between them blocks the port.",
        ),
        E::LeaseLost { lease_id } => {
            facts.insert("lease_id".into(), lease_id.to_string());
            (
                "LEASE_LOST",
                "A lease stopped being renewed",
                Severity::Warning,
                "Availability only — correctness never rests on a lease. Usually a brief \
                 network interruption to the coordinator. If it repeats, look at the \
                 connection between this laptop and coord.",
            )
        }
        E::LeaseExpired { lease_id } | E::LeaseNotFound { lease_id } => {
            facts.insert("lease_id".into(), lease_id.to_string());
            (
                "LEASE_GONE",
                "A lease was gone before the operation finished",
                Severity::Warning,
                "The operation took longer than the lease's lifetime, or the coordinator \
                 reaped it. Correctness is unaffected. If it recurs on the same files, they \
                 may be large enough that the write outlives the renewal interval.",
            )
        }
        E::MaxLeaseLifetimeExceeded { lease_id, hard_expiry } => {
            facts.insert("lease_id".into(), lease_id.to_string());
            facts.insert("hard_expiry".into(), hard_expiry.to_rfc3339());
            (
                "LEASE_LIFETIME_EXCEEDED",
                "A lease hit its hard lifetime ceiling",
                Severity::Warning,
                "An operation held one file far longer than expected. Look for an agent stuck \
                 mid-write, or a file large enough that the write cannot finish inside the \
                 ceiling.",
            )
        }

        // ---- defects ------------------------------------------------------
        E::Internal { message } => {
            facts.insert("message".into(), message.clone());
            (
                "INTERNAL",
                "Chaperone hit an internal error",
                Severity::Error,
                "This is a defect rather than a configuration problem. Pass the detail below \
                 to whoever maintains Chaperone.",
            )
        }
    };

    Some(DiagnosticReport {
        code: code.to_string(),
        title: title.to_string(),
        severity,
        path: diagnostic_path(e),
        // Filled in by `report`, which knows the acting identity and the host.
        principal: Principal::new_unchecked(""),
        host: None,
        detail: e.to_string(),
        remedy: remedy.to_string(),
        facts,
    })
}

/// The file a diagnostic is about, where the error names one. Grouping keys on
/// this, so it must be the canonical path rather than anything reconstructed.
fn diagnostic_path(e: &ChaprError) -> Option<chapr_proto::CanonicalPath> {
    use ChaprError as E;
    match e {
        E::SharingViolation { path }
        | E::PermissionDenied { path }
        | E::Io { path, .. }
        | E::RecoveryFailed { path, .. }
        | E::CommittedButUnrecorded { path, .. } => Some(path.clone()),
        _ => None,
    }
}

/// This machine's name, for telling one laptop's problem from everybody's.
fn hostname() -> Option<String> {
    let key = if cfg!(windows) { "COMPUTERNAME" } else { "HOSTNAME" };
    std::env::var(key).ok().filter(|h| !h.is_empty()).or_else(|| {
        std::fs::read_to_string("/etc/hostname")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|h| !h.is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chapr_proto::{CanonicalPath, LeaseId, VersionToken};

    fn p() -> CanonicalPath {
        CanonicalPath::new_unchecked("\\\\srv\\share\\a.md")
    }

    #[test]
    fn designed_outcomes_are_not_diagnostics() {
        // jok's cut: expected behaviour may be reported, but not as diagnostics —
        // it belongs in lease and conflict status. If these ever start producing
        // reports, the diagnostics list stops being usable.
        let expected = [
            ChaprError::LeaseHeld {
                holder: Principal::new_unchecked("CONTOSO\\other"),
                paths: vec![p()],
            },
            ChaprError::RetryBudgetExhausted { path: p(), attempts: 6 },
            ChaprError::OfficeLockPresent {
                path: p(),
                lock_file: CanonicalPath::new_unchecked("\\\\srv\\share\\~$a.md"),
            },
            ChaprError::NotFound { path: p() },
            ChaprError::AlreadyExists { path: p() },
            ChaprError::BaseVersionRequired { path: p() },
        ];
        for e in expected {
            assert!(
                classify(&e).is_none(),
                "{e:?} is a designed outcome and must not be a diagnostic"
            );
        }
    }

    #[test]
    fn environment_faults_carry_a_code_a_path_and_a_remedy() {
        let r = classify(&ChaprError::SharingViolation { path: p() }).unwrap();
        assert_eq!(r.code, "SHARING_VIOLATION");
        assert_eq!(r.path.as_ref(), Some(&p()), "grouping needs the path");
        assert!(r.remedy.contains("antivirus"), "the remedy must be actionable");
        assert_eq!(r.severity, Severity::Error);
    }

    #[test]
    fn an_io_failure_keeps_the_os_message_as_a_fact() {
        // The OS text is the only thing that distinguishes a path-length refusal
        // from a file screen from a dropped connection.
        let r = classify(&ChaprError::Io {
            path: p(),
            message: "The filename or extension is too long. (os error 206)".into(),
        })
        .unwrap();
        assert_eq!(
            r.facts.get("os_message").map(String::as_str),
            Some("The filename or extension is too long. (os error 206)")
        );
    }

    #[test]
    fn lease_trouble_is_a_warning_not_an_error() {
        // Availability, not correctness — invariant 3 does not rest on leases.
        let r = classify(&ChaprError::LeaseLost {
            lease_id: LeaseId::new_unchecked("lease-1"),
        })
        .unwrap();
        assert_eq!(r.severity, Severity::Warning);
    }

    #[test]
    fn a_committed_but_unrecorded_write_says_the_file_is_fine() {
        // The remedy must not send someone hunting for lost data that is not lost.
        let r = classify(&ChaprError::CommittedButUnrecorded {
            path: p(),
            version: VersionToken::hash(b"v"),
            message: "append failed".into(),
        })
        .unwrap();
        assert!(r.remedy.contains("file itself is correct"));
        assert!(r.facts.contains_key("version"));
    }

    #[test]
    fn every_classified_failure_has_a_nonempty_remedy() {
        // The field is required precisely so this cannot regress.
        let all = [
            ChaprError::SharingViolation { path: p() },
            ChaprError::PermissionDenied { path: p() },
            ChaprError::Io { path: p(), message: "x".into() },
            ChaprError::InvalidPath { raw: "z:x".into(), reason: "r".into() },
            ChaprError::CoordUnreachable,
            ChaprError::Internal { message: "boom".into() },
        ];
        for e in all {
            let r = classify(&e).expect("should be diagnostic-worthy");
            assert!(!r.remedy.trim().is_empty(), "{} has no remedy", r.code);
            assert!(!r.title.trim().is_empty(), "{} has no title", r.code);
            assert!(!r.detail.trim().is_empty(), "{} has no detail", r.code);
        }
    }

    #[tokio::test]
    async fn the_local_log_records_a_failure_as_one_json_line() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("nested").join("diagnostics.jsonl");
        let d = Diagnostics::new(Some(log.clone()));
        // A dead port: this is the case the local sink exists for.
        let coord = CoordClient::new("http://127.0.0.1:1");
        d.report(
            &coord,
            &Principal::new_unchecked("CONTOSO\\jsmith"),
            &ChaprError::SharingViolation { path: p() },
        )
        .await;

        let text = std::fs::read_to_string(&log).expect("the log must be created, dirs and all");
        assert_eq!(text.lines().count(), 1);
        let v: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(v["code"], "SHARING_VIOLATION");
        assert_eq!(v["principal"], "CONTOSO\\jsmith");
        assert!(v["remedy"].as_str().unwrap().contains("antivirus"));
        assert!(v["at"].as_str().is_some());
    }

    #[tokio::test]
    async fn a_designed_outcome_writes_nothing_locally_either() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("diagnostics.jsonl");
        let d = Diagnostics::new(Some(log.clone()));
        d.report(
            &CoordClient::new("http://127.0.0.1:1"),
            &Principal::new_unchecked("CONTOSO\\jsmith"),
            &ChaprError::Conflict {
                base_path: p(),
                sidecar_path: Some(p()),
                current_version: VersionToken::hash(b"v"),
                last_writer: Principal::new_unchecked("CONTOSO\\other"),
                when: chrono::Utc::now(),
            },
        )
        .await;
        assert!(!log.exists(), "a CAS conflict must not create a diagnostics log");
    }

    #[tokio::test]
    async fn an_unreachable_coord_is_recorded_locally_and_not_sent() {
        // It cannot be sent — that is the point. Recording it locally is the only
        // way anyone ever learns why writes were refused.
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("diagnostics.jsonl");
        let d = Diagnostics::new(Some(log.clone()));
        d.report(
            &CoordClient::new("http://127.0.0.1:1"),
            &Principal::new_unchecked("CONTOSO\\jsmith"),
            &ChaprError::CoordUnreachable,
        )
        .await;
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("COORD_UNREACHABLE"));
    }
}
