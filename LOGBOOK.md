---
logbook:
  project: Chaperone
  type: engineering-logbook
  version: "1.0"
  created: "2026-07-21"
  last_updated: "2026-09-10"
  last_updated_by: "Claude (C0; D-049 packaging)"

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
| `logbook/logs/2026-09.md` | 5 session entries (09-09c, 09-09b, 09-09a, 09-08, 09-07) |
| `logbook/logs/2026-08.md` | 9 session entries (08-03 … 08-28) |
| `logbook/logs/2026-07.md` | 18 session entries (07-21 … 07-22) |
| `logbook/ISSUES.md` | all 18 issues in full, live and resolved |
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
core B0/B1/B2/B8 (2026-09-08) and B3 (2026-09-09)**, and the cowork spec's two
workstreams — **W1, the endpoint slice (D-047)**, and **W2, the coord setup epic
(D-048)**, both 2026-09-09 — and **C0, migration machinery (2026-09-10)**.
Multi-backend (SMB + POSIX); Windows, Linux and macOS all run the suite in CI.
**Test scope narrowed 2026-09-10 (D-049): Windows rig + Azure VM only, Windows
and Linux endpoints, POSIX parked with its own rig** until the SMB story is good.

**Version `0.1.4`, set by jok 2026-09-09 — the tag is not pushed.** A patch bump
again, deliberately, even though `chapr_restore` gains a required argument: it
stays 0.1.x until it is tested and true, and the minor number is a claim about
proven-ness rather than a changelog of effort. Versioning is a human
responsibility; never fill in a bump.

**C0 landed 2026-09-10 — `db::migrate` is no longer `CREATE TABLE IF NOT EXISTS`.**
Plain versioned SQL in `crates/chapr-coord/migrations/`, applied in order and
recorded in a `schema_migrations` ledger (**D-041**'s option (a)). **The macro was
not used**: `sqlx::migrate!` needs sqlx's `macros` feature, which drags a
proc-macro crate and the MySQL and Postgres drivers into a SQLite-only build.
**Each migration file owns its own `BEGIN`/`COMMIT` and inserts its own ledger
row** — `raw_sql` inside a `pool.begin()` transaction makes the server future
non-`Send`, reported far away at `service_win.rs`'s `rt.spawn`; do not "tidy" it
back. Verified by a real upgrade, not only in memory: the pre-C0 binary's database
file, migrated by the new binary, no re-run on restart. **What this unblocks:**
move provenance in history (2.4), the audit chain and retention (C3/C4) — any
change that is not additive-by-table.

**Status:** three crates build clean; **449** tests pass (was 444); clippy
`-D warnings` clean. **12** tools (was 11 — `chapr_mkdir`) against **32** coord
service routes, plus the six-tab token-gated admin page whose Audit tab now
searches by path and detail. Coord has **6** subcommands (was 3 — `handover`,
`status`, `uninstall`) and its setup wizard now runs **in a browser** by default.
MSRV **1.88.0**. `cargo fmt` still drifts and remains jok's call.
*Counts re-measured 2026-09-09 by grepping `#[tool(` and `.route(`; tools and
subcommands both moved this session, so do not carry them forward unread — that is
what I-004 is for. The wizard's own two routes are deliberately **not** in the 32:
they live on a temporary loopback listener, not the service router.*

**⚠ What has NOT been driven against a real share or a real service install.**
Both workstreams are verified by the unit suite, wiremock, real local processes
and a live *local* coordinator — **not** by the rig. CI run **#16 on `fdbc2f8`
was green across 7 jobs** and predates all of it. Specifically:

- **W1** (the endpoint slice) has never touched an SMB share.
- **W2**'s service paths have **never executed**: every walkthrough used
  `--no-service` in an unelevated shell, so `install_service`, `remove_service`
  and the SCM branches of `lifecycle.rs` are untested outside compilation. That
  needs one elevated run on `CHAPR-FS`.

The plan's verification walkthrough is owed for both: rebuild the endpoint bundle
*without* defaults, revert the `clean-share` checkpoint, and confirm the four
misconfiguration states are distinguishable from the tool surface alone. **jok
has a happy-path-test skill** for driving the MCP from the user side once the
whole spec is done, which is the natural close.

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

~~**A constraint that outlives B3: `db::migrate` is `CREATE TABLE IF NOT EXISTS`
and nothing else.**~~ **Resolved 2026-09-10 by C0** — see above. A column now
reaches an existing database.

**W2 landed: the coordinator has a life beyond install (D-048).** The wizard runs
**in a browser** — `--ui`, and the default for a double-click, while `setup` keeps
the prompts — on loopback, behind a one-time token, single-shot. It is a **front
end for `SetupArgs` and nothing else**: it calls the same `setup::run`, so probe /
config write / hardening / service install / handover exist once and cannot drift.
`status` reports config, service existence, service state and port response
separately, exiting 0 only when all four hold. `uninstall` removes the service and
**keeps every byte of data**, with no `--purge`. `handover` reprints the values
and adds an `mcpServers` block — load-bearing now that bundles ship no defaults.

**There is still no `chapr-coord update`, but the reason has changed.** C0 lifted
the schema blocker on 2026-09-10; what is now missing is the *packaging* half, and
**D-049 gives it to the MSI** rather than to a subcommand. The interim path is
unchanged and still documented: uninstall → replace the binary → setup against the
**same** data directory, which keeps history and the audit trail.
**Ordering rule:** coordinator before endpoints.

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

**What's next — the MSI installer rework, jok's call (2026-09-10).**
Design settled in **`specs/coord-windows-packaging-0910.md`** and recorded as
**D-049**. MSI (WiX) authored as a **third front end to `SetupArgs`**: it owns
placement, service registration and ACLs, then invokes `chapr-coord setup
--non-interactive` with its properties, so probe / config write / handover exist
once and cannot drift. Binary → `%ProgramFiles%\Chaperone`; **data stays in
`%ProgramData%\Chaperone`**. The setup wizard leaves this package for the later
one; **the admin page stays HTML on purpose, because it is reached from an
external admin host**. The rule to carry: *install is local and belongs to the
platform's installer; operations are remote and belong on the web surface.*
**The MSI does not remove C0's successors** — it upgrades a binary and a service,
it cannot add a column.

**Cheap, and worth doing first or alongside:**
- **F2's client fix** (`rustls-tls-native-roots`) plus the missing
  endpoint-over-TLS test. Smallest change on the board, and until it lands
  TLS-by-default ships a coordinator nothing can reach (**I-018**).
- **F1 and an explicit `data_dir`** (**I-017**). Four independent locations today
  — `db_url`, `blob_root`, the inferred token directory, and the TLS directory
  beside `config_out` — collapse to one, which makes the MSI a single property
  instead of four.

**Stopped and still owed:** the rig verification itself
(`specs/rig-verification-0910.md`) got one finding into Step 1 before being
halted. Nothing in it was invalidated; it resumes when there is something worth
verifying against.

**jok's read on the direction, 2026-09-10, and it should not be softened:**
*"everything reads as overengineering to me right now — we are adding more
friction per iteration."* The sharpest evidence is that **W2 shipped on 09-09 and
half of it was proposed for removal on 09-10**. The distinction worth holding is
that **F1 and F2 are not overengineering but under-verification** — not clever
code, basics never tested — and the two have opposite cures: build less, versus
prove what exists.

**Not blocking any of the above, and still jok's:** **Q11**/I-016 before B4,
**Q4** before C3/C4, **Q1**. **B7**'s three scenarios still need a fault-injecting
proxy that does not exist. **Owed and independent:** the `traceparent`
measurement (`docs/measuring-session-identity.md`), and whether uninstalling an
MCPB clears stored `user_config`.

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
> The section is at **10123 bytes across 48 rows** — re-measured 2026-09-10, not
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
| [D-049](logbook/decisions/deployment.md#d-049) | The Windows coordinator is installed by an **MSI**, authored as a third front end to `SetupArgs`; binary in Program Files, **data stays in ProgramData**; the setup wizard defers to the later package, the admin page **stays HTML** because it is reached from an external admin host | 2026-09-10 | deployment | CURRENT (amends D-048) |
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
> 2026-09-10 at 7795 bytes, re-measured at session end.
> **Take the ~9 KB ceiling on a single entry literally**: it is the real limit,
> and prose that feels essential while writing is usually already in a decision
> body or a commit message. Headroom is thin by design — the entry here is
> replaced rather than appended to, so the section does not grow between sessions.

| Month | Entries |
|---|---|
| `logbook/logs/2026-09.md` | 5 — 2026-09-09c, 09-09b, 09-09a, 09-08, 09-07 |
| `logbook/logs/2026-08.md` | 9 — 2026-08-28, 08-25, 08-21b, 08-21, 08-19/20, 08-14, 08-06, 08-05, 08-03 |
| `logbook/logs/2026-07.md` | 18 — 2026-07-22 (a–c), 2026-07-21 (base, b–o) |

### Session 2026-09-10 — jok / Claude
**Type:** engineering (C0) + a packaging direction change
**Focus:** the rig verification was stopped one finding in. Two defects found instead, the Windows install story reopened (**D-049**), and **C0 built**.

**Worked on:**
- [x] **Built v0.1.4 release binaries and the rig bundle.** `build/chapr-coord.exe` and `build/chaperone-endpoint-rig.mcpb`. The bundle in `build/` was **0.1.3 from 09-09 12:32** — the B3 build, predating W1 and W2 — so installing it would have tested yesterday's code. Rebuilt from the template: no defaults, `coordinated_root` required, no packager note.
- [x] **F1 — a bare-path `db_url` silently disables both tokens.** Reported as "the admin page does not authenticate with the generated token"; it is neither the page nor the token. `db_file_path` (`config.rs:489`) requires a `sqlite:` prefix, `data_dir()` is built on it, and **both credentials live in that directory** — so no `admin-token` file is ever written and every admin route answers 503. `db::connect` disagrees: `SqliteConnectOptions::from_str` accepts a bare path, so the database opens, the service runs and the page loads. **Two functions hold different definitions of `db_url`, and the lenient one is the visible one.** `shared-secret` already fails closed here; `trusted-header` and `none` do not.
- [x] **F2 — the endpoint cannot use TLS at all, and trusting the certificate does not help.** The browser interstitial jok saw is the small half. `Cargo.toml:89` builds reqwest with `rustls-tls` = **webpki-roots, Mozilla's public bundle** (`webpki-roots 1.0.9` in the lock file; `rustls-native-certs` absent). No `add_root_certificate`, no CA option, **no test anywhere driving the endpoint against an HTTPS coordinator**. So a self-signed cert is refused, importing it to Trusted Root changes nothing, and an internal CA is refused too. **TLS has been on by default since phase 1, so the default configuration is one no endpoint can talk to.** Setup's own advice — "install this certificate as trusted on the laptops" — is currently false.
- [x] **C0 built and verified.** `migrations/0001_baseline.sql` (the v0.1.4 schema verbatim, carrying the table notes off the old `SCHEMA` constant) plus a ~20-line versioned migrator over a `schema_migrations` ledger. **449 tests** (+5), clippy clean.
- [x] **A real upgrade rehearsal, not an in-memory one.** This morning's pre-C0 binary created a database file; the new binary against that same file logged `schema migration applied version=1 name=baseline` and served; a second restart did not re-run it — which is the proof the ledger row reached disk, there being no sqlite3 here.

**Two C0 choices that deviate from the obvious, both commented at the site.** **Not `sqlx::migrate!`**: the macro needs sqlx's `macros` feature, which drags a proc-macro crate plus the MySQL and Postgres drivers into a build that speaks only SQLite — twenty lines cost less, and D-041's option (a) is what shipped; only the macro is skipped. **Each migration file owns its own `BEGIN`/`COMMIT` and inserts its own ledger row**: `raw_sql(..).execute(&mut *tx)` makes the server future non-`Send` and reports it at the `rt.spawn` in `service_win.rs` naming `&Pool<Sqlite>` and `&str` — the async-ownership cost the notes predicted, two attempts spent on it. One `raw_sql` on the pool keeps atomicity and uses the shape that has compiled since E-002. The implicit contract that creates is checked by `every_migration_records_its_own_version`, not remembered.

**The direction change, and jok's reason for it.** The premise offered was that coord is packaged vendor-specifically; **it is not** — SMB vs POSIX is runtime config, and what is compiled in is *host OS* (25 `cfg(windows)`/`cfg(unix)` sites). The conclusion survived anyway: the Windows build is already Windows-only, so **two of D-048's three objections to a native installer fall**, and the third — "coord holds no Windows primitives" — was already false. Scope narrowed to **Windows rig + Azure VM only**, Windows and Linux endpoints testing, POSIX parked with its own rig. Then jok stopped it: *"everything reads as overengineering to me right now. We are adding more friction per iteration."* **He is right, and the sharpest evidence is mine: W2 shipped on 09-09 and half of it was proposed for removal on 09-10.** The distinction worth keeping is that F1 and F2 are not overengineering but **under-verification** — not clever code, basics never tested — and the two have opposite cures.

**State changes:** **449** tests (was 444). `db.rs` loses `SCHEMA`; `migrations/` is new. No dependency added, no sqlx feature added. `0.1.4` stands — no version change. **D-049** written; **I-017** and **I-018** filed. Three new working docs, all in untracked `specs/`: `rig-findings-0910.md` (F1, F2), `coord-windows-packaging-0910.md` (the design), `rig-connect.md` (mapping `Z:`).

**Open questions:** unchanged and still jok's — **Q4**, **Q11** (I-016, gates B4), **Q1**. Nothing today needed a new one. Whether the MSI carries the endpoint bundle or only the coordinator is the one new open item, recorded in D-049.

**Next session start from:** **the MSI installer rework — first item, jok's call.** Design is written and settled in `specs/coord-windows-packaging-0910.md`: MSI (WiX) authored as a **third front end to `SetupArgs`**, owning placement, service and ACLs and then invoking `chapr-coord setup --non-interactive` with its properties, so probe / config write / handover are never reimplemented. Binary → `%ProgramFiles%\Chaperone`; **data stays in `%ProgramData%\Chaperone`** (jok: "I can live with that"). The setup wizard leaves this package and returns in the later one; **the admin page stays HTML deliberately, because it is reached from an external admin host** — which is the rule to carry: *install is local and belongs to the platform's installer; operations are remote and belong on the web surface.*

**Before or alongside it, and both cheap:** **F2's one-line client fix** (`rustls-tls-native-roots`) plus the missing endpoint-over-TLS test — until it lands, TLS-by-default ships a coordinator nothing can reach, and it is the smallest change on the board. **F1 and the explicit `data_dir` field** — four independent locations today (`db_url`, `blob_root`, the inferred token dir, and the TLS dir beside `config_out`) collapse to one, which is what makes the MSI expressible as a single property instead of four.

**Still owed and untouched:** the rig verification itself (`specs/rig-verification-0910.md`, stopped at Step 1), the `traceparent` measurement, and whether uninstalling an MCPB clears stored `user_config`. **Twelve commits are unpushed and `v0.1.4` still has no tag**; both jok's.


## Known Issues

> **Live issues only** — full narrative for every issue, live and resolved, is in
> `logbook/ISSUES.md`. Staleness rule: open > 30 days is flagged STALE at session
> start. Threshold: 5000 chars; the section is at **3863** — measured 2026-09-10,
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
| [I-018](logbook/ISSUES.md#i-018) | **The endpoint cannot use TLS at all** — reqwest links webpki-roots, so a private cert is refused and trusting it changes nothing. TLS-on-by-default is unreachable. | **HIGH** | 2026-09-10 | OPEN (one-line fix known) |
| [I-017](logbook/ISSUES.md#i-017) | A bare-path `db_url` silently disables both tokens — `data_dir()` needs a `sqlite:` prefix, `db::connect` does not. | MED | 2026-09-10 | OPEN (fix chosen) |
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
