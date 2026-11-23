# Feature Specification: `knowledge_process_chunk` no-ops with `status: "unchanged"` when a resubmitted chunk is identical

**Feature Branch**: `fabrik/issue-858`
**Created**: 2026-11-21
**Status**: Specified
**Input**: User description: "Implements #837.

## Problem
Since ADR-0705, resubmitting a `chunk_id` to `knowledge_process_chunk` supersedes the earlier episode, but it re-runs LLM extraction even when the text is byte-identical. `knowledge_process_document` already skips unchanged chunks by content hash. `process_chunk` doesn't (#284 was closed in favour of document mode), so clients keep their own content-hash ledgers to avoid paying for re-syncs.

## Change
- In `process_chunk`, before any embed or extract call, look up the live episode for `(group_id, chunk_id)`. If its content, attributes, title and origin are identical to the request, write nothing and return `{status: \"unchanged\", episode_uuid: <existing>}` with no embed or LLM calls. Compare with a content hash if one is stored, otherwise compare directly. Use the same normalisation as document mode, and reuse its hashing helper if there is one.
- Any difference in those fields takes today's supersede path.
- Add `unchanged_noop` to the features list from #854.
- Update the docs and the tool description.

## Acceptance
- Submit the same chunk twice. The second call returns `unchanged`, makes zero extractor and embedder calls (asserted with a counting mock), and writes no WAL record.
- Changing only the attributes or title re-extracts and supersedes as today.
- A chunk whose live episode was superseded or removed is processed normally."

## Background

Since ADR-0705, resubmitting a `chunk_id` to `knowledge_process_chunk` is a *correction*: the earlier live episode for that `(group_id, chunk_id)` is superseded and the new text is extracted afresh. This is right when the text changed, but it also happens when the resubmitted chunk is byte-identical. The service then pays for an embedding call and an LLM extraction, writes a new episode, and supersedes an old one whose content was exactly the same, producing churn and cost with no information gain.

`knowledge_process_document` already avoids this: it compares each chunk's content hash with what was stored and skips unchanged chunks. Single-chunk mode has no equivalent (issue #284, which proposed one, was closed in favour of document mode). Clients that use `knowledge_process_chunk` directly therefore keep their own content-hash ledgers purely to avoid paying for re-syncs. That duplicates logic the server is better placed to own, and a client ledger can drift from what the graph actually holds (for example after the episode was superseded or removed).

This issue makes the server do the comparison. It implements #837 and depends on #854, which introduces the `knowledge_capabilities` features list that this change adds a feature string to.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Re-sync of an unchanged chunk costs nothing (Priority: P1)

A client that re-syncs a document chunk by chunk resubmits every chunk on each sync. For chunks whose text, attributes, title and origin have not changed, the service recognises that the live episode already matches, does no embedding or extraction work, writes nothing, and tells the client so.

**Why this priority**: This is the whole point of the issue: removing LLM and embedder cost, and graph churn, from no-change re-syncs, and letting clients drop their own ledgers.

**Independent Test**: Submit a chunk, then submit it again identically with counting stand-ins for the extractor and embedder. The second call returns `unchanged` with the first call's episode UUID, the stand-ins record zero calls during it, and no WAL record is written.

**Acceptance Scenarios**:

1. **Given** a chunk was successfully ingested under `(group_id, chunk_id)` and its episode is live, **When** the same request is submitted again with identical content, attributes, title and origin, **Then** the response has `status: "unchanged"` and `episode_uuid` equal to the existing live episode's UUID.
2. **Given** the same state, **When** the identical request is submitted, **Then** zero extractor calls and zero embedder calls are made and no WAL record is appended.
3. **Given** the same state, **When** the identical request is submitted, **Then** the existing episode stays live, is not superseded, and the graph's entity, relationship and episode counts are unchanged.

---

### User Story 2 - Any real change still corrects the graph (Priority: P1)

A client that edits a chunk's text, attributes, title or origin and resubmits it gets exactly today's behaviour: re-extraction and supersession of the earlier episode.

**Why this priority**: The shortcut must never suppress a genuine correction. A false "unchanged" would silently leave stale facts believed.

**Independent Test**: Ingest a chunk, then resubmit it changing exactly one of text / attributes / title / origin at a time. Each resubmission extracts, writes a new episode, and supersedes the old one, as before the change.

**Acceptance Scenarios**:

1. **Given** a live episode for `(group_id, chunk_id)`, **When** the request differs only in `attributes`, **Then** the call re-extracts and supersedes as today and does not return `unchanged`.
2. **Given** the same state, **When** the request differs only in `title`, **Then** the same.
3. **Given** the same state, **When** the request differs only in `origin`, **Then** the same.
4. **Given** the same state, **When** the request differs in `chunk_text` by a single character, **Then** the same.

---

### User Story 3 - Superseded, removed or absent chunks are processed normally (Priority: P2)

If no live episode exists for the chunk, there is nothing to be "unchanged" against, and the chunk is ingested as a normal new chunk.

**Why this priority**: Prevents the shortcut from resurrecting nothing or blocking re-ingest after a removal; it protects correctness of the removal and supersession flows.

**Independent Test**: Ingest a chunk, mark it removed (or supersede it by another revision), then submit the original text again. It is extracted and written as a normal ingest.

**Acceptance Scenarios**:

1. **Given** the chunk's only episode was superseded by a later revision, **When** the superseded revision's exact text is resubmitted, **Then** it is compared against the *live* episode (not the superseded one) and, since it differs, takes the supersede path.
2. **Given** the chunk was removed with `knowledge_mark_chunk_removed`, **When** the same text is resubmitted, **Then** it is ingested normally.
3. **Given** no episode has ever existed for `(group_id, chunk_id)`, **When** the chunk is submitted, **Then** it is ingested normally.

---

### User Story 4 - Clients can discover the behaviour (Priority: P3)

A client can tell whether the server it is talking to supports the no-op, and the documentation and tool description describe the new status.

**Why this priority**: Without discoverability and docs, clients keep their ledgers and cannot safely rely on the new status.

**Independent Test**: Call `knowledge_capabilities` and confirm `unchanged_noop` is in `features`; read the IPC reference and the MCP tool description and confirm both describe `unchanged`.

**Acceptance Scenarios**:

1. **Given** the feature has shipped, **When** a client calls `knowledge_capabilities`, **Then** `features` contains `unchanged_noop` (and stays sorted).
2. **Given** the docs, **When** a reader looks up `knowledge_process_chunk`, **Then** they find the `unchanged` response, the definition of "identical", and the cases where the shortcut does not apply.

---

### Edge Cases

- **Group scoping**: the lookup is by `(group_id, chunk_id)`. The same `chunk_id` live in another group is never consulted.
- **Episodes from other write paths**: a live same-named episode written by `knowledge_add_episode` (not attributable to `knowledge_process_chunk`, per ADR-0292) is not treated as the chunk's own live episode and never yields `unchanged`; the call proceeds as today, including today's "left alone" handling.
- **Legacy episodes** (NULL `write_path`, treated as the chunk's own under ADR-0292) are eligible for comparison. If no content hash is stored for them, the comparison is made directly against the stored fields.
- **Multi-unit revisions**: a call using `supersede: false` or `same_revision_episode_uuids` is part of a multi-call submission of one logical chunk and does not take the shortcut.
- **More than one live episode for the `chunk_id`** (for example a multi-unit chunk): there is no single episode to compare with, so the shortcut does not apply and today's behaviour is used.
- **Oversized `chunk_text`**: an unchanged resubmission performs no ingest, so no oversize `warning` or `chunk_text_oversized` telemetry is produced by it.
- **Request validation still applies**: a malformed request (invalid `reference_time`, bad types, unknown `origin`, missing required fields) is rejected exactly as today and is never reported as `unchanged`.
- **Concurrent resubmission**: two identical calls arriving together must not produce two live episodes for one chunk; whichever result the second caller gets must be consistent with the graph (either `unchanged` or a normal supersede), never a duplicate live episode.
- **Extractor or ontology configuration changes** between the original ingest and the resubmission do not by themselves cause re-extraction; see Out of Scope.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: Before making any embedder or extractor call, `knowledge_process_chunk` MUST look up the live episode for `(group_id, chunk_id)` (restricted to episodes attributable to `knowledge_process_chunk` per ADR-0292, including legacy NULL-`write_path` rows).
- **FR-002**: If that live episode's content, attributes, title and origin are all identical to the request's, the call MUST write nothing (no episode, entity, edge, supersession or WAL record) and MUST return `status: "unchanged"` and `episode_uuid` set to the existing live episode's UUID.
- **FR-003**: An `unchanged` outcome MUST make zero embedder calls and zero LLM extractor calls.
- **FR-004**: If any one of content, attributes, title or origin differs, the call MUST follow the existing supersede path unchanged in behaviour and result shape.
- **FR-005**: Equality MUST use the same normalisation as `knowledge_process_document`'s per-chunk comparison, reusing its hashing helper where one exists. If the stored episode carries a content hash it SHOULD be used for the content comparison; otherwise the stored content is compared directly. Attributes, title and origin equality MUST be judged on their normalised stored forms, so that equivalent attribute objects (for example differing only in key order) compare equal.
- **FR-006**: Only a *live* episode counts. If the chunk's episodes are all superseded or removed, or none exist, the call MUST process the chunk as a normal ingest.
- **FR-007**: The shortcut MUST NOT apply to calls using `supersede: false` or supplying `same_revision_episode_uuids`, nor when more than one live episode exists for the `chunk_id` in the group.
- **FR-008**: A request that is invalid under today's validation rules MUST be rejected as today before the unchanged check can report success.
- **FR-009**: An explicit request `reference_time` that differs from the live episode's stored event time MUST count as a difference and take the supersede path; an omitted `reference_time` (which defaults to "now") MUST NOT count as a difference.
- **FR-010**: Any other request field that influences what is stored for the episode or what extraction would produce (for example `source_file`, `doc_id`) MUST also count as a difference when it differs from the stored value, so `unchanged` is only returned when a normal ingest would have reproduced the same episode.
- **FR-011**: The feature string `unchanged_noop` MUST be added to the single source of truth for `knowledge_capabilities` `features` introduced by #854, keeping the list sorted.
- **FR-012**: The IPC/MCP reference, the `knowledge_process_chunk` tool description, and any user-facing guide covering chunk resubmission MUST document the `unchanged` response, the definition of "identical", the cases where the shortcut does not apply, and that clients no longer need their own content-hash ledgers for chunk re-syncs. Docs describing "resubmitting a `chunk_id` supersedes" MUST be qualified accordingly.
- **FR-013**: Tests MUST cover, using counting stand-ins for the embedder and extractor and a WAL-record check: identical resubmission (zero calls, zero WAL records, same episode UUID, episode still live); a change in each of content, attributes, title and origin separately; a superseded-then-resubmitted chunk; a removed-then-resubmitted chunk; and a first-time chunk.

### Key Entities *(if applicable)*

- **Live episode**: the episode for a `(group_id, chunk_id)` that has not been superseded or removed; the only thing a resubmission is compared against.
- **Unchanged result**: a response `{status: "unchanged", episode_uuid}` signalling that nothing was written.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Submitting the same chunk twice yields `unchanged` on the second call, with 0 embedder calls, 0 extractor calls and 0 WAL records recorded during that call.
- **SC-002**: Changing only the attributes, only the title, only the origin, or only the text of a chunk and resubmitting always re-extracts and supersedes (4 of 4 single-field changes are detected).
- **SC-003**: A chunk whose live episode was superseded or removed is ingested normally in 100% of cases; `unchanged` is never returned when no live episode exists.
- **SC-004**: A client re-syncing N unchanged chunks incurs zero LLM and embedding cost and no growth in episode, entity or relationship counts.
- **SC-005**: `knowledge_capabilities` reports `unchanged_noop`, and the docs and tool description describe the `unchanged` status.

## Assumptions

- #854 (`knowledge_capabilities` and its features list) lands first; this issue only adds one feature string to it.
- The set of compared fields is those named in the issue (content, attributes, title, origin), extended by FR-009 and FR-010 to other request fields that change what a normal ingest would store, so a false `unchanged` is avoided. A deliberate omitted `reference_time` is treated as "no opinion".
- Changes in extractor model, ontology, or other server configuration since the original ingest do not invalidate an `unchanged` result; callers who want re-extraction after such a change must alter the chunk or use the existing reprocess operations.
- The response for `unchanged` carries only `status` and `episode_uuid` (plus any universal envelope fields); fields describing ingest work (entity/edge counts, `superseded_episode_uuids`) are absent or empty. Existing clients that ignore unknown statuses are unaffected, since a new `status` value is additive.
- Stored content hashes, where present, were produced with the same helper and normalisation as document mode; where absent, direct comparison is used, so no migration or backfill is needed.
- This is a performance and cost behaviour change for clients that resubmit identical chunks; it is not a protocol-breaking change, so `protocol_version` is not bumped.

## Out of Scope *(optional)*

- Re-extracting chunks because the extractor, model or ontology changed.
- Changing `knowledge_process_document`'s existing unchanged-chunk logic.
- Any client-side change (removing clients' ledgers is for those clients).
- A force/override flag to re-extract an identical chunk (can be a follow-up if wanted).
- Backfilling content hashes onto old episodes.
- Treating `knowledge_add_episode` as subject to the same no-op.

## Source References *(optional)*

- Issue #837 (origin of this request) and #284 (earlier, closed proposal)
- Issue #854 (`knowledge_capabilities` features list)
- ADR-0705 (corrections supersede episodes) and ADR-0292 (chunk ingest never supersedes unattributed episodes)
- `docs/ipc-mcp-reference.md`, "Belief time: corrections supersede, they do not delete"
