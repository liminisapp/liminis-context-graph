### Changed

- **BREAKING: request lines over 16 MiB and `chunk_text` over 512 KiB are now refused (#293).**
  Affects raw-socket clients (the Electron app through `graphiti_service.py`, the attached
  `--connect` MCP client, and any other client that writes JSON-RPC lines to the socket or named
  pipe).
  - The socket read loop used to buffer a request line without bound. It now stops at
    `LCG_REQUEST_MAX_BYTES` (default 16 MiB, at most 2 GiB; a byte count or a `KiB`/`MiB`/`GiB`
    suffix) and answers with JSON-RPC error `-32005`, `id: null` and
    `error.data = {reason: "request_too_large", limit_bytes, setting}`. The line is refused before
    parsing, so no handler runs. The rest of the line is discarded without being kept and the same
    connection keeps serving; a line that runs past four times the limit closes the connection.
    The default carries a maximal 2 MiB `knowledge_process_document` after worst-case JSON escaping.
  - `knowledge_process_chunk` refuses a `chunk_text` over `LCG_CHUNK_TEXT_MAX_BYTES` (default
    512 KiB, at most 256 MiB, in UTF-8 bytes) with `chunk_text_too_large`, before any extraction or
    write. The `LCG_CHUNK_TEXT_ADVISORY_MAX_CHARS` warning is unchanged below the cap. Send long
    texts to `knowledge_process_document`.
  - Startup now refuses a configuration whose line limit cannot carry a maximal document:
    `LCG_REQUEST_MAX_BYTES` must be at least `6 x LCG_DOCUMENT_MAX_BYTES + 64 KiB`. Operators who
    raised `LCG_DOCUMENT_MAX_BYTES` above about 2.7 MiB must also raise `LCG_REQUEST_MAX_BYTES`;
    the error names both settings and the minimum. Invalid or empty values of the new settings are
    fatal, like `LCG_DOCUMENT_MAX_BYTES`.

### Added

- **`request_size_limits` capability feature (#293).** Advertised by `knowledge_capabilities` so a
  client can detect the request-line limit and the `chunk_text` cap.

### Documentation

- **Request limits (#293).** The IPC & MCP reference, integrator guide and configuration reference
  describe the limits, the `-32005` error and the post-refusal behaviour. ADR-0293 records the
  decisions, and notes that ADR-0284 and `chunk_split` are not on `main`.
