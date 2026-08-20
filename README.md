# Chaperone

[![CI](https://github.com/Korsdal/Chaperone/actions/workflows/ci.yml/badge.svg)](https://github.com/Korsdal/Chaperone/actions/workflows/ci.yml)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)

Filesystem coordination for concurrent Claude Desktop agent sessions working against a single
on-prem fileserver.

The problem it solves is **lost updates**: two agent sessions doing read-modify-write on the same
file, where the second write silently discards the first. Alongside that, Chaperone makes every
agent-initiated change **attributable** (stamped with the AD principal that caused it) and
**recoverable** (content-addressed history, restore-to-copy).

The workload is read-heavy by design — agents read large materials (PDFs, tenders, proposals) and
write rarely, into smaller derived artifacts.

**Framing:** Chaperone is a collaboration/sync engine that values security and traceability — not a
security tool. The audit trail proves a user is responsible for their agents (accountability), not
court-grade non-repudiation.

Tool namespace: `chapr.<method>` — `chapr.read`, `chapr.write`, and so on.

## Shape

Two deployable artifacts, N-to-1:

- **`chapr-endpoint`** — one per user laptop, run as a stdio child of Claude Desktop. Does file I/O
  **as the logged-in user** (Kerberos on SMB), and owns the exclusive-open write path, the CAS
  check, the in-place write, the lease-renewal thread, and the read state machine.
- **`chapr-coord`** — exactly one, on-prem beside the fileserver. Stateful. Owns the lease table,
  version index, intent journal, history/blob store, conflict registry, audit log, and the
  change-watcher. Does **no file I/O of its own**.

Two channels that never cross: **file bytes** go endpoint → fileserver directly; **metadata**
(leases, versions, journal, audit) goes endpoint ↔ coord over HTTP+SSE. The 200 MB PDF never flows
through coord.

Auth is a pluggable boundary. The MVP asserts the logged-in OS identity (`trusted-header`) so
install requires no tokens and no prompts; Negotiate/OIDC are the hardening paths.

## Load-bearing invariants

These are assertions, not preferences. Getting them backwards loses data.

1. **Ground truth is on the share, never in coord.** Coord caches; every write re-derives the
   version from the file *under the lock* before trusting anything coord said.
2. **Version token = content hash.** `version = BLAKE3(file_bytes)`. One hash serves as CAS
   change-detector, history store key, and audit chain link.
3. **Exclusive-open + CAS is the correctness core; leases are only an optimization.** Correct with
   lock+CAS and no leases. *Not* correct with leases and no CAS. Never invert this.
4. **Version-check and write share one file handle.** Hash-then-reopen-to-write is a TOCTOU race.
   Everything from version-check to write-close happens under one held exclusive handle.
5. **All coordination state is keyed by canonical path** (DFS-resolved, NFC, casefolded, UNC,
   normalized separators). Two users naming a file differently must map to the same lease.
6. **File bytes reach the model directly; coord sees bytes only for history.** The 200 MB PDF a
   model reads never goes through coord — endpoint → share → model. A write *does* send the file's
   previous contents to coord (`PUT /blobs`), because that pre-image snapshot is what history and
   crash recovery are made of. That is the only byte flow on the control channel, it is one
   direction, and it is bounded explicitly by `http::MAX_BLOB_BYTES` (256 MiB). Both ends buffer
   whole, so that bound is also coord's per-in-flight-write memory cost — and the largest file
   Chaperone can write at all, since a write whose pre-image will not fit is refused up front.
   The bound is stated on the *route* as well as on the types: an invariant enforced on type shape
   alone let an unsized raw-body channel exist unnoticed.

Failure directions, likewise deliberate: coord unreachable on **write** → fail-closed (refuse);
coord unreachable on **read** → degrade-open (serve with `integrity = "unverified"`). A torn file
recovers to its pre-image before any reader sees it. A CAS conflict writes the loser's bytes to a
`.conflict-{user}-{ts}` sidecar and registers it — never lose either party's bytes, never fake-merge
an Office binary. An Office lock file (`~$F`) present refuses the write: humans always win.

The write path is the one place where a subtle mistake costs someone their data. It is intentionally
the most boring, linear, synchronous-looking code in the repo, and should stay that way. Writes are
in-place, never temp-then-rename — a rename carries the source ACL and strips the target's ACEs.
Crash safety comes from the journal plus the snapshot, not from an atomic rename.

### Read limits, and what a model can write back

Two independent limits, deliberately not one number:

- **`DEFAULT_MAX_INLINE_BYTES`** (1 MiB, override with `CHAPR_MAX_INLINE_BYTES`) is a *context*
  limit — how much of a file usefully enters the model's input window. Over it, the read is refused
  rather than truncated: a silently shortened body written back destroys the file's tail. The refusal
  is a **tool-level** result, not a protocol error, and it says what the caller can do instead —
  raising the cap is an operator action on that machine, so it is phrased as something to pass on
  rather than something to attempt.
- **`WRITEBACK_BUDGET_BYTES`** (128 KiB) is what a model can realistically echo back through
  `chapr_write` in one call. It refuses nothing; it reports `writable_inline=` in the envelope
  header, so a body too large to write back is still served for analysis while the model is told
  in-band to put its output in a separate, smaller file.

Collapsing these into a single cap makes reads as restrictive as writes, which is backwards here:
the share is read-heavy over large materials and writes go into smaller, *different* derived
artifacts.

**Binary content is returned as base64, not extracted.** A non-UTF-8 file (xlsx, docx, pdf) comes
back base64-encoded with `encoding=base64` in the envelope, so byte-exact round-trips are safe — but
there is no text extraction, and a compressed PDF is not analysable in that form at any size. Reading
tender PDFs *as documents* is therefore not something `chapr_read` delivers today; `ReadContent::Ref`
is defined in the proto for this and is not yet produced anywhere. Chaperone coordinates the files;
getting a PDF's text in front of a model is a separate problem.

In practice it is solved **upstream**: the workflow extracts each PDF, spreadsheet and document to a
text mirror first, and the model reads those. That is why the inline cap is sized for one extracted
document rather than for a source PDF — and why the cap, not the base64 path, is the limit that
actually matters day to day.

## Layout

A Cargo workspace of three crates, built in this order:

| Crate | Role |
| --- | --- |
| `crates/chapr-proto` | Shared wire contract — records, IDs, version token, the `chapr.*` request/response types, and an exhaustive error enum. Both binaries import it, so neither can drift. |
| `crates/chapr-coord` | Coordination service. `axum` + `sqlx`/SQLite, background jobs (lease reaping, journal sweep, blob GC), setup wizard, native service install. No Windows-specific primitives in the core. |
| `crates/chapr-endpoint` | Local MCP server. `rmcp` over stdio, pluggable filesystem backends, lease-renewal thread, read state machine. |

Also here: `packaging/` (MCPB bundle builder for the endpoint, coord config template and service
install notes) and `docs/deployment-guide.md`.

Chaperone is the reusable engine. Customer-specific deployables — which commit built binaries — live
in their own repos.

### Backends

The endpoint selects a backend at runtime (`CHAPR_BACKEND`), against a shared write-path core:

- **`smb`** — Windows. `CreateFileW` with `FILE_SHARE_NONE` for the exclusive open,
  `ReadDirectoryChangesW` for the watcher.
- **`posix`** — Linux. Advisory `flock`.

The long-term direction is that coord *announces* what the environment is and endpoints confirm it
against their own local capabilities, selecting a matching backend. Local knowledge is
authoritative.

## Install

Every release attaches built binaries, so trying Chaperone does not require a Rust toolchain on a
fileserver:

**[Releases →](https://github.com/Korsdal/Chaperone/releases)**

| Artifact | What it is |
| --- | --- |
| `chapr-coord-<ver>-windows-x86_64.exe` / `-linux-x86_64` | The coordinator. Run it with **no arguments** — the executable *is* the installer, and a bare invocation runs the setup wizard. |
| `chaperone-endpoint-<ver>-windows-x86_64.mcpb` | The endpoint, as a one-click Claude Desktop bundle (Settings → Extensions). |
| `chaperone-endpoint-<ver>-linux-x86_64.mcpb` + raw binary | Same for Linux. The raw binary is published too, because Claude Desktop's Linux story is thin and a bare binary wires into any MCP client. |
| `SHA256SUMS` | `sha256sum -c SHA256SUMS`, or `Get-FileHash` on Windows. |

Releases are built by GitHub Actions from a tag, not from someone's laptop, and are
provenance-attested. The `.mcpb` bundles are **unsigned** — signing is broken upstream in the mcpb
CLI, so Claude Desktop reports every bundle as unsigned regardless; accountability rests on the
audit trail.

Building it yourself is three words of `cargo` (below) — the binaries exist because asking a DBA to
install a Rust toolchain on a production fileserver to evaluate a tool is a rude way to say hello.

## Build

Requires **Rust 1.88+**. That is the highest `rust-version` in the locked dependency graph, not an
estimate: `darling` 0.23 and `time` 0.3.55 set the floor, with the `icu_*` 2.2 crates at 1.86 just
under it. The number is measured with `cargo metadata` and enforced by CI, because it has twice been
declared too low and each time the person who found out was someone trying to build the tree.

```sh
cargo build --workspace
cargo test  --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Building on Linux alongside a Windows checkout: set an isolated target dir
(`CARGO_TARGET_DIR=$HOME/chapr-target`) so the two toolchains don't clobber each other. The endpoint
compiles on Linux because the `windows` dependency and the SMB backend are `cfg(windows)`-gated.

The Windows builds link the CRT statically (`.cargo/config.toml`), so the shipped
binaries need no Visual C++ redistributable — the dynamic default stopped a
coordinator from starting on a clean Windows Server 2022.

Coord setup and service install:

```sh
chapr-coord                            # no arguments = the setup wizard
chapr-coord setup --non-interactive …  # unattended, for fleet rollout
chapr-coord serve --config coord.toml
```

The coordinator's own executable is the installer: there is no script to run, and a
bare invocation runs the wizard unless a `coord.toml` is present or there is no
console to prompt on. Note that `addr` (where the socket binds) and `public_url`
(what a laptop connects to) are two different settings — conflating them is how an
administrator once came to be told to configure `http://127.0.0.1:8787` fleet-wide.

See `packaging/coord/` for the config template and service install steps, and `packaging/mcpb/` for
building the endpoint bundle. Note that `build-mcpb.ps1` writes to `./build` by default, which is
git-ignored.

### Releasing

CI (`.github/workflows/ci.yml`) runs those three commands on Windows and Linux for every push and
PR, plus a build on the toolchain `Cargo.toml` declares as the MSRV — read from that field rather
than pinned in the workflow, so the two cannot disagree.

A release is a tag:

```sh
# 1. bump [workspace.package] version in Cargo.toml — a human decision, see Contributing
# 2. commit it
git tag v0.2.0 && git push origin v0.2.0
```

`.github/workflows/release.yml` then tests, builds, and packs on both OSes, generates `SHA256SUMS`,
attests provenance, and opens a **draft** release for a human to publish. It refuses to run if the
tag does not match the workspace version — the version bump is the decision, and the tag only
records it. `workflow_dispatch` runs the same pipeline without creating a release, for proving a
change to the packaging before spending a tag.

## Contributing

Issues and PRs are welcome. Contributions are inbound=outbound — anything you send in is licensed
under the same Apache-2.0 terms (section 5 of the licence). There is no CLA.

Before changing anything, three things about this codebase are worth knowing, because they are not
guessable from the code:

**The invariants above are assertions, not preferences.** They are listed under "Load-bearing
invariants" for a reason: getting one backwards loses somebody's file. In particular, leases are an
*optimization* — exclusive-open plus CAS is the correctness core. Code is correct with lock+CAS and
no leases, and *not* correct with leases and no CAS. If a change appears to let you skip the CAS
re-hash, the change is wrong.

**The write path is deliberately boring.** Everything from version-check to write-close happens
under one held exclusive handle, in straight-line blocking I/O inside `spawn_blocking`. It reads as
unfashionably synchronous and repetitive, and that is the design: correctness *ordering* matters
more than I/O concurrency on a single file. Be clever elsewhere. A PR that makes the write path
more elegant is the one most likely to be declined.

**Versioning is a human decision.** Nothing automated bumps `[workspace.package] version` — not a
tool, not CI, not an agent. The release workflow refuses a tag that does not match the version in
the tree, precisely so that the bump has to be a deliberate act by a person. Propose a version in
a PR; don't set one.

### A note on `D-nnn` / `E-nnn` / `I-nnn`

Comments throughout the source cite identifiers like `D-032`, `E-016`, or `I-005`. These index an
internal engineering logbook — decisions, work items, and issues — which is **not published**: it
is written for an internal audience and names customers, collaborators' internal tooling, and
specific share layouts.

You are not missing context you need. Each reference is provenance, not a pointer you have to
follow: the comment carrying it states the reasoning in full, which is the convention those
comments are written to. If you hit one that does not stand on its own, that is a documentation
bug worth reporting — the comment should be self-contained.

## Status

Version **0.1.0**. The v1 tool surface is complete and live-verified end to end: read · write ·
create · list · stat · delete · move/rename · history · conflicts · resolve_conflict — 10 MCP tools
against 11 coord routes.

All three crates build and test green on **Windows and Linux**, clippy clean at `-D warnings` on
both, with the live smoke suite passing on both the SMB and POSIX backends. The endpoint packs as an
MCPB bundle and installs in Claude Desktop.

Known remaining work: real Kerberos/Negotiate on the control channel (needs a domain to develop
against), DFS and drive-letter→UNC canonicalisation before real-SMB rollout, and validating SMB
mandatory-lock semantics on an actual fileserver — the one thing a dev environment can't stand in
for. MCPB signing is broken upstream, so the MVP ships unsigned; accountability rests on the audit
trail. Delivering a large PDF's *content* to a model is unsolved (see "Read limits" above): the
bytes arrive base64, which is not analysable — either an `EmbeddedResource` content block or
`ReadContent::Ref` needs to become real, or PDF reading stays outside the tool surface.

### Scope

**In (v1):** the tool surface above; leases with renewal and all-or-none ordered sets; intent
journal with lazy and proactive recovery; content-addressed history with restore-to-copy; conflict
registry; audit log; whole-file dedup; the untrusted-data envelope on reads; SMB and POSIX backends.

**Deferred:** ACL-aware metadata index (v2), chunked dedup (v2), cell-level xlsx CAS (v2), dashboard
push (v2), coord HA (v3), cloud backends.

**Out of scope:** cross-file atomic transactions, three-way merge of Office binaries, CRDT or
character-level co-editing, multi-server replication.

## Security notes

The endpoint runs as the logged-in user and ACLs are enforced by that token on the direct
filesystem path — there is no impersonation or delegation layer. Every lease, write, restore, and
history entry is stamped with the acting AD principal. The audit trail is a primary deliverable, not
a byproduct.

Two things to know explicitly:

- **Existence leak (documented v1 limitation).** `coord.resolve(path)` returns version, size, and
  mtime regardless of the caller's ACL, because the watcher indexes as a service account. Accepted
  for a flat-permission department; the v2 fix is an ACL-aware index.
- **The control plane carries file bytes, and it is not access-controlled in v1.** Reads still go
  endpoint → share → model under the user's own token and never touch coord. But since D-026 a write
  snapshots its pre-image to coord (`PUT /blobs`), so coord's blob store holds file content, and
  `GET /blobs/{version}` applies no ACL check — nor does any other coord route, by design in the
  MVP's `trusted-header` posture, which authenticates nothing (see below). Anyone who can reach
  coord's port can fetch any snapshotted version. This is acceptable only because coord sits on the
  internal network beside the fileserver; treat reachability of coord as equivalent to read access to
  file history, and put enforced auth (E-015) ahead of any deployment where that is not true.
  (This section previously claimed bytes only ever come through the user's own open. That stopped
  being true when pre-image snapshots started flowing to coord.)
- **Cross-agent prompt injection.** `chapr.read` wraps returned content in an explicit
  untrusted-data envelope, and the tool description states that the content is data from a shared
  drive, possibly written by another party, and never to be treated as instructions.

## License

[Apache License 2.0](LICENSE). Copyright 2026 SerenIT ApS and Prompted EV — Chaperone is jointly
owned by the two companies, and was open-sourced by agreement of both. See [NOTICE](NOTICE).

The reasoning for going open, in the owners' words: *don't give larger companies a reason to go
build the same tool*, and *if it works, everyone should be able to grab it*.

Apache-2.0 rather than MIT because this ships as binaries into other companies' networks, where it
holds exclusive handles on their files: it carries an explicit patent grant, and its NOTICE
requirement keeps two-company attribution attached to a redistributed build rather than leaving it
in a repo the recipient never sees. `.mcpb` bundles are therefore built with LICENSE and NOTICE
inside them.

Still `publish = false` — Chaperone is two deployables, not a library dependency. Open source and
published-to-crates.io are separate decisions.

Contributions are inbound=outbound, with no CLA — see [Contributing](#contributing).

---

🤖 Generated with [Claude Code](https://claude.com/claude-code)

Chaperone is built with AI assistance under human review. Architecture decisions are human-owned and
recorded in an engineering logbook kept outside this repo; every commit is human-reviewed before it
lands.
