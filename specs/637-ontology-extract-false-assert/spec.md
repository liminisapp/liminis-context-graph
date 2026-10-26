# Feature Specification: Ontology `extract: false` — assert-only types excluded from the LLM extractor

**Feature Branch**: `fabrik/issue-637`
**Created**: 2026-09-30
**Status**: Specified
**Input**: User description: "Ontology extract: false — assert-only types excluded from the LLM extractor (implements #636)"

## Background

Host applications increasingly ship a base ontology containing *structural* types that the host itself asserts through `knowledge_assert_entity` / `knowledge_assert_relationship` — for example a `Source` entity type and a `DERIVED_FROM` relation (motivating community report: #636). Today every declared ontology type is rendered into the extraction prompts, so the LLM extractor keeps minting those types from prose (e.g. a `Source` entity each time a wiki page is mentioned).

Since #616 (`identity: true`), an extracted entity whose primary type is identity-bearing is created with that type's identity `kind`. For an assert-only type this produces fuzzy, unkeyed entities inside an identity-bearing kind, which collide with or duplicate the host's canonically keyed assertions. A "do not extract" sentence in the type's `description` is only a prompt hint and cannot guarantee anything.

The ontology format needs a way to declare that a type is *assert-only*: present for everything except LLM extraction.

State of `main` at time of writing:
- `EntityTypeDef` has `name`, `description`, `parent`, `identity`; `RelationTypeDef` has no equivalent flag.
- The entity and edge extraction system prompts render every declared type.
- `Ontology::identity_kind()` assigns an identity kind to any extracted entity whose primary type is identity-bearing.
- `content_hash` appends `identity` only when at least one type sets it, so unflagged ontologies keep their old hash.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Assert-only types never appear to the extractor (Priority: P1)

A host author declares `Source` (`identity: true, extract: false`) and `DERIVED_FROM` (`extract: false`) in the group's ontology. The LLM extraction vocabulary must contain neither.

**Why this priority**: This is the primary lever — removing the types from the vocabulary prevents nearly all unwanted extractions at the source.

**Independent Test**: Render the entity and edge system prompts for a group with that ontology and assert neither name (nor its description) appears; render for an ontology without the flag and assert the prompt is byte-identical to today's.

**Acceptance Scenarios**:

1. **Given** an ontology declaring `Source` with `extract: false`, **When** the entity extraction system prompt is rendered, **Then** `Source` is not listed.
2. **Given** an ontology declaring `DERIVED_FROM` with `extract: false`, **When** the edge extraction system prompt is rendered, **Then** `DERIVED_FROM` is not listed.
3. **Given** an ontology with no `extract` key on any type, **When** the prompts are rendered, **Then** they are byte-identical to the output before this change.

---

### User Story 2 - Stray extractor output never receives an assert-only identity kind (Priority: P1)

Even with the type removed from the prompt, the extractor may still emit the label (open mode, a hallucinated label, or prose that invites it). Such an output must be treated as off-ontology, never as the assert-only type.

**Why this priority**: The prompt is advisory; this is the correctness guarantee that protects identity-bearing kinds from pollution.

**Independent Test**: Run ingestion with a canned extraction emitting an entity typed `Source` (and an edge typed `DERIVED_FROM`) under an ontology flagging both `extract: false`; assert the entity is not given kind `Source` and is `Unclassified` with `original_entity_type = "Source"`, and the edge is not kept as `DERIVED_FROM`.

**Acceptance Scenarios**:

1. **Given** `Source` is `identity: true, extract: false`, **When** the extractor emits an entity with type `Source` in strict mode, **Then** the entity is reclassified `Unclassified`, its original label is preserved in `original_entity_type`, and its kind is not `Source`.
2. **Given** the same ontology in open mode, **When** the extractor emits `Source`, **Then** the entity is likewise reclassified `Unclassified` with the original label preserved and no `Source` kind (open-mode behaviour for `extract: false` types is decided during Research and recorded in the ADR; the lean is reclassify in both modes).
3. **Given** `DERIVED_FROM` is `extract: false`, **When** the extractor emits an edge of that type, **Then** it goes through the existing off-ontology relation path and is never stored as `DERIVED_FROM`.

---

### User Story 3 - Hosts can still assert the types (Priority: P1)

`extract: false` is purely an extraction-time concept. All host-driven paths treat the type as fully declared.

**Why this priority**: The flag is useless if it breaks the reason the types exist.

**Independent Test**: With the flagged ontology, call `knowledge_assert_entity {kind: "Source", …}` and `knowledge_assert_relationship` with `DERIVED_FROM`; both succeed with the normal typing, `kind`, and identity-scoping.

**Acceptance Scenarios**:

1. **Given** `Source` is `identity: true, extract: false`, **When** a host calls `knowledge_assert_entity {kind: "Source", …}`, **Then** the entity is created identity-scoped to kind `Source`.
2. **Given** `DERIVED_FROM` is `extract: false`, **When** a host calls `knowledge_assert_relationship` with that type, **Then** the relationship is created typed `DERIVED_FROM`.
3. **Given** a type that is both `identity: true` and `extract: false`, **When** the identity-set guard (`-32003`) compares stored vs. configured identity sets, **Then** the type counts toward the identity set exactly as if it were extractable.
4. **Given** a child type whose `parent` is an `extract: false` type, **When** label stamping / ancestry is computed, **Then** the parent chain is honoured as today.

---

### User Story 4 - Reprocessing never retypes into an assert-only type (Priority: P2)

**Why this priority**: Reprocessing derives types from extracted evidence, so it would otherwise be a back door for the same pollution.

**Independent Test**: Run `knowledge_reprocess_entity_types` on entities whose evidence suggests `Source`; none are retyped into `Source`.

**Acceptance Scenarios**:

1. **Given** an `extract: false` type `Source`, **When** `knowledge_reprocess_entity_types` runs, **Then** no extracted entity is retyped into `Source`.

---

### User Story 5 - Flag changes are visible as drift but not identity-refused (Priority: P2)

**Why this priority**: Operators need the change detected, but flipping extractability must not be blocked like an identity-set change.

**Independent Test**: Hash the same ontology with and without `extract: false` on one type; hashes differ. Hash an ontology with no flagged type; hash equals today's. Reload after flipping is accepted.

**Acceptance Scenarios**:

1. **Given** an ontology with no `extract: false` type, **When** `content_hash` is computed, **Then** the value is identical to the pre-change value.
2. **Given** a type flipped to `extract: false`, **When** `content_hash` is computed, **Then** it differs and drift is reported.
3. **Given** a running group whose only ontology change is flipping `extract`, **When** `knowledge_reload_ontology` (#627) is called, **Then** the reload is accepted and `identity_refusal` is `null`.

---

### Edge Cases

- A type declared `extract: false` in an ontology where the extractor is in open mode and emits that label: handled per US2 scenario 2 (Research decision, ADR).
- `extract: false` on a type that is the `parent` of extractable types: the flag is per-type and not inherited; ancestry and label stamping remain unchanged.
- `extract: false` with no `identity` flag (e.g. `DERIVED_FROM`, or a non-identity entity type): valid; behaves per FR-001–FR-004.
- Explicit `extract: true` and an absent key are equivalent; neither alters prompts or hash.
- Every entity (or relation) type in an ontology flagged `extract: false`: prompts must still render validly (an empty vocabulary must not produce a malformed prompt).
- A stray extracted entity that is reclassified `Unclassified` must not be silently dropped; its original label is preserved.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: `EntityTypeDef` and `RelationTypeDef` (and their raw deserialization structs) MUST accept an optional boolean `extract`, defaulting to `true` when absent.
- **FR-002**: Types with `extract: false` MUST NOT appear in the entity or edge extraction system prompts.
- **FR-003**: An ontology with no `extract: false` type MUST render byte-identical extraction prompts to those produced before this change.
- **FR-004**: An extracted entity whose type is an `extract: false` entity type MUST NOT receive that type's identity `kind` from the extraction path.
- **FR-005**: Such an entity MUST be reclassified via the existing off-ontology handling (`Unclassified`, original label preserved in `original_entity_type`; ADR-0033/ADR-0310/ADR-0312). Behaviour in open mode MUST be decided in Research and recorded in the ADR; the expected outcome is reclassification in both strict and open modes.
- **FR-006**: An extracted relationship whose type is an `extract: false` relation type MUST go through the existing off-ontology relation path and MUST NOT be stored as that type.
- **FR-007**: `knowledge_assert_entity` and `knowledge_assert_relationship` MUST treat `extract: false` types as fully declared (typing, `kind`, validation).
- **FR-008**: `extract: false` types MUST count fully toward `identity` sets and the identity-set guard (`-32003`), `parent` ancestry and label stamping, and ontology drift detection.
- **FR-009**: `knowledge_reprocess_entity_types` MUST NOT retype an entity into an `extract: false` type from extracted evidence.
- **FR-010**: `content_hash` MUST include `extract: false` entries only when at least one type sets the flag (following the `identity` precedent), so ontologies without the flag hash exactly as before.
- **FR-011**: Flipping `extract` on a type MUST change `content_hash` and surface as drift, MUST NOT change the identity set, and MUST NOT cause `knowledge_reload_ontology` to be identity-refused.
- **FR-012**: `docs/ontology.md` MUST document the field and include an assert-only example (`Source` + `DERIVED_FROM`); the open-mode decision MUST be recorded in `docs/adr/0637-<slug>.md` (and indexed in `docs/adr/index.md`).

### Key Entities

- **Entity type definition**: ontology entity type (`name`, `description`, `parent`, `identity`) gaining `extract`.
- **Relation type definition**: ontology relation type gaining `extract`.
- **Assert-only type**: any type with `extract: false` — declared and assertable, but never offered to or accepted from the LLM extractor.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: With `Source` (`identity: true, extract: false`) and `DERIVED_FROM` (`extract: false`) declared, the rendered entity and edge extraction prompts list neither.
- **SC-002**: Ingesting prose mentioning a wiki page or source yields zero entities with kind `Source`; a canned extraction emitting `Source` yields an `Unclassified` entity with `original_entity_type = "Source"` and not kind `Source`.
- **SC-003**: `knowledge_assert_entity {kind: "Source", …}` and `knowledge_assert_relationship` with `DERIVED_FROM` succeed and are identity-scoped / correctly typed.
- **SC-004**: `knowledge_reprocess_entity_types` produces zero retypings of extracted entities into `Source`.
- **SC-005**: A regression test shows an ontology without the flag yields byte-identical prompts and an identical `content_hash` to the pre-change values.
- **SC-006**: Flipping `extract` on a type changes the hash and reports drift, and `knowledge_reload_ontology` accepts the change (not identity-refused).

## Assumptions

- `extract` is per-type and not inherited through `parent`.
- The change is an additive ontology-format field: no database schema or storage change, no IPC method additions (existing MCP `ToolSpec` registry and parity tests are unaffected).
- Existing off-ontology handling (`Unclassified`, `original_entity_type`, off-ontology relation path) is reused rather than redesigned.
- Existing ontologies and stored data require no migration.

## Out of Scope

- Host-side composition of base ontologies.
- Any new rejection behaviour on `knowledge_assert_*` for `extract: false` types.
- Changes to DB schema or storage.

## Source References

- Issue #636 (community report, motivating case); #616 (`identity: true`); #615 (kind-scoped identity); #627 (`knowledge_reload_ontology`)
- `crates/core/src/ontology.rs` (`EntityTypeDef`, `RelationTypeDef`, `identity_kind()`, `content_hash`), `crates/core/src/extractor.rs`, prompts module
- ADR-0033, ADR-0310, ADR-0312 (off-ontology handling); `docs/ontology.md`
