# Feature Specification: Corroboration that doesn't over-count one author restating a fact across dated primary notes

**Feature Branch**: `fabrik/issue-880`
**Created**: 2026-11-22
**Status**: Specified
**Input**: User description: "Research: corroboration over-counts one author restating a fact across dated primary notes. Support (ADR-0752, Decision 4) counts **distinct documents** per same-fact key `(group, source, target, name)`. That stops a derived page from corroborating itself, but it treats every primary document as independent evidence. In a real notebook, many primary documents aren't independent: daily notes carry the same open action or decision forward day after day; standup recaps and weekly digests restate facts already in earlier notes by the same author. So a fact copied through ten daily notes reports `corroboration: 10`. Research and propose how support/corroboration should treat restatements within primary content. Options to weigh: (1) count independent sources, not documents, via a caller-declared source or author key on episodes (parallel to `origin`); (2) separate first assertion from carried-forward restatements, reporting `support` alongside a `restatements` count and keeping the raw document count; (3) leave counting alone and expose the shape (distinct dates, distinct authors, first-asserted episode) at read time. Whatever is chosen should preserve ADR-0752's properties: read-time aggregation, no edge dedup at ingest, no backfill, identical results after WAL replay. Acceptance: a short ADR draft choosing an approach, with the API delta for `support` / `corroboration` and the lint categories that read them; and a fixture or eval extension that measures it: a corpus where a fact is restated across many dated primary notes by one author and stated once by an independent source, with the expected support for each."

## Background

ADR-0752 made derived content (agent-written summaries, answers) cite a fact without corroborating it. It did so by tagging each episode with a caller-declared `origin`, and by counting `support` per **distinct document** over same-fact sibling edges. `corroboration` is `primary + external`. Support is aggregated at read time, ingest never deduplicates edges, there is no backfill, and the numbers come out identical after WAL replay.

That closes the case where a derived page corroborates itself. It leaves open a second inflation that comes from primary content. In a real notebook (the consumer is shadoworg/liminis-notes), primary documents are often not independent evidence of each other:

- Daily notes carry the same open action or decision forward day after day ("still waiting on Acme's contract").
- Standup recaps and weekly digests restate facts already recorded in earlier notes by the same author.

A fact copied through ten daily notes therefore reports `corroboration: 10`. This is the same kind of inflation ADR-0752 removed for derived content, now caused by one author repeating themselves. Weak-support and contradiction lint, and anything that ranks by corroboration, end up skewed by how often someone re-types something rather than by how well the fact is attested.

lcg's own retrieval evaluation already encodes the right intuition. In `docs/retrieval-quality-evaluation.md`, a primary standup recap is a gain-1 near-duplicate of its source note: the evaluation knows a recap is not new evidence, but the support count does not.

This is **not** liminisapp/liminis-context-graph#732, which covers derived observations and mental models. This issue is about how primary evidence is counted. Companion app-side issues: shadoworg/liminis-notes#1368, shadoworg/liminis-notes#1369 (the latter group includes agent-made summaries mis-stamped as `primary`, which is an origin-stamping problem and separate from this one).

This issue is a **research deliverable**: choose an approach and prove it with a measurable fixture. It is not the implementation of the chosen approach.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - A decision record that picks one counting approach (Priority: P1)

A maintainer (and the downstream app team) need a short, reviewable ADR draft that weighs the three candidate approaches and chooses one (or a justified combination), so implementation can be planned from a settled decision instead of an open design question.

**Why this priority**: Everything else (fixture expectations, API changes, lint behaviour) depends on which approach is chosen. Without the decision there is nothing to implement or measure.

**Independent Test**: Read the ADR draft on its own. It names the chosen approach, states why the other options were not chosen, shows the exact changes to the `support` / `corroboration` annotations and anything new beside them, and lists each lint category that reads those values with how its behaviour changes.

**Acceptance Scenarios**:

1. **Given** the three options in the issue, **When** the ADR draft is read, **Then** each option is evaluated against the same criteria, including cost to the caller, ability to handle unlabelled legacy data, and the four ADR-0752 properties, and a single approach is chosen.
2. **Given** the chosen approach, **When** the "API delta" section is read, **Then** it states which existing response fields change meaning (if any), which are added, and confirms a client that sends no new input sees only additive keys (or states precisely what else changes and why).
3. **Given** the chosen approach, **When** the "lint categories" section is read, **Then** every lint category that reads `support` or `corroboration` is named, with its before and after behaviour for the restated-fact scenario.
4. **Given** the draft, **When** it is checked against ADR-0752, **Then** it explicitly confirms read-time aggregation, no edge dedup at ingest, no backfill and identical results after WAL replay, or documents and justifies any deviation.

---

### User Story 2 - A fixture that measures restatement handling (Priority: P1)

A maintainer needs a corpus and expected values that let anyone check, before and after implementation, whether support/corroboration treats an author's restatements correctly.

**Why this priority**: The acceptance for the research explicitly requires a measurable fixture, and it is the only objective way to tell whether the chosen approach does what the ADR claims.

**Independent Test**: Run the fixture or eval extension against the current behaviour. It reports the current (inflated) support for the restated fact, and the expected support under the chosen approach is documented next to it so the gap is visible.

**Acceptance Scenarios**:

1. **Given** a fact restated across many dated primary notes by one author, **When** support is computed with today's behaviour, **Then** the fixture records the inflated count (one per note) as the baseline.
2. **Given** the same corpus, **When** the expected values for the chosen approach are read, **Then** the fixture states the expected support for the fact restated by one author, the fact stated once by an independent source, and a fact stated by both.
3. **Given** the fixture, **When** it is replayed through the WAL, **Then** the support values are identical to those from the original ingest.

---

### User Story 3 - Downstream consumers know what to build against (Priority: P2)

The app team (shadoworg/liminis-notes) needs to know from the ADR what, if anything, they must declare on ingest (for example a source or author identifier) and what they will receive on reads, so their weak-support and contradiction lint and ranking can be updated.

**Why this priority**: The value is realised in the app, but it can only start once the decision is made; it depends on Stories 1 and 2.

**Independent Test**: The ADR includes a short consumer-facing note listing new inputs, new or changed outputs, and behaviour for notes that don't supply the new input.

**Acceptance Scenarios**:

1. **Given** an app that does not supply any new input, **When** it reads support, **Then** the ADR states precisely what it gets (unchanged counts, or a documented default).
2. **Given** an app that does supply the new input for new notes only, **When** older notes lack it, **Then** the ADR states how mixed legacy and new data is counted.

---

### Edge Cases

- A fact restated by the same author after a long gap (months) versus carried forward on consecutive days: does the time window change the count?
- Two different people restating the same fact from the same earlier meeting (not independent in reality, but different authors).
- A primary note that is a verbatim copy of another primary note's content, and one that rephrases it.
- Notes with no author or source information at all (legacy data, no backfill): they must still be counted deterministically.
- A document split into several chunks/episodes: restatement logic must not double-count the chunks of one note (ADR-0752 already notes the no-`doc_id` over-count gap).
- A restating note that is stamped `primary` but was actually agent-made (liminis-notes#1369): out of scope here, but the fixture should not depend on origin being correct for the restatement case.
- A fact first asserted by one source, later restated by that source, and later independently confirmed by a second source: the independent confirmation must register as new support.
- Superseded or removed notes: counts reflect episodes live at read time, as today, and a belief query's `as_of` keeps working as ADR-0752 Decision 7 defines.
- External-origin content (clipped third-party material) restated repeatedly by the same external source.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The research MUST produce an ADR draft that evaluates at least the three options named in the issue (independent-source counting, first-assertion-versus-restatement separation, expose-the-shape-only) and selects one approach, or a justified combination.
- **FR-002**: The ADR draft MUST state the API delta for `support` and `corroboration` on the reads that currently carry them (`knowledge_find_relationships`, `knowledge_find_entities`): which values change, which keys are added, and the effect on a client that supplies no new input.
- **FR-003**: The ADR draft MUST list every lint category that reads `support` or `corroboration`, and for each describe the changed behaviour for the repeated-restatement scenario.
- **FR-004**: The chosen approach MUST preserve the four ADR-0752 properties: aggregation at read time, no edge deduplication at ingest, no backfill of existing data, and identical results after WAL replay. Any deviation MUST be explicitly flagged and justified in the ADR.
- **FR-005**: The ADR draft MUST specify how episodes with no source/author information (all pre-existing data) are counted, so results are deterministic and documented.
- **FR-006**: The ADR draft MUST state whether a time window or date ordering is part of the restatement rule, and if so how "first assertion" is determined deterministically (including ties).
- **FR-007**: The ADR draft MUST state how the chosen approach interacts with `origin` (derived/answer still never corroborate), with the existing distinct-document counting, and with the `as_of` belief query.
- **FR-008**: The ADR draft MUST keep the raw per-document counts available to consumers that want them, or explicitly justify removing them.
- **FR-009**: The research MUST deliver a fixture or eval extension containing a corpus in which one fact is restated across many dated primary notes by one author and the same fact (or an equivalent one) is stated once by an independent source.
- **FR-010**: The fixture MUST document the expected support/corroboration for each fact under the chosen approach, and the current behaviour's value as a baseline, so a later implementation can be checked against it.
- **FR-011**: The fixture MUST include at least these cases: single-author restatement across many days; a single independent statement; a fact with both a restating author and an independent source; and notes with no source/author information.
- **FR-012**: The fixture MUST be consistent with the existing retrieval-quality fixture's treatment of primary standup recaps as gain-1 near-duplicates of their source notes, or the ADR MUST explain any difference.
- **FR-013**: The ADR number and file name MUST follow the repository convention `docs/adr/<issue_number>-<slug>.md` (so `0880-…`), and the ADR MUST cite ADR-0752 and distinguish itself from #732.
- **FR-014**: The deliverable MUST be limited to the research outputs above (ADR draft and fixture/eval extension, plus any doc/index updates they require); it MUST NOT change production counting behaviour.

### Key Entities *(if applicable)*

- **Episode**: a unit of ingested content. It carries `origin`, an optional `doc_id`, and a reference time. A candidate new attribute is a caller-declared source or author key (parallel to `origin`).
- **Same-fact key**: the tuple `(group, source entity, target entity, name)` over which sibling edges are aggregated for support.
- **Support**: per-origin counts of distinct documents attesting a fact. `corroboration` is the primary + external total.
- **Restatement**: a later same-fact episode from the same source as an earlier one, which does not add independent evidence.
- **First assertion**: the earliest episode stating a fact from a given source.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A reviewer can read the ADR draft alone and state which approach was chosen, why the others were rejected, and what changes in the API, within one reading and without consulting the issue thread.
- **SC-002**: For the fixture's fact restated across N (≥10) dated primary notes by one author, the documented expected corroboration under the chosen approach is independent of N, while the baseline under current behaviour equals N.
- **SC-003**: For the fixture's fact stated once by an independent source plus restated by one author, the documented expected corroboration reflects two independent sources, not N+1.
- **SC-004**: Support values for every fixture fact are identical before and after a full WAL replay.
- **SC-005**: Every lint category that reads `support` or `corroboration` is accounted for in the ADR (zero unnamed readers).
- **SC-006**: Notes without any source/author information produce the same support as today (or the ADR states the exact documented difference), so no existing graph needs a backfill.

## Assumptions

- This issue's deliverable is research: an ADR draft and a measuring fixture or eval extension. Implementing the chosen counting behaviour is a separate follow-up issue.
- The ADR is numbered `0880-<slug>` per the repository's issue-numbered ADR rule.
- Any new caller-declared source/author key follows the `origin` model: declared by the caller, never authenticated, and only as reliable as the caller's declaration.
- Two lint layers read `support` / `corroboration`. lcg's own deterministic lint (`crates/core/src/lint/weak.rs`, `lint/contradiction.rs`, per ADR-0755) is in this repository, and the ADR analyses it as the in-repo readers. The consuming app (shadoworg/liminis-notes) has its own weak-support and contradiction lint, ranking and UI that read the same values through the API; the ADR describes what those need from the API, and the app-side changes are handled in the companion app issues.
- Mis-stamped origin (agent-made summaries marked `primary`) is handled app-side and is not solved by this work.
- Restatement is judged per source/author key over same-fact siblings, using the existing same-fact key; semantic or embedding-based "is this a paraphrase" judgement is not required.
- The fixture can extend the existing retrieval-quality corpus or be a separate small corpus, whichever the researcher finds fits the existing eval structure better.

## Out of Scope *(optional)*

- Implementing the chosen counting change in production code, schema, or the MCP tool registry.
- Derived observations and mental models (#732).
- Correcting origin stamps on agent-made content (liminis-notes#1369).
- Ingest-time edge deduplication or any backfill/migration of existing episodes.
- Changing the app's lint, ranking or UI (companion app issues cover these).
- Authenticating author or source claims.

## Source References *(optional)*

- `docs/adr/0752-episode-origin-and-corroboration.md` (Decisions 1, 2, 4, 7: origin, derivation, distinct-document support, read-time annotation)
- `docs/retrieval-quality-evaluation.md` (near-duplicate handling: primary standup recap at gain 1)
- liminisapp/liminis-context-graph#732 (derived observations; distinct from this issue)
- shadoworg/liminis-notes#1368, shadoworg/liminis-notes#1369 (companion app issues)
