### Added

- **Embedder roles and instruction profiles (#790, ADR-0790).** Every embed call now states whether
  the text is a query (a search string) or a document (stored text), so asymmetric models (BGE,
  nomic, e5) can get the prefixes they were trained with.
  - `LCG_EMBEDDING_PROFILE` takes `none` (default), `bge-v1.5`, `nomic-v1.5`, `e5` or `embedder`;
    `LCG_EMBEDDING_QUERY_PREFIX` and `LCG_EMBEDDING_DOCUMENT_PREFIX` override one side each. An unknown
    profile fails startup and lists the valid names. With `embedder`, the embedder applies its own
    instruction when it advertises support (`input_type`), and a text never receives two instructions.
  - **With no configuration nothing changes:** under `none` every string sent to the embedder is
    byte-identical to before.
  - The document prefix is part of the workspace's embedding identity, so changing it reports
    `mismatch` until a rebuild records the new one; the query prefix is not, and needs no re-embed.
  - `knowledge_status` gains additive `embedding_instructions` and `embedding_doc_prefix` fields. The
    macOS sidecar accepts an optional `input_type` and reports its real model id
    (`coreml-<model file stem>`, still `coreml-bge-base-en-v1.5` by default).
  - See [configuration](docs/configuration.md#embedding-instruction-profiles).
