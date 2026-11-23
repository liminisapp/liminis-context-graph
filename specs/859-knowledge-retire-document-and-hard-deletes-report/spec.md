# Feature Specification: `knowledge_retire_document`, and hard deletes report (optionally prune) orphaned entities

**Feature Branch**: `fabrik/issue-859`
**Created**: 2026-11-23
**Status**: Specified
**Input**: User description: "Implements #838. ## Problem - **No single call retires a whole document.** `knowledge_process_document` with empty content is refused (`crates/core/src/document/ingest.rs` ~83–91). Per-chunk soft retirement is `knowledge_mark_chunk_removed`, and document replacement is `knowledge_supersede_document` (#707), which needs a successor. - **Hard deletes leave orphans.** `knowledge_delete_by_source` and `knowledge_delete_chunk_episode` leave entities with no remaining mentions, as their own docs admit (`handlers.rs` ~3198–3203). Those entities still come back from `knowledge_find_entities`. ## Change - **Add `knowledge_retire_document(doc_id, group_id, effective_at?)`.** It supersedes every live episode of the document with reason `document_removed`. Nothing is deleted, so the history stays readable, as with `chunk_removed`. It's idempotent, so retiring an already-retired document is a no-op that reports zero episodes. It's a write method, so it goes through `--writable-groups`, the WAL and replay. - **Report orphans from hard deletes.** `knowledge_delete_by_source` and `knowledge_delete_chunk_episode` return `orphaned_entity_uuids` (and a count), meaning entities left with no live mentioning episode. Don't delete them by default. A `prune_orphans: true` flag deletes them, together with any edges left with no episodes. Record the choice in the ADR, along with how it relates to #361's group purge and #828's retention research. - Add `retire_document` to the features list from #854. ## Acceptance - Ingest a two-chunk document and retire it. Belief reads no longer return its facts, `get_episodes` still lists the episodes as superseded with reason `document_removed`, and a rebuild from the WAL reproduces this. - A hard delete that leaves an entity unmentioned reports it, and with `prune_orphans` the entity is gone from `find_entities`. - The MCP tool count rises by one, and the docs, CHANGELOG and `llms-full.txt` are updated."

## Background

A client that tracks documents (e.g. a note or file that was deleted at the source) currently has no single way to say "this document is gone". Empty-content ingest is refused; `knowledge_mark_chunk_removed` works one chunk at a time; `knowledge_supersede_document` (#707) requires a replacement document that does not exist for a plain removal. The only whole-document option is the hard delete `knowledge_delete_by_source`, which destroys history and so is the wrong tool when the intent is "this is no longer believed, but keep the record".

Separately, the hard deletes (`knowledge_delete_by_source`, `knowledge_delete_chunk_episode`) are documented as leaving entities that no longer have any mentioning episode. Those entities still surface from `knowledge_find_entities`, so deleted content keeps appearing in search. Callers currently cannot even tell which entities were stranded.

This issue adds the missing soft whole-document retirement, makes orphaning visible from hard deletes, and offers an explicit opt-in to prune the orphans. It is deliberately conservative: nothing is deleted unless the caller asks. The relationship to the group purge (#361) and the retention research (#828) is recorded in an ADR. It builds on the capabilities list added in #854 (#858 is a prerequisite).

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Retire a whole document without deleting it (Priority: P1)

A client whose source document was removed calls `knowledge_retire_document` with the document's `doc_id` and group. All of that document's live episodes stop being believed, while the episodes and their history remain readable.

**Why this priority**: This is the headline capability and the only way to soft-remove a whole document without a successor.

**Independent Test**: Ingest a two-chunk document, retire it, then read beliefs and episodes; rebuild from the WAL and repeat.

**Acceptance Scenarios**:

1. **Given** a document with two live chunk episodes, **When** it is retired, **Then** belief-filtered relationship reads no longer return facts supported only by those episodes, and `knowledge_get_episodes` still lists both episodes as superseded with reason `document_removed`.
2. **Given** a retired document, **When** the graph is rebuilt from the WAL, **Then** the same retired state results.
3. **Given** an already-retired document, **When** it is retired again, **Then** the call succeeds, changes nothing and reports zero episodes retired.
4. **Given** a facts also asserted by another, live document, **When** the first document is retired, **Then** that fact remains believed through the other document's episode.
5. **Given** a group not in `--writable-groups`, **When** the call is made, **Then** it is refused exactly like other write methods.

---

### User Story 2 - Hard deletes report the entities they strand (Priority: P2)

A caller hard-deleting by source or deleting a single chunk episode learns which entities were left with no remaining mention, so it can decide what to do.

**Why this priority**: Visibility is the safe, non-destructive part of the change and is required before anyone can responsibly prune.

**Independent Test**: Delete an episode that is the only mention of an entity and check the response lists it while `find_entities` still returns it.

**Acceptance Scenarios**:

1. **Given** an entity mentioned only by the episodes being deleted, **When** `knowledge_delete_by_source` or `knowledge_delete_chunk_episode` runs without `prune_orphans`, **Then** the response includes the entity's UUID in `orphaned_entity_uuids` and the matching count, and the entity is still present.
2. **Given** an entity also mentioned by a remaining episode, **When** the delete runs, **Then** it is not reported.
3. **Given** a delete that strands nothing, **When** it runs, **Then** `orphaned_entity_uuids` is empty and the count is zero.

---

### User Story 3 - Opt in to pruning orphans (Priority: P3)

A caller passes `prune_orphans: true` so the stranded entities, and any edges left with no episodes, are removed in the same operation and no longer appear in `knowledge_find_entities`.

**Why this priority**: Convenience on top of reporting; callers can already prune manually through other means once they have the list.

**Independent Test**: Run the same delete as Story 2 with `prune_orphans: true`; confirm the entity is gone from `find_entities`, and that a WAL rebuild reproduces this.

**Acceptance Scenarios**:

1. **Given** a delete that strands an entity, **When** `prune_orphans: true` is passed, **Then** the entity is removed, `find_entities` no longer returns it, and the response still lists it in `orphaned_entity_uuids` together with pruning counts.
2. **Given** edges left with no supporting episodes, **When** pruning runs, **Then** those edges are removed as well; edges still supported by a remaining episode are kept.
3. **Given** a pruned delete, **When** the graph is rebuilt from the WAL, **Then** the pruned entities and edges stay absent.
4. **Given** `prune_orphans` is omitted or `false`, **When** the delete runs, **Then** no entity or edge beyond the existing behaviour is deleted.

---

### User Story 4 - Discoverable by clients (Priority: P3)

A client or MCP agent can detect and call the new method.

**Why this priority**: Without registration the capability is invisible to MCP clients.

**Independent Test**: Read the capabilities/features list and the MCP tool list.

**Acceptance Scenarios**:

1. **Given** the service, **When** the features list from #854 is read, **Then** it contains `retire_document`.
2. **Given** the MCP-over-stdio surface, **When** tools are listed, **Then** `knowledge_retire_document` is present as a write-scope tool and the tool count is one higher than before.

---

### Edge Cases

- `doc_id` unknown in the group (no episode carries it): rejected with an error and no side effects, consistent with `knowledge_supersede_document`; this is distinct from the idempotent already-retired case.
- A document already superseded by another document (`document_superseded`): its episodes are no longer live, so retirement is a no-op reporting zero episodes and does not overwrite the existing supersession reason.
- Some of a document's episodes were individually corrected or chunk-removed earlier: only the remaining live episodes are retired; earlier supersession reasons are untouched.
- Episodes of the document ingested after retirement are not retroactively retired.
- `effective_at` without a timezone offset is rejected with no side effects; a past or future value is accepted.
- Omitted `group_id` is an error (no all-groups retirement).
- Hard delete where entities are mentioned by superseded (but not deleted) episodes: those entities are not orphans, because a mention still exists and history must stay readable.
- Pruning must never leave an edge whose endpoint entity was pruned.
- A failed or refused call writes nothing to the WAL.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The service MUST provide `knowledge_retire_document` accepting `doc_id`, `group_id` and an optional `effective_at`.
- **FR-002**: It MUST supersede every live episode of that document in that group with supersession reason `document_removed`, deleting nothing.
- **FR-003**: Retired episodes MUST remain listed by `knowledge_get_episodes`, marked superseded with reason `document_removed`.
- **FR-004**: Belief-filtered reads MUST stop returning facts supported only by retired episodes; facts also supported by a live episode MUST remain believed.
- **FR-005**: Retiring a document with no live episodes (already retired, or superseded) MUST succeed as a no-op and report zero episodes retired, writing nothing durable.
- **FR-006**: An unknown `doc_id` for the group, an omitted `group_id`, or an `effective_at` lacking a timezone offset MUST be rejected with no side effects.
- **FR-007**: `effective_at`, when supplied, MUST follow the same semantics as in `knowledge_supersede_document` (RFC 3339 with offset; past or future allowed; a future date keeps the fact believed and in force until then); when omitted, retirement takes effect at recording time.
- **FR-008**: The call MUST be a write-scope method subject to `--writable-groups`, MUST be recorded in the WAL, and a rebuild from the WAL and checkpoint restore MUST reproduce the retired state.
- **FR-009**: The response MUST report the document, group, the number of episodes retired, and their UUIDs.
- **FR-010**: `knowledge_delete_by_source` and `knowledge_delete_chunk_episode` MUST return `orphaned_entity_uuids` and a count of them, listing entities that, as a result of that delete, have no remaining mentioning episode.
- **FR-011**: Without `prune_orphans`, neither delete method MUST delete any entity or edge beyond what it deletes today.
- **FR-012**: With `prune_orphans: true`, the orphaned entities MUST be deleted, along with any edges left with no supporting episodes, in the same operation; the response MUST report the number of entities and edges pruned.
- **FR-013**: Pruning MUST be reproduced by WAL replay so a rebuild does not resurrect pruned entities or edges.
- **FR-014**: Both delete methods MUST continue to honour their existing group-scoping and `--writable-groups` behaviour; `prune_orphans` MUST NOT widen the set of groups affected.
- **FR-015**: `retire_document` MUST be added to the features list introduced by #854.
- **FR-016**: `knowledge_retire_document` MUST be registered as a write-scope MCP tool, with the registry's tool and scope counts updated (total up by one).
- **FR-017**: The IPC/MCP reference, README where relevant, CHANGELOG and `llms-full.txt` MUST document the new method, the new response fields and the `prune_orphans` flag, and the stale "orphans remain" notes on the delete methods and the "to retire a whole document" guidance MUST be updated.
- **FR-018**: An ADR MUST record the decision to report by default and prune only on opt-in, and state how this relates to #361's group purge and #828's retention research.

### Key Entities

- **Document**: A set of episodes sharing a `doc_id` within a group; the unit retired.
- **Orphaned entity**: An entity touched by a hard delete that, after the delete, is mentioned by no remaining episode.
- **Supersession reason `document_removed`**: A new reason alongside `revision`, `chunk_removed` and `document_superseded`, meaning "the document was removed; there is no replacement".

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: After retiring a two-chunk document, 100% of facts supported solely by it are absent from belief-filtered reads, and both episodes remain visible as superseded with reason `document_removed`.
- **SC-002**: A rebuild from the WAL after retirement yields identical belief and episode reads to the pre-rebuild graph.
- **SC-003**: Retiring the same document twice reports zero episodes the second time and adds no durable record.
- **SC-004**: A hard delete that strands an entity reports its UUID in 100% of cases; with `prune_orphans` that entity is absent from `knowledge_find_entities`, and without it nothing extra is deleted.
- **SC-005**: The MCP tool count is exactly one higher, `retire_document` appears in the features list, and the docs, CHANGELOG and `llms-full.txt` describe the change.

## Assumptions

- `group_id` is a single required string, matching `knowledge_supersede_document`, rather than the `group_ids` array used by `knowledge_mark_chunk_removed`; the issue specifies `group_id`.
- "Orphaned" means no remaining mention from any existing episode, including superseded ones. An entity whose only mentions are superseded episodes is not orphaned, so pruning never erases readable history. Only entities that lost a mention in the current delete are considered; pre-existing orphans elsewhere in the graph are not swept.
- `prune_orphans` is optional and defaults to `false`.
- Response field names beyond `orphaned_entity_uuids` and its count (e.g. pruned entity/edge counts) follow existing response naming conventions and are settled in planning.
- Retiring is not undone by later ingests; re-ingesting the document under the same `doc_id` creates new live episodes as usual.
- The ADR is numbered by this issue per the repository's ADR convention.

## Out of Scope

- A group-wide or global orphan sweep / garbage-collection method (relates to #828 retention research).
- Changing the group purge `knowledge_delete_by_group` (#361) beyond documenting its relationship.
- Un-retiring a document.
- Making `knowledge_process_document` accept empty content as retirement.
- Cascading retirement of documents that reference one another.

## Source References

- #838 (parent request), #854 (features list), #858 (prerequisite), #707 (`knowledge_supersede_document`), #361 (group purge), #828 (retention research)
- `docs/ipc-mcp-reference.md` sections on supersession, `knowledge_mark_chunk_removed` and `knowledge_process_document` limits
