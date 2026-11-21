# Feature Specification: Stop ontology extraction from creating entities named after the note ID

**Feature Branch**: `fabrik/issue-824`
**Created**: 2026-11-21
**Status**: Specified
**Input**: User description: "Ontology extraction creates junk entities named after the note ID (13 on v2, e.g. project-quill-core-1). Extraction under an ontology creates entities named after the note itself, such as `project-quill-core-1`. On the v2 corpus there are 13 such nodes under ontology v3.1, against 2 in the free-form graph (#819's baseline; E13 in `docs/retrieval-experiments.md`). These nodes collect edges that belong to the real entities. They show up in walks and entity search as noise. The likely cause is an ontology `Document` entity type inviting the model to extract the note as an entity, naming it from the note ID in the chunk header. Change (for Research to confirm): Find where the name comes from: the chunk header, `source_description`, or the note's title line. Then stop extraction producing an entity whose name is the episode's own source ID, by prompt, by a post-extraction filter, or both. If an entity of the source note is wanted, it should be the asserted `Document`/`Source` entity Liminis already supplies, not an extracted one. Acceptance: On the v2 corpus under ontology v3.1, note-ID entities go from 13 to 0, with no loss of link recall or typing (#819's metrics). A unit test: a chunk whose header carries a note ID produces no entity of that name. Out of scope: Merging or deleting existing junk nodes in stored graphs. `knowledge_merge_entities` already covers that by hand."

## Background

When extraction runs under an ontology, the model sometimes emits an entity whose name is the identifier of the note being processed (for example `project-quill-core-1`) rather than a real-world thing the note talks about. On the v2 corpus under ontology v3.1 there are 13 such entities, versus 2 in the free-form (no ontology) graph, as measured by #819's baseline (experiment E13 in the retrieval experiments notes).

These "note-ID entities" are harmful: they absorb edges that belong to the genuine entities the note describes, and they then appear as noise in graph walks and entity search results, lowering retrieval quality. The suspected cause is that the ontology defines a `Document` entity type, which invites the model to treat the note itself as an entity and name it from the note ID shown in the chunk header. The fix should come from locating where the model picks up that name (chunk header, source description, or the note's title line) and preventing extraction from producing entities that merely name the episode's own source. When a graph entity for the source note is desired, Liminis already supplies an asserted `Document`/`Source` entity; that — not an extracted entity — is the correct representation.

This issue is blocked by #819, which provides the baseline and metrics (link recall, typing) used to judge success.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Note-ID entities are no longer extracted (Priority: P1)

An operator ingests notes under an ontology. Chunks carry a header that includes the note's ID. Extraction must not return an entity whose name is that note ID (or the episode's own source identifier).

**Why this priority**: This is the defect itself; every other outcome depends on it.

**Independent Test**: Run extraction (or the post-extraction processing step with a representative model response) on a chunk whose header carries a note ID, where the model output includes an entity named after that ID; verify no entity with that name is stored.

**Acceptance Scenarios**:

1. **Given** a chunk whose header carries note ID `project-quill-core-1` and an ontology that includes a `Document` entity type, **When** extraction runs, **Then** no entity named `project-quill-core-1` is created.
2. **Given** an extraction result that proposes an entity whose name equals the episode's source ID, **When** the result is processed, **Then** that entity is dropped and edges that referenced only it do not create junk nodes.
3. **Given** the same chunk, **When** extraction runs, **Then** all other legitimate entities and relationships from the chunk are still extracted as before.

---

### User Story 2 - No regression in link recall or typing on the v2 corpus (Priority: P1)

An evaluator re-runs the v2 corpus under ontology v3.1 and compares against #819's metrics.

**Why this priority**: The fix is only acceptable if it removes noise without removing real signal.

**Independent Test**: Run the #819 evaluation on the v2 corpus under ontology v3.1 before and after the change; count note-ID entities and compare link recall and typing metrics.

**Acceptance Scenarios**:

1. **Given** the v2 corpus under ontology v3.1, **When** it is extracted after the change, **Then** the number of entities named after their own note ID is 0 (down from 13).
2. **Given** the same run, **When** #819's link recall and typing metrics are computed, **Then** neither is lower than the pre-change baseline (within the metric's normal run-to-run noise).

---

### User Story 3 - The asserted source entity remains the way to represent the note (Priority: P2)

An operator who wants the note itself represented in the graph relies on the `Document`/`Source` entity Liminis asserts, not on one the model extracts.

**Why this priority**: Confirms the change does not remove a capability, only the faulty duplicate path.

**Independent Test**: Ingest a note and verify the asserted Document/Source entity is still created and linked as before.

**Acceptance Scenarios**:

1. **Given** a note ingested under an ontology, **When** ingestion completes, **Then** the asserted `Document`/`Source` entity exists exactly as it did before this change.

---

### Edge Cases

- The model names the entity with a variant of the note ID (different case, with/without file extension, surrounding punctuation, path prefix); these should be treated as the same identifier.
- A genuine real-world entity legitimately shares a name with a note ID (for instance, a project literally called "project-quill-core-1" mentioned in the text of another note); the rule targets an entity matching *its own episode's* source ID, or a lowercase ID-shaped `[[wikilink]]` target cited in *this episode's* body that names a note already ingested in the group — not an arbitrary other note's ID that this episode does not cite.
- A chunk that has no note ID or source identifier in its header: behaviour is unchanged.
- An extracted edge whose endpoint is the dropped note-ID entity: it must not leave dangling references or recreate the entity implicitly.
- Free-form (no ontology) extraction, which already has a small residual of such entities (2 on v2), should not regress and may benefit.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: Extraction MUST NOT produce a persisted entity whose name is the identifier of the episode/note being extracted (its own source ID), as it appears in the chunk header or source description.
- **FR-002**: The determination of where the note-ID name originates (chunk header, source description, or title line) MUST be established and the fix MUST address that origin.
- **FR-003**: The prevention MUST be robust to the model ignoring any prompt guidance; a deterministic safeguard after extraction MUST drop any entity that still matches the episode's own source ID.
- **FR-004**: Matching of the note ID to entity names MUST tolerate trivial variations (case, surrounding whitespace/punctuation, file extension).
- **FR-005**: Relationships that reference a dropped note-ID entity MUST be handled so that no junk node is created and no unrelated entity or edge is lost.
- **FR-006**: The asserted `Document`/`Source` entity supplied by Liminis MUST remain unaffected.
- **FR-007** *(amended after the maintainer's trace of the 13 v2 nodes — all are other notes' IDs cited as `[[wikilinks]]`, none is the note's own ID; narrowed after review because no name shape separates them from `GPT-4`/`covid-19`)*: An extracted entity MUST also be dropped when **all** of these hold: (a) its name, *before normalisation*, is verbatim the target of a `[[wikilink]]` in the episode body, and that target is all-lowercase, contains no whitespace and is ID-shaped (a letter plus a `-`, `_` or digit); and (b) that target is the source ID (doc ID, or source-file stem) of an episode already ingested in the **same group**. Edges to it are dropped as for the own-ID rule, and it counts in the same `entities_dropped_source_id` counter. A link to a real entity (`[[Quill Core]]`), a capitalised concept link (`[[GPT-4]]`), a lowercase link to a note not in the group (`[[machine-learning]]`), and an ID-shaped name that is neither the episode's own ID nor such a citation MUST NOT be dropped. A note ingested after the one that cites it is not caught by this rule (accepted). The own-ID rule (FR-001) is kept.
- **FR-010** *(added with the FR-007 narrowing)*: The text extraction prompt MUST tell the model that source citations such as `[[note-id]]` or "(from [[note-id]])" identify other notes and are not entities to extract. The prompt is the primary fix; the FR-003/FR-007 filter is the guarantee. The prompt change invalidates the committed v2 cassette keys (re-extraction is a maintainer step).
- **FR-008**: A unit test MUST cover: a chunk whose header carries a note ID yields no entity of that name.
- **FR-009**: The change MUST apply under ontology extraction; it MUST NOT change behaviour for chunks without a source identifier.

### Key Entities *(if applicable)*

- **Note-ID entity**: An extracted entity whose name equals the source ID of the episode it was extracted from; the defect being removed.
- **Asserted Document/Source entity**: The entity Liminis creates deterministically to represent the source note; the sanctioned way to have the note in the graph.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: On the v2 corpus under ontology v3.1, the count of note-ID entities falls from 13 to 0.
- **SC-002**: Link recall on that corpus, as defined by #819, is not lower than the pre-change baseline.
- **SC-003**: Typing accuracy on that corpus, as defined by #819, is not lower than the pre-change baseline.
- **SC-004**: A unit test demonstrating that a chunk with a note ID in its header produces no entity of that name passes in CI.
- **SC-005**: Note-ID entities no longer appear in entity search or graph-walk results for the v2 corpus under ontology v3.1.

## Assumptions

- #819 lands first and provides the metrics (link recall, typing) and the 13-vs-2 baseline; this issue is blocked on it.
- "The note's own source ID" is available at extraction time (e.g., via the chunk header or episode source metadata), so it can be compared with extracted entity names.
- Dropping the offending entity and its edges is acceptable because those edges, per the problem report, "belong to the real entities" and are better attached there; any re-attachment of edges to real entities is not required.
- Both a prompt-level change and a post-extraction filter are permitted; Research decides the exact mix, subject to FR-003.
- Existing graphs are not modified by this change.

## Out of Scope *(optional)*

- Merging or deleting existing junk nodes in stored graphs; `knowledge_merge_entities` already covers that manually.
- Redesigning the ontology's `Document` entity type, beyond whatever minimal change Research finds necessary.
- Changes to the asserted Document/Source entity behaviour.
- Improvements to free-form extraction beyond not regressing.

## Source References *(optional)*

- Issue #819 (baseline and metrics); experiment E13 in the retrieval experiments notes (`docs/retrieval-experiments.md`, introduced with #819).
- `docs/ontology.md`, `docs/extraction-quality-evaluation.md`
