# Decision Log — Deployment & Operations

> Child document of `LOGBOOK.md` (logbook protocol v1.0). **Most recent first.**
> Scope: installer, service, packaging, releases, auth, admin authority, hosting.
>
> The index of *all* decisions — every ID, title, date, theme and status — lives in
> `LOGBOOK.md` under `## Decision Log`. This file holds the bodies only.
> New entries are appended here by `/logbook decide` per the theme rule in
> `LOGBOOK.md`'s YAML (`sections.decision_log.themes`).

---

<a id="d-048"></a>
### D-048 — The coordinator's install experience: a browser wizard, and an uninstall that keeps the data — 2026-09-09

**Problem:** jok's ask, verbatim: *"Click the installer - open an actual UI setup
wizard - point it to the installer - make it run as a service instead of through
an application/terminal that kills coord when it is closed. Give it a place for
uninstall as well - and backlog an 'update' functionality."* Taken as **one epic**
rather than piecemeal (his call).

**First, the correction that shrank the epic.** `setup` has installed an
**auto-start Windows service** since E-016 (`setup.rs:833` → `service_win::install`
with `ServiceStartType::AutoStart`). The fragility jok hit on the rig was mine: I
told him to run `chapr-coord serve` in a terminal, which bypasses the service
entirely. So "make it run as a service" needed no work — what was missing was
every question *after* install, and any front end other than prompts.

**Choices:**

- **The wizard is a browser page, not a native dialog.** Coord already has `axum`
  and already serves an HTML admin page, so a served page costs **no new
  dependency**. A native dialog means a GUI toolkit inside a binary whose whole
  delivery story is one self-contained executable, is Windows-only (coord runs on
  Linux), and would put UI code in the crate whose standing rule is that it holds
  no Windows primitives. `--ui`, and the default for a **double-click**; the named
  `setup` subcommand still gives the terminal prompts, so the rule is discoverable
  without a flag to turn anything off.
  - **It is a front end for `SetupArgs` and nothing else.** The page collects
    values and calls the same `setup::run` in its unattended mode. There is no
    second copy of probe / config write / hardening / service install / handover,
    so the two front ends **cannot** drift. Anything the browser can express,
    `--non-interactive` flags can express.
  - **Guards:** 127.0.0.1 on an ephemeral port (never a wildcard, even for the
    seconds it lives), a one-time token compared in constant time with the admin
    token's primitive, single-shot shutdown after one successful apply, and the
    URL printed to the console as well as opened — a wizard reachable only through
    an auto-opened browser fails on a server that has none.
  - **A found constraint worth recording:** the two must not call each other.
    `setup::run` redirecting to the UI made them mutually recursive, and a
    recursive `async fn` whose other arm owns an HTTP server has a future that can
    never be `Send` — which stops the wizard's own handler from being a valid axum
    handler, reported only as *"the trait bound is not satisfied"*. The front end
    is chosen in `main`, which is where a choice between front ends belongs.
- **`uninstall` removes the service and keeps every byte of data** (jok's call).
  The audit trail is a primary deliverable and history is only restorable when the
  database and blobs come from the same moment, so a command that deleted them
  would irreversibly destroy the two things the product exists to preserve. **No
  `--purge`**: a flag that erases an audit trail is a flag someone puts in a
  script. It prints the paths and leaves the decision to a human. The cost is
  stated rather than hidden — the tokens remain live until the directory is gone.
- **`status` reports four things separately**, because they fail separately: the
  config loads, the service exists, it runs, the port answers. Exit code 0 only
  when all four hold, so a monitor needs no parsing. It deliberately does **not**
  ask the SCM which config the service was registered with; it tells the operator
  how to check (`sc qc`) rather than guessing.
- **`handover` reprints, and adds an `mcpServers` block plus `--out`** (jok chose
  the file option knowing the credential cost). This is load-bearing *because* the
  bundle template now ships no defaults: three values have to be distributed, and
  distribution by retyping is where a root typo'd as `charptest` came from. The
  `command` field is a marked placeholder — coord has never seen the user's
  machine, and a plausible-looking wrong path is worse than an obvious hole.
- **The update path is backlogged behind C0**, and the reason is concrete rather
  than cautious: `db::migrate` is `CREATE TABLE IF NOT EXISTS` and nothing else,
  so an update needing a column would silently do nothing. Documented in the
  deployment guide as uninstall → replace → setup against the same data
  directory, which keeps history.

**Four defects the walkthrough caught, each of which would have shipped:**

1. **The form ignored the args it was handed**, so a double-click's
   `%ProgramData%` paths were replaced by built-in defaults and the install would
   have landed beside the executable — undoing the one thing
   `default_for_wizard` exists to do. Now pre-filled from the args, with a test.
2. **The handover re-read the config from disk** — from a directory setup had just
   hardened, so an unelevated run reported "could not be re-read" after otherwise
   succeeding. `setup::run` now returns what it applied; the values were already
   in hand.
3. **`--out` reused the data directory's ACL** (Administrators + SYSTEM), which
   locked the file against the operator who asked for it. A handover file its
   author cannot read is not a safer file. Now creator + Administrators, the
   latter by SID because the group name is localised.
4. **`IO error in winapi call`** for a missing service. 1060 (no such service) and
   5 (access denied) mean *opposite* things — absent versus
   present-but-invisible — and reporting the second as absence would send someone
   to reinstall over a working install.

**Made by:** jok (the epic, the wizard-versus-dialog call, uninstall's data
policy, the handover file) / Claude (mechanism, the service correction, and the
four defects above) | **Review date:** N/A
**Status:** CURRENT

---

<a id="d-042"></a>
### D-042 — What the audit trail claims: an amendment scoping D-024, not a reversal of it — 2026-09-07

**Problem:** **D-040(4) says "make the audit claim true."** Before building the machinery that would
make it true, the claim itself has to be stated precisely — otherwise the work delivers something
stronger-sounding than it is. Three findings set the stakes, none of them previously booked:

1. **No integrity at all today.** `audit_log` is an ordinary table in coord's read-write SQLite file
   (`db.rs:83-98`); "append-only" is a property of the code that touches it and nothing more. A local
   administrator can edit or delete rows and leave no trace. There is no chain, no enforcement and no
   verifier.
2. **Writing is easier than reading, which is backwards for an accountability record.** `POST /audit`
   takes `kind`, `path`, `session_id` and `detail` **verbatim from any authenticated caller**
   (`http.rs:729-747`), while *reading* the trail requires `AdminAuth` (`http.rs:754-758`).
3. **The principal is asserted, not verified** (D-024's own accepted limitation), and a per-deployment
   shared secret admits any endpoint holding it, which may then name any principal.

**The tension.** D-024 priced the trail at *"accountability, not court-grade non-repudiation"*, and
**that same framing justified deferring E-015, choosing `trusted-header`, and shipping unsigned
bundles (I-003)**. If "make the claim true" is read as raising D-024's bar, all three reopen at once.

**Options:** **(a)** An amendment that *scopes* D-024 — states precisely what a chain does and does
not claim, D-024 otherwise standing. **(b)** A reversal — treat the audit claim as a commitment that
pulls E-015 and bundle signing in with it. **(c)** Leave D-024 alone and build the chain silently.

**Chosen (jok): (a), an amendment scoping D-024. D-024 stands.**

**What the chain will and will not claim, stated so nobody has to infer it:**
- **It claims tamper-evidence about *records*.** Given the chain, rows cannot be altered or removed
  after the fact without detection. That is a real and useful property: it makes "the trail says what
  it said yesterday" checkable.
- **It does not claim proof about *people*.** The principal in a row is **asserted** by an endpoint
  that authenticated with a per-deployment secret. A chain over an asserted identity proves the
  record was not edited; it does not prove who acted. **Binding identity to a verified subject is
  E-015**, which stays deferred and stays pluggable (D-037(3)).
- **Therefore: not non-repudiation, and the documentation must keep saying so.** The audience is the
  customer's own organisation (D-040(5)), for whom "these records have not been tampered with" is
  worth having; it is not evidence for a dispute with a third party.

**Consequences.** E-015 stays deferred — this amendment deliberately does not reopen it, and it must
not be cited as a reason to. I-003's unsigned bundles are likewise untouched. Two things do become
in-scope because they are cheap and inside the scoped claim: **aligning write authorisation with read
authorisation** on `POST /audit`, and **validating `kind`/`path` server-side** rather than trusting
the caller's strings. The chain must be **coord-derived, never caller-supplied** — D-006's standing
rule for `version_log.prev_hash`, which already demonstrates the pattern in this schema.

**Made by:** jok (the posture, and that it scopes rather than reverses) / Claude (the three findings
and the claim/limit wording) | **Review date:** when E-015 is built, which is what would let the
claim about people change | **Status:** CURRENT

<a id="d-035"></a>
### D-035 — The endpoint is delivered as an MCP server, not as a Claude Desktop extension — 2026-08-19

**Trigger.** jok, while correcting a factual error I had written into the README ("Claude Desktop's Linux story is thin" — it is not; Desktop runs on Linux, and jok runs it), made the larger point: **MCP is a standard.** ChatGPT and GitHub Copilot speak it too. If Chaperone works on other harnesses, the addressable use case stops being one vendor's tooling. So: what delivery format is close to plug-and-play *without* locking users to Anthropic?

**Finding: the code was already there; only the packaging assumed a vendor.** `server.rs` names Claude **zero** times — the tool descriptions and the MCP `instructions` string say "agent", "model", "another party". It is `rmcp` (the official SDK) over stdio, and every setting is an environment variable. "Run this command with this environment" is expressible by every MCP host, so *that* is the portable interface, and it already existed. The vendor-specific part was exactly `manifest.template.json`'s `${__dirname}`, `${user_config.*}`, `user_config` and `platforms` — MCPB constructs. Strip them and what remains is one command and three environment variables.

**There is no cross-vendor bundle format to target.** `.mcpb` is Anthropic's; nothing equivalent exists for Copilot or ChatGPT. So "plug-and-play without vendor lock" is not a new package format. It is two things.

**Decisions.**

1. **The binary prints its own client registration — `chapr-endpoint print-config <host>`.** `generic` emits the portable `mcpServers` block; `claude-code` emits a `claude mcp add` one-liner. This is D-032's philosophy (*the executable is the installer*) applied to the endpoint's client side rather than coord's service side. It resolves its own absolute path and emits the environment it actually has, so on a configured machine the output needs no editing. **Config to stdout, advice to stderr**, so `print-config generic > .mcp.json` is a usable file — the same discipline the MCP server itself observes, for the same reason.

2. **It never writes to a host's config file.** Printing is inspectable and reversible; installing is neither, breaks when the host changes its schema, and needs an uninstall path to be honest. Rejected deliberately, not overlooked.

3. **The bare binary ships for every platform.** Previously Linux only, on the mistaken reasoning above — which meant a Windows user of Claude Code, VS Code or Cursor had **nothing to download but a Desktop bundle**. The real distinction is bundle-format versus bare command, and it is orthogonal to the OS.

4. **macOS joins the release matrix** (`macos-arm64`). Cheap: the endpoint has no `cfg(target_os)`, no `cfg(unix)` and no `libc` use anywhere, so the split is Windows-vs-not and macOS takes the same POSIX path. Labelled **built, not exercised** in the release notes — a green artifact must not imply a tested one (the same SKIP-is-not-PASS rule as D-032's self-test).

5. **Only Claude Desktop and Claude Code are claimed.** Other hosts should work and `generic` is aimed at them, but we do not test them, so the docs say so rather than listing vendors we have never run.

**Found while verifying, and worth more than the planned work:** the server introduced itself as `{"name":"rmcp","version":"2.2.0"}`. `Implementation::default()` reports the **SDK**, and every host displays `serverInfo.name` in its server list — so Chaperone announced itself to every client as the library it happens to be built with, telling an operator nothing about which build they were running. Trivial to fix and invisible until the goal became presenting well in *other* vendors' interfaces. Extracted as `server_identity()` beside `instructions()` so it is testable without standing up a server, and asserted against the SDK default so it cannot regress.

**Not done:** publishing to the MCP Registry (`server.json`). That is discovery rather than install, and its current requirements need checking first.

**Made by:** jok (framing and the call), implemented and verified against current Claude Code documentation rather than from memory. | **Review date:** N/A
**Status:** CURRENT

<a id="d-034"></a>
### D-034 — Releases are CI-built artifacts on a tag, not committed binaries — 2026-08-19

**Problem.** D-033 makes the repo open, and jok's framing was the customer deployable's: *people who grab open-source off GitHub can package binaries themselves, but making it easy is the kind thing to do.* the customer deployable solves this by **committing** `coord/chapr-coord.exe` and `endpoint/chaperone-endpoint.mcpb` into the deployable repo (D-025, revised) — correct there, because a customer's coord host clones the repo and must be functional with no toolchain.

**That precedent does not transfer to this repo, and the difference is worth stating.** the customer deployable is a *deployable*; Chaperone is the *source*. Committing binaries here would put a 9 MB blob into every clone forever, and — worse — create an artifact with no verifiable relationship to a commit. "Which source built this .exe?" has no answer when a human dropped it in. **Chosen: GitHub Releases, built by Actions from a tag.** A release artifact is then reproducible by anyone with the tag, and the honest answer to that question is in the workflow log.

**Decisions inside that.**

1. **The tag must match `[workspace.package] version`, or the release fails.** Not a warning — a hard gate with an error naming CLAUDE.md's rule that versioning is a human responsibility. The bump is the decision; the tag only records it. Without this, `git tag v0.2.0` on a tree that says `0.1.0` produces a release whose `.mcpb` reports the old version to Claude Desktop, which identifies bundles by name+version and would show the upgrade as already-installed.
2. **The release is created as a DRAFT.** CI assembles; a human publishes. Consistent with every other human-owned gate in this project.
3. **`workflow_dispatch` runs the whole pipeline and creates nothing.** A tag is not a cheap thing to spend on finding out whether the packaging works.
4. **`build-mcpb.ps1` is cross-platform but refuses to cross-compile.** `-Platform` defaults to the host and must equal it. A bundle labelled `linux` while carrying a `.exe` installs cleanly and then fails to start — a failure that surfaces on a user's machine, not ours. The workflow therefore builds each OS's bundle on that OS's runner. The script also gained template instantiation (`-Template` + `-Version`), so CI does not hand-edit JSON, with a hard error on any surviving `{{PLACEHOLDER}}` because `mcpb validate` accepts the literal string `{{VERSION}}` as a version.
5. **Neither workflow sets `RUSTFLAGS`, and that is load-bearing.** The env var **replaces** `target.*.rustflags` from `.cargo/config.toml` rather than adding to it, so the obvious `RUSTFLAGS: -D warnings` would silently drop `+crt-static` (D-032) and ship binaries that need the Visual C++ redistributable — precisely the failure that cost hours at the first customer install. Both files say so at the point of temptation.
6. **CI also builds at the declared MSRV (1.85).** That number was wrong once already, in the direction that stops a reader from building the tree at all.

**Found by running it rather than reading it.** `build-mcpb.ps1` took `$ChaperoneRoot` from `$PSScriptRoot` **as a param default** — and `$PSScriptRoot` is empty inside `param()` when a script is invoked as `powershell -File`, which is exactly how CI invokes it. It had worked only because every previous caller dot-invoked it from a shell. Fixed by resolving in the body with an `$MyInvocation` fallback. A latent bug that would have failed the first release on the runner and looked like a CI problem.

**Verified locally.** Full endpoint bundle built and packed on Windows: `mcpb validate` **passes with the new `"license": "Apache-2.0"` key** — a real risk, since the manifest schema is closed and rejects unknown keys (the reason `"//"` comment keys were removed) — and the packed archive contains LICENSE (11.1 kB) and NOTICE (664 B), so D-033's §4(a)/(d) obligation is satisfied in the artifact a user actually receives, not only in the repo. Version-gate script exercised against matching, mismatched, and dry-run refs; both workflow files parse; `build-mcpb.ps1` parses under Windows PowerShell 5.1.

**Not verified, and cannot be from here — see I-011.** Neither workflow has run on GitHub. `pwsh` on the ubuntu runner, `mcpb pack` on Linux, and provenance attestation (which needs a public repo) are all unproven. The Linux endpoint bundle is knowingly the least-exercised artifact we would ship; the raw Linux binary is published alongside it for that reason, and the release notes say so rather than implying parity.

**Made by:** jok. | **Review date:** N/A
**Status:** CURRENT

<a id="d-032"></a>
### D-032 — The executable is the installer; a bind address is not a URL; the CRT ships inside the binary — 2026-08-14

**Context.** The first customer install succeeded but cost hours, and none of the causes were the customer's environment. All three were ours, and all three were the kind of defect that only appears when someone who did not write the code runs it on a machine we have never seen.

**Decisions.**

1. **`chapr-coord.exe` drives installation. No script, in either mode.** `install-coord.ps1` is deleted rather than fixed. It broke on a PowerShell 5.1-vs-7.x difference (assigning to the automatic `$args`, then splatting `@args`), and inspection showed every step it performed — elevation check, `mkdir`, `setup --non-interactive`, `/healthz` self-test, handover — was already in the exe or belonged there. Keeping it meant maintaining two install paths and a whole language runtime in the critical path for no capability. A bare invocation with no arguments now runs the wizard; `serve` still wins when a `coord.toml` is present or there is no console to prompt on, so scripted and SCM paths are untouched. The exe holds its console open on failure when it owns the window, because a double-clicked installer whose error message vanishes with the window is why a wrapper felt necessary in the first place.

2. **`addr` and `public_url` are separate fields, and the confusion is refused rather than warned about.** `print_endpoint_snippet` formatted `cfg.addr` as the client URL, so an administrator who bound `0.0.0.0` — the correct thing to bind — was handed a URL no laptop could use, and one who accepted the loopback default was handed one that reached only itself. `Config::advertised_url()` is now the single source of every client-facing URL, and `validate()` rejects a loopback `public_url` while `addr` listens on a routable interface. Rejected, not warned: the resulting config *looks* finished — the service starts, the admin page works on the box — and the failure surfaces only as every laptop being unable to connect. A deliberate loopback deployment still passes, because there `addr` says so too.

3. **The wizard asks for the share, and offers the machine's own.** `share_unc` was only collected behind the change-watcher prompt, so the handover printed `\\FILESRV\AICollab` as an example while the real value was `\\servername\mappe$`. It is now asked unconditionally and offered as a pick-list from `NetShareEnum`. The filter is the load-bearing part: it excludes `STYPE_SPECIAL` (`C$`, `ADMIN$`, `IPC$`) and **not** a trailing `$`, because a hidden share an administrator created is not administrative — filtering on the `$` would have dropped exactly the share the feature exists to find. Shape is also checked on the unattended path, where a flag value mangled by whichever shell invoked us would otherwise be written straight into the config and become the canonical prefix everything is keyed by (invariant 5).

4. **Static CRT, workspace-wide.** Rust's MSVC target links the C runtime dynamically, so both shipped binaries needed the Visual C++ redistributable — invisible on any developer machine, and a hard stop on a clean Windows Server 2022. `.cargo/config.toml` sets `crt-static` for the whole workspace, not just coord: the endpoint inside the `.mcpb` carried the identical dependency, which is a latent fault we had not met rather than one we had ruled out. Verified externally with `dumpbin /dependents`, including on the binary extracted from the packed bundle. The trade, stated in INSTALL.md rather than hidden: CRT security fixes now arrive with a Chaperone rebuild instead of via Windows Update. For two self-contained programs delivered as single files, being able to run at all on an untouched server wins.

5. **Verification ships in the binary too — `chapr-endpoint self-test`.** The live smoke suites in `examples/` cover more, and can be run nowhere that matters: they need `cargo`, and a sales laptop has an extension and nothing else. The self-test reuses the lib's own `ops`/`read`/`write` paths and reports PASS/FAIL/**SKIP**, where a check that could not run is never a pass — the mapped-drive branch (E-022) is still unconfirmed against a real server, and a green line for a check that did not execute would bury that. Gated on one explicit `args().nth(1)` comparison rather than a `clap` parser, because Claude Desktop launches this binary with no arguments and expects MCP on stdio; nothing may reinterpret the no-argument case.

**Consequence worth carrying forward.** Two of these three defects were invisible to every test we had because they live in *what the software tells a human*, not in what it computes. The handover is now a returned `String` with a test asserting the bind address never appears in it as a URL — the class of guard that could not exist while the function only called `println!`.

**Made by:** jok (all three defects found by using the installer as a customer would) / Claude (the
fixes) | **Review date:** N/A | **Status:** CURRENT *(trailer added 2026-09-07: the entry never
carried one, per the footnote flagged on 2026-08-21. CURRENT by inspection — D-040 confirms it, on
the grounds that deployment "already took its large step in D-032".)*

<a id="d-031"></a>
### D-031 — Admin authority: a token enforces, a role follows; auth changes as a dual-mode cutover — 2026-08-12
**Refines D-029** (annotated there in the same change, per the settled-decision rule). **Branch:** `feat/admin-authority`.
**Trigger:** jok's parked task — configure coord from the browser — makes Settings the system's **first muting admin surface, and the most powerful mutation there is**: it can set `auth = "disabled"`, move the blob store, or rewrite backend routes. Everything shipped before it was read-only, which is precisely what made deferring the admin role honest.
**Problem with D-029 as written.** It said admin is a **role** resolved by the pluggable `Authenticator`. That holds where the auth mode authenticates. It does not hold under `trusted-header` (`auth.rs`), which accepts any non-empty `X-Chapr-Principal` — a role built on that authorizes nothing, it attributes. Gating config-write on it would be gating on a claim.
**Chosen (jok): two layers, not two alternatives.** (1) An **admin token** is the enforced layer now: 256 bits, generated at first start, kept as `admin-token` in the coordinator's data directory. **Its confidentiality is the ACL the installer already applies** (D-029: Administrators + the service account, `Users` denied) — no key store, no hashing, no password policy, nothing new to get wrong. (2) **`admin_principals`** carries D-029's role model forward unchanged, to be enforced when a mode that authenticates is configured (E-015). **Token is permanent break-glass (jok) and cannot be switched off** — it is the way back in when the auth mode is wrong. That leaves "a former administrator may have a copy", answered by **rotation** rather than removal (`POST /admin/token/rotate`).
**Rejected: username/password.** It costs a hash (argon2/bcrypt), strength rules and a forgot-it path, and buys nothing here — the confidentiality story is identical (both live in the ACL'd directory), except a password is something a human must remember or store elsewhere, which is usually worse than a file only administrators can read. A username only earns its place when there are several people to distinguish, and for that `admin_principals` under a real auth mode is the answer, not a local password table.
**jok's idea that fixed a defect in the plan — the dual-mode cutover.** The original plan let you change `auth` and discover a misconfiguration through support calls. `auth` is now **primary + optional fallback**: primary rejects → try the fallback, and the `Authenticator` trait was widened to return `Outcome { principal, mode }` so *which* mode admitted each request is a first-class fact rather than a side channel. An administrator watches the new mode take over and retires the old one.
**And it reversed one of my constraints.** `auth` was on the restart-required list because changing it live is how you lock yourself out. With a credential independent of the auth mode you *cannot* lock yourself out, so that objection falls away — `auth`/`auth_fallback` are the only hot-reloadable fields (`RELOADABLE_FIELDS`). Everything else is wired into something built once at bring-up (bound listener, open pool, watcher OS thread, background tickers) and is honestly reported as pending a restart. **No self-restart** — a clear message beats fragile automation.
**Two findings that changed the design.** (1) **The obvious gate lies.** "Remove the fallback when its usage hits zero" is wrong twice over: a cumulative counter never returns to zero after early use (so the measure is a *rolling* ring), and fallback usage alone cannot distinguish a finished cutover from one where every client is failing — a rejected client never appears in the fallback's count. The gate therefore needs **no recent fallback use AND no recent rejections**, and the panel shows both. (2) **Env overrides would have made the Settings tab lie.** `Config::load` layers `CHAPR_COORD_*` **on top of** the file, so editing an overridden field would save a value silently discarded on the next load. `Config` now records `overridden_by_env`, the UI marks those fields locked, and a save that touches one is **refused** rather than written.
**Also corrected while building:** `GET /admin` must **not** be token-gated — it is where the token is entered, so gating it is a closed loop. Only the data routes are (`/admin/overview`, `/admin/settings`, `/admin/token/rotate`, `/diagnostics/query`, `/leases/query`, `/audit/query`). **`/conflicts/query` deliberately stays ungated** because the endpoint shares it (`chapr.conflicts`), so open conflicts remain readable without a token — unchanged from before, not a new opening.
**Made by:** jok (token-not-password after the argument, permanent break-glass, the dual-mode cutover idea, both-conditions gate) / Claude (the D-029 tension, the rolling-vs-cumulative and rejection-blindness findings, the env-override trap, the `/admin` gating correction) | **Review date:** N/A | **Status:** CURRENT

<a id="d-029"></a>
### D-029 — Admin authority on coord: a role on the Authenticator seam, not OS elevation; data dir gated by installer ACL — 2026-08-12
**Trigger:** the pilot puts coord **directly on the fileserver** (the customer's IT admin's call), so its blob store — which since D-026 holds real file content — sits on a disk the pilot users can already reach, and `GET /blobs/{version}` applies no ACL check (I-002 note 2).
**Problem:** two distinct exposures, easily conflated. (1) **Filesystem** — the blob root and SQLite DB are browsable and deletable by anyone who can read the server's disk. (2) **HTTP** — anyone who can reach coord's port can fetch any blob by version hash, and `/history` hands out the hashes ungated. (2) is the larger hole: it is remote and needs no server access at all.
**jok's proposal:** gate visibility behind **elevation** on the server, and use that as the foundation for a coord dashboard/UI so the CLI stops being the way to monitor and manage coord.
**Chosen (jok, after pushback): split the two gates.**
- **Filesystem → installer-set restrictive ACL.** Blob root + DB owned by Administrators + the service identity, `Users` denied. Not a code change — an installer step. Portable by construction: the Linux equivalent is `chown root:chapr` + `0750`. "The installer locks the data directory" is a reusable primitive, not a per-customer fix.
- **Control plane → an admin *role*, resolved by the existing pluggable `Authenticator` (D-016/D-023)** — **not** elevation.
**Why elevation is the wrong axis for the dashboard (the pushback jok accepted):** (1) Elevation is **local to the coord host**. A dashboard's entire value is not having to RDP into the fileserver; if the admin UI requires an elevated process there, it is a CLI with HTML — the rendering moved, the access did not. (2) It **does not generalize**. Elevation is a Windows UAC concept; on Linux it is root, in a container it is meaningless. With a Linux coord and cloud backends as the north star, expressing authorization as "is this process elevated" makes the dashboard Windows-only forever — the exact per-customer re-architecture the pluggable auth seam exists to prevent. (3) It **conflates two questions**: "who may read blob bytes on disk" and "who may see leases/audit/history and force-release a lease". Elevation answers the first well and the second poorly.
**Shape of the admin role:** deployment *declares* who administers this coordinator — one more field on the same axis as `backend` and `auth`. Principals or an AD group under `trusted-header`, a group SID under `negotiate`, a claim under `oidc`. The wizard asks; nothing customer-specific enters the code. This is the environment-aware-coord model of the north star applied to administration.
**⚠ REFINED BY D-031 (2026-08-12): a role alone is not enough, because under `trusted-header` nothing authenticates.** `TrustedHeaderAuth` accepts any non-empty `X-Chapr-Principal`, so "admin = principal X" authorizes nothing — it attributes. That was tolerable while the admin surface was read-only; it stopped being tolerable the moment the surface could rewrite the config and therefore turn auth off. D-031 keeps this role model **unchanged as `admin_principals`**, to be enforced when a mode that actually authenticates is configured (E-015), and adds an **admin token** as the layer that enforces now. Two layers, not a replacement.
**Sequenced (jok):** I-006 → installer ACL → `Caller` on `GET /blobs/{version}` + `/history` → E-022. The `Caller` step is **attribution, not enforcement**: under `trusted-header` it authenticates nothing, but it makes every blob fetch attributable in the audit trail and puts the two content-bearing routes on the enforcement path *before* E-015, instead of leaving them as a bypass the day auth is switched on. This is I-002's own recommendation applied to the routes that now carry bytes.
**Deliberately not decided yet:** whether the admin-role seam and the read-only query routes (leases / conflicts / audit / history / GC status — **none exist today**) ride the pilot round or wait for the UI. Filed as E-024. **Resolved same day per D-030:** split into E-024a (role + routes, pilot) and E-024b (UI, deferred).
**Refinement found during implementation (commit `cbafd3e`): the blob route cannot be attributed *in the audit trail*, as this entry assumed.** `AuditEvent` is keyed by `(canonical_path, session_id)` and a `GET /blobs/{version}` fetch has neither — the version is content-addressed and **deduplicated**, so one hash may name bytes shared by several paths, and no session travels on the request. Forcing it would mean inventing a path, which is the opposite of what an audit trail is for. So the gate landed as **`Caller` + a structured log line**, and durable attribution for content reads moves to **E-026**'s diagnostics/access store, whose record shape actually fits. The enforcement-path half of the decision (the routes are gated *before* E-015) is delivered in full.
**Made by:** jok (both gates, the sequence, the `Caller` call) / Claude (the elevation-vs-role argument, the HTTP-exposure half, the audit-shape refinement) | **Review date:** N/A | **Status:** CURRENT

<a id="d-025"></a>
### D-025 — Deployment packaging + two-repo split (Chaperone + the customer deployable) — 2026-07-22
**Problem:** Session end-state = MCPB endpoint bundles ready + coord setup material ready + an install/use guide. The GitHub structure needed deciding.
**Decisions (jok):**
- **Two repos.** **Chaperone** = source + **reusable** packaging tooling + generic deployment guide (the "move to another customer" material). **the customer deployable** = this customer's **concrete deployable** (Windows MCPB + on-prem Windows coord). the customer deployable is a sibling folder now, to become its own repo (git not yet initialised — human's step).
- **Build depth:** produce materials + real release binaries + assemble the bundle; the human runs `mcpb pack` + the Claude Desktop install test.
- ~~**Binaries not committed** — build scripts pull them from the Chaperone build (source-light repo).~~ **REVISED 2026-07-22 (jok):** for usability, the customer deployable is now **self-contained** — the deployable binaries (`coord/chapr-coord.exe`, `endpoint/chaperone-endpoint.mcpb`) **are committed**, so a clone/copy runs on a bare on-prem server with no Rust/source. `.gitignore` still excludes build scratch, runtime data (`*.db`/`blobs`/`tls`) and signing keys. `install-coord.ps1` resolves the binary beside the script first. (Reason: a git clone was otherwise non-functional on the coord host — the coord deliverable is exe+config+script together, unlike the endpoint whose `.mcpb` is self-contained.) Rebuild + re-commit binaries when the source changes.
**Built + verified:** `Chaperone/packaging/mcpb/` (manifest.template.json + build-mcpb.ps1 + README), `Chaperone/packaging/coord/` (config.template.toml + service-install.md), `Chaperone/docs/deployment-guide.md`; `<customer-repo>/endpoint/` (manifest.json + build.ps1 + .gitignore), `<customer-repo>/coord/` (coord.toml + install-coord.ps1 + .gitignore), `<customer-repo>/INSTALL.md` + README.md. Endpoint MCPB = `type:binary` server; the **only** user-config is `coord_url` (identity auto-derived, D-024). **Verified:** release build green; the customer deployable endpoint bundle assembles (manifest + exe); **`mcpb validate` passes** (manifest_version 0.3, official CLI); coord `setup --non-interactive` flags used by `install-coord.ps1` produce a correct trusted-header/smb config + endpoint snippet.
**Left to the human:** `mcpb pack` + Claude Desktop install test; the real coord service install (needs elevation); `git init` + push of the two repos.
**Made by:** jok (calls) / Claude (impl) | **Review date:** N/A | **Status:** CURRENT

<a id="d-024"></a>
### D-024 — Coord host = on-prem Windows (confirmed); MVP identity = zero-setup ambient OS identity, enforced auth deferred — 2026-07-22
**Concept doc updated in the same change:** §13.1 (MVP note).
**Problem:** With E-015 about to be built as enforced OIDC, jok pushed back on **setup overhead vs. product intent**: Chaperone is a **collaboration / sync engine that *values* security + traceability — not a security tool**. The target customer **owns the on-prem Windows server** and is accessed by **internal users**; an install requiring Entra app registration + MSAL + a multi-step guide disrespects that infrastructure and over-engineers a simple use case. The audit trail's purpose here is **accountability** ("a user is responsible for their agents"), **not** court-grade non-repudiation.
**Decisions (jok):**
- **Coord host for this project = on-prem Windows — CONFIRMED.** Resolves the standing host question (was Opt 2 vs Opt 3).
- **MVP identity = ambient OS identity, zero setup.** The endpoint derives the logged-in Windows principal automatically and presents it; coord runs `trusted-header` mode (D-016) and stamps it into the audit trail. No token input, no app registration, no cert — install ergonomics ≈ "here's your fileserver directory." **Honest-but-unenforced** (a tampered endpoint could assert any principal) — **accepted + documented as an intentional MVP limitation** (in the spirit of §13.2), because this is a v0.1 MVP for cooperating internal users.
- **Enforced auth is deferred, NOT dropped — and stays pluggable.** The D-016 `Authenticator` seam makes the mechanism a per-deployment config choice, which is exactly what stops auth from becoming a per-customer re-architecture headache: `trusted-header` (this MVP) · `negotiate` (on-prem Windows hardening — reuses the same Kerberos as the share, still zero user setup) · `oidc` (cloud/hybrid). Q1 (`jsonwebtoken` validator) + Q3 (`oid`+display audit identity) from this session are **banked** for the `oidc` branch.
**Consequences:** E-015 (enforced auth) → **deferred** to production-hardening / security-requiring customers. New small item **E-023** = the zero-setup OS-identity endpoint change (coord side already exists). Overengineering avoided; the collaboration core is untouched by the auth choice. Refines D-023 (which stays valid — this picks the seam's simplest mode for the MVP stage).
**Made by:** jok (calls) / Claude (analysis) | **Review date:** N/A | **Status:** CURRENT

<a id="d-023"></a>
### D-023 — Control-plane auth: pluggable, generic OIDC (revises §13.1 "no OAuth") — 2026-07-22
**Concept doc updated in the same change:** §13.1 + §2.
**Problem:** E-015's settled plan was Negotiate/Kerberos SPNEGO on the control channel ("no OAuth", §13.1). The target customer is **on-prem Windows Server SMB inside an Entra-managed / hybrid environment**; no test AD domain is available, but jok has an Azure tenant with rights to stand up a small fileserver. jok asked: does committing to Entra token auth make the control channel a Microsoft-only one-trick pony?
**Key clarification (the crux):** two channels, two identity stories (invariant 6). **Data path** (endpoint→SMB) = **OS** Kerberos, issuer-agnostic (on-prem AD / Entra DS / Entra Kerberos) — **no Chaperone auth code**. **Control channel** (endpoint↔coord) is the only place Chaperone authenticates; the **validator on coord** is what must be trustworthy for the audit trail (the I-001 spoofable-principal problem). So the pluggable thing is the validator.
**Decision (jok):**
- Control-plane auth stays **pluggable** (the existing D-016 `Authenticator` boundary); **no single IdP baked in**, selected by `CHAPR_COORD_AUTH`.
- Primary production mechanism = a **generic OIDC JWT validator** on coord: `issuer` / `jwks_uri`(discovery) / `audience` / principal-claim (`upn`/`oid`/`email`/…) all from config. Entra is **one configured issuer**; any OIDC IdP (Okta, Keycloak, Auth0, Ping, …) works by config, because validation is standard OIDC — genericity is free there.
- Token **acquisition** on the endpoint is provider-specific, behind a `TokenSource` seam (Entra **WAM/MSAL** built first; others later, YAGNI).
- `negotiate` retained for pure Kerberos realms; `trusted-header`/`disabled` for dev.
**Consequences:** (1) a token-validating coord need not be domain-joined/in-realm → **Linux/cloud coord can authenticate on-prem endpoints** (aligns D-019 Opt-4). (2) **E-015 is no longer blocked on an AD domain** — the OIDC flow is buildable/testable in jok's Azure tenant now. (3) Data-plane needs no Chaperone auth code — just confirm the OS Kerberos read works. (4) The on-prem customer visit is for validating **SMB mandatory-lock semantics** (`CreateFileW share=NONE`) + the end-to-end token flow, **not** for building auth. **Caveat:** Azure Files SMB ≠ on-prem Windows Server SMB for edge lock/oplock/DFS behaviour — the mandatory-lock guarantee must be proven on the real target.
**Made by:** jok (calls) / Claude (analysis) | **Review date:** N/A | **Status:** CURRENT

<a id="d-019"></a>
### D-019 — Deployment + backend-agnostic roadmap (brainstorm outcome) — 2026-07-21
**Full detail:** `~/.claude/plans/giggly-dancing-zebra.md` (deployment shortlist, wizard design, adapter model).
**Decisions locked (jok):**
- **Endpoints:** MCPB (settled).
- **Coord deployment:** near-term per-customer = Windows Service (Opt 2) or Linux/watcher-off (Opt 3, pending the customer's use-case answer); **north star = Opt 4** (Linux coord + per-backend *push* watcher). Key finding: coord is already OS/backend-agnostic bar the watcher, and the watcher is non-correctness (composite-key index → out-of-band edit = cache miss), so Linux coord is viable now.
- **Backend-adapter model (V2→V3):** endpoint `Backend` trait + `Capabilities`; coord unchanged. Forks: **backend-opaque `VersionToken`** (BLAKE3 for synth, ETag for cloud — relaxes invariant 2); **uniform coordination except the journal** (capability-gate the journal via `atomic_writes`; `write_cas` per-backend by necessity; audit/history/conflict/lease stay uniform); **accept POSIX advisory `flock`** (fileserver trade-off, document it). Sequence: refactor SMB behind the trait → POSIX → cloud (S3/Azure/Graph). Frame: coordination value ∝ 1/native-caps; governance/history/audit/injection layer always additive.
**Made by:** jok (calls) / Claude (analysis) | **Review date:** N/A | **Status:** CURRENT

<a id="d-018"></a>
### D-018 — E-016: coord installer + service + TLS — 2026-07-21
**Problem:** Make coord installable/operable on-prem (the "easy win" of the deployment brainstorm).
**Built:** `chapr-coord` CLI (`serve` default / `setup` / `run-service`[win]); `config.rs` — TOML config with **defaults → file → env** precedence (env-lookup injected for testable precedence); `run_server(cfg)` extracted from `main`; **rustls TLS** via `axum-server bind_rustls` when `[tls]` set (endpoint `reqwest` gained `rustls-tls`); `setup.rs` wizard (interactive dialoguer + unattended flags, host probe, TOML write, service install, `/healthz` self-test, endpoint snippet); `service_win.rs` native Windows SCM (`windows-service`) + systemd unit on Linux. **Deferred:** cert generation (wizard collects paths), real Negotiate/Kerberos (E-015), rich TLS self-test.
**Verified:** coord 72 tests, clippy clean; live — non-interactive `setup` → TOML → `serve --config` → `/healthz`; self-signed cert → HTTPS `/healthz`. Windows SCM install needs elevation (best-effort).
**Made by:** jok (design forks) / Claude (impl) | **Review date:** N/A | **Status:** CURRENT

<a id="d-016"></a>
### D-016 — I-001/I-002: pluggable auth boundary — 2026-07-21
**Problem:** Coord trusted a body-supplied `principal` (I-001, spoofable → corrupts the audit trail); endpoint↔coord channel unauthenticated (I-002). Full Kerberos can't be tested without an AD domain (human chose the pluggable-boundary approach).
**Choice:** `auth` module — an `Authenticator` trait + a `Caller` axum extractor. Impls: `DisabledAuth` (default; falls back to body principal — existing tests unaffected), `TrustedHeaderAuth` (dev: trusts `X-Chapr-Principal`, set by the endpoint's `CoordClient`), `NegotiateAuth` (SSPI/SPNEGO placeholder, domain-only). **All principal-bearing handlers** (acquire/record_audit/register_conflict/resolve_conflict/recover/move/open_journal/append_version_log) now use `caller.0.unwrap_or(body_principal)` — the authenticated identity wins, closing I-001. Selected via `CHAPR_COORD_AUTH`. Live-verified: with auth on, the audit trail attributes to the connection identity, not the body; missing identity → 401. **Residual (E-015): real Negotiate/Kerberos + TLS on the transport** (needs a domain).
**Made by:** jok (approach) / Claude (impl) | **Review date:** N/A
**Status:** CURRENT

