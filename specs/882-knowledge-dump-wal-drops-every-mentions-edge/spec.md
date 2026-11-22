# Feature Specification: knowledge_dump_wal must preserve MENTIONS edges created by native ingest

**Feature Branch**: `fabrik/issue-882`
**Created**: 2026-11-22
**Status**: Specified
**Input**: User description:

> ## Problem
>
> `knowledge_dump_wal` silently drops every `MENTIONS` edge created by native ingest, so a graph compacted with a dump and then rebuilt from it has **zero** `MENTIONS` edges. Episodes, entities and relationship edges all survive, so the loss is easy to miss.
>
> Found by the #828 research (ADR-0828, "Incidental findings", F1) and confirmed against `main`:
>
> - `Conn::insert_mentions_edge` (`crates/core/src/db.rs`, ~line 1239) creates `(ep)-[:MENTIONS {group_id}]->(en)` with **no `uuid`** property.
> - The dump's MENTIONS phase (`crates/core/src/dump.rs`, ~lines 653–663) skips any MENTIONS row whose `uuid` is empty, logging `[DUMP WARN] MENTIONS ep=… en=… has null uuid — skipping` under the comment "pre-migration edges".
> - In the #828 measurements, 792 of 792 document-mode MENTIONS rows had a null uuid; a rebuild from the dump had 0 MENTIONS against 792 from the WAL.
>
> Consequences after a dump→rebuild: entity→episode provenance is gone. That empties `episode_uuids` / `source_descriptions` on `get_entity_neighbors` and `get_entities_by_source`, `find_entities` `include_episodes` (#857), the mention-based fallback of `knowledge_walk` `include_provenance` (#860), entity `support`/`corroboration` counts, and anything else that reads `MENTIONS`.
>
> ## Acceptance criteria
>
> - A dump of a graph built by native ingest (`process_chunk`, `process_document`, `add_episode`) contains every `MENTIONS` edge, and a rebuild from that dump has the same `MENTIONS` count (and the same `(episode, entity)` pairs) as the source graph.
> - A regression test builds a small graph through the normal write paths, dumps, rebuilds in a fresh workspace and asserts `MENTIONS` parity (alongside the existing node/edge parity checks, e.g. `crates/core/tests/handlers_wal_dump.rs`).
> - Existing graphs whose MENTIONS edges have no uuid are also dumped correctly (the fix must not depend on a backfill having run).
>
> ## Approach notes (worker decides)
>
> Either key the dump's MENTIONS MERGE on the `(episode uuid, entity uuid, group_id)` endpoints instead of the edge uuid, or mint uuids at write time **and** handle existing uuid-less rows. The first is likely simpler and needs no migration; check that WAL replay of the emitted MERGE stays idempotent (ADR-0026, ADR-0028).
>
> ## Out of scope
>
> Any retention/pruning work (ADR-0828 contingency design). Downgrade compatibility is not a concern.

## Background

`knowledge_dump_wal` compacts a graph into a replayable WAL-style dump. Its MENTIONS phase skips any MENTIONS row without a uuid, on the assumption that such rows are legacy "pre-migration" edges. Native ingest, however, creates every `(episode)-[:MENTIONS]->(entity)` edge without a uuid. As a result, a dump of any natively ingested graph contains no MENTIONS edges at all, and a graph rebuilt from that dump has zero. In the #828 measurements, 792 of 792 document-mode MENTIONS rows were skipped.

Nodes and relationship edges survive the round trip, so the loss is silent. What breaks is everything that reads MENTIONS for entity→episode provenance:

- `episode_uuids` / `source_descriptions` on `get_entity_neighbors` and `get_entities_by_source`
- `find_entities` with `include_episodes` (#857)
- the mention-based fallback of `knowledge_walk` `include_provenance` (#860)
- entity `support` / `corroboration` counts
- any other MENTIONS reader

The defect was found by the #828 research (ADR-0828, "Incidental findings", F1) and confirmed against `main`.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - A dump of a natively ingested graph preserves every MENTIONS edge (Priority: P1)

An operator compacts a graph that was built by native ingest (`process_chunk`, `process_document`, `add_episode`) using `knowledge_dump_wal`, then rebuilds a workspace from the resulting dump. The rebuilt graph has exactly the same MENTIONS edges as the source graph, so entity→episode provenance still works.

**Why this priority**: This is the defect itself. Without it, compaction silently destroys provenance for all natively ingested content.

**Independent Test**: Build a small graph through the normal write paths, dump it, rebuild from the dump in a fresh workspace, and compare the MENTIONS edge set (count and `(episode, entity)` pairs) between source and rebuilt graphs.

**Acceptance Scenarios**:

1. **Given** a graph whose MENTIONS edges were created by native ingest (no edge uuid), **When** it is dumped and rebuilt in a fresh workspace, **Then** the rebuilt graph has the same MENTIONS count and the same set of `(episode, entity)` pairs as the source.
2. **Given** a rebuilt graph from such a dump, **When** `find_entities` is called with `include_episodes`, or `get_entity_neighbors` / `get_entities_by_source` is called, **Then** the provenance fields (`episode_uuids`, `source_descriptions`) are populated as in the source graph.
3. **Given** the dump of such a graph, **When** the dump is produced, **Then** no `[DUMP WARN] MENTIONS … has null uuid — skipping` is logged for native-ingest edges.

---

### User Story 2 - Existing uuid-less MENTIONS edges dump correctly without a backfill (Priority: P1)

An operator with an existing graph whose MENTIONS edges already lack a uuid on disk can dump and rebuild it correctly, without first running any migration or backfill.

**Why this priority**: Deployed graphs already contain uuid-less edges; a fix that only affected newly written edges would leave all current data lossy.

**Independent Test**: Take a graph containing MENTIONS edges with no uuid (as written by the current code), dump and rebuild it, and verify MENTIONS parity without any prior backfill step.

**Acceptance Scenarios**:

1. **Given** a graph with pre-existing uuid-less MENTIONS edges and no backfill having run, **When** it is dumped and rebuilt, **Then** all of those edges are present in the rebuilt graph.
2. **Given** a graph containing a mix of MENTIONS edges with a uuid and without, **When** it is dumped and rebuilt, **Then** all of them are present and none is duplicated.

---

### User Story 3 - Replaying the dump stays idempotent (Priority: P2)

Replaying a dump (or the MENTIONS portion of it) more than once, or over a partially populated graph, does not create duplicate MENTIONS edges.

**Why this priority**: Recovery and resume flows (ADR-0026, ADR-0028) rely on WAL replay being idempotent; a fix that broke this would trade one defect for another.

**Independent Test**: Replay the MENTIONS statements of a dump twice into the same graph and verify the MENTIONS count is unchanged after the second replay.

**Acceptance Scenarios**:

1. **Given** a graph that already contains a MENTIONS edge between an episode and an entity, **When** the dump's statement for that edge is replayed, **Then** no second edge is created.
2. **Given** a dump replayed in full twice into the same workspace, **When** the second replay completes, **Then** the MENTIONS count equals the count after the first replay.

---

### User Story 4 - Regression coverage (Priority: P2)

A regression test guards against the defect returning, exercising the normal write paths rather than hand-built fixtures.

**Why this priority**: The defect went unnoticed because existing parity checks did not cover MENTIONS.

**Independent Test**: The test itself, which must fail on the pre-fix code and pass after.

**Acceptance Scenarios**:

1. **Given** the existing dump/rebuild parity tests (e.g. `crates/core/tests/handlers_wal_dump.rs`), **When** the new test runs, **Then** it builds a small graph via the normal write paths, dumps it, rebuilds in a fresh workspace, and asserts MENTIONS parity alongside the existing node/edge checks.
2. **Given** the pre-fix behavior, **When** the new test runs, **Then** it fails (the rebuilt graph has zero MENTIONS).

---

### Edge Cases

- A MENTIONS edge whose episode or entity endpoint is missing from the graph (dangling): the dump must not fabricate endpoints; behavior must match how other edge kinds treat dangling endpoints.
- Multiple MENTIONS edges between the same `(episode, entity)` pair, in the same or different `group_id`: parity must be defined on `(episode, entity, group_id)`, and rebuild must not collapse distinct groups into one.
- A graph with zero MENTIONS edges: the dump and rebuild still succeed and the rebuilt graph has zero.
- A graph with a large number of MENTIONS edges (hundreds or more, per the 792-edge measurement): dump size and phase behavior remain consistent with other edge phases (batching, progress reporting).
- MENTIONS edges that do carry a uuid (written by other paths): they continue to round-trip, and the uuid is preserved where it exists.
- Replaying the dump onto a graph already containing the same MENTIONS edges (idempotency, see User Story 3).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: `knowledge_dump_wal` MUST emit every MENTIONS edge present in the graph, including edges that have no uuid.
- **FR-002**: A graph rebuilt from a dump MUST contain the same MENTIONS edges as the source graph, with identical count and identical `(episode, entity)` pairs (per `group_id`).
- **FR-003**: Dumping a graph with uuid-less MENTIONS edges MUST work without any prior backfill or migration having run.
- **FR-004**: Applying the dump's MENTIONS statements MUST be idempotent: replaying them over a graph that already holds the edge MUST NOT create duplicates (consistent with ADR-0026 and ADR-0028).
- **FR-005**: The dump MUST NOT log the "null uuid — skipping" warning for MENTIONS edges that are now dumped; a warning, if kept, applies only to rows that genuinely cannot be dumped.
- **FR-006**: MENTIONS edges that already carry a uuid MUST continue to be dumped and rebuilt correctly, with no duplicates when uuid-bearing and uuid-less edges coexist.
- **FR-007**: A regression test MUST build a small graph through the normal write paths (native ingest), dump it, rebuild it in a fresh workspace, and assert MENTIONS count and `(episode, entity)` pair parity, alongside the existing node/edge parity checks.
- **FR-008**: After a dump→rebuild, entity→episode provenance readers (`get_entity_neighbors`, `get_entities_by_source`, `find_entities` `include_episodes`, `knowledge_walk` `include_provenance` mention fallback, entity support/corroboration counts) MUST return the same results as on the source graph.
- **FR-009**: The fix MUST NOT change the dump's behavior for any non-MENTIONS node or edge phase.

### Key Entities *(if applicable)*

- **MENTIONS edge**: A directed edge from an Episode to an Entity, carrying `group_id` and (only for some edges) a `uuid`. Native ingest creates it without a uuid. It is the sole carrier of entity→episode provenance.
- **Dump (WAL dump)**: The compacted, replayable representation of a graph produced by `knowledge_dump_wal`, from which a workspace can be rebuilt.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: For a graph built by native ingest, the MENTIONS count in a graph rebuilt from its dump equals the source count (e.g. 792 of 792 in the #828 measurement scenario, versus 0 before).
- **SC-002**: The set of `(episode, entity, group_id)` MENTIONS tuples in the rebuilt graph is identical to the source graph's.
- **SC-003**: Provenance reads (`episode_uuids`, `source_descriptions`, `include_episodes`, support/corroboration counts) on the rebuilt graph match the source graph for the same queries.
- **SC-004**: A graph with existing uuid-less MENTIONS edges achieves SC-001 and SC-002 with no backfill step run.
- **SC-005**: Replaying a dump's MENTIONS statements a second time leaves the MENTIONS count unchanged.
- **SC-006**: The new regression test fails against the pre-fix code and passes after the fix, and the existing dump/rebuild tests still pass.

## Assumptions

- The identity of a MENTIONS edge for dump/rebuild purposes is its endpoints plus `group_id`: `(episode uuid, entity uuid, group_id)`. Two MENTIONS edges with the same triple are treated as the same edge.
- The choice of mechanism (key the dump's MERGE on endpoints versus minting uuids at write time and handling legacy rows) is left to the planning/implementation stages. Whichever is chosen must satisfy FR-003 (no backfill dependency) and FR-004 (idempotent replay).
- Downgrade compatibility (older binaries reading newer dumps) is not a concern, per the issue.
- "Normal write paths" means `process_chunk`, `process_document` and `add_episode`.
- MENTIONS edges with missing endpoints are handled the same way the dump handles dangling endpoints for other edge kinds.

## Out of Scope *(optional)*

- Retention or pruning work (the ADR-0828 contingency design).
- Downgrade or backward compatibility with older binaries.
- Changes to non-MENTIONS dump phases.
- A standalone backfill or migration tool for adding uuids to existing MENTIONS edges (the fix must work without one).

## Source References *(optional)*

- ADR-0828, "Incidental findings", F1 (origin of this issue)
- ADR-0026 (episode-cursor WAL resume) and ADR-0028 (WAL replay idempotency constraints)
- #857 (`find_entities` `include_episodes`) and #860 (`knowledge_walk` `include_provenance`): features that depend on MENTIONS surviving a rebuild
- `crates/core/tests/handlers_wal_dump.rs` (existing dump/rebuild parity tests)
