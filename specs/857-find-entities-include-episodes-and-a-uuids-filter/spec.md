# Feature Specification: find_entities include_episodes, and a uuids filter on get_episodes

**Feature Branch**: `fabrik/issue-857`
**Created**: 2026-11-21
**Status**: Specified
**Input**: User description: "Implements #831.

## Problem
`knowledge_find_entities` returns `EntityRow` with `episode_uuids` and `source_descriptions` always empty. `knowledge_get_entity_neighbors` and `knowledge_get_entities_by_source` fill both fields, so the same field means different things on different methods. The handler already works out the mentioning episodes for its `support` counts (`origin_reads::entity_support`, `crates/core/src/handlers.rs` ~1532–1555), but `SupportTally` keeps only per-origin counts and discards the UUIDs. Clients can't answer \"which sources mention this entity?\" without scanning episodes themselves.

## Change
- Add an opt-in `include_episodes: true` to `knowledge_find_entities`. When it's set, fill each hit's `episode_uuids` with its **live** mentioning episodes and `source_descriptions` with theirs, plus `source_files` (from #853 where present). Reuse the support computation, with no second query per hit. Cap the result per entity (for example 50) and report `episodes_truncated: true` when the cap is hit.
- Add a `uuids: [...]` filter to `knowledge_get_episodes`, so a client holding episode UUIDs can fetch just those. Keep it group-scoped as today.
- Document the field semantics in `docs/ipc-mcp-reference.md` and the MCP tool descriptions: when it's empty, and when it's filled.
- Add `entity_provenance` to the features list from #854.

## Acceptance
- `find_entities` without the flag is byte-identical to today.
- With the flag, a hit's `episode_uuids` equals the live episodes that `get_entity_neighbors` reports for the same entity, and superseded episodes are excluded.
- `get_episodes(uuids: [a, b])` returns exactly those episodes. An unknown UUID is simply absent, not an error.
- Tests, docs, CHANGELOG and `llms-full.txt` are updated."

## Background

`knowledge_find_entities` returns entity rows whose `episode_uuids` and `source_descriptions` fields are always empty, while `knowledge_get_entity_neighbors` and `knowledge_get_entities_by_source` populate both. The same field therefore means different things depending on the method, and a community bug report (#831) shows clients being misled by the empty values. Clients that want to know "which sources mention this entity?" currently have to scan episodes themselves.

The service already determines each entity's mentioning episodes when it computes the per-origin `support` counts on `find_entities` hits, but it keeps only the counts and discards the episode identities. Exposing those identities, on request, closes the gap without extra per-hit lookups. A client that then holds episode UUIDs also needs a way to fetch just those episodes, which `knowledge_get_episodes` cannot do today. Source-identity fields (`source_file`, stored separately from `chunk_id`) landed in #853, and the capabilities/feature list landed in #854 (both closed), so this work builds on them.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Ask which episodes mention each found entity (Priority: P1)

A client calls `knowledge_find_entities` with `include_episodes: true` and gets, for each hit, the live episodes that mention it, along with their source descriptions and source files.

**Why this priority**: This is the core bug-report fix: it makes the field meaningful on `find_entities` and answers the "which sources mention this entity?" question in one call.

**Independent Test**: Ingest several episodes mentioning an entity, supersede one of them, call `find_entities` with the flag, and compare the hit's `episode_uuids` against `get_entity_neighbors` for the same entity.

**Acceptance Scenarios**:

1. **Given** an entity mentioned by episodes A, B and C where C has been superseded, **When** `find_entities` is called with `include_episodes: true`, **Then** the hit's `episode_uuids` contains A and B and not C.
2. **Given** the same entity, **When** `get_entity_neighbors` is called for it, **Then** its live episode UUIDs equal the set returned by `find_entities` with the flag.
3. **Given** a hit with the flag set, **When** the response is read, **Then** `source_descriptions` holds the descriptions of those same episodes and `source_files` holds their source files wherever one was recorded.
4. **Given** an entity with more mentioning live episodes than the per-entity cap, **When** the flag is set, **Then** at most the cap's worth of episodes are returned and the hit carries `episodes_truncated: true`.
5. **Given** an entity below the cap, **When** the flag is set, **Then** `episodes_truncated` is false or absent, consistent with the documented contract.

---

### User Story 2 - Existing callers see no change (Priority: P1)

A client that does not pass `include_episodes` receives exactly the response it receives today.

**Why this priority**: `find_entities` is a widely used read path, and the IPC protocol is consumed by the Electron app. A silent change would break clients.

**Independent Test**: Compare the serialized `find_entities` response before and after the change for the same graph and query, with the flag absent and with it set to `false`.

**Acceptance Scenarios**:

1. **Given** a request without `include_episodes`, **When** `find_entities` runs, **Then** the response is byte-identical to the pre-change response, with `episode_uuids` and `source_descriptions` empty and no new keys.
2. **Given** a request with `include_episodes: false`, **When** it runs, **Then** the response is identical to the flag-absent response.

---

### User Story 3 - Fetch specific episodes by UUID (Priority: P2)

A client holding episode UUIDs, for example from Story 1, calls `knowledge_get_episodes` with `uuids: [...]` and receives exactly those episodes.

**Why this priority**: It completes the provenance round trip but is useful independently.

**Independent Test**: Create several episodes in two groups, request a subset by UUID, and verify the result set.

**Acceptance Scenarios**:

1. **Given** episodes a, b and c, **When** `get_episodes` is called with `uuids: [a, b]`, **Then** exactly episodes a and b are returned.
2. **Given** `uuids: [a, <unknown>]`, **When** called, **Then** only a is returned and no error is raised.
3. **Given** a UUID that exists in a different group than the one the request is scoped to, **When** called, **Then** that episode is not returned.
4. **Given** no `uuids` key, **When** called, **Then** behaviour is unchanged.

---

### User Story 4 - Discover and understand the capability (Priority: P3)

A client or integrator can detect that the service supports this feature and can read when the fields are empty versus filled.

**Why this priority**: Without discovery and documentation the feature is unusable by third-party clients, but it doesn't affect core behaviour.

**Independent Test**: Call `knowledge_capabilities` and inspect the features list; read the reference docs and MCP tool descriptions.

**Acceptance Scenarios**:

1. **Given** a running service, **When** `knowledge_capabilities` is called, **Then** the features list includes `entity_provenance`.
2. **Given** the reference docs and the MCP tool descriptions for `find_entities` and `get_episodes`, **When** read, **Then** they state when `episode_uuids`, `source_descriptions` and `source_files` are empty and when they are filled, and describe `include_episodes`, `episodes_truncated` and `uuids`.

---

### Edge Cases

- `include_episodes` is passed as a non-boolean value: treated the same way other malformed boolean options on this method are treated (error or ignored, matching existing convention).
- An entity has no live mentioning episodes (all superseded or none): the lists are empty and `episodes_truncated` is not set.
- An episode has no recorded `source_file` (older data): `source_descriptions` still carries its description, and `source_files` omits it rather than inserting blanks.
- Several mentioning episodes share a description or source file: the per-entity lists remain index-aligned with `episode_uuids`, or are de-duplicated; the chosen behaviour is documented (see Assumptions).
- `uuids` is an empty array: returns no episodes (not "all episodes").
- `uuids` contains duplicates or non-string items: duplicates return each episode once; malformed items follow the method's existing parameter-validation convention.
- `uuids` is combined with the existing `last_n` or other filters: the filters combine, with `uuids` narrowing the result.
- The cap is hit on one entity but not another in the same response: `episodes_truncated` is reported per hit.
- Deterministic ordering when truncating, so repeated calls return the same subset.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: `knowledge_find_entities` MUST accept an optional boolean `include_episodes`, defaulting to false.
- **FR-002**: When `include_episodes` is absent or false, the response MUST be byte-identical to today's, with no new keys and the provenance fields empty.
- **FR-003**: When `include_episodes` is true, each hit's `episode_uuids` MUST contain the UUIDs of that entity's live mentioning episodes, excluding superseded episodes.
- **FR-004**: When `include_episodes` is true, each hit's `episode_uuids` MUST equal the live episodes that `knowledge_get_entity_neighbors` reports for the same entity (subject to the cap in FR-007).
- **FR-005**: When `include_episodes` is true, each hit's `source_descriptions` MUST contain the source descriptions of the episodes in `episode_uuids`.
- **FR-006**: When `include_episodes` is true, each hit MUST also report `source_files` for those episodes where a source file is recorded (per #853), and omit those where none is.
- **FR-007**: The number of episodes reported per entity MUST be capped (default 50), and a hit MUST carry `episodes_truncated: true` when the cap removed at least one episode. Truncation MUST be deterministic.
- **FR-008**: Populating the episode fields MUST reuse the mentioning-episode computation already performed for the `support` counts, and MUST NOT add a per-hit query.
- **FR-009**: The `support` counts on each hit MUST be unchanged by this work, and MUST reflect the full set of mentioning episodes, not the capped list.
- **FR-010**: `knowledge_get_episodes` MUST accept an optional `uuids` array and, when present, return exactly the episodes whose UUIDs are listed (and which are in scope), and no others.
- **FR-011**: Unknown UUIDs in `uuids` MUST be silently absent from the result, not an error.
- **FR-012**: The `uuids` filter MUST remain group-scoped exactly as `get_episodes` is today; an episode in another group is never returned.
- **FR-013**: Without `uuids`, `knowledge_get_episodes` MUST behave exactly as today.
- **FR-014**: `entity_provenance` MUST be added to the features list returned by `knowledge_capabilities`.
- **FR-015**: The MCP tool descriptions and input schemas for `knowledge_find_entities` and `knowledge_get_episodes` MUST be updated to expose `include_episodes` and `uuids`, and their documented semantics MUST state when `episode_uuids`, `source_descriptions` and `source_files` are empty and when they are filled.
- **FR-016**: `docs/ipc-mcp-reference.md` MUST document the new parameters, the new response fields, the cap and `episodes_truncated`, and the field semantics across `find_entities`, `get_entity_neighbors` and `get_entities_by_source`.
- **FR-017**: The CHANGELOG and `docs/llms-full.txt` MUST be updated to describe the change.
- **FR-018**: The IPC protocol change MUST remain backward compatible with existing clients, including the Python-side service protocol, and the parity tests MUST be updated if the new fields are covered by them.

### Key Entities

- **Entity hit (EntityRow)**: A `find_entities` result; gains optional provenance (`episode_uuids`, `source_descriptions`, `source_files`, `episodes_truncated`) populated only on request.
- **Episode**: A source record that mentions entities; may be live or superseded, and carries a description and, where recorded, a source file.
- **Features list**: The capability advertisement from `knowledge_capabilities`, gaining `entity_provenance`.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: For every entity in a test corpus, `find_entities` with the flag returns the same live-episode UUID set as `get_entity_neighbors` (up to the cap), with zero superseded episodes included.
- **SC-002**: `find_entities` responses without the flag are byte-for-byte identical to pre-change responses across the existing test suite and a fixed-query comparison.
- **SC-003**: A client can go from a free-text entity query to the full set of mentioning episodes in two calls (`find_entities` with the flag, then `get_episodes` with `uuids`), with no episode scanning.
- **SC-004**: Enabling the flag adds no per-hit database queries compared with the existing `support` computation.
- **SC-005**: `get_episodes` with a `uuids` list returns exactly the requested, in-scope, existing episodes, with zero errors for unknown UUIDs.
- **SC-006**: `knowledge_capabilities` lists `entity_provenance`, and the docs, MCP tool descriptions, CHANGELOG and `llms-full.txt` all describe the new behaviour.

## Assumptions

- The per-entity cap is 50 and is a fixed service default, not a request parameter. The issue gives 50 as an example, and a caller-tunable cap is out of scope.
- Which episodes survive truncation is a deterministic, documented ordering (for example most recent first); the exact order is a planning decision, so long as it is stable.
- `episodes_truncated` is emitted only on hits when the flag is set, and is omitted or false when not truncated, so flag-off responses stay byte-identical.
- "Live" means not superseded, using the same definition `get_entity_neighbors` and the `support` computation already use.
- `source_files` is a new optional per-hit field alongside the existing two, populated from the source-file value introduced in #853.
- `source_descriptions` and `source_files` follow the convention `get_entity_neighbors` and `get_entities_by_source` already use for shared or missing values.
- `uuids` accepts any number of UUIDs; no separate maximum is imposed beyond existing request-size limits.
- The existing `support` computation is the single source for the episode identities; extending its internal tally to retain identities is acceptable as an implementation detail.

## Out of Scope *(optional)*

- Making the episode cap configurable per request.
- Populating episode fields on other methods or changing `get_entity_neighbors` / `get_entities_by_source` behaviour.
- Returning full episode bodies inside `find_entities` hits; `get_episodes` with `uuids` serves that need.
- Pagination of an entity's mentioning episodes beyond the cap.
- Any change to how supersession is determined.

## Source References *(optional)*

- Community bug report: #831
- Source-identity fields: #853 (closed)
- Capabilities and features list: #854 (closed)
