---
logbook:
  project: Chaperone
  type: engineering-logbook
  version: "1.0"
  created: "2026-07-21"
  last_updated: "2026-09-08"
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

active_sessions: ["Claude: 2026-09-09"]
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
| `logbook/decisions/architecture.md` | 22 decision bodies — data model, protocol, read/write path, backends, invariants, coord internals |
| `logbook/decisions/deployment.md` | 12 decision bodies — installer, service, packaging, releases, auth, admin authority, hosting |
| `logbook/decisions/process.md` | 6 decision bodies — naming, licensing, repo posture, publication, agent/plugin behaviour |
| `logbook/decisions/product.md` | 4 decision bodies — product scope, positioning, market boundaries (new 2026-08-21, D-037) |
| `logbook/logs/2026-09.md` | 1 session entry (09-07) |
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
corrected (2026-08-25), **Phase A "Truth" delivered (2026-09-07)**, and
**Phase B's correctness core opened and its first three items landed
(2026-09-08)**. Multi-backend (SMB + POSIX); Windows, Linux and macOS all run the
suite in CI.

**Version `0.1.3`** (set by jok 2026-08-25), tagged `v0.1.3`. **No bump this
session** — it stays 0.1.x until it is tested and true; the minor number is a claim
about proven-ness, not a changelog of effort. Versioning is a human
responsibility; never fill in a bump.

**Status:** three crates build clean; **396** tests pass (was 384); clippy
`-D warnings` clean on Windows **and** in WSL. **29** coord routes + the **11-tool**
MCP surface (unchanged), plus the six-tab token-gated admin page. MSRV **1.88.0**.
`cargo fmt` still drifts (**214** files in `chapr-endpoint` alone) and remains
jok's call — see the incident note in the session entry.

**There is now a local test rig, and it is the session's most reusable output.**
A Hyper-V VM **`CHAPR-FS`** (Windows Server 2022) on an **Internal** switch:
no physical NIC bound, **no default gateway on either side**, `192.168.221.0/24`
(chosen not to collide with corp `172.16.43.0/24` or Hyper-V's NAT
`172.19.176.0/20`). Reachable only from this workstation. **Share on `D:`,
coordinator's db/blobs/audit on `C:`** — jok's correction, mirroring the pilot, and
it converts **I-009**'s mitigation from behavioural to structural because there is
no `..\..` path from `D:\` to `C:\`. Auth is a stored credential
(`cmdkey /add:CHAPR-FS`), because the VM is a workgroup member with **no Kerberos
realm** — which is itself one of the postures the product must handle.

**Three test environments, three jobs, none replacing another (D-045).**
**CI** — every push, three OSes, regression; loopback share, no realm, no latency.
**Local rig** — rapid development, install process, file integrity, kill-mid-write,
mapped drives; zero RTT, no realm. **Azure VM fileserver** (demo tenant, available,
not yet used) — the **auth path** (Kerberos/Negotiate/OIDC) and **timing under real
network latency**, which nothing else covers. **E-015 is therefore no longer blocked
on environment, only on priority.**

**What Phase B closed, and what it cost.** **I-007 is RESOLVED** — the last
invariant-4 violation. The rename runs through the handle held since the source's
CAS (`SetFileInformationByHandle(FileRenameInfo)` + `DELETE` in the access mask, the
method D-027 settled empirically); the bug was a single `drop`. **The two-path
`FsPrimitives::rename` was deleted**, with `winfs::move_file` and `posixfs::rename`,
because it can only be reached by closing the handle first — renaming is now a
method on the *held file*, so the safe order is the only expressible one.
**`move_cas_core` went from zero tests to six**, plus five Win32 tests, and the
`DELETE` addition is **mutation-checked**.

**B8: the self-test's exit code no longer contradicts its own output.**
`Report::finish()` returned `failed` only, so a run that verified almost nothing
exited 0 while *printing* D-032's SKIP-is-not-PASS rule. `Skip` split into
**`Unverified`** (applies here, did not run → **counts**) and **`NotApplicable`**
(cannot apply to this backend → does not). Demonstrated on identical
infrastructure: `CHAPR_SELFTEST_DIR` as UNC → `8 passed, 1 unverified`, **exit 1**;
as `Z:\` → `9 passed`, **exit 0**.

**E-022 is fully verified and its live `HIGH` row is retired.** `Z:\` resolved to
`\\CHAPR-FS\chaprtest\` through the real `WNetGetUniversalNameW` against a real
server — the first execution of that branch outside a fake table, and the whole
write path was then exercised through the letter. Covered permanently by an opt-in
`#[ignore]`d test (`CHAPR_TEST_MAPPED_DRIVE`) and by CI, which now maps a drive.

**Invariant 3 is now measured off-box.** `exclusive open is honoured by the server`
passed against a **remote** Windows Server 2022, not only the 2026-08-14 pilot and
CI's loopback share. B0's Windows e2e leg is green (run #14, `cf25082`) — all five
scenarios, and its lock check is `#[cfg(windows)]` so it cannot have skipped.

**⚠ Three changes are committed as of 2026-09-09 and still unproven; they land
on the next push.** B8's new exit-code semantics across all five CI legs; the
`net use Z:` step; and whether the POSIX legs correctly report **`N/A`** rather
than going red. WSL says they compile and pass — but only Actions can say the
workflow itself is right. **Read that run before anything else.**

*This paragraph said "committed" on 2026-09-08 and it was false.* The whole of
that session — B1, B2, B8, E-022's verification, every logbook and decision
entry — sat **uncommitted in the working tree** until 2026-09-09, and CI's
newest run was still **#14 on `cf25082`**. Nothing was lost, but for a day the
project's newest correctness work existed in exactly one place with no copy
anywhere. It is now three commits, verified at **396 tests, 0 failed, clippy
`-D warnings` clean** before committing — re-measured, not carried over.
**The habit this costs: `git status` belongs in the session-end protocol, beside
the logbook write.**

**I-016 is the open finding, and it has teeth (B4 / Q11).** An **overwrite-move**
runs `DELETE FROM version_log / journal / conflicts WHERE path = src`. Open
conflicts become **orphaned** — every reader needs the row, nothing scans for stray
sidecars, so the loser's bytes survive as a file no query can reach and no
`resolve_conflict` can name. And because `gc.rs` derives retention **solely** from
`version_log`, the source's history blobs fall out of every retention set and are
**permanently reclaimed**. *"src is consumed"* is true of the name, not of the
content that just moved to `dst`. **Not fixed — it needs jok's semantics call.**

**Q6 is settled and the answer is a limit, not a feature (D-044).** MCP gives a
stdio server **nothing** that identifies a conversation — `transport/io.rs` has zero
session references, `rmcp::SessionId` is a server-minted UUID for an HTTP header,
and `Meta`'s eight reserved keys contain no conversation id. Desktop multiplexes
every conversation over one process, so `sess-{pid}` spans an application run.
Accepted as the ceiling; the concept gets **renamed** to stop overclaiming, and
`traceparent` (SEP-414) gets an empirical probe before this is called final.
**Consequence: Phase C can deliver a verifiable record that still cannot say which
agent acted** — only which endpoint run did.

**D-037 amended 2026-09-08 (jok): the criterion is classical fileserver
semantics, not geography.** What Chaperone depends on is a mandatory lock, ACLs
that survive, and an identity-preserving rename — properties of the *server*, not
its location. A cloud-**hosted** Windows fileserver is inside the line; a
cloud-**native** object store is outside it. **V3-cloud stays OUT**, reaffirmed.
The line does not move; the 2027-02-21 review question gets better: *"has anyone
built agent coordination for classical fileserver semantics?"*

**Read-path refusals still come in two kinds (D-039, I-015)** — a *container* (PDF,
Office, image, archive) refused by magic bytes with advice naming what to read
instead, and *non-UTF-8 text* refused as a different thing, naming the encoding and
the human remedy while a `NON_UTF8_TEXT` warning carries evidence to diagnostics.
Chaperone coordinates files; it does not convert encodings or extract text (E-028).
The classifier chooses **a message, never an outcome**.

**Before touching the write path,** read the behaviour notes in
`logbook/state-history.md` (2026-08-05): SQLite runs in **WAL** (10 s busy timeout);
coord `PUT /blobs` is bounded at **256 MiB**, which is also the largest file
Chaperone can write at all; `put_blob` runs **before** `journal_open`.

**Known and accepted (not a bug):** after setup hardens the data directory, an
**unelevated** `chapr-coord serve --config <that file>` cannot read its own config.
The real deployment runs as a service account with access.

**Environment:** a Linux toolchain exists in WSL, building with an isolated
`CARGO_TARGET_DIR=$HOME/chapr-target` so the Windows `target/` is never clobbered.
**jok pushes** — the SSH key is passphrase-protected *by design*, not broken. But
**`api.github.com` is readable unauthenticated for a public repo**, so CI results
*are* reachable from here; last session recorded them as unknowable and that was an
untested assumption.

**What's next:**
1. **Push, then read the CI run** — the three unproven changes above. Red on the
   POSIX legs means the `N/A` classification is wrong; red on Windows means the
   `net use` step is.
2. **Phase B continues** — **B3** (journal the move window, so D-013's
   "stale-but-recoverable" becomes true; Q10) or **B6** (the verb-uniformity sweep
   the previous roadmap never asked for: `create` alone skips the `~$F` preflight,
   `restore(copy)` takes no lease or path lock, `delete`/`move` return `Conflict`
   with `sidecar_path` pointing at the file itself and create **no** sidecar).
3. **I-016 needs jok's semantics call** before B4 can be built.
4. **Cheap and unanswered:** does Claude Desktop populate `traceparent`? One
   `RequestContext` parameter and a log line, and the rig can answer it.
5. **Phase C** stays gated on Q4; **Phase D** is the cheapest phase and partly
   delivered.

Deferred engineering (E-015, E-020, E-021, E-024b, E-028, V3-cloud) →
`logbook/BACKLOG.md`. **E-022 is no longer on that list.** Note **E-021 ↔ V3-cloud
is still circular as written** and wants untangling.

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
> The section is at **8791 bytes across 44 rows** — re-measured 2026-09-08, not
> carried over — so there is room for roughly 22 more decisions before this needs
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
> **`D-009` was never issued.** D-001…D-008, D-010…D-045; nothing was deleted or
> retracted, so stop looking for it. (Recorded 2026-09-07 with the D-038 dedupe —
> see that session's entry.)

| ID | Decision | Date | Theme | Status |
|----|----------|------|-------|--------|
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
> 2026-09-07, after a two-part session and several rounds of cutting.
> **Take the ~9 KB ceiling on a single entry literally**: it is the real limit,
> and prose that feels essential while writing is usually already in a decision
> body or a commit message. Headroom is thin by design — the entry here is
> replaced rather than appended to, so the section does not grow between sessions.

| Month | Entries |
|---|---|
| `logbook/logs/2026-09.md` | 1 — 2026-09-07 |
| `logbook/logs/2026-08.md` | 9 — 2026-08-28, 08-25, 08-21b, 08-21, 08-19/20, 08-14, 08-06, 08-05, 08-03 |
| `logbook/logs/2026-07.md` | 18 — 2026-07-22 (a–c), 2026-07-21 (base, b–o) |

### Session 2026-09-08 — jok / Claude
**Type:** engineering (Phase B: B0 verified, B1, B2, B8) + infrastructure (first local test rig)
**Focus:** the four gating questions, then the correctness core, then the thing that had been blocking every "is it really true?" question for two months — a real fileserver we control.

**Worked on:**
- [x] **B0 answered in ten minutes, after a session recorded it as unanswerable from this laptop.** `gh` is absent and the SSH key is passphrase-protected, but **`api.github.com` is readable unauthenticated for a public repo** — so run #14 (`cf25082`) was there all along. `e2e (windows-latest, smb)`: every step success, including the three that had never executed. The mandatory-lock check cannot have passed vacuously — `mandatory_lock_check` is `#[cfg(windows)]`, so on that leg it compiles in and can only PASS or FAIL. **The lesson generalises: "unreachable from here" was an assumption nobody had tested.**
- [x] **Q6 settled by reading the SDK, which nobody had done.** MCP gives a stdio server **nothing** that identifies a conversation: `transport/io.rs` has zero session references; `rmcp::SessionId` is a server-minted UUID for the `Mcp-Session-Id` header on HTTP transports only; `Meta`'s eight reserved keys contain no conversation id. Worse than "per-process" implies — Desktop multiplexes *every* conversation over one process. **D-044:** accept per-**run**, rename so it stops overclaiming, and probe `traceparent` (SEP-414) empirically before calling it final.
- [x] **Q12 settled → B1, I-007 closed.** The rename now runs **through the handle held since the source's CAS** — `SetFileInformationByHandle(FileRenameInfo)` plus `DELETE` in the access mask, exactly as D-027 predicted. The bug was one `drop`. **`FsPrimitives::rename(src,dst)` was deleted entirely** along with `winfs::move_file` and `posixfs::rename`: a two-path rename is only reachable by closing the handle first, so leaving it available invites reintroducing this. **Mutation-checked** — removing `DELETE` fails four of five new `winfs` tests with `ERROR_ACCESS_DENIED`.
- [x] **B2 — `move_cas_core` had zero tests, now six**, driving the real core against the real POSIX backend and real files with only coord mocked. Plus five Win32 tests covering the `FILE_RENAME_INFO` buffer, whose alignment I initially hand-waved (`Vec<u8>` is 1-aligned; the struct holds a `HANDLE`) and then made provable with `Vec<u64>`.
- [x] **B8 — the self-test's exit code stops contradicting its own output.** `Report::finish()` returned `failed` only, so a run that verified almost nothing exited 0 while *printing* D-032's SKIP-is-not-PASS rule. `Skip` splits into **`Unverified`** (applies, did not run → counts) and **`NotApplicable`** (cannot apply here → does not). **Demonstrated on identical infrastructure:** one env var different gave exit 1 versus exit 0.
- [x] **The rig exists.** Hyper-V VM `CHAPR-FS` (Server 2022) on an **Internal** switch — no physical NIC, no default gateway either side, `192.168.221.0/24` chosen not to collide with corp or Hyper-V's NAT. **jok's correction mid-build, and it was right:** share on **`D:`**, coordinator's db/blobs/audit on **`C:`**, mirroring the pilot — which also converts **I-009**'s mitigation from behavioural to structural, since there is no `..\..` path from `D:\` to `C:\`.
- [x] **E-022 closed for real.** `Z:\` → `\\CHAPR-FS\chaprtest\` through the real `WNetGetUniversalNameW` against a real server — the first execution of that branch outside a fake table, and the thing its live `HIGH` row had waited for since August. Covered permanently by an opt-in `#[ignore]`d test and by CI, which now maps a drive.

**Verified against the rig, not asserted:** `smoke_parts` 14/14 — including *move: source gone* / *move: dest has v2*, i.e. **B1's rename executing against real SMB**. `smoke_pilot` 11/11 after a first-run blip (below). Self-test **9 passed / exit 0** via `Z:`, **8 passed / 1 unverified / exit 1** via UNC. `exclusive open is honoured by the server` **passed against a remote Server 2022** — invariant 3 measured off-box for the first time.

**Two findings only a real server produced.** A **transient sharing violation** on the first 512 KiB write to a fresh share, then 15 consecutive passes — almost certainly Defender scanning new files, a cause `mandatory_lock_check`'s own error text already names. **Chaperone failed closed with the pre-image intact**, which is the correct direction now *observed*. And name resolution silently fell through to **mDNS/IPv6 link-local** (`CHAPR-FS.local`) because a `hosts` entry I gave jok had backtick escapes that did not survive copy-paste — my error, and a reminder that resolution order matters where the customer has no DNS for the fileserver.

**I-016, found by reviewing jok's admin-panel screenshot rather than by reading code with a hypothesis.** An **overwrite-move** runs `DELETE FROM version_log / journal / conflicts WHERE path = src`. Two consequences, both traced: open conflicts become **orphaned** — every reader needs the row, nothing scans for stray sidecars, so the loser's bytes survive as a file no query can reach and no `resolve_conflict` can name; and because `gc.rs` derives retention **solely** from `version_log`, the source's history blobs fall out of every retention set and are **permanently reclaimed**. *"src is consumed"* is true of the name, not of the content that just moved to `dst`. This is **B4 / Q11** with a mechanism attached — filed, deliberately not fixed, because it needs a semantics call.

**D-037 amended (jok), line unmoved.** A demo tenant appeared carrying a **VM fileserver hosted in Azure**, which forced the question of whether that is scope creep. It is not — and the reason produced a better boundary than "on-prem" was: the criterion is **classical fileserver semantics** (mandatory lock, surviving ACLs, identity-preserving rename), which is about the *server*, not its location. A Windows VM in a cloud datacentre is inside the line; an object store is outside it. **V3-cloud stays OUT**, reaffirmed explicitly. It also improves the 2027-02-21 review question from the near-unfalsifiable *"did cloud get commoditised?"* to *"has anyone built agent coordination for classical fileserver semantics?"*. **Two corrections of mine are recorded in the amendment**, because I first read "Azure fs" as Azure Files (PaaS) and drew two conclusions a VM fileserver does not support.

**Test environments now split three ways (D-045):** CI = every push, three OSes, regression. **Local rig** = rapid development, install process, file integrity — at zero latency, in a workgroup with **no Kerberos realm**, which is exactly why `net view` answered `System error 5` today. **Azure VM** = the auth path (Kerberos/Negotiate/OIDC) **and timing under real latency** — the first environment where lease renewal, retry budgets and the 256 MiB pre-image upload are exercised at non-zero RTT. **E-015 stops being blocked on environment and becomes blocked only on priority.**

**Verified:** **396** tests (from 384), 0 failed; clippy `-D warnings` clean on Windows **and** in WSL, the latter because the POSIX `N/A` classification is `#[cfg(not(windows))]` and nothing I ran on Windows compiled it.

**State changes:** I-007 **RESOLVED**; E-022 **DONE, fully verified** (`HIGH` row retired); **I-016** filed; **D-044**, **D-045** written and **D-037 amended**; `logbook/logs/2026-09.md` created. Tests 384 → 396. **No version change** — `0.1.3` stands; versioning is jok's.

**One thing I got wrong, recorded because the fix is a habit not a patch:** `cargo fmt -p chapr-endpoint -- <one file>` reformatted the **whole crate**, touching 19 files I had never edited — against the standing rule that fmt drift is jok's call. Reverted; the 5 files I did edit still carry formatting inside them, and the crate's other 214 drifted files are untouched.

**Open questions:** **15 now** — Q6 and Q12 closed. Still gating: **Q1** (does 5.1 chunked reads survive on its merits), **Q4** (chain versus erasure), and now **Q11** with I-016 attached, which is sharper than when it was hypothetical. Unanswered and cheap: does Claude Desktop populate `traceparent`?

**Next session start from:** **push, and read the CI run it triggers** — three changes land unproven there: B8's new exit-code semantics on all five legs, the `net use Z:` step, and whether the POSIX legs correctly report `N/A` rather than going red. WSL says they compile and pass; only Actions can say the workflow is right. **Green →** Phase B continues with **B3** (journal the move window, Q10) or **B6** (the verb-uniformity sweep the previous roadmap never asked for). **Then I-016 needs jok's semantics call** before B4 can be built. The rig is ready and reproducible: start `chapr-coord` on `CHAPR-FS`, `cmdkey` is stored, `Z:` maps on demand, and there is a `clean-share` checkpoint to revert to — **do revert it**, because 23 open conflicts accumulated across four smoke runs and nothing reaps them, correctly.

---


## Known Issues

> **Live issues only** — full narrative for every issue, live and resolved, is in
> `logbook/ISSUES.md`. Staleness rule: open > 30 days is flagged STALE at session
> start. Threshold: 5000 chars; the section is at **3419** — measured 2026-09-08,
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
