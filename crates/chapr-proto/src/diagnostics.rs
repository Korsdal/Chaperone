//! Operational diagnostics — the "why did it break" channel (E-026, D-030).
//!
//! ## Diagnostics are not the audit trail
//!
//! The audit trail records **what a principal did**: every lease, write, restore
//! and history entry, stamped with the AD principal that caused it. It is a
//! primary deliverable and it has to stay readable as an accountability record.
//!
//! This channel records **why an operation failed** — `ERROR_SHARING_VIOLATION`
//! on one user's laptop, a path that would not canonicalise, a lease whose
//! renewal stopped. Different question, different reader, different retention.
//! Merging them makes the audit trail unreadable, which D-027 established is a
//! primary-deliverable concern, so they are deliberately separate stores.
//!
//! ## Only the unexpected belongs here
//!
//! A CAS conflict and a document a human has open in Word are **designed
//! outcomes**, not faults. They surface as conflict and lease *status* (they have
//! their own place in the admin UI, with their own vocabulary — "both parties'
//! bytes are kept"). Recording them here would bury the one entry an
//! administrator actually needs to find.
//!
//! ## Why this exists at all
//!
//! Adopted principle (jok, 2026-08-12): *we cannot anticipate every environment
//! failure, but we can make failures discoverable.* That is a better strategy
//! than trying to enumerate a customer's environment up front — FSRM file
//! screens, path-length limits, NFD-normalising Mac clients, antivirus holding
//! handles open. Each of those becomes a diagnostic at runtime instead of a
//! question nobody thought to ask.
//!
//! Which is why [`DiagnosticReport::remedy`] is a required field rather than a
//! nicety. An administrator reading this needs to know what to *do*; a message
//! that only says what went wrong sends them back to the source.

use crate::{CanonicalPath, Principal};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How much attention one diagnostic deserves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Something did not work and a person has to act. Data is intact — a
    /// Chaperone operation that could have lost data fails closed instead.
    Error,
    /// Worth knowing, degrades something, does not block work. A lease that
    /// stopped renewing is the archetype: availability suffers, correctness does
    /// not (invariant 3).
    Warning,
}

/// Where a diagnostic group is in its lifecycle.
///
/// The transitions are an admin action and are **not** implemented yet (they need
/// the admin role, E-024a). The field exists from the start so the UI slice does
/// not need a migration to add it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticState {
    #[default]
    Open,
    Acknowledged,
    Resolved,
}

/// One occurrence of a failure, as the endpoint saw it.
///
/// Sent per event; coord folds it into a group (see [`DiagnosticGroup`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticReport {
    /// Stable machine code — the grouping key together with `path`. Derived from
    /// the [`crate::ChaprError`] variant, so it does not drift with wording:
    /// `SHARING_VIOLATION`, `IO`, `PERMISSION_DENIED`, `INVALID_PATH`, …
    pub code: String,
    /// One line, in an administrator's language rather than the code's.
    pub title: String,
    pub severity: Severity,
    /// The file involved, when there is one. `None` for endpoint-wide problems
    /// such as an unreachable coordinator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<CanonicalPath>,
    /// Who was acting. Not for blame — it is how an administrator tells "one
    /// laptop is misconfigured" from "this is happening to everybody".
    pub principal: Principal,
    /// Which machine reported it, for the same reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// The full underlying message, including any OS error text.
    pub detail: String,
    /// **What fixes it.** Required: a diagnostic that only states the problem
    /// sends the reader back to the source code.
    pub remedy: String,
    /// Free key/value context — an OS error number, a lock-file path, the drive
    /// letter that would not resolve. Deliberately open-ended: the whole point is
    /// to capture environment surprises nobody modelled in advance, and a new
    /// fact must not need a schema change. `BTreeMap` so the JSON is stable and
    /// two reports of the same problem compare equal.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub facts: BTreeMap<String, String>,
}

/// A group of identical failures, keyed by `(code, path)`.
///
/// Grouped rather than one row per event, because the failure that matters most
/// is usually the one repeating. A bounded retry against a busy path, or an agent
/// looping on a misconfigured drive letter, would otherwise fill the list with
/// hundreds of identical rows and hide everything else.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticGroup {
    pub id: String,
    pub code: String,
    pub title: String,
    pub severity: Severity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<CanonicalPath>,
    pub detail: String,
    pub remedy: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub facts: BTreeMap<String, String>,
    pub state: DiagnosticState,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    /// How many times this has happened, across every occurrence ever recorded —
    /// not the length of `occurrences`, which is trimmed.
    pub count: i64,
    /// The most recent occurrences, newest first. Bounded: the count is the
    /// history, this is the evidence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub occurrences: Vec<DiagnosticOccurrence>,
}

/// One recorded sighting within a group.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticOccurrence {
    pub at: DateTime<Utc>,
    pub principal: Principal,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

/// Body of `POST /diagnostics/query`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticsQuery {
    /// Only groups in these states. Empty means every state.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<DiagnosticState>,
    /// Only this code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Cap on groups returned, newest activity first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Response of `POST /diagnostics/query`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticsResponse {
    pub groups: Vec<DiagnosticGroup>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> DiagnosticReport {
        let mut facts = BTreeMap::new();
        facts.insert("os_error".into(), "32".into());
        DiagnosticReport {
            code: "SHARING_VIOLATION".into(),
            title: "Could not open a file exclusively".into(),
            severity: Severity::Error,
            path: Some(CanonicalPath::new_unchecked("\\\\srv\\share\\a.md")),
            principal: Principal::new_unchecked("CONTOSO\\jsmith"),
            host: Some("LAPTOP-04".into()),
            detail: "sharing violation opening \\\\srv\\share\\a.md exclusively".into(),
            remedy: "Something else holds the file open. Check for antivirus scanning \
                     the share, or a backup agent."
                .into(),
            facts,
        }
    }

    #[test]
    fn report_round_trips() {
        let r = report();
        let back: DiagnosticReport =
            serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn absent_optionals_are_omitted_from_the_wire() {
        let r = DiagnosticReport {
            path: None,
            host: None,
            facts: BTreeMap::new(),
            ..report()
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("path"), "{json}");
        assert!(!json.contains("host"), "{json}");
        assert!(!json.contains("facts"), "{json}");
    }

    #[test]
    fn a_report_with_only_required_fields_deserialises() {
        // Wire-compatibility: an older endpoint sends no path/host/facts.
        let json = r#"{
            "code": "IO", "title": "t", "severity": "error",
            "principal": "CONTOSO\\a", "detail": "d", "remedy": "r"
        }"#;
        let r: DiagnosticReport = serde_json::from_str(json).unwrap();
        assert_eq!(r.path, None);
        assert!(r.facts.is_empty());
    }

    #[test]
    fn state_defaults_to_open() {
        assert_eq!(DiagnosticState::default(), DiagnosticState::Open);
    }

    #[test]
    fn severity_orders_error_above_warning() {
        // The UI sorts "needs attention" first; make the ordering explicit rather
        // than incidental to declaration order.
        assert!(Severity::Error < Severity::Warning);
    }

    #[test]
    fn query_defaults_to_everything() {
        let q = DiagnosticsQuery::default();
        assert!(q.states.is_empty());
        assert_eq!(q.code, None);
        assert_eq!(q.limit, None);
        // An empty query must serialise to `{}` so a caller can send nothing.
        assert_eq!(serde_json::to_string(&q).unwrap(), "{}");
    }
}
