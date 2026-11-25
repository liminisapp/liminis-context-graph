# ADR 0293: Request Line Limit and `chunk_text` Cap

**Status**: Accepted
**Date**: 2026-11-25
**Issue**: #293 (raised in Validate review of #284 / PR #286).

## Context

The socket and named-pipe transport read each request as one newline-terminated line through
`BufReader::lines()`, which buffers until it sees a newline: a single request could hold as much
memory as the caller cared to send, before any validation ran. The size caps that exist did not
help. `LCG_DOCUMENT_MAX_BYTES` ([ADR-0862](0862-document-max-bytes-strict-2mib-default.md)) is
checked after the whole line is read and parsed, and `knowledge_process_chunk` had no cap at all
beyond the advisory warning (`LCG_CHUNK_TEXT_ADVISORY_MAX_CHARS`, #407).

**Premise correction.** The issue was filed against `chunk_split::split_into_units` and
ADR-0284 ("No upstream size cap on `chunk_text` exists at the IPC/transport layer"). Neither is on
`main`: PR #286 (the #284 work) was closed unmerged and #284 was closed not-planned, so there is no
`chunk_split.rs`, no ADR-0284 and no Consequences note to edit. The two real gaps remain, and the
cap still bounds memory and extraction cost for a chunk. If #284 is re-specified, its ADR should
point here, and this ADR gains a pointer back.

## Decision

1. **A bounded line reader.** `connection.rs` replaces `BufReader::lines()` with `BoundedLines`,
   which keeps at most `LCG_REQUEST_MAX_BYTES` bytes of one line. A line over the limit is refused
   before JSON parsing and dispatch, so no handler runs and no state changes. The limit applies to
   the bytes before `\n`: the terminator does not count, a trailing `\r` does, a line of exactly
   the limit is accepted and one byte more is refused.
2. **Cancel-safety constrains the design.** While a cooperative request runs
   ([ADR-0861](0861-process-chunk-progress-and-cancel-on-disconnect.md)) the loop reads the socket
   inside a `tokio::select!`, so the reader's future can be dropped at any point. `read_until` is
   not cancel-safe in tokio. `BoundedLines` therefore keeps all its state (the accumulator and a
   `Reading`/`Discarding` state) in the struct and uses only `fill_buf` and `consume`, with no
   `.await` between a state change and the matching `consume`.
3. **Refusal: `-32005`, null `id`.** The error carries `data = {reason: "request_too_large",
   limit_bytes, setting: "LCG_REQUEST_MAX_BYTES"}`; the message names the limit and the setting
   that raises it. It is written as soon as the limit is crossed, not at the end of the line, so
   it always says "more than N bytes" and never an exact size (an exact count would need the
   whole line read first). The id is null because the line was never parsed, as for `-32700`.
4. **After the refusal the connection recovers, with a discard budget.** The reader drops the
   rest of the line without keeping it, then serves the next line normally. If the oversized
   line runs past `4 x` the limit before its newline, the connection is closed instead.
   - Rejected: closing at once. It races the client's still-running write, so the client can
     get `EPIPE`/`ECONNRESET` and lose the error it needed.
   - Rejected: an unbounded drain. An endless stream would hold the connection forever.
5. **Pipelining.** An oversized line read ahead while a cooperative request runs is queued as a
   marker and refused after the in-flight response, so response order is preserved. Read-ahead is
   bounded in bytes as well as lines (the reader is not polled once the queued lines reach the
   limit), so a connection holds about twice the limit, not nine times. Across connections the
   bound is roughly `connections x 2 x limit` plus the 64 KiB read buffer each.
6. **Settings, strict at startup** (`crates/core/src/request_limits.rs`, grammar shared with
   `LCG_DOCUMENT_MAX_BYTES` via `size_parse.rs`; `GiB` is accepted only where the bound is at
   least 1 GiB):
   - `LCG_REQUEST_MAX_BYTES`: default **16 MiB**, upper bound **2 GiB**.
   - `LCG_CHUNK_TEXT_MAX_BYTES`: default **512 KiB**, upper bound 256 MiB.
   - An empty, zero, malformed or out-of-range value is fatal and names the variable and value.
7. **The consistency invariant.** The line limit must satisfy
   `LCG_REQUEST_MAX_BYTES >= 6 x LCG_DOCUMENT_MAX_BYTES + 64 KiB`. A JSON string can spend six
   bytes (`\u00XX`) on one body byte, and 64 KiB covers the envelope and other parameters. Startup
   is refused otherwise, naming both settings and the minimum. The 16 MiB default carries a
   maximal 2 MiB document (about 12 MiB escaped). The limit is **not derived** from the document
   cap when unset: an operator who raises the document cap above about 2.7 MiB must raise the line
   limit too, and gets a refusal telling them exactly what to set, rather than a service that
   accepts the configuration and rejects its own maximal documents. The 2 GiB upper bound
   accommodates `LCG_DOCUMENT_MAX_BYTES=256MiB` (about 1.5 GiB); operators who configure that opt
   in to the exposure.
8. **A separate `chunk_text` cap, not a reuse of the document cap.** Reusing
   `LCG_DOCUMENT_MAX_BYTES` would still accept a 2 MiB `chunk_text`, the scale the issue calls
   pathological; a chunk is meant to be about the advisory 8,000 characters, and 512 KiB is about
   64 times that. A separate setting lets an operator keep chunks tight while allowing big
   documents. It is a byte cap (`String::len()` is O(1), no `chars().count()` scan on huge
   strings), checked straight after `chunk_text` is extracted, before any other work. The refusal
   is `Error::Ipc("chunk_text_too_large: ...")`, code `-32000`, matching `document_too_large:`.
   The advisory warning is unchanged for anything at or under the cap.
9. **The limit is a parameter of `serve_connection`**, not a global read inside it, following
   `heartbeat` and `write_timeout`, so tests can use a tiny limit despite the once-per-process
   install.
10. **Attached `--connect` MCP.** The attached client discards replies whose `id` is not its own,
    which would swallow the null-id refusal and turn it into a 30 s timeout. It now treats a
    null-id `-32005` as the answer to its single in-flight call.
11. **Capabilities.** One feature string, `request_size_limits`, lets clients detect that large
    lines now fail.

## Consequences

- **Behaviour change.** A raw-socket client (the Electron app via `graphiti_service.py`, the
  attached `--connect` MCP client, any other) that sends a line over 16 MiB now gets `-32005`, and
  a `chunk_text` over 512 KiB gets `chunk_text_too_large`. Both are above every known legitimate
  request, both are raisable, and the changelog fragment flags them.
- **Operator friction.** Raising `LCG_DOCUMENT_MAX_BYTES` above about 2.7 MiB now also needs
  `LCG_REQUEST_MAX_BYTES`. The startup error gives the exact minimum.
- **Memory.** The bound is per line plus read-ahead, not RSS. Tests assert the accumulator and
  the bytes consumed from the stream, not process memory.
- **Out of scope.** The MCP stdio transport (`--mcp-stdio`) reads stdin through rmcp's own reader
  and does not share this loop. `chunk_split`'s algorithm is not touched.
