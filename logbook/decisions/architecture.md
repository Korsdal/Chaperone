# Decision Log — Architecture

> Child document of `LOGBOOK.md` (logbook protocol v1.0). **Most recent first.**
> Scope: data model, wire protocol, read/write path, backends, invariants, coord internals.
>
> The index of *all* decisions — every ID, title, date, theme and status — lives in
> `LOGBOOK.md` under `## Decision Log`. This file holds the bodies only.
> New entries are appended here by `/logbook decide` per the theme rule in
> `LOGBOOK.md`'s YAML (`sections.decision_log.themes`).

---

<a id="d-030"></a>
### D-030 — Subagent fan-out: serialize intra-session rather than merge sidecars; resolve drive letters rather than require them; diagnostics separate from audit — 2026-08-12
**Trigger:** verifying `main.rs:38` while scoping D-028 — `session_id = format!("sess-{}", std::process::id())`. The SessionId is **per endpoint process**, so every subagent in one Claude Desktop session shares one MCP server, one session, **one lease identity**. The pilot's central workflow (`the verification stage`, described in its own skill as "a large fan-out" of parallel subagents) is therefore *intra*-session — exactly where leases contribute nothing.
**Problem 1 — what intra-session contention actually costs.** Live bytes stay safe: exclusive-open serialises the writes in the OS and CAS catches the loser, so invariant 3 holds and neither party's bytes are lost. Two real costs remain. (a) The **conflict sidecar becomes the normal path** rather than the exception whenever two subagents touch one file — the conflict registry fills with self-conflicts, each surfacing on the next touch, and the audit trail (a primary deliverable) stops reading as an accountability record. (b) **`session_reads` is session-keyed**, so E-010's read-before-write assertion passes when subagent B writes a version subagent A fetched — the one guard meant to catch "you are writing something you did not read" does not fire between subagents.
**jok's proposal:** let the sidecars happen, then have the coordinating agent merge the `.yaml` products once all subagents return, with a coord rescan. **The "lease covers the whole workflow" half is correct and already true** (per the SessionId finding). **The merge half is rejected:** (1) sidecars are a data-loss backstop, not a work queue — routing the happy path through the error path degrades the audit trail; (2) merging `case.yaml` *is* a real merge — `log`/`aabne_spoergsmaal` are append-only lists but `stages.X.status` and `aabne_punkter[].status` are mutually-exclusive scalars, so a merge is *sometimes* mechanically safe, which is the worst property a data-integrity product can have; (3) coord cannot do the rescan being imagined — the watcher's rescan is `(path,mtime,size)` cache invalidation, and content-aware merging in coord would break invariant 1 and invariant 6's intent; (4) the timing is wrong — sidecars appear *during* the fan-out and surface on next touch, so subagent #7 inherits #2–#6's open conflicts mid-flight and may try to resolve them.
**Chosen (jok): prevent the collision instead of cleaning up after it.** Two composing parts — (a) **in-process serialisation**: a mutex per canonical path in the endpoint, so subagents in one process queue and sidecars never arise intra-session; touches neither coord, the wire contract, nor the invariants. (b) **Plugin-neutral instruction** extending D-028's rule: a subagent in a fan-out writes its **own** artifact, and the coordinating agent performs the single write to shared state. That is jok's step 4 without the sidecars, because the subagents never touched the shared file. The plugin already does this correctly for dossiers (one per document); only the per-stage `case.yaml` update breaks the pattern.
**Problem 2 — a blocked write must not read as a failure.** Waiting is now on the happy path, and CLAUDE.md's own failure direction warns an LLM will otherwise retry forever. **Chosen (jok):** a blocked write returns a **successful, typed queue result** — queued, expected wait, call again after X, do not retry now, do not abandon the task — not an error. The distinction is behavioural, not cosmetic: an error triggers abandonment or a retry storm, while a success carrying instructions gets followed. The terminal state after the per-file budget is exhausted says "ask the human", never "failed". jok's framing — a salesperson running a workflow already expects it to take time — is what makes real serialisation affordable: latency is the cheap axis here, integrity is not.
**Problem 3 — drive letters: resolve, do not require.** jok offered to make an identical drive letter across users a product requirement. **Rejected in favour of resolution.** A requirement whose violation cannot be detected is not a safeguard: one laptop mapped `P:` instead of `Z:` errors nowhere, and yields two lease keys, torn-file markers invisible under the other alias, and a watcher invalidating keys nobody uses — directly contradicting jok's own discoverability principle below. `WNetGetUniversalNameW` is one Win32 call plus a per-process mount-table cache — less work than documenting and supporting the requirement — and it makes endpoint keys match the **watcher's** UNC keys, so the watcher stops being dead weight. **Shape: resolve when possible, reject loudly when not**, so no silently-wrong state exists. For reusability the distinction matters: a requirement is a per-customer install condition, a resolution is a property of the product. (E-022, scoped down — the customer confirmed no DFS.)
**Problem 4 — diagnostics are not audit.** jok's principle, adopted: *we cannot anticipate every environment failure, but we can make failures discoverable.* Better strategy than enumerating environment questions — FSRM screens, path length, Mac/NFD clients and AV interference all become discoverable at runtime rather than needing to be predicted. **Chosen shape:** (1) audit and diagnostics stay **separate stores** — audit records what a principal did, diagnostics records why an operation failed (`ERROR_SHARING_VIOLATION 32` on a given laptop); merging them makes audit unreadable, which D-027 established is a primary-deliverable concern. (2) The endpoint reports structured failure events to coord over the channel it already has (it already POSTs audit); the admin role reads them. (3) **Also a local file log on the endpoint**, because a failure *before* coord connectivity — wrong coord URL, TLS mismatch, blocked port — cannot phone home. Today the endpoint is a stdio child of Claude Desktop and its stderr goes nowhere; that is a standalone gap.
**E-024 split (jok):** the admin **role** + read-only query routes ride the pilot round (the diagnostics log is unreadable without them); the **dashboard UI is explicitly not pilot work** and is deferred. Flagged because role + diagnostics + serialisation + E-022 in one round is materially larger than the sequence agreed earlier this session, and the UI is where a pilot plan slips.
**Agreed order.** *Pilot-blocking:* I-006 · E-022 (resolve + reject) · diagnostics incl. local file log · intra-session serialisation with the typed queue result. *Pilot-important and small:* installer ACL · `Caller` on `/blobs` + `/history` · E-025 coordinated root (also closes I-010 and carries D-028's rule). *Deferred:* dashboard UI.
**Made by:** jok (all four calls, the discoverability principle, the E-024 split) / Claude (the SessionId finding, the merge-direction argument, the undetectable-requirement argument) | **Review date:** N/A | **Status:** CURRENT

<a id="d-027"></a>
### D-027 — Audit remediation: baseline version-log entries, torn-file marker persistence, both Office lock conventions — 2026-08-06
**Branch:** `fix/pilot-data-integrity` (off `main` @ `468489e`). **Trigger:** a full-codebase adversarial audit (8 dimensions × independent refutation; 32 candidates → 10 confirmed → 7 distinct defects). Findings were verified against source before any fix; the auth-related candidates were correctly refuted (`Caller` is attribution, not authorization — adding it to the 12 ungated routes has zero security delta under `trusted-header`, which authenticates nothing).
**Problem 1 — the history store silently lost the pre-agent version of every human-authored file.** A write snapshots the bytes it *replaces* (`put_blob(current)`, step 7) and logs the version it *produces* (`commit_tail`, `blob_hash = v_new`). For a Chaperone-authored file those line up one write apart, so every blob ends up referenced. For a file Chaperone did **not** author — anything a human wrote, including an out-of-band edit between two agent writes — the pre-image matched no `version_log` row, so GC saw a plain orphan and evicted it past `write_grace` (1 h default, daily sweep). Live bytes were never at risk; *recoverability* was, and that is a primary deliverable. `gc.rs:22-24` shows the author anticipated a put_blob/version-log split and added `write_grace` — which does not cover this case. **Second site found during implementation:** the overwrite-move's destination pre-image, since `move_paths` logs the *source* version as dst's new head.
**Chosen (jok): baseline version-log row.** New `VersionEvent::Baseline` + wire `PreImage {version,size}`. Coord records a baseline entry for an unrecognised pre-image **inside the same transaction and ahead of** the produced version, so `prev_hash` links and the chain reads in true order; skipped when the chain already names the bytes. Rejected: (b) a GC-side keep-rule only — stops the eviction but leaves the pre-image invisible to `chapr.history` and unrestorable, i.e. half a fix; (c) also storing the post-image — closes the one-write lag entirely but doubles per-write upload, the exact cost D-026 analysed. **Baseline rows are attributed to the sentinel `"(pre-existing)"`,** not the observing agent: it did not author those bytes, and the audit trail is a primary deliverable.
**Problem 2 — torn-file detection protected only the first reader.** `journal::recover` deleted the entry and audited `crash_recover` in one transaction, but **nothing repairs the torn file**. Reader #2 found a Clean path, hashed the still-torn bytes and got them back as `Integrity::Verified` — silent corruption handed to a model, the exact inversion of "the reader never sees torn bytes". Compounded at `read.rs:250`, where the clear happened *before* the pre-image bytes were in hand, so a failed `get_blob` destroyed the marker and served nothing.
**Chosen (jok): compare `intended_version`, keep the marker.** That field was written at every journal open (`backend.rs:366`) and read back by **nothing** — it is exactly what separates "genuinely torn" from "committed, only `journal_clear` failed". `recover` is now a pure inspection: no delete, no audit. The endpoint hashes the file; equal → clear the stale entry and serve the file's own newer bytes `Verified`; unequal (or `intended_version` absent) → serve the pre-image and **retain** the marker, superseded later by a real write (`journal::open` is INSERT OR REPLACE). The `crash_recover` audit moved endpoint-side because only the endpoint can hash the file and so only it knows a recovery happened. Rejected: repair-on-read (makes the read path mutate the share, contradicting "reads mutate nothing"); ordering-fix-only (leaves reader #2 corrupted). **Side benefit:** this defuses the worst consequence of the deferred lease-renewal defect (I-008) — a reaped lease mid-write no longer lets a concurrent reader delete the marker.
**Problem 3 — `chapr.restore mode=in_place` had no Office pre-flight.** The only mutating verb missing `check_human_lock`, and it does no CAS by design ("a restore is a deliberate overwrite"), so that check was the sole guard between an agent restore and a document a human had open. Fixed.
**Problem 4 — `human_lock_path` implemented only one of Office's two conventions (refines D-F).** Excel/PowerPoint prepend `~$` to the whole filename; **Word replaces the first two characters** (`Quarterly.docx` → `~$arterly.docx`). The grammar produced only the prepended form, so the pre-flight **never fired for any `.docx`** — "humans always win" silently did not hold for Word, on write, delete, move *or* the restore just fixed. On a tender/proposal workload that is the dominant file type. **Chosen (jok):** `human_lock_paths() -> Vec<CanonicalPath>` returning every shape the backend's convention can produce; caller refuses if any exists. Rejected: branching on extension (drifts, and guesses which app holds the file). Sliced by `char`, not byte — accented business filenames would panic a byte slice.
**Problem 5 — the overwrite-move's snapshot upload sat inside the unlocked window.** `put_blob` ran *after* `drop(dfile)`, stretching the handle-free gap before the rename across a round-trip of up to 256 MiB. **Fixed to the extent it can be cheaply:** the upload now runs under the still-held handle, closing immediately before the rename (two syscalls). **The residual gap is a real invariant-4 violation and is NOT fixed — see I-007.**
**Empirically settled this session (`SetFileInformationByHandle` experiment, local NTFS):** `MoveFileExW` **fails** (ERROR_SHARING_VIOLATION, 32) while the source is held `FILE_SHARE_NONE`, so the current API genuinely cannot hold the handle across the rename — but `SetFileInformationByHandle(FileRenameInfo)` with `ReplaceIfExists` **succeeds** while holding an exclusive `share=NONE`+`DELETE` handle. So invariant 4 *is* achievable on this verb; the code simply does not use the API that achieves it. A fourth test confirmed the window is silently exploitable (foreign write inside it → destroyed by the rename, no snapshot, no error). **Caveat: NTFS only — must be re-proven on the real SMB target.**
**Also fixed:** `MOVEFILE_COPY_ALLOWED` dropped from `winfs::move_file` — it silently degraded any cross-volume move into CopyFile+DeleteFile, making the destination a *new* file inheriting the target directory's ACEs and losing the original's, which is exactly the ACL loss the never-temp-rename rule forbids and directly contradicted the doc comment above it. Cross-volume moves now fail with `ERROR_NOT_SAME_DEVICE` instead of quietly doing the wrong thing. **And:** README's "bytes only ever come through the user's own open" corrected — false since D-026, because coord's blob store holds file content and `GET /blobs/{version}` applies no ACL check.
**Wire compatibility:** all additions are `#[serde(default)]`, so old and new peers interop.
**Deferred, filed:** I-007 (move invariant 4), I-008 (lease renewal), I-009 (path aliasing), I-010 (share-root confinement).
**Made by:** jok (all five calls + the scope boundary) / Claude (audit, verification, implementation) | **Review date:** N/A | **Status:** CURRENT

<a id="d-026"></a>
### D-026 — Invariant 6: coord DOES see bytes, for history only (resolution (a)) — 2026-08-05
**Docs updated in the same change:** `README.md` §invariant 6 + new "Read limits" section; `crates/chapr-proto/src/tools.rs` §Invariant 6. **Still to update (gitignored, cannot ride the PR):** `CLAUDE.md` invariant 6 and `specs/fmcp-architecture-concept.md` — both still say "they never cross".
**Problem:** Invariant 6 read *"Write path carries bytes; control channel carries metadata. **They never cross.**"* That was false in the implementation and had been from the start: `write_cas_core` sends the file's previous contents to coord on every write (`PUT /blobs`), because the pre-image snapshot is what history and crash recovery are made of. The rule was enforced only on **type shape** (`tools.rs`: "no byte-carrying field on any coord-facing type in this module"), and `PUT /blobs` sends a raw `application/octet-stream` body rather than a proto type — so it never tripped the rule as written. Consequence: nobody sized that channel, it inherited axum's 2 MB `DefaultBodyLimit`, and **every write to a file already larger than 2 MB failed**. The failure landed on the *pre-image*, not the new content, which is why the ceiling looked unrelated to what was being written. Surfaced by Kristian's `fix/pilot-readiness`, which rewrote the README invariant unilaterally — a settled-decision change, hence this entry.
**Options:** **(a)** Accept that pre-image bytes flow to coord for history; size the channel explicitly. **(b)** Preserve the invariant literally: pre-images go to a blob store the endpoint writes directly, coord holding only metadata and keys.
**Chosen (jok): (a)**, on the merits rather than as a concession to the existing code. `MAX_BLOB_BYTES` = 256 MiB on the route, mirrored locally by `MAX_PRE_IMAGE_BYTES` so an oversized file is refused up front with an explanatory message instead of a bare 413 from mid-write. **The rule is now stated on the route as well as the type** — an invariant enforced on type shape alone does not hold.
**Rationale + the scaling question jok raised:** Coord RAM is not the binding constraint at any plausible scale — 8 GB requires ~32 concurrent 256 MB writes ≈ ~500 users routinely writing 256 MB files, versus a 20-user design target. The genuine cost is that the pre-image upload is **synchronous inside the exclusive-handle window**, so lock-hold time = upload time (~2.3 s per 256 MB on gigabit, ~23 s on 100 Mbps, exceeding the 120 s request timeout below ~18 Mbps). jok's position — a company running 10–20 people on an on-prem SMB fileserver does not have a 10 Mbps link — was accepted as correct; the genuinely exposed population is **sales laptops on hotel WiFi or tethering**, not slow offices. Even there it does not bite, for two reasons: the stated write workload is small derived artifacts (a ~75 MB pre-image is needed to hit the timeout at 5 Mbps), and a timeout **fails closed before any byte is written** (`put_blob` is step 7; the in-place overwrite is steps 9–10; the lease releases via the inner-async-block fix), so the worst case is a held lock and a refused write — never data loss.
**Consequences:** (1) 256 MiB is simultaneously coord's per-in-flight-write memory cost **and the largest file Chaperone can write at all** — a new hard product limit, now documented. (2) Both ends buffer whole (`Bytes` in, `Vec<u8>` out); streaming is the obvious future lever if the ceiling ever binds. (3) Dedup does **not** save this bandwidth in the common path — write N's pre-image is write N−1's *content*, which coord never stored, so each write ships genuinely new bytes. (4) Invariant 6's *intent* survives: the 200 MB PDF a model **reads** still flows endpoint → share → model and never touches coord.
**Made by:** jok (the call, and the bandwidth correction) / Claude (analysis, implementation) | **Review date:** N/A | **Status:** CURRENT

<a id="d-022"></a>
### D-022 — E-017 scope: push watch endpoint (direct-apply, coord-local DTO) — 2026-07-22
**Full detail:** `~/.claude/plans/chapr-e017-push-watch.md`. **Status of work:** DONE (2026-07-22, live-verified Windows + Linux).
**Problem:** Scope E-017 (push-based `WatchSource` + `POST /watch/event`) so an external watcher — a Linux coord's inotify feeder, a POSIX-backend watcher, or a cloud webhook — can drive coord's existing watch effects (index invalidation, conflict auto-close, overflow rescan). Two forks flagged.
**Forks locked (jok):**
- **D-G — Direct `watch::apply` per request.** The handler calls the already-public, unit-tested `watch::apply(&st, &event)` directly (one request → one effect). No channel/runner on the push path — the `run`/`ChannelSource` pipeline exists only to bridge a *blocking* OS thread to async, which an async HTTP handler doesn't need. Chosen over the handoff note's channel-pipeline hint (pure overhead here). `run`+`ChannelSource` stay for `watch_win` only.
- **D-H — Coord-local `WatchEventRequest` DTO** in `http.rs` (serde, `tag="type"`), mapping to the internal `watch::WatchEvent`; proto promotion deferred until a second **Rust** consumer exists (D-004/D-005 precedent; the first external watcher may be non-Rust curl/webhook needing only a stable JSON shape).
**Documented (not asked):** ungate `mod watch` (drop `#[cfg(windows)]` on `main.rs:33`, keep it on `watch_win`/`service_win`) — `watch.rs` is pure `sqlx`, zero Windows dep. Pushed path is **already canonical** (coord is backend-agnostic, can't know the grammar; watcher owns canonicalization, as `watch_win::to_canonical` does). Route gated by the existing `Caller` extractor (authorization-only; internal audit attribution stays `SERVICE\chapr-watcher`).
**Made by:** jok (D-G/D-H) / Claude (analysis) | **Review date:** N/A | **Status:** CURRENT

<a id="d-021"></a>
### D-021 — E-019 scope: POSIX backend (shared §7 core, per-backend grammar) — 2026-07-22
**Full detail:** `~/.claude/plans/chapr-e019-posix-backend.md`. **Status of work:** DONE (2026-07-22, dual-platform live-verified — SMB 14/14 Windows, POSIX 14/14 Linux).
**Problem:** Scope E-019 (first real second-backend) to prove the E-018 adapter model generalizes to a genuinely different capability profile: advisory `flock` (not mandatory), case-sensitive `/`-joined paths, inotify, no Office-lock convention. Four forks flagged.
**Forks locked (jok):**
- **D-C — Cross-platform buildable via `fs4`.** `PosixBackend`/`posixfs` use `fs4` (portable file locks) so the endpoint compiles + unit-tests on the Windows dev box; inotify gated `#[cfg(target_os="linux")]`; the `windows` dep moves behind `[target.'cfg(windows)']`. WSL + an Ubuntu 24.04 VM are available for a real Linux live-verify. **Caveat:** `fs4` locks are advisory on Linux / mandatory on Windows — Windows unit tests validate CAS *logic*; advisory-vs-external-editor behaviour is verified only in WSL.
- **D-D — Extract the §7 core.** Refactor the §7 write ordering into one shared generic over a `LockedFile` primitive seam + `PathGrammar`; both backends supply primitives only. One write path (invariant 4 preserved: cores stay synchronous inside `spawn_blocking`, coord calls via `WriteCtx.rt.block_on`). Bigger upfront refactor than E-018's coarse per-backend `write_cas` — the careful part. Prove byte-identical on Windows before POSIX lands.
- **D-E — `PathGrammar` per backend** (`sep`/`casefold`/`human_lock_path`/`sidecar`/`restored`/`join`); `canonicalize` takes the grammar; the hardcoded `\`-join in `read.rs:348` `list` becomes `grammar.join()`.
- **D-F — Per-backend `human_lock_path() -> Option<CanonicalPath>`.** SMB → `~$F` (pre-flight refuses); POSIX → `None`. "Humans always win" degrades to advisory-only on POSIX (accepted D-019 trade-off).
**Invariant note:** `VersionToken` stays BLAKE3 on POSIX (`token_kind:Blake3`, `native_cas:false`); ETag opacity is E-021, deferred. Local backend selection via `CHAPR_BACKEND` env (D-A: endpoint is authoritative, not coord).
**Made by:** jok (D-C..D-F) / Claude (analysis) | **Review date:** N/A | **Status:** CURRENT

<a id="d-020"></a>
### D-020 — E-018 Backend trait + coord backend-discovery seam — 2026-07-21
**Full detail:** `~/.claude/plans/giggly-dancing-zebra.md` (the implementation plan).
**Problem:** Realise the backend-adapter model (D-019): make the endpoint fileserver-agnostic (the MCP is the gate; coord was already ~agnostic) so one per-OS MCPB drives many backends, coord announcing which. **Deployment framing (jok):** MCPB is per **client OS** (Win/mac/Linux), NOT per fileserver — each bundle carries every backend its OS can drive; coord names the backend at runtime.
**Two forks locked this session:**
- **D-A — Write trust boundary: local-authoritative, coord-advisory.** The endpoint infers/verifies its backend locally and NEVER round-trips to coord to decide how to write; coord's announced backend is authoritative only on the read path (which already resolves) and a cross-check/degrade-trigger elsewhere. Preserves invariant 3; no write→coord round-trip. *(jok)*
- **D-B — Backend registry: config-driven prefix map, SQLite table deferred behind `backend_for`.** Coord holds a global `backend` + empty `backend_routes` in `Config`→`AppState`; storage sits behind one fn + one wire field, so a later table swap is invisible to proto/endpoint. Chosen for static mixed-backend topology. (A table would NOT break invariant 1 — routing is coord's own deployment policy, not cached file ground truth; deferred purely on "no runtime-mutability need yet.") *(jok)*
**Built (all green, behaviour-identical; coord 75 / endpoint 33 / proto 25, clippy-clean):**
- **Endpoint `Backend` trait** (`backend.rs`): `Capabilities`, `WriteCtx`, `CommitReceipt`, coarse per-backend `write_cas`/`create`/`delete_cas`/`restore_*`/`move_cas` (the §7 core moved verbatim, invariant 4 intact), `SmbBackend` sole impl (`FileSource` + `Backend`), `map_os_err` dedup, `select_backend`. Journal gated on `atomic_writes` (always-taken for SMB → identical). Version-log/audit tail hoisted to the async tool layer. Server holds `Arc<dyn Backend>`; reads via `as_file_source()` upcast (Rust 1.75). **`VersionToken` untouched (BLAKE3); `token_kind` descriptive only.**
- **Discovery seam:** proto `BackendKind`/`BackendDescriptor` + optional `ResolveResponse.backend` (wire-backward-compatible); coord `backend_for()` longest-prefix classify + announce; endpoint read-path cross-check (`select_backend` vs local `Backend::kind()`); wizard `--backend` + `CHAPR_COORD_BACKEND` env.
- **Mechanical defaults (Claude):** `Arc<dyn Backend>` dispatch; `map_os_err` shared fn; conflict-registry stays inside `write_cas`; `canonicalize` stays a free fn; single global backend now.
**Verified:** full-surface **live smoke 14/14** through real `winfs` + coord (create/read/CAS-write/CONFLICT+sidecar/history/restore/move/delete); live wire `resolve → {"backend":{"kind":"smb"}}`. Dev tool kept: `chapr-endpoint --example smoke_parts`.
**Made by:** jok (D-A/D-B) / Claude (impl) | **Review date:** N/A | **Status:** CURRENT

<a id="d-017"></a>
### D-017 — E-007: blob GC + retention — 2026-07-21
**Problem:** §12 blob GC/retention, unimplemented. Numbers (90d / last-10 / ~50 GB) are the unvalidated open item #2.
**Choice:** `gc` module — reference-counted mark-and-sweep with the version log as the reference set. Keep a blob's bytes iff it's within a file's newest `per_file_floor` versions (window function) OR its newest version-log timestamp is within `retention_days`; evict everything else (orphans + old-beyond-floor). A `ceiling_bytes` valve then evicts oldest age-kept-non-floor blobs until under. **Bytes only** — version-log metadata untouched (audit retention), so history stays answerable and a GC'd version fails cleanly with `VersionNotFound`. Background job on a timer (`CHAPR_COORD_GC_SECS`, default daily). All thresholds are tunable defaults (numbers still open item #2). 4 unit tests (floor, age, orphan, ceiling).
**Made by:** Claude (spec-driven) | **Review date:** N/A
**Status:** CURRENT

<a id="d-015"></a>
### D-015 — E-013: change-watcher (§14), trait-isolated on coord — 2026-07-21
**Problem:** Coord needs a change-notify watch to invalidate the version index on out-of-band edits + auto-close conflict sidecars + handle the buffer-overflow rescan. Watcher is Windows (ReadDirectoryChangesW) but coord was platform-agnostic (human chose trait-isolation).
**Choice:** Platform-agnostic core (`watch` module: `WatchEvent`, effects `invalidate`/`rescan`/`on_removed`, the `apply`/`run` runner, a `WatchSource` trait via RPITIT `+ Send`, `ChannelSource`) — fully unit-tested against a channel source. The Windows `ReadDirectoryChangesW` source (`watch_win`) runs a blocking watch loop on an OS thread feeding a channel; `Overflow`/`ERROR_NOTIFY_ENUM_DIR` → rescan. Both modules `#[cfg(windows)]` (the only source is Windows), and the `windows` dep is `[target.'cfg(windows)']`, so coord's core still builds on any OS without the watcher. Enabled via `CHAPR_COORD_WATCH_DIR`/`CHAPR_COORD_SHARE_UNC`. Path mapping is a lowercase string-join approximation of §5.1 (NFC/DFS not applied — documented). **Closes the previously-deferred watcher-inferred conflict close.**
**Made by:** jok (trait-isolation) / Claude (impl) | **Review date:** N/A
**Status:** CURRENT

<a id="d-014"></a>
### D-014 — E-010: structural read-before-write — 2026-07-21
**Problem:** §6.2 invariant: coord rejects any `base_version` it hasn't recorded this session as having read. Was unimplemented (CAS-only).
**Choice:** Low-churn design — a `ReadReceipt {session_id, path, version}` + coord `session_reads` table, rather than threading session onto the resolve/refresh request shapes. Recording points: `chapr.read` (after serving), and write/create **on commit** (so a writer can chain without re-reading). Enforcement: write/delete/move call `assert_read(base_version)` up front (write & delete before the lease; move: src up front, dst inside the blocking overwrite branch). Force-writes and restore/create skip the assert (no base_version). Recording is best-effort (must not fail a read/commit); asserts propagate `BaseVersionNotRecorded`. Defence-in-depth over CAS (which already requires base_version == current content hash).
**Made by:** Claude (spec-driven) | **Review date:** N/A
**Status:** CURRENT

<a id="d-013"></a>
### D-013 — E-012: chapr.move (atomic dual-lease rename) — 2026-07-21
**Problem:** The last v1 verb. Move must re-key coord state (version-log chain + open journal/conflict entries) from src→dst canonical path atomically. Human chose the atomic-coord-endpoint approach.
**Choices:**
- **New coord `POST /move`** (`mv` module) does the migration in **one transaction**: plain move re-keys `version_log`/`journal`/`conflicts` src→dst; overwrite move discards src's coord state (dst keeps its own lineage); then appends a `Move` version-log entry on dst and audits (`write_commit`, detail "move from {src}" — `AuditKind` has no dedicated move). Raw SQL inside the tx (not the lock-taking helpers) to avoid re-entrant lock deadlock.
- **Ordering: SMB rename FIRST** (the file is ground truth, invariant 1), then coord migration. The gap between them is the unavoidable no-cross-system-transaction window (explicit non-goal); a failure there leaves coord stale-but-recoverable.
- **SMB rename via `MoveFileExW`** (`winfs::move_file`) — a true rename preserving the file's ACL, `WRITE_THROUGH` for durability, `REPLACE_EXISTING` on overwrite. Not read+create+delete (wrong ACL semantics, non-atomic).
- **CAS both sides:** src always (`src_base_version` required); dst only when it exists (overwrite → `dst_base_version` required, else `BaseVersionRequired`). Endpoint hashes each under an exclusive handle, closes before renaming.
- Dual lease `{src,dst}` acquired all-or-none in canonical order via the existing coord `acquire` (lease manager renews it).
**Made by:** jok (approach) / Claude (impl details) | **Review date:** N/A
**Status:** CURRENT

<a id="d-012"></a>
### D-012 — E-011: the remaining tool verbs (all but move) — 2026-07-21
**Problem:** Round out the tool surface: list/stat/history/create/delete/restore (human chose "everything except move").
**Choices:**
- **No proto or coord changes** — every verb reuses existing coord endpoints (history/resolve/list_conflicts/put_blob/get_blob/journal/version_log/audit) and the winfs primitives. Endpoint-only work.
- **create/delete/restore** live in a new `ops` module as their own **explicit, boring** sequences (not routed through the verified `write::cas_write`, which stays untouched) — their steps genuinely differ: create has no pre-image (CREATE_NEW, version_log `Create`, no journal); delete is soft (snapshot pre-image → journal → `remove_file` → version_log `Delete`, recoverable); restore reinstates old bytes.
- **restore**: copy (default, §6.5) writes a `.restored-{ts}` sibling via `create_new_file` (no lease, new file); in_place takes the lease + full snapshot→journal→overwrite path. Both audit `Restore` (the `AuditKind::Restore` finally has an emitter).
- **list**: `std::fs::read_dir` + one scoped `list_conflicts` call to annotate `open_conflicts` per entry (surface-on-touch, §11); per-entry `version` omitted to avoid hashing every file.
- **stat**: resolve for version(cache)/journal/lease; on a cache miss it hashes the file to produce the required version (stat isn't the hot path); degrades to a local hash if coord is down.
- **delete CAS mismatch** → `Conflict` with `sidecar_path = the file itself` (delete has no losing content to park; the CONFLICT means "changed, re-read").
- All mutation verbs acquire/release through the `LeaseManager` (renewal applies).
**Made by:** jok (scope) / Claude (impl details) | **Review date:** N/A
**Status:** CURRENT

<a id="d-011"></a>
### D-011 — E-005 slice 4: lease-renewal manager — 2026-07-21
**Problem:** §9 prescribes a background renewal thread so a held lease outlives its 90 s TTL. Fully specified — built directly, no scoping question.
**Choices:**
- `LeaseManager` (endpoint) owns the held-lease set (`tokio::Mutex<HashMap<LeaseId, Held>>`) + a coord client + interval (default 30 s = TTL÷3). `spawn_renewer` runs a background `tokio` task ticking the interval and calling `renew_all_once`.
- **Async-ownership rule (notes §5): never hold the lock across an `.await`.** `renew_all_once` snapshots live ids under the lock, drops it, renews over the network, then re-locks briefly to mark failures.
- Failed renewal (expired/reaped/coord-unreachable) → lease marked **lost** and no longer renewed; a holder learns via `is_held` (concept §15). Correctness never depends on this (invariant 3: exclusive-open + CAS is the core; a lost lease degrades to the conflict path).
- **Integrated into the write path:** `write()` now acquires/releases via the manager, so the renewer (a separate task) keeps a lease alive even during a slow blocking `cas_write`. `ChaprServer::new` creates one `Arc<LeaseManager>` and spawns the renewer once; server clones share it.
- **No current multi-tool-call lease holder**, so `is_held`-based lost-lease surfacing to the agent is wired but not yet exercised by a caller; it matters once an operation holds a lease across turns.
**Made by:** Claude (spec-driven) | **Review date:** N/A
**Status:** CURRENT

<a id="d-010"></a>
### D-010 — E-005 slice 3: conflict registry + audit wiring + windows-rs write path — 2026-07-21
**Problem:** Slice 3 = the write path. Chosen (human): build the full conflict registry (E-009) and audit wiring (E-008) first, then the §7 write path. Built in three verified stages.
**Choices:**
- **E-009 conflict registry:** `conflicts` table + `conflict` module (register/count_open/list/resolve) with `conflict_open`/`conflict_resolve` audit. Surface-on-touch via a new `open_conflicts` field on `ResolveResponse` (zero extra round-trip). `chapr.conflicts` + `chapr.resolve_conflict` rmcp tools. **Watcher-inferred close (`inferred_from_deletion`) deferred** with the change-watcher (needs Win32).
- **E-008 audit wiring:** `session_id` stored **on the lease** (added to `AcquireLeaseRequest` + `leases` table) so `lease_grant`/`lease_renew`/`lease_expire` attribute without re-supplying it. `lease_expire` emitted by the reaper and both renew force-expire branches. `write_commit` emitted by the endpoint via a new `POST /audit` record endpoint (coord can't see writes — bytes never cross the control channel). `restore` unwired (no restore op yet). `lease_release` has no AuditKind (clean release isn't audited; only expiry is).
- **Write path (windows-rs):** the §7 core is one synchronous `cas_write` inside a single `spawn_blocking`; coord durability calls use `block_on` on that blocking thread, so the exclusive `HANDLE` never crosses an `.await`. `winfs::ExclusiveFile` = `CreateFileW` + `FILE_SHARE_NONE` (RAII close on drop, incl. early `?`). In-place overwrite+`SetEndOfFile`+`FlushFileBuffers` — never temp-rename. CAS conflict → `create_new_file` sidecar (`F.conflict-{user}-{ts}.ext`) + register + `Conflict` error. Fail-closed at the journal step. `windows` is a normal (un-gated) dep — the endpoint is Windows-only by design; features needed: Win32_Foundation/Storage_FileSystem/Security/System_IO.
- **Concurrency model** (settled, not asked): lease acquire/release async bracket; steps 3–11 synchronous under one handle. Small derived-artifact writes → brief blocking is fine.
**Made by:** jok (scope) / Claude (impl details) | **Review date:** N/A
**Status:** CURRENT

<a id="d-008"></a>
### D-008 — E-005 slice 2: audit log + read path + rmcp server — 2026-07-21
**Problem:** Slice 2 scope. Chosen (human): build the coord audit log first, then the full read path incl. spec-complete Dangling recovery + the rmcp stdio server.
**Choices:**
- **Audit log** (`audit_log` table + `audit` module: `record`/`query`, `AuditKind`↔string). Append-only, principal+session-stamped, lock-free append. Governance read exposed as `POST /audit/query`. This slice wires **only** the `crash_recover` kind (the others — lease_grant, write_commit, … — are E-008).
- **Dangling recovery** = `journal::recover`: re-verifies dangling (entry present + owning lease dead), clears the entry, records `crash_recover` audit, returns `RecoveredFrom`. Endpoint `POST /journal/recover`. Coord never writes bytes — the reader fetches+serves the pre-image.
- **Read state machine** (`read` module) behind a **`FileSource` trait** (StdFs real + mock in tests) so every §8 branch is unit-testable without Win32 or a concurrent writer. Clean (cache-hit trusts version / miss hashes+refreshes), Live (bounded re-resolve wait → RetryBudgetExhausted), Dangling (recover→get_blob→serve pre-image, integrity=recovered), degrade-open on CoordUnreachable (integrity=unverified). Live-state is driven by re-resolving journal_state, not by read errors.
- **rmcp 2.2.0 API** (verified against the vendored source, not guessed): `ContentBlock::text` (not `Content`); `CallToolResult::success(vec![ContentBlock])`; `ServerInfo = InitializeResult` is `#[non_exhaustive]` → build via `default()` + field set; `#[tool_handler(router = self.tool_router)]` to use the stored `ToolRouter` field (bare `#[tool_handler]` defaults to `Self::tool_router()` and leaves the field unread); tool name derives from the fn (`chapr_read`); serve via `ServiceExt::serve(stdio())` + `.waiting()`.
- **Untrusted-data envelope (§13.3)** applied in the rmcp server layer (not the read module) — the read module returns raw bytes; the tool wraps + labels integrity/version/recovery in-band. Tool description also states content is untrusted data.
- **Endpoint logs to stderr** (stdout is the MCP JSON-RPC channel). Session id = per-process for now.
**Made by:** jok (scope) / Claude (impl details) | **Review date:** N/A
**Status:** CURRENT

<a id="d-007"></a>
### D-007 — E-005 slice 1: proto DTO promotion + endpoint foundation — 2026-07-21
**Problem:** First slice of the endpoint. Handoff picked "foundation" (crate + proto promotion + coord client + canonicaliser; no windows-rs/rmcp). Several concrete choices followed.
**Choices:**
- **Promoted the coord-local DTOs into proto** (`AcquireLeaseRequest`, `RefreshIndexRequest`, `OpenJournalRequest`, `ClearJournalRequest`, `PutBlobResponse`, `AppendVersionLogRequest`, `HistoryQuery`) now that the endpoint is the second consumer — one shared contract, both binaries in lockstep. They sit in `tools.rs` alongside the coord-facing `ResolveRequest`/`ResolveResponse`. Note the name pair `LeaseAcquireRequest` (tool-facing, raw `uri`s) vs `AcquireLeaseRequest` (coord-facing, canonical paths + principal).
- **chapr-endpoint is a *library* this slice** — no `[[bin]]`/rmcp yet; the stdio server arrives with the read path. Keeps the slice free of MCP + Win32.
- **Coord HTTP client on `reqwest`**, HTTP-only (`default-features=false`, `["json"]`). Error contract closes coord's loop: transport failure → `CoordUnreachable`; HTTP error status → the `ChaprError` parsed from the body. Auth deferred (I-002).
- **Canonicaliser** does the platform-independent normalisation (separators → `\`, NFC, casefold, collapse repeats preserving UNC `\\`, strip trailing). **Deferred to the windows-rs slice:** DFS resolution and drive-letter→UNC mapping (need Win32). **Casefold approximated** by Unicode lowercase until a full-casefold crate is justified. It is the sole legitimate minter of `CanonicalPath`.
- **Client tests via `wiremock`** (mock coord, no live dep); plus a runnable `examples/coord_ping.rs` that exercises the real client against a live coord (kept as a dev smoke tool).
**Made by:** jok (slice choice) / Claude (impl details) | **Review date:** N/A
**Status:** CURRENT

<a id="d-006"></a>
### D-006 — History store: file-per-blob; GC/retention deferred — 2026-07-21
**Problem:** E-006 builds the content-addressed history. Two forks flagged at start: blob storage medium, and how much GC/retention to build (the §12 numbers are unvalidated open item #2).
**Options:** Storage — A: file-per-blob on coord's volume | B: BLOB column in SQLite. GC — defer vs build reference-counted mark-and-sweep now.
**Chosen (both human-confirmed):** **file-per-blob**, **GC deferred**. File-per-blob matches §12's placement ("separate mount from the operational DB"), keeps SQLite lean, and makes dedup free (content-address collision = dedup hit). GC deferred because it's tunable runaway-protection, not correctness, and the retention numbers are explicitly unvalidated — filed as **E-007**.
**Follow-on choices:**
- Blobs sharded two levels: `<root>/ab/cd/<full-hex>`. Written **temp-then-rename** so a crash can't leave a half-written blob under its final name. FS faults → `ChaprError::Internal` (coord's own storage, not a protocol branch).
- Coord **always re-hashes** received bytes to derive the key — never trusts a caller-supplied version.
- `version_log`: append-only, `id AUTOINCREMENT` for order; coord **owns the chain**, deriving `prev_hash` from the current head on append (never trusts the caller). Kept long even after blob GC (metadata ≠ bytes).
- `coord.history` returns proto `HistoryResponse` (the `{version,timestamp,writer,size,event}` projection), newest-first.
- Blob/version-log/history exposed as coord-local endpoints (`PUT /blobs`, `GET /blobs/{version}`, `POST /version-log`, `POST /history`) + DTOs — promote to proto in E-005.
- `blob_root` added to `AppState` (default `chapr-blobs`, `with_blob_root` builder); `CHAPR_COORD_BLOBS` env, dir created at startup.
- Missing-blob fetch → `VersionNotFound` (empty path field — a raw blob fetch has no path context; version is the key).
**Made by:** jok (storage + GC scope) / Claude (impl details) | **Review date:** N/A
**Status:** CURRENT

<a id="d-005"></a>
### D-005 — E-004 scope: intent journal + detection only; blob store split out — 2026-07-21
**Problem:** E-004's title is "intent journal + crash recovery," but actual pre-image byte-restore is unavoidably endpoint work (E-005 — it needs the Win32/SMB write path; coord cannot write bytes to the share). So how much coord machinery to build now was a real fork, flagged for session start.
**Options:** A: journal + detection only (record entries, report Clean/Live/Dangling, flag dangling on startup) | B: also build the content-addressed blob store + version log with byte-carrying store/fetch endpoints.
**Chosen:** **A** (human-confirmed). Recovery here means **detect and flag**, never restore. Keeps E-004 a tight, byte-free, fully-testable coord slice and defers the blob store — with its retention/GC concerns (§12) and byte-over-wire endpoints — to its own item (**E-006**), which precedes the endpoint's recover-then-serve.
**Follow-on choices:**
- `journal` table, one row per `path` (PK) — at most one in-flight write per path (the writer holds the exclusive lease). `open` is `INSERT OR REPLACE` (a pre-existing row is a superseded dangling write; safe under the exclusive lease). `clear` is idempotent `DELETE`.
- **journal_state** computed live in `resolve`: no entry → Clean; entry + owning lease alive → Live; entry + owning lease dead → Dangling (§8.1). `state_for_path` is lock-free (hot read path).
- `open`/`clear` mutate under the coarse lock; coord stamps `opened_at` itself.
- **No lease-ownership validation on `open`** — the write path is trusted (E-005 wires it correctly), and detection correctness doesn't depend on it since Live/Dangling is derived from actual lease liveness at resolve time.
- Proactive **startup scan** (`scan_dangling`, §15) logs a WARN per dangling entry (path + principal); no restore.
- Journal open/clear exposed as coord-local DTOs (`POST /journal`, `POST /journal/clear`) — promote to proto in E-005 (endpoint = second consumer).
**Made by:** jok (scope) / Claude (impl details) | **Review date:** N/A
**Status:** CURRENT

<a id="d-004"></a>
### D-004 — Version-index resolve contract: composite key; refresh stays coord-local — 2026-07-21
**Problem:** E-003 builds the version index + `coord.resolve`. Concept §4.2 states the index is "keyed by (canonical_path, mtime, size)"; §8.1 phrases staleness as the endpoint comparing "mtime/size differ from cache." Same outcome, two possible wire shapes — and the proto `coord.resolve` types (which E-005 consumes) had to be shaped one way. Also: with no change-watcher yet (deferred, needs SMB/Win32), the cache can only be populated lazily, requiring a refresh (write) call that proto doesn't define.
**Options:** Resolve keying — A: composite key (request carries observed mtime/size; coord returns `cached_version=Some` only on exact match) | B: path key + echo cached stat (endpoint compares locally). Refresh call — coord-local DTO vs add to proto now.
**Chosen (both human-confirmed):** **A — composite key**, and **refresh stays coord-local**. Rationale: (A) mirrors §4.2's stated key exactly, leaves proto `ResolveResponse` unchanged (only `ResolveRequest` gains `mtime`+`size`), and makes "changed file = miss" fall out of the key rather than endpoint-side comparison logic. Refresh coord-local mirrors the E-002 `AcquireLeaseRequest` precedent — promote to proto in E-005 when the endpoint (second consumer) needs it.
**Follow-on choices (not separately relitigated):**
- `version_index` table: one row per `path` (PK), columns `version, mtime_ms, size, updated_at_ms`. Resolve matches `WHERE path=? AND mtime_ms=? AND size=?`; refresh is an `ON CONFLICT(path) DO UPDATE` upsert.
- `resolve`/`refresh` are **lock-free** — resolve is a hot-path pure read (§8.1 "keep it fast"), refresh is a blind idempotent upsert with no read-modify-write to race. The coarse lock stays reserved for lease acquire/renew/reap.
- **Renewal** caps new expiry at `hard_expiry` (never past the 20-min ceiling); past-ceiling → force-expire + `MaxLeaseLifetimeExceeded`; lapsed heartbeat → `LeaseExpired`; unknown → `LeaseNotFound`. Each failure also deletes the dead lease.
- **Reaper** is a background task (default 30 s, `CHAPR_COORD_REAP_SECS`) that runs the same expiry sweep under the coarse lock. Proactive only — the lazy sweep in `acquire` remains the correctness backstop.
- `journal_state` in `resolve` is hardcoded `Clean` until the intent journal exists (E-004).
**Made by:** jok (keying + refresh location) / Claude (impl details) | **Review date:** N/A
**Status:** CURRENT

<a id="d-003"></a>
### D-003 — Coord persistence = SQLite; leases persisted with lazy expiry — 2026-07-21
**Problem:** Concept §18 open item #3: confirm the coord persistence engine *before* writing the schema. E-002 writes the first schema, and leases are ephemeral (90 s TTL) so it was a real question whether to persist them yet at all.
**Options:** Engine — A: SQLite (notes' working assumption) | B: PostgreSQL (heavier, growth hedge) | C: abstract/defer. Lease storage — persist now vs in-memory-only for E-002.
**Chosen:** **SQLite**, and **persist leases now** (both human-confirmed via question). Rationale: single on-prem instance, ~20 users, embedded, trivial backup — Postgres is overkill the notes explicitly warn against; a storage trait now is the over-abstraction they warn against. Persisting establishes the migration/pool layer E-003/E-004 build on and matches notes §8 step 2 ("with SQLite persistence"). **Resolves §18 open item #3.**
**Implementation choices that follow (not separately relitigated):**
- `sqlx` **runtime** queries (`sqlx::query`, not `query!`) → no build-time `DATABASE_URL`, no live DB to compile.
- Timestamps stored as **epoch-millis INTEGER**, not RFC 3339 text, so the lazy-expiry sweep (`WHERE expiry_ms <= ?`) compares exactly. chrono conversion at the Rust boundary.
- **Lazy expiry** instead of a background reaper: expired leases are `DELETE`d inside the acquire transaction before the conflict check. The reaper thread + renewal are E-003.
- **Coarse lock**: one `tokio::sync::Mutex` around the acquire/release critical section (notes §4), spanning the sqlx awaits.
- Lease modelled as **one row per (lease_id, path)** — an N-path set is N rows sharing an id; makes the per-path conflict check a trivial indexed lookup.
- **Coord HTTP DTO (`AcquireLeaseRequest`) lives in chapr-coord, not proto**, for now — endpoint doesn't exist yet. Promote to proto in E-005 when the second consumer appears. Reuses proto value/response/error types.
- **`principal` carried in the request body** as an auth shim until Negotiate/Kerberos is wired (filed as I-001).
**Made by:** jok (engine + persist) / Claude (impl details) | **Review date:** N/A
**Status:** CURRENT

<a id="d-002"></a>
### D-002 — chapr-proto concrete type choices (beyond the language-agnostic spec) — 2026-07-21
**Problem:** Concept §5.2/§6 define record and tool shapes language-agnostically. Turning them into Rust required concrete choices the spec deliberately left open; future sessions shouldn't relitigate them.
**Options / choices made:**
- **Timestamps:** `chrono::DateTime<Utc>` (RFC 3339 on the wire). Rejected raw unix-i64 (unreadable in SQLite/audit) and `time` (chrono is the more common default). Adds `chrono` to the shared contract — not in the notes §2 stack table, but timestamps are pervasive across records.
- **VersionToken owns the hash:** `VersionToken::hash(bytes)` lives in proto (invariant 2 in one function); `from_hex` validates 64 lowercase-hex. `blake3` is a proto dependency.
- **ChaprError wire form:** internally tagged on `code` in `SCREAMING_SNAKE_CASE` (`LEASE_HELD`, `CONFLICT`, …), matching the concept's error names. `thiserror` for `Display`/`Error`.
- **CanonicalPath is an *unvalidated* newtype** (`new_unchecked`): §5.1 canonicalisation needs Win32 (DFS resolve), so it stays in chapr-endpoint. Proto trusts the caller ran the canonicaliser. Documented on the type.
- **WriteMode internally tagged** so `Force{reason}` structurally carries its mandatory reason — a force write is unconstructable without a justification.
- **Empty success responses** are distinct empty structs (`DeleteResponse{}`, etc.) rather than a shared `Ack`, so the contract names each result type.
- **Workspace members:** only `chapr-proto` is listed; `chapr-coord`/`chapr-endpoint` are commented out until built (E-002/E-005), so `cargo build --workspace` stays green.
**Chosen:** as above. **Made by:** jok / Claude | **Review date:** N/A
**Status:** CURRENT

