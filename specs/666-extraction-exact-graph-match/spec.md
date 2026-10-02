# Feature Specification: Exact graph match beats cosine endpoint salvage; UUID-level self-loop guard

**Feature Branch**: `fabrik/issue-666`
**Created**: 2026-10-01
**Status**: Specified
**Input**: User description: "Extraction: exact graph match must beat cosine endpoint salvage; UUID-level self-loop guard (remainder of #645)"

## Background

Issue #645 (community report) described extraction merging or re-pointing distinct sibling entities (for example `ACME-ADR-24001` … `ACME-ADR-24007`). v0.16.3 (#650) shipped the identifier-token veto, which covers #645's digit-sibling cases. Four parts of #645 remain unaddressed:

1. **Salvage ignores the stored graph.** Off-list endpoint salvage in extraction (`crates/core/src/episode.rs`) cosine-matches an edge endpoint missing from the batch's own entity list against **only the batch's** entity names, and rewrites it to the best match at or above `DEDUP_THRESHOLD` (0.85). It never checks whether that endpoint exists **exactly** in the stored graph; that happens only later, in Phase C. An endpoint naming an entity ingested earlier (`Foo` in the graph, `Fooz` in the batch) is therefore rewritten onto the merely similar batch entity, unless #650's veto happens to catch a digit or single-letter difference.
2. **Self-loop guard compares names, not UUIDs.** The post-salvage self-referential filter compares names, and Phase C has no guard of its own. When two differently named endpoints both resolve to the same UUID (through dedup merges or salvage), the edge is inserted with `source == target`.
3. **Digit-free distinct codes still merge.** #650's veto considers only tokens containing digits and single letters, so short all-letter codes differing by one character (`ACDS` vs `ACDM`) still merge by embedding.
4. **Identity kinds could skip fuzzy matching entirely.** For entity kinds declared `identity: true` (#616) the name *is* the identity, so embedding-path merges and salvage onto such entities arguably should not happen at all.

A missed merge is recoverable; a wrong merge (or a wrong re-pointed edge) is not, short of re-ingestion. This work is therefore biased toward conservatism.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Exact stored-graph match wins over cosine salvage (Priority: P1)

A user ingests a document whose edge names an endpoint that is not in that document's entity list but already exists in the graph from an earlier ingest. The edge attaches to that stored entity, not to a merely similar entity in the current batch.

**Why this priority**: This is the core defect — a silent wrong re-point of an edge, unrecoverable without re-ingestion.

**Independent Test**: Seed the graph with entity `Foo`; extract a batch containing entity `Fooz` and an edge whose endpoint is `Foo` (off-list), with fixed embeddings making `Foo`/`Fooz` cosine ≥ 0.85 and no #650 veto token difference. The edge's endpoint resolves to the stored `Foo`.

**Acceptance Scenarios**:

1. **Given** stored entity `Foo` and a batch entity `Fooz` with cosine ≥ 0.85, **When** an edge references off-list endpoint `Foo`, **Then** the endpoint resolves to stored `Foo` and salvage is not attempted for it.
2. **Given** an off-list endpoint whose only case-different form (`foo`) exists in the stored graph in the same group, **Then** it resolves to that entity (case-insensitive).
3. **Given** a stored entity with the same name but in a different group, **Then** it is not an exact hit and the endpoint proceeds as before.
4. **Given** a name that exists in the stored graph under more than one kind, **Then** resolution follows the #616 multi-kind rules (eligible kinds only; the existing ambiguity handling applies rather than guessing).
5. **Given** an off-list endpoint with no exact match in the stored graph and a ≥ 0.85 batch entity that passes the veto, **Then** salvage proceeds exactly as today.

---

### User Story 2 - Edges never become self-loops via UUID collision (Priority: P1)

When two differently named endpoints resolve to the same entity UUID, the edge is dropped rather than stored as a self-loop, and the drop is visible.

**Why this priority**: Self-loop edges are meaningless data produced silently; the existing name-level guard cannot see them.

**Independent Test**: Extract an edge between two names that dedup-merge (or salvage) onto the same entity; verify no edge is persisted and the extraction result counts the drop.

**Acceptance Scenarios**:

1. **Given** an edge whose source and target names resolve to the same UUID after Phase C endpoint resolution, **When** Phase C commits, **Then** the edge is not inserted.
2. **Given** such a drop, **Then** it is logged and counted in the extraction result alongside the existing dropped-edge counts, distinct from name-level self-reference and unresolvable-endpoint drops.
3. **Given** an edge with two genuinely distinct resolved UUIDs, **Then** it is inserted unchanged.

---

### User Story 3 - Sibling-family documents produce no cross-sibling re-pointing (Priority: P1)

A corpus of documents naming `X-24001` … `X-24007` with cross-references ingests so that every endpoint stays on its own sibling and no self-loops appear.

**Why this priority**: This is #645's original reported scenario and the end-to-end acceptance for the whole remainder.

**Independent Test**: Integration test ingesting documents naming `X-24001` … `X-24007` with cross-references.

**Acceptance Scenarios**:

1. **Given** the sibling-family corpus, **When** it is ingested, **Then** zero edge endpoints are re-pointed to a different sibling.
2. **Given** the same corpus, **Then** zero self-loop edges exist in the graph.

---

### User Story 4 - Short all-letter distinct codes are not merged by embedding (Priority: P2)

Codes such as `ACDS` and `ACDM` stay distinct. Research decides the mechanism: either extend the identifier veto to treat short all-uppercase tokens (acronym/code shape, for example ≤ 6 characters) as distinguishing tokens, or leave it to the opt-in LLM check (#652) and document that limitation. The conservative option must win ties.

**Why this priority**: Real but narrower than the P1 defects, and the veto extension risks blocking genuine aliases.

**Independent Test**: With fixed embeddings forcing cosine ≥ 0.85, `ACDS`/`ACDM` either are vetoed (if the veto is extended) or the documented limitation is asserted by a test and in docs.

**Acceptance Scenarios**:

1. **Given** the Research decision to extend the veto, **When** `ACDS` and `ACDM` are resolved at cosine ≥ 0.85, **Then** they are not merged, and genuine aliases such as `PostgreSQL`/`Postgres` and `New York`/`New York City` still merge.
2. **Given** the Research decision to defer to #652, **Then** the limitation and its opt-in mitigation are documented and the behaviour is pinned by a test.

---

### User Story 5 - Identity kinds never fuzzy-merge (Priority: P3, optional)

Research weighs disabling embedding-path merges and salvage onto entities whose kind is `identity: true`. If adopted, such entities only match by exact name.

**Why this priority**: Optional hardening; value depends on how identity kinds are used in practice.

**Independent Test**: If adopted, an identity-kind entity and a similar-named incoming entity at cosine ≥ 0.85 do not merge and are not salvage targets.

**Acceptance Scenarios**:

1. **Given** adoption, **When** an incoming entity is similar to an `identity: true` stored entity, **Then** it is inserted separately; exact-name matches still resolve.
2. **Given** non-adoption, **Then** the reasoning is recorded and existing behaviour is unchanged.

---

### Edge Cases

- An off-list endpoint exactly matching a stored entity whose kind is not eligible for the edge's relation (per #616) — must not be force-matched.
- An endpoint present both in the batch list and in the stored graph — unchanged (batch list handling already precedes salvage).
- An exact stored hit combined with the other endpoint resolving to the same UUID — handled by the UUID self-loop guard (Story 2).
- Whitespace/normalisation differences — "exact" uses the same case-insensitive normalisation as existing name matching.
- #650's existing veto tests and `salvage_vetoed` counting continue to pass unchanged.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: Before cosine salvage, each off-list edge endpoint MUST be looked up exactly (case-insensitive) in the stored graph, restricted to the same group and eligible kinds, consistently with #616 multi-kind resolution.
- **FR-002**: An exact stored-graph hit MUST resolve the endpoint to that entity and MUST skip salvage for it, even when a batch entity scores ≥ `DEDUP_THRESHOLD`.
- **FR-003**: Only endpoints with no exact match in the stored graph MAY be salvage candidates; existing salvage and veto behaviour is otherwise unchanged.
- **FR-004**: In Phase C, after endpoint resolution, any edge whose resolved source and target UUIDs are equal MUST be dropped rather than inserted.
- **FR-005**: Each UUID-level self-loop drop MUST be logged and counted in the extraction result alongside the existing dropped-edge counts.
- **FR-006**: Research MUST decide, and the implementation MUST then cover with tests, how digit-free distinct short codes (`ACDS`/`ACDM`) are treated: veto extension for short all-uppercase tokens, or documented deferral to #652. The decision MUST NOT regress genuine alias merges.
- **FR-007**: Research MUST decide whether `identity: true` kinds are excluded from embedding-path merges and salvage targets; the outcome MUST be tested if adopted, or documented if rejected.
- **FR-008**: All changes MUST apply at extraction time only; WAL replay behaviour is unchanged.
- **FR-009**: No schema change; the change is patch-sized.
- **FR-010**: #650's existing veto tests MUST continue to pass.

### Key Entities

- **Off-list endpoint**: An edge endpoint name absent from the batch's own extracted entity list.
- **Extraction result counts**: The existing set of dropped-edge counters, extended with a UUID-level self-loop count.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A fixed-embedding test shows an off-list endpoint exactly matching a stored entity resolves to it even when a similar batch entity scores ≥ 0.85.
- **SC-002**: A test shows two endpoints resolving to the same UUID never yield a self-loop edge and the drop is counted in the extraction result.
- **SC-003**: Whatever is decided for FR-006 and FR-007 is covered by tests, and all pre-existing #650 veto tests pass.
- **SC-004**: An integration test of #645's sibling-family scenario (`X-24001` … `X-24007` with cross-references) shows zero re-pointed endpoints and zero self-loops.

## Assumptions

- "Exact" means the same case-insensitive normalised name already used for Phase B's exact match.
- The #616 kind-resolution rules define which stored kinds are eligible; this issue reuses them rather than redefining them.
- Stories 4 and 5 are intentionally left to Research; absent clear evidence of safety, the conservative outcome (no new merges, documented limitation) is preferred.
- Existing bad merges are not repaired; recovery remains re-ingestion.

## Out of Scope

- Repairing graphs already corrupted by prior wrong merges.
- Changes to WAL replay, schema, or the opt-in LLM dedup check (#652).
- Changes to the Phase B exact-match logic or the #650 digit/single-letter veto beyond what FR-006 decides.

## Source References

- #645 (original report), #650 (identifier veto), #652 (LLM dedup check), #616 (kind-scoped identity / multi-kind resolution)
- `crates/core/src/episode.rs` (off-list salvage, name-level self-reference filter, Phase C)
- `specs/650-extraction-dedup-merges-distinct/spec.md`
