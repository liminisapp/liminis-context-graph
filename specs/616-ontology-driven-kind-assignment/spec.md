# Feature Specification: Ontology-driven kind assignment — identity-bearing entity types during extraction

**Feature Branch**: `fabrik/issue-616`
**Created**: 2026-09-29
**Status**: Specified
**Input**: User description: "Let the ontology mark entity types as identity-bearing, so extraction assigns them as an entity's `kind`. An extracted `Person \"Aurora\"` then stays distinct from a `Technology \"Aurora\"`, while non-identity types keep merging by name as they do today."

## Background

Community report **#614** (point 3) asked for extracted entities of different real-world kinds to stay distinct even when they share a name. **#615** (merged) made `Entity.kind` part of identity — `(group_id, kind, name)` — and made an entity's kind immutable, but deliberately left extraction assigning the default kind `Entity` to everything. Only *asserted* entities currently get a non-default kind.

This issue is the second phase: letting extraction assign kinds. It is kept separate from #615 because extracted typing is inconsistent: the same real-world "Aurora" can be typed `Technology` in one chunk and `Product` in another. If every extracted type were an identity key, one thing would split into several nodes. Kind assignment during extraction therefore has to be **opt-in per type**, decided by the ontology author, who knows which types are stable enough to carry identity (typically `Person`, `Organization`).

The governing principle: **in lcg, identity is decided once, at creation, and never inferred afterwards.** #615 applied this to assertions (kind is immutable). This issue extends it to extraction, which has two consequences settled in triage:

- **A WAL rebuild cannot apply an identity flag retroactively.** Replay is purely mechanical Cypher re-execution: it replays the `kind` each record already carries and never calls the extractor. Flipping a flag and rebuilding yields exactly the same graph.
- **Kind cannot be re-derived from stored labels.** If `"Aurora"` was extracted as `Person` in one chunk and `Technology` in another, non-identity rules already merged them into one node carrying both chunks' edges. lcg cannot tell whether that was one thing typed inconsistently or two things wrongly merged; the information needed to separate them was never recorded.

So a change to a group's identity-bearing set that would reinterpret existing data is **refused**, not guessed at (the #414 principle applied to identity). The remedy is re-ingesting the group from source.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Keep a person and a technology of the same name apart (Priority: P1)

An ontology author marks `Person` as `identity: true`. During extraction, an entity typed `Person` named "Aurora" is created with kind `Person`; a different chunk's "Aurora" typed `Technology` (not identity-bearing) is created with the default kind `Entity` carrying the `Technology` label. The two are separate entities.

**Why this priority**: This is the whole point of the issue and of #614 point 3.

**Independent Test**: Ingest two episodes with a stubbed extractor that returns "Aurora" as `Person` in one and `Technology` in the other, under an ontology with `Person: identity: true`; assert two entities exist.

**Acceptance Scenarios**:

1. **Given** an ontology with `Person` marked `identity: true`, **When** "Aurora" is extracted as `Person` in one episode and as `Technology` in another, **Then** the graph holds two entities: one with kind `Person`, one with kind `Entity` carrying the `Technology` label.
2. **Given** the same extraction with **no** identity flags, **When** both episodes are ingested, **Then** the graph holds one entity, exactly as today.
3. **Given** `Person` is identity-bearing, **When** "Aurora" is extracted as `Person` in two different episodes, **Then** both resolve to the same single kind-`Person` entity.
4. **Given** `Person` is identity-bearing and a `Technology` "Aurora" (kind `Entity`) exists, **When** a `Person` "Aurora" is extracted, **Then** it is never merged into the `Technology` entity.

---

### User Story 2 - Existing ontologies and corpora are undisturbed (Priority: P1)

Any ontology without identity flags, and any workspace with none, behaves byte-for-byte as before, including WAL replay of existing corpora.

**Why this priority**: Regression safety; extraction and replay are the core of the service.

**Independent Test**: Replay the #217 real-corpus capture and the existing extraction test suites unchanged; compare entity, relationship and episode counts.

**Acceptance Scenarios**:

1. **Given** an ontology file with no `identity` keys, **When** it is loaded, **Then** every entity type is non-identity and extraction assigns kind `Entity` throughout.
2. **Given** an existing corpus with no identity flags, **When** it is replayed from the WAL, **Then** entity, relationship and episode counts are identical to before this change.

---

### User Story 3 - Safely evolve the identity-bearing set (Priority: P2)

An ontology author adds `identity: true` to a type the group has never extracted. It is accepted and takes effect for later extractions. Adding or removing the flag for a type the group already holds entities of is refused with a clear error, so no existing node is silently reinterpreted.

**Why this priority**: Without this guard, the hazard described in the Background silently corrupts graphs; but it is only reachable after Story 1 exists.

**Independent Test**: Build a group with `Person`-labelled entities, change the flag, load the ontology, and assert the refusal and its message; repeat with a type no entity carries and assert acceptance.

**Acceptance Scenarios**:

1. **Given** a group whose entities carry no `Organization` label, **When** the ontology gains `Organization: identity: true`, **Then** the ontology is accepted, the group's recorded identity-bearing set is updated, and later `Organization` extractions get kind `Organization`.
2. **Given** a group containing entities labelled `Person`, **When** the ontology gains `Person: identity: true`, **Then** loading is refused with an error naming `Person` and the number of affected entities.
3. **Given** a group with identity-bearing `Person` and existing `Person` entities, **When** the flag is removed, **Then** loading is refused with the same style of error.
4. **Given** a refused change, **When** the ontology is restored to its previously recorded identity-bearing set, **Then** the group loads normally and no data has been modified.

---

### User Story 4 - Reprocessing never changes kind (Priority: P2)

`knowledge_reprocess_entity_types` continues to rebuild descriptive labels from the ontology but never alters an entity's kind, even when reclassification lands on a different identity-bearing type. It may report such disagreements for human review.

**Why this priority**: Preserves #615's guarantee; a merge-or-split decision belongs to a person.

**Independent Test**: Create a kind-`Person` entity whose reclassification would land on identity-bearing `Organization`; run reprocess; assert kind and the `kind ∈ labels` invariant are unchanged.

**Acceptance Scenarios**:

1. **Given** an entity with kind `Person` that reclassifies under identity-bearing `Organization`, **When** `knowledge_reprocess_entity_types` runs, **Then** the entity's kind is still `Person` and `Person` is still in its labels.

---

### Edge Cases

- **Multiple identity-bearing types on one entity (FR-003)**: extraction yields one *primary* type per entity; parent-hierarchy ancestors are added as extra labels (e.g. `Rfc` → `Document`). Kind is decided by the primary type alone, and ancestor labels never confer kind. Hence an `Rfc` extracted under a hierarchy where `Document` is identity-bearing but `Rfc` is not gets kind `Entity`; the result never depends on label order or ontology declaration order.
- **Unclassified / undeclared types**: an entity whose type is undeclared (open mode) or reclassified to `Unclassified` (strict mode) is non-identity and gets kind `Entity`.
- **Name normalization**: the flag is matched against the normalized (PascalCase) type name, like every other ontology type match.
- **Legacy groups (no recorded set)**: entities extracted before this change were all created with kind `Entity`, so an absent record is equivalent to an empty identity-bearing set.
- **Refusal scope**: a refused ontology change affects only the offending group; other groups and service startup are unaffected, and no entity is modified.
- **Ontology source changes**: the comparison applies equally when a group's ontology comes from its per-group file or from the workspace fallback, and when the source it resolves through changes.
- **Asserted entities carrying an identity label**: an asserted entity of kind `Person` counts as an entity carrying the `Person` label for the D1 check; flipping `Person`'s flag is refused in that group.
- **Ancestor labels count as carried labels**: the D1 check looks at labels carried, so flipping the flag on an ancestor type such as `Document` is refused if existing `Rfc` entities carry the `Document` label. This is conservative by design.
- **Rebuild from WAL / published streams**: replay restores each record's own kind and never re-evaluates flags; a consumer's own ontology governs its own extraction (published ontology remains documentation only).
- **General drift**: because the identity flag changes the ontology content hash, existing drift detection (#451) will also warn; that warning is separate from, and weaker than, the D1 refusal.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The ontology format MUST support a per-entity-type `identity: true` flag. Absent means `false`. Existing ontologies MUST behave exactly as today.
- **FR-002**: During extraction, an entity whose extracted type is identity-bearing in its group's resolved ontology (#446) MUST be created with `kind` set to that type. All other extracted entities MUST use the default kind `Entity` and merge by name exactly as today. Entity resolution for an identity-bearing kind MUST match only entities of the same kind within the group.
- **FR-003**: Kind MUST be determined solely by the entity's primary extracted type. Identity status is never inherited from a parent type, and no ordering of labels or ontology declarations may influence it. An entity whose primary type is not identity-bearing gets kind `Entity` even if an ancestor type is identity-bearing.
- **FR-004**: The identity-bearing set in force MUST be recorded per group when entities are first extracted under it (D2), stored outside the WAL alongside the group's existing position record. An absent record MUST be interpreted as an empty set.
- **FR-005**: When a group's resolved ontology is loaded, its identity-bearing set MUST be compared with the recorded one. A difference that changes the identity status of a label carried by any existing entity in the group MUST be refused with an error naming the label and the number of entities affected (D1). A difference touching only labels no existing entity carries MUST be accepted and the record updated. A refusal MUST NOT modify any data and MUST NOT affect other groups.
- **FR-006**: `knowledge_reprocess_entity_types` MUST NOT change `kind` and MUST NOT remove `kind` from `labels` (D3). It MAY report an entity whose classification disagrees with its identity-bearing kind, without acting on it.
- **FR-007**: Asserted entities MUST continue to use the caller's `kind` strictly, unaffected by the identity flag (#615 behaviour preserved).
- **FR-008**: `docs/ontology.md` MUST document the flag, which types should carry it (stable, well-defined types such as people and organizations, not inconsistently-typed ones), that it is evaluated at extraction time only, that WAL rebuild cannot apply it retroactively, and that it cannot be changed for a type the group already holds without re-ingesting from source.

### Key Entities

- **Entity type (ontology)**: a declared type, now with an optional `identity` flag; identity-bearing types become the entity's `kind` on extraction.
- **Entity kind**: part of identity `(group_id, kind, name)`, immutable after creation (#615). Default `Entity`.
- **Identity-bearing set stamp**: a per-group record of which entity types were identity-bearing when the group's entities were extracted; checked whenever the group's ontology loads. Stored outside the WAL beside the group's position record.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: With `Person` marked `identity: true`, extracting `"Aurora"` as `Person` in one chunk and as `Technology` in another yields **two** entities: kind `Person`, and kind `Entity` carrying the `Technology` label.
- **SC-002**: With no identity flags, the same extraction yields **one** entity, exactly as today.
- **SC-003**: Adding `identity: true` for a type no existing entity in the group carries is accepted, and subsequent extractions of that type get it as their kind.
- **SC-004**: Adding or removing `identity: true` for a type existing entities in the group already carry is refused, and the error names the label and the number of entities affected.
- **SC-005**: `knowledge_reprocess_entity_types` never changes an entity's `kind`, verified against an entity whose reclassification lands on a different identity-bearing type.
- **SC-006**: Existing corpora with no identity flags replay to identical entity, relationship and episode counts, including the #217 real-corpus capture.
- **SC-007**: An entity whose primary type is non-identity but whose ancestor type is identity-bearing is created with kind `Entity`, independent of label or declaration order.

## Assumptions

- Extraction produces one primary type per entity; ancestor labels come from the ontology's `parent` hierarchy. (FR-003 resolved on this basis.)
- Because all pre-#616 extraction assigned kind `Entity`, a group with no recorded set is correctly modelled as having an empty identity-bearing set.
- The ontology remains loaded once per group per process (per existing behaviour), so the D1 comparison runs at that load point and a running process does not re-check mid-flight.
- Both the workspace-wide and per-group ontology files gain the flag with the same syntax.
- Whether the new stamp is stored as extra fields on the existing position record or a sibling record is a Plan-stage decision; the requirement is only "per group, outside the WAL, alongside the position record."

## Out of Scope

- **General per-group ontology drift detection**: #451, unaffected (D2). This issue records only the identity-set slice it needs and does not depend on #451.
- **Re-ingest tooling**: D1's remedy is re-ingesting from source; this issue documents it but does not automate it.
- **Merge-or-split tooling** for entities that reprocessing reports as disagreeing with their kind.
- Changing the kind of an existing entity (immutable per #615).

## Source References

- Issues: #614 (community report), #615 / PR #617 (kind-scoped identity, kind immutability, reprocess guard), #446 (per-group ontologies), #440 / #526 (per-group model stamp on `WalPositionRecord`), #451 (general drift), #414 (refuse rather than guess), #217 (real-corpus capture).
- `docs/ontology.md`; `crates/core/src/db.rs` (`WalPositionRecord`); `crates/core/src/handlers.rs` (kind immutability check, reprocess restamp via `labels_with_kind`); `crates/core/tests/fixtures/real_corpus_wal/README.md`.
