### Added

- **`knowledge_move_document`: rename or move a document without re-extracting it (#903,
  [ADR-0903](docs/adr/0903-move-document-rekey.md), feature `move_document`).** A consumer that
  renamed a note had to remove it and ingest it again, paying for extraction twice on unchanged
  text. The new `write`-scope method re-keys a document's episodes from an old `doc_id` / source
  path to a new one (optionally rewriting chunk ids by prefix) with no extractor, LLM or embedder
  call; entities, relationships, embeddings, belief history and origin are untouched. `moves` is a
  batch applied all-or-nothing (a folder rename is one call). The destination must have no episodes
  or document state of any kind (no episode of any liveness at the new `doc_id`, path or chunk-id
  prefix, no revision or supersession record for the new `doc_id`); anything live in the way is
  `occupied_destination` (a merge, not a move), anything else `destination_has_history`. Also
  refused, with nothing changed and nothing logged: unknown source, cross-group, a chunk-id
  collision between moves of one batch, identical identity and batch conflicts.
  The batch is applied in one database transaction, so a failure part-way leaves nothing changed or
  logged and re-sending works; the refusals are also returned as structured `error.data.refusals`
  (`{index, code, message}`); and a move waits (up to 30 s) for an in-flight `knowledge_process_chunk`
  of the moving chunks.
  Re-keys the document-mode revision and document-supersession state too, so the next ingest at the
  new `doc_id` is still a no-op. Logged as fixed-valued, uuid-keyed updates, so rebuild, dump and
  overlapping replay agree. `dry_run` supported. MCP tool count 59 → 60, IPC methods 60 → 61.
  - Raw-socket clients: purely additive (a new method and the `move_document` feature string); no
    existing method's scope or response semantics change, so this is not breaking and
    `PROTOCOL_VERSION` is unchanged.
