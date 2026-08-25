# Decision Log — Product & Scope

> Child document of `LOGBOOK.md` (logbook protocol v1.0). **Most recent first.**
> Scope: product scope, positioning, market boundaries, what this is and is not for.
>
> The index of *all* decisions — every ID, title, date, theme and status — lives in
> `LOGBOOK.md` under `## Decision Log`. This file holds the bodies only.
> New entries are appended here by `/logbook decide` per the theme rule in
> `LOGBOOK.md`'s YAML (`sections.decision_log.themes`).
>
> **Created 2026-08-21 with D-037.** A fourth theme was added because none of
> architecture / deployment / process covered a decision about *what product this
> is and for whom* — jok's call, over stretching `deployment`.

---

<a id="d-039"></a>
### D-039 — Chaperone coordinates; it does not extract or transcode. That is an add-on, not a missing feature — 2026-08-25

**Trigger:** the `bug-hunt-2508` investigation (I-015) established that `chapr_read` refused every
non-UTF-8 text file and described it to the agent as "an unrecognised binary format … worth their
attention". The obvious reading was "a capability regressed, restore it", and the costed options ran
from a message fix up to encoding detection with transcoding both ways.

**jok's framing settled it instead, and it is the more useful answer:** *"It is a good place to stop
and consider what this product does vs what it does not do — it is not a file extraction service, it
is a coordination service, and the coordination part works. The extraction of useful text is not
Chaperone's job, and if it fails it should fail loudly, to surface that to the user — it should not
give the message that something is wrong with the service itself. The issue is the files."*

**Decided.**

1. **Refusing is correct and stays.** A file Chaperone cannot hand over as text is refused, loudly.
   That is not a gap to be closed later; it is the boundary working. The pre-guard behaviour served
   such files as base64, which was quieter and no more readable — base64 Danish prose is exactly as
   unanalysable as base64 of a PDF.
2. **Chaperone does not convert encodings, and this is a *product* boundary, not only a technical
   one.** There is a technical argument too and it is strong — the write side can only emit UTF-8, so
   transcoding on read would make a verbatim echo silently rewrite the file in a different encoding
   and record it as a deliberate edit under the user's own principal, and for `.bat`/`.ps1`/`.ini`
   files the original encoding is a *requirement* rather than an accident. But the boundary would
   hold even if that were free.
3. **What was actually broken was the message, and the audience it never reached.** A refusal must
   not imply a fault in the service, must not describe a text file as a binary, and must carry a
   diagnosis somebody can act on. Fixed accordingly: the refusal names the encoding and gives the
   human remedy, and a `NON_UTF8_TEXT` diagnostic files the structural evidence into the channel
   administrators already use. Message and diagnosis, not capability.
4. **Extraction and transcoding are a separate deliverable** — a "File extraction MCP for
   Chaperone", booked as **E-028**. Deliberately a different artifact: it iterates on its own cadence,
   a customer can bring their own pipeline instead, and it keeps the coordinator lean. This is the
   same reasoning the roadmap review already reached for extraction-as-a-third-deployable.

**What this resolves.** **I-005** has carried three options since 2026-08-05 for how a PDF's content
reaches a model, the third being *"declare PDF reading out of scope and coordinate only the derived
artifacts."* That option is now chosen, and it generalises: it is not about PDFs, it is about every
form of "the bytes are not text a model can read", encodings included. **D-028** (extracted text
mirrors live on the *uncoordinated* side, written by the customer's own pipeline) is reinforced
rather than reversed — mirror *coordination* is still the open question, and this decision does not
touch it.

**What it does not decide.** Whether the mirror-producing pipeline writes UTF-8. That is still open,
still unverified, and still the thing that sets I-015's real severity: Python's `open(p,'w')` with no
`encoding=` resolves to cp1252 on a Danish-locale Windows box, and if any mirror is written that way
then every mirror containing a Danish character is refused — the pilot's main workload failing on its
main content. The refusal now diagnoses that accurately and names the producing step as the fix,
which is all a coordinator can do about it. **Somebody still has to ask Kristian.**

**Cost accepted:** on a share of legacy-encoded files, agents cannot read them at all until someone
re-saves them. That is a real reduction in what an agent can do on day one, taken deliberately,
because the alternative is a coordinator that quietly changes people's files.

**Amended 2026-08-25, same day — where this decision's own framing is weakest.** jok asked the
question that tests it: *if the extraction solution is agentic and writes through Chaperone, does
Chaperone create files it then refuses to read?* Established against the real tools rather than
argued:

- **An agent authoring text cannot get this wrong.** `decode_content`'s `utf8` arm is
  `String::into_bytes()`, valid UTF-8 by construction, so there is no encoding for an agent to
  choose badly. Verified readable-back for Danish `æøå`, typographic punctuation and `€`, emoji and
  CJK, a BOM as leading content, and the empty string. This is now pinned by
  `any_utf8_write_can_be_read_back_as_text` — the property was load-bearing and unasserted.
- **Through `base64`, it can.** `binary_guard` runs on read only; nothing guards the write path. A
  base64 write of CP1252 bytes is accepted, lands verbatim, and the next read refuses it. Also
  pinned, deliberately, by `a_base64_write_of_code_page_bytes_is_accepted_then_refused_on_read`.

The reachable sequence is specific and partly of this project's own making: a read is refused → the
refusal names `allow_binary` as the way to copy exact bytes → an agent told *"produce a mirror of
this file"* reads "copy" as its job → it writes the copy → the same unreadable encoding now sits in
a new place. The refusal's wording hedges, and hedging is not a guarantee.

**What that costs this decision.** Point 1's *"the issue is the files"* holds only while the
producer is outside Chaperone. With an agentic producer the refusal stops being an environment fact
and becomes **a loop Chaperone created** — it accepted the write, then refused the read — and the
`NON_UTF8_TEXT` remedy's "fix the producing step" points at a prompt rather than a Python script.
That is a materially weaker position than this decision assumed, and it is recorded here rather
than left to be rediscovered.

**Mitigation chosen (jok): guidance, not enforcement.** The rule now sits in
`server::instructions()` — author text as `utf8`; `base64` reproduces bytes and is for copying, not
authoring; and specifically do not answer a refused read by copying its bytes elsewhere. jok's
reasoning: *"It's non-deterministic by nature, but it carries weight if the agents understand the
failure mode before they create the base64-loop that we fear. If the model understands it, they can
avoid it, and better, they can explain to the user what went wrong."* One clause was added to
`ContentEncoding::Base64`'s schema doc as well, since that field is where the choice is actually
made; the write/create tool descriptions were left alone on context-cost grounds.

**A symmetric guard on write was rejected**, and the reason is worth keeping: it would refuse
byte-exact copying, which is `allow_binary`'s one legitimate use — a copy of a CP1252 file *should*
stay CP1252. **Still available if guidance proves insufficient in the pilot:** diagnose at write
time without blocking, so a write that lands bytes a later read would refuse files its own
`NON_UTF8_TEXT` at creation, attributed with version and principal. That closes the loop provably
rather than advisorily, at the price of a warning per file on a legitimate bulk copy.

**This is also evidence in the D-028 question**, not a resolution of it: mirrors written *through*
Chaperone carry a failure mode that mirrors written *beside* it do not.

**Made by:** jok (the boundary and the framing) / Claude (the investigation, the message and
diagnostic design, and the write-side argument) | **Review date:** with E-028, or when a customer's
share proves unreadable enough to change the calculus | **Status:** CURRENT

<a id="d-037"></a>
### D-037 — On-prem is the product; cloud stays deferred on a market judgment, not an architectural exclusion — 2026-08-21

**Trigger:** a roadmap review (`specs/reviewed_roadmap.md`, local, this session) established that phases 3–6
of the 0.2→0.5 plan each widen what coord knows and stores. That made "what product is this, and for
whom" the question that had to be answered before sequencing any of it.

**Problem:** CLAUDE.md's north star generalises the v1 shape to "any fileserver, any endpoint OS", and
D-019 booked a V3 cloud leg (S3 → Azure → Graph) with an invariant-2 relaxation (E-021) to
accommodate it. Whether that leg is still the plan decides several live items at once: E-021,
V3-cloud, E-020, the standing of D-023's OIDC rationale, and whether the roadmap's phase 6 librarian
is worth building at all.

**jok's assessment — the commercial argument, and the stronger one.** A cloud agent-collaboration
solution will be built within roughly six months by Microsoft, AWS, Oracle, Anthropic, OpenAI or
someone of that size. SerenIT (8 people) + Prompted (1) cannot compete there. On-prem is where the
gap is high and where the cloud vendors structurally will not go, because the customer's fileserver
is not in their control plane. The offer is a simpler product, functional solutions, and **data
sovereignty for small and medium businesses — who very often have at least one on-prem fileserver.**

**Decisions (jok):**

**(1) Near-term focus is on-prem only.** No cloud integrations. No "Chaperone for SharePoint", no
"Chaperone for S3", in the development window this covers.

**(2) Recorded as a market judgment, not a product boundary — explicitly revisitable.** jok chose
this over the offered stronger option of moving cloud to the **Out** list. Consequences, all
deliberate:
- Cloud backends stay **Deferred**, not Out. `README.md`'s deferred list is therefore already
  correct and needs no change.
- **V3-cloud and E-021 stay booked.** Neither closes.
- **Invariant 2 (`version = BLAKE3(file_bytes)`) stays provisional.** The available prize was making
  it permanent and deleting D-019's backend-opaque `VersionToken` fork. That prize is deliberately
  **not** taken: it is the piece that would be expensive to undo if the six-month assessment is
  wrong. Optionality was bought at the price of a softer invariant, knowingly.
- CLAUDE.md's "any fileserver" north star survives as the long-term aspiration. Only the near-term
  focus narrows.
- **Review date 2027-02-21**, six months out, per the assessment's own horizon. This is the first
  decision in the log with a real review date rather than N/A, because it rests on a market
  prediction that will be observably right or wrong by then.

**(3) Identity: the deployment posture varies, so the pluggable boundary is load-bearing.** jok
declined all three offered branches and constrained the question instead: *"Current one is entra
managed (the customer running it), but the solution should be able to handle different
possibilities."*
- The **current customer is Entra-managed**, which is D-023's stated target and confirms it.
- **D-016's `Authenticator` boundary and D-023's pluggable-validator choice both stand, reinforced.**
  This is not over-engineering; variation in customer identity posture *is* the requirement.
- **E-015 does not halve.** Claude argued twice this session that on-prem-only collapses E-015 to
  `negotiate` alone and kills OIDC's rationale. Both times wrong, and recorded because the error is
  instructive: D-023 chose OIDC for three reasons and only one — cloud/hybrid coord — is weakened
  here. The other two are untouched: OIDC is **testable today in jok's Azure tenant while Negotiate
  needs an AD domain that does not exist**, and it is IdP-agnostic rather than Microsoft-only. An
  on-prem fileserver does **not** imply an on-prem AD — an SME with a NAS and Entra-joined laptops
  has SMB with local accounts and no Kerberos realm to Negotiate against.
- **Consequence for roadmap item 1.1:** a per-endpoint bearer token is a **bridge**, not an
  architecture. Do not build issuance, rotation, revocation or a per-endpoint registry for it. That
  also means 1.1 needs no new table, so it does not run into the absent migration machinery
  (`db.rs` carries zero `ALTER TABLE`).

**(4) Phase 6 (the librarian) is rescoped to linkage only; retrieval is dropped.** Keep the part a
filesystem genuinely lacks and a cloud vendor will not build for someone else's fileserver:
mirror-to-source edges, "this proposal cites that tender", version families. **Drop** FTS5, BM25 and
any search surface — that is precisely the checkbox a hyperscaler ships, and it is where the
six-month argument bites hardest. **Unchanged prerequisite:** the ACL-aware design is still required.
Dropping retrieval does not drop it, because aggregated linkage metadata is disclosive on its own —
filenames, timestamps, principals and relationships reveal organisational structure and deal flow
without a byte of content. Same argument as §13.2's existence leak, one step worse.

**(5) Filed under a new `product` theme.** None of architecture / deployment / process covered
product scope, positioning or market boundary. jok chose a fourth theme over stretching `deployment`
(where D-019 and D-023 live). `sections.decision_log.themes`, `child_docs` and the Child Documents
table in `LOGBOOK.md` were updated in the same change.

**Rejected:** (a) **cloud in the Out list** — the simplification was real (closes V3-cloud and E-021,
firms invariant 2) but it trades optionality for a prediction, and jok priced the prediction as not
that certain; (b) **dropping phase 6 entirely** — the linkage half is genuinely un-commoditisable and
is the one thing on the roadmap a filesystem cannot do at all; (c) **resolving identity to a single
mechanism** — the customer base varies and D-016's seam already absorbs that.

**Not done in this pass, deliberately:** no backlog rows edited, no doc corrections applied (none
turned out to be required — see (2)), and no version implication acted on. Timing is jok's call.

**Made by:** jok (all five calls, and the commercial assessment they rest on) / Claude (roadmap
review, consequence analysis, and two corrected overreaches on E-015) | **Review date:** 2027-02-21
**Status:** CURRENT
