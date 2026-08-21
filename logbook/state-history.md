# Current State — history

> Child document of `LOGBOOK.md` (logbook protocol v1.0). **Newest first.**
>
> `## Current State` in `LOGBOOK.md` is marked VOLATILE and is meant to be
> *rewritten* at each session end. In practice it was appended to for a month and
> reached 20,539 chars — most of it narrative that had already stopped being
> "current". On 2026-08-21 it was condensed to a statement of what is true now,
> and every displaced paragraph was moved here **verbatim**, under the date it
> was written. Nothing was deleted.
>
> Read this when you need the reasoning behind a state change; read
> `LOGBOOK.md` when you need to know where the project stands.

---

## Displaced 2026-08-21b (phase 1 delivered; version 0.1.2)


**Phase:** implementation — v1 complete, installed at a customer, and
**pilot-tested on real hardware (2026-08-14)**. Multi-backend (SMB + POSIX),
Windows + Linux verified, macOS build-verified only.

**Version `0.1.1`** (set by jok 2026-08-20; the "hold until customer-tested" gate
is closed). Versioning is a human responsibility — never fill in a bump.

**Status:** three crates build clean; tests **324** Windows / **321** Linux (the
difference is `cfg(windows)` tests); clippy `-D warnings` clean on both. **29** coord
routes + the full **11-tool** MCP surface (concept §6's ten, plus `chapr_restore`)
+ a six-tab token-gated admin page with a config-writing Settings tab. MSRV
**1.88.0**, measured across the locked graph and read by CI out of `Cargo.toml`.

*All five numbers re-measured by `/logbook audit` on 2026-08-21 — tests by running
them (324 pass / 0 fail), routes by counting `.route(` in `http.rs`, tools by
counting `#[tool(` in `server.rs`, clippy by running it. Routes and tools had
drifted (recorded as 11 and 10); see I-004 and I-013.*

**The pilot closed the standing gate:** SMB mandatory locking *is* honoured by the
customer's Windows Server 2022 share, so invariant 3's foundation is measured, not
assumed.

**Open source (Apache-2.0, D-033), and the repo is two repos.** The original was
renamed `-archive` and stays private; a new public repo took the canonical name.
`LOGBOOK.md` and `CLAUDE.md` are removed from the public repo *and* its history.
**This file and everything under `logbook/` is therefore local-only, and the only
copy of project memory — and the only record of who authorised the relicense.**
It still needs a home in a private repo.

**Before touching the write path,** read the behaviour notes in
`logbook/state-history.md` (2026-08-05): SQLite runs in **WAL** (10 s busy
timeout); coord `PUT /blobs` is bounded at **256 MiB**, which is also the largest
file Chaperone can write at all, because a write snapshots the pre-image;
`put_blob` runs **before** `journal_open`; non-UTF-8 reads return base64.

**Known and accepted (not a bug):** after setup hardens the data directory, an
**unelevated** `chapr-coord serve --config <that file>` cannot read its own
config. The real deployment runs as a service account with access; hand-write a
config elsewhere to run coord by hand.

**Environment:** a Linux toolchain exists in WSL, building with an isolated
`CARGO_TARGET_DIR=$HOME/chapr-target` so the Windows `target/` is never clobbered.

**Project memory restructured 2026-08-21 (D-036).** `LOGBOOK.md` is an index; the
bodies are in eight child docs under `logbook/`. `/logbook end` now has a new
first step: **move the outgoing newest session entry into its month file** under
`logbook/logs/` before writing the new one. `/logbook decide` appends the body to
the matching `logbook/decisions/<theme>.md`, then adds one index row.

**What's in flight:** *(nothing — clean tree)*

**Blocker, unchanged:** ⚠️ Claude's git tool context cannot authenticate to
GitHub (`Permission denied (publickey)`); the on-disk key is rejected and `gh` is
not installed. **jok must push/fetch.**

**Audit run 2026-08-21.** `/logbook audit` verified version, MSRV, tests, clippy,
crate names, admin tabs and every child-doc count against the code; it found the
**route count (11 → 29)** and **tool count (10 → 11)** stale in both `CLAUDE.md`
and here, plus `CLAUDE.md` still on `0.1.0`. All corrected. I-004 stays **OPEN,
widened to the recurring pattern** (MED); the missing check is filed as **I-013**.

**What's next:**
1. **Give D-001 and D-032 a `**Status:**` line** — neither has one (a pre-existing
   gap, indexed `CURRENT ᵃ` by inspection). Trivial, but it is a claim about a
   decision's standing, so it is jok's to confirm.
2. **Classify the pilot's drive-mapping finding** — needs the diagnostic code from
   the Errors tab. `INVALID_PATH` = correct behaviour; a write *accepted* under a
   divergent canonical path = **broken invariant 5, HIGH**. The only open item
   that might be a data-integrity defect. E-022's mapped-drive branch is still
   unconfirmed.
3. **Check the CI run on macOS** (I-012) — first run to get past coord, so the
   first time `chapr-endpoint`'s suite executes there at all.
4. **`gh release create` + provenance attestation** (I-011) — never executed
   anywhere; a real tag is the first execution.
5. **I-005 — how a PDF's content reaches a model.** Not a pilot blocker (the
   workflow extracts to text mirrors upstream), but the residual cap question is
   real: a 200-page tender's extracted `.txt` runs 0.5–1 MB against a 512 KiB
   default.
6. **Have Kristian run `chapr-endpoint self-test` against the real share**, with
   `CHAPR_SELFTEST_DIR` on a **mapped drive**, to exercise E-022's unverified branch.
7. **Update `docs/init spec/fmcp-architecture-concept.md` to match D-026** — it
   still asserts the two channels "never cross". (CLAUDE.md's half is done.)

Deferred engineering (E-015, E-020, E-021, E-024b, V3-cloud) → `logbook/BACKLOG.md`.



## 2026-08-20 — the version gate closed

**Version (2026-07-22, jok):** hold at **`0.1.0`** until the prototype has been customer-tested; bump only after that. Not a per-feature bump. **⚠️ The condition was met on 2026-08-14** — the prototype has now been installed and tested on the customer's hardware. The hold is therefore satisfied and the bump is awaiting jok's decision; it must not be filled in without it (CLAUDE.md: versioning is a human responsibility).

**Version gate CLOSED — `0.1.1`, set by jok 2026-08-20.** The rule was "hold 0.1.0 until the prototype has been customer-tested"; it has been, and the first release after going open source carries the bump. Recorded because the recommendation on the table was `0.2.0` and jok chose `0.1.1`: the release adds a subcommand (`print-config`), raises the MSRV 1.85 → 1.88, adds a platform, and changes how the server identifies itself to hosts, which is conventionally more than a patch. jok's call, and the version is jok's to make. Side effect worth knowing either way: with the version changed, Claude Desktop can tell the bundles apart again, so users no longer have to *remove* the extension before installing a new build.

## 2026-08-19/20 — open source, the repo split, distribution

**OPEN SOURCE as of 2026-08-19 (D-033).** Apache-2.0, by agreement of both owning companies. LICENSE + NOTICE at the root, SPDX headers on 64 files, and `.mcpb` bundles carry LICENSE + NOTICE inside because the bundle is a redistribution. `publish = false` stays — open source and on-crates.io are separate decisions.

**The repo is now two repos.** The original was renamed to `-archive` and stays **private** (it keeps the PRs, and the pre-purge history); a new **public** repo took the canonical name and URL. `LOGBOOK.md` and `CLAUDE.md` are removed from the public repo *and* from its history — a force-push alone would not have sufficed, because GitHub's `refs/pull/*/head` refs survive one and are enumerable. **This file is therefore local-only, and is the only copy of project memory as well as the only record of who authorised the relicense.**

**Distribution is real (D-034, D-035).** GitHub Actions builds every release from a tag: `windows-x86_64`, `linux-x86_64`, `macos-arm64`, three artifacts each — coordinator, `.mcpb` bundle, and a bare endpoint binary. The endpoint is delivered as a **standard MCP server**, not as a Desktop extension: `chapr-endpoint print-config <host>` prints its own registration for Claude Code or any MCP host, and the server now identifies itself as `chaperone` rather than as `rmcp`. Only Claude Desktop and Claude Code are claimed as tested.

### Build environment

**Build note (new this session):** a Linux toolchain now exists in WSL (build-essential + pkg-config + rust 1.97.1 + clippy). Linux builds use an isolated `CARGO_TARGET_DIR=$HOME/chapr-target` so the Windows `target/` is never clobbered. The endpoint compiles on Linux because the `windows` dep + `SmbBackend`/`winfs` are `cfg(windows)`-gated and `PosixBackend`/`posixfs` (via the portable `fs4` crate) are not.

## 2026-08-14 — pilot on customer hardware

**What the pilot proved — the standing gate since 2026-07-22 is closed:** SMB **mandatory locking is honoured by the real fileserver**. Invariant 3's foundation is measured, not assumed. The audit trail worked on first contact, and the diagnostics pipeline surfaced a real misconfiguration nobody had predicted (see below).

**Correction to how the pilot's third finding was first described:** one endpoint could reach coord but had not mapped the share as a network drive. This was caught by the **endpoint**, which reported it via `POST /diagnostics`; coord displayed it. Coord does **not** validate paths against `share_unc` anywhere — that field is read only by `watch_win.rs`. The distinction matters operationally: the safety net only works while an endpoint can reach coord, which is exactly why `%LOCALAPPDATA%\Chaperone\diagnostics.jsonl` exists as the second place to look.

**2026-08-14 — installer rebuilt so the `.exe` drives the work (D-032).** The pilot install *worked* but the install *process* was the time-sink, and every cause was ours. `install-coord.ps1` assigned to PowerShell's automatic `$args` and then splatted `@args` — which behaves differently in 5.1 and 7.x — while doing nothing the exe could not do itself; **deleted**. The handover built its client URL from `cfg.addr`, so an administrator was told to configure `http://127.0.0.1:8787` fleet-wide; `addr` and the new `public_url` are now separate fields, and `validate()` refuses a loopback URL while the listener is reachable. The share was only asked for behind the watcher prompt, so the handover printed a generic example instead of the real value; it is now asked unconditionally, offered from `NetShareEnum` (filtering on `STYPE_SPECIAL`, **not** on a trailing `$`, so a hidden share like the customer's survives), and shape-checked on the unattended path too. `vc_redist_x64` was missing on the server: MSVC linked the CRT dynamically, so `.cargo/config.toml` now sets `crt-static` **workspace-wide** — the endpoint inside the `.mcpb` had the identical latent fault. Verified with `dumpbin /dependents` on both binaries, including the one extracted from the packed bundle: no `vcruntime140.dll`. A bare `chapr-coord` with no args now runs the wizard instead of `serve` (which would bind loopback on built-in defaults and look like success), and holds its console open on failure. Also new: **`chapr-endpoint self-test`**, so a customer laptop with no toolchain can verify its own deployment — including two-session contention, which needs two *sessions*, not two machines.

**Found while live-testing the above, and fixed:** the admin token was created *after* `harden_data_dirs`, so an unelevated setup run locked itself out of the directory it had just restricted and produced a coordinator whose admin page could never be signed into (fail-closed, but a dead install). This is the **second** time this ordering has bitten — `generate_self_signed`'s doc comment records the first. The rule is now explicit: everything the wizard must create, it creates before it locks the door.

## 2026-08-06 — data-integrity merges

**`main` is at `665d582`.** Two PRs merged 2026-08-06: **#3** `docs/invariant-6-restatement` (`1f58b5f`) and **#4** `fix/pilot-data-integrity` (`665d582`). Nothing is in flight.

**Merged 2026-08-06 — `fix/pilot-data-integrity` (D-027 audit remediation), commits `af0210f` + `52fef13`.** Baseline version-log entries (blob GC no longer reclaims the pre-agent version of every human-authored file, nor the overwrite-move's destination pre-image); torn-file markers that outlive one read (`intended_version` finally read back, so reader #2 is protected too); the missing Office pre-flight on `restore in_place`; **both** Office lock conventions (Word's `~$`-minus-two was never matched, so "humans always win" did not hold for any `.docx`); the move snapshot back under the held handle; `MOVEFILE_COPY_ALLOWED` dropped so cross-volume moves fail instead of silently stripping ACEs; committed writes no longer reported as failed; README content-safety claim corrected. **Tests 175 → 186, clippy `-D warnings` clean.** Deferred and filed: **I-007..I-010**.

**Also merged 2026-08-06 — `docs/invariant-6-restatement` (PR #3, `1f58b5f`).** README invariant 6 restated as settled per D-026 + a "Read limits" section; `tools.rs` invariant-6 rule extended to cover **routes** as well as types — the gap that let `PUT /blobs` go unsized in the first place.

## 2026-08-05 — outside contributions, and write-path behaviour changes

**Collaboration state (new 2026-08-05):** the repo now takes **outside contributions**. Two PRs merged from `Korsdal/Chaperone`: **#1** `fix/pilot-readiness` (Kristian Ole Schou-Pedersen — nine data-loss/availability defects, merged intact at jok's direction) and **#2** `fix/split-read-writeback-caps` (read cap vs write-back budget split + a live base64-transcript corruption gap found in what #1 merged). `main` is at **`468489e`**. Tests **153 → 175**; clippy `-D warnings` clean. The first tests that reach the MCP layer now exist — `ChaprServer` had never been constructed outside `main.rs`, which is precisely why the `from_utf8_lossy` binary corruption shipped. **Working model:** merge good work intact and fix forward on main, rather than stacking review branches.

**Behaviour changes worth knowing before touching the write path:** SQLite runs in **WAL** (10 s busy timeout) — four users plus reaper plus GC previously serialised into `SQLITE_BUSY` → 500, which was itself the trigger for the lease leak. Coord `PUT /blobs` is bounded at **256 MiB**, which is now also **the largest file Chaperone can write at all** (a write snapshots the pre-image, so the limit applies to the *existing* file). `put_blob` runs **before** `journal_open`. Reads return **base64 with `encoding=base64`** for non-UTF-8 files and carry **`writable_inline=`** in the envelope header. MSRV is **1.85** (clap needs it); version still **0.1.0**.

## Delivery record — the cumulative "What's done" list

> This list grew one bullet per epic from 2026-07-21 to 2026-08-03. Each bullet's
> reasoning is in the decision it cites (`logbook/decisions/`) and the session
> that produced it (`logbook/logs/`); the delivered-work table is in
> `logbook/BACKLOG.md` under **Delivered**. Kept verbatim.

**What's done:**
- Architecture concept and Rust implementation notes are settled and complete — see `init spec/`.
- Naming closed: project is **Chaperone**, tool namespace `chapr.<method>` (the spec's `FMCP`/`fs.*` are historical working names).
- `CLAUDE.md` written; logbook system installed (`.claude/commands/logbook.md`).
- **E-001 DONE — Cargo workspace + `chapr-proto`** (the shared wire contract; 7 modules; `ChaprError` exhaustive). 21 tests, clippy-clean. See D-002.
- **E-002 DONE — `chapr-coord` lease core.** SQLite-persisted lease table, all-or-none `lease_acquire` in canonical order + `lease_release`, lazy expiry, one coarse `Mutex`, axum HTTP with `ChaprError`→status mapping, env config + graceful shutdown. See D-003; auth shim tracked as I-001.
- **E-003 DONE — renewal + reaper + version index.** `lease_renew` (expiry capped at hard ceiling; force-expire/expired/not-found branches) + background `reaper` (proactive, default 30 s; lazy sweep remains the backstop). Version index (`version_index` table) with `coord.resolve` (composite `(path,mtime,size)` key, fills `lease_state`) + lazy `PUT /index` refresh (both lock-free). Proto `ResolveRequest` gained `mtime`+`size`. **Verified: coord 24 / proto 22 tests, clippy-clean, live smoke test (renew/resolve-hit/resolve-miss/index).** See D-004.
- **E-004 DONE — intent journal + crash detection.** `journal` module (`open`/`clear`, lock-free `state_for_path`, `scan_dangling`); `resolve`'s `journal_state` real Clean/Live/Dangling; `POST /journal` + `/journal/clear`; proactive startup scan. Detection only. See D-005.
- **E-006 DONE — content-addressed history.** `history` module: file-per-blob store (sharded, temp-rename, whole-file dedup) + coord-owned `version_log` chain; `PUT /blobs`, `GET /blobs/{version}`, `POST /version-log`, `POST /history`; `blob_root` on AppState + `CHAPR_COORD_BLOBS`. GC deferred to E-007. See D-006.
- **E-005 slice 1 DONE — endpoint foundation.** Promoted 7 coord DTOs into proto. **chapr-endpoint** lib: `canon` + `coord_client` + `coord_ping` example. See D-007.
- **E-005 slice 2 DONE — audit log + read path + rmcp `chapr.read`.** See D-008.
- **E-005 slice 3 DONE — conflict registry + audit wiring + write path.** E-009 conflict registry (store, surface-on-touch, `chapr.conflicts`/`resolve_conflict`, audit). E-008 audit wiring (lease_grant/renew/expire via session-on-lease; write_commit via `POST /audit`). windows-rs write path: `winfs::ExclusiveFile` (CreateFileW FILE_SHARE_NONE) + boring `cas_write` (one spawn_blocking, one handle, §7 all branches, sidecar-on-conflict, fail-closed) + `chapr.write`. **Verified: coord 46 / endpoint 26 tests, clippy-clean, live MCP write→read-back on disk + CAS conflict + audit trail.** See D-010.
- **E-005 slice 4 DONE — lease-renewal thread.** `lease_manager::LeaseManager` + background renewer. E-005 complete (slices 1–4). See D-011.
- **E-011 DONE — list/stat/history/create/delete/restore.** See D-012.
- **E-012 DONE — `chapr.move`.** Atomic coord `/move` re-key tx + `MoveFileExW` dual-lease rename + CAS both sides. **Verified: coord 48 tests, live plain + overwrite move.** See D-013.
- **🎉 v1 tool surface complete** — all of concept §6 (read/write/create/delete/move/list/stat/history/conflicts/resolve_conflict), live-verified end-to-end.
- **Version:** baseline **0.1.0** (human-approved). CLAUDE.md Status current.
- **Open items resolved:** §18 #3 (persistence engine = SQLite).

- **Hardening DONE (2026-07-21):** E-010 read-before-write, E-013 change-watcher, I-001/I-002 auth boundary, E-007 blob GC — all verified. See D-014/D-015/D-016/D-017.
- **Deployment brainstorm SETTLED (2026-07-21):** coord shortlist (north star = Opt 4, Linux coord + per-backend push watcher) + backend-adapter model, all forks locked. See D-019 and the plan file `~/.claude/plans/giggly-dancing-zebra.md`.
- **E-016 DONE (2026-07-21) — coord installer + service + TLS.** `chapr-coord setup` wizard (interactive + unattended), TOML config (defaults→file→env), rustls TLS serving, native Windows SCM + systemd, `/healthz` self-test + endpoint-config output. coord 72 tests, clippy-clean; live-verified (setup→serve→healthz; self-signed cert→HTTPS healthz). See D-018.
- **E-017 DONE (2026-07-22) — push watch endpoint.** `POST /watch/event` (direct `watch::apply`, coord-local `WatchEventRequest` DTO, `Caller`-gated); `mod watch` ungated (gate kept on `watch_win`); dead-code guards on the in-process-source items. +3 wiring tests. See D-022.
- **E-019 DONE (2026-07-22) — POSIX backend (second backend).** Shared §7 core extracted (`LockedFile` + `FsPrimitives` + `PathGrammar`); `SmbBackend` thin delegator (byte-identical); new `PosixBackend` + `posixfs` (portable `fs4` advisory `flock`) + `PosixGrammar`; `BackendKind::Posix`; `CHAPR_BACKEND` selection; `windows`/`winfs`/`SmbBackend` gated `cfg(windows)`. **Dual-platform live-verified** (SMB 14/14 on Windows, POSIX 14/14 on Linux). See D-021.
- **Deployment packaging DONE (2026-07-22, D-025).** Two-repo split: `Chaperone/packaging/` (reusable MCPB + coord tooling) + `docs/deployment-guide.md`; **<customer-repo>/** = **self-contained** Windows deployable (commits `coord/chapr-coord.exe` + `endpoint/chaperone-endpoint.mcpb`; scripts find the binary beside them). Endpoint MCPB packs + `mcpb validate`-passes (v0.3) + **installs in Claude Desktop** (confirmed). **Signing broken in mcpb 2.1.2 (I-003)** → ship unsigned for the MVP (accountability = audit trail). Remaining human steps: `git init`/push both repos; on-prem coord service install (elevated); customer prototype test.
- **Chaperone repo LIVE (2026-08-03).** `git init` → commit `636485a` (58 files) → **pushed** to `git@github.com:Korsdal/Chaperone.git`, branch `main`, **private**. Added `README.md` (first outside-reader doc; carries the "Generated with Claude Code" signature — README/PRs only, never commit messages) and `.gitattributes` (LF in-repo, CRLF for `*.ps1`). `.gitignore` hardened: `build/` + `*.mcpb` were the real gap (`build-mcpb.ps1 -OutDir ./build` copies the release exe there), plus `*.pdb`/IDE/`*.log` and `/target/`→`**/target/`. 19 GB `target/` confirmed excluded; author is repo-local `jok@serenit.dk`. **the customer deployable is still unpushed** — and must NOT reuse this `.gitignore` (it deliberately commits `.mcpb` + `.exe`).
