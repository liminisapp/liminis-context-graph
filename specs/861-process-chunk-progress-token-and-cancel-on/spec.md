# Feature Specification: process_chunk progress token and cancel-on-disconnect before commit; document per-connection concurrency

**Feature Branch**: `fabrik/issue-861`
**Created**: 2026-11-23
**Status**: Specified
**Input**: User description: "Implements #836 (part 1; part 2 is deferred, see below).

## Problem
- `knowledge_process_chunk` accepts no `_progress_token`. Streaming is wired only for `process_document`, rebuild and reprocess.
- A non-streaming request is **not cancelled when the client disconnects**, because `handle_connection` awaits dispatch to completion (`crates/service/src/main.rs` ~53–97). A client that times out and retries on a new connection makes LCG extract the same chunk twice.
- Each connection runs one request at a time, and the docs never say so. A search sent on the same connection as a slow ingest waits behind it.

## Change
- **Progress.** `process_chunk` accepts `_progress_token` and emits phase notifications (`embed`, `extract`, `commit`), using the same mechanism and shape as `process_document`.
- **Cancel on disconnect.** When the connection drops, cancel a non-streaming `process_chunk` (and `process_document`, if it doesn't already cancel) at the next cancellation point **before commit**. Once commit has started, let it finish. Reuse the existing cancellation plumbing (`cancel_shutdown` tests). Log the cancellation.
- **Document concurrency.** In the IPC reference, say that requests on one connection are handled one at a time, and that clients wanting parallel ingest or reads alongside ingest should open several connections. Give the guidance on timeouts too.
- Add `chunk_progress` and `cancel_on_disconnect` to the features list from #854.

## Deferred (part 2 of #836, not in this issue)
`deadline_ms`, joining a duplicate in-flight resubmit, and pipelining several requests on one connection.

## Acceptance
- With a progress token, a client receives the phases in order.
- Disconnecting during extraction (a slow mock extractor) results in no commit, no WAL record and no episode, and the server stays healthy.
- Disconnecting during commit lets the write finish."

## Background

Ingest clients talk to the service over a Unix socket. Three gaps make slow chunk ingestion awkward and wasteful today:

1. **No progress for single-chunk ingest.** `knowledge_process_document`, rebuild and reprocess can stream phase progress when the caller supplies `_progress_token`. `knowledge_process_chunk` — the most commonly used ingest call, and one that can run for a long time because it embeds and runs LLM extraction — cannot, so a caller sees nothing until it finishes.
2. **Abandoned requests keep running.** A non-streaming request is not cancelled when its client disconnects, because the connection handler waits for the request to finish before it looks at the socket again. The common failure: a client times out, drops the connection, and retries on a new one. The service then extracts the same chunk twice, paying for LLM work whose result nobody is waiting for.
3. **The concurrency model is undocumented.** Each connection handles one request at a time. A search sent on the same connection as a slow ingest silently queues behind it. Integrators have no documented guidance on opening several connections or on choosing timeouts.

This is part 1 of #836. Part 2 (`deadline_ms`, joining a duplicate in-flight resubmit, pipelining several requests on one connection) is deliberately deferred. This issue follows the `knowledge_capabilities` feature registry from #854, where each shipped capability adds its own feature string.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Stream phase progress from process_chunk (Priority: P1)

An ingest client sends `knowledge_process_chunk` with a `_progress_token` and receives progress notifications for the embed, extract and commit phases before the terminal result, in the same frame shape as `knowledge_process_document`.

**Why this priority**: It is the lowest-risk, user-visible piece and the foundation for the cancellation story: the phase boundaries it defines are what "before commit" refers to.

**Independent Test**: Send `knowledge_process_chunk` with a `_progress_token` against a mock extractor and assert that progress frames for `embed`, `extract` and `commit` arrive in that order, followed by the normal terminal result. Send the same request without a token and assert the response is unchanged.

**Acceptance Scenarios**:

1. **Given** a connected client, **When** it sends `knowledge_process_chunk` with a `_progress_token`, **Then** it receives progress frames for the phases in order (`embed`, `extract`, `commit`) and then the terminal result.
2. **Given** a request without `_progress_token`, **When** `knowledge_process_chunk` runs, **Then** no progress frames are sent and the response is identical to today's.
3. **Given** a chunk that is an unchanged no-op or fails validation before any work starts, **When** it is sent with a token, **Then** the client still receives a well-formed terminal response (result or error), with no progress frame for a phase that never ran.

---

### User Story 2 - Cancel an abandoned ingest before commit (Priority: P1)

When a client disconnects while its `knowledge_process_chunk` (or `knowledge_process_document`) is still working, the service stops at the next cancellation point before commit, leaving no trace of the abandoned write. If commit has already started, it completes.

**Why this priority**: This removes duplicate LLM extraction on timeout-and-retry, which is the main cost problem, and it must not risk partial writes.

**Independent Test**: With a deliberately slow mock extractor, send `knowledge_process_chunk` without a token, close the socket during extraction, and assert no episode, no WAL record and no commit; assert the service still answers `health_check` and a following request on a new connection succeeds. Repeat with a disconnect during commit and assert the write completes.

**Acceptance Scenarios**:

1. **Given** a non-streaming `knowledge_process_chunk` in the extraction phase, **When** the client disconnects, **Then** the request is cancelled before commit, nothing is written to the graph or WAL, no episode is created, and the service stays healthy.
2. **Given** a `knowledge_process_chunk` whose commit has started, **When** the client disconnects, **Then** the commit runs to completion and the episode and WAL record exist.
3. **Given** a client that timed out and retries the same chunk on a new connection after the first one disconnected during extraction, **When** the retry runs, **Then** it extracts and commits once; the abandoned first attempt left no state behind.
4. **Given** a `knowledge_process_document` request, **When** the client disconnects, **Then** it is cancelled at the next pre-commit cancellation point with the same guarantees, unless it already cancels on disconnect today, in which case existing behaviour is preserved.
5. **Given** any cancellation caused by a disconnect, **When** it happens, **Then** the service logs the cancellation, identifying the method and that the cause was client disconnect.

---

### User Story 3 - Documented per-connection concurrency (Priority: P2)

An integrator reading the IPC reference learns that requests on one connection are handled one at a time, that parallel ingest or reads alongside ingest need multiple connections, and how to choose timeouts and recover from a timeout.

**Why this priority**: Documentation only, no behaviour change, but it prevents the head-of-line surprise that prompted the issue.

**Independent Test**: Read the IPC reference and confirm it states the one-at-a-time rule, the multiple-connection guidance, the timeout guidance, and the cancel-on-disconnect behaviour including the "commit, once started, finishes" caveat.

**Acceptance Scenarios**:

1. **Given** the IPC reference, **When** an integrator looks up connection behaviour, **Then** it states that requests on a single connection are processed sequentially and a search behind a slow ingest waits.
2. **Given** that section, **When** an integrator wants parallel ingest or reads alongside ingest, **Then** it tells them to open several connections.
3. **Given** that section, **When** an integrator picks a client timeout, **Then** it explains that disconnecting cancels the in-flight request before commit (so retrying on a new connection does not double-extract unless commit had begun), and that a request which timed out during commit may still have succeeded.

---

### User Story 4 - Advertise the new capabilities (Priority: P2)

A client calling `knowledge_capabilities` can tell whether the server supports `process_chunk` progress and cancel-on-disconnect by checking for the `chunk_progress` and `cancel_on_disconnect` feature strings.

**Why this priority**: It lets clients adopt the behaviour safely against older servers, per the #854 registry convention.

**Independent Test**: Call `knowledge_capabilities` and assert both strings are present and the list remains sorted, unique and well-formed.

**Acceptance Scenarios**:

1. **Given** a server with this change, **When** a client calls `knowledge_capabilities`, **Then** `features` contains `chunk_progress` and `cancel_on_disconnect`.
2. **Given** an older server without them, **When** a client checks the list, **Then** absence signals that the behaviour is unsupported.

---

### Edge Cases

- A client disconnects after the terminal response was already written: no cancellation, no error logged.
- A client disconnects exactly at the extract-to-commit boundary: either outcome is acceptable only if it is all-or-nothing — either no write at all, or a fully committed write; never a partial one.
- A client disconnects before the request starts executing (for example, queued behind an earlier request on the same connection): the request does not start, or is cancelled at the first cancellation point.
- A streaming request (with `_progress_token`) whose client disconnects: it is also cancelled before commit, since a disconnected client can no longer receive progress or the result.
- A disconnect during `knowledge_process_document` that has already committed some chunks: chunks already committed stay committed; only work not yet at commit is cancelled. This matches the per-chunk commit semantics of that method.
- A progress frame cannot be written because the socket is closed: this is treated as a disconnect, not an error that fails the request after commit has started.
- Many connections dropping concurrently must not leak tasks, locks or resources, and must not block other connections.
- Cancelled work must not leave the group's write lock held, a half-registered job, or a dangling WAL entry.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: `knowledge_process_chunk` MUST accept an optional `_progress_token` parameter and, when it is present and non-null, stream progress frames before the terminal response, using the same mechanism and frame shape as `knowledge_process_document`.
- **FR-002**: With a progress token, `knowledge_process_chunk` MUST emit progress for the phases `embed`, `extract` and `commit`, in that order, each only when that phase actually runs.
- **FR-003**: Without `_progress_token`, `knowledge_process_chunk` behaviour and response shape MUST be unchanged.
- **FR-004**: When a client's connection closes while a `knowledge_process_chunk` request is in flight, the service MUST cancel the request at the next cancellation point before commit begins, whether or not the request is streaming.
- **FR-005**: A `knowledge_process_chunk` cancelled by disconnect before commit MUST produce no graph mutation, no WAL record and no episode.
- **FR-006**: Once commit has started, the service MUST let it run to completion even if the client has disconnected.
- **FR-007**: `knowledge_process_document` MUST be cancelled on client disconnect under the same rules (at the next pre-commit cancellation point; in-progress commit finishes), unless it already does so, in which case that behaviour MUST be preserved and covered by a test.
- **FR-008**: A cancellation caused by client disconnect MUST be logged, including the method name and the reason.
- **FR-009**: A cancelled request MUST leave the service healthy: it keeps serving other and new connections, and releases any locks or resources the request held.
- **FR-010**: The cancellation MUST reuse the service's existing cancellation mechanism rather than introduce a parallel one, and MUST be exercised by tests in the same style as the existing shutdown-cancellation tests.
- **FR-011**: The IPC reference MUST state that requests on a single connection are handled one at a time, and that clients wanting parallel ingest, or reads alongside ingest, should open several connections.
- **FR-012**: The IPC reference MUST give timeout guidance: that disconnecting cancels in-flight ingest before commit, that retrying on a new connection after a timeout is therefore safe in that window, and that a request which times out during commit may still complete.
- **FR-013**: The IPC reference MUST document the `_progress_token` parameter and phases for `knowledge_process_chunk`, and the cancel-on-disconnect behaviour.
- **FR-014**: `knowledge_capabilities` `features` MUST include `chunk_progress` and `cancel_on_disconnect`, observing the registry rules from #854 (sorted, unique, lower_snake_case, append-only).
- **FR-015**: The change MUST NOT alter the protocol version (additive change only), and MUST NOT break existing clients, including the Electron app's Python client, which do not send a token or rely on the old behaviour.
- **FR-016**: The change MUST be recorded in an ADR named by this issue number (per repository convention) and noted in the CHANGELOG as additive.

### Key Entities

- **Progress notification**: A `{"type":"progress",...}` frame correlated by the caller's `_progress_token`, sent before the terminal response.
- **Cancellation point**: A place in an ingest request where the service checks whether it has been cancelled; the last one is immediately before commit begins.
- **Connection**: One socket session, which processes one request at a time.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: With a progress token, 100% of successful `knowledge_process_chunk` calls deliver `embed`, `extract`, `commit` progress in order before the result.
- **SC-002**: When a client disconnects during extraction against a slow mock extractor, zero episodes, zero WAL records and zero graph mutations result, and the next `health_check` succeeds.
- **SC-003**: When a client disconnects during commit, the episode and WAL record are present afterwards in 100% of runs.
- **SC-004**: A timeout-and-retry on a new connection results in exactly one extraction of the chunk for disconnects occurring before commit.
- **SC-005**: An integrator can answer from the IPC reference alone: "does a search wait behind my ingest on the same connection?" (yes) and "how do I avoid that?" (use another connection).
- **SC-006**: `knowledge_capabilities` lists both new feature strings, and all existing capabilities and parity tests continue to pass.

## Assumptions

- The phase names are exactly `embed`, `extract` and `commit`, as in the issue; if the chunk flow's real phases differ slightly, the researcher aligns them to these names and reports any mismatch.
- "Same mechanism and shape as `process_document`" means the existing progress frame format; no new frame types are introduced.
- Cancellation is cooperative at existing or newly added pre-commit checkpoints; an in-flight LLM call may run to its next checkpoint rather than being aborted instantly.
- Detecting a disconnect during a non-streaming request requires the connection handler to watch the socket while the request runs; the request continues to be the only one processed on that connection (no pipelining).
- A client that half-closes its write side after sending a request but still waits for a response should not be treated as a disconnect; only a fully closed or errored connection counts. The exact detection rule is a Plan-stage decision.
- No Python client or Electron changes are part of this issue.
- Dependency #858 has landed before this work starts.

## Out of Scope *(optional)*

- `deadline_ms` request parameter (part 2 of #836).
- Joining a duplicate in-flight resubmit to an already-running request (part 2).
- Pipelining or concurrent handling of multiple requests on one connection (part 2).
- Cancelling other long-running methods on disconnect (for example `add_episode`, rebuild, reprocess), unless they already do.
- Rolling back or interrupting a commit once it has started.
- Changes to the Python client or Electron app.

## Source References *(optional)*

- Issue #836 (parent, parts 1 and 2); issue #854 / ADR-0854 (capabilities and feature registry)
- ADR-0005 (streaming IPC progress framing via `_progress_token`)
- `docs/ipc-mcp-reference.md` (IPC reference to update)
- Existing shutdown-cancellation tests (`cancel_shutdown`)
