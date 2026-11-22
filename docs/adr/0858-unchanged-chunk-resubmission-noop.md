# ADR-0858: `knowledge_process_chunk` No-Ops on an Identical Resubmission

**Status**: Accepted
**Date**: 2026-11-21
**Issue**: #858 (implements #837)

## Context

Since [ADR-0705](0705-belief-time-separate-from-validity.md), resubmitting a `chunk_id` to
`knowledge_process_chunk` supersedes the earlier live episode. That is right when the text changed,
but it also ran a full embed and LLM extraction, wrote a new episode and superseded an old one when
the resubmitted chunk was byte-identical: cost and churn with no information gain.
`knowledge_process_document` already skips unchanged chunks by content hash; single-chunk clients
kept their own hash ledgers, which can drift from what the graph actually holds.

## Decision

Before any embed or extract call, `knowledge_process_chunk` reads the live episodes of
`(group_id, chunk_id)`. If there is **exactly one**, it is attributable to `knowledge_process_chunk`
([ADR-0292](0292-chunk-ingest-never-supersedes-unattributed-episodes.md): `write_path` `chunk` or NULL), and
it matches the request on every compared field, the call writes nothing and returns
`{"success": true, "status": "unchanged", "chunk_id", "source_file", "episode_uuid": <existing>}`.
No embedder call, no extractor call, no WAL record, no oversize warning or telemetry. Any other
outcome takes the ordinary supersede path, unchanged.

The check lives in `handle_knowledge_process_chunk` (`crates/core/src/handlers.rs`), after all request
validation (a malformed request is still rejected) and after `state.check_identity` (an identity
refusal applies to a no-op as to an ingest). The comparison is the pure function
`chunk_unchanged::chunk_is_unchanged`, fed by `Conn::live_chunk_snapshot_for_chunk`, which uses the
same liveness predicate as `live_chunk_lineage_for_chunk`.

**"Identical"** means all of:

- **content**: byte equality with the stored `content`. Document mode hashes the exact source bytes
  (`sha256_hex(text.as_bytes())`, no trim, line-ending or Unicode folding), so byte equality is the
  same normalisation. If the stored row carries a `chunk_hash` it is compared with the hash of the
  request instead; chunk-mode rows store none.
- **attributes**: equal as parsed JSON, so key order and whitespace do not matter; a missing or
  non-object value is `{}`.
- **origin**: a NULL stored origin is `primary`, as in every other reader.
- **`source_file`**: the stored column; a legacy row without it must carry the exact
  `"{source_file}:{chunk_id}"` description, otherwise it counts as different.
- **`doc_id`**: omitted means none.
- **`reference_time`**: compared (as an instant, at microsecond precision, against `valid_at`) only
  when the request supplied one; an omitted value means "now" and is never a difference.

`knowledge_process_chunk` has no `title` parameter (it exists only for `knowledge_process_document`);
`source_file` is the chunk-mode analogue and is compared.

The shortcut never applies when `supersede` is `false` or `same_revision_episode_uuids` is supplied
(a multi-unit revision), when more than one live episode exists for the `chunk_id`, when any live
same-named episode is not chunk-attributable (a foreign `knowledge_add_episode` or a document-mode
episode), or when no live episode exists (never ingested, superseded, or removed). The check fails
closed: an unparsable or NULL stored value counts as different.

## Consequences

- A re-sync of N unchanged chunks costs no LLM or embedding calls and does not grow the graph.
  Clients no longer need their own content-hash ledgers for chunk re-syncs.
- `unchanged` responses carry no ingest counters (`nodes_extracted`, `superseded_episode_uuids`, ...).
  A client that read those fields on every `success: true` must tolerate their absence.
- A change of extractor, model or ontology does **not** by itself cause re-extraction of an
  identical chunk; alter the chunk or use the reprocess operations.
- The read is lock-free. Two identical resubmissions of a live chunk both return `unchanged`; a
  first-time chunk submitted concurrently takes today's path, and ADR-0705's commit-instant ordering
  leaves exactly one live episode.
- Additive: a new `status` value and the `unchanged_noop` feature string in `knowledge_capabilities`
  ([ADR-0854](0854-knowledge-capabilities-protocol-version-and-feature-registry.md)); `PROTOCOL_VERSION`
  is not bumped. This qualifies ADR-0705's "resubmitting supersedes" rule without changing its text.

## Out of scope

A force/override flag, re-extraction on configuration change, backfilling hashes, and applying the
same no-op to `knowledge_add_episode`.
