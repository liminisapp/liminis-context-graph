# ADR-0859: `knowledge_retire_document`, and Hard Deletes Report (Optionally Prune) Orphaned Entities

**Status**: Accepted
**Date**: 2026-11-23
**Issue**: #859 (implements #838)

## Context

Two gaps, found by a client that tracks documents:

1. **No single call retires a whole document.** `knowledge_process_document` refuses empty
   content, `knowledge_mark_chunk_removed` works one chunk at a time, and
   `knowledge_supersede_document` ([ADR-0707](0707-document-supersession.md)) needs a successor. The
   only whole-document option was the hard delete `knowledge_delete_by_source`, which destroys the
   history — the wrong tool for "this is no longer believed, but keep the record".
2. **Hard deletes leave orphans, silently.** `knowledge_delete_by_source` and
   `knowledge_delete_chunk_episode` delete episodes only. An entity whose only mentions were those
   episodes stays in the graph and keeps coming back from `knowledge_find_entities`, and the caller
   cannot tell which entities were stranded.

## Decision

### Retire a document: soft, idempotent, no new record

`knowledge_retire_document {group_id, doc_id, effective_at?, origin?}` (write scope, `GroupRequired`
in `--writable-groups`) supersedes every live episode of the document with reason `document_removed`.
It reuses `supersede_episodes_by_uuids(.., None, "document_removed")`, the `chunk_removed` shape
(`superseded_by` and `superseded_by_kind` are NULL), so nothing is deleted, `valid_at` / `invalid_at`
are never written and the statement is uuid-keyed and fixed-valued: WAL replay, dump compaction and
checkpoint restore reproduce it, and replaying an overlapping tail is idempotent.

- **Idempotent.** A document with no live episode reports `superseded_count: 0` and writes nothing —
  not even a WAL record. "Live" is `superseded_at IS NULL`, exactly as
  `knowledge_supersede_document` reads it, so a document already superseded by another document
  (even with a pending future date) is a no-op and keeps its reason.
- **Rejected with no side effects**: unknown `doc_id`, omitted `group_id`, `effective_at` without a
  timezone offset, or (ADR-0752) a `derived` / `answer` actor retiring a `primary` / `external`
  document. All checks complete before any mutation.
- **Concurrency.** It takes the per-document ingest lock and then the write lock, the order
  `knowledge_process_document` uses, so it cannot interleave with an in-flight ingest of the same
  document. It supersedes pending (uncommitted) revision episodes as well, like
  `knowledge_supersede_document`.
- **No durable record, so a limited `effective_at`.** A document supersession stores a
  `DocSupersession` row because it needs a *superseding* document; a removal has none, and forcing a
  sentinel into that table would break ADR-0707's one-superseding-document and cycle invariants. So
  `superseded_at = max(recorded, effective_at)`. A **future** `effective_at` keeps the facts believed
  *and in force* until then (`belief.supersession.cause: document_removed`, `in_force_until` =
  `superseded_at`); a **past** one behaves as "removed now" — the retroactive "believed but no longer
  in force" window of `knowledge_supersede_document` is not available. `in_force` and
  `supersession_info` in `belief.rs` gain a `DocumentRemoved` cause for this; every other read keys
  on "no superseder" and treats the reason like `chunk_removed` (lint maps it to `removed`).
- Un-retiring is out of scope (ADR-0707: withdrawing a supersession is a later primitive). Re-ingesting
  the document under the same `doc_id` creates live episodes as usual.

### Hard deletes: report by default, prune only on opt-in

Both hard deletes return `orphaned_entity_uuids` and `orphaned_entity_count`, plus
`pruned_entity_count` / `pruned_edge_count` (always present, `0` without pruning). An **orphan** is an
entity that lost a mention in this delete and is mentioned by **no existing episode — superseded ones
included**. Candidates are collected from the doomed episodes' `MENTIONS` before the delete (the
rels go with the episodes) and re-checked after, inside the existing per-group transaction.

- **Report, do not delete, by default.** Deleting an entity also destroys every edge touching it, so
  a bug or a wrong definition loses readable history irrecoverably. Reporting is free of that risk,
  and a caller who wants the old behaviour sees no change beyond added fields.
- **`prune_orphans: true` is explicit.** It deletes the orphans, the group's `RelatesToNode_` rows
  that cited a deleted episode and now have no surviving episode (a warranted edge has none to lose
  and is never a candidate), and any group-owned edge hopping to a pruned entity — so no edge is left
  dangling. Edges go first, then entities. The statements are uuid-keyed `DETACH DELETE`s (plus the
  direct `Entity→Entity` compat rel), drained into the owning group's WAL bucket (ADR-0402), so a
  rebuild does not resurrect the rows. A forced cross-group pointer rebind for the group follows, as
  in the group purge (ADR-0361), so a foreign layer's pointer at a pruned entity records `unbound`
  rather than going stale.
- **Scope.** Only entities that lost a mention in this call are considered: pre-existing orphans
  elsewhere in the graph are not swept, and asserted entities that were never mentioned are neither
  reported nor pruned. `prune_orphans` never widens `group_ids`; candidates and edge scans are
  filtered to the group being deleted.

### Orphan definition is deliberately stricter than lint's

`knowledge_lint`'s `orphan_entity` and the entity reads (ADR-0857/ADR-0883) count only **live**
mentions as support, so an entity mentioned only by superseded episodes is `support_lost` there. For
deletion the definition is "no existing mention at all": that entity's history is still readable, and
a delete that is about to remove data must not remove more than the caller asked for. The two
therefore diverge on purpose.

### Relationship to #361, #828 and ADR-0707

- **#361 group purge (ADR-0361).** The purge removes everything in a group. `prune_orphans` is a
  narrow, per-delete sibling: it removes only what *this* delete stranded and does not replace the
  purge. `knowledge_delete_by_group` is unchanged. Retire leaves the `DocRevision` row alone, whereas
  the purge clears it.
- **#828 retention research (ADR-0828).** ADR-0828 decided *not* to prune superseded history yet and
  sketched what an eventual prune must do (write to the owning group's WAL stream, check cross-group
  pointers). Nothing here conflicts: retire deletes nothing, and `prune_orphans` touches only the
  entities and edges of episodes the caller has just hard-deleted, never superseded history. It is
  not a retention policy, and a group-wide orphan sweep remains out of scope. Its wider orphan rule
  (no `MENTIONS` *and* no edge endpoint) is a different definition for a different operation.
- **ADR-0707.** Extends the supersession reasons with `document_removed`; leaves its record,
  cycle and conflict rules alone.

## Consequences

- One new IPC / MCP tool (`knowledge_retire_document`; 60 IPC methods, 59 MCP tools, write scope 19)
  and the `retire_document` feature string ([ADR-0854](0854-knowledge-capabilities-protocol-version-and-feature-registry.md)).
- Additive response fields on both delete methods and an optional `prune_orphans` boolean, so
  existing clients are unaffected.
- `Conn::remove_episodes_by_source` / `remove_episodes_by_chunk_id` take a `prune_orphans` argument
  and return a `DeleteOutcome`.
- `prune_orphans` runs a group-scoped scan of `RelatesToNode_(uuid, episodes)` (list membership has no
  index), only when requested.
- Known limit: a past `effective_at` on retire is not retroactive (see above).
