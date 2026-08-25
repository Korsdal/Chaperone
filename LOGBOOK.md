---
logbook:
  project: Chaperone
  type: engineering-logbook
  version: "1.0"
  created: "2026-07-21"
  last_updated: "2026-08-25"
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
| `logbook/logs/2026-08.md` | 7 session entries (08-03 … 08-21b) |
| `logbook/logs/2026-07.md` | 18 session entries (07-21 … 07-22) |
| `logbook/ISSUES.md` | all 14 issues in full, live and resolved |
| `logbook/BACKLOG.md` | live backlog + Delivered appendix (26 rows) + removed duplicates |
| `logbook/state-history.md` | narrative displaced from Current State, newest first |

---

## Current State

> **VOLATILE** — rewritten (not appended to) at every session end.
> Freshness: 7 days. If `last_updated` in YAML is older, flag as stale.
> Superseded narrative → `logbook/state-history.md`.

**Phase:** implementation — v1 complete, installed at a customer, pilot-tested on
real hardware (2026-08-14), **phase 1 of the 0.2 plan delivered (2026-08-21)** and
**corrected (2026-08-25, I-015)**. Multi-backend (SMB + POSIX); Windows, Linux
**and macOS** now all run the test suite in CI.

**Version `0.1.3`** (set by jok 2026-08-25), tagged `v0.1.3`. A *patch* bump again,
correcting phase 1's read guardrail: **it stays 0.1.x until it is tested and true**
— the minor number is a claim about proven-ness, not a changelog of effort.
Versioning is a human responsibility; never fill in a bump.

**Status:** three crates build clean; **381** tests pass (was 364); clippy
`-D warnings` clean. **29** coord routes + the **11-tool** MCP surface (unchanged —
phase 1 added no tools), plus the six-tab token-gated admin page. MSRV **1.88.0**.
*Every number here measured this session, not carried over.*

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

Deferred engineering (E-015, E-020, E-021, E-024b, E-028, V3-cloud) → `logbook/BACKLOG.md`.

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
> the new one here. Threshold: 10000 chars; index + one entry is 8898 today, so a
> session entry over ~8 KB breaches it on its own — compress or split then.

| Month | Entries |
|---|---|
| `logbook/logs/2026-08.md` | 7 — 2026-08-21b, 08-21, 08-19/20, 08-14, 08-06, 08-05, 08-03 |
| `logbook/logs/2026-07.md` | 18 — 2026-07-22 (a–c), 2026-07-21 (base, b–o) |

### Session 2026-08-25 — jok / Claude
**Type:** engineering (bug hunt → correction → boundary decision)
**Focus:** a suspected regression in the read path, brought as a written brief with an explicit instruction to verify before fixing. It confirmed, but the fix that followed was not the one the brief was costing — jok's reframing turned it from a capability question into a product boundary, and that is the session's real output.

**Worked on:**
- [x] **Verified the brief's claim, and refuted half of it.** `binary_guard`'s second clause treated "not valid UTF-8" as "not text", so every CP1252 or UTF-16 file was refused as "an unrecognised binary format … worth their attention". Reproduced against the real function with five fixtures; prior behaviour at `abefc61` confirmed by reading the tree (`render_envelope` byte-identical, no guard, so base64 with `encoding=base64`). **But the regression is narrower than it looked:** base64 Danish prose was never analysable either, so what was lost is *default round-trippability* and *accurate diagnosis*, not readability. The refusal **policy** was always defensible. Findings in `specs/bug-hunt-2508-findings.md` (local).
- [x] **jok's reframing, which changed the work:** *"It is not a file extraction service, it is a coordination service — and the coordination part works. The extraction of useful text is not Chaperone's job… it should not give the message that something is wrong with the service itself. The issue is the files."* Recorded as **D-039**, which also chose I-005's long-standing third option (declare it out of scope and coordinate the derived artifacts) and generalised it beyond PDFs. Capability booked as **E-028**, a separate deployable.
- [x] **Fixed message and diagnosis, deliberately not capability.** `sniff::classify_unrecognised` (BOMs, NUL parity for endianness, high-byte ratio — `std` only, no dependency) splits unrecognised bytes into text-in-another-encoding versus binary, and **chooses a message, never an outcome**: both arms still refuse, which is what keeps its thresholds harmless. `RefusalKind` replaces `container: Option<_>`; the text class gets its own frame that never says "binary", separates Chaperone's health from the file's state in the first sentence, states the boundary in-band, and gives the human a remedy. All five classes gained the boundary sentence.
- [x] **The diagnosis now reaches the people who can act on it.** A `NON_UTF8_TEXT` warning carries the structural evidence through `diag.rs` — which was built for exactly this and which the refusal path had **never reached**, since it returns before `tool_failure`. Coord groups by `(code, path)` and the remedy points at the *producing step*, so a folder of legacy files reads as one fix. Container refusals file nothing: a PDF on a share is a designed outcome, the same line `classify` already draws. Facts are structural only — no content excerpt, so §13.2's existence leak is not widened.
- [x] **jok's follow-up question found the hole the fix had left**, and it was the sharpest moment of the session: *if extraction is agentic and writes through Chaperone, does Chaperone create files it then refuses to read?* Answer, measured: **no through `utf8`** (`String::into_bytes()` is valid UTF-8 by construction — an agent authoring text has no encoding to get wrong), **yes through `base64`** (nothing guards the write path). The reachable sequence is partly our own making: a read is refused → the refusal names `allow_binary` for copying → an agent told "produce a mirror" reads "copy" as its job → the same unreadable encoding lands somewhere new.
- [x] **Mitigated with guidance, on jok's call** — the rule now sits in `server::instructions()` beside D-028's write-routing rule, naming the specific loop, plus one clause on `ContentEncoding::Base64`'s schema doc. jok's reasoning: *"If the model understands it, they can avoid it, and better, they can explain to the user what went wrong."* A symmetric write guard was **rejected**: it would refuse byte-exact copying, which is `allow_binary`'s one legitimate use. D-039 amended to record all of this against itself.
- [x] **Two invariants pinned that were load-bearing and unasserted.** `any_utf8_write_can_be_read_back_as_text` (five hostile cases) and the companion test recording the base64 hole rather than hiding it. The first was **mutation-checked** — `decode_content`'s `utf8` arm temporarily made to emit `0xE6`, test went red naming offset 36, reverted and diffed byte-identical. A green test that cannot fail pins nothing.

**Verified:** `cargo test --workspace` **381 passing** (was 364), 0 failed; `cargo clippy --workspace --all-targets -- -D warnings` clean. Serve-versus-refuse is byte-for-byte unchanged — the three pre-existing round-trip tests pass untouched, and `classifying_the_bytes_never_serves_them` pins that no read that worked before behaves differently. Every throwaway probe reverted and `git status` confirmed clean between passes.

**State changes:** version `0.1.2` → **`0.1.3`** (jok, this session); tests 364 → 381; `sniff.rs` gains the classifier, `server.rs` gains `RefusalKind` + `report_encoding_finding` + the encoding paragraph in `instructions()`, `diag.rs` gains `record()` for findings that are not `ChaprError`s. New: **D-039**, **I-015**, **E-028**. `docs/architecture.md` updated for both refusal classes.

**Two things found and deliberately not fixed:** a UTF-8 file *with* a BOM is served as text with `U+FEFF` inside the envelope body and nothing handles BOMs in either direction (I-015 residual 2); and the decision index in `LOGBOOK.md` carries **two identical D-038 rows** — pre-existing, left alone rather than silently edited.

**Next session start from:** **a code review of the solution as it now stands** — phase 1's edits plus this session's safeguard, reviewed as one body of work rather than as two changes. Worth pointing it at the read path specifically: `sniff.rs`'s thresholds have never been exercised against anything but chosen fixtures, `binary_guard`'s new shape is one session old, and the `NON_UTF8_TEXT` diagnostic's admin-page rendering is still **unexercised** — steps 4 and 5 of that plan's verification need the real deployment and could not be run here. **Then phase 2 of the 0.2→0.5 roadmap** (`specs/chaperone-roadmap-0.2-to-0.5.md`, local). Carry into it: **the question for Kristian** — does the extraction pipeline write explicit UTF-8? Python's `open(p,'w')` with no `encoding=` resolves to cp1252 on a Danish-locale box, and if any mirror is written that way then every mirror with a Danish character in it is refused, which is the pilot's main workload failing on its main content. That answer sets I-015's real severity and nothing in code can settle it.
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
