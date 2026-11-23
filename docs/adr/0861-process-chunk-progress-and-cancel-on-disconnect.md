# ADR-0861: `process_chunk` Progress and Cancel-on-Disconnect Before Commit

**Status**: Accepted
**Date**: 2026-11-23
**Issue**: #861 (part 1 of #836)

## Context

Three gaps made slow chunk ingest awkward:

1. `knowledge_process_chunk` took no `_progress_token` ([ADR-0005](0005-streaming-ipc-progress-framing.md)
   wired streaming only for rebuild, reprocess and `process_document`).
2. A non-streaming request ran inline in the connection loop, so nothing watched the socket: a
   client that timed out and retried on a new connection made the service extract the chunk twice.
   A streaming request noticed a disconnect only when a progress write failed, and then
   `abort()`ed the dispatch task, which can drop Phase C's write guard while the blocking commit
   closure keeps running.
3. The one-request-at-a-time-per-connection model was never documented.

The cancellation points already existed in `add_episode_with_opts` (the last one is the write-lock
`select!`), but all read the process-wide shutdown token.

## Decision

- **Per-request token, reusing the existing mechanism.** The connection layer gives each request a
  `state.cancel_token.child_token()`, carried to `add_episode_with_opts` in `EpisodeOpts.cancel`
  and to `process_document` as a parameter. Every pre-commit check reads it instead of the global
  token, so shutdown still cascades and a disconnect cancels only one request. Callers that pass
  none (the library, `cancel_shutdown` tests) fall back to `state.cancel_token`.
  `dispatch_cancellable` sits beside `dispatch`, which keeps its signature.
- **Cause attribution.** `cancelled_chunks` (the shutdown "stopped" telemetry) is incremented only
  when the *shutdown* token is cancelled; a disconnect is logged by the connection layer
  (`cause=client_disconnect`).
- **Phase frames.** `process_chunk` emits `embed` and `extract` (start-of-phase markers: the two
  run concurrently in one `select!`) and `commit` (immediately before the write-lock `select!`),
  through `EpisodeOpts.progress`; send errors are ignored. `process_document` keeps its own phase
  names and its units emit no chunk frames.
- **Never tear commit.** No cancel check is added at or after lock acquisition, and the lock arm
  of that `select!` is `biased;` and first: a write that can proceed always finishes, which makes
  "commit has started" deterministic.
- **Connection loop watches the socket for two methods only** (`knowledge_process_chunk`,
  `knowledge_process_document`; `COOPERATIVE_CANCEL_METHODS`). The dispatch future is polled
  inline in a `select!` with the progress receiver and `next_line()` (cancel-safe); lines read
  meanwhile are buffered (cap 8) and served afterwards, so pipelined input is never lost. On a
  disconnect it cancels the token, **awaits the future to completion** (never drops it), discards
  the response and returns, so queued requests never start. All other methods, including the
  other streaming ones, keep the previous path, `abort()` included.
- **Disconnect rule: read EOF, a read error or a failed progress write.** A plain read cannot tell
  `close()` from `shutdown(SHUT_WR)` on a generic stream, and a client's `close()` after a timeout
  is exactly a clean EOF, so the feature cannot work otherwise; `POLLRDHUP`-style tricks are not
  portable to macOS or Windows named pipes. This deliberately departs from the spec's assumption
  that a half-close is not a disconnect. Consequence: a client that half-closes after sending a
  `process_chunk`/`process_document` request has it cancelled, e.g. `printf ... | nc -U`. The IPC
  reference says to keep the write side open until the response arrives.
- The connection loop moved to `crates/service/src/connection.rs`, generic over the stream and a
  dispatch closure, so it is unit-tested with `tokio::io::duplex` and a fake dispatcher.
- Features `chunk_progress` and `cancel_on_disconnect` are added to the
  [ADR-0854](0854-knowledge-capabilities-protocol-version-and-feature-registry.md) registry. No
  `protocol_version` bump: the change is additive.
- MCP is out of scope; attached MCP inherits the socket behaviour.

## Consequences

- A timeout-and-retry no longer double-extracts when the disconnect precedes commit; during commit
  the original may still succeed, and the retry is safe by [ADR-0858](0858-unchanged-chunk-resubmission-noop.md).
- A cancelled `process_document` leaves already-written unit episodes pending (invisible, not
  live), exactly as any failed call does ([ADR-0785](0785-document-mode-revisions-and-commit-boundary.md): a failed call
  retires nothing).
- Cancellation is cooperative: an in-flight LLM request is abandoned at the next point, and a
  provider may still bill for it.
- While 8 lines are buffered the reader is not polled, which also pauses disconnect detection.
- Deferred (part 2 of #836): `deadline_ms`, joining a duplicate in-flight resubmit, and
  pipelining or concurrent handling of several requests on one connection.
