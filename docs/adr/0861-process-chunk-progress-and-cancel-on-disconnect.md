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
  (`cause=client_disconnect`). The cause is read from the shutdown token when the cancel is
  *noticed*, not recorded when it is triggered, so a disconnect cancel that is only noticed after
  SIGTERM is counted as a shutdown stop. Known, accepted race: the counter feeds a "stopped"
  telemetry detail, the request was cancelled either way, and nothing else reads it.
- **Phase frames.** `process_chunk` emits `embed` and `extract` (start-of-phase markers: the two
  run concurrently in one `select!`) and `commit` (sent once the write lock is **held**, so the frame means "the commit has started and
  will finish"; a request still waiting for the lock behind another writer has not sent it and is
  still cancellable), through `EpisodeOpts.progress`; send errors are ignored. `process_document` keeps its own phase
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
- **Disconnect rule: a socket read error, a failed or stalled progress write, or read EOF with
  the peer fully closed.** (A pipelined line that is not valid UTF-8 is a malformed request, not a
  hang-up: the in-flight request finishes and is answered, then the connection closes, as before
  #861.) Read EOF alone cannot tell `close()` from `shutdown(SHUT_WR)`, and treating it as a
  disconnect would silently cancel requests from `socat`, `nc -N` and any client that half-closes
  after sending, which completed before #861 (changed after review). So after EOF the loop asks the
  stream whether the peer can still receive (`PeerProbe`): on a Unix socket a zero-timeout
  `poll(POLLOUT)` that reports `POLLHUP` once the peer has closed fully (Linux sets it when both
  directions are shut down; macOS when the socket can neither send nor receive), and nothing but
  `POLLOUT` after a bare `SHUT_WR`. No byte is written and no input is consumed. If the peer is
  still reachable the request keeps running and is answered, and the loop re-checks every 100 ms so
  a peer that closes fully later is cancelled then (a streaming request's failed frame write also
  covers it). Windows named pipes have no half-close, so EOF there is a disconnect. `POLLHUP`
  reporting is platform-specific, so the half-close and full-close tests drive a real
  `UnixStream` pair and the binary-level `chunk_disconnect` test half-closes a real connection;
  they run on the macOS and Linux CI jobs (the rule was verified locally on macOS only). If a
  platform ever cannot tell the cases apart, the fallback is to cancel a streaming request only
  when its next heartbeat or frame write fails and to keep EOF as a disconnect for token-less ones.
- The connection loop moved to `crates/service/src/connection.rs`, generic over the stream and a
  dispatch closure, so it is unit-tested with `tokio::io::duplex` (where EOF always means gone) and
  a fake dispatcher, and with real Unix sockets for the half-close cases.
- Features `chunk_progress` and `cancel_on_disconnect` are added to the
  [ADR-0854](0854-knowledge-capabilities-protocol-version-and-feature-registry.md) registry. No
  `protocol_version` bump: the change is additive.
- **Heartbeats for streaming callers.** While a cooperative request that sent a
  `_progress_token` is silent (the LLM call between `extract` and `commit`), the connection writes
  `{"type":"progress","phase":"heartbeat"}` every 10 s. This covers `knowledge_process_document` too,
  so existing streaming `process_document` clients can now see a `heartbeat` phase (documented in
  the IPC reference; clients ignore unknown phases). Token-less requests never see a frame.
- **Bounded frame writes.** The dispatch future is polled inline, so a progress or heartbeat
  write that blocks (a client that stopped reading) would stall the request. Each frame write is
  bounded to 5 s; a write that fails or times out is a disconnect (cancel before commit, a started
  commit still finishes), and nothing more is written to that socket.
- **Attached MCP.** `AttachedBackend` drops its socket after 30 s with no line (per-line idle read
  timeout). With cancel-on-disconnect that would cancel every ingest slower than 30 s, and it can
  never be retried to success. So the backend always sends `_progress_token` for the two
  cooperative methods (`process_chunk`/`process_document` are not MCP-streaming methods, so the
  frames are read and discarded), and the heartbeat keeps the idle timer re-armed. A remote that
  genuinely hangs or dies still stops sending frames and times out as before. Otherwise MCP is out
  of scope: it is socket-level behaviour, no tool registry change.

## Consequences

- A timeout-and-retry no longer double-extracts when the disconnect precedes commit; during commit
  the original may still succeed, and the retry is safe by [ADR-0858](0858-unchanged-chunk-resubmission-noop.md).
- A cancelled `process_document` leaves already-written unit episodes pending (invisible, not
  live), exactly as any failed call does ([ADR-0785](0785-document-mode-revisions-and-commit-boundary.md): a failed call
  retires nothing).
- Cancellation is cooperative: an in-flight LLM request is abandoned at the next point, and a
  provider may still bill for it.
- While 8 lines are buffered the reader is not polled, which also pauses disconnect detection.
- Deferred in part 1, resolved in part 2 of #836 (#914): `deadline_ms` and joining a duplicate
  in-flight resubmit shipped, see [ADR-0914](0914-process-deadline-and-inflight-join.md);
  pipelining several requests on one connection was declined, see
  [ADR-0914](0914-declined-pipelining-on-one-connection.md). The disconnect semantics decided
  here are unchanged.
