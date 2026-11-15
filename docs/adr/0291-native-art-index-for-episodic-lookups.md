# ADR-0291: Native ART Indexes for Episodic `name` and `doc_id` Lookups

**Status**: Accepted
**Date**: 2026-11-15
**Issue**: #291. Extends [ADR-0221](0221-secondary-art-index-for-entity-name-lookup.md) to the
`Episodic` table. Resolves the accepted cost recorded for chunk-id idempotency (the historical
"ADR-0053" cited in the issue is not a file in `docs/adr/`; `0053` is only a pre-consolidation
number, see the *Historical numbering* appendix of `index.md`) and the "`doc_id` lookups are
scans" consequence of [ADR-0707](0707-document-supersession.md).

## Context

Every `knowledge_process_chunk` call looks up live episodes with `Episodic.name = chunk_id` in the
target group (`Conn::live_episode_uuids_for_chunk`, `episode.rs` Phase C). Since ADR-0705,
`supersede` defaults to `true`, so this runs on **every** call, including first-time ingest. The
`Episodic` primary key is `uuid`; nothing indexed `name` or `doc_id`, so each lookup was a full
`Episodic` scan and a bulk ingest of N chunks into one group was O(N²) (the 4,374-chunk corpus of
#202). `doc_id` lookups (`doc_is_known`, `live_episode_uuids_for_doc`, ADR-0707) and the
correction reads / removals by name (`get_episode_rows_by_name`, `remove_episodes_by_chunk_id`)
had the same shape.

The issue asked for an in-process accelerator in the style of ADR-0038's `NameIndex`. That
pattern no longer exists: ADR-0221 replaced it with a database-native ART index once lbug 0.18.0
added non-primary-key secondary indexes (the repository now pins lbug 0.21.2).

## Decision

Create two native ART indexes, built by `Conn::create_episodic_lookup_indexes()` from
`build_indices_and_constraints()` and, non-fatally, from the `recover_drop_lbug_wal` and
`recover_restore_from_backup` strategies (which skip that function, as for
`entity_lookup_key_idx`):

```
CREATE ART INDEX episodic_name_idx   FOR (ep:Episodic) ON (ep.name)
CREATE ART INDEX episodic_doc_id_idx FOR (ep:Episodic) ON (ep.doc_id)
```

- **Single-column indexes, `group_id` stays a residual filter.** Equality on `name` / `doc_id` is
  highly selective. `tests/episodic_lookup_index.rs` runs `EXPLAIN` on every real query shape
  (the compound `name AND group_id AND (superseded…)` predicate, `group_id IN [...]`,
  `group_id AND doc_id`) and confirms the ART access path, with a no-index negative control.
  No materialized key column was needed, so no schema column, no `kuzu_driver.py` parity
  divergence, and no Rust-side backfill.
- **No query was changed.** The existing Cypher already used plain equality.
- **Idempotent**, using the same "already exists in catalog" suppression as ADR-0221.

### How this meets the spec's in-process-map requirements

The spec (FR-004 – FR-008) described hooks on every create/remove path, WAL-replay sync, cold-start
population and verify-before-trust. A database-maintained index satisfies the intent without
code:

| Requirement | How it holds |
|---|---|
| FR-004 create | the DB indexes every `CREATE (:Episodic …)`, whichever caller wrote it |
| FR-005 supersede / remove / purge | supersession only `SET`s unindexed `superseded_*`; `DETACH DELETE` paths (`remove_episode`, `_by_source`, `_by_chunk_id`, group purge) update the index |
| FR-006 WAL replay | replay executes raw Cypher on the same table; replayed rows are indexed (tested) |
| FR-007 / FR-008 | the DB is the only source of truth: staleness, a false "not found" and cold start cannot occur; an existing workspace gets the index built over its rows at next open |
| FR-011 | nothing in-process, no stored-column change |

NULL `doc_id` (legacy / caller-omitted) and a 4,000-character `name` index and look up correctly
(tested).

## Consequences

- Per-call chunk-id and `doc_id` lookup cost no longer grows with group size
  (`benches/episode_lookup.rs` compares a scan baseline with the indexed lookups at 1k/4k/10k;
  run on demand with `gh workflow run bench.yml`).
- **First open after upgrade builds both indexes over existing `Episodic` rows** — a blocking,
  one-shot cost, as for ADR-0221's Entity index. Its duration at scale is unmeasured.
- Index maintenance adds a small cost to each episode write and to bulk deletes; the bench
  measures only the lookup side.
- **Nothing here is transitional.** The issue expected #221's follow-up to delete an in-process
  map; because the native-index route was available, only two `CREATE ART INDEX` statements ship
  and there is nothing to delete.
- Rejected: the in-process `EpisodicNameIndex` map. It would have needed hooks on four delete
  paths plus the group-purge bulk delete, rollback handling in two transactional removals, a
  WAL-replay resync (replay bypasses `insert_episodic`) and verify-on-hit, and would duplicate what
  the database already provides.
