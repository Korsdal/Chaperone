# Contributing to Chaperone

Issues and PRs are welcome. Contributions are inbound=outbound: anything you send
in is licensed under the same Apache-2.0 terms (section 5 of the licence). There is
no CLA.

## Three things that are not guessable from the code

Read these before changing anything. Each one has cost somebody data or time.

### The invariants are assertions, not preferences

Listed in [`docs/architecture.md`](docs/architecture.md#load-bearing-invariants),
and getting one backwards loses somebody's file. In particular, **leases are an
optimization**. Exclusive-open plus CAS is the correctness core: code is correct
with lock+CAS and no leases, and *not* correct with leases and no CAS.

> [!WARNING]
> If a change appears to let you skip the CAS re-hash, the change is wrong.

### The write path is deliberately boring

Everything from version-check to write-close happens under one held exclusive
handle, in straight-line blocking I/O inside `spawn_blocking`. It reads as
unfashionably synchronous and repetitive, and that is the design: correctness
*ordering* matters more than I/O concurrency on a single file. Be clever elsewhere.

**A PR that makes the write path more elegant is the one most likely to be
declined.** The [write-path diagram](docs/architecture.md#the-write-path) shows the
ordering that has to hold.

### Versioning is a human decision

Nothing automated bumps `[workspace.package] version`: not a tool, not CI, not an
agent. The release workflow refuses a tag that does not match the version in the
tree, precisely so that the bump has to be a deliberate act by a person. Propose a
version in a PR; don't set one.

## Getting the tree green

```sh
cargo build --workspace
cargo test  --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Requires **Rust 1.88+**, the highest `rust-version` in the locked dependency
graph, measured with `cargo metadata` rather than estimated. CI runs the three
commands above on Windows, Linux and macOS, plus a build on exactly the toolchain
`Cargo.toml` declares, read from that field so the two cannot disagree.

The Windows builds link the CRT **statically** (`.cargo/config.toml`), so the
shipped binaries need no Visual C++ redistributable. This is not a preference: the
dynamic default stopped a coordinator from starting on a clean Windows Server 2022.

Two things worth knowing before you debug a CI failure:

- **Run the whole suite on the other platform, not just the named failing test.**
  Cargo stops at the first failing test binary and clippy runs after tests, so one
  reported failure can be hiding several. Building on Linux alongside a Windows
  checkout: set `CARGO_TARGET_DIR=$HOME/chapr-target` so the two toolchains do not
  clobber each other.
- **Check which commit the CI log was built from.** More than one "new" failure has
  turned out to be a re-run of an older hash.

## Cutting a release

A release is a tag:

```sh
# 1. bump [workspace.package] version in Cargo.toml (a human decision)
# 2. commit it
git tag v0.1.2 && git push origin v0.1.2
```

`.github/workflows/release.yml` then tests, builds and packs on all three platforms, checks every
artifact is present and non-empty, generates `SHA256SUMS`, attests provenance, and opens a **draft**
release for a human to publish. It refuses to run if the tag does not match the workspace version.
The version bump is the decision; the tag only records it.

`workflow_dispatch` runs the **whole** pipeline and publishes nothing: it stages the artifacts,
renders the release notes and writes the checksums, attaching both as a `dry-run-release-material`
artifact, then stops before `gh release create`. So a tag executes exactly one command that has
never run before, rather than four. Use it after any change to packaging.

## A note on `D-nnn` / `E-nnn` / `I-nnn`

Comments throughout the source cite identifiers like `D-032`, `E-016`, or `I-005`.
These index the engineering logbook: decisions, work items, and issues, kept in
[`LOGBOOK.md`](LOGBOOK.md) and the `logbook/` directory. `LOGBOOK.md` is the entry
point and a complete cold read; it indexes bodies that live in
`logbook/decisions/`, `logbook/logs/`, `logbook/ISSUES.md` and
`logbook/BACKLOG.md`.

Each reference is provenance rather than a pointer you have to follow: the comment
carrying it states the reasoning in full, which is the convention those comments are
written to. If you hit one that does not stand on its own, that is a documentation
bug worth reporting, and the comment should be made self-contained.
