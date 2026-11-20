# Feature Specification: Passage search: belief-aware local reranker (sidecar), plus opt-in LLM query expansion

**Feature Branch**: `fabrik/issue-796`
**Created**: 2026-11-18
**Status**: Specified
**Input**: User description:

> ## Problem
>
> On the v2 retrieval corpus (#788, baseline posted on #789), LCG passage search beats qmd overall: nDCG@10 0.567 vs 0.499, a paired difference of +0.068 [+0.017, +0.116]. It loses badly on **paraphrase** questions, which describe an entity by its role ("the initiative covering shift rostering") without naming it:
>
> | paraphrase, nDCG@10 | |
> |---|---|
> | `lcg-passages` | 0.150 |
> | `qmd` (hybrid + LLM query expansion) | 0.356 |
> | `qmd-rerank` (+ Qwen3-Reranker-0.6B) | **0.492** |
>
> qmd closes that gap with two stages LCG lacks:
> - **LLM query expansion** (`qmd-query-expansion-1.7B`): keyword, semantic and HyDE variants of the query, fused.
> - **A cross-encoder reranker** over the top candidates.
>
> Both are expensive (1.7 s and 3.3 s p50, against LCG's 24 ms). The reranker also wrecks temporal questions: qmd-rerank scores 0.293 there, against 0.628 for `lcg-passages`. It scores text match and is blind to which note is current.
>
> Real users ask by role and description all the time, so this gap is not just a quirk of the test set.
>
> ## Desired behaviour
>
> Two optional stages for passage search, **each one switchable on its own and measured on its own**.
>
> ### 1. Rerank stage (the primary goal)
> - After first-stage retrieval, rescore the top N candidates (N about 20–30, configurable) with a **small local cross-encoder**: a BGE-reranker-base / bge-reranker-v2-m3 class model, or similar.
> - Serve it from the Swift local-inference sidecar (`native/local-inference`, CoreML) so that Liminis Notes gets it with no new process.
> - LCG reaches it through the existing UDS/HTTP embedder transport pattern, e.g. a `/v1/rerank` route.
> - **Belief-aware:** combine the reranker score with LCG's belief and recency signals so the stage cannot repeat qmd's temporal collapse. A superseded note must not climb above the current one on text match alone.
> - Budget: p50 total ≤ 150 ms for k = 10 on Apple silicon.
> - Off by default until measured. A client opts in per call (`rerank: true`) or through config.
>
> ### 2. Query expansion (opt-in only)
> - Optionally rewrite the query with a local LLM into lexical, semantic and hypothetical-answer variants, and fuse them with RRF. The sidecar already hosts a Qwen model; LCG's extractor config is a precedent for choosing the LLM.
> - It costs about a second per query, so it is **never the default**. It's meant for deliberate agent searches, not search-as-you-type.
>
> ## Acceptance (measured on v2 with `lcg-eval retrieval`; report per category with bootstrap intervals)
>
> - **Rerank:**
>   - Paraphrase nDCG improves with a paired interval above zero.
>   - Overall nDCG doesn't drop.
>   - Temporal nDCG doesn't drop: its paired interval must include or exceed zero against `lcg-passages`.
>   - p50 is within budget.
> - **Expansion:** its gain is reported per category on top of rerank, and on its own. Ship it only if it pays for its latency on some category, and document when to use it.
> - New eval configs: `lcg-passages-rerank` and `lcg-passages-expand-rerank`, next to the existing ones.
> - Clients that don't opt in see no behaviour change.
>
> ## Sequencing and dependencies
>
> - Depends on **#788** (corpus v2 with corrected relevance labels) for measurement.
> - Measure after **#790** (query/document embedding roles), so we know how much of the paraphrase gap the query instruction alone closes.
> - Independent of **#789** (hybrid BM25 + vector). Report results with and without it.
> - Notes-side follow-up after an lcg release: the app enables rerank, and the sidecar ships the reranker model (same asset pipeline as BGE).
>
> ## Questions for Research
> - Which reranker gives the best quality per millisecond in CoreML at 512 tokens: bge-reranker-base, bge-reranker-v2-m3 or Qwen3-Reranker-0.6B?
> - How should the belief/recency signal combine with the reranker score: a weighted blend, a demotion of superseded episodes before reranking, or a tiebreak?
> - Should rerank also apply to `knowledge_find_relationships` and `knowledge_find_entities` results?

## Background

Passage search (`knowledge_search_passages`) is vector-only and fast (about 24 ms p50). On the v2 retrieval corpus it already beats qmd overall (nDCG@10 0.567 vs 0.499, paired difference +0.068 [+0.017, +0.116]) and is far ahead on **temporal** questions (0.628 vs the qmd reranker's 0.293), because LCG knows which note is current.

It loses on **paraphrase** questions: the user describes an entity by role ("the initiative covering shift rostering") without naming it. `lcg-passages` scores 0.150 nDCG@10; qmd with LLM query expansion scores 0.356; qmd plus a cross-encoder reranker scores 0.492. Real users ask this way routinely, so the gap matters beyond the test set.

qmd closes the gap with two stages LCG lacks: LLM query expansion and a cross-encoder reranker. Both are slow (1.7 s and 3.3 s p50). The reranker also collapses temporal questions because it scores text match only and cannot tell a superseded note from the current one. LCG's belief-time model (superseded episodes, see ADR-0705) is the signal that avoids this.

This feature adds the two stages to passage search as independent, opt-in, separately measurable options. The reranker is the primary goal; query expansion ships only if the measurements justify its latency. The retrieval eval architecture (ADR-0754) and corpus v2 (ADR-0788) provide the measurement harness (`lcg-eval retrieval`).

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Find a note by role or description (Priority: P1)

A Liminis Notes user (or an agent acting for them) searches for something by what it does rather than by name ("the initiative covering shift rostering"). With rerank enabled, the correct passage ranks near the top instead of being buried by lexical or embedding mismatch.

**Why this priority**: This is the measured weakness and the core value. Without it nothing else in the feature matters.

**Independent Test**: Run `lcg-eval retrieval` on corpus v2 with `lcg-passages` and `lcg-passages-rerank`; compare paraphrase-category nDCG@10 with a paired bootstrap interval.

**Acceptance Scenarios**:

1. **Given** corpus v2 and a passage search with rerank enabled, **When** paraphrase questions are evaluated, **Then** paraphrase nDCG@10 exceeds `lcg-passages` with a paired bootstrap interval entirely above zero.
2. **Given** the same run, **When** overall nDCG@10 is compared with `lcg-passages`, **Then** it does not drop.
3. **Given** a rerank-enabled search with k = 10 on Apple silicon, **When** latency is measured across the eval questions, **Then** p50 total latency is ≤ 150 ms.

---

### User Story 2 - Current notes stay above superseded ones (Priority: P1)

A user asks a question whose answer changed over time. Reranking must not lift an outdated (superseded) note above the current one merely because its text matches the query slightly better.

**Why this priority**: The stated reason qmd's reranker is unusable here is its temporal collapse (0.293 vs 0.628). Repeating that would make the feature a regression for existing strengths.

**Independent Test**: Evaluate temporal-category questions with `lcg-passages` vs `lcg-passages-rerank`; compute the paired interval of the nDCG@10 difference. Also run a targeted case with a superseded and a current note where the superseded one has stronger text match.

**Acceptance Scenarios**:

1. **Given** a superseded note and its current replacement, with the superseded note scoring higher on pure text match, **When** a reranked search runs, **Then** the current note is ranked above the superseded one.
2. **Given** corpus v2 temporal questions, **When** `lcg-passages-rerank` is compared with `lcg-passages`, **Then** the paired nDCG@10 difference interval includes or exceeds zero.

---

### User Story 3 - No change for clients that don't opt in (Priority: P1)

An existing client (the current Liminis app, MCP clients, the eval baselines) keeps calling passage search exactly as before and sees identical results and latency.

**Why this priority**: LCG's IPC contract is consumed by a shipped app; breaking it breaks the app. The feature is "off by default until measured".

**Independent Test**: Run existing passage-search tests and IPC parity tests unchanged; compare results of a default call before and after.

**Acceptance Scenarios**:

1. **Given** a passage search call with no rerank or expansion parameter and no config enabling them, **When** it runs, **Then** results, ordering and response shape are identical to the pre-feature behaviour.
2. **Given** rerank is requested but the reranker service is unavailable, **When** the search runs, **Then** it returns the un-reranked first-stage results rather than failing, and the response indicates that reranking did not apply.

---

### User Story 4 - Opt in per call or through configuration (Priority: P2)

A client enables rerank for a single call (`rerank: true`), or an operator enables it as a workspace default via configuration. An explicit per-call value overrides the configured default. The number of candidates rescored is configurable (default 50, maximum 50).

**Why this priority**: Needed to ship the feature safely and let Notes adopt it, but the ranking behaviour is the more valuable slice.

**Independent Test**: Call passage search with `rerank: true` / `rerank: false` under both config states and observe whether the rerank stage ran.

**Acceptance Scenarios**:

1. **Given** rerank disabled in config, **When** a call passes `rerank: true`, **Then** the stage runs for that call only.
2. **Given** rerank enabled in config, **When** a call passes `rerank: false`, **Then** the stage is skipped.
3. **Given** a candidate-count setting of N, **When** rerank runs, **Then** exactly the top N first-stage candidates (or all, if fewer) are rescored.

---

### User Story 5 - Deliberate agent search with query expansion (Priority: P3)

An agent doing a deliberate, non-interactive search opts in to query expansion: a local LLM rewrites the query into lexical, semantic and hypothetical-answer variants whose results are fused with reciprocal-rank fusion (RRF), optionally followed by rerank. It is never on by default and never used for search-as-you-type.

**Why this priority**: Costs about a second per query and ships only if measurements show it pays off on some category. It is independently switchable and independently measured.

**Independent Test**: Evaluate `lcg-passages-expand-rerank` (expansion + rerank) and expansion alone against `lcg-passages` and `lcg-passages-rerank`, per category with bootstrap intervals.

**Acceptance Scenarios**:

1. **Given** a call with expansion opted in, **When** it runs, **Then** the query is expanded into lexical, semantic and hypothetical-answer variants and the per-variant results are RRF-fused before any rerank.
2. **Given** expansion and rerank are each independently toggleable, **When** any of the four combinations is requested, **Then** exactly the requested stages run.
3. **Given** the local LLM is unavailable, **When** expansion is requested, **Then** search falls back to the original query's results and signals that expansion did not apply.

---

### User Story 6 - Measured, reported decision (Priority: P2)

A maintainer can reproduce the evidence for shipping or withholding each stage. The eval harness has `lcg-passages-rerank` and `lcg-passages-expand-rerank` configs alongside existing ones, and reports per-category nDCG with bootstrap intervals and latency percentiles, including expansion's gain on its own and on top of rerank.

**Why this priority**: Acceptance for both stages is defined by these measurements.

**Independent Test**: Run `lcg-eval retrieval` with the new configs and inspect the per-category report.

**Acceptance Scenarios**:

1. **Given** corpus v2, **When** `lcg-eval retrieval` runs the new configs, **Then** the report shows per-category nDCG@10 with paired bootstrap intervals against `lcg-passages` and p50 latency.
2. **Given** the measurements, **When** expansion shows no category where its gain justifies its latency, **Then** the documentation records that finding and expansion is not recommended for any use (or is not shipped).

---

### Edge Cases

- Fewer first-stage candidates than N: rescore what exists; none: return empty without calling the reranker.
- Reranker sidecar absent, timing out, or erroring: degrade to first-stage order and indicate it in the response; never fail the search.
- Candidates lacking belief/recency information (e.g., passages from non-episodic sources): treated neutrally, neither boosted nor demoted.
- Candidate text longer than the reranker's input limit (512 tokens in the evaluation): truncated deterministically; behaviour documented.
- Ties between candidates after combining signals: resolved deterministically.
- Requested result count k larger than N: the remainder is returned in first-stage order after the reranked head, with no duplicates.
- Expansion produces malformed or empty variants: use the original query alone.
- Expansion requested without rerank, and vice versa: each works independently.
- Non-macOS or non-Apple-silicon hosts, where the CoreML sidecar is not available: the default (stages off) is unaffected, and opt-in degrades as in the unavailable-service case.
- Concurrent searches: the reranker transport follows the same pooling and retry-boundary rules as the existing embedder transport.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: Passage search MUST support an optional rerank stage that rescores the top N first-stage candidates with a local cross-encoder and reorders them.
- **FR-002**: N MUST be configurable, defaulting to 50 and clamped to at most `MAX_RERANK_CANDIDATES = 50` (the default equals the cap). The default was chosen from the maintainer's measured sweep on corpus v2 (see ADR-0796), superseding the original "about 20–30" estimate.
- **FR-003**: Rerank MUST be off by default. A client MUST be able to enable it per call with `rerank: true` and via configuration; an explicit per-call value overrides configuration.
- **FR-004**: The cross-encoder MUST be served locally by the existing Swift local-inference sidecar (CoreML), with no additional process, and reached by LCG over the same UDS/HTTP transport pattern as the embedder (for example a `/v1/rerank` route).
- **FR-005**: The final order of reranked results MUST combine the reranker score with LCG's belief and recency signals such that a superseded note does not rank above its current replacement on text match alone.
- **FR-006**: Rerank MUST degrade gracefully: if the reranker is unreachable, times out, or errors, passage search MUST return first-stage results and indicate in the response that reranking did not apply.
- **FR-007**: Passage search MUST support an optional, independently switchable query-expansion stage in which a local LLM produces lexical, semantic and hypothetical-answer variants of the query, and per-variant results are fused with RRF.
- **FR-008**: Query expansion MUST never be enabled by default and MUST be switchable independently of rerank; the combination expansion + rerank MUST run expansion/fusion first, then rerank.
- **FR-009**: The LLM used for expansion MUST be selectable through configuration, following the precedent of the extractor configuration, and default to the model the sidecar already hosts.
- **FR-010**: Expansion MUST degrade gracefully when the LLM is unavailable or returns unusable output, falling back to the original query and indicating that expansion did not apply.
- **FR-011**: A call that does not opt in to either stage MUST produce results, ordering, response shape and latency unchanged from before this feature; existing IPC and MCP contract tests MUST pass without modification beyond additions.
- **FR-012**: New parameters MUST be exposed consistently on every passage-search surface (IPC dispatch and the MCP-over-stdio tool registry), with the registry's documented counts and schemas kept aligned.
- **FR-013**: `lcg-eval retrieval` MUST provide configs `lcg-passages-rerank` and `lcg-passages-expand-rerank` next to the existing ones, plus a way to measure expansion on its own, each reporting per-category nDCG with paired bootstrap intervals and latency percentiles.
- **FR-014**: Measurement MUST be run after #790 (query/document embedding roles) lands, and results MUST be reported both with and without hybrid BM25 + vector (#789) where #789 is available, so the contribution of each change is attributable.
- **FR-015**: The model choice for the reranker and the belief/recency combination method MUST be justified by measurement and recorded in an ADR numbered by this issue (per repository convention), including quality-per-millisecond comparison of the candidate rerankers at 512 tokens.
- **FR-016**: User-facing documentation MUST state when to use each stage, the latency cost, and how to enable it; for expansion it MUST say when it is worth its latency (or that no category justified it).
- **FR-017**: A scoping decision on whether rerank also applies to `knowledge_find_relationships` and `knowledge_find_entities` MUST be made during Research and recorded; this issue's required deliverable covers passage search only.

### Key Entities *(if applicable)*

- **Candidate passage**: a first-stage result with text, first-stage score, and belief/recency information (current vs superseded episode, ingest/belief time).
- **Rerank score**: the cross-encoder's relevance score for a (query, candidate) pair; combined with belief and recency signals to give the final ordering.
- **Query variant**: a lexical, semantic or hypothetical-answer rewrite of the query produced by the expansion stage; each yields its own ranked list for RRF fusion.
- **Reranker/expansion service endpoint**: the sidecar capability LCG reaches over its local transport.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: On corpus v2, paraphrase-category nDCG@10 for `lcg-passages-rerank` improves on `lcg-passages` with a paired bootstrap interval whose lower bound is above zero.
- **SC-002**: Overall nDCG@10 for `lcg-passages-rerank` is not lower than `lcg-passages`.
- **SC-003**: Temporal-category nDCG@10 for `lcg-passages-rerank` has a paired interval against `lcg-passages` that includes or exceeds zero (no statistically supported drop).
- **SC-004**: p50 total latency of a rerank-enabled passage search at k = 10 on Apple silicon is ≤ 150 ms.
- **SC-005**: Per-category nDCG@10 with bootstrap intervals is reported for expansion on its own and on top of rerank; expansion ships (and is documented as recommended) only if it improves some category with an interval above zero in a way that justifies its latency, otherwise the finding is documented.
- **SC-006**: Calls without opt-in return byte-identical results to the pre-feature behaviour, and the existing test suites (including IPC parity) pass.
- **SC-007**: With the reranker or LLM unavailable, 100% of opted-in searches still return a result set (first-stage or un-expanded) rather than an error.

## Assumptions

- Corpus v2 (#788) is merged; #790 lands before final measurements are taken. This feature can be developed and unit-tested before then, but acceptance numbers come after #790.
- Hybrid BM25 + vector (#789) is independent; if it has not landed, results are reported without it, and with it once available.
- "Belief and recency signals" mean the existing superseded-episode (belief-time, ADR-0705) and ingest-time information already stored in the graph; no new ingestion-time data is introduced by this feature.
- Rerank is delivered through the sidecar on Apple silicon (the product's target); other platforms simply do not get the stage (graceful degradation).
- The Notes app enabling rerank, and the sidecar shipping the reranker model asset (same pipeline as BGE), are follow-up work after an LCG release; this issue delivers the LCG side, the sidecar-side route/model serving needed to measure it, and the eval configs.
- Open design questions in the issue (which reranker model, how to combine the belief signal, application to entity/relationship search) are resolved by the Research stage with measurements, not by this spec; the requirements above hold for any outcome.
- Per-call parameter naming follows the issue (`rerank: true`); the expansion parameter name is chosen in planning, consistent with it.
- Search-as-you-type clients are expected to leave both stages off; no automatic latency-based switching is provided.

## Out of Scope *(optional)*

- Making rerank or expansion the default (a follow-up once measured and adopted by the app).
- The Liminis Notes app changes and shipping the reranker model asset in the app bundle.
- Hybrid BM25 + vector retrieval (#789) and query/document embedding roles (#790).
- Corpus or relevance-label changes (#788).
- Fine-tuning or training any reranker or expansion model.
- Reranking for `knowledge_find_relationships` and `knowledge_find_entities` beyond recording the Research decision (FR-017); implementing it would be a follow-up issue.
- Automatic per-query latency routing between stages.

## Source References *(optional)*

- #788 (corpus v2), #789 (hybrid baseline posted there), #790 (embedding roles)
- ADR-0754 (retrieval eval architecture), ADR-0788 (retrieval corpus v2 and bootstrap reporting), ADR-0705 (belief time separate from validity), ADR-0006 (embedder HTTP contract), ADR-0039 (UDS embedder connection pooling)
- `native/local-inference` (Swift CoreML sidecar)
