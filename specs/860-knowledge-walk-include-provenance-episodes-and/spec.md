# Feature Specification: `knowledge_walk` `include_provenance` — producing episodes and source files on each edge

**Feature Branch**: `fabrik/issue-860`
**Created**: 2026-11-21
**Status**: Specified
**Input**: User description: "Implements #839.

## Problem
`WalkEdge` (`crates/core/src/walk.rs` ~203–224) has `uuid`, `relation_type`, `fact`, endpoints, validity and score, but no episode or source provenance. The producing episodes are stored on `RelatesToEdge.episodes` (`types.rs` ~271–279). A client therefore can't turn a `knowledge_walk` result into citations.

## Change
- Add an opt-in `include_provenance: true` to `knowledge_walk`. When it's set, each `WalkEdge` carries `episodes` (the UUIDs of live episodes only) and `source_files` (from #853, falling back to `source_descriptions` for legacy rows). Cap each edge at 20 episodes and set a truncation flag when the cap is hit.
- Without the flag, the response is unchanged.
- Add `walk_provenance` to the features list from #854.

## Acceptance
- With the flag, a walk over an edge from two episodes returns both. After one is superseded, it returns only the live one.
- Without the flag, the response is byte-identical to today's.
- The docs and the tool schema are updated."

## Background

`knowledge_walk` (#820) lets an agent follow typed relations and returns the chosen edges with a ready-made `facts_text` for a reader. Each returned edge carries its fact, typed endpoints, validity window and score, but nothing about **where the fact came from**. An agent that wants to answer "who supplies the project X's team owns?" *and cite its sources* has to make a second round of calls to work out which episodes and files back each edge.

The information already exists in the graph. Every extracted edge records the episode that produced it. Since #705 episodes can be superseded (the graph stops believing them), so a naive list of producing episodes would point at passages the graph no longer stands behind. Since #853 each episode stores its `source_file` separately from the composed display value `"<source_file>:<chunk_id>"`, so a citation can name a file without splitting a string that may contain colons. Legacy episodes written before #853 have no `source_file` and only the composed display value. #854 added the `knowledge_capabilities` feature list, which is how a client discovers that this option exists.

This change adds an **opt-in** `include_provenance` flag to `knowledge_walk` so each returned edge can carry its live producing episodes and their source files, turning a walk result into citations. Without the flag the response must not change at all, so existing clients are unaffected.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Cite the sources behind a walked edge (Priority: P1)

An agent runs `knowledge_walk` with `include_provenance: true`. Each returned edge lists the UUIDs of the live episodes that produced it and the source files those episodes came from, so the agent can cite them or fetch the passages.

**Why this priority**: This is the whole feature: it is what makes a walk result citable.

**Independent Test**: Ingest two episodes (from two different files) that each produce the same relation between two entities, walk across that edge with the flag set, and check both episode UUIDs and both source files are returned on that edge.

**Acceptance Scenarios**:

1. **Given** an edge produced by two live episodes from `a.md` and `b.md`, **When** the caller walks across it with `include_provenance: true`, **Then** the edge's `episodes` contains both UUIDs and `source_files` contains `a.md` and `b.md`.
2. **Given** an edge whose producing episode has `source_file` set, **When** walked with the flag, **Then** `source_files` carries that `source_file` value exactly (a URL with a port or scheme is not split or altered).
3. **Given** a walk with several hops, a completion hop, and self-loop (literal-value) edges, **When** the flag is set, **Then** every returned edge in every hop carries the provenance fields.

---

### User Story 2 - Superseded episodes are not cited (Priority: P1)

When a producing episode has been superseded, it must not appear as provenance. The caller only sees episodes the graph still believes.

**Why this priority**: Citing a superseded passage would send users to text the graph has disowned; the acceptance criteria require it.

**Independent Test**: Take the two-episode edge from Story 1, supersede one episode, repeat the walk, and check only the live episode and its file remain.

**Acceptance Scenarios**:

1. **Given** an edge from episodes E1 and E2, **When** E1 is superseded and the walk runs with the flag, **Then** `episodes` contains only E2 and `source_files` contains only E2's file.
2. **Given** an edge whose recorded producing episodes are all superseded (or no longer exist), **When** walked with the flag, **Then** `episodes` and `source_files` are empty lists (the edge is still returned; provenance does not filter edges).
3. **Given** `mode: "history"` (which includes invalidated edges), **When** the flag is set, **Then** superseded episodes are still excluded from `episodes`.

---

### User Story 3 - Bounded output on heavily corroborated edges (Priority: P2)

An edge corroborated by very many episodes would bloat the response. Each edge reports at most 20 episodes and says when the list was cut.

**Why this priority**: Protects response size and latency; secondary to correctness of the main path.

**Independent Test**: Build an edge with more than 20 live producing episodes, walk with the flag, and check 20 are returned with the truncation flag set; repeat with exactly 20 and with fewer.

**Acceptance Scenarios**:

1. **Given** an edge with 25 live producing episodes, **When** walked with the flag, **Then** `episodes` has 20 entries and the truncation flag is `true`.
2. **Given** an edge with 20 or fewer live producing episodes, **When** walked with the flag, **Then** all are returned and the truncation flag is `false`.
3. **Given** the same walk run twice on unchanged data, **Then** the same 20 episodes are returned in the same order.

---

### User Story 4 - Legacy rows still yield a source (Priority: P2)

Edges and episodes written before #853 have no `source_file`. They should still produce a usable source reference.

**Why this priority**: Real workspaces contain legacy data; without a fallback those edges would be uncitable.

**Independent Test**: Walk across an edge whose producing episode has no `source_file` but a `source_description`, and an edge with no recorded producing episodes but a `source_descriptions` list; check both report a source.

**Acceptance Scenarios**:

1. **Given** a live producing episode with no `source_file`, **When** walked with the flag, **Then** `source_files` contains that episode's `source_description` as stored (unsplit).
2. **Given** an edge with no recorded producing episodes (a legacy row, a direct assertion, or a cross-group edge) that has a non-empty `source_descriptions`, **When** walked with the flag, **Then** `episodes` is empty and `source_files` carries those `source_descriptions`.

---

### User Story 5 - Unchanged responses and discoverability (Priority: P1)

Clients that do not ask for provenance see exactly what they see today. Clients that do can detect the option through the capabilities list and the tool schema.

**Why this priority**: Backward compatibility is an explicit acceptance criterion; the feature flag is how newer clients detect support on older servers.

**Independent Test**: Run an identical walk without the flag before and after the change and compare bytes; call `knowledge_capabilities` and inspect the MCP tool schema.

**Acceptance Scenarios**:

1. **Given** any walk request that omits `include_provenance` (or sets it to `false`), **When** executed, **Then** the response is byte-identical to the response the same request produces today: no new keys, no changed ordering.
2. **Given** the server, **When** a client calls `knowledge_capabilities`, **Then** `features` includes `walk_provenance` (and stays sorted).
3. **Given** the MCP `knowledge_walk` tool, **When** a client reads its input schema and description, **Then** `include_provenance` is described, including the live-only rule, the 20-episode cap and the truncation flag.

---

### Edge Cases

- `include_provenance` is not a boolean: a validation error, like other malformed `knowledge_walk` parameters. `false` behaves exactly as omitted.
- An edge that has recorded producing episodes but none are live: empty `episodes` and `source_files` (no fallback to the edge's stale `source_descriptions`, which would cite disowned sources).
- Two live episodes from the same file: `source_files` lists that file once.
- A producing episode UUID on the edge that no longer exists in the graph (for example removed by delete-by-source): treated as not live and omitted.
- Multi-group walks: each edge's episodes are looked up in that edge's own group.
- Edges that are returned as invalidated (history mode) keep their provenance rules unchanged: live episodes only.
- Exactly 20 live episodes is not truncation.
- The walk stays read-only: no WAL writes or graph mutation.
- `facts_text` is unchanged by the flag.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: `knowledge_walk` MUST accept an optional boolean `include_provenance`, default `false`, over both IPC and MCP.
- **FR-002**: When `include_provenance` is absent or `false`, the response MUST be byte-identical to the response produced before this change for the same request and data.
- **FR-003**: When `include_provenance` is `true`, every edge in every hop (including completion-hop edges and self-loop edges) MUST carry `episodes`, `source_files` and a boolean `episodes_truncated`, all always present (empty lists / `false` rather than omitted).
- **FR-004**: `episodes` MUST contain only the UUIDs of the edge's recorded producing episodes that are live (exist and are not superseded). Superseded or missing episodes MUST be excluded, in every walk mode including `history`.
- **FR-005**: `episodes` MUST contain at most 20 UUIDs per edge. `episodes_truncated` MUST be `true` when more than 20 live producing episodes exist and `false` otherwise. The selection and order MUST be deterministic for unchanged data.
- **FR-006**: `source_files` MUST list the distinct sources of the edge's live producing episodes: each episode's `source_file` when set, otherwise its stored `source_description` as-is (unsplit). Values are not altered.
- **FR-007**: When an edge has no recorded producing episodes at all, `source_files` MUST fall back to the edge's `source_descriptions` (and `episodes` is empty). When an edge has recorded producing episodes but none are live, `source_files` MUST be empty.
- **FR-008**: Provenance MUST NOT change which edges are returned, their ranking, scores, fallbacks, `facts_text`, or any other existing field.
- **FR-009**: A non-boolean `include_provenance` MUST be rejected with a validation error consistent with other `knowledge_walk` parameter errors.
- **FR-010**: `walk_provenance` MUST be added to the `features` list returned by `knowledge_capabilities`, keeping the list's ordering, format and uniqueness rules.
- **FR-011**: `knowledge_walk` MUST remain read-only (no WAL writes, no graph mutation) with the flag set.
- **FR-012**: The MCP tool schema and description for `knowledge_walk`, the IPC/MCP reference, `docs/llms-full.txt`, the README description of the walk where it lists response contents, and the CHANGELOG MUST document the option, the live-only rule, the cap and truncation flag, and the legacy fallback.

### Key Entities

- **Walk edge**: a returned edge in a `knowledge_walk` hop; gains `episodes`, `source_files` and `episodes_truncated` when provenance is requested.
- **Producing episode**: an episode recorded on an edge as having produced it; live while it exists and is not superseded.
- **Source file**: the `source_file` stored on an episode (#853); for legacy episodes, the composed `source_description` display value.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: With the flag, a walk across an edge produced by two live episodes returns both episode UUIDs; after one is superseded, a repeat walk returns only the live one.
- **SC-002**: A walk without the flag returns a response byte-identical to the pre-change response across the existing walk test scenarios (single, list, history, completion hop, self-loops).
- **SC-003**: Every provenance entry returned for an edge corresponds to a live episode: zero superseded or missing episodes are ever cited.
- **SC-004**: No edge ever reports more than 20 episodes, and `episodes_truncated` is `true` exactly when more than 20 live producing episodes exist.
- **SC-005**: A client can detect support from `knowledge_capabilities` (`walk_provenance`) without calling the walk.
- **SC-006**: A walk without the flag costs no extra provenance lookups compared with today.

## Assumptions

- #853 (`source_file` on episodes) and #854 (`knowledge_capabilities` feature list) are both merged; this change builds on them.
- The names of the new response fields are `episodes`, `source_files` and `episodes_truncated`. The first two are fixed by the request; the third is the "truncation flag" the request leaves unnamed.
- Provenance fields are always present (not omitted when empty) once the flag is set, so clients need no presence checks.
- "Cap is hit" means live episodes exceed 20; exactly 20 is not truncation, since nothing was omitted.
- Episode order is the order recorded on the edge, after filtering to live ones, then cut at 20.
- `source_files` entries are deduplicated per edge and carry the display value of legacy episodes unsplit, since the composed string cannot reliably be split (#853).
- The edge-level `source_descriptions` fallback applies only when the edge has no recorded producing episodes; staleness is judged by episodes, not by the stored descriptions.
- Adding an optional request flag and additive response fields does not bump the protocol version (consistent with ADR-0854).
- No ADR is required; the CHANGELOG records the addition under Added.

## Out of Scope *(optional)*

- Provenance on other tools (`knowledge_find_relationships`, `knowledge_get_entity_neighbors`, etc.) or on `facts_text`.
- Returning episode content, chunk IDs or passage text; callers fetch episodes by UUID with existing tools.
- A configurable cap or pagination of episodes beyond 20.
- Including superseded episodes (for example a "history of sources" view).
- Backfilling `source_file` on legacy rows (covered by #853).
- Changes to how edges are ranked or chosen.

## Source References *(optional)*

- #839 (originating request), #820 (`knowledge_walk`), #853 (source identity), #854 (`knowledge_capabilities`), #705 (episode supersession and edge `episodes`)
- `docs/ipc-mcp-reference.md` — "Typed relation walks" and "Source identity" sections
- ADR-0820, ADR-0854
