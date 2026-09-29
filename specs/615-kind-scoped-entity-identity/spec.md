# Feature Specification: Kind-scoped entity identity

**Feature Branch**: `fabrik/issue-615`
**Created**: 2026-09-29
**Status**: Specified
**Input**: User description: "Kind-scoped entity identity: add Entity.kind and key identity on (group_id, kind, name)" — implementation issue for community report #614.

## Background

Entity identity is `compute_lookup_key(group_id, name)` → `group_id ␟ lower(trim(name))` (`crates/core/src/db.rs:3553`), served by the ART index on `Entity.lookup_key` (#221). Kind plays no part. All entities live in the single `Entity` table, and `labels STRING[]` is a descriptive property only.

So two different kinds of thing with the same name in one group are, by definition, the same entity — and `knowledge_assert_entity`'s full-replace upsert makes the collision destructive. Reported from a real deployment (#614): asserting `Topic "adr"` resolved to the existing `KnowledgeChannel "adr"` and replaced its labels, summary and attributes wholesale, and `Topic "pset"` / `Team "pset"` collapsed into one node relabelled by whichever assert ran last.

Since #221 retired the in-process `NameIndex`, `lookup_key` is the **single** point of truth for identity, which makes this change tractable: altering what goes into the key alters identity everywhere at once.

This issue covers the `kind` column and identity keying. Extraction assigning kinds from the ontology is a separate phase (see *Out of Scope*).

### Settled design decisions (triage on #614)

- **D1 — `kind` column in the single `Entity` table.** Add `Entity.kind STRING` and change the key to `lookup_key = group_id ␟ kind ␟ lower(trim(name))`. `labels` stays the multi-valued descriptive set, and `kind` is always one of them. Rejected: a node table per kind (dynamic DDL per type, per-table vector/FTS indexes, fan-out for "search all entities", loses multi-label entities, large WAL/replay break) and first-label-as-identity (implicit, and fragile because labels are replaced wholesale on assert).
- **D2 — Omitted kind: reads broad, writes scoped.** #614 as filed says both that an omitted `kind` means the default kind `Entity` and that name-only resolution across several kinds returns an ambiguity error; those conflict. Resolved by the rule this repo already uses for `group_ids` (#413):
  - **Writes** with no `kind` (`assert_entity`; a relationship endpoint being *created*) act on the default kind `Entity` only, so a name-only write can never reach into another kind's node.
  - **Reads and resolution** with no `kind` (`find_entities`, `list_entities`, resolving an existing relationship endpoint, cross-group pointer resolution) span **all** kinds, and return an explicit ambiguity error listing the candidates when more than one matches. Never a silent pick.
  - Existing callers keep working unchanged because all existing data is kind `Entity`.
- **D3 — `lookup_key` is derived: strip on write, recompute on replay.** `lookup_key` is currently written to the WAL as a bound param (`lookup_key: $lookup_key`, `db.rs:601`). It is a pure function of `(group_id, kind, name)`, all of which the record carries. Following the rule established by #526 / ADR-0526 for derived values: strip at the `WalWriter::log_mutation` choke point, recompute on replay, ignore any copy found in an older WAL. An old record's literal `lookup_key` is never trusted; it is recomputed from the record's `kind`, defaulting to `Entity` when absent, so old corpora replay to exactly today's identities with no version marker to check.
- **D4 — Cross-group pointers carry an optional kind.** A cross-group pointer (ADR-0369) asserts `(source_group_id, endpoint_name)` with no kind, and `cross_group::resolve_endpoint` (`cross_group.rs:80`) already returns `Ambiguous` when more than one active entity matches the name. Once same-named entities of different kinds can coexist, every existing layer pointer to that name would flip to `ambiguous` on the next rebind, and an ambiguous pointer has no hop — the layer edge silently drops out of reads — triggered by a write into a *different* group from the one holding the pointer. That outcome is correct under D2 but must be handled rather than discovered: pointers gain an optional `endpoint_kind` stored alongside `endpoint_name`, and `knowledge_add_cross_group_edge` gains `source_kind`/`target_kind`. A pointer with no kind keeps resolving across all kinds.
- **D5 — Merges never cross kinds.** `knowledge_merge_entities` refuses to merge entities of different kinds. `resolve_endpoint` follows `Merged` chains via `follow_merged_into_chain`; a cross-kind merge would let a pointer asserting kind `Topic` land on a `KnowledgeChannel`, making pointer kind assertions untrustworthy.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Same-named entities of different kinds coexist (Priority: P1)

A caller asserts a `KnowledgeChannel` named "adr" with a summary and attributes, then asserts a `Topic` named "adr" in the same group. Today the second write overwrites the first; after this change they are two distinct entities.

**Why this priority**: This is the destructive data-loss bug reported in #614; everything else supports it.

**Independent Test**: Assert both entities in one group and read them back; verify two nodes exist and the channel's labels, summary and attributes are unchanged.

**Acceptance Scenarios**:

1. **Given** an empty group, **When** `assert_entity {name:"adr", kind:"KnowledgeChannel", summary, attributes}` then `assert_entity {name:"adr", kind:"Topic"}` run, **Then** two entities exist and the channel keeps its labels, summary and attributes.
2. **Given** both entities exist, **When** `assert_relationship {source_name:"adr", source_kind:"KnowledgeChannel", target_name:"adr", target_kind:"Topic", predicate:"COVERS"}` runs, **Then** the edge links the two distinct nodes and is not a self-loop.

---

### User Story 2 - Name-only reads are explicit about ambiguity; name-only writes stay scoped (Priority: P1)

A caller that does not pass `kind` must never silently hit the wrong node. Reads and resolution see all kinds and report ambiguity; writes touch only the default kind `Entity`.

**Why this priority**: Without this, adding kinds merely moves the silent-overwrite hazard.

**Independent Test**: With `KnowledgeChannel "adr"` and `Topic "adr"` present, run a name-only read, a name-only relationship resolution, and a name-only `assert_entity`.

**Acceptance Scenarios**:

1. **Given** both kinds of "adr" exist, **When** a name-only `find_entities`/`list_entities` read or relationship-endpoint resolution of "adr" runs, **Then** an ambiguity error listing each candidate's kind and uuid is returned; no candidate is chosen.
2. **Given** both kinds of "adr" exist, **When** `assert_entity {name:"adr"}` runs, **Then** the kind-`Entity` node "adr" is created or updated and the `KnowledgeChannel` and `Topic` nodes are untouched.
3. **Given** only one entity named "adr" exists (any kind), **When** a name-only read runs, **Then** it returns that entity without error.
4. **Given** an exact lookup by `(group_id, name, kind)`, **When** it runs, **Then** it returns only the matching-kind entity and never reports ambiguity.

---

### User Story 3 - Existing callers and existing WAL corpora are unaffected (Priority: P1)

Callers that never pass `kind`, and every existing WAL corpus (including the #217 real-corpus capture), behave and replay exactly as before, with no storage-version bump.

**Why this priority**: The service is consumed by the liminis Electron app over IPC, and existing corpora must remain replayable.

**Independent Test**: Replay the #217 corpus and compare entity, relationship and episode counts with the pre-change baseline; run existing parity tests unchanged.

**Acceptance Scenarios**:

1. **Given** a WAL written before this change (records carry a literal `lookup_key` and no `kind`), **When** it is replayed, **Then** every entity is kind `Entity`, its `lookup_key` is recomputed (the old literal ignored), and counts match today's.
2. **Given** new mutations are logged, **When** the WAL record is inspected, **Then** it contains no `lookup_key`.
3. **Given** extraction runs, **When** it creates or merges entities, **Then** they carry kind `Entity` and merge by name exactly as today.

---

### User Story 4 - Cross-group pointers can pin a kind (Priority: P2)

A cross-group pointer created with no kind keeps binding as today while only one kind of that name exists, becomes `ambiguous` once a second kind appears, and binds unambiguously if created with an `endpoint_kind`.

**Why this priority**: Prevents layer edges from silently dropping out of reads once kinds coexist; lower than P1 because it only manifests after same-named kinds exist across groups.

**Independent Test**: Create pointers with and without kind to "adr"; add a second kind; rebind and check binding states.

**Acceptance Scenarios**:

1. **Given** one entity "adr" and a kind-less pointer to it, **When** the pointer is resolved, **Then** it binds as today.
2. **Given** a second-kind "adr" is then created, **When** the pointer is rebound, **Then** it reports `ambiguous`.
3. **Given** a pointer created via `knowledge_add_cross_group_edge` with `target_kind:"Topic"`, **When** both kinds exist, **Then** it binds to the `Topic` entity.

---

### User Story 5 - Merges refuse to cross kinds (Priority: P2)

**Why this priority**: Protects the trustworthiness of pointer kind assertions (D5).

**Independent Test**: Attempt to merge a `Topic` and a `KnowledgeChannel`.

**Acceptance Scenarios**:

1. **Given** entities of kinds `Topic` and `KnowledgeChannel`, **When** `knowledge_merge_entities` is called on them, **Then** it is refused with an error naming both kinds and neither entity is modified.
2. **Given** two entities of the same kind, **When** they are merged, **Then** behaviour is unchanged.

---

### Edge Cases

- Existing databases with no `kind` column/values: every existing entity is treated as kind `Entity`; the migration must not alter their identity or `lookup_key` semantics beyond the recomputed key.
- Name-only write when a kind-`Entity` node does not exist but other kinds do: creates a new kind-`Entity` node; it does not adopt or modify the other kinds.
- Name-only relationship endpoint that is being *created* (does not resolve to any entity): created as kind `Entity`. If it matches several kinds, it is an ambiguity error rather than a create.
- Name matches exactly one entity of a non-`Entity` kind on a name-only read/resolution: resolves to that entity (reads are broad).
- WAL record with a `kind` but an outdated/absent `lookup_key`, or with a `lookup_key` inconsistent with its fields: the stored value is ignored and recomputed.
- `kind` values containing the key separator (U+001F) or empty/whitespace-only `kind`: rejected with a validation error so keys stay unambiguous.
- Cross-group pointer whose recorded `endpoint_kind` no longer exists after a rebind: reports unbound, as for a missing name today.
- Merged-entity chains: `follow_merged_into_chain` never crosses kinds (guaranteed by FR-009), so a pointer's kind assertion holds after following the chain.
- A self-loop check on `assert_relationship` must compare entity identity, not names, so `adr` (channel) → `adr` (topic) is not a self-loop.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: `Entity` MUST gain a `kind STRING` column; every entity MUST have exactly one kind, and `kind` MUST be present in `labels`.
- **FR-002**: `lookup_key` MUST be computed as `group_id ␟ kind ␟ lower(trim(name))`, and the ART index MUST enforce uniqueness on it, so two entities of different kinds may share a name in one group.
- **FR-003**: `knowledge_assert_entity` MUST accept an optional `kind`; when omitted it MUST act on the default kind `Entity` only (D2).
- **FR-004**: `knowledge_assert_relationship` MUST accept optional `source_kind` / `target_kind` for endpoint resolution, following D2's read rule for resolving existing endpoints (and D2's write rule when an endpoint is created).
- **FR-005**: An exact entity lookup by `(group_id, name, kind)` MUST be available, either as an option on `knowledge_find_entities` / `knowledge_list_entities` or as a dedicated call — the Plan stage decides which.
- **FR-006**: Name-only resolution that matches more than one kind MUST return an explicit ambiguity error listing the candidates' kinds and uuids. It MUST NOT pick one or write to one.
- **FR-007**: `lookup_key` MUST be stripped from WAL records at write time and recomputed on replay; a `lookup_key` found in an older WAL MUST be ignored (D3). A record with no `kind` MUST replay as kind `Entity`.
- **FR-008**: Cross-group pointers MUST support an optional `endpoint_kind`, and `knowledge_add_cross_group_edge` MUST accept `source_kind` / `target_kind` (D4). A pointer with no kind MUST resolve across all kinds.
- **FR-009**: `knowledge_merge_entities` MUST refuse to merge entities of different kinds (D5).
- **FR-010**: Extraction MUST continue to assign the default kind `Entity` and merge by name exactly as today. Ontology-driven kind assignment is out of scope.
- **FR-011**: Every `knowledge_*` schema that gains a kind parameter MUST document the omitted-kind behaviour precisely, per operation — the reads-broad / writes-scoped distinction must be explicit. This includes the hand-maintained MCP tool registry (`crates/service/src/mcp/tools.rs`) and the Python-side protocol, keeping the IPC parity tests aligned.
- **FR-012**: A `kind` value MUST be non-empty after trimming and MUST NOT contain the key separator (U+001F); violations MUST be rejected with a validation error.
- **FR-013**: The schema MUST remain in parity with graphiti's Kuzu driver conventions for existing columns; the new `kind` column is an additive extension and MUST NOT break graphiti-shaped reads/writes.

### Key Entities

- **Entity**: A node in the single `Entity` table, now identified by `(group_id, kind, name)`; `kind` is one of its `labels`. Default kind is `Entity`.
- **Kind**: A single string classifying an entity (e.g. `Topic`, `KnowledgeChannel`, `Team`); part of identity and immutable for a given entity.
- **Cross-group pointer**: A layer-edge endpoint asserting `(source_group_id, endpoint_name[, endpoint_kind])` with a binding state (bound / unbound / ambiguous).
- **WAL record**: Mutation log entry; carries `kind` but never `lookup_key`.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: In one group, `assert_entity {name:"adr", kind:"KnowledgeChannel", summary:"…", attributes:{…}}` followed by `assert_entity {name:"adr", kind:"Topic"}` yields **two** entities, and the channel keeps its labels, summary and attributes.
- **SC-002**: `assert_relationship {source_name:"adr", source_kind:"KnowledgeChannel", target_name:"adr", target_kind:"Topic", predicate:"COVERS"}` links the two distinct nodes, with no self-loop.
- **SC-003**: With both present, a name-only **read** or relationship resolution of `"adr"` returns an ambiguity error naming both kinds.
- **SC-004**: With both present, a name-only **write** (`assert_entity {name:"adr"}`) creates or updates the kind-`Entity` node and leaves both others untouched.
- **SC-005**: Existing callers that never pass `kind`, and existing WAL corpora — including the #217 real-corpus capture — behave and replay exactly as before: identical entity, relationship and episode counts, with no version bump.
- **SC-006**: A cross-group pointer to `"adr"` created with no kind binds as today while only one kind exists, reports `ambiguous` once a second kind appears, and binds unambiguously when created with an `endpoint_kind`.
- **SC-007**: Merging two entities of different kinds is refused with an error naming both kinds.

## Assumptions

- `kind` is compared verbatim after trimming (case-sensitive, matching ontology type names), unlike `name`, which is lowercased for the key. The default kind is exactly `Entity`.
- The existing invariant that `labels` begins with `Entity` (`enforce_entity_first`) is retained; a non-default kind is added to `labels` after `Entity`, so `kind` is always contained in `labels`.
- An entity's kind is immutable; "changing" kind means asserting a new entity of the other kind.
- The `lookup_key` ART index unique-key semantic (#221) is retained; only the key composition changes.
- Existing on-disk databases are migrated by the same mechanism used for prior additive schema changes (backfilling kind `Entity`); the specific mechanism is a Plan-stage decision.
- No WAL/storage version bump is introduced; old and new binaries' WAL compatibility follows D3.

## Out of Scope

- **Ontology-driven kind assignment during extraction** — marking ontology types identity-bearing so extraction assigns them as `kind` (#614 point 3). Tracked separately. Without it, extraction keeps the default kind, preserving today's merge-by-name behaviour; with it, extracted `Person "Aurora"` and `Technology "Aurora"` would stay distinct.
- **`knowledge_assert_entity` merge / ensure modes** and **`update_entity_core` not re-embedding a changed `summary`** — both listed in #614 as separate, smaller items.

## Source References

- #614 — the community report this implements.
- #413 — the reads-broad / writes-scoped rule D2 applies to kind.
- #526 / ADR-0526 (`docs/adr/0526-vectors-are-a-local-cache.md`) — derived values stripped on write and recomputed on replay (D3).
- #221 / ADR-0221 (`docs/adr/0221-secondary-art-index-for-entity-name-lookup.md`) — the `lookup_key` ART index.
- ADR-0369 (`docs/adr/0369-resolvable-cross-group-pointers.md`) — cross-group pointers, extended by D4.
- `crates/core/src/db.rs` (`compute_lookup_key`), `crates/core/src/cross_group.rs` (`resolve_endpoint`), `crates/core/src/handlers.rs`, `crates/service/src/mcp/tools.rs`.
