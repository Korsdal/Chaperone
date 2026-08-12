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
- To build: the Rust toolchain (`cargo`) and, to pack MCPBs, Node + `@anthropic-ai/mcpb`.

## Step 1 — Coordinator
1. Build: `cargo build --release -p chapr-coord`.
2. Configure + install as a service — see [`../packaging/coord/service-install.md`](../packaging/coord/service-install.md)
   (`chapr-coord setup`). Start from [`../packaging/coord/config.template.toml`](../packaging/coord/config.template.toml).
3. Choose the **backend** coord announces (`smb`/`posix`) and the **auth** mode
   (`trusted-header` for the MVP — zero end-user setup; `negotiate`/`oidc` to
   harden later, E-015).
4. Verify: `GET /healthz` → `ok`.

## Step 2 — Endpoints (MCPB)
1. Build one bundle **per client OS** — see [`../packaging/mcpb/README.md`](../packaging/mcpb/README.md).
   Identity is auto-derived from the OS logon, so the only user-config field is the
   coordinator URL.
2. Distribute the `.mcpb`. Users install it in Claude Desktop
   (Settings → Extensions) and enter the coordinator URL once.
3. On first connect the endpoint reads coord's backend announcement, confirms it
   against its own capabilities, and selects the matching backend.

## Step 3 — Verify end to end
- From a laptop: `chapr.read` a file on the share → returns content + a version.
- `chapr.write` it back with that version → succeeds; a stale write → CONFLICT +
  sidecar (bytes never lost).
- `chapr.history` shows the version log; the coord audit trail attributes each
  action to the acting user.

## When something breaks

Two places to look, and you need both — the second exists because a failure that
happens *before* coord is reachable cannot report itself to coord.

1. **Coord, for the whole fleet.** `POST /diagnostics/query` with `{}` returns
   unexpected failures grouped by `(code, path)`, newest first, each with a
   `remedy` field saying what to do, `facts` carrying the OS-level detail, and the
   users and hosts that hit it — which is how you tell one misconfigured laptop
   from a fault hitting everybody.
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
- [ ] Client OS(es) → which MCPB bundle(s) to build
- [ ] Coordinator URL baked into the bundle default / comms to users
- [ ] **Coordinated root** (`CHAPR_ROOT` / the bundle's "Coordinated location"):
      the share path, as UNC. Bounds what the endpoint will touch, and is
      announced to the model so it routes writes through Chaperone. Users on
      mapped drives are fine — a drive letter is resolved to its UNC form, so
      two laptops with different letters still key one file identically.
- [ ] TLS? (set `[tls]`)
- [ ] Backup + monitoring wired
