# ADR-0667: Read-Path Paging Cursor, Projection, All-Groups Episode Default and Status Label

**Status**: Accepted
**Date**: 2026-10-01
**Issue**: #667 (implements the community report #648; builds on #413's omitted-`group_ids` rule)

## Context

An agent reading a ~600-entity / ~360-episode graph over MCP hit three problems:

1. `knowledge_get_episodes` ignored `group_ids` (which the Liminis app already sends) and fell
   back to the single `liminis` group, so a host not using `liminis` got nothing.
2. `get_episodes(last_n=200)` is ~363K characters and `list_entities(num_results=1000)` ~485K —
   over typical tool-result limits — with no way to page, drop large fields, or narrow by name.
3. In per-group WAL layout the flat `knowledge_status.wal` block read `exists: false,
   generation_status: "not_applicable"` when the default group had no stream, although
   `wal_groups` showed every other group hydrated.

## Decision

**Group scope.** `get_episodes` parses `group_ids` with the shared `extract_optional_group_ids`
and keeps `group_id` as an alias; both given ⇒ deduplicated union; neither (or `[]`) ⇒ all groups,
matching every other read (ADR-0615 D2, "reads broad, writes scoped"). This is a deliberate
patch-level behaviour change, called out in the CHANGELOG.

**Keyset cursor, not offset.** Both orderings are newest/highest first, so an offset drifts as
rows arrive. The cursor is hex-encoded JSON `{v, t, shape, pos}` (no new dependency). `pos` is
`{uuid}` for entities and `{created_at_us, uuid}` for episodes. `shape` is a SHA-256 of the tool
tag, sorted group set, `kind` and `name_prefix` — everything that changes membership or order —
so a cursor replayed against a different query is rejected rather than silently paging a
different result set. `fields` is excluded: projection never changes membership. Handlers fetch
`limit + 1` rows to decide whether `next_cursor` is `null`.

**Episode ordering gains a `uuid` tiebreaker** (`created_at DESC, uuid DESC`). Without it paging
over equal timestamps is undefined. This changes only the previously arbitrary order of rows tied
on `created_at`. The rendered `created_at` string is second-precision, so the cursor carries
microseconds taken from the raw `TIMESTAMP`, and the keyset predicate binds it as a typed
timestamp (the param is named `created_at`, which `json_value_for_param` coerces), comparing
natively at microsecond precision.

**`next_cursor` only when asked.** Byte-identity with today's responses is a requirement, so the
key is emitted only when the request contained `cursor`, `fields` or `name_prefix`. Starting a
paged read is `cursor: ""`. This stays within ADR-0020, which allows extra envelope metadata and
requires `count == len(page)`.

**Projection** is a Rust-side key filter over the serialized item, so the row structs and default
serialization are untouched; embeddings are `#[serde(skip)]` and can never appear. For entities
the episode-provenance enrichment query is skipped when neither `episode_uuids` nor
`source_descriptions` is requested.

**Errors** use `Error::Ipc` (`-32000`) like every other validation failure; core has no `-32602`
path and adding one is out of proportion for a patch.

**Entity `name_prefix`** is `lower(e.name) STARTS WITH lower($p)`, which pushes down into `LIMIT`.
lbug's Unicode case folding is not guaranteed to match the host's, so case-insensitivity is
guaranteed for ASCII only (the same caution as `db.rs`'s lookup-key code). Episodes use a plain
case-sensitive `STARTS WITH`. Metacharacters are literal.

**`attributes` filter on episodes is deferred.** The column is a JSON string (ADR-0528); a
faithful key/value match needs a Rust post-filter that defeats `LIMIT` push-down, or a lossy
`CONTAINS`.

**Status label, not omission.** The flat `wal` block is the compatibility contract for existing
consumers (ADR-0378, #378 FR-007), so it is kept and, when a WAL root is configured, gains
`scope: "default_group"`, `default_group` and `see: "wal_groups"`. With no WAL root it is unchanged.

## Consequences

- A caller that omitted the group on `get_episodes` while holding several groups now receives
  every group's episodes.
- Paging is stable under concurrent writes; items deleted between pages cause no error.
- Existing consumers of `knowledge_status.wal` see only added keys.
