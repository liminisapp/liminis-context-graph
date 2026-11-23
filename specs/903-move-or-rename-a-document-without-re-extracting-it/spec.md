# Feature Specification: Move or rename a document without re-extracting it

**Feature Branch**: `fabrik/issue-903`
**Created**: 2026-11-23
**Status**: Specified
**Input**: User description: "## Problem

A consumer that renames or moves a note today must remove the old document and ingest it again under the new path. Its facts are extracted a second time, at full LLM cost, even though not a word changed. For a notebook app this is routine: people rename notes and reorganise folders. A real workspace reorganisation that moves about 900 meeting documents would re-extract all of them.

What identifies a document's episodes today includes its path: `source_description`, the consumer's `doc_id` (Liminis Notes uses the workspace-relative path), and chunk ids that embed the path (`<file>::<heading-path>::<n>`).

## Ask

A write method, for example `knowledge_move_document`, that re-keys a document's live episodes from one identity to another **without re-extraction**:

- Input: group, old `doc_id` or source path, new `doc_id` or source path, and optionally a chunk-id prefix rewrite (`old/path.md::` → `new/path.md::`).
- It rewrites `doc_id`, `source_description` and the matching chunk ids on the document's episodes. Entities, relationships, embeddings and belief history stay as they are.
- It's logged in the WAL as a fixed-valued, uuid-keyed update, so replay and dump are identical (in the same spirit as `knowledge_stamp_origin`, ADR-0752 Decision 8).
- Origin is unchanged by a move. When the destination prefix implies a different origin, the consumer calls `knowledge_stamp_origin` afterwards. Say in the docs whether a move into a `doc_id` that already holds episodes is refused (it should be: that's a merge, not a move).
- Batch form: many moves in one call, all applied or none, so a folder rename is a single operation.
- `knowledge_status` and `knowledge_lint` reflect the new paths right away. `stale_derived_page` and the citations report the new path.

## Acceptance

- A test ingests a document, moves it, and shows: no extractor calls; `knowledge_search_passages` and the find methods cite the new path; `knowledge_delete_by_source` on the old path deletes nothing; rebuilding from the WAL gives an identical graph.
- Refusals are covered: an unknown source, a destination that already holds episodes, and a cross-group move.
- The method is documented in `docs/ipc-mcp-reference.md` and has a `features` string so consumers can detect it.

## Consumer follow-up (not in this issue)

shadoworg/liminis-notes would detect renames (a watcher unlink and add with the same content hash, or the app's own rename and move actions) and call this instead of removing and re-ingesting."

## Background

A notebook consumer such as Liminis Notes renames notes and reorganises folders routinely. Today the only way to express a rename is to remove the old document and ingest it again under the new path. That repeats LLM extraction on text that has not changed, at full cost. A real reorganisation that moves about 900 meeting documents would re-extract all of them.

The cost comes from what identifies a document's episodes today. It includes the path in three places: the episode's `source_description`, the consumer-supplied `doc_id` (Liminis Notes uses the workspace-relative path), and chunk ids that embed the path (`<file>::<heading-path>::<n>`). Everything derived from the content (entities, relationships, embeddings and belief history) is still correct after a rename. Only the document's identity needs to change.

The graph already has a precedent for a metadata-only, no-LLM, WAL-safe rewrite of existing episodes: `knowledge_stamp_origin` (ADR-0752 Decision 8). It writes a fixed value per episode uuid, so replay, overlapping tail replay and dump all give the same result. This feature applies the same approach to document identity.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Rename or move one document without re-extraction (Priority: P1)

A consumer renames a note from `meetings/2026-03-standup.md` to `archive/2026-03-standup.md`. It asks the service to re-key that document's live episodes from the old identity to the new one. No extraction, LLM or embedding call happens. Afterwards every read surface shows the document under its new path, and nothing is left under the old one.

**Why this priority**: This is the whole value of the feature. Without it the cost problem is unsolved. It is also a complete slice on its own, since single moves work even without a batch form.

**Independent Test**: Ingest a document with a stub extractor that counts calls. Move it. Assert the extractor call count did not change, search and find results cite the new path, and the entity and relationship counts and embeddings are unchanged.

**Acceptance Scenarios**:

1. **Given** an ingested document at path A, **When** it is moved to path B, **Then** its live episodes carry the new `doc_id`, `source_description` and (when a chunk-id prefix rewrite is given) the rewritten chunk ids, and no extractor or embedder call is made.
2. **Given** a moved document, **When** `knowledge_search_passages` and the find methods return its content, **Then** they cite the new path and never the old one.
3. **Given** a moved document, **When** `knowledge_delete_by_source` is called with the old path, **Then** it deletes nothing, and calling it with the new path deletes the document.
4. **Given** a moved document, **When** the graph is rebuilt from the WAL, **Then** the result is identical to the live graph, and the dump is identical too.
5. **Given** a moved document, **When** the caller reads entities, relationships and belief history, **Then** they are unchanged and still link back to the document's episodes.

---

### User Story 2 - Apply many moves atomically (Priority: P1)

A user renames a folder holding hundreds of notes. The consumer sends all the moves in one call. Either every move is applied or none is, so the graph is never left with half a folder at the old path and half at the new one.

**Why this priority**: A folder rename is the motivating case (about 900 documents). A partial application is hard to recover from, and a call per document multiplies the failure modes.

**Independent Test**: Submit a batch of N moves where one item is invalid, and assert that no episode changed. Then submit a valid batch and assert all N moved, with one WAL entry covering them.

**Acceptance Scenarios**:

1. **Given** a batch of valid moves, **When** it is submitted, **Then** every document is re-keyed and the reply reports per-document counts.
2. **Given** a batch in which any single move is refused (unknown source, occupied destination, a conflict inside the batch), **When** it is submitted, **Then** nothing is changed, nothing is written to the WAL, and the reply names the offending move or moves.
3. **Given** a batch applied and then replayed from the WAL, **When** the graph is rebuilt, **Then** it is identical to the live graph.

---

### User Story 3 - Refusals that protect the graph (Priority: P1)

A move must never silently merge documents, cross groups, or act on a document that does not exist. The service refuses these cases with a clear error and changes nothing.

**Why this priority**: A wrong move corrupts document identity, and a later `knowledge_delete_by_source` or supersession would then hit the wrong content. The issue lists these refusals as acceptance.

**Independent Test**: Attempt a move for an unknown source, a destination that already holds live episodes, and a cross-group move. Assert each fails with a distinct error and the graph is byte-for-byte unchanged.

**Acceptance Scenarios**:

1. **Given** an old identity that matches no live episodes in the group, **When** a move is requested, **Then** it is refused as an unknown source.
2. **Given** a destination `doc_id` or source path that already holds live episodes, **When** a move is requested, **Then** it is refused, because that is a merge and not a move.
3. **Given** a request whose old identity lives in a different group from the supplied `group_id`, **When** a move is requested, **Then** it is refused, and no episode in either group changes.
4. **Given** a missing `group_id`, **When** a move is requested, **Then** it is an error, as for the other group-scoped write methods.

---

### User Story 4 - Derived outputs report the new path immediately (Priority: P2)

After a move, the status, lint, derived-page staleness and citation outputs all show the new path without waiting for a rebuild or a re-ingest.

**Why this priority**: The move is only trustworthy if the consumer's view is consistent straight away. It is lower than the core move because it follows from the core move as long as those surfaces read the re-keyed fields.

**Independent Test**: Ingest a primary document and a derived page that cites it. Move the primary document. Assert `knowledge_status`, `knowledge_lint` (including `stale_derived_page`) and citations name the new path.

**Acceptance Scenarios**:

1. **Given** a moved document, **When** `knowledge_status` is called, **Then** counts are unchanged and any per-source information lists the new path.
2. **Given** a derived page that cites a moved document, **When** `knowledge_lint` runs, **Then** `stale_derived_page` and any other finding name the new path.
3. **Given** a moved document, **When** citations are produced, **Then** they carry the new path.

---

### User Story 5 - Consumers can detect and learn the method (Priority: P2)

A consumer can detect support for the move method from the capabilities reply and find its behaviour documented, including the merge-refusal rule.

**Why this priority**: The consumer follow-up (Liminis Notes) must feature-detect before calling it, so that it can fall back to remove-and-re-ingest against older services.

**Independent Test**: Call `knowledge_capabilities` and assert the new `features` string is present. Check the reference doc has an entry for the method.

**Acceptance Scenarios**:

1. **Given** a service that supports the method, **When** `knowledge_capabilities` is called, **Then** `features` includes a string for it, in sorted order with no duplicates.
2. **Given** the reference doc, **When** a consumer looks up the method, **Then** it finds its parameters, reply shape, refusals, scope and group requirement, and the rule on moving into a document that already holds episodes.

---

### Edge Cases

- **Origin is unchanged by a move.** If the new location implies a different origin, the consumer calls `knowledge_stamp_origin` afterwards. The move does not re-evaluate origin rules.
- **Live episodes identify and occupy; superseded history moves with the document.** A document is identified by its live episodes, and a destination counts as "holding episodes" only when it holds live ones. Superseded revisions of the moved document are re-keyed with it (otherwise `knowledge_delete_by_source` on the old path would still find them and lint citations would still name the old `doc_id`). Deleted episodes do not exist and are not touched.
- **Leftover document state at the destination.** A destination `doc_id` that holds no live episode but still has document revision rows (a hard delete keeps them) or a supersession record is refused (`destination_has_history`), because moving into it would merge two revision histories.
- **Chunk-id prefix rewrite is optional.** When it is given, only chunk ids that start with the old prefix are rewritten. If a rewritten chunk id would collide with a live chunk id elsewhere, the move is refused.
- **A document with no `doc_id`.** A document ingested without a `doc_id` is identified by its source path, so it can still be moved.
- **Identical old and new identity.** This is refused as a no-op or invalid request, and nothing is written.
- **Conflicts inside one batch.** Two moves naming the same source, two moves naming the same destination, and a destination that is another move's source in the same batch are all refused. The whole batch is checked against the state before the batch, so a swap or a chain is not supported.
- **Repeating a move.** Re-sending a move that was already applied is refused as an unknown source (the old identity no longer exists). The WAL entry itself is fixed-valued, so replay and overlapping tail replay do not change the result.
- **Supersession and belief history.** Existing supersession and invalidation links are not changed. They continue to resolve after the move.
- **Empty batch.** An empty list of moves is refused as invalid input, or accepted as a no-op that writes nothing. It is never a partial write.
- **Concurrent ingest into the destination.** A destination that is claimed by an ingest between validation and application must not end in a silent merge. The check and the apply are atomic with respect to other writes.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The service MUST provide a write method, `knowledge_move_document`, that re-keys the live episodes of a document from an old identity to a new identity within one group.
- **FR-002**: A move MUST rewrite the `doc_id` and `source_description` of every live episode of the document, so they reflect the new identity.
- **FR-003**: When a chunk-id prefix rewrite (old prefix, new prefix) is supplied, a move MUST rewrite the chunk ids of the document's episodes that start with the old prefix. When none is supplied, chunk ids MUST be left unchanged.
- **FR-004**: A move MUST NOT invoke the extractor, any LLM, or the embedder.
- **FR-005**: A move MUST leave entities, relationships, embeddings and belief history (supersession, invalidation, corroboration) unchanged. Their links to the document's episodes MUST still resolve.
- **FR-006**: A move MUST NOT change episode origin.
- **FR-007**: A document MUST be addressable by its `doc_id` when it has one, and otherwise by its source path.
- **FR-008**: A move MUST be refused, and nothing changed, when the old identity matches no live episodes in the group (unknown source).
- **FR-009**: A move MUST be refused, and nothing changed, when the new identity already holds live episodes. This is a merge, not a move, and the reference doc MUST say so.
- **FR-010**: A move MUST be refused, and nothing changed, when the old identity belongs to a different group than the request's `group_id`. `group_id` MUST be required, and a missing one is an error.
- **FR-011**: A move MUST be refused when a chunk-id rewrite would collide with an existing live chunk id.
- **FR-012**: A move MUST be refused when the old and new identities are identical.
- **FR-013**: The method MUST accept a batch of moves in one call and apply them all-or-nothing. If any item is refused, no item is applied and no WAL entry is written, and the reply identifies each refused item and its reason.
- **FR-014**: A batch MUST be validated against the state before the batch. Duplicate sources, duplicate destinations, and a destination that is also another item's source MUST be refused.
- **FR-015**: A move MUST be recorded in the WAL as a fixed-valued, episode-uuid-keyed update, in the manner of `knowledge_stamp_origin`, so that replay, overlapping tail replay, dump and rebuild-from-WAL all produce an identical graph.
- **FR-016**: After a move, `knowledge_status`, `knowledge_lint` (including `stale_derived_page`), `knowledge_search_passages`, the find methods and citations MUST report the new path and none of them the old one, with no rebuild or re-ingest needed.
- **FR-017**: After a move, `knowledge_delete_by_source` on the old path MUST delete nothing, and on the new path MUST delete the document's episodes.
- **FR-018**: The reply MUST report, per move, the old and new identity and the number of episodes re-keyed. A repeat of an applied move MUST be refused (FR-008), not silently succeed.
- **FR-019**: The method MUST support a dry run, as `knowledge_stamp_origin` does, that validates and reports what would change without writing to the graph or the WAL.
- **FR-020**: The method MUST be added to the MCP-over-stdio tool surface with a scope bucket and to the scope tables in the reference doc.
- **FR-021**: `knowledge_capabilities` MUST list a new `features` string for the method, following the existing rules (sorted, unique, `lower_snake_case`, append-only).
- **FR-022**: The method MUST be documented in `docs/ipc-mcp-reference.md`: parameters, batch form, reply shape, refusals, scope and group requirement, the unchanged-origin rule with the `knowledge_stamp_origin` follow-up, and the merge-refusal rule.
- **FR-023**: Tests MUST cover the following. A move makes no extractor calls. Search and find results cite the new path. `knowledge_delete_by_source` on the old path deletes nothing. A WAL rebuild gives an identical graph. The refusals are an unknown source, an occupied destination, and a cross-group move. Atomic batch behaviour is also covered.

### Key Entities

- **Document**: The set of live episodes sharing one identity, which is a `doc_id` or, absent that, a source path. It is the unit that a move re-keys.
- **Episode**: The stored record of an ingested chunk. A move rewrites its `doc_id`, `source_description` and chunk id, and leaves its uuid, content, origin and links unchanged.
- **Move**: One old-identity to new-identity re-key, with an optional chunk-id prefix rewrite. A batch is a list of moves applied as one unit.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Moving a document makes zero extractor, LLM and embedder calls, so the cost of a rename is independent of document size and goes from the full cost of re-extraction to nothing.
- **SC-002**: A folder rename covering about 900 documents completes as one call, with all documents re-keyed or none.
- **SC-003**: After a move, 100% of search, find, status, lint and citation outputs for the document name the new path, and none name the old path.
- **SC-004**: A graph rebuilt from the WAL after one or more moves is identical to the live graph, and WAL dumps are identical.
- **SC-005**: Each of the three required refusals (unknown source, occupied destination, cross-group) leaves the graph unchanged and is covered by an automated test.
- **SC-006**: A consumer can detect the capability from `knowledge_capabilities` and find the method documented, without reading the source.

## Assumptions

- The method name is `knowledge_move_document`, as the issue suggests.
- The method is a `write`-scope method, not `admin`. A rename is a routine consumer operation, unlike `knowledge_stamp_origin`, which is a one-off adoption tool. Planning can revisit this.
- A "batch" is a list of move items in one request, and a single move is the same request with one item.
- When no chunk-id prefix rewrite is supplied, chunk ids are left unchanged. The consumer decides whether chunk ids embed the path, and supplies the rewrite if they do.
- Live episodes identify a document and occupy a destination. Superseded revisions of the moved document are re-keyed with it, and a destination with leftover revision or supersession state is refused.
- No upper bound on batch size is specified. A folder rename of about 900 documents must work.
- Dry run follows the shape that `knowledge_stamp_origin` uses.
- The destination must be free of live episodes before the batch. Chains and swaps within a batch are not supported, and a consumer that needs them issues separate calls.

## Out of Scope *(optional)*

- The consumer side in shadoworg/liminis-notes: detecting renames (a watcher unlink and add with the same content hash, or the app's own rename and move actions) and calling this method.
- Merging two documents (moving into a `doc_id` that already holds episodes).
- Moving documents across groups.
- Changing the content, chunking or extraction of a document. A move is for unchanged content only.
- Re-evaluating origin for the new location. The consumer calls `knowledge_stamp_origin` itself.
- Changing the ID, content or text of an episode, or the derived entity data.

## Source References *(optional)*

- ADR-0752 (episode origin and corroboration), Decision 8: the fixed-valued, uuid-keyed update pattern used by `knowledge_stamp_origin`.
- `docs/ipc-mcp-reference.md`: `knowledge_stamp_origin`, `knowledge_capabilities` `features` rules, and the scope tables.
