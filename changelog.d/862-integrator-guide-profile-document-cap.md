### Upgrading

- **`LCG_DOCUMENT_MAX_BYTES` is now validated strictly at startup (#862).** An invalid value now
  **fails startup** with an error naming the variable and the value, where it used to log a warning and
  fall back to the default. Invalid means empty, non-numeric, `0`, negative, a decimal, `4MB`/`4m`
  (only a byte count or a `KiB`/`MiB` suffix is accepted) or above 256 MiB. **An empty
  `LCG_DOCUMENT_MAX_BYTES=` is fatal too**: unset the variable instead of exporting it empty. Check
  the environment your launcher or service manager passes before upgrading. Affects every deployment
  that sets the variable. The default
  rises from 512 KiB to 2 MiB, which refuses nothing that was accepted before.

### Added

- **Integrator guide and minimal client profile (#862, ADR-0862).** New
  [`docs/integrator-guide.md`](docs/integrator-guide.md), linked from the docs index and the README:
  the transport (socket, newline-delimited JSON-RPC, one request at a time per connection, timeouts),
  discovery with `knowledge_capabilities`, groups and their defaults, ingest / update / retire / delete
  side by side, search and neighbours with recommended parameters (`min_score`,
  `LCG_SEARCH_MIN_SCORE`, `include_episodes`), provenance and citations, and health checks. It
  defines a **minimal client profile** of nine methods whose names, required parameters, documented
  result fields and error codes are kept compatible across releases: `health_check`,
  `knowledge_capabilities`, `knowledge_process_chunk`, `knowledge_process_document`,
  `knowledge_retire_document`, `knowledge_delete_by_source`, `knowledge_search_passages`,
  `knowledge_find_entities` and `knowledge_get_entity_neighbors`. A test fails if one leaves the
  dispatcher or the MCP registry (bar `health_check`, which is not an MCP tool), if a required
  parameter changes, or if the guide's list and the test's disagree.
- **`document_max_bytes_env` capability feature (#862).** Advertised by `knowledge_capabilities`
  when the server has the 2 MiB default and the strictly validated `LCG_DOCUMENT_MAX_BYTES`.

### Changed

- **`knowledge_process_document` accepts documents up to 2 MiB by default (#862, ADR-0862).** The
  cap was 512 KiB, which refused real web pages of about 600 K characters. A larger document is
  still refused with `document_too_large` (the message names the cap and `LCG_DOCUMENT_MAX_BYTES`)
  and is never truncated. A 2 MiB document of mock-provider ingest peaked at about 0.6 to 0.7 GB
  RSS in a debug build and took 17 to 41 s; see the ADR for the measurements.
- **The oversize `knowledge_process_chunk` `warning` and tool description now point at
  `knowledge_process_document` (#862)** instead of telling the caller to split the input. The
  warning's `type` and fields are unchanged; only the `message` text changed.
