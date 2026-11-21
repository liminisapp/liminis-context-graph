# Feature Specification: Replay embedding recompute — retry with backoff, and a durable per-group record of zero-vector rows surfaced in knowledge_status

**Feature Branch**: `fabrik/issue-808`
**Created**: 2026-11-19
**Status**: Specified
**Input**: User description: "Replay embedding recompute: retry with backoff, and record zero-vector rows per group in knowledge_status (implements #799)

Community report: #799, items 1 and 2 of its follow-up comment. Companion issue: (b) the `knowledge_reembed_missing` repair tool, which is blocked by this one.

During a long WAL replay, transient embedder errors (timeouts, connection resets, HTTP 5xx/503) make embedding recompute fail. A failed row is stored with a same-dimension zero vector and counted in `ReplayStats::embeddings_recompute_failed`. The row is then invisible to vector search. Two gaps make this worse:

1. **No retry on timeout.** `OaiEmbedder` and the UDS path retry only a broken connection, once. A comment in the code says this was a deliberate decision (#510's spec excluded retry-on-timeout). `embed_batch` is all-or-nothing, so one timeout fails a whole replay window: `LCG_REPLAY_EMBED_WINDOW_SIZE`, default 64 rows, up to 4 vector keys each.
2. **Failures aren't durable or visible.** `embeddings_recompute_failed` lives only in that replay's result. Per-row log details stop after `EMBED_FAILURE_LOG_CAP` (10). `knowledge_status` reports only `embedding_model_status` per group. Operators can't tell that search is degraded, and nothing records *which* rows need repair.

Real-world case (#799): a cold replay of about 80k mutations against a local CPU ONNX embedder ran for about 4 h, with intermittent `error sending request` failures. The cause was a second client competing for the embedder.

Fix 1. Bounded retry with backoff in the replay recompute path: retry a failed `embed_batch` call on transient errors (timeouts, connection reset or broken pipe, HTTP 5xx including 503, and 429 if applicable) with exponential backoff and jitter; bound attempts and backoff with env vars, e.g. `LCG_REPLAY_EMBED_RETRIES` (default ~4) and `LCG_REPLAY_EMBED_BACKOFF_MS` (base ~500 ms, capped), documented in `docs/operations.md`. On retry exhaustion for a multi-row window, split the window and retry the halves recursively, down to a single row, so one bad text fails only its own row. Never retry non-transient errors (4xx other than 429, wrong dimension, non-finite values) or a single-row request that exhausts its retries; count those as failures, as today. Rows count as failed only after retries are exhausted. A retry must never duplicate a row or overwrite an embedding already written successfully. Scope: the replay recompute path, including `knowledge_rebuild_from_wal`'s foreground and background paths; whether live ingest (`add_episode`) shares the policy is for Plan to decide, default replay only. Record the reversal of #510's no-retry-on-timeout decision for this path in an ADR, covering why and what bounds it.

Fix 2. Durable per-group record of zero-vector rows, plus a status count: when replay (or any path that falls back to a zero vector because the embedder failed) stores a zero-vector fallback, record `(kind, row uuid)` for the group in a durable per-group record (Plan chooses the mechanism: a sidecar under the workspace or WAL directory like the identity-set stamp, or a DB table or column; it must survive a restart; a full rebuild should reset it to that rebuild's own failures). Exclude rows that legitimately have no source text (`embeddings_recompute_skipped_no_text`, `embeddings_skip_rows`) — those aren't failures (#526). Add `embeddings_recompute_failed` (or `zero_vector_rows`) under `knowledge_status` `wal_groups.<gid>`, next to `embedding_model_status`, with a per-kind breakdown (`name`, `fact`, `summary`, `content`); it must read 0 when nothing failed, stay accurate past the 10-row log cap, be additive and backward compatible, and add no locks that lengthen `knowledge_status` blocking during a rebuild. Expose a small API on the record (list, remove) so issue (b) can select rows to repair and decrement the count.

Acceptance criteria: (1) replay against a test embedder that times out or returns 503 intermittently, then succeeds within the retry budget, finishes with `embeddings_recompute_failed == 0` and no zero vectors stored; (2) one 'poison' text that always fails, in a 64-row window, results in exactly that one row failing, the others get real embeddings; (3) when retries are exhausted, the failure count and per-kind breakdown appear in `knowledge_status`, are correct when more than 10 rows fail, and survive a restart; (4) non-transient errors are not retried, and a test asserts the attempt count; (5) env vars are validated at startup (an invalid value names itself, as with #737's vars) and documented; (6) the existing replay, #440 cache and #526 skip tests still pass; (7) CHANGELOG `Unreleased` entry and an ADR.

Out of scope: the repair tool itself (issue (b)); re-embedding renamed entities (ADR-0379 decision 6); changing how live ingest handles embedder failure, unless Plan argues for it."

## Background

During a long WAL replay, transient embedder errors (timeouts, connection resets, HTTP 5xx/503, possibly 429) cause embedding recompute to fail. A failed row is stored with a same-dimension zero vector and counted in the replay's `embeddings_recompute_failed` statistic. A zero-vector row is invisible to vector search, so search quality silently degrades.

Two gaps make this worse:

1. **No retry on timeout.** The embedder clients retry only a broken connection, once. That was a deliberate decision in #510 (its spec excluded retry-on-timeout). Because batch embedding is all-or-nothing, a single timeout fails a whole replay window (default 64 rows, up to 4 vector keys each).
2. **Failures are neither durable nor visible.** The failure count lives only in the result of the replay that produced it. Per-row log lines stop after a cap of 10. `knowledge_status` exposes only `embedding_model_status` per group, so operators cannot tell that search is degraded, and nothing records *which* rows need repair.

The real-world trigger (#799) was a cold replay of ~80k mutations against a local CPU ONNX embedder that ran ~4 h with intermittent `error sending request` failures, caused by a second client competing for the embedder. This issue implements items 1 and 2 of that report's follow-up. A companion issue (b), a `knowledge_reembed_missing` repair tool, depends on the record created here.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Replay survives transient embedder failures (Priority: P1)

An operator replays a large WAL (cold rebuild or recovery) against an embedder that intermittently times out, resets connections, or returns 503/429. The replay retries transient failures with bounded exponential backoff, so rows are embedded properly instead of being stored as zero vectors.

**Why this priority**: This is the root cause of silent search degradation in #799; fixing it prevents most zero-vector rows from ever being created.

**Independent Test**: Replay a WAL against a test embedder that fails the first N attempts of some requests with a timeout or 503 and then succeeds within the retry budget; assert `embeddings_recompute_failed == 0` and that no stored embedding is a zero vector.

**Acceptance Scenarios**:

1. **Given** an embedder that times out intermittently but succeeds within the retry budget, **When** a replay runs, **Then** no rows are counted as failed and no zero vectors are stored.
2. **Given** an embedder returning HTTP 503 for the first attempts of a window, **When** the replay retries with backoff, **Then** the window is embedded successfully and each row is stored exactly once.
3. **Given** an embedder that returns a 4xx other than 429, a wrong-dimension vector, or non-finite values, **When** the replay processes the affected request, **Then** it is attempted once, not retried, and counted as a failure as it is today.
4. **Given** invalid values for the retry/backoff settings, **When** the service starts, **Then** startup fails with an error naming the offending setting (consistent with the #737 settings).

---

### User Story 2 - A poison text fails only its own row (Priority: P1)

When retries for a multi-row window are exhausted, the window is split and the halves retried recursively down to a single row, so one text the embedder can never process does not take 63 healthy neighbours down with it.

**Why this priority**: Without isolation, one bad text costs a whole window of embeddings; this is the main blast-radius reduction beyond plain retry.

**Independent Test**: Replay a 64-row window containing exactly one text that always fails; assert exactly one row is counted as failed and the other 63 rows have real embeddings.

**Acceptance Scenarios**:

1. **Given** a 64-row window with one always-failing text, **When** the replay runs, **Then** exactly that row is counted failed (zero-vector stored, recorded) and all other rows get real embeddings.
2. **Given** a single-row request that exhausts its retries, **When** the replay processes it, **Then** it is not retried further or split, and is counted as a failure.
3. **Given** a split window where one half succeeds on retry, **When** the other half later fails, **Then** the successfully written embeddings from the first half are not overwritten or duplicated.

---

### User Story 3 - Operators can see how many rows are degraded, and which (Priority: P1)

After a replay in which retries were exhausted for some rows, an operator calls `knowledge_status` and sees, per group, how many rows are stored with a fallback zero vector, broken down by kind (`name`, `fact`, `summary`, `content`). The count is accurate regardless of how many rows failed, survives a service restart, and reads 0 when nothing failed. The underlying record also identifies the specific rows so a repair tool can act on them.

**Why this priority**: Without visibility and a durable record, degraded search is undetectable and unrepairable; issue (b) is blocked on this.

**Independent Test**: Run a replay with more than 10 permanently failing rows across several kinds; assert `knowledge_status` `wal_groups.<gid>` shows the exact total and per-kind counts, restart the service, and assert the same values.

**Acceptance Scenarios**:

1. **Given** a replay with 25 rows that fail after retries, **When** `knowledge_status` is called, **Then** the group reports 25 with a per-kind breakdown summing to 25 (not capped at 10).
2. **Given** a restart after such a replay, **When** `knowledge_status` is called, **Then** the same counts are reported.
3. **Given** a replay where nothing failed, **When** `knowledge_status` is called, **Then** the count is 0 (field present) and every per-kind count is 0.
4. **Given** rows skipped because they have no source text or are in the embedding-skip set, **When** `knowledge_status` is called, **Then** they are not included in the count.
5. **Given** a group with recorded failures, **When** a full rebuild of that group runs, **Then** the record afterwards reflects only that rebuild's own failures.
6. **Given** a rebuild in progress, **When** `knowledge_status` is called, **Then** its blocking time is not lengthened by the new field.

---

### User Story 4 - Repair tooling can consume the record (Priority: P2)

The follow-up repair tool (issue (b)) can list the recorded `(kind, row uuid)` entries for a group and remove entries once repaired, which decrements the reported count.

**Why this priority**: Not user-visible by itself, but it is the contract the blocked companion issue depends on.

**Independent Test**: Unit-test the record API: add entries, list them (all and by kind), remove one, and confirm the status count drops by one and the removal persists across restart.

**Acceptance Scenarios**:

1. **Given** a record with entries, **When** the list operation is called, **Then** it returns every `(kind, uuid)` entry for the group.
2. **Given** a recorded entry, **When** it is removed, **Then** it no longer appears in the list and the status count decreases by one.
3. **Given** a request to remove an entry that is not recorded, **When** the removal runs, **Then** it is a no-op and not an error.

---

### Edge Cases

- The embedder returns a different dimension or non-finite values for some rows: non-transient, no retry, counted as failed and recorded.
- Retry budget exhausted for a window of size 1: counted as failed immediately, no further splitting.
- A window is split and one half is retried while the other has already been written: no duplicated rows, no overwriting of embeddings already written successfully.
- Process stops (crash or shutdown) mid-replay after some failures were recorded: the recorded entries must still match rows actually stored with a zero vector after restart (no phantom entries for rows never written, no missing entries for rows written as zero).
- Rows with no source text, or in the embedding-skip set (#526), are never recorded as failures.
- The #440 embedding cache: cached hits are not embedder calls and must not be retried or recorded as failures; a failed row must not poison the cache with a zero vector.
- An incremental (non-full) replay or recovery that later embeds a previously-failed row successfully: the stale entry must not keep the row counted as degraded.
- Backoff must be bounded: total retry time per request cannot exceed the configured attempts times the capped backoff; very large configured values are rejected or clamped by validation.
- Existing consumers of `knowledge_status` that ignore unknown fields continue to work unchanged.
- Background rebuild and foreground rebuild both apply the same retry policy and the same recording.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: During WAL replay embedding recompute (including `knowledge_rebuild_from_wal` in both its foreground and background paths), a failed embedding request MUST be retried when the error is transient: timeout, connection reset/broken pipe, HTTP 5xx (including 503), and HTTP 429.
- **FR-002**: Retries MUST use exponential backoff with jitter, bounded by a configurable maximum attempt count (default about 4 retries) and a configurable base backoff (default about 500 ms) with a cap, so total time per request is bounded.
- **FR-003**: The retry count and backoff settings MUST be configurable via environment variables (proposed `LCG_REPLAY_EMBED_RETRIES` and `LCG_REPLAY_EMBED_BACKOFF_MS`), validated at service startup with an error that names the offending variable, consistent with the #737 settings, and documented with their defaults in the operations documentation.
- **FR-004**: When a multi-row window exhausts its retries, the system MUST split the window and retry the halves recursively down to a single row, so a single always-failing text fails only its own row.
- **FR-005**: Non-transient errors (HTTP 4xx other than 429, wrong dimension, non-finite values) MUST NOT be retried and MUST be counted as failures as they are today; a single-row request that exhausts its retries MUST NOT be retried or split further and MUST be counted as a failure.
- **FR-006**: A row MUST be counted in `embeddings_recompute_failed` and stored with a same-dimension zero vector only after retries (and splitting) are exhausted or a non-transient error occurs.
- **FR-007**: Retrying or splitting MUST NOT duplicate any row or overwrite an embedding that was already written successfully.
- **FR-008**: Whenever the replay recompute path (or any other path that falls back to a zero vector because the embedder failed) stores a zero-vector fallback, the system MUST record `(kind, row uuid)` for that group in a durable per-group record that survives service restart.
- **FR-009**: Rows that legitimately have no source text (`embeddings_recompute_skipped_no_text`) or are in the embedding-skip set (`embeddings_skip_rows`) MUST NOT be added to the record (#526).
- **FR-010**: A full rebuild of a group MUST reset that group's record so that, afterwards, it contains only the failures of that rebuild.
- **FR-011**: When a previously recorded row is subsequently embedded successfully or removed by any path, its entry MUST NOT remain in the record as a stale failure.
- **FR-012**: `knowledge_status` MUST report, under `wal_groups.<gid>` alongside `embedding_model_status`, the number of zero-vector fallback rows (proposed field name `embeddings_recompute_failed`) with a per-kind breakdown for `name`, `fact`, `summary` and `content`.
- **FR-013**: The reported count MUST be 0 (field present, all per-kind counts 0) when nothing has failed, MUST be exact regardless of the per-row log cap (10), and MUST be identical after a service restart.
- **FR-014**: The new status fields MUST be additive and backward compatible, and reading them MUST NOT add locking that lengthens `knowledge_status` blocking time during a rebuild.
- **FR-015**: The record MUST expose a small programmatic API to list entries (for a group, optionally by kind) and to remove entries, such that removing an entry decrements the reported count; this API is the contract for the follow-up repair tool (issue (b)).
- **FR-016**: Existing replay behaviour covered by current tests, including the #440 embedding cache and #526 skip semantics, MUST be unchanged except for the added retry, splitting and recording.
- **FR-017**: The decision to reverse #510's "no retry on timeout" for the replay path MUST be recorded in an ADR stating why and what bounds it (attempt count, backoff cap, window splitting), and the change MUST have a CHANGELOG `Unreleased` entry.

### Key Entities *(if applicable)*

- **Zero-vector record (per group)**: A durable set of `(kind, row uuid)` entries for rows stored with a fallback zero vector because the embedder failed. Kinds: `name`, `fact`, `summary`, `content`. Supports list and remove and is the source of the status counts.
- **Retry policy**: The bounded attempt count, base backoff and backoff cap governing replay embedding retries.
- **Replay embedding window**: A batch of rows (default 64, up to 4 vector keys each) embedded in one request; splittable on retry exhaustion.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Against an embedder that fails intermittently with timeouts or 503s but succeeds within the retry budget, a replay finishes with `embeddings_recompute_failed == 0` and zero stored zero vectors.
- **SC-002**: With one permanently failing text in a 64-row window, exactly 1 row is counted as failed and the other 63 receive real embeddings.
- **SC-003**: After retry exhaustion on more than 10 rows, `knowledge_status` reports the exact total and per-kind counts, and the same values after a restart.
- **SC-004**: For non-transient errors, a test asserts exactly one attempt per request (zero retries).
- **SC-005**: An invalid retry/backoff setting prevents startup with a message naming that setting; valid defaults and ranges are documented.
- **SC-006**: The existing replay, #440 embedding-cache and #526 skip test suites pass unchanged.
- **SC-007**: A group with no failures reports 0 for the total and every per-kind count, and `knowledge_status` blocking time during a rebuild is not measurably lengthened.
- **SC-008**: An ADR and a CHANGELOG `Unreleased` entry exist for the change.

## Assumptions

- Scope is the replay recompute path only (including foreground and background `knowledge_rebuild_from_wal`). Live ingest (`add_episode`) keeps its current embedder-failure behaviour unless Plan argues for sharing the policy; if Plan extends it, the record must still capture any zero-vector fallbacks that path produces.
- The storage mechanism for the record (sidecar file vs. database table/column) is a Plan decision; the requirement is durability across restart, atomic consistency with the stored zero-vector rows, and no added blocking in `knowledge_status`.
- Default of about 4 retries and a ~500 ms base backoff with a cap are starting values; Plan may tune them, provided they are documented and bounded.
- HTTP 429 is treated as transient; if the embedder supplies a retry-after hint, honouring it is optional and bounded by the same cap.
- The new status field is named `embeddings_recompute_failed` (matching the existing replay statistic); Plan may choose `zero_vector_rows` if it avoids confusion, so long as it is additive and documented.
- Failures from the replay's cache-hit path (#440) are not possible, since no embedder call is made; only embedder-served rows are eligible for recording.
- Removing an entry from the record via the API does not itself re-embed anything; repair is issue (b).

## Out of Scope *(optional)*

- The repair tool itself (`knowledge_reembed_missing`, issue (b)).
- Re-embedding renamed entities (ADR-0379 decision 6).
- Changing how live ingest handles embedder failure, unless Plan argues for it.
- Fixing the root cause of embedder contention in the reporter's deployment (#799's second client competing for the embedder).

## Source References *(optional)*

- #799 — community report (items 1 and 2 of its follow-up comment)
- #510 — prior decision excluding retry-on-timeout
- #526 — skip semantics for rows with no source text
- #440 — embedding cache
- #737 — startup validation of environment settings
- ADR-0379 — decision 6 (renamed entities)
