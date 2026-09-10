# Changelog

Every entry answers the same three questions in the same order: **what was the
problem**, **what was changed**, **what was deferred**. Each section is capped at
about six lines, with a table wherever the rows share a shape.

The cap is deliberate. A release note nobody finishes reading is approved without
being checked, and unchecked descriptions drift away from the code — which is a
failure this project has filed against itself more than once. Depth belongs in
`LOGBOOK.md` and its decision records, which are written for a reader who has
already chosen to go deep. This file is for the reader deciding whether to care.

Versions appear here only once a human has set them; work in flight sits under
`Unreleased`.

---

## Unreleased

*(nothing in flight)*

---

## 0.1.4 — 2026-09-10

Set by jok. Four things to know before upgrading:

- **`chapr_restore` gains a required argument** — an in-place restore must now
  state the version it expects to find (or `absent`).
- **`chapr_stat` and `chapr_history` refuse a directory** and name `chapr_list`
  instead. `stat` used to report `permission denied` on a healthy share, and
  `history` used to answer `[]` as though a folder simply had no history.
- **The Windows coordinator is an MSI**, and the bare `chapr-coord.exe` is no
  longer published. There is no macOS coordinator. Endpoints are unchanged.
- **Upgrading an existing coordinator:** stop and remove the old service first
  (`sc.exe delete chapr-coord`), then install the MSI. It does not adopt a
  service installed by hand. Your data directory and config are untouched.

### What was the problem

Two cowork sessions against a real share found the engine behaving and not
explaining itself. Four misconfigurations were byte-identical from the tool
surface. `restore(in_place)` performed no CAS, so an agent could destroy content
it had never read. Errors named the wrong subject: a create reported `not found`
for the file it was asked to create; a conflict named the sidecar rather than the
file. A directory appeared in a listing as `size: 0`, and none could be created.

### What was changed

| Area | Change |
|---|---|
| Refusals | Carry the root, the setting it came from, and when it was read — a stale root and a wrong one looked identical. All paths formatted plainly; two of three were Debug-escaped, doubling backslashes on every refusal |
| `restore(in_place)` | Requires the state the caller observed and CAS-checks it. A soft-deleted target comes back at its **original name**, so recoverability is kept in practice |
| `chapr_mkdir` | New. Refuses a name resembling a sibling, deterministically by edit distance; numbered siblings (2026/2027) are exempt |
| Errors | A conflict names the file, and the sidecar only when one exists; a missing parent names the **parent** |
| Audit | Every refusal is recorded, reads included, with a greppable reason; `/admin` gains path and detail search |
| Provenance | A forced write records `write_forced`, visible in `chapr_history` where agents look; `chapr_move` returns its version; listings carry an entry type |
| Packaging | The bundle template ships no defaults, requires the coordinated root, and no longer renders our own notes in the install dialog |

### What was deferred / backlogged

Move provenance in history needs real columns and therefore C0's migration
machinery. The coordinator setup epic — a UI wizard, uninstall, update — is
specified and unbuilt, which leaves an installer dependent on being told the URL
and share path by hand. No `chapr_config` tool: refusal enrichment covers three
of the four misconfiguration states, and the fourth is a decision about growing
the tool surface.

### Also in this release: the installer, TLS, and the CI that proves them

**The release asset set changes.** On Windows the coordinator ships as
`chapr-coord-<ver>-windows-x86_64.msi` and the bare `.exe` is gone; **there is no
macOS coordinator** at all. Endpoints are unchanged: a `.mcpb` bundle and a bare
binary on all three platforms. Eight artifacts instead of nine.

#### What was the problem

The coordinator had no defined way to reach a Windows server — the executable
went wherever it was double-clicked, and service registration and ACL hardening
had never executed outside compilation. Separately, both packages were built by
hand: packaging ran for the first time when a tag was pushed, so a broken
manifest surfaced as a broken release, and the installer's only test was one
person's elevated shell.

#### What was changed

A WiX 6 MSI: placement, service, firewall rule, licence and finish dialogs,
uninstall that keeps every byte of the data directory. It carries no custom
action — the service writes its own config on first start from the values it was
registered with, and never touches one that already exists.

CI now builds both packages on every push and **installs the MSI and verifies
it** — service, both tokens, firewall rule, `/healthz`, and data surviving
uninstall. The release checks its staged set by name rather than by count.

The endpoint reads the machine's certificate store, so an HTTPS coordinator with
a private certificate finally works. `data_dir` is one explicit config value
instead of four inferred locations. Console output is ASCII.

#### What was deferred

**Nothing is code-signed**, the MSI included, so Windows asks before running the
installer. The MSI does not adopt a coordinator service installed by hand — an
older install must be removed first.

### Also released here: the truth pass and the correctness core

Everything below sat unreleased since 0.1.3 and ships in this version. Kept in
its own words rather than rewritten, because each part was checked as written.

#### The truth pass (Phase A) — what was the problem

Several things the code said about itself were false, and one of them was
dangerous: after a `chapr_move` renamed a file successfully, a failure to reach
the coordinator reported *"NOTHING WAS CHANGED. …Chaperone deliberately refuses
writes"* — the opposite of what had happened, to an agent that would then act on
it. The rest were comments and tool descriptions that had drifted from the
behaviour they described.

#### What was changed

| Site | Change |
|---|---|
| `backend.rs` move tail | A failed coordinator call after the rename now reports committed-but-unrecorded, naming the destination — the shape `write`, `create`, `delete` and `restore` already used |
| `error.rs` | That variant's message is verb-neutral; it said "write to {path} committed on disk", already false for a delete and a move |
| `write.rs`, `backend.rs` | Fail-closed is documented where it actually starts (`assert_read`), not two coord calls later at the journal |
| `ops.rs`, `tools.rs` | Restore no longer claims it "cannot clobber a concurrent writer"; it performs no CAS, and the docs now say so |
| `server.rs` | Move's description stops implying the audit trail follows a rename; an unrecognised file type no longer instructs the model to escalate |

#### What was deferred

Restore's spec-versus-code disagreement (concept §6.5 says it runs the full write
path, which would imply a CAS) is recorded, not resolved — it needs a decision.
Journalling the move window, so a stale coordinator is genuinely recoverable, and
auditing the history an overwrite-move discards both remain open in the roadmap's
correctness phase. Holding the handle through the rename is still I-007.

#### The correctness core (B1, B2, B3, B8)

`move_cas_core` violated invariant 4 — it hashed the source, released the handle,
then renamed — so the CAS proved nothing about the bytes that moved (I-007). The
rename now runs through the handle held since the source's CAS, and the two-path
`FsPrimitives::rename` was deleted so the unsafe order is no longer expressible.
A move records its intent before renaming, and the coordinator discharges that
record inside the migration's own transaction, which makes completing an
interrupted move exactly-once by construction (B3, D-046). The self-test's exit
code stops contradicting its own output: a check that could not run counts, one
that cannot apply here does not (B8).

---

## 0.1.3 — 2026-08-25

### What was the problem

`chapr_read` refused every text file that was not valid UTF-8 as "an unrecognised
binary format … worth their attention". On a Danish document share that means any
file containing the language's own alphabet, and the message blamed the file's
integrity rather than its encoding — sending the reader looking for corruption
that was not there.

### What was changed

A classifier splits unrecognised bytes into text-in-another-encoding versus a
binary container, and **chooses a message, never an outcome**: both still refuse,
which is what keeps its thresholds harmless. The encoding case gets its own
wording that names the encoding, separates Chaperone's health from the file's
state in the first sentence, and gives the human a remedy. A `NON_UTF8_TEXT`
warning carries the structural evidence to the coordinator's diagnostics, grouped
so a folder of legacy files reads as one fix rather than an outage.

### What was deferred / backlogged

Converting encodings and extracting text stay outside Chaperone — it coordinates
files, and that boundary is deliberate. The capability is booked as a separate
deployable (E-028). A UTF-8 file *with* a byte-order mark is still served with the
mark inside the envelope body (I-015 residual). Writes remain unguarded: an agent
can still use base64 to land a file a later read will refuse, mitigated by
guidance to the model rather than by a symmetric write check.

---

## Earlier versions

`0.1.0` through `0.1.2` predate this file. Their history is in the git log and in
`LOGBOOK.md`; nothing was reconstructed here, because a changelog entry invented
after the fact is the drift this format exists to prevent.
