# Chaperone

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

## Build

Requires Rust 1.85+ (`clap` and `clap_builder` declare 1.85, `axum-server` 1.82 — the tree does
not build on the 1.75 this used to claim).

```sh
cargo build --workspace
cargo test  --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Building on Linux alongside a Windows checkout: set an isolated target dir
(`CARGO_TARGET_DIR=$HOME/chapr-target`) so the two toolchains don't clobber each other. The endpoint
compiles on Linux because the `windows` dependency and the SMB backend are `cfg(windows)`-gated.

Coord setup and service install:

```sh
chapr-coord setup                      # interactive; or --unattended
chapr-coord serve --config coord.toml
```

See `packaging/coord/` for the config template and service install steps, and `packaging/mcpb/` for
building the endpoint bundle. Note that `build-mcpb.ps1` writes to `./build` by default, which is
git-ignored.

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

Proprietary — `UNLICENSED`, not published to crates.io.

---

🤖 Generated with [Claude Code](https://claude.com/claude-code)

Chaperone is built with AI assistance under human review. Architecture decisions are human-owned and
recorded in the project logbook; every commit is human-reviewed before it lands.
