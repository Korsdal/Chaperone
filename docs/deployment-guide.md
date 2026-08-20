# Chaperone — deployment guide (generic)

How to stand Chaperone up at a customer. This is the reusable playbook; a concrete
per-customer instantiation (binaries filled in, one OS, one coord) lives in that
customer's own deployable repo.

## The shape

Two artifacts, N-to-1:

- **Coordinator** — one per environment, on-prem beside the fileserver. A single
  binary (`chapr-coord`) + SQLite + a blob store. Owns leases, version index,
  journal, history, conflicts, audit. Does **no** file I/O.
- **Endpoints** — one MCPB per laptop, a stdio child of Claude Desktop. Does file
  I/O as the logged-in user; owns the write path, CAS, read state machine.

File **bytes** go endpoint → fileserver directly; **metadata** goes endpoint ↔
coord over HTTP. The one deliberate exception is the **pre-image snapshot**: a
write sends the bytes it is about to replace to coord's blob store, because that
snapshot is what history and crash recovery are made of (D-026). It caps the
largest writable file at 256 MiB. The 200 MB PDF a model *reads* still never
touches coord.

## Prerequisites
- A shared fileserver: **SMB** (Windows) or **POSIX** (Linux/NFS).
- A host for the coordinator that can reach the share's network and that laptops
  can reach over HTTP(S).
- Claude Desktop on each laptop.
- To build from source: the Rust toolchain (`cargo`) and, to pack MCPBs, Node +
  `@anthropic-ai/mcpb`. Not needed if you take the published
  [Releases](https://github.com/Korsdal/Chaperone/releases) — the binaries there are built by CI
  from a tag, with `SHA256SUMS` alongside them.

## Step 1 — Coordinator
1. Get the binary: download `chapr-coord-<version>-<os>` from the project's
   [Releases](https://github.com/Korsdal/Chaperone/releases) page (checksums in `SHA256SUMS`), or
   build it with `cargo build --release -p chapr-coord`. The result is one self-contained
   `.exe` — no Visual C++ redistributable, no Rust on the target host, no script.
2. Copy it to the coordinator host and run it **elevated** with no arguments. That
   *is* the installer: a bare invocation runs the setup wizard. See
   [`../packaging/coord/service-install.md`](../packaging/coord/service-install.md);
   [`../packaging/coord/config.template.toml`](../packaging/coord/config.template.toml)
   documents every field if you would rather write the config by hand.
3. Answer four questions: **listen address**, **hostname the laptops connect to**,
   **share to coordinate**, and whether to run the change-watcher. The first two are
   separate on purpose — a bind address is not a URL (D-032).
4. Choose the **backend** coord announces (`smb`/`posix`) and the **auth** mode
   (`trusted-header` for the MVP — zero end-user setup; `negotiate`/`oidc` to
   harden later, E-015).
5. Read the handover it prints. It is the whole set of things to pass on: the admin
   token, the coordinator URL, the coordinated share, where both logs live, and what
   to back up.
6. Verify: `GET /healthz` → `ok`, **from a laptop** rather than from the coordinator
   itself. Loopback working proves nothing about what a user will experience.

## Step 2 — Endpoints

The endpoint is a standard MCP server over stdio, so the install route depends on the
**host**, not on the fileserver. Both routes ship the same binary and behave identically;
identity is auto-derived from the OS logon either way.

**Route A — Claude Desktop (one-click).**
1. Get one bundle **per client OS**: download `chaperone-endpoint-<version>-<os>.mcpb` from
   [Releases](https://github.com/Korsdal/Chaperone/releases), or build it — see
   [`../packaging/mcpb/README.md`](../packaging/mcpb/README.md). A packager who builds it can
   pre-fill the coordinator URL and coordinated location as defaults, so users type nothing;
   released bundles ship without those defaults, so users enter them once.
2. Distribute the `.mcpb`. Users install it in Claude Desktop
   (Settings → Extensions) and enter the coordinator URL once.

**Route B — Claude Code, or any other MCP host.** `.mcpb` is Desktop's install format, so
elsewhere the artifact is the bare `chapr-endpoint-<version>-<os>` binary.
1. Put the binary somewhere stable (it is self-contained — no runtime, no redistributable).
2. Set `CHAPR_COORD_URL` and `CHAPR_ROOT`, then let the binary print its own registration:
   `chapr-endpoint print-config claude-code` for a `claude mcp add` one-liner, or
   `chapr-endpoint print-config generic` for the portable `mcpServers` JSON block —
   which is also what Claude Code reads from `.mcp.json`. The config goes to stdout
   alone, so it can be redirected straight into a file.
3. For a fleet, do step 2 once and distribute the resulting block; the only
   machine-specific part is the binary's path.

Either way, on first connect the endpoint reads coord's backend announcement, confirms it
against its own capabilities, and selects the matching backend.

Only Claude Desktop and Claude Code have been driven end to end. Other MCP hosts should work
— nothing in the tool surface is vendor-specific — but they are not tested here.

## Step 3 — Verify end to end
- From a laptop: `chapr.read` a file on the share → returns content + a version.
- `chapr.write` it back with that version → succeeds; a stale write → CONFLICT +
  sidecar (bytes never lost).
- `chapr.history` shows the version log; the coord audit trail attributes each
  action to the acting user.

## The admin page

`GET /admin` on the coordinator — the same host and port the laptops use. Served
by coord itself, so there is nothing extra to install and it works on a network
with no route out.

Six tabs: **Overview** (counts, plus what this coordinator is configured for),
**Errors**, **Conflicts**, **Leases**, **Audit trail**, **Settings**.

**Signing in.** The page asks for the coordinator's admin token, kept as
`admin-token` in the coordinator's data directory — the directory beside the
database, which the installer restricts to administrators and the service account.
The setup wizard prints it; you can read the file again at any time.

The token deliberately does **not** depend on the connection auth mode. That is
what makes changing the auth mode safe: a wrong setting cannot lock you out of the
page you need in order to undo it. It cannot be switched off for the same reason,
so **rotation** (a button on the Settings tab) is how you answer "someone who has
left may still have a copy".

**Changing the connection auth mode.** Do it as a cutover, not a switch:

1. Set the new mode as the primary and keep the old one as the **fallback**.
   Both apply immediately; no restart.
2. Watch the Settings tab. It shows which mode has been admitting the recent
   requests, and how many have been rejected.
3. Remove the fallback when the page says it is safe — which needs **both** no
   recent fallback use **and** no recent rejections. One without the other cannot
   tell a finished cutover from one where every client is simply failing: a client
   that cannot authenticate never appears in the fallback's own usage count.

Other settings save to the config file. Only `auth` and `auth_fallback` take effect
without a restart; the page says which of your changes are live and which are
waiting, rather than implying everything reloads. A field held by a
`CHAPR_COORD_*` environment variable is shown as locked — saving it would write a
value the environment discards on the next start.

## When something breaks

Two places to look, and you need both — the second exists because a failure that
happens *before* coord is reachable cannot report itself to coord.

1. **Coord, for the whole fleet.** The admin page's Errors tab, or
   `POST /diagnostics/query` with `{}` for the same data as JSON: unexpected
   failures grouped by `(code, path)`, newest first, each with a `remedy` field
   saying what to do, `facts` carrying the OS-level detail, and the users and
   hosts that hit it — which is how you tell one misconfigured laptop from a fault
   hitting everybody.
2. **The laptop, for that laptop.** The endpoint appends the same records as JSON
   lines to `%LOCALAPPDATA%\Chaperone\diagnostics.jsonl` (override with
   `CHAPR_DIAG_LOG`). Check here for a wrong coordinator URL, a TLS mismatch or a
   blocked port — none of which can be reported over the network they break.

**Only unexpected failures land there.** A CAS conflict, a document a human has
open in Word, a busy file — those are designed outcomes, not faults, and they
surface as conflict and lease state instead. If the diagnostics list is empty,
that is the intended steady state.

## Operations
- **Availability:** coord availability == write availability. Run it as a service,
  monitor `/healthz`.
- **Backup:** the SQLite DB **and** the blob store, together (audit + history).
- **Storage:** history retention defaults (90 d / last-10 / 50 GB) are tunable;
  validate against real write volume after a pilot.

## Per-customer checklist
- [ ] Backend type (SMB / POSIX) → coord `backend`
- [ ] Coord host + address + persistent DB/blob volumes
- [ ] Auth mode (MVP `trusted-header` vs enforced)
- [ ] Admin page URL passed to whoever supports this (`<coord>/admin`)
- [ ] Admin token handed over (printed by `setup`; also `admin-token` in the data
      directory). Rotate it when someone with access leaves.
- [ ] Client OS(es) → which MCPB bundle(s) to build
- [ ] Coordinator URL baked into the bundle default / comms to users
- [ ] **Coordinated root** (`CHAPR_ROOT` / the bundle's "Coordinated location"):
      the share path, as UNC. Bounds what the endpoint will touch, and is
      announced to the model so it routes writes through Chaperone. Users on
      mapped drives are fine — a drive letter is resolved to its UNC form, so
      two laptops with different letters still key one file identically.
- [ ] TLS? (`chapr-coord setup --tls-generate` makes a self-signed pair if there is no
      internal CA — it still has to be trusted on the laptops)
- [ ] Backup + monitoring wired
