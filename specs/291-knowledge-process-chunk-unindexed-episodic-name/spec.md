# Feature Specification: In-process accelerator for Episodic `name` and `doc_id` lookups

**Feature Branch**: `fabrik/issue-291`
**Created**: 2026-11-15
**Status**: Specified — design superseded by [ADR-0291](../../docs/adr/0291-native-art-index-for-episodic-lookups.md)
**Design note**: This spec was written around an in-process, non-persisted accelerator
(FR-004 – FR-008, FR-011, FR-013 and the "transitional, #221 deletes it" assumptions). The
implementation instead uses database-native ART indexes on `Episodic.name` and `Episodic.doc_id`,
because #221 had already replaced the `NameIndex` precedent with native indexes (lbug ≥ 0.18).
The index is persisted by the database, so FR-011's "in-process only" and FR-013's "transitional"
do not apply as written; FR-004 – FR-008 hold because the database maintains the index. The
goals (FR-001 – FR-003, FR-009, FR-010, SC-001 – SC-005) are unchanged. See ADR-0291 for the
requirement-by-requirement mapping.
**Input**: User description: "

## Background

#284's `reconstruct_prior_chunk_text` idempotency mechanism (`crates/core/src/handlers.rs`, ADR-0053) added a lookup that runs on **every** `knowledge_process_chunk` call, including first-time ingest of a `chunk_id` never seen before:

```
Db::get_episodes_by_chunk_id(chunk_id, group_id)
  -> MATCH (ep:Episodic) WHERE ep.name = $name AND ep.group_id = $gid ...
```

`crates/core/src/schema.rs` defines `Episodic`'s primary key as `uuid`, not `name` — confirmed via `CREATE NODE TABLE IF NOT EXISTS Episodic (...)`. lbug has no secondary property index for exact-match equality on an arbitrary field (only FTS and vector indexes exist elsewhere in this schema; see `CREATE_FTS_INDEX`/`CREATE_VECTOR_INDEX` call sites in `schema.rs`/`db.rs`). So `ep.name = $name` is necessarily a full-table exact-match scan.

Cost is O(episodes-in-group) per call. Since the lookup runs unconditionally per `knowledge_process_chunk` call (not just on replace), a bulk sequential ingest of N chunks into the same group is O(N²) overall — the same corpus shape that produced #202 (4,374 chunks in one sequential run).

## Precedent

This is the same defect class #219/#283 already fixed for `Entity.name` lookups: lbug can't index that shape natively, so the fix was an **in-process accelerator** (`crates/core/src/name_index.rs::NameIndex`, ADR-0038) — a `HashMap`-backed structure kept in sync with every entity create/merge/delete, always re-verified against the DB before being trusted (so staleness can only degrade to a miss, never return a wrong result).

## Proposed fix

Add an equivalent in-process accelerator for `Episodic.name` (scoped by `group_id`), e.g. `EpisodicNameIndex`, mirroring `NameIndex`'s structure and invalidation contract:
- Insert on every episode creation (`add_episode`'s Phase C commit, for both `knowledge_add_episode` and `knowledge_process_chunk` callers).
- Remove on every deletion path that can remove an `Episodic` row: `remove_episodes_by_chunk_id`, `remove_episodes_by_uuids`, `remove_episode`, `remove_episodes_by_source`.
- `get_episodes_by_chunk_id` and any other exact-match `Episodic.name` lookup consult the accelerator first, falling back to (or verifying against) the DB.

## Scope note

This was flagged during #284's Validate-stage review as correct but out of scope for a comment-review fix — it requires threading a new field through `Db`/`AppState` and updating every episode create/delete call site, comparable in size to the already-tracked #288 (per-chunk_id locking). ADR-0053's Consequences section documents this as an accepted, quantified cost pending this follow-up.

## References

- ADR-0053 (chunk-splitting-and-chunk-id-idempotency), Consequences section
- ADR-0038 (NameIndex) / #219 / #283 — the precedent for this exact defect class
- #202 — the corpus shape (4,374 sequential chunks) that makes O(N²) real, not theoretical

Maintainer comment 2026-07-31: this fix is expected to be transitional. Implement the cheapest correct thing that removes the scan; do not generalise it or build shared infrastructure other call sites would adopt. Once lbug 0.18.x native secondary indexes land (#190/#220), #221 will replace NameIndex and whatever ships here with real indexes, and #221 should explicitly list whatever this ships as something it deletes.

Maintainer comment 2026-11-15: Priority raised. Since Temporal 1 (ADR-0705), `supersede` defaults to `true` on `knowledge_process_chunk`, so the `Episodic.name = chunk_id` lookup runs on every call. ADR-0707 notes `doc_id` lookups are full scans too. Liminis Notes re-indexes whole notebooks (first run, after a chunker change, the document-mode cut-over in #733). Scope: an in-process accelerator in the style of NameIndex (ADR-0038), covering both (group_id, name) for chunk_id lookups and (group_id, doc_id), kept in sync on insert, supersede, purge and WAL replay. Acceptance bar: a bulk ingest of a few thousand chunks into one group shows per-call lookup cost that doesn't grow with group size.
"

## Background

Every `knowledge_process_chunk` call looks up existing episodes for the incoming `chunk_id` (stored as the episode's `name`) within the target group, to support chunk-level idempotency (ADR-0053). The graph store cannot index that property for exact-match, so each lookup scans every episode in the group. Cost per call grows with the group, and a bulk sequential ingest of N chunks is O(N²) overall. The corpus in #202 (4,374 sequential chunks into one group) shows this is real, not theoretical.

The problem worsened with document/temporal supersession (ADR-0705, ADR-0707): `supersede` now defaults to true on chunk processing, so the lookup runs on **every** call rather than only on replacement. Document-addressing lookups by `doc_id` (existence and uniqueness checks, ADR-0707) are likewise unindexed scans. The primary consumer, Liminis Notes, re-indexes whole notebooks on first run, after a chunker change, and during the document-mode cut-over (#733), so each of those pays the quadratic cost.

The same defect class was already fixed for entity-name lookups with an in-process accelerator (ADR-0038, #219/#283): a map kept in sync with every mutation and always verified against the database before being trusted, so staleness can only degrade to a miss and never yield a wrong answer. This issue applies that proven pattern to episode `name` and `doc_id`.

This fix is explicitly **transitional**: native secondary indexes (lbug 0.18.x+, tracked by #190/#220) will make it unnecessary, and #221 is expected to delete it. The goal is the cheapest correct change that removes the scan — not reusable infrastructure.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Bulk chunk ingest runs in linear time (Priority: P1)

An application (e.g., Liminis Notes) ingests thousands of chunks sequentially into a single group via `knowledge_process_chunk`. The per-call cost of finding prior episodes for a `chunk_id` must stay flat as the group grows.

**Why this priority**: This is the reported defect and the stated acceptance bar; it blocks full-notebook re-indexing at realistic sizes.

**Independent Test**: Ingest a few thousand distinct chunks into one group and compare the chunk-id lookup cost at the start versus the end of the run; it must not grow with the number of episodes in the group.

**Acceptance Scenarios**:

1. **Given** a group holding thousands of episodes, **When** a never-before-seen `chunk_id` is processed, **Then** the "no prior episodes" answer is obtained without scanning the group's episodes.
2. **Given** a group holding thousands of episodes, **When** a previously ingested `chunk_id` is re-processed, **Then** its prior episodes are found without scanning the group's episodes, and the existing idempotency/supersession behavior is unchanged.
3. **Given** a bulk ingest of N chunks into one group, **When** total lookup time is measured, **Then** it scales roughly linearly with N rather than quadratically.

---

### User Story 2 - Document lookups by `doc_id` avoid group scans (Priority: P1)

Operations that resolve episodes by `(group_id, doc_id)` — such as existence and uniqueness checks used by document supersession — no longer scan every episode in the group.

**Why this priority**: The maintainer raised scope to cover `doc_id` because document-mode ingest and re-indexing hit the same quadratic pattern.

**Independent Test**: With a large group, record and look up documents by `doc_id` and confirm lookup cost does not grow with group size and that results match a direct database query.

**Acceptance Scenarios**:

1. **Given** a large group with many distinct `doc_id`s, **When** an episode is looked up by `(group_id, doc_id)`, **Then** the result is found without scanning the group.
2. **Given** a `doc_id` that does not exist in the group, **When** it is looked up, **Then** "not found" is returned without a group scan.
3. **Given** the same `doc_id` in two different groups, **When** looked up in each, **Then** each lookup sees only its own group's episode.

---

### User Story 3 - Accelerator never returns wrong answers (Priority: P1)

The accelerator stays consistent with the database across every path that creates, supersedes, or removes episodes, including restart and WAL replay; any staleness degrades to a miss that is resolved against the database.

**Why this priority**: Correctness is non-negotiable; a stale positive or a falsely-empty answer would break idempotency and cause duplicate or lost chunks.

**Independent Test**: Exercise create, supersede, each removal path, service restart, and WAL replay/rebuild, and after each confirm the accelerated lookup returns exactly what a direct database query returns.

**Acceptance Scenarios**:

1. **Given** an episode removed by chunk id, by uuids, by single episode, or by source, **When** its `chunk_id` or `doc_id` is later looked up, **Then** the removed episode is not returned.
2. **Given** a chunk re-processed with `supersede` true, **When** prior episodes are looked up, **Then** the result reflects the post-supersession state identically to the un-accelerated lookup.
3. **Given** the service restarts or the graph is rebuilt from the WAL, **When** a chunk or document lookup is made, **Then** results are correct (the accelerator is repopulated or safely falls back to the database).
4. **Given** the accelerator holds an entry that no longer matches the database, **When** it is consulted, **Then** the entry is not trusted and the lookup resolves against the database.
5. **Given** an episode created by `knowledge_add_episode` (not `knowledge_process_chunk`), **When** its name is later looked up, **Then** it is found.

---

### Edge Cases

- Multiple episodes sharing one `name` within a group (e.g., superseded generations of the same chunk): all of them must be returned, as before.
- The same `name` or `doc_id` in different groups must never cross-contaminate.
- Episodes created before the accelerator is populated (cold start, existing graphs): lookups must still be correct, with population cost paid once rather than per call.
- A failed or rolled-back episode creation must not leave an entry that makes the episode appear to exist.
- Episodes with no `doc_id` (legacy or caller-omitted) must not be indexed under a bogus `doc_id` key.
- Concurrent ingest into different groups, and concurrent removal during lookup, must not produce wrong results.
- WAL replay and rebuild-from-WAL must keep the accelerator in sync or invalidate it.
- Group deletion/purge must not leave stale entries that survive into a recreated group of the same id.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The system MUST resolve "episodes in group G with name N" (the `chunk_id` lookup) without scanning all episodes in G in the common case.
- **FR-002**: The system MUST resolve "episode(s) in group G with `doc_id` D" without scanning all episodes in G in the common case.
- **FR-003**: Both lookups MUST be keyed by group; entries from one group MUST never be returned for another.
- **FR-004**: The accelerator MUST be updated on every episode creation, for both `knowledge_add_episode` and `knowledge_process_chunk` callers.
- **FR-005**: The accelerator MUST be updated on every path that removes or supersedes an `Episodic` row, including removal by chunk id, by uuids, by single episode, and by source, supersession on re-processing, and any purge path.
- **FR-006**: The accelerator MUST remain correct across WAL replay and rebuild-from-WAL, either by being updated during replay or by being invalidated and repopulated.
- **FR-007**: Results from the accelerator MUST be verified against the database before being trusted, so that staleness can only cause a miss that falls back to the database, never an incorrect result (same contract as ADR-0038).
- **FR-008**: A lookup for a `chunk_id` or `doc_id` that the accelerator reports absent MUST still produce a correct answer when the accelerator is cold or not authoritative (e.g., after restart), without returning false "not found".
- **FR-009**: Externally observable behavior of `knowledge_process_chunk`, `knowledge_add_episode`, and all removal/supersession operations MUST be unchanged; this is a performance-only change with no change to the IPC protocol, tool surface, or stored schema.
- **FR-010**: Lookups returning multiple episodes for one name within a group MUST return the same set as the un-accelerated query.
- **FR-011**: The accelerator MUST be in-process only (not persisted) and MUST NOT require changes to the on-disk schema.
- **FR-012**: The change MUST be the minimal correct implementation and MUST NOT introduce shared, general-purpose indexing infrastructure for other call sites.
- **FR-013**: The accelerator and its call-site hooks MUST be clearly identifiable as transitional (e.g., documented with a decision record) so that #221 can enumerate and delete them when native secondary indexes are adopted.
- **FR-014**: Automated tests MUST cover: lookup correctness after each create/supersede/remove path, group isolation, cold-start/restart behavior, WAL replay, and staleness degrading to a database fallback.
- **FR-015**: A measurable check MUST demonstrate that per-call lookup cost does not grow with group size over a bulk ingest of a few thousand chunks.

### Key Entities *(if applicable)*

- **Episode (`Episodic`)**: A stored ingest unit, identified by uuid, scoped to a group, carrying a `name` (equal to the `chunk_id` for chunk ingest) and an optional `doc_id`.
- **Episode lookup accelerator**: An in-process, non-persistent structure mapping `(group_id, name)` and `(group_id, doc_id)` to the matching episodes, always verified against the database and discarded when native indexes arrive.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: In a bulk ingest of at least a few thousand sequential chunks into one group, per-call chunk-id lookup time at the end of the run is within a small constant factor of the time at the start (it does not grow with group size).
- **SC-002**: Total lookup cost for a bulk sequential ingest of N chunks scales linearly (not quadratically) with N.
- **SC-003**: `(group_id, doc_id)` lookups on a large group complete in time independent of group size.
- **SC-004**: Across the full test suite, accelerated lookups return results identical to direct database queries in every create, supersede, remove, restart, and WAL-replay scenario (zero divergences).
- **SC-005**: No change in observable API behavior: existing parity and idempotency tests pass unchanged.

## Assumptions

- Group sizes of interest are in the thousands to tens of thousands of episodes; memory for an in-process map of that size is acceptable (as with the entity-name accelerator).
- `doc_id` is unique within a group (ADR-0707), but `name` may map to more than one episode, so the name lookup must return a set.
- Covering `(group_id, doc_id)` is in scope per the 2026-11-15 maintainer comment, which supersedes the original issue text that only mentioned `name`.
- Cold-start population of the accelerator (lazily or at open) is acceptable provided its cost is paid once, not per call.
- This work is transitional and will be deleted by #221; no effort is spent making it reusable.

## Out of Scope *(optional)*

- Per-`chunk_id` locking (#288).
- Replacing the accelerator or `NameIndex` with native lbug secondary indexes (#221, #190/#220).
- Indexing any other property or other node/edge tables.
- Changes to the document-mode ingestion design (#733) or supersession semantics (ADR-0705/0707).
- Changes to the IPC protocol, MCP tool surface, or stored schema.

## Source References *(optional)*

- ADR-0053 (chunk-splitting-and-chunk-id-idempotency), Consequences section
- ADR-0038 (NameIndex), #219, #283 — precedent for the same defect class
- ADR-0705 (supersede defaults to true), ADR-0707 (document supersession; `doc_id` lookups are scans)
- #202 (4,374-chunk sequential corpus), #221 (native index replacement), #190/#220 (lbug upgrade), #288 (per-chunk_id locking), #733 (document-mode cut-over)
