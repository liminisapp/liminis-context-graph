# Feature Specification: Entity merge consolidates summaries and always re-embeds

**Feature Branch**: `fabrik/issue-651`
**Created**: 2026-10-01
**Status**: Specified
**Input**: User description: "Entity merge: consolidate summaries instead of concatenating, and always re-embed summary_embedding (implements #647)"

## Background

When extraction merges an extracted entity into an existing one (a merge decision, on either the exact-name path or the embedding-candidate path), the merged summary is built by joining the existing summary and the incoming summary with a space. Community report #647 shows the result: over a multi-chunk document, or many documents, a frequently mentioned entity's summary grows into a run-on of per-chunk descriptions in arrival order. They often contradict each other ("is added" next to "remains unchanged"; a rejected design next to the adopted one). Since summaries became searchable (0.14.x), these run-ons feed full-text search, summary vector search (#470) and agents that read entity summaries.

Two defects result:

1. **Unbounded concatenation.** Each mention appends another sentence, contradictions included.
2. **A stale `summary_embedding`.** The merge writes only the new `summary`; it never recomputes `summary_embedding`. The vector therefore stays the embedding of the first summary ever extracted while the text keeps changing, so summary vector search ranks on a meaning the text no longer has. Whether WAL replay refreshes the embedding for a merged summary must also be checked: if replay re-embeds and the live path does not, a live database and a rebuilt one rank the same entity differently.

The root cause of *wrong* merges (numeric/identifier near-duplicates) is addressed separately in #650 and is unaffected by this issue: consolidation runs only after a merge has been decided. Existing graphs with wrongly merged entities cannot be split automatically; recovery is re-ingesting after 0.16.3.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Merged summaries are consolidated, not concatenated (Priority: P1)

A user ingests several documents that each describe the same entity differently, including contradictory statements. After ingestion, the entity's summary is one concise, current description rather than every chunk's text appended.

**Why this priority**: This is the reported symptom; it degrades retrieval quality and agent reasoning.

**Independent Test**: With a stubbed extractor returning a canned consolidation, merge into one entity five times with differing/contradictory summaries; assert the stored summary is the consolidated text, is within the length cap, and is not a concatenation of the inputs.

**Acceptance Scenarios**:

1. **Given** an existing entity with a summary and an incoming merge whose summary adds new information, **When** the merge is applied and an extractor is configured, **Then** the stored summary is the extractor's consolidated output.
2. **Given** an incoming summary that conflicts with the existing one, **When** consolidated, **Then** the request instructs the extractor to prefer the most recent/most specific statement and drop superseded claims rather than list both.
3. **Given** an incoming summary that is empty, or a substring of the existing summary, **When** the merge is applied, **Then** no extractor call is made and the existing summary is kept unchanged.

---

### User Story 2 - Summary embedding always matches the stored summary (Priority: P1)

After any merge that changes `summary`, `summary_embedding` is the embedding of the stored summary, so summary vector search ranks on the current meaning.

**Why this priority**: A stale vector silently corrupts search ranking.

**Independent Test**: Merge into an entity, then compare the stored `summary_embedding` with a fresh embedding of the stored `summary`; repeat after a WAL rebuild.

**Acceptance Scenarios**:

1. **Given** a merge that changes the summary, **When** the merge is written, **Then** `summary` and `summary_embedding` are written in the same statement and the embedding corresponds to the new summary.
2. **Given** a database rebuilt from the WAL, **When** a merged entity is read, **Then** its `summary_embedding` equals the embedding of its stored summary, matching the live database's.

---

### User Story 3 - Bounded fallback when consolidation is unavailable (Priority: P1)

If no extractor is configured, or the consolidation call fails, the merge still succeeds with a bounded summary and a refreshed embedding.

**Why this priority**: Unbounded concatenation must never be reachable, and extraction must not fail because consolidation did.

**Independent Test**: Run merges with (a) no extractor and (b) a stub that errors; assert the summary length is at most the cap, cut at a sentence boundary, favouring the most recent text, and the embedding is refreshed.

**Acceptance Scenarios**:

1. **Given** no extractor is available, **When** a merge adds information, **Then** the summary is a deterministic merge keeping the most recent text up to the cap, cut at a sentence boundary.
2. **Given** the consolidation call returns an error or empty output, **When** the merge is applied, **Then** the same deterministic fallback is used, the chunk's extraction is not failed, and the embedding is refreshed.

---

### User Story 4 - Deterministic WAL replay without LLM calls (Priority: P1)

The WAL records the resulting summary; replay reproduces identical stored summaries and never calls an LLM.

**Why this priority**: Rebuilds must be deterministic and free of external dependencies.

**Independent Test**: Ingest with a stub extractor that counts calls, rebuild from the WAL with a stub that fails on any call; assert summaries are identical and the call count is zero during replay.

**Acceptance Scenarios**:

1. **Given** a WAL containing merges, **When** it is replayed, **Then** stored summaries match the live database and no consolidation call occurs.
2. **Given** vectors are not stored in the WAL (#526), **When** replay applies a merged summary, **Then** the embedding is recomputed from the recorded summary.

---

### Edge Cases

- Incoming summary equals, is empty, or is contained in the existing summary: skip the LLM call, no change (and no needless re-embed).
- Existing summary is empty and incoming is non-empty: the incoming summary is used (subject to the cap) without needing consolidation.
- Several merges into the same entity within one chunk or batch: each consolidation must build on the prior result, not on the stale pre-batch summary.
- Consolidation output exceeds the cap: it is bounded the same way as the fallback (sentence-boundary cut).
- Consolidation call is slow or cancelled: cancellation behaves as it does for the existing dedup check; the write lock is never held while waiting.
- Single sentence longer than the cap (no sentence boundary): hard-truncate at the cap.
- Embedding provider failure at merge time: behave as for the existing insert path's embedding failure (no silently stale vector).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: On a merge whose incoming summary adds information, the system MUST produce a single consolidated summary using the configured extractor (Anthropic API or OpenAI-compatible `--extractor-*` endpoint), prompted to merge the existing and incoming descriptions into one concise, current description, preferring the most recent or most specific statement on conflict and dropping superseded claims.
- **FR-002**: The consolidation call MUST run in the lock-free decision phase (alongside the dedup check) and MUST NOT run under the write lock; only the resulting summary and embedding are written under the lock.
- **FR-003**: The system MUST skip the consolidation call when the incoming summary is empty or is a substring of the existing summary.
- **FR-004**: Consolidation SHOULD be batched per chunk where practical to limit call count.
- **FR-005**: Whenever a merge changes `summary`, the system MUST recompute `summary_embedding` from the new summary and write both in the same update.
- **FR-006**: Every merged summary MUST be bounded by a hard length cap (~600 characters), applied to extractor output as well as to the fallback.
- **FR-007**: When no extractor is available or the consolidation call fails, the system MUST fall back to a deterministic bounded merge that keeps the most recent text up to the cap, cutting at a sentence boundary. Unbounded concatenation MUST NOT occur on any path.
- **FR-008**: A failed consolidation MUST NOT fail the chunk or episode; it degrades to the fallback.
- **FR-009**: The WAL MUST record the resulting (consolidated or fallback) summary; replay MUST apply the recorded summary verbatim, MUST NOT call any LLM, and MUST recompute `summary_embedding` from it (vectors are not stored in the WAL, per #526).
- **FR-010**: The investigation MUST establish whether WAL replay currently re-embeds merged summaries; live and rebuilt databases MUST end up with the same embedding policy (embedding equals embedding of stored summary).
- **FR-011**: Consolidation MUST NOT affect whether two entities merge; it runs only after a merge decision.
- **FR-012**: No schema change; no change to the IPC/MCP method surface.

### Key Entities

- **Entity**: Graph node with `summary` (text, FTS-searchable) and `summary_embedding` (vector used by summary vector search).
- **Merge decision**: Outcome of resolution that folds an extracted entity into an existing one, carrying the resulting summary.
- **Extractor**: The configured LLM endpoint already used for extraction and (per #650) dedup checks.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Five successive merges with differing or contradictory summaries (stubbed extractor) yield one summary within the cap that is not a concatenation of the inputs.
- **SC-002**: After any merge, stored `summary_embedding` equals the embedding of the stored `summary`, in the live database and after a WAL rebuild (100% of merged entities in tests).
- **SC-003**: With no extractor, or a failing one, merged summaries never exceed the cap and the embedding is still refreshed.
- **SC-004**: WAL replay reproduces identical stored summaries with zero consolidation calls.
- **SC-005**: A test or Review inspection confirms the consolidation call occurs outside the write lock.
- **SC-006**: Merges with an empty or already-contained incoming summary make zero extractor calls.

## Assumptions

- The cap is ~600 characters; the exact value is an implementation choice, fixed as a named constant and documented.
- The consolidation call uses the same extractor configuration as extraction; no new CLI flag is required (a flag to disable consolidation is not assumed).
- Embeddings use the existing embedding provider already used for `summary_embedding` on insert.
- Delivered in milestone 0.16.3 as a patch-sized change.

## Out of Scope

- Preventing wrong merges (numeric/identifier near-duplicates) — #650.
- Splitting or repairing already wrongly merged entities in existing graphs (recovery is re-ingestion).
- Keeping per-source description history as structured data (would need a schema change).
- Deferred/background consolidation of existing run-on summaries in current databases.
- Schema, IPC or MCP surface changes.

## Source References

- Issues: #647 (report), #650 (dedup veto), #470 (summary vector search), #526 (vectors not stored in WAL)
- `crates/core/src/episode.rs` (merge branches ~766/~782; Phase C write ~876–880)
- `crates/core/src/dedup_adapter.rs` (existing lock-free `is_duplicate` call)
