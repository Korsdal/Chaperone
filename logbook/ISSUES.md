# Known Issues

> Child document of `LOGBOOK.md` (logbook protocol v1.0).
> Staleness rule: issues open > 30 days are flagged STALE at session start.
>
> `LOGBOOK.md` keeps a one-line-per-issue table of the **live** ones, linking here.
> This file holds every issue in full, live and resolved.
>
> Restructured from a table on 2026-08-21: the narrative had reached 2.4 KB inside
> single table cells, which is the wrong container for prose. Text is verbatim;
> only the container changed.

---

## Live

<a id="i-016"></a>
### I-016 — An overwrite-move silently discards the source's open conflicts **and** its recoverable history.
**Severity:** MED | **Since:** 2026-09-08 | **Status:** OPEN — needs a semantics call (B4 / Q11)

Found by reviewing the admin panel's **Open conflicts** tab against the local rig, which is worth
noting on its own: the finding came from looking at real output, not from reading code with a
hypothesis. The panel showed a plain move having correctly re-keyed a conflict (`smoke_a_moved.txt`
paired with the historical sidecar name `smoke_a.conflict-…`); following *why* that worked led to the
branch where it does not.

**`mv.rs:47-55`, the `overwrite` arm:**

```sql
DELETE FROM version_log WHERE path = ?1   -- ?1 = src
DELETE FROM journal     WHERE path = ?1
DELETE FROM conflicts   WHERE base_path = ?1
```

The comment reads *"Destination keeps its own history/journal/conflicts; src is consumed."* The plain
move re-keys all three onto `dst`; the overwrite branch throws the source's away. Two consequences,
both traced through the code rather than inferred:

**1. Orphaned conflicts.** Every reader of `conflicts` keys on `base_path`, `conflict_id` or
`sidecar_path` (`conflict.rs:82,96,110,164`, `watch.rs:124`) — all require the row to exist — and
**nothing scans the share for orphaned sidecar files.** So the losing party's bytes remain on disk as
a file no query can reach and no `chapr.resolve_conflict` can name, because the `conflict_id` is
gone. The bytes survive; the pending human decision disappears. Against the standing rule *"register
it, surface on next touch"*, it can no longer surface.

**2. Destroyed history.** `gc.rs` derives its retention set **solely** from `version_log` — last-N per
path (`gc.rs:94-96`), recent-by-timestamp (`:109`), plus in-flight journal pre-images (`:122`). Delete
a path's rows and its blobs fall out of every set and are reclaimed on the next pass. This is
therefore not de-indexing: **the source file's recoverable history is permanently destroyed.**

**Why it is not simply a bug.** *"src is consumed"* is true of the **name**, but its history and its
pending decisions describe **content that just moved to `dst`**. Merging two histories onto one path
raises a real question the plain-move branch never faces — which entry becomes the head? That is
exactly **Q11** (*"May a move destroy recoverable history?"*) and **B4** (*"audit the overwrite
discard, settle §12's append-only status"*). The roadmap posed the question; this supplies the
mechanism and shows the current answer is **yes, silently**.

**Reachability:** requires an overwrite-move of a file that has an open conflict or history worth
keeping — narrow, but entirely plausible: an agent tidying up by moving a corrected file over the
original is the obvious path to it. The **destination** is protected (its pre-image is snapshotted and
`dst_base_version` is required); it is the **source** that loses.

**Not fixed deliberately.** The fix is a semantics decision, not a patch — jok's call under B4.
Options as they stand: (a) migrate src's conflicts and history onto dst as the plain branch does, and
settle head precedence; (b) refuse an overwrite-move while src has open conflicts, the way the `~$F`
pre-flight refuses; (c) keep the discard but make it an audited, recorded event rather than a silent
`DELETE`. **(c) is the minimum** — an append-only claim (§12) and an unlogged `DELETE` cannot both be
true.

<a id="i-002"></a>
### I-002 — endpoint↔coord channel unauthenticated.
**Severity:** ~~MED~~ LOW | **Since:** 2026-07-21 | **Status:** **MOSTLY RESOLVED** 2026-08-21 (roadmap item 1.1)

**What landed.** A `shared-secret` auth mode: one per-deployment token (`endpoint-token`, beside the admin token in the ACL'd data directory) **plus** the principal header, both required. It is the wizard's default for new installs. The warning this issue carried for five weeks — *"the day E-015 turns on enforcement, the ungated routes stay reachable while the gated ones begin rejecting"* — was **acted on before enforcement, not after**: a new `Authenticated` extractor closed the nine routes that carried no extractor at all, so the split this issue predicted never opened. Two of those nine were *mutating* and had never been named anywhere: `POST /journal/clear` (destroys crash-recovery state) and `PUT /blobs` (writes into the content store).

Verified over real HTTP, not only in unit tests: a bare principal header → 401, a well-formed but unissued 64-char token → 401, unauthenticated `journal/clear` → 401, the issued token → 200. `/healthz` and `GET /admin` stay open deliberately (monitoring; and the page where the admin token is typed), pinned by a test.

**Why LOW rather than closed.** Three residuals, all documented in `docs/security.md` rather than hidden:
1. **The principal is still asserted, not proven.** An endpoint holding the secret can name any user. Binding identity to a verified subject is E-015 — and per D-037 that stays *pluggable*, because the deployment posture varies (the current customer is cloud-managed with no on-prem realm to Negotiate against).
2. **`GET /blobs/{version}` still applies no ACL check** to an *authenticated* caller. The exposure narrowed from "anyone who can reach the port" to "anyone holding the deployment's secret" — which is every endpoint.
3. **One secret per deployment**, so revoking one laptop means rotating for all of them. Deliberate: per-endpoint credentials need issuance, rotation and a registry, which means a table, and `db.rs` has no migration machinery — machinery E-015 would then throw away.

**Closes fully when E-015 lands.** The route-count warning below is now historical; it was 20 ungated of 29, of which 9 had no extractor and are now guarded.

**Original entry, kept verbatim:**

Dev boundary in place (`X-Chapr-Principal`, D-016); **TLS transport** available (E-016, D-018). Production mechanism rescoped to **pluggable generic OIDC** (D-023, no longer domain-blocked) → **E-015**. **Two notes added 2026-08-06 (D-027 audit).** (1) Only 9 of 22 coord routes take the `Caller` extractor. Today that is **not** a hole — `Caller` is attribution, not authorization, and `TrustedHeaderAuth` accepts any non-empty header, so the 13 ungated routes grant an attacker nothing the gated ones do not. **But the day E-015 turns on enforcement, those 13 routes stay reachable while the 9 begin rejecting** — the gap converts into a real bypass at exactly the moment auth is supposed to start working. Audit them as part of E-015, not before. (2) Since D-026 coord's blob store holds **file content**, and `GET /blobs/{version}` applies no ACL check, so control-plane reachability now implies read access to file history. README corrected 2026-08-06; this raises E-015's priority for any deployment where coord's port is not already trusted.

**Route count corrected 2026-08-21 (`/logbook audit`).** The note above says "9 of 22 coord routes" take the `Caller` extractor. **There are 29**, counted from `.route(` in `http.rs` (all production; none test-only). So the ungated set is **20, not 13** — the gap this issue warns about is 7 routes wider than recorded, and it grew silently as routes were added. The argument is unchanged and still correct: today `Caller` is attribution rather than authorization and `TrustedHeaderAuth` accepts any non-empty header, so the ungated routes grant nothing; the day E-015 turns on enforcement, they stay reachable while the gated ones begin rejecting. **Concrete consequence for E-015: the audit list is 20 routes, and re-count it at the time rather than trusting this number — it has now been wrong twice.**

<a id="i-003"></a>
### I-003 — MCPB bundle signing non-functional in `@anthropic-ai/mcpb` 2.1.2.
**Severity:** LOW | **Since:** 2026-07-22 | **Status:** OPEN

**Verified empirically:** `node-forge` throws "PKCS#7 verification not yet implemented" → `mcpb verify` reports *every* bundle "not signed"; the produced signature doesn't validate under openssl; Claude Desktop shows "not signed" (self-signed or otherwise). **Workaround:** ship the MVP **unsigned** (accountability comes from the audit trail, not the bundle sig). Revisit when the toolchain is fixed; validate against Claude Desktop's actual signature requirements before investing in a company code-signing cert.

<a id="i-004"></a>
### I-004 — `CLAUDE.md` **Status** drifts from reality, and is auto-loaded before the logbook can correct it
**Severity:** ~~LOW~~ MED | **Since:** 2026-08-03 | **Status:** OPEN (recurring)

**Scope widened 2026-08-21 (`/logbook audit`).** Filed as one stale paragraph, kept open as the standing issue for the *pattern*, because the original claims were fixed and the section went stale again inside a week. Severity raised LOW → MED on that basis: this is not a cosmetic doc nit, it is the first thing every session reads.

**History.**
- **2026-08-03 — filed.** Said `chapr-endpoint` "is in progress" with the rmcp stdio server, the windows-rs write path and the lease-renewal thread **remaining** — all three had shipped (`server.rs`, `read.rs`, `winfs.rs`, `write.rs`, `lease_manager.rs` all exist; v1 surface complete, dual-backend, dual-platform). Also still described the project as SMB-only.
- **2026-08-14 — those two fixed.** §Status rewritten to match reality. Marked closed in Current State.
- **2026-08-21 — RECURRED, three new stale claims**, all in the same section, all measured wrong: version `0.1.0` with "the next bump is unblocked and is jok's call" (it was set to `0.1.1` on 2026-08-20, so the gate had closed); "the full 10-tool MCP surface" (**11** — concept §6's ten plus `chapr_restore`); "11 coord routes" (**29**). Corrected the same day.

**Why it matters, restated because it is the whole point of the issue.** `CLAUDE.md` is auto-loaded at every session start, so an agent reads it *before* `LOGBOOK.md` gets a chance to correct it. A stale §Status is therefore not passive — it actively seeds a session with wrong facts, and the 2026-08-03 filing recorded exactly that happening (that session had to source `README.md` from the logbook instead). The 2026-08-21 recurrence is worse in kind: "the bump is jok's call" invites an agent to raise a version question that was already settled, which is the one area where CLAUDE.md declares human authority.

**Why it keeps happening — the actual mechanism.** Both instances share a cause: the claims are *counts and states derived from the code*, written by hand, with nothing checking them. Every one of the five numbers in §Status is mechanically verifiable in seconds (`cargo test`, `grep -c '#\[tool('`, `grep -c '.route('`, `cargo clippy`, `grep version Cargo.toml`) and none of them was. **That gap is filed separately as I-013** — this issue is the recurring symptom; I-013 is the missing check.

**Closes when** either the numeric claims leave `CLAUDE.md` (pointing at a generated or measured source instead), or I-013 lands a check that fails when they drift. Correcting the prose again on its own does **not** close it — that has now been tried twice.

**Original entry as filed 2026-08-03, kept verbatim** (the rule is not to destroy entries; the rewrite above widened the scope rather than replacing the record):

> Says `chapr-endpoint` "is in progress" with "the rmcp stdio server + read path, the windows-rs exclusive-open write path, and the lease-renewal thread" **remaining** — all three shipped (`server.rs`, `read.rs`, `winfs.rs`, `write.rs`, `lease_manager.rs` all exist; v1 tool surface complete, dual-backend, dual-platform). Also still describes the project as SMB-only. **Why it matters:** CLAUDE.md is auto-loaded at every session start, so an agent reads the stale version *before* the logbook corrects it — this session had to source `README.md` from LOGBOOK.md instead. Fix = rewrite CLAUDE.md §Status + the SMB-only framing to match Current State. Unassigned.

<a id="i-005"></a>
### I-005 — **Delivering a large PDF's content to a model is unsolved** — `chapr_read` cannot support tender analysis.
**Severity:** ~~HIGH~~ MED | **Since:** 2026-08-05 | **Status:** OPEN

**Verified by reading the tree:** `read.rs` returns `ReadContent::Inline{bytes}` on every path, `server.rs` always wraps in `ContentBlock::text`, and there is **no** MIME, extraction, `Image` or `Resource` handling anywhere. A non-UTF-8 file comes back base64 — byte-exact and safe to round-trip, but a compressed PDF is not analysable in that form **at any size**, so this is not a cap problem and raising `CHAPR_MAX_INLINE_BYTES` does not fix it. It never worked: pre-2026-08-05 a PDF returned `from_utf8_lossy` mojibake, which is worse than refusal because a model may confabulate content and report it as read from the tender. **Why it matters:** the project's own framing is "agents read large materials (PDFs, tenders, proposals)" — this is the pilot's central workflow. **Three options:** `ContentBlock::Resource(EmbeddedResource)` with `mimeType: application/pdf` (available in rmcp 2.2 today — variants verified `Text`/`Image`/`Audio`/`Resource`/`ResourceLink`; host-side support in Claude Desktop needs testing) · make `ReadContent::Ref` real (defined in proto, produced nowhere) · declare PDF reading out of scope and coordinate only the derived artifacts. Needs a human decision. Unassigned. **DOWNGRADED HIGH→MED 2026-08-12 (D-028):** it is not a pilot blocker, because the workflow does not go through `chapr_read` at all. Kristian's `the tender-pipeline plugin` plugin extracts every PDF/xlsx/docx to a `extracted-text\` text mirror in stage 1 (the extraction script, page markers preserved) and stages 2–5 read only those derived artifacts — exactly the read-heavy/write-small workload CLAUDE.md describes. **Correcting this session's earlier claim** that raising `CHAPR_MAX_INLINE_BYTES` cannot help: true of a compressed PDF, but irrelevant once the PDF is never read through Chaperone. The residual, genuinely open work is smaller: a 200-page tender's extracted `.txt` runs 0.5–1 MB against a 512 KiB default cap (`server.rs:46`), so the cap and the `writable_inline` threshold need a pilot-realistic setting. The three original options (`EmbeddedResource` / real `ReadContent::Ref` / out of scope) remain the answer for direct binary reading, which is now a **capability question, not a pilot dependency**.
<a id="i-007"></a>
### I-007 — **`move_cas_core` violates invariant 4** — version-check and mutation are not under one handle.
**Severity:** MED | **Since:** 2026-08-06 | **Status:** **RESOLVED** 2026-09-08 (Phase B, B1+B2)

**Resolution.** The rename now runs **through the handle held since the source's CAS**, so nothing
can change the verified bytes between check and mutation. `LockedFile` gained `rename_to(dst,
replace)`; on Windows it is `SetFileInformationByHandle(FileRenameInfo)` with `DELETE` added to
`winfs::open_existing`'s access mask, exactly as D-027 established empirically. On POSIX `rename(2)`
never needed the fd closed, and `flock` is held on the open file description, so it survives the
rename. **`sfile` is no longer dropped before the rename** — that `drop` was the bug, in one line.

**`FsPrimitives::rename(src, dst)` was removed entirely**, along with `winfs::move_file` and
`posixfs::rename`, and a comment on the seam says why: a two-path rename can only be reached by
closing the handle first, so leaving it available invites reintroducing this. Renaming is now a
method on the *held file*, which makes the safe order the only expressible one.

**Verified, not asserted.** Mutation-checked — removing `DELETE` from the access mask fails four of
the five new `winfs` tests with `ERROR_ACCESS_DENIED` (5), so the fix is load-bearing rather than
incidental. `the_exclusive_lock_survives_the_rename` proves the actual invariant-4 property: a second
exclusive open is still refused at the *new* name, and succeeds only once the holder drops.
**Exercised against a real remote Windows Server 2022** the same day (`smoke_parts`: *move: source
gone*, *move: dest has v2*), not only on local NTFS — which is precisely the "unproven on real SMB"
condition this issue was deferred on.

**B2 closed with it:** `move_cas_core` had **zero** tests and now has six, driving the real generic
core against the real POSIX backend and real files with only coord mocked — happy path, stale source
version, missing `dst_base_version`, stale destination version, overwrite, and coord failing *after*
the rename (A1's regression, at the core rather than the tool layer).

**A platform difference found while writing them, worth keeping:** `fs4` is `LockFileEx` on Windows,
which is **mandatory**, so `std::fs::read` of a still-locked file fails with `ERROR_LOCK_VIOLATION`
(33). The first version of `rename_guards_non_overwrite` read the destination before dropping the
holder and failed — and the failure was *evidence for* the property being tested: the lock followed
the file through the rename.

**Original entry, kept verbatim:**

Both handles are dropped before `prims.rename` (`backend.rs`), so the move verb does hash-then-reopen-to-mutate with only the advisory dual lease covering the gap. **D-027 narrowed but did not close it:** `put_blob` moved under the held handle, cutting the window from a ≤256 MiB round-trip to two syscalls. **Empirically settled (D-027):** `MoveFileExW` cannot hold the handle (ERROR_SHARING_VIOLATION 32), but `SetFileInformationByHandle(FileRenameInfo)` **succeeds** while holding an exclusive `share=NONE`+`DELETE` handle — so the invariant *is* achievable; the fix is to add `DELETE` to `winfs::open_existing`'s access mask (currently `GENERIC_READ\|GENERIC_WRITE`) and rename through the handle. **Deferred solely on schedule risk: proven on local NTFS, unproven on real SMB.** Not reachable Chaperone-to-Chaperone (the all-or-none `{src,dst}` lease excludes other sessions); the exposed case is a non-Chaperone writer that opens, writes and closes inside the window — its bytes are destroyed by the rename with no snapshot and no conflict. POSIX is simpler: `rename(2)` does not require closing, so the handle can simply be held. **Test harness now exists** (the `FsPrimitives` stub added in D-027) — `move_cas_core` still has zero tests.


<a id="i-009"></a>
### I-009 — Path aliasing: `normalize` resolves neither `.` nor `..`, breaking invariant 5.
**Severity:** LOW | **Since:** 2026-08-06 | **Status:** OPEN

`pathgrammar.rs` — an aliased path opens the same file on the OS but keys coord state under a different string, so two namings of one file get two leases, and a torn-file marker or conflict sidecar can be orphaned under the alias. Deferred because agents do not naturally generate aliased paths: they use what `coord.resolve` and `chapr.list` hand them. Becomes real if a human or a config ever supplies a path by hand. Related: E-022 (DFS + drive-letter→UNC) is the same area of `canon.rs`.

<a id="i-011"></a>
### I-011 — Release + CI workflows had never executed on GitHub.
**Severity:** LOW | **Since:** 2026-08-19 | **Status:** **PARTLY RESOLVED** 2026-08-20

CI is green on Windows and Linux, and a `workflow_dispatch` proved the release **build** half — including `pwsh` + `mcpb pack` on the ubuntu runner, the one thing no laptop could check. Still unproven: `gh release create` (never executed anywhere), provenance attestation, and the whole pipeline on the new macOS runner. The dispatch now exercises checksums and notes too, so a tag is one unproven command rather than four. Closes when a real tag produces a draft release.

<a id="i-012"></a>
### I-012 — macOS is shipped but unexercised, and its endpoint tests have never run.
**Severity:** MED | **Since:** 2026-08-20 | **Status:** **RESOLVED** 2026-08-21 (roadmap item 1.5)

**Resolution (jok's call, over the alternative of deleting the artifact):** `macos-latest` joined **both** the unit-test job and the new end-to-end job. So the endpoint's suite runs there for the first time, and the coordinator + smoke suites + shipped self-test run there too.

**The limit is stated in three places rather than implied** — the workflow, the generated release notes and the README: POSIX uses **advisory** `flock`, so a green macOS run proves the endpoint drives a shared filesystem without crashing. It proves nothing about invariant 3 for a Mac talking to an **SMB** share, which is the realistic Mac deployment. Invariant 3 is proven on Windows against a real SMB share and nowhere else. The self-test agrees by construction: its mandatory-lock check reports SKIP off Windows, and a SKIP is not a pass.

**Caveat on the closure:** the job has never executed — every step was proven verbatim locally on Windows, but the macOS legs and whether Actions accepts the YAML are unverified until the first push. If the macOS leg fails, expect hostname resolution (the job pins its own name in `/etc/hosts` precisely because a fresh runner may not resolve it) and reopen this rather than patching around it.

**Original entry, kept verbatim:**

`macos-arm64` joined the release matrix (D-035) on the strength of the endpoint having no `cfg(target_os)`, no `cfg(unix)` and no `libc` use — an argument, not a test result. It has already produced one genuine safety bug: `harden_data_dirs` was fooled by `/var` being a symlink into `/private` and tried to chmod a shared system directory, which only the runner's permissions prevented (fixed, `3b30ea8`). **`chapr-endpoint`'s suite has still never run on macOS at all** — cargo stops at the first failing test binary, so coord's failure hid it on every macOS run so far. Until a green macOS run exists, treat the macOS artifacts as build-verified only, which is what the release notes say.

<a id="i-013"></a>
### I-013 — Nothing verifies the docs' numeric claims against the code, so they drift silently
**Severity:** LOW | **Since:** 2026-08-21 | **Status:** OPEN

**Filed as the cause behind I-004's recurrence** (I-004 is the symptom: `CLAUDE.md` §Status going stale twice in three weeks). The common factor in both instances is that `CLAUDE.md`, `LOGBOOK.md` and `README.md` state **counts and states derived from the code** — test totals, tool count, route count, clippy status, version, MSRV — written by hand, with nothing that fails when they stop being true. Prose drifts quietly; a failing check does not.

**Measured on 2026-08-21, which is how the drift was found at all:** version `0.1.1` ✓, MSRV `1.88.0` ✓, tests 166/126/32 = **324 passing, 0 failed** ✓ (exactly as claimed), clippy `-D warnings` clean ✓, six admin tabs ✓ — but **tools claimed 10, actual 11** and **routes claimed 11, actual 29**. The two that drifted are precisely the two that grow as features land, which is the pattern to expect: a number that only changes when someone adds code is the number nobody remembers to update.

**Every one of these is a one-liner.** `grep -c '#\[tool(' crates/chapr-endpoint/src/server.rs` · `grep -c '\.route(' crates/chapr-coord/src/http.rs` · `cargo test --workspace` · `cargo clippy --workspace --all-targets -- -D warnings` · `grep '^version' Cargo.toml` · `grep rust-version Cargo.toml`. The cost of the check is far below the cost of one session starting from wrong facts.

**Options, none chosen yet — needs a human call on how much machinery is warranted for a two-person project.** (a) **A CI step** that greps the docs for the claimed numbers, recomputes them, and fails the build on a mismatch — catches drift at the moment it is introduced, but hard-codes doc-parsing into CI. (b) **Fold it into `/logbook audit`** as a standing checklist, so the numbers are re-measured whenever an audit runs — zero machinery, but only as reliable as the cadence of running it, and this audit was the first in the project's history. (c) **Stop stating counts in prose** and have `chapr-coord`/`chapr-endpoint` report them (`--version --verbose`, or the admin Overview tab already renders route health), leaving the docs to link rather than assert. (c) removes the failure mode instead of detecting it, and is the only option that scales as more numbers accumulate.

**Note against over-fixing this.** Chaperone's framing is a collaboration engine, not a compliance product; three stale numbers in an internal doc cost one confused session, not a data-integrity defect. (b) is probably the right size unless the drift recurs a third time, in which case (c).

**2026-08-24 — the two counts that actually drifted are now gone from `README.md`, which is option (c) applied narrowly.** The "Ten tools against eleven coord routes" sentence was **removed rather than corrected** to 11/29, on the reasoning that a number nothing checks should not be asserted in the document most likely to be read first; the README's Status section names the tool surface instead, which is what a reader wants anyway. `CLAUDE.md` §Status still asserts both counts (11 tools / 29 routes, measured 2026-08-21), so I-004's surface is unchanged and the (a)/(b)/(c) call is still open for everything else. See also **I-014**, the same root cause in source doc comments.

<a id="i-014"></a>
### I-014 — Storage units mixed decimal and binary, and three doc comments stated the wrong constant
**Severity:** LOW | **Since:** 2026-08-24 | **Status:** **FIXED** 2026-08-24 (instances); prevention open, see I-013

**Trigger:** jok, proof-reading the README rewrite: *"MiB and MB was presented next to each other. It is not wrong, but it is like presenting the metric and the imperial system side by side."*

**Sibling of I-013, not a duplicate.** I-013 is about *counts* in the prose docs (tools, routes, test totals) drifting as features land. This is about *constants* being described in two unit systems and, in three places, with the wrong value — mostly in **source doc comments**, which I-013's scope does not reach. Same root cause: a hand-written number about code with nothing that fails when it stops being true.

**The cosmetic half.** Every ceiling in the codebase is 1024-based (`MAX_BLOB_BYTES = 256 * 1024 * 1024`, `DEFAULT_MAX_INLINE_BYTES = 1024 * 1024`, `WRITEBACK_BUDGET_BYTES = 128 * 1024`, `GcConfig::ceiling_bytes = 50 * 1024³`), so IEC units are the accurate ones. But the illustrative "200 MB PDF" sat one or two lines from "256 MiB" in `README.md`, `docs/architecture.md`, `docs/deployment-guide.md` and `crates/chapr-proto/src/tools.rs` — the two numbers a reader most wants to compare, in two different systems.

**The half that was actually wrong:**
- `crates/chapr-endpoint/src/main.rs:26` documented the `CHAPR_MAX_INLINE_BYTES` default as **512 KiB**. The constant is `1024 * 1024` = **1 MiB**. Wrong by 2×, and it is the number an operator reads before overriding the cap.
- `crates/chapr-endpoint/src/server.rs:39` reasoned from "512 KiB is roughly 130k tokens" fifteen lines above its own note "**Raised from 512 KiB to 1 MiB for the tender workload**". The comment contradicted itself; the opening half was stale leftover from before the raise, left behind *in the same comment block* that records the raise.
- `crates/chapr-coord/src/gc.rs:37` and `docs/deployment-guide.md` labelled the blob ceiling **50 GB** for a `50 * 1024 * 1024 * 1024` constant. Not a style choice: it understated the real ceiling by 7.4% (50 GiB = 53.7 GB).
- `crates/chapr-proto/src/tools.rs:42-43` called axum's `DefaultBodyLimit` **2 MB**, where `crates/chapr-coord/src/http.rs:143` calls that same default 2 MiB.

**Fixed 2026-08-24** in the README-iteration commit: four docs plus `CLAUDE.md` and five source files onto binary units, and the three wrong values corrected. `cargo check` and `cargo clippy -D warnings` clean after (comment-only changes).

**Deliberately not converted, so the next reader does not "fix" them:** the test-fixture sizes in `server.rs` (`400 KB`, `425 KB` ×2, `600 KB`) are approximations of raw byte counts, internally consistent, and converting them is churn no reader benefits from. `version.rs`'s "multi-GB/s" stays decimal because throughput is correctly SI. One residual inconsistency accepted: `server.rs:47` now reads "~400 KiB tender" while the test at `server.rs:1818` still says "400 KB" about the same tender.

**Why this stays filed rather than resolved-and-forgotten:** nothing prevents a recurrence. The stale 512 KiB survived a change to the very constant it described, three paragraphs away from it. I-013's option (a) — a CI check that recomputes claimed numbers — would only have caught this if its scope included `//!` and `///` comments, which I-013's write-up does not mention. Worth deciding **together with I-013**, not separately.

<a id="i-015"></a>
### I-015 — The binary guard refused non-UTF-8 *text* and told the agent to report it as a suspicious binary
**Severity:** ~~MED~~ LOW | **Since:** 2026-08-21 (phase 1, item 1.3) | **Status:** **MOSTLY RESOLVED** 2026-08-25

**`binary_guard`'s second clause asked the wrong question.** `if container.is_some() || std::str::from_utf8(bytes).is_err()` treated "not valid UTF-8" as "not text". Every legacy single-byte encoding produces invalid UTF-8 *precisely when* the text contains non-ASCII characters, so the test misclassified exactly the files that carry a language's own alphabet. On a Danish share: Windows-1252 `æ`=0xE6, `ø`=0xF8, `å`=0xE5 (`æble` is `E6 62 6C 65`, and 0x62 is not a continuation byte), and UTF-16LE — what PowerShell 5.1 `>` and older Notepad "Unicode" produce — failed identically. Neither is a container, so the class was `None`.

**Reproduced 2026-08-25**, all five fixtures refused: CP1252 `.txt`, CP1252 `.csv`, UTF-16LE with and without BOM, and a 4-byte buffer with one high byte. Full investigation, blast radius and costed options in `specs/bug-hunt-2508-findings.md` (local, `specs/` is untracked).

**The message was the worse half, and is the half that mattered.** The `None`-class refusal told the model the file "is an unrecognised binary format", that "a model handed base64 tends to recognise the container header" (there is none), and — the expensive sentence — "an unrecognised binary on the share is worth their attention". So the agent did not merely fail to read a routine Danish `.txt`; it raised a phantom finding with the user, in their own words, about their own data. For a product whose pitch is traceability, an agent inventing findings is worse than one saying "I could not read that".

**No size threshold, no partial success.** The guard sees the whole file with no window and runs *before* the size cap, so one non-ASCII byte refused a 1 MiB document. For Danish prose of any length the probability of that byte is ~1: the failure correlated with the customer's language, not with file size or type.

**Not a recorded policy choice.** Roadmap 1.3's "done when" named containers only; `LOGBOOK.md` said "refuses binary containers (16 formats, magic bytes)"; `docs/architecture.md` said "a PDF, Office document, image or archive". Nothing in this file or `BACKLOG.md` mentioned encodings, UTF-16, code pages or BOMs. `sniff.rs`'s own module doc stated the policy the call site then broke — *"a false refusal of a readable file is a worse bug than a missed refusal of a binary one"* — and omits the `BM` signature to hold that line for two bytes. **Note the drift direction: the opposite of I-013.** There the prose went stale about the code; here the prose was right about the intent and the code exceeded it.

**A regression, but narrower than it looked.** At `abefc61` (v0.1.1) `render_envelope` was byte-identical to today's and there was no guard, so a CP1252 file came back base64 with `encoding=base64` and the verbatim-echo note — degraded but round-trippable with no opt-in. What was lost is *default round-trippability* and *accurate diagnosis*, not readability: base64 Danish text was never analysable either. The refusal **policy** was therefore always defensible. The classification and the message were not.

**Fixed 2026-08-25 — message and diagnosis, deliberately not capability.** jok's framing settled the scope: *"it is not a file extraction service, it is a coordination service — and the coordination part works."* See D-039.
1. **`sniff::classify_unrecognised`** splits unrecognised bytes into `NonUtf8Text{evidence}` and `Binary` using BOMs, NUL parity (which also gives UTF-16 endianness) and a high-byte ratio. No dependency: `std` only. It **chooses a message, never an outcome** — both arms still refuse — which is stated in the module doc, because the `BM`-bitmap argument applies with full force the moment that changes.
2. **`RefusalKind`** replaces `container: Option<Container>` with `Container` / `NonUtf8Text` / `UnknownBinary`, and the text class gets its own frame: never the word "binary"; Chaperone's health separated from the file's state in the first sentence; the boundary stated in-band ("Chaperone coordinates files; it does not convert encodings or extract text"); a human remedy instead of a warning; and `allow_binary` reframed as the *encoding-preserving* copy path, which is what it actually is here. The other four classes gained the same boundary sentence so all five read in one register.
3. **A `NON_UTF8_TEXT` diagnostic** (`Severity::Warning`) now files the structural evidence — likely encoding, BOM, offset and value of the first invalid byte, high-byte share, size — through `diag.rs`, which was built for exactly this and which the refusal path had never reached (it returns before `tool_failure`). Coord groups by `(code, path)`, and the remedy points at the *producing step* so a folder's worth of legacy files reads as one fix rather than a flood. **Facts are structural only, no content excerpt**, so §13.2's existence leak is not widened into a content leak (cf. I-002 residual 2). **Container refusals file nothing** — a PDF on a share is a designed outcome, the same judgement `classify` already makes for a CAS conflict or an Office lock.

Tests 364 → 381 (this entry said 378; the commit body and a re-measure both say 381 — corrected 2026-09-07). Clippy `-D warnings` clean. Serve-versus-refuse is byte-for-byte unchanged: no read that worked before behaves differently, pinned by `classifying_the_bytes_never_serves_them` and the three pre-existing round-trip tests passing untouched.

**Why LOW rather than closed.** Non-UTF-8 text is still unreadable *as text* — now by design, documented, and with an accurate refusal, rather than by accident with a misleading one. Two residuals:
1. **The capability is deferred, not delivered.** Transcoding and extraction belong in the separate add-on booked as E-028, per D-039.
2. **A UTF-8 file *with* a BOM is served as text with `U+FEFF` inside the envelope body**, and nothing handles BOMs in either direction. Verbatim echo round-trips it, but a model that retypes the first line drops an invisible character and changes the file's bytes and version. Same root cause, small, and belongs with any future BOM handling rather than with this fix.
3. **An agent can still create a file Chaperone will refuse, through `base64`.** `binary_guard` runs on read only, so a base64 write of non-UTF-8 bytes is accepted and the next read refuses it — Chaperone accepting a file it will not serve. The `utf8` path *cannot* do this (`String::into_bytes()` is valid UTF-8 by construction), so an agent authoring text has no encoding to get wrong; both facts are now pinned by tests (`any_utf8_write_can_be_read_back_as_text`, `a_base64_write_of_code_page_bytes_is_accepted_then_refused_on_read`). **Mitigated 2026-08-25 by guidance, not enforcement** — `server::instructions()` now carries the authoring rule and names the specific loop (do not answer a refused read by copying its bytes elsewhere), plus a clause on `ContentEncoding::Base64`'s schema doc. Advisory by nature and deliberately so: a symmetric write guard would refuse byte-exact copying, which is `allow_binary`'s one legitimate use. See the D-039 amendment for the full reasoning and for the write-time-diagnostic option held in reserve.

**Still unanswered, and it is the question that set the severity:** per D-028 the primary read path is the extraction pipeline's text mirrors, and Python's `open(p,'w')` with no `encoding=` resolves to cp1252 on a Danish-locale Windows box (PEP 686 changes this only in 3.15). If any mirror is written that way, every mirror containing a Danish character is refused — the pilot's main workload failing on its main content. The refusal now says so accurately and files a diagnostic naming the producing step, which is the best a coordinator can do about it, **but somebody still has to ask Kristian.**

---
**2026-09-07 — the sibling axis, and a reversal of one of this issue's own choices (jok's call).**
I-015 fixed the *severity* axis: a text file in a code page is no longer described as a suspicious
binary. Phase A of the 2808 roadmap fixed the **content** axis on the arm this issue left alone — the
`RefusalKind::UnknownBinary` advice branch, which still ended *"an unrecognised binary on the share is
worth their attention."*

**This reverses a deliberate decision made here on 2026-08-25**, and the superseded argument is worth
keeping because it is a good one: *"for bytes that really are not text, 'worth their attention' is
honest and stays — it is only wrong about a text file in a code page."* True on the severity axis.
What it misses is that **reaching that arm establishes nothing**: it means only that no magic number
matched and the bytes did not classify as text. Chaperone has not determined the file is malformed,
misplaced or suspicious — so attaching a judgement, plus an instruction to escalate, asserts more
than the classifier knows. Every sibling arm (`Document`, `Archive`, `Image`, `Database`) reports a
fact and stops; this one now does too, naming what `chapr_stat` and `chapr_list` can still do.

Recorded here rather than as a new decision, on jok's call: this scopes I-015's own reasoning rather
than settling anything new, and refusal wording is what this issue owns. The 08-25 argument is
preserved verbatim in the test that used to assert the opposite
(`unrecognised_binary_is_refused_with_generic_advice`), so the next reader meets both sides at the
point of change.


## Resolved

<a id="i-001"></a>
### I-001 — Coord trusted a body-supplied `principal` (spoofable).
**Severity:** MED | **Since:** 2026-07-21 | **Status:** RESOLVED

2026-07-21 (D-016): auth boundary — all principal-bearing handlers use the authenticated `Caller` over the body. Residual real-Kerberos work is E-015.

<a id="i-006"></a>
### I-006 — Coord Windows service reports RUNNING before the listener is up.
**Severity:** MED | **Since:** 2026-08-05 | **Status:** **RESOLVED** 2026-08-12, `cbafd3e` (slice 1)

From Kristian's deliberately-deferred list, and it already bit once: the rustls provider panic (`ServerConfig::builder()` with both `aws-lc-rs` and `ring` linked) happened *after* every healthy-looking startup log, leaving the SCM reporting RUNNING with nothing listening. The panic is fixed (D-026 session; explicit provider install), but the **reporting order** is not — any future startup failure after the SCM handshake produces the same silent-dead-service. Fix = signal RUNNING only once `bind`/`bind_rustls` has succeeded. Unassigned.

<a id="i-008"></a>
### I-008 — A single failed lease renewal permanently stops renewing a live lease.
**Severity:** LOW | **Since:** 2026-08-06 | **Status:** **RESOLVED** 2026-08-12, `8dbde25`

`lease_manager.rs` marks a lease lost after one renewal failure and never retries, so coord reaps it mid-write. **Downgraded from MED by D-027:** the dangerous consequence was that the live journal entry then read as Dangling and a concurrent reader's `recover` **deleted** the marker while the write was still in flight; `recover` no longer deletes, so that path is closed. What remains is availability-only, and invariant 3 holds regardless (correctness rests on exclusive-open + CAS, never on the lease). Fix = retry with backoff before declaring a lease lost. **Pilot note (2026-08-12, D-028):** severity left at LOW — D-027 downgraded it on reasoning that still holds (availability only; invariant 3 rests on exclusive-open + CAS, never on the lease). But the pilot's central workflow is `the verification stage`, described in its own skill as "a large fan-out" of parallel long-running subagents, which is precisely where one transient renewal failure bites. Expect this to be the most likely "it hung" report in the pilot. **Re-rating needs a human call.** **RESOLVED 2026-08-12 (slice 5, `8dbde25`) — fixed *with* the Leases tab rather than before or after it**, because that tab renders the defect: a lease whose renewal stopped sits there as `stale`. Build the view and what it shows has to be true. Fix: a **definitive** answer from coord (`LeaseNotFound`/`LeaseExpired`/`MaxLeaseLifetimeExceeded`) gives up immediately; a **transient** one keeps the lease and retries on the next tick. Tolerance is **derived, not picked** — after `ttl_s / interval` consecutive failures a full TTL has passed without a heartbeat, so the lease has certainly lapsed and believing otherwise would be false; `ttl_s` comes from the grant, not a constant. A success resets the counter (consecutive failures are what matter). **Also now visible:** giving up reports a `LEASE_LOST` diagnostic — previously the symptom lived only in the endpoint's stderr, which goes nowhere. That required the renewer to share the diagnostics sink, which turned `ChaprServer::with_diagnostics` from a builder into a constructor parameter (two collaborators need the same sink; a builder applied afterwards updated only one). The old test asserting the defect correctly failed and now asserts the fix.

<a id="i-010"></a>
### I-010 — No share-root confinement on caller-supplied paths.
**Severity:** MED | **Since:** 2026-08-06 | **Status:** **RESOLVED** 2026-08-12, `d887eb6` (slice 2, E-025)

`canon.rs` applies no root check, so `chapr.read`/`chapr.write` accept any absolute local path or a UNC to an arbitrary host — the endpoint will act on anything the user's own token can reach. ACLs still bound it (no privilege escalation), so this is not a sandbox escape; it is a blast-radius and prompt-injection concern, and the injection threat model is already explicit in this project (§13.2, `chapr.read`'s untrusted-data envelope). Fix = a configured root, checked after canonicalisation. Deliberately deferred from the D-027 PR to keep it focused on data integrity.

