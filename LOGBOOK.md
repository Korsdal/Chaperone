---
logbook:
  project: Chaperone
  type: engineering-logbook
  version: "1.0"
  created: "2026-07-21"
  last_updated: "2026-08-21"
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
    threshold_chars: 8000
    # Split by THEME, not by year (jok, 2026-08-21): all 34 entries fall inside
    # five weeks, so a date axis discriminates nothing. `/logbook decide` appends
    # the body to the theme file below and adds one row to the index in this file.
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
| `logbook/decisions/architecture.md` | 20 decision bodies — data model, protocol, read/write path, backends, invariants, coord internals |
| `logbook/decisions/deployment.md` | 11 decision bodies — installer, service, packaging, releases, auth, admin authority, hosting |
| `logbook/decisions/process.md` | 5 decision bodies — naming, licensing, repo posture, publication, agent/plugin behaviour |
| `logbook/decisions/product.md` | 1 decision body — product scope, positioning, market boundaries (new 2026-08-21, D-037) |
| `logbook/logs/2026-08.md` | 6 session entries (08-03 … 08-21) |
| `logbook/logs/2026-07.md` | 18 session entries (07-21 … 07-22) |
| `logbook/ISSUES.md` | all 13 issues in full, live and resolved |
| `logbook/BACKLOG.md` | live backlog + Delivered appendix (26 rows) + removed duplicates |
| `logbook/state-history.md` | narrative displaced from Current State, newest first |

---

## Current State

> **VOLATILE** — rewritten (not appended to) at every session end.
> Freshness: 7 days. If `last_updated` in YAML is older, flag as stale.
> Superseded narrative → `logbook/state-history.md`.

**Phase:** implementation — v1 complete, installed at a customer, pilot-tested on
real hardware (2026-08-14), and **phase 1 of the 0.2 plan delivered (2026-08-21)**.
Multi-backend (SMB + POSIX); Windows, Linux **and macOS** now all run the test
suite in CI.

**Version `0.1.2`** (set by jok 2026-08-21). A *patch* bump carrying phase 1,
deliberately: **it stays 0.1.x until it is tested and true** — the minor number is a
claim about proven-ness, not a changelog of effort. Versioning is a human
responsibility; never fill in a bump.

**Status:** three crates build clean; **364** tests pass (was 324); clippy
`-D warnings` clean. **29** coord routes + the **11-tool** MCP surface (unchanged —
phase 1 added no tools), plus the six-tab token-gated admin page. MSRV **1.88.0**.
*Every number here measured this session, not carried over.*

**What phase 1 changed, in one line each:**
- **1.3** `chapr_read` refuses binary containers (16 formats, magic bytes, never
  extension) with advice naming what to read instead. Base64 is now **opt-in on
  read** (`allow_binary`) — a behaviour change for any caller that relied on the
  old default.
- **1.2** TLS on by default in the shipped template; plaintext warns at every
  start; a missing cert and a scheme/TLS mismatch are both refused with a usable
  message.
- **1.1** The control channel **authenticates**: `shared-secret` requires a
  per-deployment token *plus* the principal header. The wizard's default. Nine
  routes that carried no guard at all are closed.
- **1.5 / 1.4** macOS in CI; a new `e2e` job stands up a coordinator and drives the
  real smoke suites + the shipped self-test, with the Windows leg on a **real SMB
  share**.

**The honest limits of 1.1, because the docs must keep saying so:** the token proves
*this is one of our endpoints*, not *this is that user* — an endpoint holding the
secret can still name any principal, and the blob store applies no ACL check to an
authenticated caller. One secret per deployment, so revoking one laptop means
rotating for all of them. Binding identity to a verified subject is E-015.

**⚠️ Nothing in phase 1 has run in CI yet.** Every step of the new job was proven
verbatim locally on Windows, but `New-SmbShare` on a GitHub runner, both POSIX legs
and whether Actions even accepts the YAML are unverified until the first push. Treat
1.4 and 1.5 as *delivered but unexercised* until that run is green.

**Project memory is now public (this session, jok's call).** `LOGBOOK.md` and
`logbook/` are **tracked**, reversing D-033's exclusion of them. Customer
identifiers were scrubbed first: the deployable repo, the collaborator's plugin, its
scripts and its folder names are described by role, not named. Contributors listed
in `NOTICE` stay named. **Write for that audience from here on** — candid about
engineering, never about a customer's internals. `CLAUDE.md` stays untracked. This
also retires the "only copy on one laptop" risk that had been open since 2026-08-14.

**On-prem is the product (D-037).** Cloud is deferred on a *market judgment*, not an
architectural exclusion — review **2027-02-21**. V3-cloud and E-021 stay booked and
invariant 2 stays provisional, which is the price paid for keeping the option open.

**Before touching the write path,** read the behaviour notes in
`logbook/state-history.md` (2026-08-05): SQLite runs in **WAL** (10 s busy timeout);
coord `PUT /blobs` is bounded at **256 MiB**, which is also the largest file
Chaperone can write at all, because a write snapshots the pre-image; `put_blob` runs
**before** `journal_open`.

**Known and accepted (not a bug):** after setup hardens the data directory, an
**unelevated** `chapr-coord serve --config <that file>` cannot read its own config.
The real deployment runs as a service account with access; hand-write a config
elsewhere to run coord by hand.

**Environment:** a Linux toolchain exists in WSL, building with an isolated
`CARGO_TARGET_DIR=$HOME/chapr-target` so the Windows `target/` is never clobbered.

**What's in flight:** *(nothing — pushed at `d4db4de`; the CI run is the open
question, not the code)*

**Blocker — DIAGNOSIS CORRECTED 2026-08-21, it was wrong for five weeks.** This
was recorded as *"the on-disk key is rejected (`Permission denied (publickey)`)"*.
**The key is not rejected.** `~/.ssh/id_ed25519` is **passphrase-protected**, no
ssh-agent is running, and Claude's Bash context is non-interactive — so it cannot
supply the passphrase, offers no usable key, and GitHub answers
`Permission denied (publickey)`, which *looks* like a bad key and is not. Proven by
`ssh-keygen -y -f ~/.ssh/id_ed25519` returning "incorrect passphrase supplied".

Two consequences. It is **fixable** — Windows' `ssh-agent` service (currently
`Disabled`) plus `git config --global core.sshCommand` pointed at
`C:\Windows\System32\OpenSSH\ssh.exe`, because git from bash otherwise resolves
`ssh` to Git's MSYS build, which cannot see the Windows agent. And it is arguably
**not worth fixing**: a passphrase only a human can supply is why an unattended
process cannot push to the public repo, which for a project whose framing is
accountability is a feature. **jok pushes** stays the working policy; it just is not
a defect.

*(Separately: jok's own `git push` failed once this session with
`kex_exchange_identification: Connection closed by remote host`. Unrelated — that is
GitHub's transient SSH throttle, not a key or network problem, and a retry
succeeded. Two failures with opposite causes, easy to conflate, and this entry
previously did.)*

**What's next:**
1. **The CI run** — the first push decides whether 1.4 and 1.5 hold. Expect SMB
   share creation and macOS hostname resolution to be where it breaks; both are
   one-line fixes in `ci.yml`.
2. **Phase 1's own leftovers:** the lease-leak suite needs a fault-injecting proxy
   that does not exist in the repo; kill-mid-write/journal-recovery and the Office
   `~$F` refusal still have no example. `cargo fmt` is still absent from CI because
   365 files drift from rustfmt — a separate mechanical commit, jok's call.
3. **D-D′ — should D-028's line move?** The gate on Track B. Needs jok + Kristian,
   not code. Everything else in Track B (the mirror convention, `chapr.stat` mirror
   fields, chunked reads) is independent of the answer.
4. **Phase 2, the sovereignty floor** — 2.2 supply chain (cheap, procurement-facing),
   2.3 publish the decision records (now much smaller: the logbook is public, so it
   is a cross-referencing job), 2.1 targeted purge (watch the dedup hazard).
5. **`gh release create` + provenance attestation** (I-011) — still never executed;
   a real tag is the first execution.
6. **Have Kristian run `chapr-endpoint self-test` against the real share**, with
   `CHAPR_SELFTEST_DIR` on a **mapped drive**, to exercise E-022's unverified branch.
7. **Update `specs/fmcp-architecture-concept.md` to match D-026** — it still
   asserts the two channels "never cross". (CLAUDE.md's half is done.)

Deferred engineering (E-015, E-020, E-021, E-024b, V3-cloud) → `logbook/BACKLOG.md`.

---

## Active Sessions

> Populated by engineers/agents at session start. Cleared at session end.
> If this section is non-empty when you start: coordinate before working.

*(empty)*

---


## Decision Log

> **Index only** — one row per decision, most recent first. Bodies are in
> `logbook/decisions/<theme>.md`; click an ID to jump to its entry.
> Threshold: 8000 chars; the section is at **7124** (measured 2026-08-21 after
> D-038), so only ~6 more rows fit before the index itself needs a call (split by
> theme into per-theme index tables). Re-measure rather than trusting this
> number — it has already gone stale twice, which is I-013's whole point.
>
> `/logbook decide`: append the body to the theme file (see
> `sections.decision_log.themes` in the YAML), then add one row here.
> Never delete a row — mark `SUPERSEDED-BY-D-NNN` or `INVALIDATED`.

| ID | Decision | Date | Theme | Status |
|----|----------|------|-------|--------|
| [D-038](logbook/decisions/process.md#d-038) | Project memory is published: `LOGBOOK.md` and `logbook/` become tracked, customer identifiers scrubbed | 2026-08-21 | process | CURRENT |
| [D-038](logbook/decisions/process.md#d-038) | Project memory is published: `LOGBOOK.md` and `logbook/` become tracked, customer identifiers scrubbed | 2026-08-21 | process | CURRENT |
| [D-037](logbook/decisions/product.md#d-037) | On-prem is the product; cloud stays deferred on a market judgment, not an architectural exclusion | 2026-08-21 | product | CURRENT (review 2027-02-21) |
| [D-036](logbook/decisions/process.md#d-036) | Project memory splits by theme under `logbook/`; the root file becomes an index | 2026-08-21 | process | CURRENT |
| [D-035](logbook/decisions/deployment.md#d-035) | The endpoint is delivered as an MCP server, not as a Claude Desktop extension | 2026-08-19 | deployment | CURRENT |
| [D-034](logbook/decisions/deployment.md#d-034) | Releases are CI-built artifacts on a tag, not committed binaries | 2026-08-19 | deployment | CURRENT |
| [D-033](logbook/decisions/process.md#d-033) | Chaperone is Apache-2.0; attribution rides in NOTICE and in every file | 2026-08-19 | process | CURRENT |
| [D-032](logbook/decisions/deployment.md#d-032) | The executable is the installer; a bind address is not a URL; the CRT ships inside the binary | 2026-08-14 | deployment | CURRENT ᵃ |
| [D-031](logbook/decisions/deployment.md#d-031) | Admin authority: a token enforces, a role follows; auth changes as a dual-mode cutover | 2026-08-12 | deployment | CURRENT ᵃ |
| [D-030](logbook/decisions/architecture.md#d-030) | Subagent fan-out: serialize intra-session rather than merge sidecars; resolve drive letters rather than require them; diagnostics separate from audit | 2026-08-12 | architecture | CURRENT ᵃ |
| [D-029](logbook/decisions/deployment.md#d-029) | Admin authority on coord: a role on the Authenticator seam, not OS elevation; data dir gated by installer ACL | 2026-08-12 | deployment | CURRENT ᵃ |
| [D-028](logbook/decisions/process.md#d-028) | Chaperone stays plugin-neutral: the MCP announces the coordinated root, the agent reinterprets its own writes | 2026-08-12 | process | CURRENT ᵃ |
| [D-027](logbook/decisions/architecture.md#d-027) | Audit remediation: baseline version-log entries, torn-file marker persistence, both Office lock conventions | 2026-08-06 | architecture | CURRENT ᵃ |
| [D-026](logbook/decisions/architecture.md#d-026) | Invariant 6: coord DOES see bytes, for history only (resolution (a)) | 2026-08-05 | architecture | CURRENT ᵃ |
| [D-025](logbook/decisions/deployment.md#d-025) | Deployment packaging + two-repo split (Chaperone + the customer deployable) | 2026-07-22 | deployment | CURRENT ᵃ |
| [D-024](logbook/decisions/deployment.md#d-024) | Coord host = on-prem Windows (confirmed); MVP identity = zero-setup ambient OS identity, enforced auth deferred | 2026-07-22 | deployment | CURRENT ᵃ |
| [D-023](logbook/decisions/deployment.md#d-023) | Control-plane auth: pluggable, generic OIDC (revises §13.1 "no OAuth") | 2026-07-22 | deployment | CURRENT ᵃ |
| [D-022](logbook/decisions/architecture.md#d-022) | E-017 scope: push watch endpoint (direct-apply, coord-local DTO) | 2026-07-22 | architecture | CURRENT ᵃ |
| [D-021](logbook/decisions/architecture.md#d-021) | E-019 scope: POSIX backend (shared §7 core, per-backend grammar) | 2026-07-22 | architecture | CURRENT ᵃ |
| [D-020](logbook/decisions/architecture.md#d-020) | E-018 Backend trait + coord backend-discovery seam | 2026-07-21 | architecture | CURRENT ᵃ |
| [D-019](logbook/decisions/deployment.md#d-019) | Deployment + backend-agnostic roadmap (brainstorm outcome) | 2026-07-21 | deployment | CURRENT ᵃ |
| [D-018](logbook/decisions/deployment.md#d-018) | E-016: coord installer + service + TLS | 2026-07-21 | deployment | CURRENT ᵃ |
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
| [D-001](logbook/decisions/process.md#d-001) | Project name: Chaperone / chapr.<method> | 2026-07-21 | process | CURRENT ᵃ |

ᵃ No `**Status:**` line exists in the source entry (a pre-existing gap, not
introduced by the split). CURRENT by inspection, 2026-08-21 — worth a one-line fix.

---

## Session Log

> The **newest** entry lives here in full, so a cold `/logbook start` can read
> `Next session start from:` without opening a child doc. Older entries are in
> `logbook/logs/YYYY-MM.md`, most recent first.
>
> `/logbook end`: **move the entry below into its month file first**, then write
> the new one here. Threshold: 10000 chars; index + one entry is 8898 today, so a
> session entry over ~8 KB breaches it on its own — compress or split then.

| Month | Entries |
|---|---|
| `logbook/logs/2026-08.md` | 5 — 2026-08-19/20, 08-14, 08-06, 08-05, 08-03 |
| `logbook/logs/2026-07.md` | 18 — 2026-07-22 (a–c), 2026-07-21 (base, b–o) |

### Session 2026-08-21b — jok / Claude
**Type:** engineering (roadmap review → phase 1 delivered)
**Focus:** review the untracked 0.2→0.5 roadmap against the code, then run phase 1 through. Ended with a scope decision (D-037), five delivered items, a version bump, and the logbook itself going public.

**Worked on:** *(per-item detail lives in Current State above and in the roadmap; this is the record of what happened, not a second copy of what it does)*
- [x] **Audited the roadmap proposal item-by-item against the tree** — three parallel explore passes (coord auth/TLS/GC, endpoint read path/tools/watcher, CI/examples/leases/docs). Verdicts + `file:line` evidence in `specs/reviewed_roadmap.md` (local); plan rewritten as v2 with v1 preserved beside it. **Item IDs kept stable** so the audit's verdicts still map 1:1. The audit's own findings mattered as much as the plan: 1.2 was already ~90% shipped, 4.2's lease mechanism does not work as the plan assumed, and 1.1's acceptance criterion already passed before anything was built.
- [x] **D-037** — on-prem is the product; cloud deferred on a *market judgment*, not an architectural exclusion (review 2027-02-21). New `product` decision theme. Phase 6 rescoped to linkage-only; identity stays pluggable.
- [x] **Phase 1 delivered, all five items** — 1.3 binary read guardrail (new `sniff` module), 1.2 TLS residue, 1.1 authenticated control channel (new `endpoint_token` module + `Authenticated` extractor over nine unguarded routes), 1.5 macOS in CI, 1.4 the e2e job. **Tool count unchanged at 11** — phase 1 added no tools, which item 3.2's reasoning about context cost required.
- [x] **Version → 0.1.2, set by jok.** A patch bump, deliberately: "it stays 0.1.x until it is tested and true."
- [x] **The logbook is now tracked (D-038)**, reversing D-033's exclusion of it, with customer identifiers scrubbed first. Also **`specs/`**: a new gitignored directory for jok's working material, so `docs/` holds only what a reader needs (three files, down from nine).
**Three bugs no test could see, all found by running the thing rather than testing it:**
- **`serve` never validated its config.** `Config::load` went straight to `run_server`; the structural rules were enforced only by the wizard's `probe` and the admin settings API. So a **hand-written** config — exactly what the shipped template invites — started a server with the auth allow-list, bind-address-as-URL and loopback rules all unchecked. Found by pointing `serve` at an https URL with no `[tls]` and watching it come up happily. `validate()` now runs inside `load`, after `apply_overrides` so an env var cannot smuggle past it.
- **The non-interactive wizard produced TLS with an `http://` URL.** The only scheme reconciliation lived inside `interactive_fill`, which `--non-interactive` skips entirely. Merely wrong before; **fatal** once the mismatch rule landed, and it would have broken the scripted installer.
- **The self-test reported "coordinator reachable" with no credential.** `/healthz` is deliberately open, so a wrong token surfaced several checks later as something unrelated. It now probes an authenticated route and stops with the real diagnosis.

**Verified, not assumed:**
- Every CI step run **verbatim locally on Windows** before being written into the workflow: config generation, hostname startup, the `pkill`→`taskkill` fallback, all four enforcement assertions, and the self-test at **8 passed / 0 failed / exit 0** — including *"a second exclusive open was refused while the first was held"*.
- Enforcement checked over real HTTP with curl, not only in unit tests: issued token → 200 with a real body; forged 64-char token → 401; principal header alone → 401; `journal/clear` unauthenticated → 401; `/healthz` → 200.
- Both smoke suites green against a live coordinator (`smoke_parts` 14/14, `smoke_pilot` 11/11 in 7.4 s including a 16 MiB blob).

**Mistakes of my own, recorded because they will recur:**
- **`sed` on Rust source bit three times.** An insertion anchored on a fn name landed *under* the `#[tokio::test]` belonging to the next function; a `\n` inside a temporary `println!` became real newlines, so deleting "the line" left five orphaned fragments; and a backslash in a folder-name pattern failed the whole expression. Use the file tools for code.
- **The worst one was nearly silent.** After removing an orphaned attribute the test count came out 182 where 183 was expected — chasing that single-test gap found I had **disabled an existing auth test** (`content_routes_require_an_identity_under_enforced_auth`): still compiling, no longer running. Count your tests.
- **Overreached twice on E-015**, claiming on-prem-only collapses it to `negotiate` and kills OIDC's rationale. Wrong both times: D-023 chose OIDC for three reasons and only the cloud one weakens here. Recorded in D-037 rather than quietly corrected.

**After the first CI runs — three commits, and one lesson worth more than the fixes:**
- **`edbc8eb`** — run 1 died on windows-latest at its *first step*, `cargo build`: `Could not resolve host: static.crates.io`. Cause was CI's new shape, not the runner alone — three jobs with warm caches became six, four brand new, all doing a cold `cargo fetch` in parallel. Fixed with `needs: check` (e2e reuses the cache `check` just saved, and skips entirely on a red build), a per-OS `shared-key`, `CARGO_NET_RETRY: 10`, sparse registry. It did not recur, so: transient, load-related.
- **`d4db4de`** — two real faults. clippy `useless_format` (`config.rs:554`), **not** a phase-1 regression: the line dates from `b1e7814` and only failed once CI's stable clippy reached **1.98.0**. Deleted rather than given clippy's `.to_string()` fix, which would have preserved a tautology. And macOS e2e died on **`sed -i`** — a GNU-ism; BSD sed reads the script as a backup suffix. Fixed by not editing the file: `CHAPR_COORD_AUTH=shared-secret` uses the documented defaults→file→env precedence.
- **The lesson, worth more than either fix: "clippy clean" was measured on the wrong toolchain.** Local was **0.1.97**; CI enforces `stable` = **1.98.0**. Installing 1.98.0 locally found a **second** lint the reported legs structurally could not show — `chunks_exact_to_as_chunks` in `watch_win.rs`, `cfg(windows)`, so never compiled by ubuntu or macOS. Windows hit it the moment its build cleared DNS. Fixing only what was reported would have bought another red. `as_chunks::<2>()` confirmed on MSRV by compiling against 1.88.0, not by reading release notes. Now green at both ends: clippy + 364 tests on 1.98.0, build + tests on 1.88.0.

**State changes:** version `0.1.2`; 364 tests; new modules `chapr-endpoint/src/sniff.rs` and `chapr-coord/src/endpoint_token.rs`; new `e2e` CI job (3 legs); new `product` decision theme; `LOGBOOK.md` + `logbook/` tracked. Docs corrected where they described the old behaviour: `security.md` (the control plane authenticates now, with the two remaining limits stated plainly), `architecture.md` (read limits + auth), `deployment-guide.md` (TLS and auth as explicit wizard steps), README, the MCPB manifest, and the coord config template.

**Open questions:** (1) **`cargo fmt` is still not in CI** — 365 files drift from rustfmt, so adding it means reformatting the tree. Mechanical, but a separate commit and jok's call. (2) **1.4 covers 2 of its 5 named scenarios**: the lease-leak suite needs a fault-injecting proxy that does not exist in the repo, and kill-mid-write/journal-recovery and the Office `~$F` refusal have no example at all. (3) **The e2e job has now run twice, and what it proved is narrower than green.** Actions accepts the YAML, and the macOS leg reached the auth-restart step — so share setup, coordinator startup and **both smoke suites passed on macOS**, the first end-to-end validation there (I-012). Still unproven after two runs: **everything Windows-specific** — `New-SmbShare`, the coordinator over UNC, and the mandatory-lock check, which is the whole point of that leg — plus the auth-restart step and the self-test on any leg. (4) **D-D′ is the gate on Track B** and needs jok + Kristian, not code.

**Next session start from:** **the CI run at `d4db4de`** (pushed; two earlier runs are already analysed above). Both 1.98 lints are fixed and verified against CI's exact toolchain, so `check` should be green on all three OSes; `e2e` then runs for the first time with the `sed` fix in place. **The one thing still entirely unproven is the Windows leg past clippy** — `New-SmbShare`, the coordinator over UNC, and the mandatory-lock check, which is the only automated evidence for invariant 3 this project would have. If it breaks there, that is real and not a one-line fix.

Then, in order: **jok's two carried-over items** — the ssh-agent decision (recommendation: leave the passphrase human-gated, and treat "jok pushes" as policy rather than a defect) and nothing else outstanding on the blocker now that its diagnosis is corrected above. Then **phase 1's leftovers** in open question (2) — the fault proxy, the kill-mid-write and Office-lock examples, and the `cargo fmt` reformat call. Then the **D-D′ conversation** before anything in Track B gets designed.

---

## Known Issues

> **Live issues only** — full narrative for every issue, live and resolved, is in
> `logbook/ISSUES.md`. Staleness rule: open > 30 days is flagged STALE at session
> start. Threshold: 5000 chars.

| ID | Description | Severity | Since | Status |
|----|-------------|----------|-------|--------|
| [I-002](logbook/ISSUES.md#i-002) | endpoint↔coord channel unauthenticated. | ~~MED~~ LOW | 2026-07-21 | **MOSTLY RESOLVED** 2026-08-21 |
| [I-003](logbook/ISSUES.md#i-003) | MCPB bundle signing non-functional in `@anthropic-ai/mcpb` 2.1.2. | LOW | 2026-07-22 | OPEN |
| [I-004](logbook/ISSUES.md#i-004) | `CLAUDE.md` **Status** drifts from reality, and is auto-loaded before the logbook can correct it. | ~~LOW~~ MED | 2026-08-03 | OPEN (recurring) |
| [I-005](logbook/ISSUES.md#i-005) | **Delivering a large PDF's content to a model is unsolved** — now *refused* rather than silently unanalysable (1.3). | ~~HIGH~~ MED | 2026-08-05 | OPEN (failure mode fixed, capability not) |
| [I-007](logbook/ISSUES.md#i-007) | **`move_cas_core` violates invariant 4** — version-check and mutation are not under one handle. | MED | 2026-08-06 | OPEN |
| [I-009](logbook/ISSUES.md#i-009) | Path aliasing: `normalize` resolves neither `.` nor `..`, breaking invariant 5. | LOW | 2026-08-06 | OPEN |
| [I-011](logbook/ISSUES.md#i-011) | Release + CI workflows had never executed on GitHub. | LOW | 2026-08-19 | **PARTLY RESOLVED** 2026-08-20 |
| [I-013](logbook/ISSUES.md#i-013) | Nothing verifies the docs' numeric claims against the code, so they drift silently. | LOW | 2026-08-21 | OPEN |

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
| E-020 | SQLite backend-registry table + admin API (runtime-mutable routing) | LOW | 1–2 | TODO |
| E-021 | Relax `VersionToken` to backend-opaque (BLAKE3 for synth, ETag for cloud) | LOW | 1 | TODO |
| V3-cloud | Cloud backends S3 → Azure → Graph (conditional-PUT / lease / ETag adapters + cloud-event watchers) | LOW | large | TODO |
| E-024b | Coord dashboard / admin UI | LOW | large | DEFERRED |
| E-022 | Drive-letter → UNC canonicalisation (+ DFS, now droppable) | HIGH | 1 | DONE (one branch unverified) |

---

*Logbook version: 1.0 | Created: 2026-07-21 | Split into child docs: 2026-08-21*
*To reuse: copy this file to a new project, clear live section content, adjust YAML frontmatter.*
