# Decision Log — Project & Process

> Child document of `LOGBOOK.md` (logbook protocol v1.0). **Most recent first.**
> Scope: naming, licensing, repo + collaboration posture, agent/plugin behaviour.
>
> The index of *all* decisions — every ID, title, date, theme and status — lives in
> `LOGBOOK.md` under `## Decision Log`. This file holds the bodies only.
> New entries are appended here by `/logbook decide` per the theme rule in
> `LOGBOOK.md`'s YAML (`sections.decision_log.themes`).

---

<a id="d-045"></a>
### D-045 — Three test environments with separate jobs, and a self-test whose exit code stops lying — 2026-09-08

**Trigger:** **Q12** — *"is a `New-SmbShare` on a GitHub runner enough to unblock I-007, or does it
wait for the customer's Windows Server 2022?"* — plus jok's own framing that the absence of a real
test environment *"will keep blocking development of Chaperone."*

**Problem:** every correctness question this project has asked for two months has ended in "unproven
against a real server." B0 was gated on it, I-007 was gated on B0, E-022 had carried a live `HIGH`
row since August for one branch nobody could exercise, and three of B7's scenarios have no home at
all. A per-question workaround was never going to clear that; the environment was the missing thing.

**Decision (1) — Q12: the CI leg clears it. Phase B proceeds.** The evidence, gathered this session:
CI's share is a genuine `New-SmbShare` reached over UNC (`\\COMPUTERNAME\chaprci`), i.e. through the
SMB **server driver**, not a local NTFS path; and `mandatory_lock_check` is `#[cfg(windows)]`, so on
that leg it compiles in and can only PASS or FAIL, never skip. The step exited 0, so it passed. With
the 2026-08-14 pilot separately measuring real Windows Server 2022, the honest reading is **the pilot
is the evidence and CI is the regression guard**. I-007's method was in any case already settled
empirically by D-027, so it never rested on CI alone.

**Decision (2) — three environments, three jobs, none replacing another.**

| Environment | Covers | Cannot cover |
|---|---|---|
| **CI** (GitHub runner, loopback share) | every push, three OSes, regression | remote server, real latency, a Kerberos realm |
| **Local Hyper-V rig** (`CHAPR-FS`, Server 2022) | rapid development, install process, **file integrity**, kill-mid-write, mapped drives | identity — it is a workgroup with **no realm** |
| **Azure VM fileserver** (demo tenant) | the **auth path** — Kerberos / Negotiate / OIDC — and **timing under real network latency** | nothing yet; it is the most faithful of the three |

The rig's isolation is deliberate and structural: an **Internal** Hyper-V switch with no physical NIC
bound and **no default gateway on either side**, on `192.168.221.0/24` (chosen not to collide with
the corporate `172.16.43.0/24` or Hyper-V's NAT `172.19.176.0/20`). The share lives on **`D:`** and
the coordinator's database, blobs and audit trail on **`C:`**, mirroring the customer pilot. That
separation is not cosmetic: it converts **I-009**'s mitigation from behavioural (*"agents do not
generate aliased paths"*) into structural, because there is no relative traversal from `D:\` to
`C:\` for `..\..\chapr-coord\coord.db` to exploit.

**Decision (3) — B8: an UNVERIFIED check exits non-zero.** `Report::finish()` returned `failed` only,
so a run in which almost nothing executed still exited 0. The module **printed** D-032's
SKIP-is-not-PASS rule and then returned an exit code contradicting it — and an exit code is the only
part of that output a script reads. `Outcome::Skip` therefore splits in two:

- **`Unverified`** — applies here, did not run (no `CHAPR_ROOT`, folder not on a mapped drive).
  **Counts toward the exit code.** An operator must not read *"we did not look"* as *"it is fine."*
- **`NotApplicable`** — cannot apply to this backend or platform (the mandatory-lock probe against
  advisory POSIX locks; drive letters on a platform that has none). **Does not count**, because it is
  a correct outcome rather than a gap.

Collapsing the two would either fail every POSIX deployment for a meaningless check, or let a real
Windows gap pass silently. **Demonstrated the same session:** identical infrastructure, one
environment variable different — `CHAPR_SELFTEST_DIR` as UNC gave `8 passed, 1 unverified` → **exit
1**; as `Z:\` gave `9 passed` → **exit 0**. Before B8 both were 0.

**Consequence, accepted deliberately:** B8 would have turned CI's Windows e2e leg red, because it
points the self-test at a UNC path. **Fixing CI rather than softening B8** — `net use Z:` and point
`CHAPR_SELFTEST_DIR` there — was jok's call, and it makes CI *verify* E-022's branch on every run
instead of skipping it. That change is committed but **proves out only on the next push**; the POSIX
legs' `N/A` classification was cross-checked in WSL (clippy clean, 172 tests) rather than assumed.

**Rejected:** (a) **a Docker/WSL Samba rig** — faster to iterate, but Samba is not Windows Server and
could not settle mandatory-lock questions, making it a functional-loop rig rather than an invariant-3
one; (b) **waiting for the customer's server** for I-007 — it would have blocked Phase B on an
environment nobody controls, for a method D-027 had already proven; (c) **an escape hatch flag on
B8** (`--allow-unverified`) — it reintroduces exactly the loophole being closed, and the self-test is
what a *customer* runs.

**Made by:** jok (Q12, the three-environment split, fix-CI-not-B8, the `C:`/`D:` separation) / Claude
(the CI-run evidence, the Unverified/NotApplicable split, the isolation design) | **Review date:** N/A
**Status:** CURRENT

---

<a id="d-038"></a>
### D-038 — Project memory is published: `LOGBOOK.md` and `logbook/` become tracked, with customer identifiers scrubbed — 2026-08-21

**Trigger:** jok, on being told the roadmap review and the new plan were sitting untracked in `docs/` and carried customer-identifying strings: *"Scrub the customer name, keep the generic documentation for it. Keep Kristian (he is named in license notice as contributor). Add the logs to the repo — they belong there for open source → open project."*

**Problem:** D-033 untracked `LOGBOOK.md` and `CLAUDE.md` when the project went Apache-2.0, and purged them from history at a force-push cost recorded as unrepeatable. The stated reason was that ~200 KB of candid internal reasoning is written for an internal audience, and names a customer deployable, a collaborator's plugin internals and share paths. The 2026-08-21 split inherited that rule for the whole `logbook/` directory.

That left two problems pulling in opposite directions. **The memory was single-laptop and irreplaceable** — flagged on 2026-08-14 and made worse by the split, which spread it across nine local-only files instead of one. And an *open* project whose reasoning is invisible is only half open: the decision log is the answer to almost every "why is it like this" a contributor could ask, and 2.3 existed purely to re-author a subset of it for the public.

**Chosen (jok): publish it, having removed what actually needed removing.**

- `LOGBOOK.md` and `logbook/` are **tracked**. The `.gitignore` block that argued for their exclusion is kept as history with the reversal recorded in place, rather than deleted — the argument was sound for a private-repo premise that no longer holds.
- **The scrub is what makes this safe rather than merely decided.** The customer deployable, the collaborator's tender-pipeline plugin, its three Python scripts and its Danish folder names are now described **by role** — "the extraction script", "the extracted-text mirror", "`<customer-repo>/coord`". 26 occurrences across 8 files, plus 7 in the two roadmap docs. **The engineering reasoning never depended on the names**, which is the test that made this a scrub rather than a rewrite: D-028 reads identically without them.
- **Named people stay.** Johannes Korsdal and Kristian Schou are this project's own contributors, listed in `NOTICE` under the licence. Anonymising a contributor in his own project's memory would be theatre.
- `CLAUDE.md` **stays untracked**. It is agent instructions, not project memory, and nobody asked for it. A separate call if it comes up.

**Deliberately published anyway, and worth being explicit about:** open issue narratives with exact `file:line` locations, **including unfixed ones** (I-007's invariant-4 violation, I-009's path aliasing). That is consistent with `docs/security.md`, which already says there is no embargo process because pretending otherwise would be theatre for a project this size. Anyone reading those issues learns what a determined reader of the source would learn anyway, and gains the maintainers' own assessment of severity — which is a feature of an open project, not a leak.

**Consequences:**
1. **The single-laptop risk is retired.** Project memory now has the same durability as the code.
2. **Item 2.3 shrinks substantially.** It was "extract and re-author 37 cited IDs for a public audience"; it becomes a cross-referencing job, because the bodies are now in the repo.
3. **Future entries must be written for this audience.** Candid about engineering, never about a customer's internals. That is a standing constraint on every `/logbook` write from here, and the reason it is recorded as a decision rather than a chore.
4. **D-033 is narrowed, not invalidated** — its licensing and repo-split halves stand untouched; only its treatment of the logbook is reversed.

**Rejected:** (a) **keep it private and finish 2.3 as originally scoped** — pays to write a public subset while the real reasoning stays on one laptop, and the two would drift; (b) **publish unscrubbed** — the names add nothing a reader needs and are not the project's to publish; (c) **a private second repo for the logbook** — the 2026-08-14 answer, still unimplemented five weeks later, which is its own evidence about how well that works.

**Made by:** jok (the call, and the scrub/keep line — customer internals out, contributors in) / Claude (the scrub, and the disclosure inventory behind it) | **Review date:** N/A
**Status:** CURRENT


<a id="d-036"></a>
### D-036 — Project memory splits by theme under `logbook/`; the root file becomes an index — 2026-08-21

**Problem.** `LOGBOOK.md` had reached 233,764 chars. Every section governed by a `threshold_chars` value in its own YAML was over it — Decision Log by 12× (96,442/8,000), Session Log by 8× (81,564/10,000), Backlog 3.7×, Known Issues 2.6×. The breach was not news: the Backlog section carried an inline note dated 2026-08-05 recording that it was over threshold "and had not been flagged before", and that archiving "needs a human call, the same standing question as Decision Log (44 KB) and Session Log (51 KB)". Those two figures had since doubled. The protocol's `NEVER auto-split` rule meant the file could only grow until a human said otherwise, and the cost was paid at every `/logbook start`: a cold read of the whole file, in which the most-read section — Current State, marked VOLATILE — was 20,539 chars of narrative appended session after session instead of rewritten.

**Options considered on four separate axes, each settled by jok.**

**(1) Decision-log split axis.** A: **by year**, the protocol's own `decisions/YYYY.md` pattern — mechanical, one fixed append target for `/logbook decide`, but all 34 entries fall inside five weeks (2026-07-21 → 08-19), so `decisions/2026.md` would hold everything and discriminate nothing. B: **by theme** — three files, better for lookup ("why SQLite" does not mean opening 96 KB), at the cost that a single `child_doc_pattern` field cannot express it and each new entry needs a theme judgment. C: **by status** — 33 of 34 are CURRENT, so it splits almost nothing. **Chosen: B, by theme.** The friction it introduces is answered by writing the rule down (see below) rather than leaving it to improvisation.

**(2) Where child docs live.** A: the protocol's root-relative paths (`decisions/`, `logs/`, `ISSUES.md`, `BACKLOG.md`). B: **one `logbook/` directory**. **Chosen: B**, and the reason is not tidiness — it is D-033. `LOGBOOK.md` is gitignored because the repo went public under Apache-2.0, and it was purged from history at a force-push cost recorded as one that "cannot be repeated cheaply". Every child doc inherits that property; the decision bodies alone are 96 KB of the same candid internal reasoning, naming the customer deployable and share paths. Option A needs four ignore rules and a fifth for every child doc added later — each one a chance for nobody to remember. One directory needs one rule, and a file added later is covered by construction. Secondary: `ISSUES.md` and `BACKLOG.md` at the root of a *public* Rust repo read as public project docs, which is precisely the wrong signal.

**(3) What stays in the root file.** **Chosen: an index, not a container.** One row per decision (ID, title, date, theme, status, anchor link), per live issue, and per live backlog item. This is what does the actual shrinking — 96 KB of decision bodies becomes a 6.2 KB table — and it bounds future growth to one line per item, so the thresholds hold instead of being re-breached quietly. Explicit `<a id="d-nnn"></a>` anchors were added to the child docs because a markdown renderer generates a full-title slug, so bare `#d-035` links would have been broken on arrival.

**(4) Newest session entry.** A: **stays in the root** — protocol start-step 6 ("read `Next session start from:` in the most recent Session Log entry") works with no extra file open, at the cost of moving the outgoing entry into its month file at every `/logbook end`. B: all entries in `logs/`, root holds a pointer — no per-session churn, but the start ritual changes. **Chosen: A.** The cold-start read is the whole point of the exercise; one cut-and-paste per session is the cheaper side of the trade. The instruction is recorded in the section header itself and in `CLAUDE.md`, not left to memory.

**Current State was rewritten, not archived (20,539 → 3,918).** It is marked VOLATILE and is *meant* to be rewritten; it had instead been appended to for a month. Every displaced paragraph moved **verbatim** into `logbook/state-history.md` under the date it was written, so nothing was destroyed — only relocated out of the read path. What was kept is what a resuming session actually needs: version, test counts, the closed pilot gate, the local-only warning, the write-path behaviour notes, the git-auth blocker, and six next actions.

**The vendored protocol file was deliberately not edited.** `.claude/commands/logbook.md` comes from the co-creator's AgentHarness — the same system the end-user plugin uses — so editing it would fork shared tooling for a project-local choice. The theme rule therefore lives in `LOGBOOK.md`'s own YAML (`sections.decision_log.themes`, plus `child_doc_pattern: "logbook/decisions/{theme}.md"` and `keep_newest_in_root: 1`), which is the config surface the protocol already reads. The four `child_doc_pattern` values now deviate from the protocol defaults; that deviation is the point of this entry.

**Integrity, because a split is exactly where project memory gets silently truncated.** All 34 decision bodies, 22 moved session entries and 33 backlog rows verified **byte-identical** by `cmp` against a pre-change backup; all 12 issue narratives byte-identical to their source table cells, including a raw `\|` inside a code span in I-007 that a naive table parse would have eaten. LF endings and UTF-8 preserved (checked deliberately — the 2026-08-19/20 session records `perl -0pi` double-encoding a whole file). Known Issues changed container, not content: a table whose cells had reached 2.4 KB of prose became one `### I-NNN` section per issue.

**Two pre-existing defects found and left for a human, not silently fixed.** (1) **D-001 and D-032 have no `**Status:**` line at all** — indexed as `CURRENT ᵃ` by inspection, with a footnote saying so. (2) **I-004 contradicts itself**: Current State recorded it closed on 2026-08-14 and `CLAUDE.md` does now read as it should, but the issue row still says OPEN. The row was preserved as written and flagged (footnote ᵇ) rather than closing an issue on an agent's own authority. Both are `/logbook audit` work. A third, milder one *was* actioned: a duplicate `E-007` row (one TODO, one DONE, same scope) was dropped from the live table and preserved verbatim under **Removed duplicates** in `BACKLOG.md`, per the rule against destroying entries.

**Result.** `LOGBOOK.md` 233,764 → 27,282 chars (88% smaller), eight child docs under `logbook/`, and every threshold-governed section under its limit: Decision Log 6,220/8,000 · Session Log 8,988/10,000 · Known Issues 1,880/5,000 · Backlog 1,169/5,000. **Two headroom facts worth carrying forward:** the decision index fits ~15 more rows before it needs its own call, and the Session Log at 8,988 means a single session entry over ~8 KB breaches the threshold on its own.

**Made by:** jok (all four axes) / Claude (execution + verification) | **Review date:** N/A
**Status:** CURRENT

<a id="d-033"></a>
### D-033 — Chaperone is Apache-2.0; attribution rides in NOTICE and in every file — 2026-08-19

**Problem.** The workspace carried `license = "UNLICENSED"` and a README section reading "Proprietary". That was the default from day one, never a decision — and it is now wrong in two directions: it does not describe what jok intends (open source), and it says nothing about the fact that Chaperone is the work of **two companies** — SerenIT (Johannes Korsdal) and Prompted (Kristian Schou).

**Why open at all — the owners' argument, recorded because it is the part that will not be re-derivable later.** Two reasons, both strategic rather than legal: *"don't give larger companies a reason to just build the same tool"* — a closed niche tool invites a well-resourced vendor to reimplement it and out-distribute us, whereas an open one makes reimplementation the more expensive path — and *"if it works, everyone should be able to grab it."* Note what this implies about where the business is: not in withholding the source, but in the deployment, the fileserver knowledge, and the support around it. A future proposal to close it back up has to argue against both of those.

**Authority.** Chaperone is dual-owned, so a relicense needs both owners, and it has them: **Morten Hallberg (SerenIT ApS)** and **Kristian Schou (Prompted EV)** agreed to the Apache-2.0 release. This is also not retroactive on the build already installed at the pilot customer — that copy was delivered under the terms in force at the time; Apache-2.0 governs this and every subsequent release.

**Options.** **A: stay proprietary** — no change, but the licence line keeps saying something nobody decided. **B: MIT/BSD** — shortest, most permissive, zero ceremony. **C: Apache-2.0** — permissive with an explicit patent grant, an attribution obligation (NOTICE), and a stated contribution term.

**Chosen: C, Apache-2.0.** MIT would be fine for a library someone vendors. Chaperone is not that: it ships as **binaries into other companies' networks**, where it holds an exclusive handle on their files. Two clauses earn their weight in that setting — §3's explicit patent grant (a permissive licence that is silent on patents is a question an acquiring company's legal team will ask, and MIT has no answer), and §4(d)'s NOTICE requirement, which keeps two-company attribution attached to a redistributed build instead of leaving it in a repo the recipient never sees. §5 also settles inbound contributions as same-terms with no CLA, which is the right amount of process for a two-person project.

**Shape of the change.**
- `LICENSE` — the canonical Apache-2.0 text, verbatim (`md5 3b83ef96…`, the well-known checksum for that file; fetched, not retyped).
- `NOTICE` — two copyright lines, one per owning company (`SerenIT ApS`, `Prompted EV`), then the two contributors (Johannes Korsdal, Kristian Schou). **Revised 2026-08-20 (jok): the owners who authorised the relicense are deliberately NOT named in NOTICE.** The reasoning, which is sound and worth keeping: the copyright holder is the *entity*, not the person, so naming the companies is already complete attribution under §4; listing a non-contributing owner under "Contributors" would be factually wrong; and NOTICE files get scraped into SBOMs and compliance reports, so a name in one attracts mail — pointing that at a CEO with no involvement in the code is a cost with no benefit. Consequence to hold onto: the *authorisation* for going open is a governance fact recorded here in this entry and nowhere public, which is the correct place for it but does mean this logbook is the only record of it. Entity names are as of 2026-08-19; Prompted is mid-registration as an ApS, and the suffix sweep is one `grep -rl` over the ~66 files carrying a copyright line.
- **Per-file SPDX headers** on all 55 `.rs` files, the four `Cargo.toml`s, `.cargo/config.toml`, the coord config template, and the MCPB build script. jok's call over metadata-only. The reason it matters here specifically: individual source files travel — pasted into an issue, vendored, quoted in a support thread — and a file with no header carries no licence at all once separated from the repo.
- The **`.mcpb` bundle now ships `LICENSE` and `NOTICE` inside it**, and `manifest.template.json` declares `"license": "Apache-2.0"`. This is not decoration: the bundle *is* a redistribution under §4(a)/(d), landing on a laptop that will never see this repository. Getting the licence right in the repo and dropping it at the packaging step would have satisfied nothing.
- `publish = false` **stays.** Open-sourcing and publishing to crates.io are separate decisions; these are two deployables, not a library dependency, and nothing about Apache-2.0 asks for a registry.

**Made by:** Morten Hallberg (SerenIT ApS) and Kristian Schou (Prompted EV), jointly — recorded by jok. Licensing, like versioning, is a human call (CLAUDE.md). | **Review date:** N/A
**Status:** CURRENT

<a id="d-028"></a>
### D-028 — Chaperone stays plugin-neutral: the MCP announces the coordinated root, the agent reinterprets its own writes — 2026-08-12
**Trigger:** reading Kristian's tender-pipeline plugin (7 skills + 3 bundled Python scripts) while scoping the pilot. The plugin contains **zero** references to `chapr`/`chaperone`: the extraction script does its own `Path.rglob` / `open()` / `write_text()`, and every skill instructs the agent to write with ordinary file tools.
**Problem:** the plugin has independently arrived at exactly the workload Chaperone exists for — stage 3 is "a large fan-out" of parallel subagents, and `case.yaml` is the declared "single source of truth for case state", updated after every stage. Concurrent read-modify-write on one small shared file is the lost-update case. Today it is held together by convention (`the review stage`: "merge, never overwrite") — a hand-rolled substitute for CAS. Meanwhile **I-005 turns out to be solved outside Chaperone**: stage 1 extracts every PDF/xlsx/docx to a `extracted-text\` text mirror with `=== SIDE n ===` page markers, and stages 2–5 read only those derived artifacts. The model never needs a 24 MB PDF through `chapr_read`.
**The line drawn (jok):** Chaperone coordinates **shared mutable state**, not **regenerable bulk output**.
- *Agent-written contended state* — `case.yaml`, `INDEX.md`, the search-index file, `stakeholders.yaml` — must route through `chapr_write` with `base_version`.
- *Script-written bulk output* — the `extracted-text\` mirror, the spreadsheet generator's generated xlsx — stays uncoordinated. Single-agent, write-once into a fresh tree, so there is no lost update to prevent, and the files are derived and regenerable: the cheapest possible thing to lose. Reads by `pdfplumber` likewise stay direct — reads mutate nothing and the library needs a real handle.
**Chosen (jok): plugin-neutral, via the MCP's own instruction surface** — not a plugin-specific integration, and no tender logic in Chaperone. Two parts: (1) extend `ServerHandler::get_info().instructions` (`server.rs:576` — the slot already exists and already carries the injection framing) with the reinterpretation rule: for files under a coordinated root, prefer `chapr_write` + `base_version` over ordinary write tools, **regardless of what a skill's own text says**. (2) **Announce the coordinated root**, because the rule is unusable without a boundary — an agent cannot apply "under a coordinated root" without knowing where that is.
**Why the boundary is free:** the configured root is the same value that fixes **I-010** (no share-root confinement on caller-supplied paths). One config field serves both *enforcement* (refuse canonicalised paths outside it) and *agent guidance* (announce it in `instructions`). I-010 stops being a standalone chore. Filed as E-025.
**Honest limit — this reaches agent-issued writes only.** Instruction text cannot reach inside a subprocess: when a skill says "run `scripts/the extraction script extract`", the writes happen in a Python process the MCP server never observes. No wording fixes that. It is acceptable **only** because the line above puts exactly the script-written files on the uncoordinated side.
**Consequence: no change to the customer deployable is required.** Verified by reading all seven skills — every contended file is agent-written; the scripts write only regenerable output (the spreadsheet generator is explicitly "a GENERATED VIEW — never edit data in Excel, always in the YAML"), and `the index validator` only reads. Kristian is worth contacting for one thing: `the review stage` is **SPEC ONLY, not yet implemented**, so its workbook write is the cheapest place in the plugin to get right from the start rather than retrofit.
**Rejected:** (a) a Chaperone-side integration for the `the plugin` skills — specificity where the requirement is reusability; (b) asking Kristian to rewrite the plugin against `chapr_*` — a per-plugin dependency in both directions, and unnecessary given the above.
**Made by:** jok (plugin-neutrality as the requirement; the "if too complex, find a workaround" boundary) / Claude (plugin audit, the agent-vs-script split, the I-010 overlap) | **Review date:** N/A | **Status:** CURRENT

<a id="d-001"></a>
### D-001 — Project name: Chaperone / chapr.<method> — 2026-07-21
**Problem:** The architecture spec shipped with provisional working names (`FMCP`, `fs.*` namespace, reverse-DNS identifiers) that had to be replaced before the first commit.
**Options:** A: keep `FMCP`/`fs.*` — free now, costly to rename across spec + tool surface + audit event names later | B: pick the real name up front.
**Chosen:** B — **Chaperone**, tool namespace `chapr.<method>`. Rename is cheap before any code exists.
**Made by:** project team | **Review date:** N/A | **Status:** CURRENT *(trailer added 2026-09-07: the entry never carried one, per the footnote flagged on 2026-08-21. CURRENT by inspection — the naming has held through every subsequent decision.)*
