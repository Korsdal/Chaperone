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

### What was the problem

Several things the code said about itself were false, and one of them was
dangerous: after a `chapr_move` renamed a file successfully, a failure to reach
the coordinator reported *"NOTHING WAS CHANGED. …Chaperone deliberately refuses
writes"* — the opposite of what had happened, to an agent that would then act on
it. The rest were comments and tool descriptions that had drifted from the
behaviour they described.

### What was changed

| Site | Change |
|---|---|
| `backend.rs` move tail | A failed coordinator call after the rename now reports committed-but-unrecorded, naming the destination — the shape `write`, `create`, `delete` and `restore` already used |
| `error.rs` | That variant's message is verb-neutral; it said "write to {path} committed on disk", already false for a delete and a move |
| `write.rs`, `backend.rs` | Fail-closed is documented where it actually starts (`assert_read`), not two coord calls later at the journal |
| `ops.rs`, `tools.rs` | Restore no longer claims it "cannot clobber a concurrent writer"; it performs no CAS, and the docs now say so |
| `server.rs` | Move's description stops implying the audit trail follows a rename; an unrecognised file type no longer instructs the model to escalate |

### What was deferred / backlogged

Restore's spec-versus-code disagreement (concept §6.5 says it runs the full write
path, which would imply a CAS) is recorded, not resolved — it needs a decision.
Journalling the move window, so a stale coordinator is genuinely recoverable, and
auditing the history an overwrite-move discards both remain open in the roadmap's
correctness phase. Holding the handle through the rename is still I-007.

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
