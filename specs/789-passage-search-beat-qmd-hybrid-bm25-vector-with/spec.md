# Feature Specification: Hybrid BM25 + vector passage search with RRF and top-rank bonus

**Feature Branch**: `fabrik/issue-789`
**Created**: 2026-11-18
**Status**: Specified
**Input**: User description: "Passage search: beat qmd. Hybrid BM25+vector with RRF, belief-aware ranking, optional query expansion, explain trace

## Problem

qmd (tobi/qmd) is the search half of the \"LLM wiki\" setups that Liminis Notes is compared against. In the first real #754 eval run (with PR #786 applied, Fernhill fixture, k=10), it edges out LCG:

| | recall@10 | MRR | nDCG@10 | p50 |
|---|---|---|---|---|
| lcg-passages | 0.963 | 0.740 | 0.789 | 8 ms |
| qmd-rerank | 0.975 | 0.766 | 0.817 | 1.8 s |

By category, qmd wins **answer** questions (MRR 0.95 vs 0.85, nDCG 0.963 vs 0.889) and **paraphrase** questions (recall 1.0 vs 0.95). Neither wins **temporal** questions: both put the superseded note above the current one 90% of the time. LCG wins **direct** questions and latency.

The cause is visible in the code. `search_passages` (`crates/core/src/search.rs`) is **pure vector cosine**. Entity search already does hybrid BM25 + HNSW with RRF fusion, but passage search does not. qmd does BM25 + vector + RRF with a top-rank bonus, and optional LLM query expansion and reranking.

## Goal

Beat qmd-rerank on the v2 corpus (#788): overall nDCG@10, and every category, with non-overlapping or clearly reported CIs. Keep the default passage-search p50 under 100 ms. Each step below is measured on its own, and a step that doesn't help is dropped.

## Steps, in order

1. **Hybrid passage search.** BM25 over episode text plus HNSW, fused with RRF. This reuses the entity-search fusion code and the existing FTS machinery. Add qmd's **top-rank bonus**: extra weight for documents ranked first, or in the top 3, by either retriever. Make it the default once it measures better. It targets the answer-question gap.
2. **Embedder task prefixes.** Configurable query and document instruction prefixes, for models that expect them (for example nomic's `search_query:` / `search_document:`, or e5's `query:` / `passage:`). The eval ran nomic without prefixes, so LCG's vectors may be underperforming. Changing document prefixes needs a re-embed: say how the existing embeddings are handled.
3. **Belief-aware ranking: the axis qmd can't compete on.** qmd has no idea a note was superseded. Add `believed_only` and/or a demotion factor for passages whose episode belief has ended (revision, `chunk_removed`, `doc_id` supersession, ADR-0705/0707). This targets the temporal category, where both systems currently fail. It depends on #787 being resolved.
4. **Query expansion, optional and off by default.** A small local model generates lexical variants, semantic rewrites and a hypothetical-answer passage, like qmd's expansion. Each variant is searched, and the results are fused with the original query weighted higher. It targets paraphrase recall. Report the added latency, and enable it per request.
5. **An `explain` trace.** An opt-in per-result breakdown: BM25 rank, vector rank, fused score, bonus, belief adjustment, and which expansion variant matched. This is for debugging, and so the Notes agent can say why a passage was returned.
6. **Reranking: measure only.** In the first run, qmd's reranker added +0.004 nDCG. Don't build it unless v2 shows a real gap after steps 1–4.

## Acceptance

- Each step lands with an eval run on #788 (and v1 as a smoke test). The report includes before and after numbers with CIs, and the latency.
- Default-path changes (steps 1–3) don't regress direct questions or latency beyond the stated bound.
- `origins` filtering (ADR-0752) keeps working on every new path.
- The MCP / IPC reference docs are updated for any new parameters.

## Related
#754, #786, #787, #788, ADR-0752, ADR-0705 / ADR-0707. Background: the 2026-11 agent-memory survey (qmd: query expansion, RRF with top-rank bonus, rerank blend, `--explain`)."

Maintainer scope updates from the issue discussion (2026-11-18), which narrow the original request:
- Step 2 (embedder task prefixes) moved to #790 (landed: ADR-0790). This issue is measured on top of whichever embedding profile is active.
- Query expansion (step 4) and reranking (step 6) are tracked in #796; graph-aware fused search is #797. "Keep this issue to hybrid BM25 + vector with RRF for `knowledge_search_passages`, measured per category on v2, so that each lever shows its own gain. #797 uses this issue's BM25 passage list as one of its fused inputs."

## Background

`knowledge_search_passages` currently ranks episode passages by vector cosine alone. Entity and fact search already fuse BM25 and HNSW results with Reciprocal Rank Fusion, and an episode full-text index already exists, but passages do not use it. The competing tool qmd fuses BM25 and vector rankings with a bonus for top-ranked documents, which is why it edges out LCG on *answer*-type questions (where the exact wording of the query appears in the note).

Provisional v2 baseline (corpus `meridian-notebook-2`, #792; BGE-base sidecar embedder with no query instruction; k=10; Sonnet grounded judge; 95% bootstrap CIs; *provisional* because #788 is still fixing incomplete relevance labels):

| config | nDCG@10 | recall@10 | grounded recall | p50 |
|---|---|---|---|---|
| `lcg-passages` | 0.567 [0.518, 0.614] | 0.776 | 0.883 | 24 ms |
| `qmd` | 0.499 [0.460, 0.538] | 0.807 | 0.600 | 1.7 s |
| `qmd-rerank` | 0.455 [0.412, 0.498] | 0.717 | 0.633 | 3.3 s |

nDCG@10 by category: direct 0.756 / 0.613 / 0.569; paraphrase 0.150 / 0.356 / 0.492; temporal 0.628 / 0.555 / 0.293; answer 0.789 / 0.700 / 0.575; multi-hop 0.655 / 0.447 / 0.397; passage 0.477 / 0.315 / 0.411 (columns: `lcg-passages` / `qmd` / `qmd-rerank`).

The v2 gap is concentrated in **paraphrase** questions, which refer to an entity by role without using words that appear in the note. Hybrid BM25 may *hurt* those, because the descriptor words are absent from the note, so the change must be measured per category and not just overall. Temporal is currently a strength and must not regress.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Hybrid passage ranking (Priority: P1)

A Notes agent or eval harness calls `knowledge_search_passages` and gets passages ranked by a fusion of lexical (BM25 over episode text) and semantic (vector) retrieval, with extra weight for passages that either retriever ranks at or near the top, so that questions whose answer shares exact wording with the note rank the right note higher.

**Why this priority**: This is the core of the issue and the lever that targets the *answer*-question gap. #797 builds on its BM25 list.

**Independent Test**: Run `knowledge_search_passages` on a fixture where a note matches the query by exact terms but is semantically distant, and one that matches semantically but shares no terms. Both appear in the result; the exact-term note ranks higher than under vector-only search.

**Acceptance Scenarios**:

1. **Given** a note that contains the query's distinctive terms but whose embedding is only moderately similar, **When** hybrid search runs, **Then** it ranks higher than under vector-only search.
2. **Given** a note that is a semantic paraphrase match with no shared terms, **When** hybrid search runs, **Then** it is still returned (vector retriever is not suppressed by an empty BM25 list).
3. **Given** a document ranked first, or in the top 3, by either retriever, **When** fusion runs, **Then** it receives the top-rank bonus and the bonus is reflected in its fused ordering relative to a document ranked lower by both.
4. **Given** a query with no lexical matches at all, **When** hybrid search runs, **Then** the result equals what the vector retriever alone would return (no error, no empty result).

---

### User Story 2 - Choose retrieval mode per request, and default only when proven (Priority: P1)

A caller (or eval harness) can explicitly request vector-only or hybrid passage search, so that before/after numbers can be produced from the same build, and the default becomes hybrid only once measurement shows it is better.

**Why this priority**: The issue requires that each lever is measured on its own and dropped if it does not help. Without a per-request switch the A/B cannot be run on one binary.

**Independent Test**: Call the same query with each mode and confirm the vector-only mode returns exactly what the pre-change implementation returned.

**Acceptance Scenarios**:

1. **Given** a request that selects vector-only mode, **When** search runs, **Then** results match the pre-change behavior (same ordering and scores).
2. **Given** a request with no mode specified, **When** search runs, **Then** the configured default mode is used, and that default is hybrid only if the eval gate in this spec passed.
3. **Given** an unknown mode value, **When** the request is made, **Then** it is rejected with a validation error rather than silently ignored.

---

### User Story 3 - Measured per-category eval report (Priority: P1)

The maintainer runs the #754 eval harness against the v2 corpus (#788) and the v1 corpus (smoke) and gets before/after nDCG@10, recall@10, MRR with 95% CIs, per category and overall, plus passage-search latency, so the decision to adopt hybrid (and the top-rank bonus separately) is evidence-based.

**Why this priority**: The acceptance criteria require an eval report; the paraphrase risk makes per-category numbers essential.

**Independent Test**: The report exists, includes vector-only, hybrid without bonus, and hybrid with bonus as separate rows, per category, with CIs and p50/p95 latency.

**Acceptance Scenarios**:

1. **Given** the v2 corpus, **When** the eval is run for vector-only, hybrid (no bonus) and hybrid (with bonus), **Then** the report shows each lever's own contribution per category.
2. **Given** the v1 corpus, **When** the same eval is run, **Then** results are included as a smoke test and do not show an unexplained large regression.
3. **Given** the hybrid configuration regresses a category beyond the criteria below, **When** the report is reviewed, **Then** the gap is stated explicitly and the default is not flipped (or the offending component is dropped).

---

### User Story 4 - Origin filtering and group filtering keep working (Priority: P2)

A caller that restricts passage search by `origins` (ADR-0752) or `group_ids` gets only matching episodes from both retrievers, in hybrid mode as in vector-only mode.

**Why this priority**: An explicit acceptance criterion; a lexical path that ignores filters would leak out-of-scope passages.

**Independent Test**: Fixture with episodes of several origins in several groups; query with an `origins` filter and a `group_ids` filter in hybrid mode and confirm no excluded episode appears, including ones that match only lexically.

**Acceptance Scenarios**:

1. **Given** an episode that matches the query's terms strongly but has an excluded origin, **When** hybrid search runs with an `origins` filter, **Then** it is absent.
2. **Given** an episode in a group not listed in `group_ids`, **When** hybrid search runs, **Then** it is absent.
3. **Given** an empty `origins` array, **When** the request is made, **Then** it is rejected as today.

---

### User Story 5 - Documented interface (Priority: P2)

An integrator reading the IPC / MCP reference sees the new parameter(s) and the changed meaning of `score` and `min_score` for `knowledge_search_passages`, and the MCP tool schema matches.

**Why this priority**: Acceptance criterion; Notes and the MCP tool surface consume this API.

**Independent Test**: The reference docs and the MCP tool input schema list the same parameters; the docs describe score semantics.

**Acceptance Scenarios**:

1. **Given** the IPC reference, **When** a reader looks up `knowledge_search_passages`, **Then** the mode parameter, its values, its default, and the score/`min_score` semantics are documented.
2. **Given** the MCP tool registry, **When** the tool schema is listed, **Then** it exposes the same parameter.

---

### Edge Cases

- Query with no lexical tokens after tokenisation (punctuation only, stop-word only): the lexical retriever contributes nothing; the result falls back to vector ranking without error.
- Query containing characters that are special in the full-text query syntax: handled safely, with no query-parse error surfacing to the caller.
- Episode full-text index missing or still building (for example degraded mode, or right after a rebuild): hybrid search degrades to vector-only and does not fail; the degradation is observable to operators.
- Embedder unavailable but the lexical index is available: behavior is consistent with the existing entity-search policy; it is not silently worse than today's error behavior for passage search.
- `num_results` larger than the number of candidates: returns what exists; no duplicates after fusion.
- The same episode returned by both retrievers appears once.
- Superseded episodes (`revision`, `chunk_removed`) are still returned by default, as documented today. Belief-aware demotion is not part of this issue.
- Very long episodes or documents: lexical scoring must not make latency unbounded.
- Ties in fused score are broken deterministically.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: `knowledge_search_passages` MUST support a hybrid mode that retrieves candidates from a lexical (BM25) ranking over episode text and from the existing vector ranking, and fuses the two ranked lists with Reciprocal Rank Fusion using the same fusion semantics as entity search.
- **FR-002**: Fusion MUST apply a top-rank bonus: a passage ranked first, or in the top 3, by either retriever receives additional weight. The bonus MUST be switchable independently of RRF fusion (at least for evaluation) so its contribution can be measured on its own.
- **FR-003**: The request MUST accept a retrieval-mode selector with at least `vector` (pre-change behavior, unchanged) and `hybrid`. An unrecognised value MUST be rejected with a validation error.
- **FR-004**: `vector` mode MUST return results identical in membership, order and score to the pre-change implementation.
- **FR-005**: The default mode when none is specified MUST be configurable and MUST ship as `hybrid` only if the eval gate (SC-001 to SC-004) passes; otherwise it ships as `vector` and the hybrid evidence is documented.
- **FR-006**: The `origins` filter (ADR-0752) and `group_ids` filter MUST be applied to both the lexical and vector candidate lists, so no excluded episode can enter through either path.
- **FR-007**: The meaning of `min_score` and of the returned `score` in hybrid mode MUST be defined and documented. `min_score` continues to act as a floor on semantic similarity from the vector retriever, and a passage found only by the lexical retriever is not excluded by it. The returned `score` orders results, and the response MUST still let a caller recover semantic similarity where the vector retriever matched.
- **FR-008**: When the lexical retriever yields no results, or the episode full-text index is unavailable, hybrid mode MUST fall back to vector-only ranking rather than returning an error or an empty list.
- **FR-009**: Query text MUST be sanitised for the full-text query syntax, so that arbitrary user queries never cause a parse error.
- **FR-010**: Each passage appears at most once in results, and ordering is deterministic for equal fused scores.
- **FR-011**: The lexical ranked list MUST be available as a distinct, reusable intermediate (not only as part of the fused output), since #797 consumes it as an input to graph-aware fusion.
- **FR-012**: The IPC / MCP reference docs and the MCP tool input schema for `knowledge_search_passages` MUST be updated for every new or changed parameter, with the tool registry tests updated alongside.
- **FR-013**: An eval report on the v2 corpus (#788) and the v1 corpus (smoke) MUST be produced, with vector-only, hybrid-without-bonus and hybrid-with-bonus as separate configurations, reporting nDCG@10, recall@10 and MRR with 95% CIs, overall and per category, plus p50 and p95 latency.
- **FR-014**: Existing behavior that is not part of this change MUST be preserved: superseded episodes are still returned by default, and the existing response fields remain present.
- **FR-015**: The decision recorded for each lever (RRF fusion, top-rank bonus) MUST state whether it was kept or dropped and why, citing the eval numbers.

### Key Entities *(if applicable)*

- **Passage (Episodic node)**: A chunk of note text with an embedding, group, origin and belief/supersession metadata; the unit being ranked.
- **Lexical ranked list**: Ordered passages from BM25 over episode text, after group and origin filtering.
- **Vector ranked list**: Ordered passages from nearest-neighbor retrieval over passage embeddings, after group and origin filtering.
- **Fused result**: A passage's position after fusion, carrying its fused score and the semantic similarity where the vector retriever matched.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: On v2, hybrid (with the bonus, if kept) has overall nDCG@10 at or above the vector-only `lcg-passages` baseline, and the report states the paired difference with its CI.
- **SC-002**: On v2, no category (direct, paraphrase, temporal, answer, multi-hop, passage) regresses against vector-only beyond its CI. In particular, temporal stays at least as good as today and paraphrase is explicitly reported. If a category regresses, the report says so and the default is not flipped on that evidence alone.
- **SC-003**: On v2, the *answer*-question category improves over vector-only, with the gain reported with a CI. This was the target gap, and if there is no gain the lever is dropped.
- **SC-004**: Default-path passage-search p50 latency stays under 100 ms on the eval corpora, reported with p95.
- **SC-005**: On v1 (smoke), no category shows a regression larger than its CI, or any larger regression is explained in the report.
- **SC-006**: With `origins` or `group_ids` set, 0 excluded episodes appear in hybrid results across the test fixtures, including episodes that match only lexically.
- **SC-007**: Vector mode reproduces pre-change results exactly on the existing passage-search tests.
- **SC-008**: The report shows RRF fusion alone and the top-rank bonus as separate rows, so each lever's own gain is visible.

## Assumptions

- #790 (embedder query/document roles and instruction profiles) has landed. Measurements use whichever embedding profile is active, and the report names it. Re-measuring under a different profile is not required.
- The episode full-text index already created for passage content is the BM25 source. No new text-indexing mechanism is introduced.
- The v2 relevance labels are still being corrected in #788, so numbers are provisional until that closes. The report states the corpus and label revision used.
- `min_score` keeps its current default and clamping, applied only to the vector retriever's similarity (FR-007). Lexical-only hits are admitted regardless. Callers needing strict similarity floors use `vector` mode. This is a deliberate, documented behavior change for hybrid mode.
- Result `score` in hybrid mode is the fused score and no longer a cosine similarity. The semantic similarity is returned in a separate field where available. Consumers that rely on `score` being cosine should select `vector` mode.
- The top-rank bonus uses qmd-style weights (extra weight for rank 1 and for ranks 2–3) as a starting point. The exact weights are tuned on v1/v2 during implementation and recorded with the report.
- Following the 2026-11-18 maintainer scope note, belief-aware ranking (original step 3), the `explain` trace (original step 5), query expansion (original step 4) and reranking (original step 6) are not in this issue. They are tracked separately (#796 covers expansion and reranking) or remain follow-ups.
- Passage search has no new required parameters, so existing callers work unchanged. A caller sending no mode gets the configured default.

## Out of Scope *(optional)*

- Embedder task prefixes and query/document roles (moved to #790, already landed).
- Query expansion and reranking (#796).
- Graph-aware fused search (#797). This issue only exposes the lexical list it will consume.
- Belief-aware ranking, `believed_only`, and supersession demotion (original step 3). Superseded passages remain returned by default. File as a follow-up once #787 is resolved and the temporal category is the target.
- The per-result `explain` trace (original step 5). File as a follow-up.
- Changes to entity or fact search.
- Changes to the eval corpora or relevance labels (#788, #792).
- Re-embedding existing passages.

## Source References *(optional)*

- Issue #789 and its discussion, including the provisional v2 baseline and the scope note (2026-11-18)
- Related: #754 (retrieval eval architecture, ADR-0754), #786, #787, #788, #790 (ADR-0790), #792, #796, #797
- ADR-0752 (episode origin), ADR-0705 / ADR-0707 (belief time, document supersession)
- qmd (tobi/qmd): BM25 + vector with RRF and top-rank bonus
