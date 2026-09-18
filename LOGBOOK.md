---
logbook:
  project: Chaperone
  type: engineering-logbook
  version: "1.0"
  created: "2026-07-21"
  last_updated: "2026-09-14"
  last_updated_by: "Claude (round-three findings answered; D-050, I-020/I-021 resolved, I-022 open; v0.1.5 set)"

state:
  phase: "implementation"
  status: idle          # idle | in-flight | blocked | complete
  sprint: null

sections:
  current_state:
    freshness_days: 7
    volatile: true
    child_doc_pattern: "logbook/state-history.md"
  decision_log:
    # RAISED 8000 -> 12000 by jok, 2026-09-07, rather than splitting the index.
    # The reasoning, because a raised limit looks like a moved goalpost and this
    # one is not: the index is ALREADY the compressed form. D-036's split put the
    # bodies in theme files precisely so this section would grow one line per
    # decision, and 8000 was chosen before anyone knew what the steady-state size
    # of that line-per-decision table would be. At 42 decisions it is ~8.2 KB of
    # scannable table of contents, which is not the wall the threshold guards
    # against. The alternative on the table -- moving the index into the theme
    # files -- was rejected because a cold `/logbook start` would then need four
    # file opens to answer "what has been decided", which is the one thing the
    # index exists to answer in one read. Buys ~23 more decisions.
    threshold_chars: 12000
    # Split by THEME, not by year (jok, 2026-08-21): the (then 34, now 42) entries
    # fall inside seven weeks, so a date axis discriminates nothing. `/logbook
    # decide` appends the body to the theme file below and adds one row here.
    child_doc_pattern: "logbook/decisions/{theme}.md"
    themes:
      architecture: "data model, wire protocol, read/write path, backends, invariants, coord internals"
      deployment: "installer, service, packaging, releases, auth, admin authority, hosting"
      process: "naming, licensing, repo + collaboration posture, agent/plugin behaviour"
      # Added 2026-08-21 with D-037 (jok's call): none of the three above covered a
      # decision about *what product this is and for whom*.
      product: "product scope, positioning, market boundaries, what this is and is not for"
    ask_before_split: true
  session_log:
    threshold_chars: 10000
    child_doc_pattern: "logbook/logs/YYYY-MM.md"
    # The NEWEST entry stays in this file; older ones live in the month file.
    keep_newest_in_root: 1
    ask_before_split: true
  known_issues:
    threshold_chars: 5000
    freshness_days: 30
    child_doc_pattern: "logbook/ISSUES.md"
    ask_before_split: true
  backlog:
    threshold_chars: 5000
    child_doc_pattern: "logbook/BACKLOG.md"
    ask_before_split: true

active_sessions: []
child_docs:
  - "logbook/decisions/architecture.md"
  - "logbook/decisions/deployment.md"
  - "logbook/decisions/process.md"
  - "logbook/decisions/product.md"
  - "logbook/logs/2026-09.md"
  - "logbook/logs/2026-08.md"
  - "logbook/logs/2026-07.md"
  - "logbook/ISSUES.md"
  - "logbook/BACKLOG.md"
  - "logbook/state-history.md"
---

## Purpose & Philosophy

A logbook is not a todo list. It is not a sprint board. It is a memory.

Its job:
- **For humans:** "Where were we? Why did we make that decision? What's blocked?"
- **For agents:** "What do I need to continue? What changed last session? What's dangerous to touch?"
- **For new team members:** "What is this project? How does it work? How do I start contributing?"

Three properties that make it useful vs. a file that gets abandoned:
1. **Updated at session end** — not retrospectively. If the agent always writes the handoff note, it is always current.
2. **Lives near the work** — not in Notion, not in Slack. In the repo, next to the files it describes.
3. **Asks before growing** — threshold mechanism prevents it becoming a wall nobody reads.

---

## Agent Protocol

**SESSION START:**
1. Read Current State — understand what's in flight
2. Check `active_sessions` in YAML — if non-empty, coordinate before working
3. Check Current State freshness (compare `last_updated` to today vs `freshness_days`) — flag if stale
4. Scan Known Issues for OPEN items with HIGH severity — surface to human
5. Scan Decision Log for CURRENT decisions older than 90 days — note for review
6. Read `Next session start from:` in most recent Session Log entry
7. Add yourself to `active_sessions` in YAML

**SESSION END:**
1. Write session entry (format below — `Next session start from:` is mandatory)
2. Update Current State if anything changed
3. Remove yourself from `active_sessions` in YAML
4. Update `last_updated` and `last_updated_by` in YAML
5. Before appending to any section: check section size vs `threshold_chars` — if exceeded, ask user before creating child doc

---

## Child Documents

> Split out 2026-08-21 with jok's approval, after every threshold-governed section
> had breached its limit (Decision Log by 12×, Session Log by 8×). This file keeps
> an **index** — one line per decision, issue and backlog item — while the bodies
> live under `logbook/`. Growth is now one line per item, so the thresholds hold.
>
> **This file and all of `logbook/` are TRACKED as of 2026-08-21 (D-038).** That
> reverses D-033's exclusion of them: an open project whose reasoning is invisible
> is only half open, and the memory was single-laptop and irreplaceable. Customer
> identifiers were scrubbed first — the deployable repo, the collaborator's plugin,
> its scripts and its folder names are described by role, not named. Contributors
> listed in `NOTICE` stay named. **Write every future entry for that audience:
> candid about engineering, never about a customer's internals.** `CLAUDE.md` stays
> untracked.

| Document | Holds |
|---|---|
| `logbook/decisions/architecture.md` | 24 decision bodies — data model, protocol, read/write path, backends, invariants, coord internals |
| `logbook/decisions/deployment.md` | 14 decision bodies — installer, service, packaging, releases, auth, admin authority, hosting |
| `logbook/decisions/process.md` | 6 decision bodies — naming, licensing, repo posture, publication, agent/plugin behaviour |
| `logbook/decisions/product.md` | 4 decision bodies — product scope, positioning, market boundaries (new 2026-08-21, D-037) |
| `logbook/logs/2026-09.md` | 7 session entries (09-11, 09-10, 09-09c, 09-09b, 09-09a, 09-08, 09-07) |
| `logbook/logs/2026-08.md` | 9 session entries (08-03 … 08-28) |
| `logbook/logs/2026-07.md` | 18 session entries (07-21 … 07-22) |
| `logbook/ISSUES.md` | all 19 issues in full, live and resolved |
| `logbook/BACKLOG.md` | live backlog + Delivered appendix (27 rows) + removed duplicates |
| `logbook/state-history.md` | narrative displaced from Current State, newest first |

---

## Current State

> **VOLATILE** — rewritten (not appended to) at every session end.
> Freshness: 7 days. If `last_updated` in YAML is older, flag as stale.
> Superseded narrative → `logbook/state-history.md`.

**Phase:** implementation — **v0.1.5 set by jok 2026-09-14, not yet tagged.** v1
complete, installed at a customer, pilot-tested on real hardware (2026-08-14),
hardened through phase 1 (2026-08-21), Phase A "Truth" (09-07), Phase B's
correctness core (09-08/09), the cowork slice (09-09), C0 migrations (09-10), the
packaging turn and **v0.1.4 released through CI (09-11)**, and now the
**round-three slice (09-14, D-050)**. Multi-backend (SMB + POSIX); Windows, Linux
and macOS run the suite in CI. **Test scope stays narrowed (D-049): Windows rig +
Azure VM, Windows and Linux endpoints, POSIX parked** until the SMB story is good.

**Status:** three crates build clean; **469** tests pass on Windows (**459** on
Linux — the difference is `cfg(windows)`), clippy `-D warnings` clean on both.
**12** tools against **32** coord service routes, plus the six-tab admin page;
coord has **6** subcommands. MSRV **1.88.0**. `cargo fmt` still drifts, jok's call.
*Counts re-measured 2026-09-14 by grepping `#[tool(` and `.route(`; the subcommand
count is carried from 09-11 and nothing this session touched coord's CLI.*

**What 0.1.5 is: the third happy-path round, answered.** The tester's report
(`specs/happypathfindings-round3.md`, local) was compared finding-by-finding with
the code (`…-vs-code.md`) and fixed against four decisions jok took (`…-fixplan.md`,
**D-050**). Two 0.1.4 claims were **true in the code and false at the tool
surface**: a forced write was stored `write_forced` and rendered `write` because
coord's parser had no arm for it (**I-020**), and `chapr_move` carried its version
to the handler and printed without it (**I-021**). Both fixed, both now covered by
a test that would have failed, and a correction note sits under the 0.1.4
changelog entry. **That is I-004's pattern in a new file:** a fix declared at the
documentation layer and never checked at the surface the claim is about.

**The rule D-050 makes true: any version a Chaperone tool returns is a usable
`base_version`.** Create and write always recorded a receipt; restore (both
modes) and move now do too, and `chapr_write`'s description says so. Proven
against a **real coordinator**: `smoke_parts` chains `restore-copy → write`,
`restore-in-place → write` and `move → write` with **no read in between**, 19/19.
Restore's own `base` stays CAS-only and is deliberately *not* asserted against the
read set — do not fix the inconsistency in that direction.

**I-022 is open and instrumented, not solved.** Once in four, a `base_version`
from `chapr_create` was refused as never read; the 2026-09-11 experiment ruled out
time, churn and restore. The code rules out hash-only keying and any TTL. Two
candidates survive — an endpoint **restart** (a session is `sess-{pid}`, D-044) or
a **silently failed receipt** (`let _ =` at five sites, now gone). A failed receipt
is logged at the endpoint; coord logs a refusal with what the session's read set
held; the refusal text says "this endpoint run" and names the restart case.
**The audit rows on the rig decide it:** compare `session_id` on the `create` row
for `uxt3-cas.txt` with the `refused[base_version_not_read]` row.

**A contradiction in the tester's record, unresolved.** The tester saw 0.1.4's
`needs base` refusal but a schema without `base` and the pre-09-09 descriptions.
No single build produces both. Likeliest is a host-cached tool list; N2 and 2.9
are therefore "fixed before the test ran, not observed". Ask the tester how the
schema was fetched and whether the host was restarted. The startup log line now
carries the version so a fourth "bundle NOT VERIFIED" has no excuse.

**Also this session:** the directory refusal lost its release-history sentence
(true for one of three tools that shared it); `chapr_list` names the entry type;
no model-visible config tool (jok, D-050 §4); no history schema change (§5) —
previous-name-on-move waits for B4/Q11 with I-016.

**Still true and not to relearn.** Read-path refusals come in two kinds (D-039,
I-015). SQLite runs in WAL (10 s busy timeout); `PUT /blobs` is bounded at
256 MiB; `put_blob` runs before `journal_open` — read `logbook/state-history.md`
(2026-08-05) before touching the write path. Three test environments, three jobs
(D-045): CI, the local rig `CHAPR-FS`, the Azure VM (auth path, still unused).
**jok pushes**; the SSH key is passphrase-protected, so an agent asks about the
remote rather than branching on it. WiX 6.0.2 is a local prerequisite for the MSI.
Linux clippy in WSL (`CARGO_TARGET_DIR=$HOME/chapr-target`) is not optional.

**What's next, in order.** (1) **The rig: I-022's audit rows** — five minutes on
the admin Audit tab, and the one fact this session could not get. (2) **The
endpoint over TLS on the rig** — still the one thing 0.1.4 ships proven only in a
test (I-018 hardware proof owed): `COORD_TLS=generate COORD_TLS_HOSTNAME=CHAPR-FS`,
copy `coord.crt`, set `CHAPR_COORD_CA_CERT`, self-test over
`https://CHAPR-FS:18899`. (3) **Tag 0.1.5** once (1) has been read — jok's call
and jok's push. (4) A fourth happy-path round with the version string handed to
the tester up front. **E-029's remainder** and the `traceparent` measurement stay
owed.

**Not blocking, and still jok's:** **Q11**/I-016 before B4, **Q4** before C3/C4,
**Q1**. **B7**'s three scenarios still need a fault-injecting proxy.

Deferred engineering (E-015, E-020, E-021, E-024b, E-028, E-030, V3-cloud) →
`logbook/BACKLOG.md`. **E-021 ↔ V3-cloud is still circular as written.**

---

## Active Sessions

> Populated by engineers/agents at session start. Cleared at session end.
> If this section is non-empty when you start: coordinate before working.

*(empty)*

---


## Decision Log

> **Index only** — one row per decision, most recent first. Bodies are in
> `logbook/decisions/<theme>.md`; click an ID to jump to its entry.
> **Threshold raised 8000 → 12000 by jok, 2026-09-07** (reasoning in the YAML).
> The section is at **10242 bytes across 48 rows** — re-measured 2026-09-11, not
> carried over — so there is room for roughly 15 more decisions before this needs
> another call. Re-measure rather than trusting that number; it has gone stale
> three times, which is I-013's whole point.
>
> The split this preamble used to promise — *"per-theme index tables"* — was
> **examined and dropped**: four tables of the same 42 rows inside this file is
> *larger*, and moving the index out to the theme files would make a cold
> `/logbook start` open four documents to learn what has been decided.
>
> `/logbook decide`: append the body to the theme file (see
> `sections.decision_log.themes` in the YAML), then add one row here.
> Never delete a row — mark `SUPERSEDED-BY-D-NNN` or `INVALIDATED`.
>
> **`D-009` was never issued.** D-001…D-008, D-010…D-048; nothing was deleted or
> retracted, so stop looking for it. (Recorded 2026-09-07 with the D-038 dedupe —
> see that session's entry.)

| ID | Decision | Date | Theme | Status |
|----|----------|------|-------|--------|
| [D-050](logbook/decisions/architecture.md#d-050) | **Any version a Chaperone tool returns is a usable `base_version`** — receipts on restore (both modes) and move; restore's `base` stays CAS-only; I-022 instrumented before theorised; directory refusal loses its release history; no 13th tool; no history schema change | 2026-09-14 | architecture | CURRENT (extends D-014) |
| [D-049](logbook/decisions/deployment.md#d-049) | The Windows coordinator is installed by an **MSI**; binary in Program Files, **data stays in ProgramData**; the wizard defers to the later package, the admin page **stays HTML** · **AMENDED 2026-09-11:** no custom action — the service provisions itself on first start, and the MSI **replaces** the bare Windows exe (8 release artifacts, no macOS coordinator) | 2026-09-10 | deployment | CURRENT (amends D-048) |
| [D-048](logbook/decisions/deployment.md#d-048) | The coordinator's install experience: the wizard is a **browser page** (no new dependency, not Windows-only) and a front end for `SetupArgs` only; `uninstall` keeps the data with no `--purge`; `handover` reprints; update backlogged behind C0 | 2026-09-09 | deployment | **AMENDED-BY-D-049** (wizard deferred out of the Windows package; the rest stands) |
| [D-047](logbook/decisions/architecture.md#d-047) | The cowork-findings slice: **Q13 closed as (a)** so restore cannot overwrite unseen content; `chapr_mkdir` with a deterministic near-name guard; every refusal audited; B6 pulled in whole | 2026-09-09 | architecture | CURRENT |
| [D-046](logbook/decisions/architecture.md#d-046) | The move journal is a separate table (a column cannot reach the live install until C0); interrupted moves are swept at start-up, not served from a read | 2026-09-09 | architecture | CURRENT |
| [D-045](logbook/decisions/process.md#d-045) | Three test environments with separate jobs; an UNVERIFIED self-test check exits non-zero | 2026-09-08 | process | CURRENT |
| [D-044](logbook/decisions/architecture.md#d-044) | Session identity is per endpoint *run*, not per conversation — MCP offers no alternative | 2026-09-08 | architecture | CURRENT |
| [D-043](logbook/decisions/product.md#d-043) | Track B and D-D′ close: mirror coordination re-deferred with a named trigger | 2026-09-07 | product | CURRENT |
| [D-042](logbook/decisions/deployment.md#d-042) | What the audit trail claims: an amendment scoping D-024, not a reversal | 2026-09-07 | deployment | CURRENT |
| [D-041](logbook/decisions/architecture.md#d-041) | Coord gets migration machinery: plain versioned SQL, none of D-003's rejected abstractions | 2026-09-07 | architecture | CURRENT |
| [D-040](logbook/decisions/product.md#d-040) | Chaperone is a coordination primitive: correctness → accountability → deployment | 2026-08-28 | product | CURRENT |
| [D-039](logbook/decisions/product.md#d-039) | Chaperone coordinates; it does not extract or transcode. That is an add-on (E-028), not a missing feature | 2026-08-25 | product | CURRENT |
| [D-038](logbook/decisions/process.md#d-038) | Project memory is published: `LOGBOOK.md` and `logbook/` become tracked, customer identifiers scrubbed | 2026-08-21 | process | CURRENT |
| [D-037](logbook/decisions/product.md#d-037) | On-prem is the product; cloud stays deferred on a market judgment, not an architectural exclusion · **AMENDED 2026-09-08:** the criterion is classical **fileserver semantics**, not geography | 2026-08-21 | product | CURRENT (review 2027-02-21) |
| [D-036](logbook/decisions/process.md#d-036) | Project memory splits by theme under `logbook/`; the root file becomes an index | 2026-08-21 | process | CURRENT |
| [D-035](logbook/decisions/deployment.md#d-035) | The endpoint is delivered as an MCP server, not as a Claude Desktop extension | 2026-08-19 | deployment | CURRENT |
| [D-034](logbook/decisions/deployment.md#d-034) | Releases are CI-built artifacts on a tag, not committed binaries | 2026-08-19 | deployment | CURRENT |
| [D-033](logbook/decisions/process.md#d-033) | Chaperone is Apache-2.0; attribution rides in NOTICE and in every file | 2026-08-19 | process | CURRENT |
| [D-032](logbook/decisions/deployment.md#d-032) | The executable is the installer; a bind address is not a URL; the CRT ships inside the binary | 2026-08-14 | deployment | CURRENT |
| [D-031](logbook/decisions/deployment.md#d-031) | Admin authority: a token enforces, a role follows; auth changes as a dual-mode cutover | 2026-08-12 | deployment | CURRENT |
| [D-030](logbook/decisions/architecture.md#d-030) | Subagent fan-out: serialize intra-session rather than merge sidecars; resolve drive letters rather than require them; diagnostics separate from audit | 2026-08-12 | architecture | CURRENT |
| [D-029](logbook/decisions/deployment.md#d-029) | Admin authority on coord: a role on the Authenticator seam, not OS elevation; data dir gated by installer ACL | 2026-08-12 | deployment | CURRENT |
| [D-028](logbook/decisions/process.md#d-028) | Chaperone stays plugin-neutral: the MCP announces the coordinated root, the agent reinterprets its own writes | 2026-08-12 | process | CURRENT |
| [D-027](logbook/decisions/architecture.md#d-027) | Audit remediation: baseline version-log entries, torn-file marker persistence, both Office lock conventions | 2026-08-06 | architecture | CURRENT |
| [D-026](logbook/decisions/architecture.md#d-026) | Invariant 6: coord DOES see bytes, for history only (resolution (a)) | 2026-08-05 | architecture | CURRENT |
| [D-025](logbook/decisions/deployment.md#d-025) | Deployment packaging + two-repo split (Chaperone + the customer deployable) | 2026-07-22 | deployment | CURRENT |
| [D-024](logbook/decisions/deployment.md#d-024) | Coord host = on-prem Windows (confirmed); MVP identity = zero-setup ambient OS identity, enforced auth deferred | 2026-07-22 | deployment | CURRENT |
| [D-023](logbook/decisions/deployment.md#d-023) | Control-plane auth: pluggable, generic OIDC (revises §13.1 "no OAuth") | 2026-07-22 | deployment | CURRENT |
| [D-022](logbook/decisions/architecture.md#d-022) | E-017 scope: push watch endpoint (direct-apply, coord-local DTO) | 2026-07-22 | architecture | CURRENT |
| [D-021](logbook/decisions/architecture.md#d-021) | E-019 scope: POSIX backend (shared §7 core, per-backend grammar) | 2026-07-22 | architecture | CURRENT |
| [D-020](logbook/decisions/architecture.md#d-020) | E-018 Backend trait + coord backend-discovery seam | 2026-07-21 | architecture | CURRENT |
| [D-019](logbook/decisions/deployment.md#d-019) | Deployment + backend-agnostic roadmap (brainstorm outcome) | 2026-07-21 | deployment | CURRENT |
| [D-018](logbook/decisions/deployment.md#d-018) | E-016: coord installer + service + TLS | 2026-07-21 | deployment | CURRENT |
| [D-017](logbook/decisions/architecture.md#d-017) | E-007: blob GC + retention | 2026-07-21 | architecture | CURRENT |
| [D-016](logbook/decisions/deployment.md#d-016) | I-001/I-002: pluggable auth boundary | 2026-07-21 | deployment | CURRENT |
| [D-015](logbook/decisions/architecture.md#d-015) | E-013: change-watcher (§14), trait-isolated on coord | 2026-07-21 | architecture | CURRENT |
| [D-014](logbook/decisions/architecture.md#d-014) | E-010: structural read-before-write | 2026-07-21 | architecture | CURRENT |
| [D-013](logbook/decisions/architecture.md#d-013) | E-012: chapr.move (atomic dual-lease rename) | 2026-07-21 | architecture | CURRENT |
| [D-012](logbook/decisions/architecture.md#d-012) | E-011: the remaining tool verbs (all but move) | 2026-07-21 | architecture | CURRENT |
| [D-011](logbook/decisions/architecture.md#d-011) | E-005 slice 4: lease-renewal manager | 2026-07-21 | architecture | CURRENT |
| [D-010](logbook/decisions/architecture.md#d-010) | E-005 slice 3: conflict registry + audit wiring + windows-rs write path | 2026-07-21 | architecture | CURRENT |
| [D-008](logbook/decisions/architecture.md#d-008) | E-005 slice 2: audit log + read path + rmcp server | 2026-07-21 | architecture | CURRENT |
| [D-007](logbook/decisions/architecture.md#d-007) | E-005 slice 1: proto DTO promotion + endpoint foundation | 2026-07-21 | architecture | CURRENT |
| [D-006](logbook/decisions/architecture.md#d-006) | History store: file-per-blob; GC/retention deferred | 2026-07-21 | architecture | CURRENT |
| [D-005](logbook/decisions/architecture.md#d-005) | E-004 scope: intent journal + detection only; blob store split out | 2026-07-21 | architecture | CURRENT |
| [D-004](logbook/decisions/architecture.md#d-004) | Version-index resolve contract: composite key; refresh stays coord-local | 2026-07-21 | architecture | CURRENT |
| [D-003](logbook/decisions/architecture.md#d-003) | Coord persistence = SQLite; leases persisted with lazy expiry | 2026-07-21 | architecture | CURRENT |
| [D-002](logbook/decisions/architecture.md#d-002) | chapr-proto concrete type choices (beyond the language-agnostic spec) | 2026-07-21 | architecture | CURRENT |
| [D-001](logbook/decisions/process.md#d-001) | Project name: Chaperone / chapr.<method> | 2026-07-21 | process | CURRENT |

ᵃ **Withdrawn 2026-09-07 — the claim was false.** This footnote said the marked
entries carried no `**Status:**` line. They all do, and did. Only **D-001** and
**D-032** were genuinely missing one; both were fixed today. The markers are gone.

---

## Session Log

> The **newest** entry lives here in full, so a cold `/logbook start` can read
> `Next session start from:` without opening a child doc. Older entries are in
> `logbook/logs/YYYY-MM.md`, most recent first.
>
> `/logbook end`: **move the entry below into its month file first**, then write
> the new one here. Threshold: 10000; the section sits just inside it as of
> 2026-09-14 at 4969 bytes, re-measured at session end.
> **Take the ~9 KB ceiling on a single entry literally**: it is the real limit,
> and prose that feels essential while writing is usually already in a decision
> body or a commit message. Headroom is thin by design — the entry here is
> replaced rather than appended to, so the section does not grow between sessions.

| Month | Entries |
|---|---|
| `logbook/logs/2026-09.md` | 7 — 2026-09-11, 09-10, 09-09c, 09-09b, 09-09a, 09-08, 09-07 |
| `logbook/logs/2026-08.md` | 9 — 2026-08-28, 08-25, 08-21b, 08-21, 08-19/20, 08-14, 08-06, 08-05, 08-03 |
| `logbook/logs/2026-07.md` | 18 — 2026-07-22 (a–c), 2026-07-21 (base, b–o) |

### Session 2026-09-14 — jok / Claude
**Type:** review → decisions → engineering (round-three findings) + **v0.1.5 set**
**Focus:** the third happy-path report, compared with the code finding by finding, four decisions taken, the fix built and proven against a real coordinator.

**Worked on:**
- [x] **Comparison first, as a file** (`specs/happypathfindings-round3-vs-code.md`, local). Of eleven findings: two fixed before the test ran (N2, 2.9), two real bugs the tester's observations pointed at but did not name (I-020, I-021), one mechanism the code cannot settle (N1 → I-022), one true-for-one-of-three message (N3), one stale description (list). **Two of the tester's observations contradict any single build** — the 0.1.4 refusal text alongside the pre-09-09 schema — and that stays unresolved; likeliest is a host-cached tool list.
- [x] **Four decisions, asked as questions and answered by jok** — recorded as **D-050**: receipts on restore and move (the rule "any returned version is a usable base_version"); the directory refusal's release-history sentence removed; version in the startup line, no 13th tool; no history schema change.
- [x] **I-020 — coord rendered `write_forced` as `write`.** `event_str` learned the string in `abf88c3`; `event_from_str` did not, and `_ => Write` swallowed it. One arm, plus a round-trip test that lists the enum exhaustively.
- [x] **I-021 — `chapr_move` printed no version.** `Ok(_)` at the handler. Now printed and recorded under the destination, or printing it would have set the same trap.
- [x] **Receipts everywhere a version is returned**, via one `record_read_best_effort` that logs a failed post; the `let _ =` is gone from five sites. Coord logs a refused `base_version` with what the set held for the path. The refusal text says "this endpoint run's read set" and names the restart case — the old wording asserted something the caller could see was false.
- [x] **Proven live:** `smoke_parts` against a real coordinator, 19/19, including three `→ write` chains with no read and a forced write reading back as `write_forced`. Windows 469 tests, Linux 459, clippy clean on both.
- [x] **CHANGELOG:** 0.1.5 entry; a **correction note under 0.1.4** for the two claims that were not true at the surface.

**The finding worth keeping.** Both bugs were fixes that reached the last function before the model and stopped, and both were written up as shipped. The tests that exist now are the ones that would have failed then. Same lesson as 09-11 — the thing never executed is the thing that breaks — one layer up: the thing never *observed at the surface* is the thing that is false there.

**State changes:** version **0.1.5** (jok, not tagged); **469/459** tests (was 466); tools 12, routes 32, re-measured; D-050, I-020 (resolved), I-021 (resolved), I-022 (open) filed; the 09-11 Current State displaced to `state-history.md`.

**Open questions:** **I-022's mechanism** — decided by the audit rows on the rig, not by more code. The tester's schema contradiction — a question for the tester. Q4, Q11, Q1 unchanged and jok's.

**Next session start from:** **the rig, for two things in one sitting.** First, the admin Audit tab: the `create` row for `uxt3-cas.txt` and the `refused[base_version_not_read]` row — same `session_id` means a lost receipt, different means an endpoint restart; that closes or redirects I-022. Second, the endpoint over TLS (`COORD_TLS=generate COORD_TLS_HOSTNAME=CHAPR-FS`, `CHAPR_COORD_CA_CERT`, self-test over `https://CHAPR-FS:18899`), still the one thing 0.1.4 ships proven only in a test. **Then tag 0.1.5** — jok's push. The next happy-path round gets the version string up front.

**Still owed and untouched:** E-029's remainder, the `traceparent` measurement, whether uninstalling an MCPB clears stored `user_config`, `specs/rig-verification-0910.md` stopped at Step 1.

## Known Issues

> **Live issues only** — full narrative for every issue, live and resolved, is in
> `logbook/ISSUES.md`. Staleness rule: open > 30 days is flagged STALE at session
> start. Threshold: 5000 chars; the section is at **4770** — measured 2026-09-14,
> not carried over.
>
> **The rule was applied for the first time on 2026-09-07** — it had been in the
> protocol since the split and never actually run, which is why six rows acquired
> a flag at once rather than one at a time. **The day counts are as of that date**;
> re-derive from `Since` rather than trusting them, since a hand-written age is
> exactly the kind of number I-013 exists to complain about. Being STALE says
> nothing about severity — it says nobody has looked, and for I-003 (upstream bug,
> nothing to do) and I-009 (agents do not generate aliased paths) that is arguably
> the correct outcome rather than neglect.

| ID | Description | Severity | Since | Status |
|----|-------------|----------|-------|--------|
| [I-022](logbook/ISSUES.md#i-022) | **A `base_version` from `chapr_create` was refused as never read, once in four; mechanism unknown** — two candidates (endpoint restart vs lost receipt), decided by the audit rows on the rig. | MED | 2026-09-10 | **OPEN** · instrumented 2026-09-14 |
| [I-021](logbook/ISSUES.md#i-021) | `chapr_move` returned no version — plumbed to the handler, dropped by `Ok(_)`; CHANGELOG 0.1.4 claimed it shipped. | LOW | 2026-09-09 | **RESOLVED** 2026-09-14 |
| [I-020](logbook/ISSUES.md#i-020) | **Coord rendered every `write_forced` history event as `write`** — `event_from_str` had no arm; CHANGELOG 0.1.4 claimed the opposite. | MED | 2026-09-09 | **RESOLVED** 2026-09-14 |
| [I-002](logbook/ISSUES.md#i-002) | endpoint↔coord channel unauthenticated. | ~~MED~~ LOW | 2026-07-21 | **MOSTLY RESOLVED** 2026-08-21 · **STALE 48d** |
| [I-003](logbook/ISSUES.md#i-003) | MCPB bundle signing non-functional in `@anthropic-ai/mcpb` 2.1.2. | LOW | 2026-07-22 | OPEN · **STALE 47d** |
| [I-004](logbook/ISSUES.md#i-004) | `CLAUDE.md` **Status** drifts from reality, and is auto-loaded before the logbook can correct it. | ~~LOW~~ MED | 2026-08-03 | OPEN (recurring) · **STALE 35d** |
| [I-005](logbook/ISSUES.md#i-005) | **Delivering a large PDF's content to a model is unsolved** — now *refused* rather than silently unanalysable (1.3). | ~~HIGH~~ MED | 2026-08-05 | OPEN · **STALE 33d** (failure mode fixed, capability not) |
| [I-018](logbook/ISSUES.md#i-018) | **The endpoint cannot use TLS at all** — reqwest links webpki-roots, so a private cert is refused and trusting it changes nothing. TLS-on-by-default is unreachable. | **HIGH** | 2026-09-10 | **RESOLVED** 2026-09-11 (native roots + CA option; hardware proof owed) |
| [I-017](logbook/ISSUES.md#i-017) | A bare-path `db_url` silently disables both tokens — `data_dir()` needs a `sqlite:` prefix, `db::connect` does not. | MED | 2026-09-10 | **RESOLVED** 2026-09-11 (explicit data_dir; validate refuses a bare path) |
| [I-019](logbook/ISSUES.md#i-019) | **The admin page could not sign in to any coordinator that authenticates** — one of its six routes wanted the endpoint token, and the refusal carried no body, so Continue cleared the box and said nothing. | **HIGH** | 2026-09-11 | **RESOLVED** 2026-09-11 (found on the rig; 206 tests had passed it) |
| [I-016](logbook/ISSUES.md#i-016) | An overwrite-move silently discards the source's open conflicts **and** its recoverable history. | MED | 2026-09-08 | OPEN (B4 / Q11) |
| [I-009](logbook/ISSUES.md#i-009) | Path aliasing: `normalize` resolves neither `.` nor `..`, breaking invariant 5. | LOW | 2026-08-06 | OPEN · **STALE 32d** |
| [I-011](logbook/ISSUES.md#i-011) | Release + CI workflows had never executed on GitHub. | LOW | 2026-08-19 | **PARTLY RESOLVED** 2026-08-20 |
| [I-013](logbook/ISSUES.md#i-013) | Nothing verifies the docs' numeric claims against the code, so they drift silently. | LOW | 2026-08-21 | OPEN |
| [I-015](logbook/ISSUES.md#i-015) | Binary guard refused non-UTF-8 *text* and told the agent to report it as a suspicious binary. | ~~MED~~ LOW | 2026-08-21 | **MOSTLY RESOLVED** 2026-08-25 (capability deferred to E-028, D-039) |

ᵇ **I-004 was audited 2026-08-21 and stays OPEN, scope widened.** Its original two
claims were genuinely fixed on 2026-08-14, but §Status went stale again within a week
(version, tool count, route count), so it is now the standing issue for the *pattern*
rather than for one paragraph — severity LOW → MED. The missing check is I-013.

Resolved and moved out: I-001, I-006, I-007, I-008, I-010, I-012, I-014 → `logbook/ISSUES.md` (I-007 and I-014 moved 2026-09-14 to keep the section under threshold — a row move, not a split).

---

## Backlog

> **Live items only** — notes, and the 26 delivered epics, are in
> `logbook/BACKLOG.md`. Threshold: 5000 chars.

### Gruntwork (< 2 hours each)

*(none yet)*

### Long-Term Engineering

| ID | Task | Priority | Est. Sessions | Status |
|----|------|----------|---------------|--------|
| E-029 | **Re-evaluate CI and release building** — nothing from 2026-09-10 has run in CI, and the MSI and the MCPB are both release artifacts built by hand | HIGH | 1 | TODO |
| E-030 | **Code signing** for the MSI and the release binaries — a purchase before it is engineering; `.mcpb` signing is separately broken upstream (I-003) | LOW | 1–2 | DEFERRED |
| E-015 | Enforced control-plane auth (pluggable): `negotiate` (on-prem) / **generic OIDC** validator + `TokenSource` (cloud/hybrid) | MED | 2–3 | DEFERRED |
| E-028 | **File-extraction MCP for Chaperone** — separate deployable: documents and legacy-encoded text → UTF-8 mirrors Chaperone can serve | MED | large | TODO |
| E-020 | SQLite backend-registry table + admin API (runtime-mutable routing) | LOW | 1–2 | TODO |
| E-021 | Relax `VersionToken` to backend-opaque (BLAKE3 for synth, ETag for cloud) | LOW | 1 | TODO |
| V3-cloud | Cloud backends S3 → Azure → Graph (conditional-PUT / lease / ETag adapters + cloud-event watchers) | LOW | large | TODO |
| E-024b | Coord dashboard / admin UI | LOW | large | DEFERRED |
| E-022 | Drive-letter → UNC canonicalisation (+ DFS, now droppable) | ~~HIGH~~ — | 1 | **DONE, fully verified 2026-09-08** |

---

*Logbook version: 1.0 | Created: 2026-07-21 | Split into child docs: 2026-08-21*
*To reuse: copy this file to a new project, clear live section content, adjust YAML frontmatter.*
