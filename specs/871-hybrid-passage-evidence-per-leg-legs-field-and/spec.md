# Feature Specification: Hybrid passage evidence: per-leg `legs` field and honest score docs (implements #848)

**Feature Branch**: `fabrik/issue-871`
**Created**: 2026-11-21
**Status**: Specified
**Input**: User description: "Implements #848. Spec drafted by the #863 research; full evidence in [docs/research/webbrain-retrieval-suggestions.md](https://github.com/liminisapp/liminis-context-graph/blob/main/docs/research/webbrain-retrieval-suggestions.md) (section #848, measurement E15).

## Why
In hybrid mode `score` is a rank-only fusion value (at most about 0.0233), not a relevance, and a passage doesn't say which legs found it. E15 on v2: #812's 0.5 `min_score` floor is safe (it keeps 99.3% of answerable top-1s) but it doesn't make hybrid abstain on off-topic queries: hybrid returns hits for 25 of 34 off-topic queries, against 13 in vector mode, because unfloored BM25 refills the list. Lexical-only hits can be spotted today only by `similarity == null`, and nothing documents that.

**User Scenarios & Testing**

- **Story 1 (P1): a client can tell what backed a hybrid hit.** A caller of `knowledge_search_passages` with `mode: hybrid` sees, on each passage, which legs retrieved it. *Acceptance:* a passage retrieved only by BM25 reports `legs: \"lexical\"`; only by the vector leg `\"dense\"`; by both `\"both\"`. `similarity` is unchanged.
- **Story 2 (P1): `score` is documented as rank-only in hybrid.** The tool description, `docs/ipc-mcp-reference.md` and ADR-0812's text say a hybrid `score` is a fusion value (at most ≈ 0.0233) and not a relevance, and that `min_score` floors the vector leg's cosine only. *Acceptance:* the three places agree and name `similarity` as the relevance signal.
- **Story 3 (P2): `lcg-eval` records it.** The `retrieval` report shows, per hybrid config, the share of returned hits that are lexical-only (depends on the #847 work for per-question detail).

**Requirements**

- **FR-001:** `PassageResult` gains `legs` (`dense | lexical | both`), set by `search_passages_hybrid` and `search_passages_variants` in hybrid mode, derived from `FusedHit.raw`.
- **FR-002:** The field is omitted in `vector` mode, so a vector-mode response is byte-identical to today's.
- **FR-003:** Optionally, `lexical_score` carries the raw BM25 value when the lexical leg matched, omitted otherwise.
- **FR-004:** No default changes: `LCG_SEARCH_MIN_SCORE`, `LEXICAL_RRF_WEIGHT`, ranking and fusion are untouched.
- **FR-005:** The `knowledge_search_passages` `ToolSpec` description, the IPC reference, `service_protocol.py` docs and an ADR-0812 addendum state the semantics in Story 2.
- **FR-006:** Unit tests cover the three legs values and the vector-mode omission; the `crates/service/src/mcp/tools.rs` registry counts are unchanged (no new method).

**Success Criteria**

- **SC-001:** On the v2 probe, `legs` agrees with the E15 `similarity == null` count of lexical-only hits (115 off-topic, 140 answerable under the 0.5 floor).
- **SC-002:** Every existing parity test (`ipc_parity.rs` Tier 1a/1b/1c) passes unchanged.
- **SC-003:** The vector-mode response is byte-identical on a fixture query.

**Edge cases:** a passage in both lists with a different rank in each; variants mode (a passage retrieved by several queries: report the union of legs); the lexical index unavailable (hybrid falls back to vector-only: every hit is `dense`); `rerank` on (the field survives reranking).

**Assumptions:** additive JSON fields are non-breaking for the Python client (`graphiti_service.py`); the probe in #843 (E16) does not depend on this field but reads it more cleanly.

**Not in this issue:** an abstention threshold, gating lexical-only hits (that is E16), score normalisation."

## Background

In hybrid mode, `knowledge_search_passages` fuses a BM25 ranking over episode text with the vector ranking using weighted Reciprocal Rank Fusion. The returned `score` is therefore a rank-only fusion value (at most about 0.0233). It is not a relevance, and it is not comparable across queries. The response also does not say which retrieval leg found a passage.

Experiment E15 (recorded in the #863 research, section #848) found:

- The 0.5 `min_score` floor from #812 is safe in hybrid mode. It keeps 99.3% of answerable top-1 hits.
- The floor does not make hybrid abstain on off-topic queries. Hybrid returns hits for 25 of 34 off-topic queries, against 13 in vector mode. The floor applies only to the vector leg's cosine, and unfloored BM25 refills the list.
- Today a client can spot a lexical-only hit only by noticing `similarity == null`. Nothing documents that convention.

This issue makes the evidence explicit and the score semantics honest, without changing retrieval behaviour. Policy decisions such as an abstention threshold or gating lexical-only hits belong to the E16 probe (#843).

## User Scenarios & Testing *(mandatory)*

### User Story 1 - A client can tell what backed a hybrid hit (Priority: P1)

A caller of `knowledge_search_passages` with `mode: hybrid` sees, on each returned passage, which retrieval legs found it.

**Why this priority**: This is the core capability. Clients currently have to infer lexical-only hits from an undocumented null `similarity`, and they cannot distinguish "both legs agree" from "dense only".

**Independent Test**: Run a hybrid search over a fixture graph where one passage matches only by keyword, one only semantically, and one both ways. Check the `legs` value on each returned passage.

**Acceptance Scenarios**:

1. **Given** a passage retrieved only by the BM25 leg, **When** a hybrid search returns it, **Then** it reports `legs: "lexical"`.
2. **Given** a passage retrieved only by the vector leg, **When** a hybrid search returns it, **Then** it reports `legs: "dense"`.
3. **Given** a passage retrieved by both legs, possibly at different ranks in each, **When** a hybrid search returns it, **Then** it reports `legs: "both"`.
4. **Given** any hybrid result, **When** compared with the same result before this change, **Then** `similarity`, `score`, ordering and membership are unchanged.
5. **Given** a `vector`-mode search, **When** it returns, **Then** no passage carries `legs` (or `lexical_score`), and the response is byte-identical to today's.

---

### User Story 2 - `score` is documented as rank-only in hybrid (Priority: P1)

Anyone reading the tool description, the IPC/MCP reference or ADR-0812 learns that a hybrid `score` is a fusion value and not a relevance, that `min_score` floors only the vector leg's cosine, and that `similarity` is the relevance signal.

**Why this priority**: The misleading reading of `score` is what makes clients treat a rank value as confidence. This documentation is the P1 half of the fix and is needed regardless of the field.

**Independent Test**: Read the `knowledge_search_passages` tool description, the passage-search section of the IPC reference and the ADR-0812 addendum. Each states the same three facts: hybrid `score` is rank-only (at most about 0.0233) and not a relevance; `min_score` floors the vector leg's cosine only; `similarity` is the relevance signal.

**Acceptance Scenarios**:

1. **Given** the tool description, the IPC reference and ADR-0812, **When** a reader looks up hybrid `score`, **Then** all three describe it as a rank-only fusion value, not a relevance.
2. **Given** the same three places, **When** a reader looks up `min_score` in hybrid mode, **Then** all three say it floors the vector leg's cosine only and does not stop lexical hits from appearing.
3. **Given** the Python-side protocol documentation (`service_protocol.py`), **When** a reader looks up the passage result fields, **Then** `legs` (and `lexical_score` if shipped) are documented as additive, hybrid-only fields.

---

### User Story 3 - `lcg-eval` records lexical-only share (Priority: P2)

The `retrieval` report in `lcg-eval` shows, for each hybrid configuration, the share of returned hits that are lexical-only.

**Why this priority**: It makes the E15 finding repeatable and measurable over time. The per-question detail depends on the #847 work, and the core client capability does not need it.

**Independent Test**: Run the `retrieval` report for a hybrid configuration and confirm it prints a lexical-only share that matches the count of passages with `legs == "lexical"`.

**Acceptance Scenarios**:

1. **Given** a hybrid configuration in the `retrieval` report, **When** the report is produced, **Then** it includes the fraction of returned hits whose `legs` is `lexical`.
2. **Given** a vector-mode configuration, **When** the report is produced, **Then** the lexical-only share is not shown, or is shown as not applicable.

---

### Edge Cases

- A passage appears in both the BM25 list and the vector list at different ranks: it reports `both`, and its rank and score are unaffected.
- Variants mode (a passage retrieved by several queries): `legs` is the union of the legs over all queries that retrieved it. It is `both` if any leg combination covers both, and `lexical` or `dense` only if every retrieval used that single leg.
- The lexical index is unavailable and hybrid falls back to vector-only: every hit reports `dense`.
- `rerank` is on: the `legs` value survives reranking and attaches to the same passage afterward.
- `belief_aware` re-ranking demotes a passage: `legs` is unchanged by demotion.
- A hybrid request that returns no passages: the response shape is unchanged.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: `PassageResult` gains a `legs` field with values `dense`, `lexical` or `both`. `search_passages_hybrid` and `search_passages_variants` set it in hybrid mode, derived from `FusedHit.raw`.
- **FR-002**: `legs` is omitted from the response in `vector` mode, so a vector-mode response is byte-identical to today's.
- **FR-003**: Optionally, a `lexical_score` field carries the raw BM25 value when the lexical leg matched the passage, and is omitted otherwise. It is additive and hybrid-only. If it turns out not to be cheaply available, it may be dropped without affecting the rest of the scope.
- **FR-004**: No defaults or retrieval behaviour change. `LCG_SEARCH_MIN_SCORE`, `LEXICAL_RRF_WEIGHT`, ranking, fusion, filtering and result membership are untouched.
- **FR-005**: The `knowledge_search_passages` `ToolSpec` description, the IPC reference, the `service_protocol.py` docs and an ADR-0812 addendum all state that a hybrid `score` is a rank-only fusion value (at most about 0.0233) and not a relevance, that `min_score` floors the vector leg's cosine only, and that `similarity` is the relevance signal. They also document `legs`.
- **FR-006**: Unit tests cover the three `legs` values, the variants-mode union, the vector-only fallback (all `dense`) and the vector-mode omission. The `crates/service/src/mcp/tools.rs` registry counts are unchanged, since there is no new method.
- **FR-007**: The `lcg-eval` `retrieval` report shows, per hybrid configuration, the share of returned hits that are lexical-only (Story 3, P2).

### Key Entities *(if applicable)*

- **Passage result**: A returned passage with its `score`, `similarity` and the new `legs` (and optionally `lexical_score`) fields.
- **Leg**: One of the two retrievers whose rankings are fused in hybrid mode: the vector (dense) retriever and the BM25 (lexical) retriever.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: On the v2 probe, the number of passages with `legs == "lexical"` agrees with the E15 count of `similarity == null` lexical-only hits: 115 on off-topic queries and 140 on answerable queries under the 0.5 floor.
- **SC-002**: Every existing parity test (`ipc_parity.rs` Tier 1a/1b/1c) passes unchanged.
- **SC-003**: On a fixture query, the `vector`-mode response is byte-identical before and after the change.
- **SC-004**: The tool description, the IPC reference and the ADR-0812 addendum agree on the hybrid `score` and `min_score` semantics, and each names `similarity` as the relevance signal.
- **SC-005**: For hybrid requests, the order, scores, similarities and membership of passages are identical before and after the change, and the only difference is the added fields.

## Assumptions

- Additive JSON fields are non-breaking for the Python client (`graphiti_service.py`).
- The E16 probe (#843) does not depend on this field but reads it more cleanly.
- `lexical_score` (FR-003) is the only open scope decision. It is optional, and the Plan stage decides whether it ships depending on how cheaply the raw BM25 value can be surfaced.
- Story 3's per-question detail depends on the #847 work. The aggregate share is reported regardless.
- `similarity == null` continues to work as before and remains valid, and `legs` is the documented and preferred signal.

## Out of Scope *(optional)*

- An abstention threshold for hybrid mode, and gating or dropping lexical-only hits (that is E16 / #843).
- Score normalisation or any change to how `score` is computed.
- Changes to `LCG_SEARCH_MIN_SCORE`, `LEXICAL_RRF_WEIGHT`, fusion or ranking.
- Adding a `min_score`-like floor for the lexical leg.
- A new RPC/MCP method.

## Source References *(optional)*

- [docs/research/webbrain-retrieval-suggestions.md](https://github.com/liminisapp/liminis-context-graph/blob/main/docs/research/webbrain-retrieval-suggestions.md), section #848, measurement E15
- ADR-0812 (`search-min-score-env-default`)
- #812 (`min_score` floor), #843 (E16 probe), #847 (per-question eval detail), #848 (original suggestion), #863 (research issue)
