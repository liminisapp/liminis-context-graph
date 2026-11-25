### Added

- **`deadline_ms` on `knowledge_process_chunk` and `knowledge_process_document` (#914, part 2 of #836).**
  An optional positive integer number of milliseconds, counted from when the service starts the
  request. A request that cannot reach commit in time is abandoned at its next cancellation point and
  returns a new error, JSON-RPC code `-32005` with `error.data = {"reason": "deadline_exceeded",
  "phase": …, "committed": false}`; nothing is written for a chunk (a document adds `"partial": true`:
  units already committed stay and nothing is retired). A deadline that passes once the commit has
  started does not interrupt it, and the success result carries `"deadline_exceeded": true`. Raw-socket
  clients: an unknown error code `-32005` is new, and the success key is additive; both are ignorable.
  Advertised as the `process_deadline` feature; both MCP tools list the parameter.
- **A duplicate in-flight `knowledge_process_chunk` joins the first instead of extracting again
  (#914).** An identical request (same text, attributes, origin, `source_file`, `doc_id` and
  `reference_time`) for a `(group_id, chunk_id)` that is already being ingested waits for it and
  returns the same result with `"joined": true`. If the first request is cancelled by a disconnect or
  its deadline, the joined one is not: it takes over and still succeeds. A failure of the first
  request is shared. Advertised as the `chunk_join_inflight` feature. Raw-socket clients that
  retried a timed-out chunk on a new connection and previously paid for a second extraction now get
  the first one's result.

### Changed

- **Two identical resubmissions that arrive together no longer both extract (#914).** The earlier
  behaviour ([ADR-0858](docs/adr/0858-unchanged-chunk-resubmission-noop.md)) is superseded in part by
  [ADR-0914](docs/adr/0914-process-deadline-and-inflight-join.md). Not **BREAKING**: results gain only
  optional keys, and `protocol_version` stays 1.

### Response schemas

- **`knowledge_process_chunk` result gains optional `joined` and `deadline_exceeded` (#914).** Both
  are booleans that are present only when `true`; additional properties were already allowed.

### Documentation

- **Pipelining several requests on one connection was considered and declined (#914,
  [ADR-0914](docs/adr/0914-declined-pipelining-on-one-connection.md)).** The integrator guide and the
  IPC reference keep "open one connection per concurrent request" and now recommend `deadline_ms` and
  join for timeout-and-retry.
