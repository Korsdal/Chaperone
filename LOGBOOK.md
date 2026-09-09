---
logbook:
  project: Chaperone
  type: engineering-logbook
  version: "1.0"
  created: "2026-07-21"
  last_updated: "2026-09-09"
  last_updated_by: "Claude"

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
| `logbook/decisions/deployment.md` | 12 decision bodies — installer, service, packaging, releases, auth, admin authority, hosting |
| `logbook/decisions/process.md` | 6 decision bodies — naming, licensing, repo posture, publication, agent/plugin behaviour |
| `logbook/decisions/product.md` | 4 decision bodies — product scope, positioning, market boundaries (new 2026-08-21, D-037) |
| `logbook/logs/2026-09.md` | 3 session entries (09-09a, 09-08, 09-07) |
| `logbook/logs/2026-08.md` | 9 session entries (08-03 … 08-28) |
| `logbook/logs/2026-07.md` | 18 session entries (07-21 … 07-22) |
| `logbook/ISSUES.md` | all 16 issues in full, live and resolved |
| `logbook/BACKLOG.md` | live backlog + Delivered appendix (27 rows) + removed duplicates |
| `logbook/state-history.md` | narrative displaced from Current State, newest first |

---

## Current State

> **VOLATILE** — rewritten (not appended to) at every session end.
> Freshness: 7 days. If `last_updated` in YAML is older, flag as stale.
> Superseded narrative → `logbook/state-history.md`.

**Phase:** implementation — v1 complete, installed at a customer, pilot-tested on
real hardware (2026-08-14), phase 1 of the 0.2 plan delivered (2026-08-21) and
corrected (2026-08-25), **Phase A "Truth" (2026-09-07)**, **Phase B's correctness
core B0/B1/B2/B8 (2026-09-08) and B3 (2026-09-09)**, and **the cowork-findings
slice (2026-09-09, D-047)**. Multi-backend (SMB + POSIX); Windows, Linux and macOS
all run the suite in CI.

**Version `0.1.4`, set by jok 2026-09-09 — the tag is not pushed.** A patch bump
again, deliberately, even though `chapr_restore` gains a required argument: it
stays 0.1.x until it is tested and true, and the minor number is a claim about
proven-ness rather than a changelog of effort. Versioning is a human
responsibility; never fill in a bump.

**Status:** three crates build clean; **440** tests pass (was 420); clippy
`-D warnings` clean. **12** tools (was 11 — `chapr_mkdir`) against **32** coord
routes, plus the six-tab token-gated admin page whose Audit tab now searches by
path and detail. MSRV **1.88.0**. `cargo fmt` still drifts and remains jok's call.
*Counts re-measured 2026-09-09 by grepping `#[tool(` and `.route(`; both moved
this session, so do not carry them forward unread — that is what I-004 is for.*

**⚠ What has NOT been driven against a real share.** The whole cowork slice is
verified by the unit suite, wiremock, and a live *local* coordinator — **not** by
the rig. CI run **#16 on `fdbc2f8` was green across 7 jobs** and predates all of
it. The plan's verification walkthrough is owed: rebuild the rig bundle *without*
defaults, revert the `clean-share` checkpoint, and confirm the four
misconfiguration states are distinguishable from the tool surface alone.

**The cowork sessions' lesson, which is worth more than the fixes.** Driving all
the tools against the rig showed **the engine behaving and not explaining
itself**: every "works well" finding was boundary behaviour (fail-closed on a bad
root, no damage after five failed operations, CAS under a real stale write,
drive-letter resolution), and every failure was an explanation behaviour. jok's
severity principle governs the order — **how long a user believes they are the
problem**, not how broken a feature is. Configuration opacity consumed most of a
session; the missing `mkdir` cost two tool calls.

**Restore can no longer overwrite what nobody read (Q13 CLOSED as option (a)).**
It performed **no CAS at all**, by design — "a restore is a deliberate overwrite"
— so an agent could destroy current content it had never seen. It now requires
the caller to state what it observed (a version, or `absent`) and checks that
under the exclusive handle. **A soft-deleted target plus `absent` recreates the
file at its original name**, so `chapr_delete`'s promise of recoverability is kept
in practice and not only technically. Concept §6.5 always described the full write
path; spec and code now agree, in the spec's favour.

**`chapr_mkdir` exists, and its guard is deterministic on purpose.** The tool
compares, never the model: normalise (case, separators, punctuation), then
Damerau-Levenshtein within a length-scaled budget, refuse naming the candidates,
`confirm_new` overrides and is audited. **Numbered siblings are exempt** — `2026`
and `2027` are one edit apart and both deliberate, and a guard that refuses the
second year of a deployment gets switched off, taking the typo protection with it.
Parents are never created implicitly.

**Every refusal is now audited, reads included**, as `AuditKind::Refused` with the
cause as a greppable `refused[reason]` prefix in `detail`. Transport failures are
excluded deliberately: a coordinator outage would write one row per read in a
read-heavy workload, and it is not a decision anyone made. **Note for C4/Q4:
§12's retention was sized before refusals existed.**

**Two packaging failures were mine, found by handing jok a bundle.** "NOTE to
packager" text rendered verbatim in his install dialog, and pre-filled defaults
hid a stale stored value — a root typo'd as `charptest` survived several edits and
a new build, because stored `user_config` is keyed by extension rather than
version, so a *corrected* default never reaches an existing install. The template
now ships **no defaults** and requires the root. **The binary stays permissive**
and should: D-035(3) ships the bare executable for every OS, and one that refused
to start until configured would be unusable for a Claude Code user.

**B3 landed, so D-013's "stale-but-recoverable" is now true (D-046).** A
`move_journal` row records the rename's intent beforehand; **`move_paths` deletes
that row inside its own transaction**, which is the whole design — a surviving row
*proves* the migration did not commit, so completion is exactly-once with no
idempotency logic. `moverecover.rs` resolves them at endpoint start-up: complete,
abandon, or leave it and say why.

**A constraint that outlives B3: `db::migrate` is `CREATE TABLE IF NOT EXISTS` and
nothing else.** A new *table* reaches an existing database; a new *column* is
**silently skipped**, after which an endpoint queries a column the live
coordinator lacks. **C0 is therefore blocking two things**: move provenance in
history (2.4, where jok chose real columns over a side table) and the coord update
path in W2.

**Next up is the coord setup epic (W2), as one piece of work (jok).** GUI wizard,
`uninstall`, `status`, and the **handover output** — coord printing the endpoint
values IT must distribute, which the no-defaults rule makes a prerequisite rather
than a nicety. Update is backlogged behind C0. **Correction carried forward:**
`setup` already installs an auto-start Windows service (`setup.rs:833`), so the
fragility jok hit on the rig was my telling him to run `chapr-coord serve` in a
terminal.

**D-044's `traceparent` probe is in and the measurement is still owed.** `rmcp`
2.2 surfaces `_meta`; `traceprobe.rs` logs once per distinct trace id, covering
all tools via a hand-written `call_tool`. Proven over real stdio. **One line per
conversation would make it a conversation id; one per tool call makes it a request
id; no line at all means the hook did not run.** Procedure:
`docs/measuring-session-identity.md`.

**Read-path refusals still come in two kinds (D-039, I-015)** — a *container*
refused by magic bytes with advice naming what to read instead, and *non-UTF-8
text* refused as a different thing, naming the encoding and the human remedy.
Chaperone coordinates files; it does not convert encodings or extract text
(E-028). The classifier chooses **a message, never an outcome**.

**Before touching the write path,** read the behaviour notes in
`logbook/state-history.md` (2026-08-05): SQLite runs in **WAL** (10 s busy
timeout); coord `PUT /blobs` is bounded at **256 MiB**, which is also the largest
file Chaperone can write at all; `put_blob` runs **before** `journal_open`.

**Three test environments, three jobs, none replacing another (D-045).**
**CI** — every push, three OSes, regression; loopback share, no realm, no latency.
**Local rig** (`CHAPR-FS`, Hyper-V, Server 2022, Internal switch, share on `D:`
and coord state on `C:`) — rapid development, install process, file integrity, at
zero RTT in a workgroup with **no Kerberos realm**. **Azure VM fileserver** (demo
tenant, available, unused) — the **auth path** and **timing under real network
latency**, which nothing else covers. **E-015 is blocked only on priority.**

**Known and accepted (not a bug):** after setup hardens the data directory, an
**unelevated** `chapr-coord serve --config <that file>` cannot read its own
config. The real deployment runs as a service account with access.

**Environment:** a Linux toolchain exists in WSL, building with an isolated
`CARGO_TARGET_DIR=$HOME/chapr-target` so the Windows `target/` is never clobbered.
**jok pushes** — the SSH key is passphrase-protected *by design*. `api.github.com`
is readable unauthenticated for a public repo, so CI results are reachable from
here; **job logs are not** (403), so step conclusions are the available evidence.
A YAML parser (`js-yaml` under node) validates the workflows locally, which is how
run #15's invalid `ci.yml` should have been caught before the push.

**What's next:**
1. **W2, the coord setup epic** — wizard, uninstall, status, handover. The
   handover is the load-bearing part: no-defaults means an installer must be told
   the URL and the share path, and coord is the only thing holding all three
   values.
2. **The rig walkthrough for W1**, per the plan's verification section. Nothing in
   the cowork slice has met a real share.
3. **C0** — now blocking move provenance (2.4) and W2's update path, not only
   Phase C.
4. **I-016 needs jok's semantics call** before B4; **B7**'s three scenarios need a
   fault-injecting proxy that does not exist.
5. **Owed and cheap:** run the `traceparent` measurement; answer whether
   uninstalling an MCPB clears stored `user_config` — if it does not,
   "uninstall and reinstall" is not a recovery path we can offer.

Deferred engineering (E-015, E-020, E-021, E-024b, E-028, V3-cloud) →
`logbook/BACKLOG.md`. Note **E-021 ↔ V3-cloud is still circular as written** and
wants untangling.

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
> The section is at **9369 bytes across 46 rows** — re-measured 2026-09-09, not
> carried over — so there is room for roughly 19 more decisions before this needs
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
> **`D-009` was never issued.** D-001…D-008, D-010…D-047; nothing was deleted or
> retracted, so stop looking for it. (Recorded 2026-09-07 with the D-038 dedupe —
> see that session's entry.)

| ID | Decision | Date | Theme | Status |
|----|----------|------|-------|--------|
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
> 2026-09-09 at 8453 bytes, re-measured after the cowork-slice entry.
> **Take the ~9 KB ceiling on a single entry literally**: it is the real limit,
> and prose that feels essential while writing is usually already in a decision
> body or a commit message. Headroom is thin by design — the entry here is
> replaced rather than appended to, so the section does not grow between sessions.

| Month | Entries |
|---|---|
| `logbook/logs/2026-09.md` | 3 — 2026-09-09a, 2026-09-08, 2026-09-07 |
| `logbook/logs/2026-08.md` | 9 — 2026-08-28, 08-25, 08-21b, 08-21, 08-19/20, 08-14, 08-06, 08-05, 08-03 |
| `logbook/logs/2026-07.md` | 18 — 2026-07-22 (a–c), 2026-07-21 (base, b–o) |

### Session 2026-09-09b — jok / Claude
**Type:** engineering (the cowork-findings slice, W1) + product decisions
**Focus:** jok ran two cowork sessions against the rig — a setup session and a happy-path pass over all 11 tools plus 5 refusal paths — and wrote them up. Thirteen findings, every one reproduced in code, then decided interactively and built.

**Worked on:**
- [x] **The findings landed as a file first, and two of them needed correcting.** `specs/cowork-findings-0909.md`, merged from both reports under jok's numbering. **§4 was wrong**: leases *are* acquired by every mutating verb (five release sites) — they are invisible because their lifetime is one tool call, which is D-011's own note, so the fix is `chapr_stat`'s description not the field. **1.1's recommended fix already existed** and had not helped: the resolved root is logged at `info`, but stderr is not the tool surface, `instructions` reaches the model rather than the user, a fail-closed start announces nothing, and a *stale* config starts cleanly so there is nothing to log.
- [x] **Every decision surfaced as a question rather than assumed** (jok's ask). Fourteen forks across four rounds. His answers overrode the roadmap once (**Q13 → option (a)**, where it leant (b)/(c)) and corrected my framing once: I offered "make the binary require a root", and he pointed out that would *"make the ship-exe-and-mcpb solution worthless"* — D-035(3) ships the bare executable, and one that refuses to start until configured is unusable for a Claude Code user. Config belongs after install.
- [x] **Restore can no longer overwrite what nobody read.** jok's question — *does recreating a deleted file let an agent overwrite something?* — turned out to have a bigger answer than the case he asked about: **restore performed no CAS at all**, by design, so the bazooka already existed. It now requires the caller to state what it saw (a version, or `absent`) and checks it under the exclusive handle. Five outcomes, one test each. A soft-deleted target plus `absent` **recreates the file at its original name**, so `chapr_delete`'s promise of recoverability is kept in practice rather than technically.
- [x] **`chapr_mkdir`, guarded deterministically.** jok took the tool over the refusal, with the reasoning that manual folder creation is friction and friction pushes work outside Chaperone. His own objection was non-determinism, which the design answers by putting the comparison in the **tool**: normalise, then Damerau-Levenshtein within a length-scaled budget, refuse naming the candidates, `confirm_new` overrides and is audited. **A test caught the failure that would have got it switched off** — `2026` and `2027` are one edit apart and both deliberate, so numbered siblings are exempt.
- [x] **Every refusal is audited, reads included**, with the reason as a greppable prefix, plus path and detail search on `/admin`'s Audit tab. Transport failures are excluded deliberately: a coordinator outage would write one row per read in a read-heavy workload, and it is not a decision anyone made.
- [x] **B6 pulled in whole rather than patched**, which was the right call and not obvious. `Conflict` carried only `sidecar_path` — that is *why* `delete` and `move` set it to the live file, since they park nothing and the message had to read correctly. Rewording the string alone would have fixed `write` and broken the other two. Both paths are fields now, and `create` gained the `~$F` preflight (B6a, an omission not design: Word holds `~$F` for an unsaved document).
- [x] **Two failures were mine, from the bundle I handed jok yesterday.** "NOTE to packager" text rendered verbatim in his install dialog, and I pre-filled defaults so nothing had to be typed — which is exactly what hid a stale stored value. A root typo'd as `charptest` survived several edits and a new build, and that string appears nowhere in the source: stored `user_config` is keyed by extension, not version, so a *corrected* default never reaches anyone who already installed. Template now ships no defaults and requires the root.

**The withdrawn appendix finding was real, and the lesson generalises.** jok reported the boundary refusal escaping the input path with two backslashes while the root showed one, then withdrew it as unreproducible. It was deterministic and on **every** refusal: `{out:?}` and `{raw:?}` are Debug formatting, which doubles backslashes, while the roots printed through `as_str()`. **"Did not reproduce" on a formatting complaint deserves a look at the format string.** This is the third time this session that an assumption about what could not be checked was simply untested — after "only Actions can validate the workflow" (a YAML parser did it in a second) and yesterday's "CI results are unreachable from this laptop".

**Verified:** **440** tests (from 431 mid-session, 420 at its start), 0 failed; clippy `-D warnings` clean. **Four guards mutation-checked** — the restore CAS comparison, the near-name check, `create`'s preflight and the parent-missing branch. The `create` preflight is worth noting: removing it left all 216 endpoint tests green, so the test was written *because* the mutation check found nothing. `/admin`'s new filters were driven against a live coordinator, where a `detail_like` of `refused[outside_root]` discriminated between two refusal reasons.

**State changes:** **v0.1.4**, set by jok — tag not pushed. Tool surface **11 → 12** (`chapr_mkdir`); coord routes **32** (unchanged by this slice); tests 420 → 440. New: `chapr-endpoint/src/nearname.rs`, `ChaprError::{OutsideRoot, ParentMissing, NearDuplicateName}`, `VersionEvent::WriteForced`, `AuditKind::{Refused, DirCreate}`, `EntryType`, `RestoreBase`, `MoveResponse.version`. **Q13 CLOSED.** D-047 written. CHANGELOG restructured so the previously-unreleased Phase A and correctness-core work sits inside 0.1.4 rather than reading as newer than it.

**Open questions:** **13 now** — Q13 closed by jok's restore decision, Q11 still open but no longer blocking anything I built. Still jok's: **Q4** (chain versus erasure, gates C3/C4), **Q11** (I-016, gates B4), **Q1** (5.1 chunked reads). New and small: does uninstalling an MCPB clear stored `user_config`? If not, "uninstall and reinstall" is not a recovery path we can offer anyone.

**Next session start from:** **the coord setup epic (W2), which jok chose as one piece of work** — GUI wizard, uninstall, status, handover output, with update backlogged behind C0. The handover is the load-bearing part: the no-defaults rule means an installer must be told the coordinator URL and share path, and coord is the only thing that knows all three values. Note the correction from this session: **`setup` already installs an auto-start Windows service** (`setup.rs:833`), so the fragility jok hit was my telling him to run `chapr-coord serve` in a terminal. **Also owed:** the rig walkthrough in the plan's verification section — nothing in W1 has been driven end-to-end against a real share, only against wiremock, a live local coordinator and the unit suite. Rebuild the rig bundle *without* defaults, revert the `clean-share` checkpoint first, and check that the four misconfiguration states are now distinguishable from the tool surface alone. **C0 remains the constraint** on move provenance (2.4) and on W2's update path.



## Known Issues

> **Live issues only** — full narrative for every issue, live and resolved, is in
> `logbook/ISSUES.md`. Staleness rule: open > 30 days is flagged STALE at session
> start. Threshold: 5000 chars; the section is at **3441** — measured 2026-09-09,
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
| [I-002](logbook/ISSUES.md#i-002) | endpoint↔coord channel unauthenticated. | ~~MED~~ LOW | 2026-07-21 | **MOSTLY RESOLVED** 2026-08-21 · **STALE 48d** |
| [I-003](logbook/ISSUES.md#i-003) | MCPB bundle signing non-functional in `@anthropic-ai/mcpb` 2.1.2. | LOW | 2026-07-22 | OPEN · **STALE 47d** |
| [I-004](logbook/ISSUES.md#i-004) | `CLAUDE.md` **Status** drifts from reality, and is auto-loaded before the logbook can correct it. | ~~LOW~~ MED | 2026-08-03 | OPEN (recurring) · **STALE 35d** |
| [I-005](logbook/ISSUES.md#i-005) | **Delivering a large PDF's content to a model is unsolved** — now *refused* rather than silently unanalysable (1.3). | ~~HIGH~~ MED | 2026-08-05 | OPEN · **STALE 33d** (failure mode fixed, capability not) |
| [I-016](logbook/ISSUES.md#i-016) | An overwrite-move silently discards the source's open conflicts **and** its recoverable history. | MED | 2026-09-08 | OPEN (B4 / Q11) |
| [I-007](logbook/ISSUES.md#i-007) | **`move_cas_core` violates invariant 4** — version-check and mutation are not under one handle. | MED | 2026-08-06 | **RESOLVED** 2026-09-08 (B1+B2) |
| [I-009](logbook/ISSUES.md#i-009) | Path aliasing: `normalize` resolves neither `.` nor `..`, breaking invariant 5. | LOW | 2026-08-06 | OPEN · **STALE 32d** |
| [I-011](logbook/ISSUES.md#i-011) | Release + CI workflows had never executed on GitHub. | LOW | 2026-08-19 | **PARTLY RESOLVED** 2026-08-20 |
| [I-013](logbook/ISSUES.md#i-013) | Nothing verifies the docs' numeric claims against the code, so they drift silently. | LOW | 2026-08-21 | OPEN |
| [I-014](logbook/ISSUES.md#i-014) | Storage units mixed decimal and binary; three doc comments stated the wrong constant. | LOW | 2026-08-24 | **FIXED** 2026-08-24 (prevention open, see I-013) |
| [I-015](logbook/ISSUES.md#i-015) | Binary guard refused non-UTF-8 *text* and told the agent to report it as a suspicious binary. | ~~MED~~ LOW | 2026-08-21 | **MOSTLY RESOLVED** 2026-08-25 (capability deferred to E-028, D-039) |

ᵇ **I-004 was audited 2026-08-21 and stays OPEN, scope widened.** Its original two
claims were genuinely fixed on 2026-08-14, but §Status went stale again within a week
(version, tool count, route count), so it is now the standing issue for the *pattern*
rather than for one paragraph — severity LOW → MED. The missing check is I-013.

Resolved and moved out: I-001, I-006, I-008, I-010, **I-012** → `logbook/ISSUES.md`.

---

## Backlog

> **Live items only** — notes, and the 26 delivered epics, are in
> `logbook/BACKLOG.md`. Threshold: 5000 chars.

### Gruntwork (< 2 hours each)

*(none yet)*

### Long-Term Engineering

| ID | Task | Priority | Est. Sessions | Status |
|----|------|----------|---------------|--------|
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
