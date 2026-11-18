# Feature Specification: Document mode — `knowledge_process_document` with a completeness claim, server-side chunking/diff and document context

**Feature Branch**: `fabrik/issue-785`
**Created**: 2026-11-18
**Status**: Specified
**Input**: User description: "Implements the community request #733 (document-aware extraction and reconciliation), as its first slice. This is the "document mode" in `ideas/ingestion-surface-consolidation.md` (§1, §5, §7, §8) and `ideas/document-aware-fact-extraction.md`. Both are on `main`, and they are one track.

## Problem

lcg ingests only in chunk mode: the client chunks a document, and every `knowledge_process_chunk` call is extracted on its own. For the primary consumer, Liminis Notes, every note is a document, and chunk mode costs it in four ways:

- **No completeness claim, so no retraction by absence.** The app sends only the chunks that changed. A removal the client never sends leaves the old chunk believed forever. A heading rename re-addresses a chunk: about 9.8% of 315 measured revision pairs orphan at least one `chunk_id`. The server can't tell "unchanged" from "deleted" from "renamed" (consolidation doc §5).
- **Context blindness.** The extractor sees one chunk at a time. The heading path is buried inside `chunk_id` and never reaches it (liminis #689), so it can't see the document's title, purpose or origin either. #733's example is the result: a note proposes X, rejects Y and chooses Z, and Y gets promoted to a current fact.
- **Wasted extraction.** Re-indexing an edited note re-extracts every chunk the client resends, because there is no unchanged-text no-op (the surviving half of #284; it also covers #288).
- **Two chunkers.** The app's `canonical-chunker.ts` is a workaround for a server capability that's missing (§7).

## Desired behaviour (slice 1: structural document mode)

1. **`knowledge_process_document`** (MCP + IPC). It accepts one whole document plus metadata: `doc_id` (required), `group_id`, `origin` (ADR-0752; one origin per document), title / source path, optional `effective_at`, and free `attributes`. An ingest is a **completeness claim**: "this is document D, in full, at time T".
2. **A server-minted revision identity** is recorded on every episode the call produces, so a reader can tell that an episode was observed under a completeness claim. Under chunk mode it is absent, and a caller-supplied `doc_id` does not imply it (§5). It is in the WAL and survives `knowledge_rebuild_from_wal`.
3. **An internal markdown-aware chunker in Rust.** It splits at structural boundaries: headings, paragraphs, list groups, tables with their headers. Every chunk keeps its heading path and its byte offsets into the document revision. It does not have to match the app's chunker exactly. Re-indexing is additive under ADR-0705, so a changed boundary is a new revision, and churn on cut-over is visible through `knowledge_get_correction_changes`. Add a read-only `knowledge_preview_chunks` so a client never needs its own chunker.
4. **The server computes the diff.** Against D's previous revision, it classifies each chunk as added, changed, removed or unchanged. It then applies the existing ADR-0705 operations itself: supersede by resubmission for changed chunks, `chunk_removed` for removed ones. **Unchanged chunks are skipped by content hash before extraction.** The matching strategy is the server's to choose, and it should be justified in Plan. Heading-as-metadata or paragraph-level matching are the candidates; the aim is that a heading rename doesn't re-extract the whole section.
5. **Document context reaches the extractor.** Each chunk extraction receives the title, heading path, origin and a short document map (headings plus section order), clearly marked as context rather than as source text. Facts are still cited only to the chunk they came from.
6. **Provenance spans.** Each episode records its `(revision, byte_start, byte_end)` inside the document, so a client can go from a fact to the passage it came from.
7. **Atomicity.** A failed document ingest retires nothing. Earlier beliefs stay as they are until the new revision has committed. Plan must say where the commit boundary is, either as a document-level WAL event or as composed episode operations, and show crash and replay behaviour.
8. **Cap and splitting.** There is a document-size cap (#293's cap moved to document level). Server-side splitting of oversized sections uses ADR-0705's split-unit protocol (`supersede: false` / `same_revision_episode_uuids`), which replaces the closed #284 / #282.
9. **Chunk mode is unchanged.** `knowledge_process_chunk` keeps its current contract for streaming and conversational input.

## Out of scope for this slice (slice 2, after measurement)

#733's **global interpretation pass** and **document-scoped reconciliation**: candidate assertions with modality (proposed / rejected / accepted), merged across sections before publication. The ideas doc requires these to be benchmarked first, comparing chunk-only, whole-document and hybrid extraction on 20–30 documents. File the eval as a follow-up once slice 1 provides the document substrate.

## Acceptance scenarios

- A note with 5 sections is ingested, then re-ingested unchanged. The second call makes **zero** extraction calls and reports all 5 chunks unchanged.
- One paragraph is edited. Only that chunk is re-extracted, and its old episode is superseded with reason `revision`.
- A section is deleted from the note, and the client sends only the new full document. That section's episodes end belief with reason `chunk_removed`, and their facts lose that support.
- A heading is renamed and nothing else changes. Plan's matching strategy determines how much is re-extracted, and the result is reported.
- Ingest fails midway (an extractor error). D's previous revision is still fully believed, and nothing is partially superseded.
- WAL rebuild reproduces the revision identities, spans and belief state.
- `origin: derived` on the document applies to every episode, and the existing `origin_conflict` rule (ADR-0752) applies per `doc_id`.
- A chunk-mode client sees no behaviour change.

## Related

- Community request: #733. Ideas: `ideas/ingestion-surface-consolidation.md`, `ideas/document-aware-fact-extraction.md`.
- Builds on ADR-0705 / 0706 / 0707 (belief time, correction changes, `doc_id` supersession), ADR-0752 (origin) and #291 (indexed `doc_id` lookup, shipped in 0.19.0).
- Unblocks #732: pages tracked as views over their evidence need stable document revisions to point at. It also unblocks app-side retirement of `canonical-chunker.ts` (shadoworg/liminis-notes, a follow-up).
- #596 ("lcg does it all"): the Rust markdown chunker is shared with that epic."

## Background

lcg today ingests only in chunk mode: the client owns chunking and every `knowledge_process_chunk` call is extracted in isolation. Since the Temporal work (ADR-0705/0706/0707), chunk mode can already *revise* (resubmitting a `chunk_id` supersedes earlier episodes) and *remove* (`knowledge_mark_chunk_removed`), but only on explicit instruction from the client. What chunk mode cannot do is **retract by absence**, because the server never knows a document's full extent. Document mode adds that missing piece: a **completeness claim** ("this is document D, in full, at time T"), which is the precondition for the server to compute added/changed/removed/unchanged chunks itself and retire what a revision no longer asserts.

The primary consumer, Liminis Notes, treats every note as a document and pays four costs under chunk mode:

- **No retraction by absence.** Only changed chunks are sent, so removals the client never sends stay believed forever. About 9.8% of 315 measured revision pairs orphan at least one `chunk_id` (driven by heading renames), and the server cannot tell unchanged from deleted from renamed.
- **Context blindness.** The extractor sees one chunk at a time; the heading path is buried in `chunk_id` and never reaches it, nor do the document's title, purpose or origin. Result (#733's example): a note that proposes X, rejects Y and chooses Z yields Y as a current fact.
- **Wasted extraction.** Re-indexing re-extracts every chunk the client resends; there is no unchanged-text no-op (the surviving half of #284, also covering #288).
- **Two chunkers.** The app's `canonical-chunker.ts` is a workaround for a missing server capability.

This is **slice 1** of #733: *structural* document mode (chunking, diffing, document context, provenance, atomic revision). The global interpretation pass and document-scoped reconciliation (candidate assertions with modality) are slice 2, deferred until a benchmark on 20–30 documents compares chunk-only, whole-document and hybrid extraction.

Related design material on `main`: `ideas/ingestion-surface-consolidation.md` (§1, §5, §7, §8) and `ideas/document-aware-fact-extraction.md`; ADR-0705, 0706, 0707, 0752; #291 (indexed `doc_id` lookup, shipped in 0.19.0). Unblocks #732 and the app-side retirement of `canonical-chunker.ts`. The Rust markdown chunker is shared with #596.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Ingest a whole document and re-ingest it for free (Priority: P1)

A client (Liminis Notes, an agent, a CLI) submits one whole document with metadata through `knowledge_process_document`. The server chunks it, extracts each chunk with document context, and records the episodes under a server-minted revision identity. Re-submitting identical content makes no extraction calls.

**Why this priority**: This is the core capability and the foundation everything else (diff, retraction, provenance) builds on. It alone delivers a usable document-mode ingest and removes the wasted-extraction cost.

**Independent Test**: Ingest a 5-section note, then re-ingest it unchanged; verify the first call produces episodes carrying a revision identity and spans, and the second call performs zero extraction calls and reports 5 chunks unchanged.

**Acceptance Scenarios**:

1. **Given** an empty group and a 5-section note, **When** `knowledge_process_document` is called with `doc_id`, `group_id`, `origin`, title and the note body, **Then** episodes are created for each chunk, each carrying the document's `doc_id`, `origin`, a server-minted revision identity, and `(byte_start, byte_end)` offsets into that revision.
2. **Given** the note was ingested, **When** it is submitted again byte-for-byte unchanged, **Then** zero extraction calls are made, all 5 chunks are reported unchanged, and no existing episode is superseded.
3. **Given** a document call that omits `doc_id`, **When** the call is made, **Then** it is rejected with a clear error and nothing is written.

---

### User Story 2 - Edits and deletions are applied by the server as a diff (Priority: P1)

When a revised document is submitted, the server compares it to D's previous revision, re-extracts only added or changed chunks, supersedes changed chunks' old episodes with reason `revision`, and ends belief in removed chunks' episodes with reason `chunk_removed` — without the client sending any removal.

**Why this priority**: This is the completeness claim paying off: retraction by absence, and the reason document mode exists.

**Independent Test**: Ingest a 5-section note; (a) edit one paragraph and re-ingest; (b) delete a section and re-ingest; check which chunks are extracted and which episodes lose belief and why.

**Acceptance Scenarios**:

1. **Given** an ingested note, **When** one paragraph is edited and the full document re-submitted, **Then** only that chunk is re-extracted and its previous episodes are superseded with reason `revision`; other chunks' episodes are untouched.
2. **Given** an ingested note, **When** a section is deleted and the full document re-submitted, **Then** that section's episodes end belief with reason `chunk_removed`, and facts supported only by them lose support.
3. **Given** an ingested note, **When** only a heading is renamed, **Then** the call reports, per chunk, what was classified as added/changed/removed/unchanged and how many extractions were made, and the amount of re-extraction follows the matching strategy chosen in Plan (the aim being that the section body is not re-extracted solely because its heading changed).
4. **Given** the results of a diff, **When** the response is read, **Then** it lists each chunk's classification and the episodes created, superseded or retired.

---

### User Story 3 - A failed ingest leaves the previous revision intact (Priority: P1)

If a document ingest fails part-way (extractor error, crash, size cap), nothing from the previous revision is retired; earlier beliefs hold until the new revision has fully committed.

**Why this priority**: Without this, retraction by absence would be unsafe — a partial revision could retract facts the document still asserts.

**Independent Test**: Inject an extractor failure on the third of five changed chunks; verify the document's previous revision is fully believed and no episode has been superseded or removed. Then kill the process mid-ingest and replay the WAL.

**Acceptance Scenarios**:

1. **Given** a believed revision of D, **When** a re-ingest fails midway, **Then** every episode of the previous revision remains believed, no supersession or `chunk_removed` is recorded, and the error is returned to the caller.
2. **Given** a crash during ingest, **When** the service restarts or the WAL is replayed, **Then** the graph is in either the fully-before or fully-after state for that document, never in between.
3. **Given** a failed ingest, **When** the same document is re-submitted, **Then** it succeeds and applies the full diff against the last *committed* revision.

---

### User Story 4 - The extractor sees document context (Priority: P2)

Each chunk's extraction receives the document title, the chunk's heading path, the document's origin and a short document map (headings and section order), explicitly marked as context rather than source text. Facts are cited only to the chunk they came from.

**Why this priority**: It addresses the context-blindness problem (#733's proposed/rejected/chosen example) at the level slice 1 can reach, but the substrate (US1–US3) is deliverable without it.

**Independent Test**: Capture the extraction input for a chunk and verify it includes title, heading path, origin and map in a clearly delimited context block, separate from the source text; verify extracted facts reference only the chunk's own episode.

**Acceptance Scenarios**:

1. **Given** a document with nested headings, **When** a chunk under "Decision > Options" is extracted, **Then** the extractor input contains the title, the path "Decision > Options", the origin and the document map, labelled as context.
2. **Given** an extraction, **When** facts are stored, **Then** they are attributed only to the chunk's episode, never to the context block or other chunks.

---

### User Story 5 - Preview chunks and trace a fact to its passage (Priority: P2)

A client can call a read-only `knowledge_preview_chunks` to see exactly the chunks the server would produce (with heading paths, hashes and byte spans) without writing anything, and can go from any episode to `(revision, byte_start, byte_end)` in its document.

**Why this priority**: It lets the app delete its own chunker and show provenance, but ingest works without it.

**Independent Test**: Preview a document, ingest the same document, and verify the previewed chunks match the stored episodes' boundaries, heading paths and spans exactly; verify nothing was written by the preview.

**Acceptance Scenarios**:

1. **Given** a document, **When** `knowledge_preview_chunks` is called, **Then** it returns the same chunk list that `knowledge_process_document` would use, and no graph or WAL state changes.
2. **Given** an episode from document mode, **When** it is read, **Then** its revision identity and byte span are available and slicing the revision's bytes at that span yields the chunk's source text.

---

### User Story 6 - Rebuild, origin and chunk-mode safety (Priority: P2)

Document-mode state is durable and compatible with existing rules: revision identities, spans and belief survive `knowledge_rebuild_from_wal`; the document's `origin` applies to every episode and ADR-0752's `origin_conflict` rule applies per `doc_id`; chunk-mode callers see no change.

**Why this priority**: Required for correctness and compatibility, but each is verification of properties of the core stories.

**Independent Test**: Ingest, revise and delete-section a document, rebuild from the WAL, and compare revision ids, spans and belief state before and after. Run the existing chunk-mode test suite unchanged.

**Acceptance Scenarios**:

1. **Given** a graph with several document revisions, **When** `knowledge_rebuild_from_wal` runs, **Then** revision identities, spans and the believed/superseded state of every episode are identical to before.
2. **Given** `origin: derived` on a document, **When** it is ingested, **Then** every resulting episode has origin `derived`; **When** a later call for the same `doc_id` supplies a conflicting origin, **Then** it is refused under the existing `origin_conflict` rule.
3. **Given** a chunk-mode client calling `knowledge_process_chunk` (with or without a `doc_id`), **When** it ingests, **Then** behaviour, responses and stored shape are unchanged and its episodes carry no revision identity.

---

### Edge Cases

- **Empty or whitespace-only document**: Rejected with a clear error unless it is a valid revision that removes all prior content; must be decided and documented (see Assumptions).
- **Document over the size cap**: Rejected before any extraction or write, with an error stating the cap; nothing is retired.
- **Single section larger than the chunk limit**: Split server-side using ADR-0705's split-unit protocol (`supersede: false` on non-final units, `same_revision_episode_uuids` on the final unit); the split units are one logical chunk for diffing.
- **Duplicate chunk text within a document** (e.g. repeated boilerplate): Diff must treat identical chunks deterministically, by position/occurrence, without collapsing or double-retiring.
- **Reordered sections**: Moving a section without changing its text should not re-extract it.
- **Heading rename only**: Behaviour per the matching strategy chosen in Plan; must be reported.
- **Non-markdown or malformed markdown input** (plain text, unclosed fences, HTML, front matter): Chunked best-effort without error; content is never dropped.
- **Tables and lists**: Tables keep their header with each chunk; list groups are not split mid-item where avoidable.
- **Concurrent ingests of the same `doc_id`**: Serialized; the later committed revision wins, and no two live revisions of D coexist.
- **First document-mode ingest of a `doc_id` already used in chunk mode**: Prior chunk-mode episodes are not left believed forever (see Assumptions).
- **Same content under a different `doc_id`**: Treated as a different document; no cross-document dedup of episodes.
- **Re-ingest after a failed ingest**: Diffs against the last committed revision, not the failed attempt.
- **Origin change on the same `doc_id`**: Governed by the existing `origin_conflict` rule.
- **`effective_at` supplied**: Recorded as the document's effective time; semantics follow existing temporal conventions and never rewrite belief time.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The system MUST provide a `knowledge_process_document` method over both IPC and MCP-over-stdio that accepts one whole document with `doc_id` (required), `group_id`, `origin`, title and/or source path, optional `effective_at`, and free-form `attributes`.
- **FR-002**: A `knowledge_process_document` call MUST be treated as a completeness claim: the submitted content is the whole of document `doc_id` as of that call.
- **FR-003**: The system MUST mint a revision identity per committed document ingest and record it on every episode produced by that ingest. Episodes produced by `knowledge_process_chunk` MUST NOT carry a revision identity, and a caller-supplied `doc_id` alone MUST NOT imply one.
- **FR-004**: The revision identity MUST be persisted in the WAL and MUST be reproduced identically by `knowledge_rebuild_from_wal`.
- **FR-005**: The system MUST chunk documents internally with a markdown-aware chunker that splits at structural boundaries (headings, paragraphs, list groups, tables with their headers) and records for each chunk its heading path and its byte offsets into the document revision.
- **FR-006**: The system MUST provide a read-only `knowledge_preview_chunks` method (IPC + MCP) that returns exactly the chunking `knowledge_process_document` would apply (including heading path, content hash, byte span), and MUST NOT mutate graph or WAL state.
- **FR-007**: Given a document's previous committed revision, the system MUST classify each chunk of the new revision as added, changed or unchanged, and each chunk of the previous revision with no counterpart as removed, using a server-chosen matching strategy that is justified in the Plan stage.
- **FR-008**: Chunks classified unchanged MUST be skipped before extraction, determined by content hash; their existing episodes MUST remain believed and unmodified.
- **FR-009**: For changed chunks, the system MUST extract the new content and supersede the chunk's previous episodes with reason `revision`, using the existing ADR-0705 supersession operation.
- **FR-010**: For removed chunks, the system MUST end belief in their episodes with reason `chunk_removed`, using the existing ADR-0705 operation, without the client sending any removal.
- **FR-011**: The system MUST pass each chunk's extraction a document-context block containing the document title, the chunk's heading path, the document origin and a short document map (headings and section order), clearly delimited as context and distinct from the source text.
- **FR-012**: Facts extracted from a chunk MUST be cited only to that chunk's episode, never to the context block or to other chunks.
- **FR-013**: Each episode produced in document mode MUST record `(revision, byte_start, byte_end)` identifying its passage within the document revision, readable through existing episode reads.
- **FR-014**: A document ingest MUST be atomic with respect to belief: if it fails or is interrupted before the new revision commits, no previous-revision episode may be superseded, removed or otherwise have its belief state altered.
- **FR-015**: Retraction of removed chunks and supersession of changed chunks MUST be applied only once every chunk of the new revision has been extracted and committed; the commit boundary and its crash/replay behaviour MUST be specified in the Plan stage.
- **FR-016**: The system MUST enforce a document-size cap (the document-level successor of #293's cap), rejecting oversized documents before extraction or any write, with an error that states the cap.
- **FR-017**: Sections larger than the per-chunk limit MUST be split server-side using ADR-0705's split-unit protocol (`supersede: false` on non-final units; `same_revision_episode_uuids` on the final unit), and the split units MUST be diffed as one logical chunk.
- **FR-018**: The document's `origin` MUST apply to every episode produced by the ingest, and ADR-0752's `origin_conflict` rule MUST apply per `doc_id`.
- **FR-019**: `knowledge_process_chunk` MUST keep its current contract, responses and stored shape unchanged for chunk-mode callers.
- **FR-020**: The `knowledge_process_document` response MUST report, per chunk, its classification (added/changed/removed/unchanged), the number of extraction calls made, and the episodes created, superseded or retired; progress MUST stream through the existing `_progress_token` mechanism.
- **FR-021**: Re-ingesting a byte-identical document MUST make zero extraction calls and change no belief state.
- **FR-022**: Concurrent `knowledge_process_document` calls for the same `(group_id, doc_id)` MUST be serialized so that at most one revision of the document is live at any time.
- **FR-023**: The first document-mode ingest of a `doc_id` that already has chunk-mode episodes in the group MUST NOT leave those episodes believed indefinitely; the handling MUST be specified in the Plan stage and reported in the call's response.
- **FR-024**: The new methods MUST be registered in the MCP tool surface with appropriate scope buckets (`knowledge_process_document` as `write`, `knowledge_preview_chunks` as `read`), and the tool-registry count tests, IPC parity tests and documentation MUST be updated accordingly.
- **FR-025**: A new ADR, numbered `docs/adr/0785-<slug>.md` per the issue-number rule, MUST record the chunking, matching-strategy, revision-identity and commit-boundary decisions.

### Key Entities *(if applicable)*

- **Document**: Addressed by `(group_id, doc_id)`. Has one origin, a title/source path, optional `effective_at`, free attributes, and a sequence of committed revisions.
- **Document revision**: A server-minted identity for one completeness claim that committed. It is the scope against which the next ingest is diffed and to which provenance spans refer.
- **Chunk (document-mode)**: A structural slice of a revision with heading path, content hash, and byte span; maps to one logical episode set (one or more split units).
- **Episode (document-mode)**: Existing episode record extended with revision identity and `(byte_start, byte_end)`; participates in ADR-0705 supersession (`revision`, `chunk_removed`).
- **Document context block**: The title, heading path, origin and document map supplied alongside, but separate from, a chunk's text at extraction time.
- **Chunk diff**: Per-ingest classification of chunks as added / changed / removed / unchanged.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Re-ingesting an unchanged 5-section document makes 0 extraction calls and reports 5/5 chunks unchanged.
- **SC-002**: Editing one paragraph of a 5-section document and re-ingesting results in exactly 1 chunk extracted, with its prior episode(s) superseded with reason `revision` and all other episodes untouched.
- **SC-003**: Deleting a section and re-ingesting the full document ends belief in 100% of that section's episodes with reason `chunk_removed`, with no client-sent removal.
- **SC-004**: After an injected failure at any point during a document ingest, 100% of the previous revision's episodes remain believed and 0 are superseded or removed; a crash/replay test shows only fully-before or fully-after states.
- **SC-005**: After `knowledge_rebuild_from_wal`, revision identities, byte spans and belief state of every document-mode episode are identical to the pre-rebuild values.
- **SC-006**: For every document-mode episode, slicing the revision's bytes at its recorded span reproduces the chunk's source text exactly.
- **SC-007**: `knowledge_preview_chunks` output matches the chunks stored by an actual ingest of the same document in 100% of tested documents, and a preview leaves graph and WAL state byte-identical.
- **SC-008**: The existing chunk-mode test suite and IPC parity tests pass unchanged, and chunk-mode episodes carry no revision identity.
- **SC-009**: For a heading-rename-only revision, the response reports the number of chunks re-extracted, and that number is measured and recorded in the Plan/ADR for the chosen matching strategy on a set of real revision pairs.
- **SC-010**: Every extraction call in document mode carries the document title, heading path, origin and document map, verifiable by inspecting the extractor input for a sample document.
- **SC-011**: A document above the size cap is rejected with 0 extraction calls and 0 graph writes.

## Assumptions

- **Atomic commit is at the document level.** Plan decides whether it is a document-level WAL event or composed episode operations, but retirements (supersede/`chunk_removed`) happen only after all new chunks are extracted and committed. A revision with failed chunks does not count as complete for retraction by absence (consolidation doc §10, Q2).
- **Markdown-only in slice 1.** Non-markdown input is chunked best-effort as markdown/plain text; a content-type seam for other formats is deferred.
- **Chunker need not match the app's.** A different boundary yields a new revision and measurable churn via `knowledge_get_correction_changes` (ADR-0705/0706); this is accepted and noted in release notes.
- **Matching strategy is the Plan stage's decision.** Candidates: content hash on whole chunks with heading as metadata, and paragraph-level matching. The selection is justified by measuring re-extraction on real revision pairs.
- **Chunk extraction reuses the existing per-chunk pipeline.** Document context is added to the prompt; each chunk is still extracted independently and cited only to its own episode. No whole-document extraction pass in slice 1.
- **Mode transition (consolidation doc §10, Q6).** Chunk-mode episodes in the same group sharing the document's `doc_id` are treated as the baseline the first document-mode revision replaces (superseded with reason `revision` on commit). Whether chunk-mode episodes lacking `doc_id` but matching by `chunk_id` name prefix are adopted is left to Plan.
- **Revision-identity carrier** (new episode column versus a per-revision record) is a Plan decision; the requirement is only that it is server-minted, WAL-durable, and absent in chunk mode.
- **Empty revision.** Submitting an empty document is treated as a valid completeness claim that retires all prior chunks only if explicitly allowed by Plan; otherwise it is rejected. Default to rejecting.
- **`effective_at` is recorded on the revision and follows existing temporal conventions;** it never writes `valid_at`/`invalid_at` or alters belief time.
- **Document-size cap default and configuration** (an environment variable analogous to the existing advisory chunk limit) are chosen in Plan.
- **Authentication and scopes** follow the existing MCP scope model; no new scope is introduced.
- **Large documents are ingested synchronously within one call**, with progress streaming; no background job queue is added.

## Out of Scope *(optional)*

- Slice 2: #733's global interpretation pass and document-scoped reconciliation (candidate assertions with modality — proposed/rejected/accepted — merged across sections before publication). A benchmark comparing chunk-only, whole-document and hybrid extraction on 20–30 documents is a separate follow-up issue to be filed once this slice lands.
- `knowledge_list_sources` and other document-listing reads (a follow-up; the data this slice records makes them cheap).
- Non-markdown content types and format-specific parsers.
- Retiring `canonical-chunker.ts` and adopting document mode in the Liminis Notes app (follow-up in shadoworg/liminis-notes).
- Page-as-view-over-evidence work (#732), which consumes the revisions this slice produces.
- Changes to `knowledge_process_chunk` semantics, including its advisory chunk-size warning.
- Withdrawing/un-superseding beliefs, and fact-level (as opposed to episode-level) supersession.
- Returning -32601 for unknown methods and other unrelated items from the consolidation proposal.

## Source References *(optional)*

- `ideas/ingestion-surface-consolidation.md` — §1 (claim), §5 (completeness claim, revision identity), §6 (`process_document`, `preview_chunks`), §7 (chunker ownership), §8 (in-flight interactions), §10 (open questions 2, 5, 6)
- `ideas/document-aware-fact-extraction.md`
- `docs/adr/0705-belief-time-separate-from-validity.md` (revision, removal, split units)
- `docs/adr/0706-correction-diff-and-affected-work-reads.md`
- `docs/adr/0707-document-supersession.md` (`doc_id`)
- `docs/adr/0752-episode-origin-and-corroboration.md` (`origin`, `origin_conflict`)
- Issues: #733 (community request), #732, #596, #291, #293, #284, #282, #288
