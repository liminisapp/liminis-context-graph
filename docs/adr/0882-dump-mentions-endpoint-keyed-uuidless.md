# ADR-0882: `knowledge_dump_wal` Dumps uuid-less MENTIONS Edges, Keyed on `(episode, entity, group_id)`

**Status**: Accepted
**Date**: 2026-11-22
**Issue**: #882 (finding F1 of the #828 research, "Incidental findings")

## Context

`Conn::insert_mentions_edge` — the native-ingest write path shared by `process_chunk`,
`process_document` and `add_episode` — creates `(ep)-[:MENTIONS {group_id}]->(en)` with **no
`uuid`**. The dump's MENTIONS phase skipped every row whose `uuid` was empty ("pre-migration
edges"), so a graph compacted with `knowledge_dump_wal` and rebuilt from the dump had **zero**
MENTIONS edges (792 of 792 skipped in the #828 measurements). Nodes and relationship edges
survived, so the loss was silent. Everything that reads MENTIONS for entity→episode provenance
(`episode_uuids`/`source_descriptions`, `find_entities` `include_episodes`, the `knowledge_walk`
mention fallback, `support`/`corroboration`) came back empty.

No MENTIONS reader touches `r.uuid`; all match on the endpoints. The edge uuid is only used by
graphiti-style bulk deletes by uuid.

## Decision

- **Identity of a MENTIONS edge is `(episode uuid, entity uuid, group_id)`.** The dump's MERGE is
  `MERGE (ep)-[r:MENTIONS {group_id: $group_id}]->(en)`. The edge uuid is not part of the key, so
  no backfill is needed and existing uuid-less graphs dump correctly. Replay stays idempotent
  (ADR-0026, ADR-0028): a second replay matches the existing edge. Distinct groups are not
  collapsed.
- **Two templates.** Rows with a uuid use `MENTIONS_CYPHER` (also `SET r.uuid`); uuid-less rows
  use `MENTIONS_NO_UUID_CYPHER`, which leaves `r.uuid` NULL on the rebuilt edge. The dump emits no
  "null uuid — skipping" warning any more.
- **Total paging order.** `Conn::dump_mentions_page` orders by `ep.uuid, en.uuid, r.group_id`.
  Many rows share an `ep.uuid`, so `ORDER BY ep.uuid` alone is not total and SKIP/LIMIT paging
  (`PAGE_SIZE` 500) could drop or repeat rows across a page boundary. This is MENTIONS-only; the
  other edge phases (same latent shape) are untouched (FR-009) and are a follow-up.

## Alternatives Considered

- **Mint a uuid at write time.** Does not cover existing uuid-less rows, so the dump-side change
  is needed regardless; and it changes ingest for no reader's benefit.
- **Bind JSON null / `""` for `$uuid` in one template.** Binding an untyped null into a `STRING`
  column is unverified, and a prepare/bind failure is attributed to every row sharing the
  template (silently dropping all MENTIONS). `""` would write a bogus uuid.
- **De-duplicate `entity_uuids` in ingest.** Beyond a dump fix.

## Consequences

- A dump→rebuild preserves every distinct `(episode, entity, group_id)` MENTIONS tuple, and
  provenance readers return the same results.
- **Caveat:** ingest can `CREATE` parallel duplicate `(ep, en)` edges when two mentions resolve to
  the same entity. The MERGE-based rebuild collapses them, so the rebuilt *count* can be lower
  than the source count while the tuple set is identical. Parity is defined on distinct tuples.
- `edges_dumped` now includes MENTIONS edges that were previously skipped.
- Rows with a NULL `group_id` come back with `group_id = ""`; readers match on endpoints.
