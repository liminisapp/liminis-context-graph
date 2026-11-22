# Feature Specification: Stop ontology extraction pinning value facts to unrelated entities

**Feature Branch**: `fabrik/issue-823`
**Created**: 2026-11-21
**Status**: Specified
**Input**: User description: "Ontology extraction pins value facts to unrelated entities (29 misattached on v2, e.g. a budget attached to a person). When the object of a fact is a value (a budget, a date, a price, an uptime figure), ontology extraction often attaches the edge's other end to an unrelated entity that happens to be nearby: 'Marlin Core has a monthly budget of $4,200' is stored as `Marlin Core HAS_MEASURE Tomas Kowalski`. 'Marlin Core has a scheduled launch date in March 2027' is stored as `Marlin Core SCHEDULED_FOR Basalt Supply`. 'Orbit Supply charges $43 per unit' is stored as `Orbit Supply HAS_TERM Ines Lindqvist`. The fact text is right but the endpoint is wrong. A walk then reaches the wrong entity, and the wrong entity collects facts that aren't about it. Measured on the v2 corpus against the skeleton (#819's baseline, in `docs/retrieval-experiments.md` E13): value facts stated on an edge of the right entity 259/428 (free-form), 290/428 (ontology v3.1); …pinned to an unrelated skeleton entity that the fact doesn't name 2, 29; …where every copy is misattached 2, 12. Free-form extraction rarely misattaches. Ontology extraction does, presumably because the relation menu (`HAS_TERM`, `HAS_MEASURE`, `SCHEDULED_FOR`) invites an entity target even when the object is a literal. About a third of value facts (138/428) aren't on any edge of their entity at all. The check is an exact string match, so this is a lower bound and partly a matching artefact. Research should establish how much of it is real before fixing it. Change (for Research to confirm): Find the cause: the edge prompt; the requirement that both endpoints be listed entities; entity resolution of a value-like name onto a nearby entity. Candidate fixes: Let an edge's target be the entity itself, or a value node, when the object is a literal. Have the prompt say so explicitly. Have the endpoint check from #816 flag a value fact whose other end isn't named in the fact text. Acceptance: On the v2 corpus, misattached value facts drop from 29 to at most 3, measured by `lcg-eval graph-integrity` (#819) or its reference script. Link recall, typing and direction stay where they are. No change to free-form extraction beyond noise. Out of scope: Re-extracting existing corpora. A repair for stored graphs can follow if the fix warrants one. Junk nodes named after note IDs (separate issue)."

## Background

When the object of a fact is a **literal value** (a budget, a date, a price, an uptime figure), ontology-guided extraction often stores the fact on an edge whose other endpoint is an unrelated entity that happens to be mentioned nearby:

- "Marlin Core has a monthly budget of $4,200" → `Marlin Core HAS_MEASURE Tomas Kowalski`
- "Marlin Core has a scheduled launch date in March 2027" → `Marlin Core SCHEDULED_FOR Basalt Supply`
- "Orbit Supply charges $43 per unit" → `Orbit Supply HAS_TERM Ines Lindqvist`

The fact text is right, but the endpoint is wrong. A graph walk then reaches the wrong entity, and the wrong entity collects facts that are not about it. This damages exactly the multi-hop, graph-walk retrieval the ontology exists to support (#815, #820).

**Baseline.** #819's E13 analysis on the v2 corpus, scored against the fixture skeleton:

| | free-form | ontology v3.1 |
|---|---|---|
| value facts stated on an edge of the right entity | 259/428 | 290/428 |
| …pinned to an unrelated skeleton entity that the fact doesn't name | 2 | **29** |
| …where every copy is misattached | 2 | 12 |

#819 has since shipped `lcg-eval graph-integrity`, the deterministic version of that analysis. Its definitions differ slightly from E13's: `Location` counts as a value fact rather than a link, and string matching is tighter. On the same ontology v3.1 graph it reports **293/440 stated and 27 misattached**, against E13's 290/428 and 29 (see the graph-integrity evaluation doc's "Reconciliation with the E13 baseline"). Under that command, a fact is *misattached* when an edge incident to the entity states the value, its other endpoint is a *different* skeleton entity, and the edge's fact text does not name that entity. An edge whose text does name the other entity (for example, a person set the budget) is a legitimate relation and is not misattached.

Free-form extraction rarely misattaches. Ontology extraction does, plausibly because:

- the ontology's relation menu (`HAS_TERM`, `HAS_MEASURE`, `SCHEDULED_FOR`) invites an entity target even when the object is a literal;
- the edge model requires both endpoints to be listed entities, so a literal object has nowhere legitimate to go;
- or entity resolution maps a value-like target name onto a nearby real entity.

Which of these, or which combination, is the cause is for Research to establish.

**The "not stated" gap.** About a third of value facts (138/428 in E13; 147/440 under the command) are not on any edge of their entity at all. Because the check is an exact string match, this is a lower bound and partly a matching artefact. How much of it is real must be established before anyone attempts to fix it.

**Related work already merged.** #816 added an ingest-time endpoint signature check, which marks a mismatched edge with `signature_mismatch` rather than dropping it, plus a repair. #819 added `lcg-eval graph-integrity`. #824 stopped extraction from creating entities named after the note ID. This issue was blocked on #819 and #824, and both are now closed.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Value facts stay on the entity they describe (Priority: P1)

An operator ingests a corpus under an ontology. When a note states a literal value about an entity ("Marlin Core has a monthly budget of $4,200"), the stored graph keeps that fact on an edge of Marlin Core, and the edge's other end is not some unrelated entity that happens to appear in the same note. Walking from an unrelated entity (Tomas Kowalski) no longer surfaces Marlin Core's budget.

**Why this priority**: This is the defect itself. A misattached value fact corrupts two entities at once: the right entity's neighbourhood points at the wrong place, and the wrong entity collects a fact that is not about it. Every graph-walk retrieval inherits the error.

**Independent Test**: Extract the v2 corpus under ontology v3.1 and run `lcg-eval graph-integrity` against the v2 skeleton. The misattached count is at most 3, and the stated count has not fallen.

**Acceptance Scenarios**:

1. **Given** a note stating "Marlin Core has a monthly budget of $4,200" that also mentions Tomas Kowalski in an unrelated sentence, **When** it is extracted under an ontology, **Then** the budget fact is stored on an edge incident to Marlin Core whose other endpoint is not Tomas Kowalski.
2. **Given** a note stating "Tomas Kowalski set Marlin Core's monthly budget at $4,200", **When** it is extracted under an ontology, **Then** an edge between Marlin Core and Tomas Kowalski carrying that fact is still allowed, because the fact names him.
3. **Given** the v2 corpus extracted under ontology v3.1, **When** `lcg-eval graph-integrity` is run, **Then** value facts misattached is at most 3, against a baseline of 27 from the same command.
4. **Given** the same extraction, **When** compared with a baseline extraction from the same commit without the change, **Then** typing, link presence, expected relation type and direction-correct are each within the tolerance defined in SC-003.

---

### User Story 2 - Free-form extraction is unaffected (Priority: P1)

An operator who ingests without an ontology sees no change in graph quality.

**Why this priority**: Free-form extraction already misattaches rarely (2 in E13). A fix aimed at the ontology path must not regress the default path.

**Independent Test**: Extract the v2 corpus free-form before and after the change, run `lcg-eval graph-integrity` with `--compare`, and confirm every metric is within the SC-004 tolerance.

**Acceptance Scenarios**:

1. **Given** the v2 corpus extracted free-form, **When** `lcg-eval graph-integrity --compare` is run against a free-form baseline from the same commit without the change, **Then** no metric is worse by more than the SC-004 tolerance.

---

### User Story 3 - Remaining misattachments are visible (Priority: P2)

When extraction still produces an edge whose fact states a literal value but whose other endpoint is an entity the fact text does not name, the operator can see that it happened: a count is reported at ingest, or the edge carries a mark, in the spirit of #816's `signature_mismatch`. A silent failure is not acceptable.

**Why this priority**: The P1 fix may not be perfect. A diagnostic lets an operator, or a future repair, find the residual cases without a skeleton. It is secondary to actually reducing the misattachments.

**Independent Test**: Ingest a chunk whose extraction, through a stubbed model output, produces such an edge. Confirm the reported count or the edge mark identifies it.

**Acceptance Scenarios**:

1. **Given** an extracted edge whose fact states a literal value and names only one of its two endpoints, **When** it is ingested, **Then** it is identifiable as a suspected misattachment, by a counter or by a mark on the edge.
2. **Given** an extracted edge whose fact names both endpoints, **When** it is ingested, **Then** it is not flagged.

---

### User Story 4 - Know how much of the "not stated" gap is real (Priority: P3)

A maintainer reading the evaluation docs can tell how many of the value facts that are not on any edge of their entity (147/440 under the command) are genuinely missing from the graph, and how many are present but missed by the exact-string match (for example "$4,200" against "4200 USD", or "March 2027" against "2027-03").

**Why this priority**: The issue asks for this before any fix to the gap is attempted. It decides whether a follow-up issue is warranted. It does not block the misattachment fix.

**Independent Test**: The findings, with counts broken down by cause, are recorded in the repository's evaluation documentation.

**Acceptance Scenarios**:

1. **Given** the ontology v3.1 v2 graph, **When** the "not stated" value facts are classified, **Then** the documentation records how many are absent from the graph, how many are present in a form the exact match misses, and how many are stated on some other entity.

---

### Edge Cases

- **A fact that names both entities and carries a value** ("Ines Lindqvist negotiated Orbit Supply's $43 unit price"): an edge between the two is legitimate and must not be removed or flagged as misattached.
- **One sentence carrying several value facts** about the same entity (a budget and a launch date): each stays attached to that entity.
- **A value fact about an entity that has no other entity in the chunk**: behaviour is unchanged, and the fact still lands on the entity.
- **A value-like name that resolves to a real entity** (a product called "March", or a supplier whose name contains a number): entity resolution must not make this worse, and real entities with such names must still resolve.
- **A relation type whose ontology signature declares an entity target type** (for example `SCHEDULED_FOR → Event`), used with a literal object: the fix must not trade a misattachment for a new `signature_mismatch` mark on a legitimate entity edge, or for a fabricated entity.
- **The same value fact extracted from several notes** (copies): the target counts facts, not copies. A fact with any misattached copy still counts as misattached.
- **Strict and open ontology modes**: the fix applies to both.
- **Graphs extracted before the change**: they are left as they are (see Out of Scope).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: Research MUST identify which of the suspected causes produces misattached value facts under an ontology, supported by evidence from the v2 extraction: the edge-extraction prompt, the requirement that both endpoints be listed entities, entity resolution of value-like names, or a combination. Any other cause it finds also counts.
- **FR-002**: Under an ontology, when the object of an extracted fact is a literal value, the stored edge carrying that fact MUST NOT have as its other endpoint an entity that the fact text does not name.
- **FR-003**: The fix MUST keep the value fact on an edge incident to the entity it describes. Simply discarding misattached edges is not acceptable, so the proportion of value facts that `lcg-eval graph-integrity` reports as stated MUST NOT fall.
- **FR-004**: Edges whose fact text names both endpoints MUST continue to be extracted and stored as they are today.
- **FR-005**: The fix MUST NOT introduce fabricated entities: no new nodes named after a literal value, such as "$4,200" or "March 2027", that are counted as entities in typing, entity search or the integrity report.
- **FR-006**: Free-form (no-ontology) extraction behaviour MUST NOT change beyond run-to-run noise.
- **FR-007**: An edge whose fact states a literal value and names only one of its endpoints, if extraction still produces one, MUST be identifiable after ingest, through a reported count, a mark on the edge, or both. It MUST NOT be silently stored as a normal relation.
- **FR-008**: The change MUST include deterministic automated tests, using no live model, covering at least these two cases. First, a chunk stating a value fact about one entity, with an unrelated entity present, yields no stored edge pinning that fact to the unrelated entity. Second, a fact naming both entities still yields an edge between them.
- **FR-009**: The before and after `lcg-eval graph-integrity` reports for the v2 corpus, for both ontology v3.1 and free-form, MUST be recorded in the repository's evaluation documentation, with their provenance stated, as #819 did.
- **FR-010**: Research MUST classify the v2 ontology graph's "not stated" value facts into three groups: absent from the graph, present but missed by exact matching, and stated on another entity. It MUST record the counts in the evaluation documentation. Fixing the genuinely missing ones is not required by this issue unless they share the misattachment's cause.
- **FR-011**: Any change to what the extraction prompt or the ingest path asks of the model under an ontology MUST be documented in the ontology documentation, including how literal-valued facts are represented.

### Key Entities *(if applicable)*

- **Value fact**: A skeleton fact `(entity, attribute, value)` whose value is a literal (an amount, date, price or figure) rather than another skeleton entity. This is the denominator of #819's value-fact metrics.
- **Misattached value fact**: A value fact with at least one stored edge that states the value, is incident to the entity, and has as its other endpoint a different skeleton entity that the edge's fact text does not name. This follows #819's definition.
- **Suspected misattachment (ingest-time)**: An extracted edge whose fact states a literal value and names only one of its two endpoints. It is detectable without a skeleton (User Story 3).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: On the v2 corpus extracted under ontology v3.1, `lcg-eval graph-integrity` reports **at most 3** misattached value facts, against a baseline of 27 from the same command (29 under E13's looser matching).
- **SC-002**: On the same graph, the value-facts-stated rate is no lower than the baseline extraction's (293/440 under the command, or a fresh baseline from the same commit).
- **SC-003**: On the same graph, these metrics are each within ±1 of a baseline extraction from the same commit without the change: entities found, entities typed as expected, true links present, links with the expected relation type, and direction-correct links. Direction-correct is compared as a rate when the denominators differ.
- **SC-004**: On the v2 corpus extracted free-form, every `lcg-eval graph-integrity` metric is within ±1 of a free-form baseline from the same commit without the change, and misattached stays at most 3.
- **SC-005**: Junk note-ID nodes, fixed by #824, do not increase under either extraction mode.
- **SC-006**: The "not stated" gap is broken down by cause in the evaluation documentation, so a reader can tell whether a follow-up fix is warranted.

## Assumptions

- **The measuring instrument is `lcg-eval graph-integrity`, not E13's script.** The issue allows either one. The command is in the repository and deterministic. E13's script is not committed, and its matching produces 29 rather than 27. The "at most 3" target is applied to the command's count.
- **Baselines are fresh extractions.** Extraction is LLM-driven and not deterministic, and #816's and #824's ingest changes have landed since the E13 graphs were built. So "stays where it is" is judged against a baseline extracted from the same commit without this change, using the same model and settings, not against the E13 figures. "Noise" is taken to mean ±1 on each count metric; a larger tolerance needs a stated justification.
- **A single extraction run per configuration is enough** for acceptance, matching how #816, #819 and #824 were verified. If a result is borderline, the run may be repeated and both results recorded.
- **The maintainer runs the v2 extraction.** As with #819, the kept v2 workspaces and live extraction are not reproducible from the repository alone. The maintainer runs the corpus extraction and reports the figures on the PR, and the repository's own checks are the deterministic tests in FR-008.
- **How a literal object is represented is a Plan decision.** Options include the entity itself as target, a value node, a property, or prompt guidance alone, provided FR-002 through FR-005 hold and the integrity report still recognises the fact as stated.
- **The ingest-time diagnostic (FR-007) relies on text matching** of endpoint names against the fact text, as #819's misattachment rule does. Some false positives and negatives are accepted, which is why it flags edges rather than deletes them.
- **Stated ontologies other than v3.1** benefit from the fix but are not separately measured.

## Out of Scope *(optional)*

- Re-extracting existing corpora, or repairing misattached value facts in stored graphs. A repair can follow as a separate issue if the fix warrants one.
- Junk nodes named after note IDs (#824, merged).
- Fixing value facts that are genuinely missing from the graph, beyond measuring them (FR-010), unless they share the misattachment's cause. That gets a follow-up issue if the measurement warrants it.
- Changing `lcg-eval graph-integrity`'s metric definitions or matching rules. Scoring `Location` as a link remains a possible later extension, as noted in #819.
- Edge-direction problems, which #816 covers.

## Source References *(optional)*

- #819: `lcg-eval graph-integrity` and the E13 baseline; see the graph-integrity evaluation doc (Value facts; Reconciliation with the E13 baseline) and ADR-0819.
- #816: ingest-time endpoint signature check and the `signature_mismatch` mark; see ADR-0816 and the ontology doc's signature section.
- #824: note-ID junk-entity filter; see ADR-0824.
- #815 / #820: E13 graph walks, the consumer harmed by misattached endpoints.
