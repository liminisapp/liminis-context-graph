# Feature Specification: Read-path ergonomics — group-aware episodes, paging/projection/filters on bulk reads, clearer status `wal` block

**Feature Branch**: `fabrik/issue-667`
**Created**: 2026-10-01
**Status**: Specified
**Input**: User description: "Read-path ergonomics: get_episodes honours group_ids (omitted = all groups), paging/projection/filters on bulk reads, clearer status wal block (implements #648)"

## Background

Community report #648 describes an agent driving the service over MCP against a graph of about 600 entities, 1,100 edges and 360 episodes. Three read-path problems surfaced:

1. **`knowledge_get_episodes` ignores `group_ids` and defaults to a single group.** The handler reads only `group_id` (a single string) and falls back to the default group `"liminis"`. The MCP schema declares only `group_id`. The Liminis app, however, already calls it with `group_ids` (an array) — that parameter is silently dropped, and the app always receives `liminis`. It only works today because the app keeps all its data in `liminis`. A host that never uses `liminis` gets nothing back when it omits `group_id`. Every other read (`list_entities`, `find_entities`, …) treats an omitted `group_ids` as "all groups" (#413); episodes is the outlier.
2. **Bulk reads overflow MCP tool-result limits.** `knowledge_get_episodes(last_n=200)` returns ~363K characters, `last_n=400` ~668K, and `knowledge_list_entities(num_results=1000)` (default 500) ~485K. There is no way to page through results, to drop large fields (`content`, `summary`, `attributes`), or to narrow by name.
3. **`knowledge_status` shows a misleading top-level `wal` block in per-group WAL-root mode.** It reports `exists: false, generation_status: "not_applicable"` while `wal_groups` correctly shows every group hydrated. It reads like an error.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Episodes honour `group_ids`; omitted means all groups (Priority: P1)

A host calls `knowledge_get_episodes` with `group_ids: ["g2"]` and gets g2's episodes. A host that stores nothing in `liminis` calls it with no group at all and gets episodes from every group instead of an empty result.

**Why this priority**: It is a correctness bug — a documented-by-usage parameter is silently ignored — and it makes the call useless for any host not using `liminis`.

**Independent Test**: Seed episodes in two groups; call with `{group_ids:["g2"]}`, `{group_id:"g2"}`, `{}` and the app's shape `{last_n, group_ids}`, and assert the returned episodes' groups.

**Acceptance Scenarios**:

1. **Given** episodes in groups `g1` and `g2`, **When** `{group_ids: ["g2"]}` is sent, **Then** only g2's episodes are returned.
2. **Given** the same data, **When** `{group_id: "g2"}` is sent, **Then** only g2's episodes are returned (alias still works).
3. **Given** the same data, **When** neither parameter is sent, **Then** episodes from all groups are returned.
4. **Given** the same data, **When** the app's existing shape `{last_n: N, group_ids: ["g1","g2"]}` is sent, **Then** episodes from exactly those groups are returned, bounded by `last_n`.
5. **Given** both `group_id` and `group_ids` are supplied, **Then** the groups are unioned (see FR-003).

---

### User Story 2 - Page through bulk reads (Priority: P1)

An agent pages through `knowledge_get_episodes` and `knowledge_list_entities` in small pages, using a cursor returned in each response, and visits every item exactly once in a stable order.

**Why this priority**: Without paging, large graphs cannot be read at all over MCP.

**Independent Test**: Seed N items; page with a small page size until `next_cursor` is `null`; assert the concatenation equals the full ordered set with no duplicates or omissions.

**Acceptance Scenarios**:

1. **Given** more items than one page, **When** the caller passes the returned `next_cursor` back, **Then** the next page continues exactly where the previous ended.
2. **Given** the final page, **Then** `next_cursor` is `null`.
3. **Given** items are inserted between page requests, **Then** no item already returned is returned again, and no item that existed throughout paging is skipped.
4. **Given** the same request repeated against an unchanged graph, **Then** the order is identical.

---

### User Story 3 - Project only the fields needed (Priority: P2)

An agent asks for `fields: ["uuid","name"]` and receives only those keys per item, making the response a small fraction of the full payload.

**Why this priority**: Large fields (`content`, `summary`, `attributes`) dominate payload size; dropping them is the cheapest way to fit MCP limits.

**Independent Test**: Compare serialized size of a projected vs. full response on a seeded graph and assert a bound; assert only the requested keys appear.

**Acceptance Scenarios**:

1. **Given** `fields: ["uuid","name"]`, **Then** each returned item has exactly those keys.
2. **Given** an unknown field name, **Then** the call fails with an invalid-params error naming the field.
3. **Given** `fields` omitted, **Then** the full shape is returned, as today.
4. **Given** any request, **Then** embeddings are never returned.

---

### User Story 4 - Filter by name prefix (Priority: P2)

An agent lists entities whose name starts with a prefix (case-insensitive), or fetches every chunk episode of document X by episode-name prefix.

**Why this priority**: Narrowing at the source avoids pulling and discarding large result sets.

**Independent Test**: Seed entities/episodes with varied-case names; assert only matching ones return, regardless of case.

**Acceptance Scenarios**:

1. **Given** `list_entities {name_prefix: "al"}`, **Then** entities named `Alice`, `ALPHA`, `alpha` are returned and `Bob` is not.
2. **Given** `get_episodes {name_prefix: "doc-x"}`, **Then** only episodes whose name starts with that prefix are returned.
3. **Given** `name_prefix` combined with paging and `fields`, **Then** all three compose.

---

### User Story 5 - Unambiguous status in per-group WAL mode (Priority: P3)

An operator reading `knowledge_status` in per-group WAL-root mode no longer sees a top-level `wal` block that looks like a failure; single-root mode is unchanged.

**Why this priority**: Cosmetic/diagnostic, but currently misleads operators.

**Independent Test**: Call status in each mode; assert per-group mode has no misleading `exists: false` top-level block (omitted, or explicitly labelled `"mode": "per_group"` pointing to `wal_groups`) and single-root output is unchanged.

**Acceptance Scenarios**:

1. **Given** per-group mode, **Then** the status payload does not present a top-level `wal` with `exists: false` as if unlabelled.
2. **Given** single-root mode, **Then** the status payload is unchanged.

---

### Edge Cases

- `group_ids` is an explicit empty array on `get_episodes`: follows the helper used by `list_entities` (treated as "all groups").
- `group_id` and `group_ids` both given with overlapping or duplicate entries: union, deduplicated.
- `cursor` is malformed, tampered with, or from a different query shape: invalid-params error, not a panic or silent restart.
- Page size of 0, or beyond the existing maximum: handled consistently with today's `last_n` / `num_results` bounds.
- `fields` is empty, not an array, or contains non-strings: invalid-params error.
- `name_prefix` empty string: behaves as no filter.
- `name_prefix` containing regex/LIKE metacharacters or non-ASCII characters: matched literally.
- Items deleted between page requests: paging continues without error.
- With none of the new parameters, responses are byte-identical to today (apart from the group default in Story 1).

## Requirements *(mandatory)*

### Functional Requirements

**Group scoping (`knowledge_get_episodes`)**

- **FR-001**: `knowledge_get_episodes` MUST accept `group_ids` (array), parsed with the same helper as `list_entities`/`find_entities`.
- **FR-002**: `group_id` (single string) MUST remain accepted as a single-group alias.
- **FR-003**: When both are given, the effective set MUST be their deduplicated union.
- **FR-004**: When neither is given, episodes from all groups MUST be returned.
- **FR-005**: The MCP schema and description for `knowledge_get_episodes` MUST document `group_ids`, the `group_id` alias, and the omitted-means-all semantics; IPC parity tests (`crates/core/tests/ipc_parity.rs`) MUST be updated.
- **FR-006**: The CHANGELOG MUST carry an Upgrading note stating that callers omitting the group now receive every group, not just `liminis`.

**Paging**

- **FR-007**: `knowledge_get_episodes` and `knowledge_list_entities` MUST accept an optional paging parameter (an opaque `cursor`, or an `offset`; mechanism chosen in Research, preferring one stable under concurrent inserts) and MUST return `next_cursor` (`null` at the end) when paging is used.
- **FR-008**: Result ordering MUST be deterministic, so repeated paging over an unchanged graph visits every item exactly once in the same order.
- **FR-009**: An invalid cursor MUST produce an invalid-params error.

**Projection**

- **FR-010**: Both tools MUST accept optional `fields: [...]`; only the named keys are returned per item.
- **FR-011**: Unknown field names MUST produce an invalid-params error.
- **FR-012**: Embeddings MUST never be returned, with or without `fields`.
- **FR-013**: Omitted `fields` MUST return today's full shape.

**Filters**

- **FR-014**: `knowledge_list_entities` MUST accept `name_prefix` (case-insensitive).
- **FR-015**: `knowledge_get_episodes` MUST accept `name_prefix`.
- **FR-016**: `knowledge_get_episodes` SHOULD accept an `attributes` key/value match if cheap to implement; otherwise it is deferred and the PR notes that.

**Compatibility**

- **FR-017**: With none of the new parameters, responses MUST be byte-identical to today's, other than the Story 1 group default. A regression test MUST assert this.
- **FR-018**: All new parameters are additive and optional; no schema or storage change.

**Status**

- **FR-019**: In per-group WAL-root mode, `knowledge_status` MUST either omit the top-level `wal` block or label it explicitly (e.g. `"mode": "per_group"` with a pointer to `wal_groups`) so that it does not present `exists: false` as an error.
- **FR-020**: In single-root mode, the status payload MUST be unchanged.
- **FR-021**: Parity tests for the status payload MUST be updated and the change documented.

**Registry**

- **FR-022**: MCP tool registry counts and scope buckets (`crates/service/src/mcp/tools.rs` tests) MUST remain intact; input schemas for the two tools are extended, and no tools are added.

### Key Entities

- **Episode**: A stored source passage with name, content, group, and attributes; the unit read by `get_episodes`.
- **Entity**: A graph node with name, labels, summary, and attributes; the unit read by `list_entities`.
- **Page cursor**: An opaque token encoding position in a deterministic ordering.
- **Group**: A partition of the graph identified by `group_id`.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: `get_episodes {group_ids:["g2"]}` returns exactly g2's episodes; `{group_id:"g2"}` still does; omitting both returns all groups; the app's `{last_n, group_ids}` shape returns the requested groups.
- **SC-002**: Paging `list_entities` and `get_episodes` with a small page size visits every item exactly once, in stable order, and `next_cursor` is `null` at the end.
- **SC-003**: `fields: ["uuid","name"]` returns only those keys, and the response size is below a bound asserted in a test relative to the full payload.
- **SC-004**: `name_prefix` returns exactly the matching items (case-insensitive for entities).
- **SC-005**: With no new parameters, output matches today's byte-for-byte apart from the documented group default.
- **SC-006**: In per-group mode, `knowledge_status` shows no misleading top-level `wal` with `exists: false`; single-root output is unchanged.
- **SC-007**: An agent can read the ~360-episode / ~600-entity reference graph entirely within typical MCP tool-result limits by combining paging and projection.

## Assumptions

- Patch release, milestone 0.16.4. The group-default change is a deliberate patch-level behaviour change, documented in the CHANGELOG.
- Explicit `group_ids: []` on `get_episodes` behaves as it does on `list_entities` (all groups), per the helper named in the issue.
- Union (rather than error) is the default decision for supplying both `group_id` and `group_ids`; Research may revisit only if it finds a conflict with existing conventions.
- Cursor vs. offset, the exact default page size, and the field allow-lists are decided in Research, subject to FR-007/FR-008/FR-017.
- The `attributes` filter on episodes is optional and may be deferred.
- The choice between omitting and labelling the top-level `wal` block is decided in Research, subject to FR-019–FR-021.

## Out of Scope

- Any schema or storage change.
- Paging, projection or filters on other reads (e.g. `find_entities`, relationships, nodes/edges by group) — possible follow-up.
- Changing group-default semantics of any other method.
- New MCP tools or changes to scope buckets.

## Source References

- Issue #648 (community report); issue #413 (omitted-`group_ids` = all groups)
- `crates/core/src/handlers.rs` — `handle_get_episodes`, `handle_list_entities`, `extract_optional_group_ids`
- `crates/core/src/wal_group.rs` — `DEFAULT_GROUP_ID`
- `crates/service/src/mcp/tools.rs` — `knowledge_get_episodes`, `knowledge_list_entities` specs and registry-count tests
- `crates/core/tests/ipc_parity.rs`
- Liminis app: `context-graph-socket-client.ts` `getEpisodes`, `knowledge-reader-provider.ts`
