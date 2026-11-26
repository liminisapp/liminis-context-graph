# Feature Specification: IPC request size limit (bounded socket read) and `knowledge_process_chunk` size refusal

**Feature Branch**: `fabrik/issue-293`
**Created**: 2026-11-25
**Status**: Specified
**Input**: User description:

> ## Background
>
> #284 (PR #286) added `chunk_split::split_into_units` to bound extraction damage for oversized `chunk_text`, but the service itself places no upper limit on how large a single IPC request (and therefore `chunk_text`) can be. `crates/service/src/connection.rs` (`serve_connection`, reached from `handle_connection_with`; it was in `main.rs` when this issue was filed) reads each request as one line via `BufReader::new(reader).lines()` / `next_line()`, which has no built-in maximum line length — it buffers until a newline is found, bounded only by available memory.
>
> `chunk_split.rs`'s documented pathological-input worst case (whitespace only very close to the start of many consecutive `max_chars` windows, driving cost toward `O(chars.len() * max_chars)` rather than `O(chars.len())`) is therefore reachable at whatever scale a caller submits, not just as a Big-O footnote. A `chunk_text` engineered to hit this worst case at multi-megabyte scale could tie up a blocking-pool thread for an extended period. `spawn_blocking` keeps this off the async executor (so it degrades one blocking-pool thread rather than stalling request handling generally), but with enough concurrent oversized requests this could still exhaust the blocking-thread pool — a genuine DoS-shaped surface for a service that may accept `chunk_text` from less-trusted ingestion pipelines.
>
> This was raised during Validate-stage review of #284/PR #286 (review thread on `chunk_split.rs:25`), which asked that a tracked follow-up issue exist for an IPC-layer request size limit rather than leaving it as an unlinked ADR footnote.
>
> ## Documented in
>
> ADR-0284 (`docs/adr/0284-chunk-splitting-and-chunk-id-idempotency.md`), Consequences section — "No upstream size cap on `chunk_text` exists at the IPC/transport layer."
>
> ## Scope note
>
> #282's own scope explicitly excluded introducing a size cap (only a warning threshold), and #284 carried that exclusion forward rather than re-litigating it. A hard IPC-layer request size limit (e.g. a max line length in `handle_connection`, with an actionable rejection response) is a separate, broader decision than either issue's chunk-splitting/idempotency scope — it affects every IPC method, not just `knowledge_process_chunk`.
>
> ## Proposed fix (for Research/Plan to confirm)
>
> - Add a maximum request line length in the socket read loop in `crates/service/src/connection.rs::serve_connection`, rejecting oversized requests before they reach JSON parsing / method dispatch.
> - Decide the limit relative to `LCG_CHUNK_TEXT_ADVISORY_MAX_CHARS` and to other IPC methods' payload shapes (not all of which are text-heavy).
> - Rejection response should follow the same "actionable error" shape used elsewhere (name the actual size, the limit, and what to do instead).
>
> ## Update (2026-11-25, before Specify)
>
> **Context since filing.** 0.20.0 added document mode (`knowledge_process_document`, #785), whose body is capped by `LCG_DOCUMENT_MAX_BYTES` (default 2 MiB, `crates/core/src/document_config.rs`; strict startup validation from #862). That cap is checked after the whole line has been read and parsed, so it does not bound the read. `knowledge_process_chunk` has no size cap at all. Both are still open to an unbounded line.
>
> **Acceptance criteria**
> - The IPC socket read loop stops buffering once a request line passes a configurable byte limit. It answers with a JSON-RPC error that names the observed size (or "more than N"), the limit, and the setting that raises it, and it then either recovers to the next line or closes the connection. Research/Plan picks which, and the IPC reference documents it.
> - The limit is a new setting, validated strictly at startup like `LCG_DOCUMENT_MAX_BYTES`: an invalid or empty value is fatal. Its default must sit comfortably above the largest legitimate request, so a maximal `knowledge_process_document` (2 MiB of body, after JSON escaping) is never refused. Startup refuses a configuration where the line limit can't hold a maximal document.
> - `knowledge_process_chunk` gets an actionable size refusal for `chunk_text`, consistent with the document cap. Plan decides whether it reuses `LCG_DOCUMENT_MAX_BYTES` or gets its own setting, and records why.
> - Tests: an oversized line is refused without being fully buffered, the connection behaves as documented afterwards, and a request just under the limit, including a maximal document, still succeeds.
> - Docs: the IPC/MCP reference (limits and the new error), the integrator guide, ADR-0284's Consequences note, a CHANGELOG fragment, and a `knowledge_capabilities` feature string if new behaviour warrants one.
>
> **Out of scope:** the MCP stdio transport (`--mcp-stdio`) unless Research finds it shares the read loop; changing `chunk_split`'s algorithm; and any other method's semantics.

## Background

The service reads each IPC request as one newline-terminated line from its local socket. Nothing bounds that line: the reader buffers until it sees a newline, so a single request can consume as much memory as the caller cares to send, before any validation runs. The existing size caps do not help: `LCG_DOCUMENT_MAX_BYTES` (default 2 MiB, strictly validated at startup since #862) is checked only after the full line has been read and parsed, and `knowledge_process_chunk` has no size cap at all.

This matters for two reasons:

1. **Memory exhaustion.** Any client able to reach the socket (including less-trusted ingestion pipelines) can make the service buffer an arbitrarily large line, affecting every connection and every method.
2. **Algorithmic worst case.** `chunk_split`'s documented pathological input (whitespace only near the start of many consecutive windows, cost trending toward `O(chars * max_chars)`) is reachable at whatever scale a caller submits. At multi-megabyte scale it can occupy a blocking-pool thread for a long time, and enough concurrent oversized requests could exhaust that pool.

The gap was raised in Validate review of #284/PR #286 and recorded as a footnote in ADR-0284's Consequences ("No upstream size cap on `chunk_text` exists at the IPC/transport layer"). #282 and #284 deliberately left a hard cap out of scope because it is a transport-wide decision affecting every IPC method; this issue is that decision. Document mode (#785) later added a second, larger oversize surface.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - The read loop refuses an oversized request line without buffering it (Priority: P1)

An operator or integrator running the service wants a hard upper bound on how much memory a single request can make the service hold. A client that sends a request line over the configured limit gets a clear JSON-RPC error and the service never accumulates the whole line.

**Why this priority**: This is the core vulnerability. Everything else is secondary to making the read bounded.

**Independent Test**: Open a connection, send a line well over the limit (and no newline for a while), and assert the service responds with the size-limit error having buffered no more than roughly the limit, with peak memory not growing in proportion to the amount sent.

**Acceptance Scenarios**:

1. **Given** the default limit, **When** a client sends a request line larger than the limit, **Then** the service stops buffering at the limit, and replies with a JSON-RPC error naming the observed size (or "more than N" if the line was not read to the end), the limit, and the setting that raises it.
2. **Given** an oversized line, **When** the error has been sent, **Then** the connection behaves exactly as the documented post-refusal policy states (either the next well-formed line on the same connection is served normally, or the connection is closed), and the policy is the same every time.
3. **Given** an oversized line that is never terminated by a newline, **When** the client keeps sending, **Then** the service's memory use stays bounded and it does not wait indefinitely holding the data.
4. **Given** an oversized request, **When** it is refused, **Then** it never reaches JSON parsing or method dispatch, so no handler runs and no state changes.

---

### User Story 2 - Legitimate requests, including a maximal document, are never refused (Priority: P1)

An existing integrator sends large but valid requests, up to a `knowledge_process_document` body at the document cap. Their calls must keep working unchanged.

**Why this priority**: A limit that rejects valid traffic is a regression, and the document cap is the largest legitimate payload.

**Independent Test**: Send a `knowledge_process_document` whose body is exactly `LCG_DOCUMENT_MAX_BYTES` of content that expands to the worst-case JSON escaping, plus a request just under the line limit, and assert both are accepted and processed.

**Acceptance Scenarios**:

1. **Given** default settings, **When** a client sends a maximal 2 MiB document after JSON escaping (including worst-case escaping), **Then** the request is not refused for size.
2. **Given** a request whose line is just under the line limit, **When** it is sent, **Then** it is read in full and dispatched normally.
3. **Given** a request exactly at the limit versus one byte over, **When** both are sent, **Then** the first is accepted and the second refused, with the boundary documented (whether the newline terminator counts).

---

### User Story 3 - The line limit is configurable and strictly validated at startup (Priority: P1)

An operator who legitimately handles larger documents raises `LCG_DOCUMENT_MAX_BYTES` and needs the line limit to follow, or set it explicitly. A misconfiguration must stop the service, not silently weaken protection.

**Why this priority**: The limit must be tunable and must never silently fall back, matching the `LCG_DOCUMENT_MAX_BYTES` model from #862.

**Independent Test**: Start the service with valid, invalid, empty and inconsistent values for the new setting and for `LCG_DOCUMENT_MAX_BYTES`; check each outcome and the error text.

**Acceptance Scenarios**:

1. **Given** the new setting is unset, **When** the service starts, **Then** a documented default applies and is logged at startup.
2. **Given** the new setting is empty, malformed, zero or above a documented upper bound, **When** the service starts, **Then** startup fails with an error naming the variable and the value.
3. **Given** a line limit too small to hold a maximal document at the configured `LCG_DOCUMENT_MAX_BYTES` (after worst-case JSON escaping and envelope overhead), **When** the service starts, **Then** startup is refused with an error naming both settings and what to change.
4. **Given** an operator raises `LCG_DOCUMENT_MAX_BYTES` above what the default line limit can carry, **When** the service starts without also raising the line limit, **Then** the operator gets the refusal in scenario 3 rather than a service that accepts the config but rejects its own maximal documents.

---

### User Story 4 - `knowledge_process_chunk` refuses an oversized `chunk_text` with an actionable error (Priority: P2)

A client submitting `chunk_text` that is within the line limit but unreasonably large for a single chunk gets a clear refusal before any splitting or extraction work starts, consistent with how documents over the cap are refused.

**Why this priority**: The line limit alone still allows multi-megabyte `chunk_text` (up to the line limit). A method-level cap keeps the pathological `chunk_split` worst case from being reachable at that scale. It is P2 because the line limit already removes the unbounded case.

**Independent Test**: Call `knowledge_process_chunk` with `chunk_text` just under and just over the chunk cap; assert the first proceeds and the second returns a structured error naming the observed size, the cap, and the setting that raises it, with no episode written and no splitting performed.

**Acceptance Scenarios**:

1. **Given** `chunk_text` over the chunk cap, **When** `knowledge_process_chunk` is called, **Then** it fails with a structured, actionable error (size, cap, how to raise it or what to do instead, such as using document mode) before any work or write.
2. **Given** `chunk_text` at or under the cap, **When** called, **Then** behaviour is unchanged, including the existing advisory warning threshold and chunk splitting.
3. **Given** the chunk cap's setting is invalid, **When** the service starts, **Then** startup fails like the other size settings.

---

### User Story 5 - Integrators can discover and understand the limits (Priority: P2)

An integrator reads the reference docs and `knowledge_capabilities` to learn the line limit, the chunk cap, the error shape and what happens to the connection afterwards.

**Why this priority**: A new hard failure mode needs to be discoverable so clients can handle it.

**Independent Test**: Check that the IPC/MCP reference, integrator guide and configuration docs describe the limits, the error and the post-refusal behaviour, and that capabilities advertises the new behaviour if a feature string is warranted.

**Acceptance Scenarios**:

1. **Given** the docs, **When** an integrator looks up request limits, **Then** they find the line limit, its default and relationship to the document cap, the chunk cap, the error code and shape, and the post-refusal connection behaviour.
2. **Given** a client probing `knowledge_capabilities`, **When** the service has the new behaviour, **Then** a feature string lets the client detect it (if Plan judges one warranted).

---

### Edge Cases

- A line is exactly at the limit, or one byte over; the boundary and whether the terminating newline counts are defined and tested.
- A line exceeds the limit but the newline arrives in the same read as the final over-limit bytes; the observed size reported may be exact or "more than N", and the wording is consistent.
- The oversized request arrives while other requests are queued or in flight on the same connection (the read loop already queues a few lines ahead); earlier queued requests are unaffected by a later oversized one, and ordering of responses is preserved.
- After refusal, the remaining bytes of the oversized line are either discarded up to the next newline (recover mode) or the connection is closed (close mode); in recover mode, discarding must itself not buffer the line.
- The oversized line contains multi-byte UTF-8 split across the cut point; this must not panic or produce an invalid-UTF-8 error that masks the size error.
- The oversized request has no parseable `id`, since it is refused before parsing; the error response uses a null id, as for parse errors, and the docs say so.
- Multiple concurrent connections each sending oversized lines: total buffered memory stays bounded by roughly connections × limit.
- A request whose JSON escaping makes it larger than the document body it carries (quotes, backslashes, control characters, non-ASCII escapes) still fits when the body is at the document cap.
- `LCG_DOCUMENT_MAX_BYTES` raised to its 256 MiB upper bound; the consistency check and the line limit's own upper bound must be compatible, or the refusal at startup must say so clearly.
- Attached (`--connect`) mode and the MCP stdio transport: the MCP-attached client forwards requests to the service socket and is therefore subject to the same limit; the stdio transport's own reader is a separate path (see Out of Scope).
- Windows named-pipe transport goes through the same connection loop and gets the same limit.
- `chunk_text` containing exactly the advisory-warning threshold of characters: the existing advisory behaviour is unchanged, and the new cap is distinct from it.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The IPC connection read loop MUST stop accumulating a request line once it exceeds a configured maximum number of bytes, so that the memory held for one request line is bounded by roughly that maximum regardless of how much the client sends.
- **FR-002**: A request line over the limit MUST be refused before JSON parsing and method dispatch; no handler runs and no state changes.
- **FR-003**: The refusal MUST be a JSON-RPC error response whose message names the observed size (or "more than N bytes" when the line was not read to its end), the limit, and the name of the setting that raises it, and carries a stable machine-readable error code distinct from the parse-error code. It uses a null `id`, as the request could not be parsed.
- **FR-004**: After an oversized-line refusal the connection MUST behave in one documented, deterministic way: either (a) discard the rest of that line without buffering it and continue serving subsequent lines, or (b) close the connection after sending the error. Plan selects which and records why in an ADR; the IPC reference documents it.
- **FR-005**: The maximum request line length MUST be a new operator setting, parsed strictly once at startup in the same grammar family as `LCG_DOCUMENT_MAX_BYTES` (plain byte count, or `KiB`/`MiB` suffix): empty, zero, signed, fractional, unrecognised-unit, overflowing or above-upper-bound values are fatal startup errors naming the variable and the value, never a silent fallback to the default.
- **FR-006**: The default line limit MUST sit comfortably above the largest legitimate request, so a maximal `knowledge_process_document` (body at the default 2 MiB cap, after worst-case JSON escaping and request envelope) is never refused for line length.
- **FR-007**: Startup MUST be refused, with an error naming both settings and what to change, when the effective line limit cannot hold a maximal document at the effective `LCG_DOCUMENT_MAX_BYTES` (including worst-case JSON escaping and envelope overhead).
- **FR-008**: The effective line limit MUST be logged at startup alongside the existing effective document cap.
- **FR-009**: `knowledge_process_chunk` MUST refuse a `chunk_text` larger than a defined chunk-size cap with a structured, actionable error naming the observed size, the cap and the setting that raises it (and pointing to document mode where appropriate), before any splitting, extraction or write.
- **FR-010**: Whether the chunk cap reuses `LCG_DOCUMENT_MAX_BYTES` or has its own setting is decided in Plan and the reason recorded; whichever is chosen MUST be validated strictly at startup like the other size settings, and MUST NOT change behaviour for `chunk_text` at or below the cap, including the existing `LCG_CHUNK_TEXT_ADVISORY_MAX_CHARS` warning.
- **FR-011**: The line limit MUST apply to every IPC method on the socket/named-pipe transport, including connections made by the attached MCP client, and MUST NOT alter any method's semantics for requests under the limit.
- **FR-012**: Requests under the limit, including a request one byte under it and a maximal document, MUST be read and dispatched exactly as today.
- **FR-013**: A refused oversized line MUST NOT disturb other requests already queued or in flight on the same connection, nor other connections, and response ordering for earlier requests MUST be preserved.
- **FR-014**: The new setting(s) MUST be documented in the configuration reference; the limits, the new error code/shape and the post-refusal connection behaviour in the IPC/MCP reference and integrator guide; ADR-0284's Consequences note MUST be updated to point to the resolution; and a CHANGELOG fragment MUST be added under `changelog.d/` (flagging that clients sending lines above the limit now get an error, and naming the affected raw-socket clients).
- **FR-015**: If Plan judges the new behaviour client-detectable-worthy, `knowledge_capabilities` MUST advertise it with a feature string, with the capabilities tests updated accordingly.
- **FR-016**: The change MUST NOT alter `chunk_split`'s algorithm or the semantics of any other method.

### Key Entities *(if applicable)*

- **Request line limit**: The maximum byte length of one newline-delimited IPC request on the socket transport; operator-configurable, strictly validated, has a default derived from the maximal legitimate request.
- **Chunk-size cap**: The maximum accepted `chunk_text` size for `knowledge_process_chunk`; either shared with the document cap or its own setting, per Plan.
- **Size-limit error**: A JSON-RPC error with a stable code, naming observed size, limit and the setting that raises it.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A client streaming an arbitrarily long unterminated line causes the service to hold no more than the configured limit (plus a small constant) for that request, verified by a test that sends far more than the limit and asserts the refusal arrives without the full payload being read.
- **SC-002**: With default settings, 100% of valid `knowledge_process_document` requests up to the document cap, including worst-case JSON escaping, are accepted; none is refused for line length.
- **SC-003**: A line one byte over the limit is refused with the documented error, and a line at the boundary is accepted, in automated tests.
- **SC-004**: Every invalid, empty or inconsistent value of the new setting(s), and every combination with `LCG_DOCUMENT_MAX_BYTES` that cannot hold a maximal document, fails startup with an error naming the variable(s), value(s) and remedy, covered by tests.
- **SC-005**: After an oversized refusal, the connection behaves as documented in 100% of test runs (continues serving the next line, or is closed, per the chosen policy).
- **SC-006**: A `knowledge_process_chunk` call with `chunk_text` over the chunk cap returns the actionable error with no episode written and no splitting work performed; at or under the cap it behaves as before.
- **SC-007**: The IPC/MCP reference, integrator guide, configuration reference, ADR-0284 Consequences note, CHANGELOG fragment, and (if warranted) capabilities feature string are all updated in the same PR.

## Assumptions

- The line limit applies on the socket/named-pipe transport read loop in `serve_connection`; the MCP stdio transport reads stdin through rmcp's own reader, which is a separate path (confirmed by a quick look; Research verifies).
- The line limit is measured in bytes of the raw line, and the default is a conservative multiple of the document cap derived from worst-case JSON escaping (a JSON string can expand a byte up to roughly 6x with `\uXXXX` escapes), so that default settings never refuse a maximal document. The exact default and upper bound are chosen in Plan.
- Strict parsing reuses the `LCG_DOCUMENT_MAX_BYTES` grammar (bytes, `KiB`, `MiB`), for consistency.
- A request refused before parsing has no usable `id`; a null `id` is acceptable, as it is for parse errors today.
- Choosing between recover-to-next-line and close-the-connection after a refusal, and between a shared or separate chunk cap, are design decisions deferred to Plan; the requirements hold either way.
- The new hard failure mode is acceptable as a documented, CHANGELOG-flagged behaviour change; defaults are set so no known legitimate client is affected.

## Out of Scope *(optional)*

- The MCP stdio transport (`--mcp-stdio`) and its stdin reader, unless Research finds it shares the connection read loop.
- Changing `chunk_split`'s algorithm or fixing its pathological worst case.
- Changing the semantics of any other IPC method, or the existing `LCG_CHUNK_TEXT_ADVISORY_MAX_CHARS` advisory behaviour.
- Per-method or per-client rate limiting, authentication, or limits on the number of concurrent connections.
- Bounding the size of responses.

## Source References *(optional)*

- ADR-0284, `docs/adr/0284-chunk-splitting-and-chunk-id-idempotency.md` (Consequences)
- ADR-0862, `docs/adr/0862-document-max-bytes-strict-2mib-default.md` and `crates/core/src/document_config.rs` (strict-parse precedent)
- `crates/service/src/connection.rs` (`serve_connection` read loop)
- #284 / PR #286 (origin review thread); #282; #785 (document mode); #862 (document cap validation)
