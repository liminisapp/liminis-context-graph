# Response contract (JSON Schemas)

One [JSON Schema](https://json-schema.org/) (draft-07) per method result consumers read. They are
the machine-readable shape of what `liminis-context-graph` returns, versioned with each release
and attached to the GitHub release next to the binaries.

| File | Method |
|---|---|
| `health_check.schema.json` | `health_check` (healthy / degraded / busy) |
| `knowledge_status.schema.json` | `knowledge_status` (degraded / queryable / not queryable, `wal`, `wal_groups`) |
| `knowledge_search_passages.schema.json` | `knowledge_search_passages` |
| `knowledge_find_entities.schema.json` | `knowledge_find_entities` (including `include_episodes` provenance) |
| `knowledge_find_relationships.schema.json` | `knowledge_find_relationships` (including `belief`) |
| `knowledge_process_chunk.schema.json` | `knowledge_process_chunk` |
| `knowledge_lint.schema.json` | `knowledge_lint` (per-category `details`) |

**Do not edit these files by hand.** They are generated from the schema-only types in
[`crates/core/src/contract/`](../crates/core/src/contract/) and a test fails when they are stale:

```sh
LCG_UPDATE_CONTRACT=1 cargo test -p lcg-core --lib contract::
```

The schemas describe the method's `result` only: not JSON-RPC error objects, streamed progress
lines, or the MCP envelope. Additional properties are allowed, so adding an optional field is not a
breaking change. See [IPC & MCP Reference](../docs/ipc-mcp-reference.md#response-contract-json-schemas-issue-852)
for how to fetch and use them, and [ADR-0852](../docs/adr/0852-published-response-json-schemas.md)
for the design.
