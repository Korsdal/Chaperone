---
logbook:
  project: Chaperone
  type: engineering-logbook
  version: "1.0"
  created: "2026-07-21"
  last_updated: "2026-09-07"
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
| `logbook/decisions/product.md` | 2 decision bodies — product scope, positioning, market boundaries (new 2026-08-21, D-037) |
| `logbook/logs/2026-08.md` | 9 session entries (08-03 … 08-28) |
| `logbook/logs/2026-07.md` | 18 session entries (07-21 … 07-22) |
| `logbook/ISSUES.md` | all 15 issues in full, live and resolved |
| `logbook/BACKLOG.md` | live backlog + Delivered appendix (27 rows) + removed duplicates |
| `logbook/state-history.md` | narrative displaced from Current State, newest first |

---

## Current State

> **VOLATILE** — rewritten (not appended to) at every session end.
> Freshness: 7 days. If `last_updated` in YAML is older, flag as stale.
> Superseded narrative → `logbook/state-history.md`.

**Phase:** implementation — v1 complete, installed at a customer, pilot-tested on
real hardware (2026-08-14), **phase 1 of the 0.2 plan delivered (2026-08-21)**,
**corrected (2026-08-25, I-015)**, and **Phase A "Truth" of the 2808 roadmap
delivered (2026-09-07)**. Multi-backend (SMB + POSIX); Windows, Linux **and
macOS** now all run the test suite in CI.

**Version `0.1.3`** (set by jok 2026-08-25), tagged `v0.1.3`. A *patch* bump again,
correcting phase 1's read guardrail: **it stays 0.1.x until it is tested and true**
— the minor number is a claim about proven-ness, not a changelog of effort.
Versioning is a human responsibility; never fill in a bump.

**Status:** three crates build clean; **384** tests pass (was 381); clippy
`-D warnings` clean. **29** coord routes + the **11-tool** MCP surface (unchanged
— neither phase 1 nor Phase A added tools), plus the six-tab token-gated admin
page. MSRV **1.88.0**. *Every number here measured this session, not carried
over.* `cargo fmt` still drifts (389 files) and remains jok's call.

**Phase A landed 2026-09-07 — what it changed is what the system SAYS, not what
it does.** Serve-versus-refuse and every write outcome are unchanged; seven
statements that were false are not. The one with teeth: `chapr_move` renamed the
file and then, if coord was unreachable, told the agent *"NOTHING WAS CHANGED"* —
now a `CommittedButUnrecorded` naming `dst`, the shape the other verbs already
used. That variant's own message (*"write to {path} committed on disk"*) was
also already false for `delete` and `restore`, which reuse it, and is now
verb-neutral. **Invariant 6 is finally correct in `CLAUDE.md` and the concept
spec** — D-026's own entry had booked that as a debt it could not pay while both
files were gitignored.

**New standing convention (jok, 2026-09-07): three-section changelogs.** *What
was the problem / what was changed / what was deferred*, ≤6 lines each, a table
where rows share a shape — `.github/pull_request_template.md` and `CHANGELOG.md`.
The reasoning is the load-bearing part and it generalises beyond repo files:
walls of text get approved unread, and an approval that did not really happen is
what produces the drift I-004 keeps re-filing. **I-013 proposed detecting that
drift; this attacks its cause.** It now governs how work is handed to jok
generally, not just what goes in the repo.

**Read-path refusals now come in two kinds, and this is the shape to know
(D-039, I-015).** A **container** — PDF, Office, image, archive — is refused by
magic bytes with advice naming what to read instead. **Text in an encoding other
than UTF-8** is *also* refused, but as a different thing entirely: the message
names the encoding, says plainly that nothing is wrong with the file or the drive,
and gives the human remedy, while a `NON_UTF8_TEXT` warning carries the structural
evidence to coord's diagnostics for whoever administers the share. Chaperone
coordinates files; it does not convert encodings or extract text (that is E-028).
The classifier behind the split **chooses a message, never an outcome** — both arms
refuse — and `instructions()` warns agents not to author text as base64 or to
answer a refused read by copying its bytes elsewhere.

**What phase 1 changed, in one line each:**
- **1.3** `chapr_read` refuses binary containers (16 formats, magic bytes, never
  extension) with advice naming what to read instead. Base64 is now **opt-in on
  read** (`allow_binary`) — a behaviour change for any caller that relied on the
  old default. *It also refused every non-UTF-8 **text** file as "an unrecognised
  binary format" — see I-015, fixed 2026-08-25.*
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

**⚠️ The e2e job has run, and what it proved is narrower than "green."** Actions
accepts the YAML; the **macOS** leg reached the auth-restart step, so share setup,
coordinator startup and both smoke suites passed there — the first end-to-end
validation on macOS (I-012). Still unproven after two runs: **everything
Windows-specific** — `New-SmbShare`, the coordinator over UNC, and the
**mandatory-lock check, which is the only automated evidence for invariant 3 this
project would have** — plus the auth-restart step and the self-test on any leg.
Treat 1.4 as *2 of 5 scenarios covered*, and the SMB rig as **written, not
working**: that is **B0** in the 2808 roadmap, and it gates the correctness phase.

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

**What's in flight:** **two commits made, tree clean, waiting for jok to push**
(`ahead 2`). Phase A's code and templates in one; this session's logbook writes —
including the 08-28 entry that had sat uncommitted for ten days — in the other.
`CLAUDE.md` and the concept spec are gitignored and ride neither, so **invariant
6's correction lives only on this laptop**, which is the same shape of debt D-026
recorded in the first place. `0.1.3` still stands — **no version change**, because
Phase A proves nothing new. Also in flight, unchanged: the 21 open questions in
`specs/chaperone-roadmap-2808.md` §6, four of which gate work.

**Repo-state finding, worth knowing once and then forgetting (2026-09-07).** The
branch was `ahead 3, behind 1` because the 08-25 commit existed **twice**:
`655c2f6` was pushed at 16:19, then amended locally at 16:24 as `67f613b` and
never pushed, so the local and public 08-25 commits were divergent twins with the
same message and parent. The only content difference was four lines of this file
— the 08-25 entry's "Open questions" — and all of it had already been carried
into `logbook/logs/2026-08.md` by the 08-28 session's move. Resolved by rebasing
this session's two commits onto `origin/main` and dropping the twin; the rebase
conflicted in exactly that region, which is itself the evidence it was the right
call. Nothing lost, no force-push, `67f613b` still in the reflog. **The lesson
that generalises: an amend after a push does not announce itself** — `git status`
said "ahead", not "diverged", and only a hash comparison showed why.

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

**Direction, settled 2026-08-28 (jok) — read this before the list below.**
Chaperone is a **coordination primitive, not an extraction tool**: extraction
belongs to plugins, and the collaborator's tender pipeline already does it
(confirms D-039, holds D-028's line). Priority order is **correctness core →
accountability → deployment**, deployment having already taken its large step in
D-032. **Generalise now** — several customers, not this one. **Make the audit claim
true** rather than lowering it to fit. And the design audience is **the agent, and
the person whose files it writes**: Apache-2.0 means nobody signs an invoice, so
the audit trail is not a sales artefact — it is what makes it reasonable to let
agents write to a shared drive at all. Full reasoning and the 21 open questions in
`specs/chaperone-roadmap-2808.md` (local), which **supersedes**
`specs/chaperone-roadmap-0.2-to-0.5.md`.

**Consequence:** the old roadmap's largest remaining investment, **Track B (the
document gap)**, is off the path, and **D-D′ resolves to "the line holds"** — both
to be closed *with reasoning*, not left looking planned.

**What's next:**
1. **B0 — read the Windows e2e leg's status, then get it green.** Promoted to
   first because it is now the only thing blocking the correctness phase, it is
   the sole automated evidence for invariant 3 this project would ever have, and
   **it cannot be advanced from this laptop** (no `gh`; the key is passphrase-
   protected by design). See the CI warning above.
2. **Answer the four gating questions** in 2808 §6 — **Q1/Q2** (Track B and D-D′
   disposition), **Q6** (where a per-conversation session id comes from; Phase C's
   value depends on it), **Q4** (chain immutability versus erasure), **Q12** (does a
   GitHub-runner SMB share clear I-007's bar, or does it need the customer's server).
3. ~~**Phase A, "Truth"**~~ — **DONE 2026-09-07.** All seven items, plus two
   the roadmap's list got wrong: `server.rs:2278`'s 512 KiB was already accurate
   (false positive), and the Q17 fix **overrode a deliberate 08-25 choice** to keep
   "worth their attention" for genuinely-binary bytes. The superseded argument is
   preserved in the test rather than deleted — jok may want that as a decision.
4. **Phase B, verb parity** — I-007 (the method is settled: `DELETE` in
   `winfs::open_existing`'s mask + `SetFileInformationByHandle(FileRenameInfo)`,
   per D-027), journalling the move window so D-013's "stale-but-recoverable" is
   true, auditing the overwrite discard, restore spec-versus-code (Q13 — Phase A
   deliberately did **not** pre-empt it; the code now describes itself accurately
   and still disagrees with concept §6.5), tests for `move_cas_core` (it has
   none), and 1.4's three uncovered scenarios.
5. **Phase C, accountability** — session identity, audit write-path authorisation,
   migration machinery, chain + verifier, retention, privacy doc.
6. **Phase D, deployment** — 2.2 supply chain, 2.3 decision records (now cheap),
   2.1 purge, E-022's mapped-drive branch via Kristian, I-011's first real tag.
7. **Decision entries to write** — the direction itself, migrations (no record
   exists either way), the audit-integrity posture (scoping D-024), Track B's
   disposition, and now possibly the Q17 override. Plus §6 Q20's housekeeping:
   duplicate `D-038` row, `D-009` never issued, missing `Status` trailers (D-001,
   D-032), and the STALE rule — **as of 2026-09-07 five genuinely OPEN issues
   breach it** (I-003, I-004, I-005, I-007, I-009, at 32–47 days), eight if the
   partly-resolved rows count. E-022 still holds a live `HIGH` row while marked
   DONE.

Deferred engineering (E-015, E-020, E-021, E-022, E-024b, E-028, V3-cloud) →
`logbook/BACKLOG.md`. Note **E-021 ↔ V3-cloud is circular as written** and E-022 is
missing from this line's predecessor while holding a live `HIGH` row.

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
| [D-039](logbook/decisions/product.md#d-039) | Chaperone coordinates; it does not extract or transcode. That is an add-on (E-028), not a missing feature | 2026-08-25 | product | CURRENT |
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
> the new one here. Threshold: 10000 chars; index + one entry is **6955** today
> (measured 2026-09-07, was 9741), so there is real headroom again — the drop is
> the entry being written to the three-section budget rather than anything being
> cut. A session entry much over ~9 KB still breaches the section on its own.
> Headroom is thin by design: the entry here is replaced rather than appended to,
> so the section does not grow between sessions.

| Month | Entries |
|---|---|
| `logbook/logs/2026-08.md` | 9 — 2026-08-28, 08-25, 08-21b, 08-21, 08-19/20, 08-14, 08-06, 08-05, 08-03 |
| `logbook/logs/2026-07.md` | 18 — 2026-07-22 (a–c), 2026-07-21 (base, b–o) |

### Session 2026-09-07 — jok / Claude
**Type:** engineering (Phase A, "Truth") + process
**Focus:** discovery after a 10-day break, then Phase A — everything the code said about itself that was false. One new convention came out of it, and it was jok's rather than the plan's.

**Worked on:**
- [x] **Discovery against the docs, re-measured rather than read.** Version `0.1.3`, 11 tools, 29 routes and 381 tests all hold — the first session in a while where `CLAUDE.md` §Status was accurate on every checkable number. The two holes were in the *record*, not the code: the 08-28 entry was never committed, and the 2026-09-02 session (a talk-prep history hunt, `specs/hunt-spec-encoding-thread*.md`) has no entry at all. **jok's calls:** 09-02 stays unlogged, and Q20 (track `specs/`) stays open.
- [x] **A1, the one that mattered.** `chapr_move` renames and then calls `move_paths`; a failure there propagated raw, so an unreachable coordinator rendered *"NOTHING WAS CHANGED … Chaperone deliberately refuses writes"* **after a rename that had succeeded**. It now takes `CommittedButUnrecorded` naming `dst` — the shape `create`, `delete`, `restore` and `write` already used for their tails. No proto change. D-013's ordering is untouched: it accepted a stale coordinator, never a misleading answer.
- [x] **The variant's own message was wrong too, and more widely than booked.** `"write to {path} committed on disk"` was already false for `delete` and `restore`, which reuse it. Now verb-neutral, and the tool-guidance arm with it (`"the file WAS written / Do NOT write it again"` named the wrong action for three of the four verbs).
- [x] **A2–A5.** Fail-closed documented where it actually starts (`assert_read`), not two coord calls later at the journal; restore's *"cannot clobber a concurrent writer"* replaced with what it really guarantees, in the code **and** in the tool description a model reads; move's description no longer implies the audit trail follows a rename; the Q17 escalation removed; the stale 512 KiB made historical. `CLAUDE.md`'s SSPI/Kerberos claim deleted, and **invariant 6 corrected for D-026** in both `CLAUDE.md` and the concept spec — a debt D-026's own entry had recorded as unpayable while those files were gitignored.
- [x] **A6, better than planned.** All three 08-28 probes became **unit tests**, not the two smoke examples the plan assumed: the `wiremock` + real-POSIX-backend harness already in `server::tests` covers coord-down writes and concurrent creates, so they run on **every** CI leg instead of only e2e. The coord-down test was **mutation-checked** — pointed at a live coordinator it went red on the right assertion, then reverted. Until today the fail-closed claim had no coord-unreachable write test of any arity.
- [x] **A7 — jok's addition, and the most reusable thing here.** Three-section changelogs: **what was the problem / what was changed / what was deferred**, ≤6 lines each, table where rows share a shape. New `.github/pull_request_template.md` and `CHANGELOG.md` (seeded with 0.1.3 as the worked example; Phase A sits under `Unreleased`). jok's reasoning, which is the part to keep: *"it is there to solve the human meatsleeve approving — where you draft up walls-of-text, and I approve without reading and checking."* An approval that did not really happen is what produces the drift I-004 keeps re-filing; **I-013 proposed detecting that drift, this attacks its cause.** Now also a standing rule for how work is handed to jok, not only for repo files.

**Two corrections to the roadmap's own Phase A list.** The third stale-512-KiB site (`server.rs:2278`) was already past-tense and accurate — a false positive, only a tense disagreement fixed. And the Q17 fix **overrode a deliberate 2026-08-25 choice**: a test asserted that *"worth their attention"* should **stay** for genuinely-binary bytes, on the argument that it is only wrong about a text file in a code page. True on the severity axis I-015 addressed; superseded on the content axis, since reaching that arm means only that no magic number matched. The roadmap wins on recency and on being explicitly booked, but the old argument is preserved in the test rather than deleted — **worth jok's eye, and possibly a decision entry.**

**Verified:** 381 → **384** tests, 0 failed; clippy `-D warnings` clean; BLAKE3 measured at **3406 MiB/s** (a 50 MB tender ≈ 15 ms), now a reported number rather than a comment asserting one. `cargo fmt` still drifts (389 files) and is still jok's call.

**State changes:** **no version change** — `0.1.3` stands, and Phase A proves nothing new. `CLAUDE.md` §Status test count 381 → 384, provenance line re-dated to today. `logbook/ISSUES.md:162` corrected **378 → 381**, the drift the 09-02 hunt found and did not reconcile. Three more stale counts fixed in `LOGBOOK.md`'s own Child Documents table (month entries 7→9, issues 14→15, delivered rows 26→27) — all measured, and all the same I-004 pattern found while writing this entry. Nothing written to the Decision Log or Backlog.

**Open questions:** unchanged — the four gating calls in `specs/chaperone-roadmap-2808.md` §6 (**Q1/Q2**, **Q6**, **Q4**, **Q12**), plus whether the Q17 override earns a decision entry. **B0's status is still unknown from this laptop:** no `gh` installed, and the SSH key is passphrase-protected by design, so whether the Windows e2e leg has gone green needs jok.

**Next session start from:** **B0 — read the Windows e2e leg's status on GitHub Actions.** It gates the whole correctness phase, it is the only automated evidence for invariant 3 this project would have, and it is the one item nobody can advance from this machine. **Green →** Phase B opens with **I-007** (`SetFileInformationByHandle(FileRenameInfo)` plus `DELETE` in `winfs::open_existing`'s access mask, the method D-027 already settled empirically) and tests for `move_cas_core`, which still has none. **Red →** fixing it *is* the next session. Either way the four §6 questions are still owed, and **Q6 before any Phase C machinery**: if a per-conversation session id cannot be obtained, C3–C5 chain a record that still cannot attribute. Carried forward, still unanswered: **does the extraction pipeline write explicit UTF-8?**
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
| E-022 | Drive-letter → UNC canonicalisation (+ DFS, now droppable) | HIGH | 1 | DONE (one branch unverified) |

---

*Logbook version: 1.0 | Created: 2026-07-21 | Split into child docs: 2026-08-21*
*To reuse: copy this file to a new project, clear live section content, adjust YAML frontmatter.*
