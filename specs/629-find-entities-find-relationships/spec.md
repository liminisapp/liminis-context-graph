# Feature Specification: Per-result relevance evidence and `min_similarity` floor for `find_entities` / `find_relationships`

**Feature Branch**: `fabrik/issue-629`
**Created**: 2026-09-29
**Status**: Specified
**Input**: User description: "find_entities / find_relationships: per-result relevance evidence and min_similarity floor (implements #613)"

## Background

`knowledge_find_entities` and `knowledge_find_relationships` are pure top-k searches with no relevance signal. Reported in #613: a gibberish query fills all 20 requested slots, and a correct name hit comes back followed by 19 unrelated entities. The caller cannot tell a strong match from filler.

Two properties of the current search cause this:

- Reciprocal-rank fusion (RRF) is rank-only. Every input list's raw score (BM25 score, cosine distance) is discarded before the fused order is returned, so nothing about match quality survives to the caller.
- Each vector candidate list returns up to `limit * 3` nearest neighbours however distant, so every query fills top-k.

Exposing only the fused RRF score would not fix this. RRF is rank-based, so the top hit of a gibberish query and the top hit of a perfect query get nearly identical fused scores. The caller needs the raw evidence (text match, per-path cosine similarity) and a way to set a similarity floor. `search_passages` already offers this via `min_score`; this feature brings the same semantics to the entity and relationship searches.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - See why each result was returned (Priority: P1)

A caller of `knowledge_find_entities` or `knowledge_find_relationships` wants to judge how relevant each result is, so it can tell a real match from top-k filler.

**Why this priority**: This is the core gap in #613. It is purely additive, and callers can build their own filtering from it (for example, keeping only `text_match: true` results).

**Independent Test**: Run each tool against a small graph with a known exact-name entity and a known fact. Confirm each result carries a `search` object with the documented fields. Confirm a field is `null` when that retrieval path did not return the item.

**Acceptance Scenarios**:

1. **Given** an entity whose name exactly matches the query, **When** `knowledge_find_entities` is called, **Then** that result's `search` object has `text_match: true`, a numeric `bm25_score`, and a numeric `rrf_score`.
2. **Given** an entity retrieved only by the name-vector path, **When** `knowledge_find_entities` is called, **Then** its `search` has `text_match: false`, `bm25_score: null`, a numeric `name_similarity`, and `summary_similarity: null`.
3. **Given** a fact retrieved by both BM25 and the fact-vector path, **When** `knowledge_find_relationships` is called, **Then** its `search` has `text_match: true`, numeric `bm25_score` and `fact_similarity`, and a numeric `rrf_score`.
4. **Given** any result, **When** it is returned, **Then** every similarity is cosine similarity (`1 - distance`).

---

### User Story 2 - Drop distant neighbours with `min_similarity` (Priority: P1)

A caller wants a gibberish or off-topic query to return nothing (or only genuinely similar results) instead of a full page of unrelated entities.

**Why this priority**: This is the behaviour change that fixes the reported symptom. Story 1 alone only lets callers post-filter.

**Independent Test**: Call `knowledge_find_entities` with a gibberish query and `min_similarity: 0.5`. Confirm it returns no results, or only results above the floor. Call it with an exact entity name and confirm the match is still returned.

**Acceptance Scenarios**:

1. **Given** a gibberish query with no BM25 hit, **When** `min_similarity` is set (for example 0.5), **Then** the result list is empty or contains only items whose vector similarity meets the floor.
2. **Given** an exact-name query, **When** `min_similarity` is set, **Then** the named entity is still returned. It is retrieved by BM25 and so stays eligible.
3. **Given** `min_similarity` is unset, **When** either tool is called, **Then** results and their order are identical to today's, apart from the added `search` object.
4. **Given** `min_similarity` is set, **When** the search runs, **Then** vector candidates below the floor never enter RRF fusion. They do not affect the ranking of the remaining items, and their similarity is not reported for an item they did not qualify for.

---

### User Story 3 - Evidence and floor work with the `kind` filter (Priority: P2)

A caller uses `knowledge_find_entities` with the `kind` filter from #615 and expects the same evidence and floor behaviour.

**Why this priority**: The `kind`-filtered path is a separate code path, so it needs explicit coverage. It is not a separate feature.

**Independent Test**: Call `knowledge_find_entities` with `kind` set and again with `min_similarity` set. Confirm results carry `search` evidence and the floor is honoured.

**Acceptance Scenarios**:

1. **Given** a `kind` filter, **When** `knowledge_find_entities` is called, **Then** every returned entity is of that kind and carries `search` evidence.
2. **Given** a `kind` filter and `min_similarity`, **When** the search runs, **Then** vector candidates below the floor are excluded and BM25 hits of that kind remain eligible.

---

### User Story 4 - Discover the new fields from the tool descriptions (Priority: P2)

An MCP client or agent reads the tool descriptions and input schemas and learns about the evidence fields and the `min_similarity` parameter without reading source.

**Why this priority**: These tools are consumed by agents, so the description is the primary interface documentation.

**Independent Test**: Inspect the two `ToolSpec` entries. Confirm the descriptions document the `search` object and its fields, and that the input schema for both tools lists `min_similarity` (number, 0–1).

**Acceptance Scenarios**:

1. **Given** the MCP tool registry, **When** the tool list is read, **Then** both tools' input schemas declare an optional `min_similarity` number bounded 0–1, and both descriptions document the `search` fields.

---

### Edge Cases

- **Unset `min_similarity`**: must behave exactly as before. The regression comparison is against the current fused order.
- **`min_similarity` above all candidates with no BM25 hit**: returns an empty list, not an error.
- **`min_similarity` outside 0–1**: an out-of-range value is rejected with an invalid-params error, consistent with how `search_passages` treats `min_score`. If `search_passages` does not currently validate, the two should still behave the same way.
- **Item found by several vector paths (entities)**: `name_similarity` and `summary_similarity` are reported independently. Either may be `null`.
- **Floor applied per path**: an entity can pass the floor on the name path and fail on the summary path. Only the qualifying path contributes to fusion and evidence.
- **Empty `group_ids` filter and other existing filters**: unchanged by this feature.
- **Empty query or zero candidates**: an empty result list, as today.
- **A BM25-retrieved item with weak vector similarity**: stays eligible. Its below-floor vector value is not fed to fusion, so its `search` shows `null` for that path.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The fusion step used by entity and edge search MUST carry per-item evidence alongside the fused order: the fused RRF score, plus each input list's raw value when that list retrieved the item.
- **FR-002**: Each result of `knowledge_find_entities` MUST include an additive `search` object with `rrf_score`, `text_match` (bool), `bm25_score`, `name_similarity` and `summary_similarity`.
- **FR-003**: Each result of `knowledge_find_relationships` MUST include an additive `search` object with `rrf_score`, `text_match` (bool), `bm25_score` and `fact_similarity`.
- **FR-004**: Every similarity field MUST be cosine similarity (`1 - distance`). A similarity or `bm25_score` field MUST be `null` when that retrieval path did not retrieve the item. `text_match` is `false` in that case.
- **FR-005**: Both tools MUST accept an optional `min_similarity` (number, 0–1) with the same semantics as `search_passages`' `min_score`.
- **FR-006**: When `min_similarity` is set, it MUST be applied to the vector candidate lists before fusion, so below-floor neighbours do not enter the ranking.
- **FR-007**: When `min_similarity` is set, items retrieved by BM25 MUST remain eligible regardless of their vector similarity.
- **FR-008**: When `min_similarity` is unset, the set of results and their ordering MUST be identical to current behaviour. The only difference is the added `search` object. A regression test MUST compare against the current fused order.
- **FR-009**: The `kind` filter (#615) MUST keep working, and FR-002, FR-005, FR-006 and FR-007 MUST hold on the `kind`-filtered entity search path.
- **FR-010**: The extraction-time dedup path (`hybrid_dedup_similar_entity`) MUST NOT change behaviour, and MUST NOT be affected by the changes to fusion or search.
- **FR-011**: The tool descriptions and JSON input schemas in the MCP registry MUST document the `search` fields and `min_similarity`, and the registry's own count-based tests MUST stay correct. This is a parameter change, not a new tool.
- **FR-012**: The change MUST be additive on the IPC surface, with no schema change. The IPC parity tests (`crates/core/tests/ipc_parity.rs`) and the Python-side `service_protocol.py` MUST stay aligned with the response shape.

### Key Entities

- **Search evidence (`search` object)**: Per-result record of why the item was retrieved: fused `rrf_score`, `text_match`, `bm25_score`, and per-path cosine similarities (`name_similarity`, `summary_similarity` for entities; `fact_similarity` for edges).
- **Similarity floor (`min_similarity`)**: Optional per-request threshold on vector candidates, applied before fusion.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: 100% of results from both tools include a `search` object with the documented fields, each `null` when the path did not retrieve the item.
- **SC-002**: With `min_similarity` set (for example 0.5), a gibberish query with no BM25 hit returns zero results or only results at or above the floor, while an exact-name query still returns its match.
- **SC-003**: With `min_similarity` unset, a regression test shows results and order identical to the pre-change fused order for both tools.
- **SC-004**: A `kind`-filtered `knowledge_find_entities` call returns `search` evidence and honours `min_similarity`.
- **SC-005**: The MCP tool descriptions and schemas for both tools document the new fields and parameter.
- **SC-006**: Existing dedup behaviour is unchanged, shown by the existing dedup tests still passing unmodified.

## Assumptions

- This is patch-sized: an additive API with no storage schema change.
- Raw BM25 scores and vector distances are already available from the candidate queries. Only the fusion step discards them.
- `min_similarity` follows the `search_passages` `min_score` convention: similarity is `1 - distance`, and a candidate is kept when its similarity is at or above the floor.
- Out-of-range `min_similarity` handling follows whatever validation `search_passages` applies to `min_score`, so the two tools stay consistent.
- The floor applies independently to each vector list (name, summary, fact). An item passing on one path but not another is reported only for the path it passed.
- The `search` object is included on every result without a request flag, because it is additive.

## Out of Scope

- The `text_match_only` flag from #613. With `text_match` in the evidence, a caller can filter on it. Revisit if asked.
- Changing `hybrid_dedup_similar_entity` or its thresholds.
- Any change to the default (unset `min_similarity`) ranking, or to the RRF constant or fusion algorithm.
- Storage schema changes.
- A BM25 score floor.

## Source References

- #613 (community report), #615 (`kind` filter)
- `crates/core/src/search.rs`: `rrf_fuse`, `hybrid_entity_search_kind`, `hybrid_edge_search`, `search_passages`
- `crates/service/src/mcp/tools.rs`: `knowledge_find_entities`, `knowledge_find_relationships` tool specs
- `crates/core/tests/ipc_parity.rs`; Python `service_protocol.py`
