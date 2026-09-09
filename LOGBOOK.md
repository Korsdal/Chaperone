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
| `logbook/decisions/architecture.md` | 23 decision bodies — data model, protocol, read/write path, backends, invariants, coord internals |
| `logbook/decisions/deployment.md` | 12 decision bodies — installer, service, packaging, releases, auth, admin authority, hosting |
| `logbook/decisions/process.md` | 6 decision bodies — naming, licensing, repo posture, publication, agent/plugin behaviour |
| `logbook/decisions/product.md` | 4 decision bodies — product scope, positioning, market boundaries (new 2026-08-21, D-037) |
| `logbook/logs/2026-09.md` | 2 session entries (09-08, 09-07) |
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
corrected (2026-08-25), **Phase A "Truth" delivered (2026-09-07)**, and **Phase B's
correctness core now four items in — B0, B1, B2, B8 (2026-09-08) and B3
(2026-09-09)**. Multi-backend (SMB + POSIX); Windows, Linux and macOS all run the
suite in CI.

**Version `0.1.3`** (set by jok 2026-08-25), tagged `v0.1.3`. **No bump this
session** — it stays 0.1.x until it is tested and true; the minor number is a claim
about proven-ness, not a changelog of effort. Versioning is a human
responsibility; never fill in a bump.

**Status:** three crates build clean; **420** tests pass (was 396); clippy
`-D warnings` clean. **32** coord routes (was 29 — B3 added `/move/open`,
`/move/clear`, `GET /move/dangling`) + the **11-tool** MCP surface (unchanged),
plus the six-tab token-gated admin page. MSRV **1.88.0**. `cargo fmt` still drifts
and remains jok's call.

**⚠ Seven commits are unpushed, and four changes in them are unproven in CI.**
B8's exit-code semantics across all five legs; the `net use Z:` step; whether the
POSIX legs report **`N/A`** rather than going red; and **B3's `/move/open`, which
every smoke move now calls** — a wrong route registration would surface on the e2e
legs and nowhere else in CI. **Push, then read that run before anything else.**

**The 2026-09-08 session spent a day uncommitted, and the record said otherwise.**
Current State claimed its CI-bound changes were "committed but unproven"; they were
not committed at all, and GitHub's newest run was still **#14 on `cf25082`**.
Nothing was lost — verified at 396 tests before committing — but for a day the
project's newest correctness work had no copy anywhere. **`git status` now belongs
in the session-end protocol, beside the logbook write.**

**B3 landed, so D-013's "stale-but-recoverable" is now true (D-046).** A
`move_journal` row records the rename's intent beforehand, carrying everything the
migration needs because whoever finishes it is usually not the session that started
it. **`move_paths` deletes that row inside its own transaction** — the whole design
rests on this: a surviving row *proves* the migration did not commit, so completion
is exactly-once with no idempotency logic. `moverecover.rs` resolves them at
endpoint start-up: `src` gone and `dst` hashing to the recorded version → complete;
`src` still there → drop it, nothing happened; neither, or a `dst` written since →
**leave it and say why**, because completing then would record a version the file no
longer has. Verified against a live coordinator, not only wiremock; three guards
mutation-checked.

**A constraint that outlives B3: `db::migrate` is `CREATE TABLE IF NOT EXISTS` and
nothing else.** A new *table* reaches a database that already exists; a new *column*
is **silently skipped**, after which an endpoint queries a column the live
coordinator lacks. B3 needed a column on `journal`, could not have one, and used a
separate table — which the record wanted anyway. **The next change that genuinely
needs a column must land C0 first**, so C0 is no longer only Phase C's
prerequisite. Its priority is raised on the board.

**Deployment ordering rule, new: update coord before endpoints.** A new endpoint
against an older coordinator gets 404 on `/move/open` and refuses the move —
fail-closed, because skipping the intent would restore the unrecoverable window
while reporting success. The 404 is translated into an error naming that cause and
that fix.

**D-044's one unmeasured input is now measurable, and the measurement is owed.**
`rmcp` 2.2 does surface `_meta` (the service loop swaps it into `RequestContext`
before the handler runs; `ToolCallContext::new` then discards the params' copy), so
`traceprobe.rs` logs once per distinct **trace id** — capped at 32, and covering all
eleven tools via a hand-written `call_tool`. Proven over real stdio: two distinct
ids reported once each, a repeat id with a different span stayed silent, an absent
`_meta` reported absence. **What it will tell us:** one line per conversation is a
conversation id and moves D-044's ceiling; one per tool call is a request id and
does not; **no line at all means the hook did not run, not a negative.** The
procedure is `docs/measuring-session-identity.md` and it needs a host, two
conversations, one application run — **jok's to run.**

**There is a local test rig, and it remains the most reusable output of last week.**
A Hyper-V VM **`CHAPR-FS`** (Windows Server 2022) on an **Internal** switch:
no physical NIC bound, **no default gateway on either side**, `192.168.221.0/24`
(chosen not to collide with corp `172.16.43.0/24` or Hyper-V's NAT
`172.19.176.0/20`). Reachable only from this workstation. **Share on `D:`,
coordinator's db/blobs/audit on `C:`** — jok's correction, mirroring the pilot, and
it converts **I-009**'s mitigation from behavioural to structural because there is
no `..\..` path from `D:\` to `C:\`. Auth is a stored credential
(`cmdkey /add:CHAPR-FS`), because the VM is a workgroup member with **no Kerberos
realm** — which is itself one of the postures the product must handle. **Revert the
`clean-share` checkpoint before using it.**

**Three test environments, three jobs, none replacing another (D-045).**
**CI** — every push, three OSes, regression; loopback share, no realm, no latency.
**Local rig** — rapid development, install process, file integrity, kill-mid-write,
mapped drives; zero RTT, no realm. **Azure VM fileserver** (demo tenant, available,
not yet used) — the **auth path** (Kerberos/Negotiate/OIDC) and **timing under real
network latency**, which nothing else covers. **E-015 is therefore no longer blocked
on environment, only on priority.**

**What Phase B closed earlier, and what it cost.** **I-007 is RESOLVED** — the last
invariant-4 violation. The rename runs through the handle held since the source's
CAS (`SetFileInformationByHandle(FileRenameInfo)` + `DELETE` in the access mask, the
method D-027 settled empirically); the bug was a single `drop`. **The two-path
`FsPrimitives::rename` was deleted**, with `winfs::move_file` and `posixfs::rename`,
because it can only be reached by closing the handle first — renaming is now a
method on the *held file*, so the safe order is the only expressible one.
**`move_cas_core` went from zero tests to eight**, plus five Win32 tests, and the
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
Accepted as the ceiling; the concept gets **renamed** to stop overclaiming. The
`traceparent` probe D-044 asked for now exists (above) — the code is in, the run
is owed.
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
1. **Push the seven commits, then read the CI run** — the four unproven changes
   above. Red on the POSIX legs means the `N/A` classification is wrong; red on
   Windows means the `net use` step is; a failure inside a smoke *move* means
   `/move/open` is registered or called under a different path than it looks.
2. **B6 is the remaining unblocked Phase B item** — the verb-uniformity sweep:
   `create` alone skips the `~$F` preflight, `restore(copy)` takes no lease or path
   lock, `delete`/`move` return `Conflict` with `sidecar_path` pointing at the file
   itself and create **no** sidecar. Part decision, part fix.
3. **C0 has a raised claim on attention** — not for Phase C's sake but because
   `CREATE TABLE IF NOT EXISTS` is now a known constraint on every schema change,
   and the next one needing a column cannot work around it the way B3 did.
4. **I-016 needs jok's semantics call** before B4 can be built; **Q13** before B5.
5. **Owed and cheap: run the `traceparent` measurement.** The code and the
   procedure exist (`docs/measuring-session-identity.md`); it needs a host, two
   conversations in one application run, and the log lines pasted back.
6. **Phase C** stays gated on Q4; **Phase D** is the cheapest phase and partly
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
> The section is at **9088 bytes across 45 rows** — re-measured 2026-09-09, not
> carried over — so there is room for roughly 20 more decisions before this needs
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
> **`D-009` was never issued.** D-001…D-008, D-010…D-046; nothing was deleted or
> retracted, so stop looking for it. (Recorded 2026-09-07 with the D-038 dedupe —
> see that session's entry.)

| ID | Decision | Date | Theme | Status |
|----|----------|------|-------|--------|
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
> 2026-09-09 at 7713 bytes, re-measured.
> **Take the ~9 KB ceiling on a single entry literally**: it is the real limit,
> and prose that feels essential while writing is usually already in a decision
> body or a commit message. Headroom is thin by design — the entry here is
> replaced rather than appended to, so the section does not grow between sessions.

| Month | Entries |
|---|---|
| `logbook/logs/2026-09.md` | 2 — 2026-09-08, 2026-09-07 |
| `logbook/logs/2026-08.md` | 9 — 2026-08-28, 08-25, 08-21b, 08-21, 08-19/20, 08-14, 08-06, 08-05, 08-03 |
| `logbook/logs/2026-07.md` | 18 — 2026-07-22 (a–c), 2026-07-21 (base, b–o) |

### Session 2026-09-09 — jok / Claude
**Type:** engineering (commit the owed work, the `traceparent` probe, B3)
**Focus:** jok's call was "1 + 6, then look at 2 and 3". Item 1 turned out to be larger than a chore: the work it was meant to push did not exist in git yet.

**Worked on:**
- [x] **The 2026-09-08 session was never committed, and Current State said it was.** It claimed the three CI-bound changes were *"committed but unproven"*; `HEAD` had no `Unverified` in `selftest.rs` and no `net use` in `ci.yml`, and GitHub's newest run was still **#14 on `cf25082`**. B1, B2, B8, E-022's verification, D-044, D-045, I-016 and every logbook entry sat in one uncommitted working tree for a day. Nothing was lost. **Verified before committing — 396 tests, 0 failed, clippy clean, re-measured — then committed as three coherent pieces.** The habit it costs: `git status` belongs in the session-end protocol, beside the logbook write.
- [x] **The `traceparent` probe (D-044's one unmeasured input), and it works.** `rmcp` 2.2 *does* surface `_meta` — the service loop swaps it into `RequestContext` before the handler runs and `ToolCallContext::new` then discards the params' copy, so `context.meta` is the only place a stdio server can read it. `traceprobe.rs` logs once per distinct **trace id** (keyed on the id, not the header — the span changes per operation by design), with a cap at 32 so a per-request host cannot fill a log. `call_tool` is hand-written so all eleven tools are covered; `#[tool_handler]` only generates it when the impl does not.
- [x] **Proven over real stdio, not just in tests.** Five `tools/call` requests into the built binary: two distinct trace ids reported once each, a third call reusing a trace id with a different span **stayed silent**, and a call with no `_meta` reported absence. That run also caught two of my own errors — requests are spawned as concurrent tasks so **line order is not call order** (the absence line printed before calls that arrived earlier, and the message no longer says "on the first tool call"), and the absence message contained the exact phrase the other line is grepped by.
- [x] **`docs/measuring-session-identity.md`** — what the trail can attribute today, the procedure, and a table from each possible log output to its conclusion. Reader-facing per the `docs/`-versus-`specs/` split, and indexed from README. **The measurement itself is owed and is jok's**: it needs a host, two conversations, one application run.
- [x] **B3 landed — D-013's "stale-but-recoverable" is now true.** A `move_journal` row records the intent before the rename, carrying everything the migration needs because whoever finishes it is usually not the session that started it. **`move_paths` deletes that row inside its own transaction**, and that ordering is the whole design: a surviving row *proves* the migration did not commit, so completion is exactly-once with no idempotency logic and no state where both exist. `moverecover.rs` resolves them at endpoint start-up — `src` gone and `dst` hashing to the recorded version → complete; `src` still there → drop it, nothing happened; neither, or a `dst` written since → leave it and say why. Fail-closed both ways: a move that cannot record its intent does not touch the file, and a rename that fails clears the intent it opened.

**The finding inside B3, and it outlives B3: `db::migrate` is `CREATE TABLE IF NOT EXISTS` and nothing else.** A new *table* appears on a database that already exists; a new *column* is silently skipped, after which the endpoint queries a column the live coordinator does not have. B3 wanted a column on `journal` and could not have one — so it got a separate table, which the record wanted anyway (different fields, and keyed by `src` while the file ends up at `dst`). **The next schema change that genuinely needs a column has no such escape.** C0's priority is raised on the board: it is no longer only Phase C's prerequisite, it is a constraint on every schema change.

**Verified:** **420** tests (from 396), 0 failed; clippy `-D warnings` clean. Three of B3's guards are **mutation-checked** — dropping the in-transaction delete, the `src`-exists branch, and the destination version comparison each fail the test written for them. And B3 was driven **against a live coordinator**, not only wiremock: the three routes answer, the entry round-trips through the wire types, both discharge paths work, the row survives a coord restart, and the startup scan reports it. That last check exists because the unit tests mock each side separately and would not have caught a path-string mismatch between client and router.

**One deployment consequence, stated because it is easy to get wrong: update coord before endpoints.** A new endpoint against the installed older coordinator gets 404 on `/move/open` and refuses the move — the right direction, since skipping the intent would restore the unrecoverable window while reporting success. The 404 is translated into an error naming that cause and that fix, rather than reporting "HTTP 404 Not Found" on a file operation.

**State changes:** **B3 DONE**; coord routes **29 → 32** (`/move/open`, `/move/clear`, `GET /move/dangling`); tests 396 → 420; new modules `chapr-endpoint/src/traceprobe.rs` and `moverecover.rs`; new coord table `move_journal`; `docs/measuring-session-identity.md` added and indexed from README. **Seven commits unpushed.** No version change — `0.1.3` stands; versioning is jok's.

**Open questions:** unchanged at 15, and none closed today. **Q10 is answered by B3** rather than still gating it. Still jok's: **Q11** (I-016, gates B4), **Q13** (gates B5), **Q4** (gates C3/C4), **Q1**. The `traceparent` question is now *runnable* rather than open-ended — the code and the recipe exist, the run does not.

**Next session start from:** **push the seven commits, then read the CI run.** Four changes land unproven: B8's exit-code semantics on all five legs, the `net use Z:` step, whether the POSIX legs report `N/A` rather than going red, and **B3's `/move/open` on the e2e legs** — every smoke move now makes that call, so a wrong route registration would show up there and nowhere else in CI. **Then:** **B6** (verb-uniformity sweep) is the remaining unblocked Phase B item, and **C0** has a raised claim on attention after B3's schema finding. **I-016 still needs jok's semantics call** before B4. Owed and cheap: run the `traceparent` measurement per `docs/measuring-session-identity.md` and paste the log lines. The rig is unchanged and reproducible — revert the `clean-share` checkpoint before using it.

---


## Known Issues

> **Live issues only** — full narrative for every issue, live and resolved, is in
> `logbook/ISSUES.md`. Staleness rule: open > 30 days is flagged STALE at session
> start. Threshold: 5000 chars; the section is at **3440** — measured 2026-09-09,
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
