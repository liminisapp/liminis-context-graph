# Feature Specification: Temporal (1/3): corrections end belief in what a document used to say, without deleting evidence

**Feature Branch**: `fabrik/issue-705`
**Created**: 2026-10-25
**Status**: Specified
**Input**: User description: "Temporal (1/3): corrections end belief in what a document used to say, without deleting evidence"

## Background

When a document is corrected or replaced, the graph has no way to stop believing what it used to say. On `main`:

- **Correcting a chunk** — resubmitting the same `chunk_id` with new text — is pure append. `handle_knowledge_process_chunk` adds a new episode and nothing ends the old one, so a fact the correction removed stays believed indefinitely.
- **A chunk disappearing** (a section deleted, a heading renamed) typically leads a client to call `knowledge_delete_chunk_episode`, which `DETACH DELETE`s the old episodes. The edges they produced survive and stay believed, while the record of where they came from is destroyed.
- **No link exists from a fact to the episode that produced it.** `RelatesToNode_.episodes` is never written: `insert_relates_to_edge` omits it and extraction passes `episode_uuids: vec![]`. `Episodic.entity_edges`, despite its name, holds the episode's *entity* UUIDs (`episode.rs`, `entity_edges: entity_uuids.clone()`). The `episode_uuids` returned for an edge is inferred at read time from episodes mentioning *either endpoint* — so if "Alice works at Acme" (episode E1) is corrected, the fact still looks supported by an unrelated E2 that says "Alice lives in Boston".

ADR-0673 gave the graph the start of transaction time (`ingested_at`: service clock, first-write, durable in the WAL). This issue adds the end — **when the graph stopped believing something** — without deleting the evidence that it once did.

A correction must not be confused with a change in the world. When a document is corrected, the removed fact was never true, so writing `invalid_at` would falsely claim it was true until the correction. **Correction ends belief; it never touches validity** (`valid_at` / `invalid_at`).

Part 1 of 3. #706 (what changed, and affected work) and #707 (explicit document supersession) build on it.

Related: #404 (unwritten `episodes`; FR-005) · #405 (assertion vs extraction; FR-010) · ADR-0673 · ADR-0046 §2 · ADR-0284.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Correcting a document ends belief in what it used to say (Priority: P1)

A user keeps documents — project summaries, working notes — that accumulate information and get corrected over time. When they correct one, the facts its old text asserted should stop being believed as of the correction, while remaining in history.

**Why this priority**: corrections are constant, and today a corrected error is believed forever.

**Independent Test**: ingest a chunk asserting F; resubmit the same `chunk_id` without F; query believed-only now, and as of a moment between the two ingests.

**Acceptance Scenarios**:

1. **Given** chunk X asserted F at T1, **When** X is resubmitted at T2 without F, **Then** F is not believed as of T2, is believed as of any t in [T1, T2), and remains readable as history.
2. **Given** chunk X asserted F, **When** a client reports that X no longer exists in its document, **Then** X's episodes are superseded, not deleted.
3. **Given** a heading rename re-addresses chunks whose content is unchanged, **When** the renamed chunks are ingested and the old ones reported removed, **Then** no fact present in both revisions is unbelieved at any instant.
4. **Given** a correction, **When** its facts lose belief, **Then** `valid_at` and `invalid_at` are not written.

---

### Edge Cases

- **Re-ingest fails**: the old episode stays live until a resubmission succeeds — belief ends late, never wrongly (FR-003). A client that records a chunk as ingested before the server confirms will never resend a failed correction; clients should commit chunk state only after the ingest succeeds.
- **A split chunk partially fails**: supersede only once every unit of the new ingest has succeeded.
- **The same fact in two live sources**: believed until both are superseded.
- **Correction or change?** An in-place edit cannot say which. The system never infers it: it ends belief and leaves validity alone.
- **Groups**: supersession never crosses a group (ADR-0368 / ADR-0371).
- **Legacy edges** have no producing episode and cannot be soundly backfilled — endpoint co-occurrence is not provenance (#404).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: Routine ingest and chunk removal MUST NOT delete `Episodic` nodes; they supersede them.
- **FR-002**: An episode MUST be able to record that it was superseded: when (service clock), by what (an episode, or — #707 — a document), and why (`revision`, `chunk_removed`; #707 adds `document_superseded`). Nullable, additive, kept in the WAL, and stable across rebuild, tail recovery and checkpoint restore — the ADR-0673 pattern.
- **FR-003**: Resubmitting a `chunk_id` within a group MUST supersede that chunk's live episodes in that group, at the new episode's ingest time, and only after the new ingest — every split unit — has fully succeeded.
- **FR-004**: A new, additive method MUST record that a chunk no longer exists in its document, superseding its episodes with reason `chunk_removed`. `knowledge_delete_chunk_episode` MUST keep its explicit-deletion contract unchanged. (A new method needs a matching MCP `ToolSpec`.)
- **FR-005**: Each extracted edge MUST record the episode that produced it, in the existing `RelatesToNode_.episodes` column. The producing episode's UUID is already in scope at edge insert.
- **FR-006**: Edges with no producing episode — direct assertions, cross-group edges — MUST be marked with their warrant kind. They are believed until a future retraction primitive retracts them.
- **FR-007**: An edge is believed at time t iff it was ingested at or before t and, for extracted edges, at least one episode in `episodes` was not superseded at or before t. Edges with no producing episode and no warrant marker are legacy rows: treated as believed, and reported as legacy.
- **FR-008**: No supersession path may write `valid_at` or `invalid_at`.
- **FR-009**: Relationship reads MUST accept a believed-only filter and an as-of time.
- **FR-010**: A direct assertion MUST NOT overwrite an extracted edge's content. Resolves #405 without the in-place `SET rn.fact` that matching assertions onto extracted edges would otherwise introduce.
- **FR-011**: Each extracted edge MUST record whether its `valid_at` came from the text or defaulted to `reference_time`; today the fallback is silent.
- **FR-012**: `Episodic.entity_edges` MUST be documented as holding entity UUIDs, so it is not mistaken for edge provenance.

### Key Entities *(if applicable)*

- **Episode (`Episodic`)**: a unit of ingested text. Gains nullable supersession fields: when, by what, and why it stopped being live.
- **Extracted edge (`RelatesToNode_`)**: a fact produced from exactly one episode, recorded in `episodes`.
- **Warrant kind**: a marker on edges with no producing episode (direct assertion, cross-group) saying why they are believed.
- **Belief state**: whether an edge is believed at time t, derived from ingest time and the supersession state of its producing episode(s). Distinct from validity (`valid_at` / `invalid_at`).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: After a correction, the corrected-away fact is absent from believed-only reads and present in as-of reads for any time before the correction.
- **SC-002**: Replaying a corpus of real document revisions deletes zero `Episodic` nodes through routine ingest or chunk removal.
- **SC-003**: Across heading renames, no fact present in both revisions is unbelieved at any instant.
- **SC-004**: `knowledge_rebuild_from_wal` reproduces identical belief state, including every supersession timestamp.
- **SC-005**: Believed-only reads contain no superseded copies of re-extracted facts — the duplicate rows that re-ingesting changed chunks accumulates today drop out of that view.
- **SC-006**: No supersession path writes `invalid_at`.

## Assumptions

- **Edges are not deduplicated.** Each extracted row has exactly one producing episode, which lets the existing `episodes` column carry support without a new relation and makes support loss trivial: a row lives or dies with its episode. If dedup is adopted later, the column holds several episodes and FR-007 already reads it as "any live episode".
- **The chunk protocol is unchanged.** A resubmitted chunk already carries a completeness claim over that chunk, and removal is reported explicitly, so nothing depends on #567.
- **Default reads are unchanged**: the believed-only filter ships opt-in. Making it the default is a later, documented behaviour change (precedent: #406 in 0.13.2).
- **Legacy graphs are re-ingested** rather than migrated.

## Out of Scope *(optional)*

- Recording what agents read (an agent audit trail citing edge UUIDs is a plausible later step — rows are never deleted or overwritten, so a cited UUID stays resolvable)
- Inferring correction vs change
- Edge deduplication
- #567
- Fact-level retraction from contradicting sources
- What changed and affected work (#706); explicit document supersession (#707)

## Source References *(optional)*

- #404, #405, #706, #707
- ADR-0673 (ingest-time transaction start), ADR-0046 §2, ADR-0284, ADR-0368 / ADR-0371 (group isolation)
