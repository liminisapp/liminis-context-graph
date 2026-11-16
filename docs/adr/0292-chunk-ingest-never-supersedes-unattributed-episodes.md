# ADR-0292: Chunk Ingest Never Supersedes Episodes It Cannot Attribute To Itself

**Status**: Accepted
**Date**: 2026-11-16
**Issue**: #292 (numbered by issue, per the ADR numbering rule)
**Builds on / amends**: ADR-0705 (belief time; revision supersedes by `name` within the group),
ADR-0752 (episode origin; precedent for a nullable `Episodic` column), ADR-0028 (dump parity).

## Context

Issue #292 was written against a `DETACH DELETE` replace path (#284 / PR #286) that never merged.
`main` instead meets chunk idempotency with belief-time supersession (ADR-0705): resubmitting a
`chunk_id` to `knowledge_process_chunk` ends belief in the chunk's earlier live episodes in the group,
once the new ingest has fully succeeded. "The chunk's earlier episodes" meant every live
`Episodic` with `name = chunk_id`, and nothing else.

`knowledge_add_episode.name` is free-form, so a well-behaved integrator can write
`name = "chunk-42", source_description = "notes:chunk-42"` — byte for byte what
`knowledge_process_chunk(source_file="notes", chunk_id="chunk-42")` writes — and a later routine chunk
ingest silently ended belief in that episode. The consequence is milder than the original deletion
(the row persists and can be read as history) but the caller's episode drops out of current-belief
reads as a side effect of a call that never named it. Name and `source_description` cannot tell the two
writers apart, so no text-based rule can fix it; a provenance marker is required.

## Decision

**Routine chunk ingest never supersedes, deletes or alters an episode it cannot attribute to its own
prior writes. Losing idempotency on a collision is cheap; ending belief in data the caller never named
is not.**

1. **A provenance column.** `Episodic.write_path STRING`, nullable, added by the existing
   probe-then-`ALTER TABLE ... ADD` loop with no backfill. It is distinct from `origin` (ADR-0752),
   which is the *kind of content*; `write_path` is *which write path produced the row*. Values:
   `chunk` — `knowledge_process_chunk`; `episode` — every other path (`knowledge_add_episode`, the
   library `add_episode`, the eval runner), via `EpisodeOpts::write_path` whose default is `Episode`;
   NULL — written before the column existed.
2. **Own writes are recognised by an allow-list.** A live same-named episode is the chunk's own prior
   lineage iff `write_path IS NULL OR write_path = 'chunk'`. Any other value — including one a future
   write path introduces — is left alone, so a new writer fails safe, not destructive. A deny-list
   (`<> 'episode'`) was rejected for exactly that reason.
3. **Ambiguity is judged per row, and the outcome is a fresh ingest.** Attributable rows are
   superseded as before (`supersede: false` and `same_revision_episode_uuids` honoured); the others
   are left untouched and live. The call succeeds — there is no error outcome, so bulk re-indexing
   never stalls on a collision. Each left-alone row is logged at `WARN` naming its UUID and
   `source_description`, and the UUIDs are reported in an additive `left_alone_episode_uuids` result
   field, **present only when non-empty** so an unambiguous response is unchanged.
4. **The lookup is a new method, not a changed query.** `Conn::live_chunk_lineage_for_chunk` returns
   the partition. `live_episode_uuids_for_chunk` is unchanged, so the explicit by-name operations keep
   their contracts (below). `name`/`group_id` remain the indexed anchors (ADR-0291); the marker is a
   post-filter.
5. **The #752 origin guard follows the same rule on the revise path.** A `derived`/`answer` chunk is
   refused only by a live primary/external episode it would actually revise (an eligible one). A
   foreign same-named primary row neither is superseded nor causes a refusal
   (`Conn::live_revisable_origins_for_chunk`). `knowledge_mark_chunk_removed`'s guard stays by-name.
6. **WAL and dump carry the marker.** `insert_episodic` logs `write_path` as a parameter, so replay
   reproduces it; a pre-feature WAL has no such property and replays to NULL (legacy), which is
   correct. `dump.rs` gained the column in its Cypher template, page query and row mapping (appended
   last, empty → JSON null), so a dump → rebuild round trip preserves it, NULL included.
7. **Not exposed on reads.** `write_path` is not selected by episode reads, so `knowledge_get_episodes`
   and the other read shapes are unchanged.

## Consequences

- **Legacy rows remain exposed (top-level limitation).** Every episode written before this change has
  a NULL `write_path` and is treated as the chunk's own, to keep today's behaviour and avoid duplicate
  lineages when a pre-upgrade corpus is re-ingested. A foreign `knowledge_add_episode` row written
  *before* upgrading whose `name` equals a `chunk_id` is therefore still superseded by a chunk ingest.
  So are rows written by raw Cypher (the `cypher` scope) or any other writer that leaves the column
  NULL. There is no backfill: NULL cannot be told apart from "written by `knowledge_process_chunk`".
  A test pins this behaviour so it cannot change silently.
- A post-upgrade foreign row is never superseded by a chunk ingest, whether or not its
  `source_description` mirrors `"{source_file}:{chunk_id}"`, and whether or not the incoming text
  matches.
- A collision costs a duplicate lineage, not data: the caller's episode and the new chunk episode are
  both live. The caller can rename to resolve it.
- **By-name operations are unchanged on purpose.** `knowledge_mark_chunk_removed` and
  `knowledge_delete_chunk_episode` have an explicit, caller-named "everything with this name"
  contract and still act on foreign same-named rows. That is not a gap.
- One nullable column on `Episodic` (no storage-version change). Group scoping (ADR-0368/0371) and the
  #707 pending-document-supersession clause are preserved.
- Concurrent resubmission (#288) and the name-scan cost (#291) are unaffected.
