# Feature Specification: Research: WebBrain retrieval suggestions — OR BM25 leg, eval hygiene, score semantics, merge discipline (#843, #847, #848, #849)

**Feature Branch**: `fabrik/issue-863`
**Created**: 2026-11-21
**Status**: Specified
**Input**: User description: "Research for #843, #847, #848 and #849, which are community suggestions from a review of a page-centric hybrid retrieval system. **The output is findings and recommendations, not product code.**

For each suggestion, verify it against `main` and against prior experiments. The experiments master record is `docs/retrieval-experiments.md`, landing in PR #814; read it from that branch if it hasn't merged yet. Then recommend one of three outcomes: a work issue (with a draft spec), a probe or experiment (with its design), or decline (with the evidence).

1. **#843, a BM25 leg with any-term (OR) matching, plus per-leg attribution.** Does the FTS leg of hybrid passage search (`crates/core/src/search.rs`, `sanitize_fts_query`, `fts_search_episodic`) match any term or all terms today? Was an OR leg among the E3 (#789) variants? Is per-leg attribution (dense, lexical or both) cheap to add to the response or to `lcg-eval` reports?
2. **#847, `lcg-eval` hygiene.** What do reports already record (skeleton and mapping hashes, models, fixtures)? Which of these are missing: question grouping by type (lexical against paraphrase), dense and fused results side by side, digests of every input, and `compare` refusing runs whose inputs differ?
3. **#848, scores as relevance.** What scores does each search method return today (passages in vector and hybrid modes, entities, relationships, walk, graph search)? Is any of them a fused or normalised score presented as relevance? Is any `min_score` or `min_similarity` default calibrated? Measure, on the v2 corpus, how known-irrelevant queries score against relevant ones under each mode. That decides whether #812's absolute floor is safe in hybrid mode.
4. **#849, merge discipline.** Find every place LCG merges or resolves entities or labels by embedding similarity: entity resolution at ingest, `knowledge_merge_entities` suggestions, canonicalisation, and lint near-duplicates. For each, is it single-linkage (nearest member) or centroid-based? What threshold does it use? Does the v2 graph show false merges or duplicate edges, as in the #820 parity run?

## Deliverable
`docs/research/webbrain-retrieval-suggestions.md`, with one section per suggestion and a recommendation for each, plus an update to the experiments master record if any measurement is run. Post a summary comment on each of #843, #847, #848 and #849, linking the doc. Add no product code beyond eval-only instrumentation needed for a measurement."

## Background

Four community suggestions (#843, #847, #848, #849) came out of a review of a page-centric hybrid retrieval system ("WebBrain"). Each proposes a change to how LCG retrieves, scores, evaluates, or merges. None has yet been checked against the current code or against the retrieval experiments already run (recorded in `docs/retrieval-experiments.md`, the experiments master record, which landed via PR #814).

Acting on unverified suggestions risks duplicating experiments already run (for example, whether an any-term lexical leg was among the E3 / #789 variants), changing defaults on an uncalibrated basis (for example, #812's absolute score floor in hybrid mode), or building product features for problems that do not occur in the v2 graph. This issue is research only. It turns each suggestion into an evidence-backed decision: open a work issue (with a draft spec), run a probe or experiment (with its design), or decline (with the evidence).

The four suggestions are independent questions. They share a deliverable and the v2 corpus as the measurement substrate.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Lexical leg semantics and per-leg attribution (#843) (Priority: P1)

A maintainer deciding whether to add an any-term (OR) BM25 leg to hybrid passage search needs to know what the lexical leg does today, whether this was already tested, and whether showing which leg (dense, lexical, both) produced each hit is cheap.

**Why this priority**: It bears directly on retrieval recall, the core product quality, and the experiment history (E3 / #789) may already answer it. It is the suggestion most likely to change search behaviour.

**Independent Test**: Read the #843 section of the research doc. A reader who has not seen the code can state whether the FTS leg is all-term or any-term today, whether an OR variant appeared in E3, the cost of per-leg attribution, and the recommended outcome with its supporting evidence.

**Acceptance Scenarios**:

1. **Given** the current `main`, **When** the researcher inspects the FTS leg of hybrid passage search, **Then** the doc states whether multi-term queries match any term or all terms, citing the code location.
2. **Given** `docs/retrieval-experiments.md` and the E3 (#789) record, **When** the researcher checks the variants tried, **Then** the doc states whether an OR leg was among them and, if so, its measured result.
3. **Given** the response shape and the `lcg-eval` report format, **When** the researcher assesses per-leg attribution, **Then** the doc states where it could be added, the approximate effort, and whether it changes any public response contract.
4. **Given** the findings, **When** the section concludes, **Then** it recommends exactly one of: work issue with draft spec, probe or experiment with design, or decline with evidence.

---

### User Story 2 - Score semantics and floor safety (#848) (Priority: P1)

A maintainer deciding whether #812's absolute score floor is safe in hybrid mode needs to know what each search method's score actually means, and whether known-irrelevant queries score distinguishably lower than relevant ones in each mode.

**Why this priority**: It gates a decision already in flight (#812). An uncalibrated floor in hybrid mode could silently drop relevant results or admit irrelevant ones, and it needs a measurement, not an opinion.

**Independent Test**: Read the #848 section. It contains a per-method table of returned scores and their meaning, and a measured comparison of relevant against known-irrelevant query scores per mode on the v2 corpus, ending in a safe or unsafe verdict for the hybrid-mode floor.

**Acceptance Scenarios**:

1. **Given** the search methods (passages in vector and hybrid modes, entities, relationships, walk, graph search), **When** the researcher inventories returned scores, **Then** the doc lists for each what the score is (raw similarity, fused rank score, normalised, other) and whether it is presented as relevance.
2. **Given** every `min_score` / `min_similarity` default, **When** the researcher checks provenance, **Then** the doc states whether each default was calibrated against data, and if so where.
3. **Given** the v2 corpus, a set of relevant queries and a set of known-irrelevant queries, **When** each is run under each retrieval mode, **Then** the doc reports the score distributions and the separation between the two sets per mode.
4. **Given** those distributions, **When** the section concludes, **Then** it states whether #812's absolute floor is safe in hybrid mode and, if not, what is recommended instead.

---

### User Story 3 - Merge-by-similarity inventory (#849) (Priority: P2)

A maintainer worried about false merges needs a complete inventory of every place LCG merges or resolves entities or labels by embedding similarity, how each decides (nearest member or centroid), at what threshold, and whether the v2 graph shows false merges or duplicate edges.

**Why this priority**: It affects graph integrity but is inventory-heavy and its fix, if any, is narrower than retrieval changes. Measurement on the v2 graph can reuse the #820 parity run's approach.

**Independent Test**: Read the #849 section. It contains a table with one row per merge or resolution site (location, trigger, linkage type, threshold, threshold provenance) and a finding on observed false merges and duplicate edges in the v2 graph.

**Acceptance Scenarios**:

1. **Given** the codebase, **When** the researcher searches for similarity-driven merge or resolution, **Then** the doc covers at least: entity resolution at ingest, `knowledge_merge_entities` suggestions, canonicalisation, and lint near-duplicates, and notes any others found.
2. **Given** each site, **When** the researcher classifies it, **Then** the doc states single-linkage or centroid-based and the threshold used.
3. **Given** the v2 graph, **When** the researcher samples for false merges and duplicate edges, **Then** the doc reports the findings with counts or examples, comparing against the #820 parity run.
4. **Given** the findings, **When** the section concludes, **Then** it recommends exactly one of: work issue with draft spec, probe or experiment with design, or decline with evidence.

---

### User Story 4 - `lcg-eval` report hygiene gap analysis (#847) (Priority: P2)

A maintainer who relies on `lcg-eval` comparisons needs to know what reports already record and which input-provenance and presentation features are missing, so evaluation results can be trusted and compared.

**Why this priority**: It improves trust in experiments but does not change product behaviour. It is cheap to assess by reading the report format and the `compare` command.

**Independent Test**: Read the #847 section. It contains a present/missing matrix for the four named capabilities plus what reports already record, and a recommendation.

**Acceptance Scenarios**:

1. **Given** the current `lcg-eval` report format, **When** the researcher inventories recorded fields, **Then** the doc lists what is recorded (skeleton and mapping hashes, models, fixtures, and anything else).
2. **Given** the four proposed capabilities (question grouping by type, dense and fused results side by side, digests of every input, `compare` refusing runs whose inputs differ), **When** the researcher checks each, **Then** the doc marks each as present, partial, or missing.
3. **Given** the gaps, **When** the section concludes, **Then** it recommends exactly one outcome per the issue's three options, with a draft spec if a work issue is recommended.

---

### User Story 5 - Deliverable and cross-posting (Priority: P1)

A maintainer needs one findable document carrying all four recommendations, and each originating issue needs to point to it.

**Why this priority**: Without the document and the comments, the research is not usable by the people who filed or triage the four issues.

**Independent Test**: The research doc exists at the stated path with one section per suggestion, and each of #843, #847, #848 and #849 has a summary comment linking to it.

**Acceptance Scenarios**:

1. **Given** the research is complete, **When** the doc is reviewed, **Then** it has one section per suggestion, each with a recommendation and its evidence.
2. **Given** any measurement was run, **When** the experiments master record is reviewed, **Then** it contains an entry for that measurement.
3. **Given** the doc is committed, **When** the four issues are viewed, **Then** each has a summary comment linking the doc.

---

### Edge Cases

- `docs/retrieval-experiments.md` is not on the working branch: read it from PR #814's branch or `main`, and record in the doc which revision was read.
- A suggestion is already implemented, already tried and rejected, or moot: the recommendation is decline, with the evidence cited.
- The v2 corpus has no labelled known-irrelevant queries: the researcher constructs and documents a set, and states its limits.
- Score distributions overlap heavily between relevant and irrelevant queries: the verdict is that an absolute floor is unsafe in that mode, with the overlap quantified.
- A measurement needs instrumentation: it is limited to eval-only code and is reported as such; anything needing product-code change becomes a recommended work issue, not part of this issue.
- The v2 graph shows no false merges in the sample: the doc states the sample size and method so absence of evidence is not mistaken for evidence of absence.
- Findings for one suggestion affect another (for example, per-leg attribution in #843 supports the report side-by-side view in #847): the doc cross-references rather than duplicating.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The deliverable MUST be `docs/research/webbrain-retrieval-suggestions.md`, with exactly one section per suggestion (#843, #847, #848, #849), each ending in a recommendation.
- **FR-002**: Each recommendation MUST be exactly one of: a work issue (with a draft spec), a probe or experiment (with its design), or decline (with the evidence).
- **FR-003**: Every factual claim about current behaviour MUST be verified against `main` and cite its code or document location, not rely on the issue text.
- **FR-004**: The #843 section MUST state whether the FTS leg of hybrid passage search matches any term or all terms, and cite the relevant code.
- **FR-005**: The #843 section MUST state whether an OR lexical leg was among the E3 (#789) variants and report the result if so.
- **FR-006**: The #843 section MUST assess the cost and placement of per-leg attribution (dense, lexical, both) in the response and in `lcg-eval` reports.
- **FR-007**: The #847 section MUST list what `lcg-eval` reports already record and mark each of the four proposed capabilities (question grouping by type, dense and fused results side by side, digests of every input, `compare` refusing runs whose inputs differ) as present, partial, or missing.
- **FR-008**: The #848 section MUST inventory the score each search method returns (passages in vector and hybrid modes, entities, relationships, walk, graph search), identify any fused or normalised score presented as relevance, and state whether each `min_score` / `min_similarity` default is calibrated.
- **FR-009**: The #848 section MUST include a measurement, on the v2 corpus, of score distributions for relevant against known-irrelevant queries under each retrieval mode, and a verdict on whether #812's absolute floor is safe in hybrid mode.
- **FR-010**: The #849 section MUST inventory every similarity-driven merge or resolution site, covering at least entity resolution at ingest, `knowledge_merge_entities` suggestions, canonicalisation, and lint near-duplicates, and for each state single-linkage or centroid-based and the threshold used.
- **FR-011**: The #849 section MUST report whether the v2 graph shows false merges or duplicate edges, with the sampling method and size, compared against the #820 parity run.
- **FR-012**: Any measurement run MUST be recorded as an entry in `docs/retrieval-experiments.md`, following that record's existing conventions.
- **FR-013**: The change set MUST add no product code beyond eval-only instrumentation required for a measurement, and any such instrumentation MUST be identified as eval-only in the doc.
- **FR-014**: A summary comment linking the research doc MUST be posted on each of #843, #847, #848 and #849.
- **FR-015**: Where a work issue is recommended, the doc MUST include a draft spec in the project's Spec Kit shape (user scenarios, `FR-NNN`, `SC-NNN`, edge cases, assumptions); where a probe is recommended, it MUST include the design (question, method, data, success and failure criteria).
- **FR-016**: The doc MUST record the revision of `docs/retrieval-experiments.md` and of `main` that the findings were checked against.

### Key Entities *(if applicable)*

- **Suggestion**: One of #843, #847, #848, #849, each with a question set, evidence, and a single recommendation.
- **Recommendation**: One of work issue (with draft spec), probe or experiment (with design), or decline (with evidence).
- **Measurement**: A run on the v2 corpus (score distributions; v2 graph merge sampling) recorded in both the research doc and the experiments master record.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: The research doc exists at the stated path and has four sections, one per suggestion, each containing exactly one recommendation drawn from the three allowed outcomes.
- **SC-002**: Every question posed in the issue body is answered explicitly in the corresponding section, with no question left unaddressed; where it cannot be answered, the doc says why.
- **SC-003**: The #848 verdict is backed by per-mode score distributions for relevant and known-irrelevant queries, so a reader can decide #812's floor without re-running anything.
- **SC-004**: The #849 inventory lists every similarity-driven merge or resolution site found, each with linkage type and threshold, and a reader can tell which sites have no calibrated threshold.
- **SC-005**: Each of #843, #847, #848 and #849 has a summary comment linking the doc.
- **SC-006**: The diff contains no changes to product behaviour; the only non-doc code is eval-only instrumentation, if any.
- **SC-007**: Every recommended work issue has a draft spec a Fabrik Specify stage could adopt without further context, and every recommended probe has a design runnable by someone who did not do the research.

## Assumptions

- The v2 corpus and its graph are available in the working environment, and `lcg-eval` can run against them; if not, the doc states the limitation and the affected question is marked unmeasured rather than guessed.
- `docs/retrieval-experiments.md` is already on `main` (PR #814 merged); if it is not, it is read from that PR's branch.
- "Known-irrelevant queries" can be constructed (for example off-topic questions unrelated to the corpus) when no labelled set exists, and the construction method is documented.
- The #820 parity run's results are findable in the repository or in issue history, so #849's v2 findings can be compared against them.
- A recommendation of "decline" is a legitimate outcome and carries the same evidential standard as the others.
- The summary comments on the four issues are posted by the Fabrik workflow after the doc is committed.

## Out of Scope *(optional)*

- Implementing any of the four suggestions, including the OR lexical leg, per-leg attribution in the response, report changes, score normalisation, or merge-logic changes. Those become follow-up work issues if recommended.
- Changing #812's floor or any `min_score` / `min_similarity` default; this research only informs that decision.
- Fixing any false merges or duplicate edges discovered in the v2 graph (they are reported, and a fix would be a follow-up).
- Re-running prior experiments (E3 and others) beyond what is needed to answer a question; their recorded results are cited.
- Evaluating the WebBrain system itself; only the four suggestions as applied to LCG are in scope.

## Source References *(optional)*

- Issues #843, #847, #848, #849 (the suggestions); #789 (E3 variants); #812 (absolute score floor); #820 (parity run); PR #814 (experiments master record)
- `docs/retrieval-experiments.md`
- `crates/core/src/search.rs` (`sanitize_fts_query`, `fts_search_episodic`)
