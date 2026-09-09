// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! Does the host tell us which conversation a tool call belongs to?
//!
//! **This module answers a question; it changes no behaviour.** Nothing here
//! affects a read, a write, an exit code or a session id. It observes what the
//! MCP host puts in a request's `_meta` and says so in the log, once per
//! distinct trace context.
//!
//! # Why it exists
//!
//! **D-044** accepted that a Chaperone session is per endpoint *run* —
//! `sess-{pid}` — because MCP hands a stdio server nothing that identifies a
//! conversation: `transport/io.rs` has no session concept, `rmcp::SessionId` is
//! a server-minted UUID for an HTTP header, and `Meta`'s reserved keys contain
//! no conversation id. Claude Desktop multiplexes every conversation over one
//! process, so `sess-{pid}` spans an application run, and the consequence is
//! stated plainly in the decision: **Phase C can deliver a verifiable record
//! that still cannot say which agent acted**, only which endpoint run did.
//!
//! One input to that was left unmeasured. **SEP-414** adds the W3C
//! `traceparent` to `_meta`, `rmcp` surfaces it ([`Meta::get_traceparent`]), and
//! a host is free to populate it per request. If a host sends a trace id that is
//! stable across the calls of one conversation and differs between
//! conversations, D-044's ceiling moves and Phase C's accountability claim gets
//! its missing half. If it sends nothing, the negative is what closes the
//! question — and a measured negative is worth more than the same conclusion
//! reached by reading the SDK, which is all D-044 had.
//!
//! # What the log will say, and how to read it
//!
//! - **`no traceparent`** (once per run, listing the keys that *were* sent) — a
//!   tool call arrived carrying no trace context. On its own, with no
//!   `new trace context` line anywhere in the run, D-044 stands as written.
//!   It does **not** mean every call lacked one: a host may populate `_meta`
//!   for some calls and not others, and both lines can appear in one run.
//! - **`new trace context`**, once per distinct **trace id**. The shape of the
//!   answer is in *how many* of these appear and *when*:
//!   - one line, then silence across many tool calls → a stable context. Compare
//!     a second conversation in the same Desktop run: a different trace id there
//!     is a **per-conversation** identifier, which is what would move D-044.
//!   - a new line on every tool call → the host mints a trace **per request**,
//!     which is correct tracing behaviour and useless as a conversation id.
//! - **`stopped reporting`** — [`MAX_REPORTED_CONTEXTS`] distinct contexts seen.
//!   That is itself the per-request answer, and the cap is what keeps a probe
//!   from filling an operator's log.
//!
//! **Line order is not call order.** `rmcp` spawns each request as its own
//! task, so concurrent tool calls report in whatever order they are served —
//! measured, not assumed: driving the binary with five calls put the line for
//! the call that carried no `_meta` *first*. Read the set of lines a run
//! produced, never the sequence.
//!
//! **No line at all is not an answer.** One of the three lines above is written
//! on the first tool call of every run, so their absence means the hook did not
//! execute — a build without it, or a host that never called a tool — and not
//! that the host sent nothing. Read silence as "not measured", the same
//! distinction B8 drew for the self-test's `UNVERIFIED`.
//!
//! Keyed on the **trace id**, not the whole `traceparent`: the span id changes
//! per operation by design, so keying on the full value would report every call
//! as new and measure nothing.
//!
//! What is *not* measured here is the delivery: `rmcp`'s service loop swaps a
//! request's `_meta` into `RequestContext` before the handler runs
//! (`service.rs`, "swap meta firstly, otherwise progress token will be lost"),
//! and `ToolCallContext::new` then drops the params' own copy — which is why
//! [`crate::server::ChaprServer::call_tool`] reads `context.meta` and not the
//! request. That plumbing is the SDK's; the tests below pin only this module's
//! side of it, from the JSON shape a host actually sends inward.
//!
//! Only key *names* are logged for the rest of `_meta`, never values — the
//! probe's question does not need them, and `_meta` is host-controlled content.

use std::collections::HashSet;
use std::sync::Mutex;

/// How many distinct trace contexts get a log line before the probe goes quiet.
///
/// A host minting a trace per request would otherwise write one line per tool
/// call for the life of the process. Thirty-two is far more than needed to tell
/// "stable" from "per-call" apart, and small enough to stay readable.
pub const MAX_REPORTED_CONTEXTS: usize = 32;

/// Observes `_meta` across the tool calls of one endpoint run.
///
/// Shared behind an `Arc` because [`crate::server::ChaprServer`] is `Clone` and
/// every clone must consult **one** set of already-seen contexts; a per-clone
/// probe would re-report the same trace id.
#[derive(Default)]
pub struct TraceProbe {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// Trace ids already reported this run.
    seen: HashSet<String>,
    /// Whether the "no traceparent" line has been written.
    reported_absent: bool,
    /// Whether the cap line has been written.
    reported_cap: bool,
}

/// What the probe has learned that it has not said yet.
///
/// Returned rather than logged directly so the decision — *is this new?* — is
/// testable without capturing a `tracing` subscriber.
#[derive(Debug, PartialEq, Eq)]
pub enum Observation {
    /// A tool call carried no `traceparent`. Reported once per run — not once
    /// per such call, and not a claim about the run's other calls.
    Absent {
        /// The `_meta` keys that *were* present, sorted. Empty means no `_meta`.
        keys: Vec<String>,
    },
    /// A trace context not seen before in this run.
    NewContext {
        /// The trace-id field: the part that is stable within one trace.
        trace_id: String,
        /// The full header value, so a reader can see version and flags.
        traceparent: String,
        /// The `_meta` keys present alongside it, sorted.
        keys: Vec<String>,
        /// How many distinct contexts have now been seen, this one included.
        distinct: usize,
    },
    /// [`MAX_REPORTED_CONTEXTS`] reached; the probe will say nothing further.
    Capped {
        /// Distinct contexts seen at the moment reporting stopped.
        distinct: usize,
    },
}

impl TraceProbe {
    /// Note one tool call's `_meta`, and return what is worth logging about it.
    ///
    /// `None` — the common case after the first few calls — means nothing new
    /// was learned. `keys` is only carried into the returned observation; it
    /// never affects whether one is produced.
    pub fn observe(&self, traceparent: Option<&str>, keys: &[String]) -> Option<Observation> {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match traceparent {
            None => {
                if st.reported_absent {
                    return None;
                }
                st.reported_absent = true;
                Some(Observation::Absent {
                    keys: keys.to_vec(),
                })
            }
            Some(tp) => {
                let trace_id = trace_id_of(tp)?;
                if st.seen.contains(&trace_id) {
                    return None;
                }
                if st.seen.len() >= MAX_REPORTED_CONTEXTS {
                    if st.reported_cap {
                        return None;
                    }
                    st.reported_cap = true;
                    return Some(Observation::Capped {
                        distinct: st.seen.len(),
                    });
                }
                st.seen.insert(trace_id.clone());
                Some(Observation::NewContext {
                    trace_id,
                    traceparent: tp.to_string(),
                    keys: keys.to_vec(),
                    distinct: st.seen.len(),
                })
            }
        }
    }
}

/// The trace-id field of a W3C `traceparent`, or `None` if it is not one.
///
/// Format is `version-traceid-spanid-flags`, the trace id being 32 lowercase hex
/// characters. A malformed value is *not* logged as a context: it would either
/// key the seen-set on garbage or, worse, invite reading a host's private string
/// as an identifier. Returning `None` means the probe stays silent, which is the
/// right failure direction for something that exists only to observe.
fn trace_id_of(traceparent: &str) -> Option<String> {
    let id = traceparent.split('-').nth(1)?;
    if id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(id.to_ascii_lowercase())
    } else {
        None
    }
}

impl Observation {
    /// Write this observation to the log, at `info` — an operator reading a
    /// session's stderr should not need `RUST_LOG` set to find the answer.
    pub fn log(&self) {
        match self {
            Observation::Absent { keys } => tracing::info!(
                meta_keys = ?keys,
                // Deliberately does not contain the phrase the other line is
                // grepped by: a message that matches the search for its own
                // opposite is a trap for whoever reads the log later.
                "no traceparent: a tool call arrived with no trace context. If this run reports \
                 none at all, a Chaperone session stays per endpoint run (D-044)"
            ),
            Observation::NewContext {
                trace_id,
                traceparent,
                keys,
                distinct,
            } => tracing::info!(
                %trace_id,
                %traceparent,
                meta_keys = ?keys,
                distinct,
                "new trace context (SEP-414). One line per conversation would make this a \
                 conversation id; one line per tool call makes it a request id (D-044)"
            ),
            Observation::Capped { distinct } => tracing::info!(
                distinct,
                cap = MAX_REPORTED_CONTEXTS,
                "stopped reporting trace contexts: this host mints one per request, which answers \
                 the question and is not a conversation id (D-044)"
            ),
        }
    }
}

/// The `_meta` key names on a request, sorted. Names only — see the module note.
pub fn meta_keys(meta: &rmcp::model::Meta) -> Vec<String> {
    let mut keys: Vec<String> = meta.0.keys().cloned().collect();
    keys.sort();
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A well-formed traceparent with the given trace id and an arbitrary span.
    fn tp(trace_id: &str, span: &str) -> String {
        format!("00-{trace_id}-{span}-01")
    }

    const T1: &str = "0af7651916cd43dd8448eb211c80319c";
    const T2: &str = "4bf92f3577b34da6a3ce929d0e0e4736";

    #[test]
    fn absence_is_reported_once_and_carries_the_other_keys() {
        let p = TraceProbe::default();
        let keys = vec!["progressToken".to_string()];
        assert_eq!(
            p.observe(None, &keys),
            Some(Observation::Absent { keys: keys.clone() })
        );
        assert_eq!(p.observe(None, &keys), None, "second call adds nothing");
    }

    #[test]
    fn a_stable_trace_id_is_reported_once_however_the_span_changes() {
        let p = TraceProbe::default();
        let first = p.observe(Some(&tp(T1, "00f067aa0ba902b7")), &[]);
        assert!(matches!(
            first,
            Some(Observation::NewContext { distinct: 1, .. })
        ));
        // Same trace, different span — one operation later in the same trace.
        assert_eq!(p.observe(Some(&tp(T1, "b7ad6b7169203331")), &[]), None);
    }

    #[test]
    fn a_second_conversation_shows_up_as_a_second_context() {
        let p = TraceProbe::default();
        p.observe(Some(&tp(T1, "00f067aa0ba902b7")), &[]);
        match p.observe(Some(&tp(T2, "00f067aa0ba902b7")), &[]) {
            Some(Observation::NewContext {
                trace_id,
                distinct: 2,
                ..
            }) => assert_eq!(trace_id, T2),
            other => panic!("expected a second context, got {other:?}"),
        }
    }

    /// The case that would otherwise fill a log: a trace minted per request.
    #[test]
    fn per_request_traces_stop_being_reported_at_the_cap() {
        let p = TraceProbe::default();
        for i in 0..MAX_REPORTED_CONTEXTS {
            let id = format!("{i:032x}");
            assert!(
                p.observe(Some(&tp(&id, "00f067aa0ba902b7")), &[]).is_some(),
                "context {i} should still be reported"
            );
        }
        assert_eq!(
            p.observe(Some(&tp(T1, "00f067aa0ba902b7")), &[]),
            Some(Observation::Capped {
                distinct: MAX_REPORTED_CONTEXTS
            })
        );
        assert_eq!(
            p.observe(Some(&tp(T2, "00f067aa0ba902b7")), &[]),
            None,
            "the cap line is written once, not per call"
        );
    }

    #[test]
    fn absence_then_presence_both_get_reported() {
        let p = TraceProbe::default();
        assert!(p.observe(None, &[]).is_some());
        assert!(
            p.observe(Some(&tp(T1, "00f067aa0ba902b7")), &[]).is_some(),
            "a host that starts sending one must still be noticed"
        );
    }

    #[test]
    fn a_malformed_traceparent_is_not_read_as_an_identifier() {
        let p = TraceProbe::default();
        for bad in [
            "",
            "00",
            "00--00f067aa0ba902b7-01",
            "00-not-hex-here-00f067aa0ba902b7-01",
            // Right shape, one character short: a truncated id is not an id.
            "00-0af7651916cd43dd8448eb211c80319-00f067aa0ba902b7-01",
        ] {
            assert_eq!(p.observe(Some(bad), &[]), None, "{bad:?} was accepted");
        }
        // And having seen only garbage, the probe has recorded nothing.
        assert!(p
            .observe(Some(&tp(T1, "00f067aa0ba902b7")), &[])
            .is_some_and(|o| matches!(o, Observation::NewContext { distinct: 1, .. })));
    }

    /// The shape a host actually sends, read the way the handler reads it.
    ///
    /// This is the half of the probe that could be wrong in a way the log could
    /// not show: if `_meta` were read from the wrong place, or the reserved key
    /// spelled differently, every run would print "no traceparent" and the
    /// negative would be mine rather than the host's.
    #[test]
    fn a_hosts_meta_object_is_read_as_a_trace_context() {
        let raw = serde_json::json!({
            "traceparent": "00-0af7651916cd43dd8448eb211c80319c-00f067aa0ba902b7-01",
            "progressToken": 7,
        });
        let meta = rmcp::model::Meta(raw.as_object().unwrap().clone());
        assert_eq!(
            meta_keys(&meta),
            vec!["progressToken".to_string(), "traceparent".to_string()],
            "keys are sorted, and values are not carried"
        );
        let p = TraceProbe::default();
        match p.observe(meta.get_traceparent(), &meta_keys(&meta)) {
            Some(Observation::NewContext { trace_id, keys, .. }) => {
                assert_eq!(trace_id, T1);
                assert!(keys.contains(&"progressToken".to_string()));
            }
            other => panic!("host meta was not read as a context: {other:?}"),
        }
    }

    /// The negative case, equally load-bearing: an `_meta` with no trace context
    /// must report *absence*, which is what tells a reader the hook ran at all.
    #[test]
    fn a_meta_without_a_trace_context_reports_absence_not_silence() {
        let raw = serde_json::json!({ "progressToken": 7 });
        let meta = rmcp::model::Meta(raw.as_object().unwrap().clone());
        assert_eq!(meta.get_traceparent(), None);
        assert_eq!(
            TraceProbe::default().observe(meta.get_traceparent(), &meta_keys(&meta)),
            Some(Observation::Absent {
                keys: vec!["progressToken".to_string()]
            })
        );
    }

    #[test]
    fn trace_id_parsing_is_case_insensitive_and_lowercased() {
        assert_eq!(
            trace_id_of("00-0AF7651916CD43DD8448EB211C80319C-00f067aa0ba902b7-01").as_deref(),
            Some(T1)
        );
    }
}
