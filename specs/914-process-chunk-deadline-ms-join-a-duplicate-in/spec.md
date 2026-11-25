# Feature Specification: process_chunk deadline_ms, join a duplicate in-flight resubmit, and pipelining (implements #836, part 2)

**Feature Branch**: `fabrik/issue-914`
**Created**: 2026-11-25
**Status**: Specified
**Input**: User description: the original issue body, verbatim:

> ## Problem
>
> #861 shipped part 1 of community request #836: a `process_chunk` progress token, cancel-on-disconnect before commit, and documented per-connection concurrency. Three parts of the request were deliberately deferred.
>
> 1. **No deadline.** No method accepts a deadline. A client that can only wait N seconds has to time out and disconnect (cancel-on-disconnect now handles that), and it gets no structured "abandoned before commit" answer.
> 2. **A duplicate resubmit extracts again.** A client that retries the same `(group, chunk_id)` on a new connection while the first attempt is still running pays for a second extraction, unless the first connection fully closed and was cancelled in time.
> 3. **One request at a time per connection.** Concurrency means opening several connections; there is no way to pipeline requests on one connection, correlated by `id`.
>
> ## Change
>
> 1. **Optional `deadline_ms`** on `knowledge_process_chunk` and `knowledge_process_document`.
>    - **Before the deadline:** the request runs normally.
>    - **Past the deadline, before commit:** LCG abandons the request at the next cancellation point and returns a structured error, for example code `deadline_exceeded` with the phase reached. Nothing is written.
>    - **Once commit has started:** it finishes, and the response says the deadline was exceeded but the write completed.
>    - **Applies to:** the same cancellation points as cancel-on-disconnect (#861, ADR-0861).
> 2. **Join a duplicate in-flight resubmit.**
>    - **When:** a `process_chunk` arrives for a `(group, chunk_id)` that is already in flight with *identical* content.
>    - **What happens:** the second request waits for the first and receives the same result, marked as joined, instead of extracting again.
>    - **Different content:** behaviour is unchanged, using the existing in-flight registry from #859.
>    - **If the first request is cancelled** (disconnect or deadline): the joined one must not inherit the cancellation. It either takes over the work or starts fresh, and the ADR says which.
> 3. **Pipelining on one connection (decide first).** Either support several in-flight requests on one connection, correlated by `id`, with responses possibly out of order, or record an ADR decision not to. In both cases, keep the "open N connections" guidance in the integrator guide (#862) accurate.
>
> Each shipped capability gets a feature string in `knowledge_capabilities` and a row in the reference's feature-strings table (#885). Changelog entries are `changelog.d/` fragments (#904).
>
> ## Out of scope
> - An async bulk-ingest job API (also suggested in #836); file it separately if wanted.
> - Any change to part 1's disconnect semantics.
>
> ## Acceptance
> - **Deadline:** with a slow mock extractor, a request with a short `deadline_ms` returns `deadline_exceeded` before commit, with no episode or WAL record. A deadline that passes during commit still commits.
> - **Join:** two identical `process_chunk` requests for one chunk id, sent concurrently on two connections, make exactly one extraction call, and both get a success result.
> - **Pipelining:** either implemented with tests, or declined in an ADR.

## Background

Community request #836 asked for better control over slow chunk ingest. #861 (ADR-0861) shipped the first part: a `_progress_token` with `embed` / `extract` / `commit` phase frames and heartbeats, cancel-on-disconnect before commit (a started commit always finishes), and documentation that a connection serves one request at a time. Three things were left for this issue:

- **No structured deadline.** A client that can only wait N seconds must time out and drop the connection. Cancel-on-disconnect then stops the work, but the client never receives an answer saying the request was abandoned before anything was written, as opposed to "the connection died and I don't know".
- **Duplicate resubmits pay twice.** A client that times out and retries the same `(group_id, chunk_id)` on a new connection while the first attempt is still extracting starts a second embed and LLM extraction. Cancel-on-disconnect only helps if the first connection has fully closed and was cancelled before commit. Today an identical resubmit is a no-op only once the first attempt has *committed* (ADR-0858); while an attempt is in flight, the service only registers it so that the no-op never fires underneath it.
- **Concurrency needs N connections.** There is no way to pipeline several requests on one connection and correlate them by `id`. The integrator guide (#862) tells clients to open one connection per concurrent request.

The service already has the building blocks: a per-request cancellation token with cancellation points up to write-lock acquisition, a "commit has started" boundary that is never interrupted, and an in-flight registry keyed by `(group_id, chunk_id)` (ADR-0858, ADR-0859).

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Bound a slow ingest with a deadline and get a structured answer (Priority: P1)

An integrator ingesting chunks can only afford to wait a fixed time for each one. They pass `deadline_ms` on `knowledge_process_chunk` (or `knowledge_process_document`). If the work cannot reach commit in time, they get a structured `deadline_exceeded` error naming the phase reached, and know that nothing was written, so a retry is safe. If the commit had already started when the deadline passed, the write completes and the success response says the deadline was exceeded.

**Why this priority**: It is the headline of the request and the smallest slice that is useful on its own. It also builds directly on the cancellation points that already exist.

**Independent Test**: With a mock extractor that sleeps longer than the deadline, send `knowledge_process_chunk` with a short `deadline_ms` and assert a `deadline_exceeded` error carrying the phase, and no episode or WAL record. Separately, make the deadline pass while the commit is in progress and assert the episode is written and the response flags the exceeded deadline.

**Acceptance Scenarios**:

1. **Given** a request with `deadline_ms` comfortably longer than the work, **When** it runs, **Then** it succeeds with the same result as a request without `deadline_ms`, and the result does not report an exceeded deadline.
2. **Given** a slow extractor and a short `deadline_ms`, **When** the deadline passes before commit starts, **Then** the service abandons the request at the next cancellation point and returns a `deadline_exceeded` error that names the phase reached; no episode, entity, edge or WAL record is written and the service stays healthy.
3. **Given** a request whose deadline passes after the group write lock has been acquired (commit started), **When** the commit completes, **Then** the write is fully committed and the success response states that the deadline was exceeded but the write completed.
4. **Given** a request still waiting for the group write lock behind another writer when its deadline passes, **When** the next cancellation point is reached, **Then** it returns `deadline_exceeded` without writing (the lock wait is before commit).
5. **Given** a `knowledge_process_document` request whose deadline passes before the revision's final commit step, **When** it is abandoned, **Then** it returns `deadline_exceeded`, units already committed stay in place and nothing is retired, as for any cancelled document ingest (ADR-0861).
6. **Given** `deadline_ms` that is not a positive integer, **When** the request is validated, **Then** it is rejected with the usual validation error before any work is done.

---

### User Story 2 - A duplicate in-flight resubmit joins the first request instead of extracting again (Priority: P1)

An integrator whose client timed out retries the same chunk, with identical content, on a new connection while the first attempt is still running. The service recognises the duplicate, makes the second request wait for the first, and returns the same result, marked as joined. Exactly one extraction happens. If the first request is cancelled (disconnect or deadline), the joined request is not cancelled with it.

**Why this priority**: It removes the double extraction cost that motivated the part of #836 cancel-on-disconnect could not cover, and it is the natural companion of the deadline: a deadline-driven client retries.

**Independent Test**: Send two identical `knowledge_process_chunk` requests for one chunk id concurrently on two connections with a counting mock extractor; assert exactly one extraction call and a success result on both, the second marked as joined. Then cancel the first by disconnect and by deadline and assert the second still completes with success.

**Acceptance Scenarios**:

1. **Given** a `process_chunk` for `(group, chunk_id)` is in flight, **When** a second request arrives with identical content (same comparison as an unchanged resubmission, ADR-0858), **Then** it waits for the first and receives the same success result, marked as joined, and the extractor is called once in total.
2. **Given** a `process_chunk` is in flight, **When** a second request arrives for the same `(group, chunk_id)` with different content, **Then** behaviour is unchanged from today (no join).
3. **Given** a joined request is waiting, **When** the first request is cancelled by its client disconnecting before commit, **Then** the joined request does not inherit the cancellation: it completes the ingest and returns success, with exactly one episode written for the chunk.
4. **Given** a joined request is waiting, **When** the first request is abandoned by its deadline before commit, **Then** the joined request likewise completes successfully rather than returning `deadline_exceeded` on the first request's behalf.
5. **Given** a joined request carries its own `deadline_ms` or its own client disconnects, **When** that happens while waiting, **Then** only the joined request returns `deadline_exceeded` or is dropped; the first request is unaffected.
6. **Given** a joined request, **When** the first request fails for a reason other than cancellation, **Then** the joined request receives an answer (the shared failure) and does not hang.

---

### User Story 3 - Decide pipelining on one connection and keep the guidance true (Priority: P2)

The service owner settles whether several requests may be in flight on one connection, correlated by `id`, with responses possibly out of order. The outcome is either a tested implementation or a recorded decision not to, and in both cases the integrator guide's connection guidance is accurate afterwards.

**Why this priority**: The request explicitly asks for a decision first and allows either answer, so it carries less risk and no hard dependency on the other two stories, but whichever way it goes the documentation must not be left stale.

**Independent Test**: If implemented: two requests sent back to back on one connection, the first slow and the second fast, return both results correlated by `id`, with the fast one first. If declined: the decision record exists, and the integrator guide and IPC reference state the one-at-a-time rule with no contradicting text.

**Acceptance Scenarios**:

1. **Given** the decision is to implement pipelining, **When** several requests are sent on one connection with distinct `id`s, **Then** each response carries its request's `id`, responses may arrive out of order, and the existing per-connection behaviours (ordering for clients that do not pipeline, progress frames, disconnect cancellation) remain well-defined and documented.
2. **Given** the decision is not to implement pipelining, **When** the ADR is merged, **Then** it records the reasons and the integrator guide keeps the "open N connections" guidance, with the reference and guide pointing at it.
3. **Given** either outcome, **When** an integrator reads the integrator guide and the IPC reference, **Then** neither contains a statement that contradicts the shipped behaviour (including the current note that progress frames carry no token because a connection never has two requests in flight).

---

### Edge Cases

- **Deadline already elapsed on arrival or tiny `deadline_ms`**: the request is abandoned at the first cancellation point; a request that returns before any cancellation point (an unchanged no-op, a validation error) is unaffected by the deadline and succeeds or fails as it does today.
- **Deadline passes exactly as the write lock is acquired**: once the lock is held the commit finishes (never torn); the response reports the exceeded deadline together with the completed write.
- **Deadline and disconnect together**: whichever is noticed first cancels the request before commit; either way nothing is written, and after commit starts both are ignored.
- **Deadline on a streaming request (`_progress_token`)**: the terminal frame is the same `deadline_exceeded` error; progress frames sent before it stay valid.
- **Join when the first request ends as an `unchanged` no-op or an ordinary success**: the joined request receives the same result, marked joined.
- **Join when several requests wait on one in-flight chunk**: each is served; the extractor is still called at most once for the identical content (or, if the first was cancelled, once more in total for the survivors).
- **Join not applicable**: `supersede=false`, `same_revision_episode_uuids` supplied, or a request that differs in any compared field behaves as today; a `knowledge_mark_chunk_removed` in flight for the same key is not joined.
- **Cancellation of the first request while joined requests wait**: no request is left waiting forever, none inherits the cancellation, and exactly one episode results for the chunk.
- **Shutdown while a request is waiting or joined**: the shutdown token still cancels all of them as today.
- **Different groups, same `chunk_id`**: never joined.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: `knowledge_process_chunk` and `knowledge_process_document` MUST accept an optional `deadline_ms`, a positive integer number of milliseconds measured from when the service receives the request. When omitted, behaviour is unchanged. An invalid value MUST be rejected with the standard validation error before any work is done.
- **FR-002**: A request that reaches its deadline before commit has started MUST be abandoned at the next cancellation point (the same points as cancel-on-disconnect, ADR-0861, including waiting for the group write lock) and MUST return a structured error with reason `deadline_exceeded` that names the phase reached. Nothing MUST be written: no episode, entity, edge or WAL record for a chunk request.
- **FR-003**: Once the commit has started, the request MUST complete it regardless of the deadline, and a successful response for a request whose deadline had passed MUST say so (the deadline was exceeded but the write completed). A request that finishes within its deadline MUST NOT carry that indication.
- **FR-004**: For `knowledge_process_document`, a deadline that passes before the final revision commit MUST leave already-committed units in place and retire nothing, consistent with ADR-0861's cancelled-document behaviour, and the error MUST make it clear that the call did not complete.
- **FR-005**: When a `knowledge_process_chunk` request arrives for a `(group_id, chunk_id)` that already has a request in flight with identical content, the service MUST NOT start a second extraction. The new request MUST wait for the in-flight request and return the same success result, marked as joined. "Identical" MUST use the same comparison as an unchanged resubmission (ADR-0858: content, attributes, origin, `source_file`, `doc_id` and, when supplied, `reference_time`).
- **FR-006**: A request with different content for the same `(group_id, chunk_id)`, a request in a different group, a request with `supersede=false` or `same_revision_episode_uuids`, and a `knowledge_mark_chunk_removed` call MUST keep today's behaviour and MUST NOT be joined.
- **FR-007**: A joined request MUST NOT inherit the cancellation of the request it joined, whether by client disconnect or deadline. The outcome MUST be that the joined request still ends with a success result and exactly one live episode for the chunk. The ADR MUST state which mechanism is used (the joined request takes over the work, or starts fresh).
- **FR-008**: A joined request's own `deadline_ms` and its own client disconnect MUST apply only to it; they MUST NOT cancel the request it joined or other joined requests.
- **FR-009**: When the in-flight request ends in a failure that is not a cancellation, joined requests MUST receive an answer and MUST NOT hang. The ADR MUST state whether they share the failure or retry.
- **FR-010**: The service owner MUST decide, in an ADR named for this issue per the repository's ADR numbering rule, whether to support several in-flight requests on one connection (correlated by `id`, responses possibly out of order) or to decline. If implemented, it MUST come with tests covering out-of-order completion and correlation by `id`, and with its interaction with progress frames, heartbeats and cancel-on-disconnect documented. If declined, the ADR MUST record the reasons.
- **FR-011**: In either outcome of FR-010, the integrator guide's connection guidance ("open N connections") and the IPC reference's connections section MUST be updated so they are accurate, including the statement that progress frames carry no token.
- **FR-012**: Each shipped capability (deadline, join, and pipelining if implemented) MUST appear as a feature string in `knowledge_capabilities` and as a row in the reference's feature-strings table, following the append-only rules of ADR-0854. `protocol_version` MUST NOT change unless an existing behaviour changes incompatibly.
- **FR-013**: The IPC reference MUST document `deadline_ms`, the `deadline_exceeded` error and the exceeded-but-committed success indication, and the joined marker, and ADR-0861's "deferred" note MUST be resolved by a reference to the new ADR.
- **FR-014**: Part 1's disconnect semantics (ADR-0861) MUST NOT change; the changelog entry MUST be a `changelog.d/<issue>-<slug>.md` fragment, not an edit to `CHANGELOG.md`.
- **FR-015**: If the attached MCP backend is a path to these methods, the new parameter and response fields MUST pass through it correctly, or the spec/ADR MUST state that they do not.

### Key Entities *(if applicable)*

- **Deadline**: an optional per-request time budget, counted from receipt, enforced only before commit begins.
- **In-flight chunk request**: a running `process_chunk` (or `mark_chunk_removed`) registered by `(group_id, chunk_id)`, with the content identity it was submitted with.
- **Joined request**: a duplicate `process_chunk` that waits on an in-flight request with identical content and returns its result, flagged as joined.
- **`deadline_exceeded` outcome**: either an error (abandoned before commit; includes the phase reached) or a success response carrying an indication that the deadline passed but the write completed.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: With a slow mock extractor, a `process_chunk` with a short `deadline_ms` returns `deadline_exceeded` with a phase name before commit, and afterwards the graph has no episode, entity, edge or WAL record for it.
- **SC-002**: A request whose deadline passes during commit is committed in full and its response reports the exceeded deadline; an equivalent request within its deadline reports nothing of the kind.
- **SC-003**: Two identical concurrent `process_chunk` requests on two connections produce exactly one extraction call and two success results, one marked joined.
- **SC-004**: When the first of two such requests is cancelled by disconnect, and separately by deadline, before commit, the second returns success and exactly one live episode exists for the chunk.
- **SC-005**: A request with different content for the same chunk id, or in a different group, performs its own extraction and is not marked joined.
- **SC-006**: `knowledge_capabilities` lists a feature string for every capability this issue ships, each with a row in the feature-strings table, and a documentation check finds no statement contradicting the shipped connection behaviour.
- **SC-007**: Pipelining is either covered by passing tests (out-of-order responses correlated by `id`) or declined in a merged ADR, and the integrator guide matches the choice.
- **SC-008**: The existing part-1 tests (progress frames, cancel-on-disconnect, half-close handling) pass unchanged.

## Assumptions

- `deadline_ms` is relative to receipt of the request, not an absolute timestamp, so client and service clocks do not matter.
- `deadline_ms` is capped only by validation (positive integer); no maximum is imposed. A request with no deadline behaves exactly as today.
- The error uses the structured-error shape already used for other refusals (`error.data.reason`), with the reason `deadline_exceeded` and the phase reached. The exact JSON-RPC code and the name of the success-side indication are design choices for Plan/ADR; they must be documented in the reference.
- A joined request receives the same result as the request it joined; the result gains a joined marker. Whether joined requests with a progress token receive phase frames or only heartbeats is a design choice for Plan/ADR; they MUST at least receive heartbeats so a per-line idle timeout does not drop them.
- Joining applies to `knowledge_process_chunk` only; `knowledge_process_document` has its own per-document ingest lock and is not joined.
- Feature string names (for example `process_deadline`, `chunk_join_inflight`, and a pipelining name if implemented) are proposals to be fixed in Plan, subject to ADR-0854's naming rules.
- Pipelining is decided by the ADR; the Plan stage should evaluate it first, as the issue asks. Either decision satisfies the acceptance criteria.
- The deadline is enforced at the cancellation points that already exist; no new cancellation points are required by this issue. If a phase has none finer than "next point", a deadline can be noticed late, which is the same accepted limitation as disconnect cancel (ADR-0861).
- Provider billing for an LLM request already sent is not recoverable by a deadline, as with disconnect cancel.

## Out of Scope *(optional)*

- An async bulk-ingest job API (also suggested in #836); file it separately if wanted.
- Any change to part 1's disconnect semantics (ADR-0861), including the half-close rules and heartbeat behaviour.
- Deadlines on methods other than `knowledge_process_chunk` and `knowledge_process_document`.
- Joining duplicate `knowledge_process_document` calls.
- Interrupting a commit that has started.

## Source References *(optional)*

- #836 (community request), #861 / ADR-0861 (progress, cancel-on-disconnect, deferred items), #858 / ADR-0858 (unchanged-resubmit comparison and in-flight check), #859 / ADR-0859 (in-flight registry use), #862 (integrator guide), #885 (feature-strings table), #854 / ADR-0854 (capability registry), #904 / ADR-0904 (changelog fragments)
- `docs/adr/0861-process-chunk-progress-and-cancel-on-disconnect.md`, `docs/adr/0858-unchanged-chunk-resubmission-noop.md`, `docs/integrator-guide.md`, `docs/ipc-mcp-reference.md`
