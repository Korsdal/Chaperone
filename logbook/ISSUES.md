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
**Severity:** MED | **Since:** 2026-08-06 | **Status:** OPEN

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

---

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

