# Feature Specification: `knowledge_reembed_missing` — repair zero-vector embeddings across all kinds

**Feature Branch**: `fabrik/issue-809`
**Created**: 2026-11-21
**Status**: Specified
**Input**: User description: "## Problem

Community report: #799, the main proposal. **Blocked by #808**, which adds the durable per-group record of zero-vector rows that this issue selects from.

A row stored with a zero-vector embedding after a failed recompute stays invisible to vector search for good. The only repairs today:
- `knowledge_backfill_summary_embeddings` (#470, `crates/core/src/backfill_summary_embeddings.rs`). It covers `summary_embedding` only and re-embeds *every* entity unconditionally.
- A full `knowledge_rebuild_from_wal`, which takes hours on large groups.

Nothing repairs `name_embedding`, `fact_embedding` or `content_embedding`.

## Proposal: `knowledge_reembed_missing(group_id, kinds?, dry_run?)` (admin)

- **Selection:** the rows recorded by #808 for `group_id`, filtered by `kinds` (`name`, `fact`, `summary`, `content`; default all). There's no need to detect zero vectors in queries: `db.rs` has no cheap way to read a `FLOAT[]` back, as the backfill module's doc notes.
- **Work:**
  - Recompute each row from its co-located source text: the entity name, the summary, the edge fact, the episode content. This is the same pairing #440's replay recompute uses.
  - Batch the calls and use #808's retry and backoff policy.
  - Use the embedder's document role and instruction profile, as replay does (#790).
- **Result:** per-kind `candidates`, `reembedded`, `failed`, `skipped_no_source`. `dry_run` only counts and makes no embedder calls.
- **Bookkeeping:** a successful re-embed removes the row from #808's record, so the `knowledge_status` count goes down. A failed row stays recorded.
- **Scope:** admin tool registry, with group-scoped writes. Respect `--writable-groups` (#742), as other admin writes do.

## The key constraint: HNSW-indexed columns

All four embedding columns carry global vector indexes (`db.rs` ~L1248–1290):
- `entity_name_embedding_idx`
- `entity_summary_embedding_idx`
- `episodic_content_embedding_idx`
- `edge_fact_embedding_idx`

lbug rejects a `SET` on an indexed column ("used in one or more indexes", ADR-0379). `knowledge_backfill_summary_embeddings` handles this by:
1. dropping the index;
2. writing with plain `SET` while holding `write_lock` from the candidate read through the rebuild;
3. rebuilding the index, toggling `state.indices_built` so concurrent searches stay safe.

This tool must do the same, and Plan must address:
- **Cost and blast radius.** The indexes span *all* groups, so a repair of one group blocks writes everywhere and rebuilds a database-wide index. Embed every candidate *before* dropping anything, so the index stays down only for the writes. Drop and rebuild only the indexes for the kinds that actually have candidates. With zero candidates, touch nothing.
- **Reuse the backfill's index-lifecycle code.** Factor it out rather than duplicate it.
- **Failure safety.** If the rebuild fails, the service must still recover: `build_indices_once` on the next search, as today. Never leave `indices_built` wrong.
- **Progress events and cancellation**, like the backfill.

## Acceptance criteria
1. Rows recorded by #808 with zero vectors get real embeddings and are found by vector search afterwards. Cover each of the four kinds.
2. Rows not in the record are untouched, and their embeddings are byte-identical.
3. `dry_run` reports counts and makes no embedder calls and no writes.
4. With zero candidates, no index is dropped.
5. Only indexes for kinds with candidates are dropped and rebuilt. A search concurrent with a repair returns correct results once the repair finishes.
6. The `knowledge_status` count from #808 drops to 0 after a full repair, and stays at the failed count on a partial repair.
7. `--writable-groups` refuses an unlisted group.
8. The MCP tool count goes up by one, and docs, CHANGELOG and an ADR are updated.

## Out of scope
- Re-embedding renamed entities, or detecting a changed source text (ADR-0379 decision 6). That's a possible follow-up.
- Changing `knowledge_backfill_summary_embeddings`' behaviour, beyond refactoring the shared index-lifecycle code."

## Background

When an embedder call fails during WAL replay, the affected row is stored with a same-dimension zero vector. A zero vector is invisible to vector search, so the row silently drops out of search results and stays that way. This was reported in #799: a cold replay of roughly 80k mutations against a local CPU embedder, with intermittent send errors, left rows unsearchable.

#808 (merged) did two things: replay now retries and splits failing embed calls, and every row that still ends up with a fallback zero vector is recorded durably as `(group, kind, uuid)` per group. `knowledge_status` reports the count per group, by kind. The record says *which* rows need repair. Nothing yet acts on it.

Today's repair options are poor:
- `knowledge_backfill_summary_embeddings` (#470) covers only the summary embedding of entities, and re-embeds every entity whether or not it is broken.
- A full `knowledge_rebuild_from_wal` takes hours on large groups.
- Nothing repairs the name embedding (entities), the fact embedding (relationships) or the content embedding (episodes).

This issue adds an administrative operation that re-embeds exactly the recorded rows, for any of the four embedding kinds, and clears them from the record once they are repaired.

One operational constraint shapes the feature. All four embedding columns carry a database-wide vector index, and the database refuses writes to an indexed column. The existing summary backfill works around this by dropping the index, writing, and rebuilding it while holding the write lock. The same lifecycle is needed here, but the indexes span all groups. Repairing one group therefore blocks writes everywhere while the index is down. The feature must keep that window as small as possible and must never leave search broken if something fails midway.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Repair recorded zero-vector rows for a group (Priority: P1)

An operator sees in `knowledge_status` that a group has rows with zero-vector embeddings. They run `knowledge_reembed_missing` for that group. Each recorded row is re-embedded from its stored source text and becomes findable by vector search again. The status count for the group drops accordingly.

**Why this priority**: This is the core value. Without it the record from #808 is diagnostic only and the only remedy is an hours-long rebuild.

**Independent Test**: Build a group whose rows of all four kinds carry zero vectors and are recorded. Run the tool with defaults. Confirm each kind now has a real embedding, a vector search for each row's text finds it, and the group's status count is zero.

**Acceptance Scenarios**:

1. **Given** a group with recorded zero-vector rows of each of the four kinds, **When** the operator runs the tool with no `kinds` filter, **Then** every recorded row is re-embedded, the result reports per-kind `candidates`, `reembedded`, `failed` and `skipped_no_source`, and vector search finds each repaired row.
2. **Given** the same group, **When** the repair finishes with every row successful, **Then** `knowledge_status` reports `zero_vector_rows.total` of 0 for the group.
3. **Given** a group with recorded rows, **When** the operator passes `kinds` containing only `name`, **Then** only recorded `name` rows are repaired and the records for other kinds are untouched.
4. **Given** rows in the group that are not in the record, **When** the tool runs, **Then** those rows are untouched and their stored embeddings are byte-identical before and after.

---

### User Story 2 - Preview the repair without side effects (Priority: P2)

Before committing to a repair that can block writes, the operator runs the tool in `dry_run` mode to see how many rows per kind would be repaired.

**Why this priority**: The repair has a database-wide cost, so operators need to know whether it is worth running before they run it.

**Independent Test**: Run the tool with `dry_run` on a group with recorded rows. Confirm that the per-kind candidate counts match the record, that the embedder received no calls, and that no row, index or record entry changed.

**Acceptance Scenarios**:

1. **Given** a group with recorded rows, **When** the tool runs with `dry_run`, **Then** it reports per-kind `candidates` and makes no embedder calls, no writes and no index changes.
2. **Given** a `dry_run` followed by a real run, **When** compared, **Then** the real run's `candidates` equal the dry run's.

---

### User Story 3 - Partial failure leaves a correct, retryable state (Priority: P2)

The embedder is still flaky during the repair. Rows that cannot be embedded fail individually without aborting the rest, and stay recorded so a later run can retry them.

**Why this priority**: The repair exists because the embedder was unreliable. It has to degrade gracefully when the embedder still is.

**Independent Test**: Run a repair with an embedder that fails for some rows. Confirm that successful rows are repaired and cleared from the record, failed rows are reported in `failed` and remain recorded, and the status count equals the failed count.

**Acceptance Scenarios**:

1. **Given** an embedder that fails for some of the candidates, **When** the tool runs, **Then** the other candidates are repaired, the failed ones are reported in `failed`, and the status count equals the number of failed rows.
2. **Given** a partial repair, **When** the operator runs the tool again after the embedder recovers, **Then** the previously failed rows are repaired and the count reaches 0.
3. **Given** transient embedder errors, **When** the tool embeds candidates, **Then** it applies the same bounded retry and backoff policy that replay uses (#808), so a brief outage does not mark a row failed.

---

### User Story 4 - Minimal index disruption and safe recovery (Priority: P1)

The repair touches database-wide vector indexes. It must disturb them as little as possible, and a failure must never leave search wrong or the service unable to recover.

**Why this priority**: Indexes span all groups and writes are blocked while one is down. A repair that over-reaches or leaves the index broken would turn a one-group fix into a service-wide outage.

**Independent Test**: (a) Run with zero candidates and confirm no index is dropped. (b) Run with candidates of one kind and confirm that only that kind's index is dropped and rebuilt. (c) Search concurrently with a repair and confirm results are correct once it finishes. (d) Force the index rebuild to fail and confirm the next search rebuilds the index.

**Acceptance Scenarios**:

1. **Given** a group with no recorded rows, or a `kinds` filter matching none, **When** the tool runs, **Then** no index is dropped or rebuilt and no write lock is held for repair work.
2. **Given** candidates of only the `fact` kind, **When** the tool runs, **Then** only the fact index is dropped and rebuilt.
3. **Given** a search issued while the repair is running, **When** the repair finishes, **Then** the search (or a repeat of it) returns correct results, including the repaired rows.
4. **Given** that rebuilding an index fails, **When** the repair ends, **Then** the failure is reported, the service's record of whether indexes are built is correct, and the next search rebuilds the indexes as it does today.
5. **Given** the embedder is slow, **When** the tool runs, **Then** all embedding calls complete before any index is dropped, so the index is down only for the writes.

---

### User Story 5 - Admin-only, group-scoped, observable and cancellable (Priority: P3)

The tool is available only through the admin scope, is refused for groups the service is not allowed to write, and reports progress and honors cancellation like the existing backfill.

**Why this priority**: These are the required guard rails for a destructive-adjacent admin operation, but they do not add repair capability.

**Independent Test**: Start the service with `--writable-groups` excluding a group. Call the tool for that group and confirm refusal. Call it for a permitted group with a progress token and confirm progress events, then cancel a run and confirm a clean stop.

**Acceptance Scenarios**:

1. **Given** `--writable-groups` that does not list the group, **When** the tool is called for it, **Then** the call is refused, as other group-scoped admin writes are, with no embedder calls or changes.
2. **Given** a progress token, **When** the tool runs, **Then** it streams progress events before the terminal result.
3. **Given** a running repair is cancelled, **When** it stops, **Then** the database is left consistent: rows already repaired are cleared from the record, the rest stay recorded, and the indexes are left built or safely recoverable.
4. **Given** the MCP-over-stdio tool surface, **When** it is listed, **Then** the tool appears in the admin scope and the total tool count is one higher than before.

---

### Edge Cases

- **Group with no record or an unknown group**: reports zero candidates for every kind and changes nothing.
- **Invalid `kinds` value** (not one of `name`, `fact`, `summary`, `content`): rejected with a clear error before any work, naming the valid values.
- **Recorded row no longer exists** (for example removed by free-form Cypher or a delete shape replay does not recognise, per ADR-0808): the stale record entry is removed and the row is counted in `skipped_no_source`.
- **Recorded row exists but its source text is empty or missing**: nothing can be embedded. It is counted in `skipped_no_source`, not `failed`, and it stays recorded.
- **Embedder unavailable or not configured**: the tool fails the affected rows (or the call, if no embedder is configured at all) without dropping any index.
- **Embedder returns a malformed vector** (wrong dimension, non-finite values): that row is `failed`, not written. A zero or invalid vector is never written as a "repair".
- **Run cancelled or the service stops mid-repair**: no phantom state. A row is cleared from the record only if its new embedding was actually written.
- **Concurrent repairs or other index-maintenance admin operations** (for example the summary backfill or an index rebuild): must not corrupt the index state. The tool serializes with them or refuses, rather than racing.
- **Repair of one group while other groups have recorded rows**: the other groups' rows and records are untouched, although the shared indexes are rebuilt for all groups.
- **Re-run after full success**: zero candidates, so nothing is touched (idempotent).
- **Entity summary vs. name**: `summary` and `name` are independent kinds with separate sources and indexes. Repairing one does not touch the other.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The service MUST provide an admin-scope operation `knowledge_reembed_missing` taking `group_id` (required), `kinds` (optional) and `dry_run` (optional, default false).
- **FR-002**: `kinds` MUST accept any non-empty subset of `name`, `fact`, `summary`, `content`, and default to all four. Any other value MUST be rejected before any work is done.
- **FR-003**: Candidates MUST be selected solely from the durable zero-vector record for `group_id` (from #808), filtered by `kinds`. The tool MUST NOT scan rows or read stored vectors to detect zero vectors.
- **FR-004**: Each candidate MUST be re-embedded from the source text stored on the same row: entity name for `name`, entity summary for `summary`, relationship fact for `fact`, episode content for `content`. This is the same pairing the replay recompute uses.
- **FR-005**: Embedding MUST use the embedder's document role and instruction profile, as replay does (#790), and MUST be batched.
- **FR-006**: Embedding MUST use the same bounded retry, backoff and failure-splitting policy as replay (#808), so that a single un-embeddable text fails only its own row.
- **FR-007**: The result MUST report, per kind, `candidates`, `reembedded`, `failed` and `skipped_no_source`.
- **FR-008**: With `dry_run`, the tool MUST report per-kind `candidates` and MUST make no embedder calls, no writes, no record changes and no index changes.
- **FR-009**: A candidate that is re-embedded and written successfully MUST be removed from the zero-vector record, so the `knowledge_status` `zero_vector_rows` count for the group decreases by that amount.
- **FR-010**: A candidate that fails to embed, or whose write fails, MUST stay in the record and be counted in `failed`. One row's failure MUST NOT abort the repair of the others.
- **FR-011**: A recorded row that no longer exists MUST have its stale record entry removed and be counted in `skipped_no_source`. A recorded row that exists but has no usable source text MUST be counted in `skipped_no_source` and stay recorded.
- **FR-012**: A vector returned by the embedder that is of the wrong dimension, non-finite, or all zeros MUST NOT be written, and its row MUST count as `failed`.
- **FR-013**: Rows not in the record MUST NOT be modified, and their stored embeddings MUST remain byte-identical. The only writes are to the embedding column of recorded rows and to the record itself.
- **FR-014**: All embedding calls for every candidate MUST complete before any vector index is dropped, so an index is down only for the writes and its rebuild.
- **FR-015**: Only the indexes for kinds that have at least one candidate to write MUST be dropped and rebuilt. With zero candidates to write, the tool MUST NOT drop any index or hold the write lock for repair work.
- **FR-016**: The index lifecycle (drop, write under the write lock, rebuild, keep the service's "indexes built" state correct so concurrent searches stay safe) MUST be a single shared implementation used by both this tool and `knowledge_backfill_summary_embeddings`, not a duplicate. The backfill's observable behaviour MUST NOT change.
- **FR-017**: If an index rebuild fails, the tool MUST report the failure, MUST leave the service's "indexes built" state accurate (not built), and the service MUST recover on the next search by rebuilding the indexes as it does today.
- **FR-018**: A search that runs concurrently with a repair MUST NOT return errors or corrupted results. Once the repair finishes, searches MUST return correct results that include the repaired rows.
- **FR-019**: The tool MUST stream progress events when the caller supplies a progress token, and MUST honor cancellation. Cancelling MUST leave the database consistent and the indexes built or recoverable (FR-017). Rows already written MUST be cleared from the record and the rest MUST stay recorded.
- **FR-020**: The tool MUST be registered only in the admin scope of the MCP-over-stdio tool surface, raising the registered tool count by exactly one, with the registry's count assertions updated accordingly.
- **FR-021**: The tool MUST respect `--writable-groups` (#742) exactly as other group-scoped admin writes do, and MUST refuse a group that is not writable before any embedder call or change. This includes `dry_run` if other admin tools apply the policy to dry runs.
- **FR-022**: The tool MUST NOT change the persisted shape of the zero-vector record or the `knowledge_status` fields from #808. It only consumes and updates them.
- **FR-023**: Documentation (README tool/scope tables and the admin-tool reference), the CHANGELOG, and a new ADR named per the repo convention (`docs/adr/0809-<slug>.md`, with its index row) MUST be updated to describe the tool, its index-lifecycle trade-offs, and the sharing of the lifecycle code with the backfill.

### Key Entities *(if applicable)*

- **Zero-vector record entry**: A durable `(group, kind, uuid)` record from #808 that marks a row stored with a fallback zero vector. It is the input for selection and is removed when the row is successfully repaired.
- **Embedding kind**: One of `name` (entity name), `summary` (entity summary), `fact` (relationship fact), `content` (episode content). Each pairs one embedding column with one source text column and one database-wide vector index.
- **Repair result**: Per-kind counters `candidates`, `reembedded`, `failed`, `skipped_no_source`, plus any index-rebuild failure.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: After a full successful repair of a group with recorded rows of all four kinds, 100% of those rows are returned by vector search for their own text, and the group's `zero_vector_rows.total` is 0.
- **SC-002**: After a partial repair, `zero_vector_rows.total` for the group equals the number of rows reported `failed` plus those reported `skipped_no_source` that remain recorded.
- **SC-003**: Rows outside the record have byte-identical embeddings before and after a repair, in 100% of tested cases.
- **SC-004**: A `dry_run` produces zero embedder calls, zero database writes and zero index drops, and its per-kind candidate counts equal those of an immediately following real run.
- **SC-005**: A run with zero candidates drops zero indexes. A run with candidates of N kinds drops and rebuilds exactly N indexes.
- **SC-006**: No index is dropped until the last embedder call for the run has returned (verified by event ordering).
- **SC-007**: After an injected index-rebuild failure, the next search succeeds and returns correct results without manual intervention.
- **SC-008**: A group that is not writable under `--writable-groups` is refused 100% of the time with no embedder calls and no changes.
- **SC-009**: Repairing a group's recorded rows takes time proportional to the number of recorded rows plus one index rebuild per affected kind. It does not scale with the total number of entities, as the unconditional summary backfill does, and it is far shorter than a full WAL rebuild on large groups.
- **SC-010**: The admin tool count increases by exactly one, and the documentation, CHANGELOG and ADR are present.

## Assumptions

- The zero-vector record, its APIs and the `knowledge_status` `zero_vector_rows` field from #808 are merged and available. This issue builds on them and does not alter them.
- The record can be briefly stale: an entry may point at a row that was deleted by a path replay does not recognise. Removing such entries during repair is the intended cleanup (ADR-0808).
- An entry for a row with empty source text stays recorded, because that row cannot be repaired and the operator should still see it. Rows with no source text normally never enter the record. This case is mainly defensive.
- Vectors are a local cache of the embedder's output (ADR-0526), so repair writes derived data in place. It is not a new graph mutation that must be replicated in a WAL. Plan decides the mechanism, as long as FR-013 and FR-019 hold.
- A repair blocks writes service-wide while an index is down. This is accepted, as it is for the existing backfill, and is mitigated by FR-014 and FR-015 rather than eliminated.
- Embedding is attempted with the service's currently configured embedder, model and dimension. A model mismatch behaves as it does for replay and the backfill.
- The tool is invoked manually by an operator or an agent. No automatic or scheduled repair is part of this issue.
- Result naming (`candidates`, `reembedded`, `failed`, `skipped_no_source`) follows the issue text and the existing JSON-RPC conventions. The exact response envelope is decided in Plan.

## Out of Scope *(optional)*

- Re-embedding renamed entities, or detecting that a source text changed since the embedding was computed (ADR-0379 decision 6). This is a possible follow-up.
- Changing `knowledge_backfill_summary_embeddings`' behaviour, beyond refactoring the shared index-lifecycle code.
- Detecting zero vectors by reading stored vectors, or repairing zero-vector rows that are not in the #808 record.
- Reducing the database-wide blast radius of index drops (for example per-group or incremental index updates).
- Automatic or scheduled repair, or any change to replay's retry and recording behaviour.
- Any change to the persisted shape of the record or of `knowledge_status`.

## Source References *(optional)*

- #799 — community report and proposal
- #808 / ADR-0808 — bounded replay retry and durable zero-vector record (the prerequisite)
- #470 — `knowledge_backfill_summary_embeddings`
- ADR-0379 — indexed-column writes and the drop/write/rebuild approach
- #440 / ADR-0440 — recompute on replay; #790 — document role and instruction profile
- ADR-0526 — vectors as a local cache; #742 — `--writable-groups`
