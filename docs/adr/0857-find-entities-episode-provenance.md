# ADR-0857: `find_entities` Episode Provenance and a `uuids` Filter on `get_episodes`

**Status**: Accepted
**Date**: 2026-11-22
**Issue**: #857 (implements #831)

## Context

`knowledge_find_entities` always returned empty `episode_uuids` and `source_descriptions`, while
`get_entity_neighbors` and `get_entities_by_source` filled them, so clients were misled (#831).
The handler already derives each hit's mentioning episodes for its `support` counts
(`origin_reads::entity_support`) but kept only per-origin document counts. A client holding
episode UUIDs also had no way to fetch just those episodes.

## Decision

- **Opt-in `include_episodes`** (strict boolean, default `false`) on `knowledge_find_entities`.
  Without it the response is byte-identical: `EntityRow` and `ScoredEntity` are untouched and the
  new keys are written onto the serialized JSON only when the flag is set.
- **One query, shared.** `Conn::entity_mention_rows` is the single batched MENTIONS query,
  extended with `source_description`, `source_file` and `created_at`. `entity_mention_origins`
  is a thin wrapper that keeps its tuple shape for `entities_with_corroborating_mention`.
  `entity_support_with_episodes` collects the live episodes during the same loop that builds the
  `support` tally; `entity_support` is the flag-off wrapper. No per-hit query.
- **Liveness** is `EpisodeOriginInfo::is_live`, the predicate `support` uses. It does not also
  exclude pending document revisions, which `get_entities_by_source` does.
- **List shapes.** `episode_uuids` and `source_descriptions` are index-aligned and not
  de-duplicated (as on `get_entity_neighbors`). `source_files` is de-duplicated, in first-appearance
  order, carries only stored `source_file` values (no legacy attribution fallback) and is therefore
  not aligned. `episodes_truncated` is present only when `true`.
- **Cap and order.** At most `ENTITY_EPISODE_CAP = 50` episodes per entity, newest first
  (`created_at DESC, uuid DESC`, the `get_episodes` order), so truncation is deterministic. The cap
  limits the response only; `support` always counts every live document.
- **`uuids` on `get_episodes`.** `ep.uuid IN $uuids` joins the existing predicates in
  `retrieve_episodes_page_by_uuids` (the original method wraps it), so group scope is unchanged. An
  empty array short-circuits to no episodes; unknown uuids are absent; duplicates collapse.
  `last_n` defaults to `uuids.len()` when `uuids` is given and `last_n` is not, and an explicit
  `last_n` still caps. `uuids` does not by itself enable paging. A cursor is bound to the uuid set
  through `read_page::episodes_uuids_shape_hash`, a separate function so the pinned `shape_hash`
  and live cursors are untouched.
- **Capability.** `entity_provenance` is appended to the feature registry (ADR-0854); no
  `PROTOCOL_VERSION` bump, since the change is additive.

### Neighbors parity reads as "live subset"

`get_entity_neighbors` and `get_entities_by_source` fill `episode_uuids` from
`get_episode_info_for_entities`, which has **no** liveness filter, so they include superseded
episodes. Changing them is out of scope. FR-004 ("equals the live episodes `get_entity_neighbors`
reports") is therefore implemented as: a `find_entities` hit's `episode_uuids` equals the *live
subset* of the neighbors set. The reference docs state the difference per method.

## Consequences

- Flag-off `find_entities` and every other read are unchanged; additive IPC change only.
- `find_entities` with the flag still loads every mentioning row for its hits (as `support` already
  does); the cap bounds only the response.
- `support` (distinct documents) and the capped list (episodes) need not agree; documented.
- Rejected: making neighbors / by_source live-only (breaking behaviour change); a second sibling
  query (two round-trips); extending `SupportTally` (shared with lint and edge support, and it
  counts documents, not episodes); a request-tunable cap (out of scope).
