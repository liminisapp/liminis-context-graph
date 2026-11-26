# ADR-0914: `process_chunk` / `process_document` Deadline, and Joining a Duplicate In-flight Resubmit

**Status**: Accepted
**Date**: 2026-11-25
**Issue**: #914 (part 2 of #836; the pipelining decision is [ADR-0914 (declined-pipelining)](0914-declined-pipelining-on-one-connection.md))

## Context

[ADR-0861](0861-process-chunk-progress-and-cancel-on-disconnect.md) shipped progress, cancel-on-disconnect
before commit and the documented one-request-at-a-time connection model, and deferred three things.
This ADR settles two of them.

1. **No deadline.** A client that can only wait N seconds had to time out and disconnect. That cancels
   the work, but the client never received an answer saying "abandoned before anything was written".
2. **A duplicate resubmit extracts again.** A client that retries the same `(group_id, chunk_id)` on a
   new connection while the first attempt is still running paid for a second embed and LLM extraction.
   [ADR-0858](0858-unchanged-chunk-resubmission-noop.md)'s no-op only helps once the first attempt has
   *committed*; while one is in flight, the service only registered it so the no-op never fires
   underneath it.

## Decision

### Deadline (`deadline_ms`)

- **Parameter.** Optional `deadline_ms` on `knowledge_process_chunk` and `knowledge_process_document`:
  a positive integer number of milliseconds. Zero, negatives, floats and non-numbers are rejected with
  the usual validation error (`-32000`) before any registration or work. A value too large to represent
  as an `Instant` means no deadline.
- **Clock origin: handler entry.** The clock starts when the handler has validated the request, not
  when the connection read the line. The handler is the only layer common to the socket, standalone
  MCP and attached MCP, and it already receives the request's cancellation token. The cost is that a
  line queued behind another request on the same connection (the 8-line buffer of ADR-0861) does not
  count its queue time. Documented in the reference.
- **Mechanism: a child token, no new cancellation points.** `RequestDeadline` (`crates/core/src/deadline.rs`)
  hands the handler a child of the request's token and a timer task that, when the deadline passes,
  records that it fired and cancels the child. The child replaces the parent in `EpisodeOpts.cancel` /
  `process_document`, so the existing ADR-0861 cancellation points enforce the deadline unchanged. The
  timer is aborted on drop, so it never fires after the request ends.
- **Cause attribution.** When the work returns `Error::Cancelled`, the handler classifies it: it is
  `DeadlineExceeded` only if the timer fired **and the parent token is not cancelled**. A disconnect or
  shutdown that is also in effect is reported as the cancellation it is. `note_cancelled` reads only
  the shutdown token, so a deadline never inflates `cancelled_chunks`.
- **Never tear commit.** No check is added at or after write-lock acquisition. After the work returns
  `Ok`, the handler compares the clock with the deadline: if it has passed, the success body gains
  `"deadline_exceeded": true`. The key is absent otherwise. An `unchanged` no-op returns before any
  cancellation point, so it is never flagged.
- **Error shape.** Code `-32006`, `error.data = {"reason": "deadline_exceeded", "phase": …,
  "committed": false}`. `process_document` adds `"partial": true` (units already committed stay,
  nothing is retired, [ADR-0785](0785-document-mode-revisions-and-commit-boundary.md)).
- **Phase vocabulary.** Chunk: `embed`, `extract` (everything through resolution and dedup, up to the
  lock wait), `awaiting_write_lock` (the lock wait, still before commit) and, for a joined request,
  `awaiting_inflight`. Progress frames are unchanged: `awaiting_write_lock` and `awaiting_inflight`
  appear only in errors. Document: `chunked`, `extracting`. The `committing` progress frame
  is sent after the last cancellation check, once the revision commit has begun, so a
  `deadline_exceeded` error never reports it.
- **Known late notice.** Points with no cancellation point (the per-document lock, the unchanged-check
  read lock, an in-flight LLM call between points) notice a deadline late. Same accepted limitation as
  disconnect cancel; provider billing for a request already sent is not recoverable.

### Joining a duplicate in-flight `process_chunk`

- **When a request joins.** A `process_chunk` with `supersede` true and no `same_revision_episode_uuids`
  arrives for a `(group_id, chunk_id)` that has an in-flight *leader* with an identical request, and
  the leader is the only in-flight registration for that key. "Identical" is the ADR-0858 comparison
  applied request-to-request: text, attributes as parsed JSON, resolved origin, `source_file`,
  `doc_id` and the explicit `reference_time`. `reference_time`: both omitted, or the same explicit
  instant, join; omitted versus explicit does not (the omitted one takes "now" at its own start).
  `ChunkFingerprint::from_view` destructures `ChunkRequestView` exhaustively, so a field added to the
  ADR-0858 view cannot silently be left out of join identity.
- **What does not join.** Different content, a different group, `supersede=false`,
  `same_revision_episode_uuids`, `knowledge_mark_chunk_removed`, `process_document`, and any case where
  a second registration (a differing edit or a removal) is also in flight. These keep today's
  behaviour, so commit-instant ordering ([ADR-0705](0705-belief-time-separate-from-validity.md)) still
  decides them. Requiring `chunk_inflight_count == 1` is what preserves that: joining while a differing
  request was in flight could reorder commits (an A, C, D sequence ending on C).
- **Registry.** A process-wide static keyed by `(AppState address, group_id, chunk_id)` in
  `crates/core/src/chunk_join.rs`, like `doc_locks()` in `document/ingest.rs`, rather than an
  `AppState` field: about a hundred test and bench files construct `AppState` literally. Election is one
  critical section (registry lock, then `chunk_inflight`'s). **Followers do not register in
  `chunk_inflight`**, so `move_document`, `retire_document`, `mark_chunk_removed` and the no-op's
  `alone` rule see exactly what they saw before.
- **Result sharing.** The leader publishes its success body (without `duration_seconds`, `joined`,
  `deadline_exceeded` or `warning`) on a `watch` channel. Each follower rebuilds its own per-request
  fields: `duration_seconds`, `joined: true`, its own `deadline_exceeded` and its own oversize
  `warning`. `ChunkTextOversized` telemetry stays leader-only. A follower of a leader that ended
  `unchanged` gets that result plus `joined`.
- **If the first request is cancelled (disconnect, deadline, shutdown, panic): restart, not takeover.**
  The leader publishes nothing, and its handle's `Drop` removes the entry and then publishes
  `Abandoned`. Each follower goes back through the election: one becomes the new leader and the rest
  join it. A follower never inherits the cancellation, and the extractor still runs at most once per
  surviving leader. Takeover (a spawned, ref-counted-cancel task detached from connection polling)
  would preserve the cancelled leader's partial extraction, but it detaches work from the connection
  that ADR-0861 awaits to completion, and the saving is only the abandoned portion of one extraction.
  Restart is the smaller design and keeps every request's work inside its own connection task.
- **If the first request fails for another reason: share the failure.** Followers receive the same
  error with its structured code (`Error::Shared`, looked through by `dispatch`). Retrying would turn a
  deterministic failure (an extractor error) into N serial extractions.
- **No lost wake-up, no hang.** Remove-then-publish in `Drop` means every follower sees an outcome even
  when the leader's future is dropped, aborted at shutdown or panics (`Abandoned`). A follower waits in
  a `select!` against its own token, so its own deadline or disconnect (`deadline_exceeded` with phase
  `awaiting_inflight`, or `Cancelled`) applies only to it; the shutdown token cancels followers too.
- **Heartbeats, not phase frames.** A follower with a `_progress_token` receives the connection layer's
  10 s heartbeats (so a per-line idle timeout, including attached MCP's 30 s, does not drop it) but no
  `embed`/`extract`/`commit` frames; the leader's frames belong to the leader's connection.

### Capabilities, MCP, contract

- Feature strings `process_deadline` and `chunk_join_inflight`
  ([ADR-0854](0854-knowledge-capabilities-protocol-version-and-feature-registry.md)); additive, so
  `protocol_version` stays 1.
- MCP: `deadline_ms` is added to both tools' input schemas and descriptions. Standalone MCP calls
  `dispatch`, so the handler-owned deadline works with no wiring; attached MCP forwards params and
  `error.data` verbatim, so the new parameter and error pass through.
- `contract/knowledge_process_chunk.schema.json` gains optional `joined` (both shapes) and
  `deadline_exceeded` (ingested shape).

## Consequences

- A deadline-driven client can retry safely: a `deadline_exceeded` error means nothing was written, and
  a retry that arrives while another is still running joins it instead of extracting twice.
- ADR-0858's statement that two identical resubmissions that arrive together both take the ordinary
  path is superseded in part: they now join when the first is joinable and alone in flight. A
  resubmission that arrives while a differing request is also in flight still takes the ordinary path.
- A cancelled leader's partial extraction is discarded; a surviving follower extracts afresh.
- A leader failure is shared, including a transient one; the client retries as it would after any error.
- Joined responses report their own `duration_seconds`, which includes the wait.
- Logs: `process_chunk joined in-flight request` and `join leader abandoned; re-electing` on stderr. No
  new telemetry event.
