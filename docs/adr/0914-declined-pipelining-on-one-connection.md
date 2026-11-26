# ADR-0914: Pipelining Several Requests on One Connection — Declined

**Status**: Accepted
**Date**: 2026-11-25
**Issue**: #914 (part 2 of #836; see also [ADR-0914 (process-deadline-and-inflight-join)](0914-process-deadline-and-inflight-join.md))

## Context

#836 asked whether one connection could carry several in-flight requests, correlated by `id`, with
responses possibly out of order. Today a connection serves **one request at a time**: lines are read,
dispatched and answered in order ([ADR-0861](0861-process-chunk-progress-and-cancel-on-disconnect.md)),
and concurrency means opening several connections. The issue asked for a decision, with either answer
acceptable, and for the integrator guide to stay accurate.

## Decision

**Decline.** The service keeps one request in flight per connection. Clients that want concurrency
open N connections.

Reasons:

1. **Response order is part of the contract existing clients rely on.** `socat`/`nc` pipelines, shell
   scripts and any client that writes several lines and reads several replies assume the nth reply
   answers the nth line. Enabling pipelining by default would silently reorder their replies;
   enabling it only on an opt-in or a negotiated mode adds protocol surface (and probably a
   `protocol_version` bump) for a need that deadline and join now cover.
2. **Streaming frames carry no `id` or token.** `{"type":"progress",...}` and heartbeat lines
   ([ADR-0005](0005-streaming-ipc-progress-framing.md)) identify their request only by being the
   one in flight on the connection. Several concurrent streaming requests would need a frame field
   for the request, an amendment to ADR-0005 that every existing parser must tolerate.
3. **Disconnect and half-close handling are wired to a single request.** ADR-0861's rules (EOF versus
   full close, the 100 ms recheck, bounded frame writes, cancel-then-await-to-completion) assume one
   dispatch future per connection. With several, "the client disconnected" would have to cancel all
   of them, and a stalled frame write for one would stall the others unless every response went
   through a shared writer.
4. **Non-cooperative streaming methods are spawned and aborted**, and `knowledge_close` ordering is per
   connection. Both assume a single request in flight.
5. **The motivating cost is gone.** The community need was to bound a slow ingest and to retry it
   without paying twice. `deadline_ms` gives a structured answer and a safe retry, and a duplicate
   resubmit now joins the in-flight request, so the remaining benefit of pipelining is saving a few
   socket opens.

## Consequences

- The integrator guide and the IPC reference keep "one request at a time per connection; open one
  connection per concurrent request", and now point at this ADR and at `deadline_ms` and join for
  timeout-and-retry. "Progress frames carry no token because a connection never has two requests in
  flight" stays true.
- No pipelining feature string is added to `knowledge_capabilities`.
- Revisit only with a concrete client that cannot open connections cheaply; it would need an opt-in
  mode, an `id` on every frame and a `protocol_version` decision, designed together.
