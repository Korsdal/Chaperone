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
