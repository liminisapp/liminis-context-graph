# ADR-0615: Kind-Scoped Entity Identity — `Entity.kind` and `lookup_key = group_id ␟ kind ␟ name`

**Status**: Accepted
**Date**: 2026-09-29
**Issue**: #615 (implements community report #614)
**Amends**: ADR-0221 (`lookup_key` composition, uniqueness wording), ADR-0283 (the scan-fallback
authority is kind-aware), ADR-0369 (pointers gain an optional kind), ADR-0526 (`lookup_key` joins
vectors as a derived value stripped on write)

## Context

Entity identity was `group_id ␟ lower(trim(name))`. Kind played no part, so two different kinds of
thing with the same name in one group were, by definition, the same entity, and
`knowledge_assert_entity`'s full-replace upsert made the collision destructive (#614: asserting
`Topic "adr"` overwrote the existing `KnowledgeChannel "adr"`'s labels, summary and attributes).

Since #221 retired the in-process `NameIndex`, `Entity.lookup_key` is the single point of truth
for identity, so changing what goes into the key changes identity everywhere at once.

## Decision

**D1 — a `kind STRING` column in the single `Entity` table.** `lookup_key = group_id ␟ kind ␟
lower(trim(name))`. `labels` stays the descriptive multi-set and `kind` is always one of them
(appended after `Entity`, `enforce_entity_first` retained). The default kind is exactly `Entity`.
Kind is compared verbatim after trimming (case-sensitive), must be non-empty and must not contain
U+001F (FR-012), and is immutable. Rejected: a node table per kind (dynamic DDL, per-table
vector/FTS indexes, fan-out for "all entities", no multi-label entities) and first-label-as-identity
(implicit; labels are replaced wholesale on assert).

**D2 — reads broad, writes scoped** (the rule #413 set for `group_ids`).
- *Writes* with no `kind` (`assert_entity`) act on kind `Entity` only, so a name-only write can
  never reach another kind's node.
- *Reads and resolution* with no `kind` span all kinds. `knowledge_resolve_entity`,
  `assert_relationship` endpoint resolution, merge's `canonical_name`, and cross-group pointer
  resolution return `Error::AmbiguousEntity` (JSON-RPC code `-32002`, `error.data.candidates =
  [{uuid, kind}]`) when more than one kind matches. Never a silent pick.
- `knowledge_find_entities` / `knowledge_list_entities` are multi-row reads: they gain an optional
  `kind` *filter* (omitted = all kinds) and return every candidate with its `kind`. They never
  pick one, so they never need to report ambiguity; the exact single-entity lookup (FR-005) is the
  dedicated `knowledge_resolve_entity` tool. This is the reading of User Story 2 AC 1 the plan
  adopted: the ambiguity *error* belongs where exactly one entity is required.
- `assert_relationship` never creates endpoints (a name that does not resolve is a hard error, as
  before), so D2's "endpoint being created" clause has nothing to act on today. There is no
  self-loop guard; `adr`(channel) → `adr`(topic) is two distinct uuids and a test pins that.
- Broad resolution costs one ART probe per **known kind**. `Db` keeps a superset-only registry of
  kinds (always containing `Entity`), seeded by `SELECT DISTINCT kind` at migrate/open, grown by
  `insert_entity`, and refreshed by the post-replay backfill funnel. An all-`Entity` database
  pays exactly the single probe it always did. On a total miss the scan fallback runs (and
  self-heals `kind` + `lookup_key`). An out-of-band write of a brand-new kind is invisible to
  broad resolution until the next refresh — the same accepted limitation as ADR-0221 FR-011.
  Alternatives rejected: an unindexed scan (regresses an indexed probe to O(N)) and a second
  `name_key` ART index (per-insert cost on the extraction hot path and a second derived value to
  strip/recompute). If databases with many kinds make N probes matter, a `name_key` index is the
  follow-up.

**D3 — `lookup_key` is derived: strip on write, recompute on replay.**
- `WalWriter::log_mutation` (the one choke point, next to `strip_vector_params`) strips
  `lookup_key` from params. `dump.rs` calls `log_mutation`, so it is covered too.
- Replay's new `inject_derived_lookup_key` runs for every row whose Cypher contains `$lookup_key`,
  computes the key from the record's `group_id`, `kind` (absent ⇒ `Entity`) and `name`, and
  overwrites any stored value. An old WAL's literal key is never trusted, so old corpora replay to
  today's identities with no version marker.
- Because `update_entity_core`'s record used to carry neither `group_id` nor `kind`, its template
  now also does `SET e.group_id = $group_id, e.kind = $kind` (idempotent — kind is immutable) so
  the record carries every input. An *old* update record still lacks them: injection binds NULL,
  and the post-replay backfill recomputes every NULL key, including on incremental/tail replay.
- `dump.rs`'s entity template gains `n.kind = $kind` so compaction / dump→replay does not flatten
  every entity to `Entity`; its keys still come from the backfill, as before.
- **Downgrade is unsupported**: a pre-#615 binary replaying a WAL written after this change finds
  `$lookup_key` unbound. Upgrade is safe in both directions of data (old WAL → new binary).
- `wal_strip` (#577) is deliberately not extended: old on-disk `lookup_key` bytes are inert.

**Migration and backfill.** `migrate()` adds the column (`ALTER TABLE Entity ADD kind STRING`) and
the `SchemaState` marker is renamed `entity_lookup_key_backfill` → `entity_kind_lookup_key_v2`, so
every existing database runs the backfill exactly once. The backfill widened from `lookup_key IS
NULL` to `kind IS NULL OR lookup_key IS NULL`: an upgraded database's rows all have a NULL kind and
an old-format (non-NULL) key, and so do rows replayed from an old WAL or the #217 corpus. It sets
`kind = 'Entity'` and recomputes the key. It stays one `SET` per row (the plan called for chunked
`UNWIND`, but replay itself no longer uses `UNWIND` — it prepares once and binds per row inside a
transaction — so the proven per-row shape was kept; a one-time O(N) cost).

**D4 — pointers carry an optional kind.** `CrossGroupPointer.endpoint_kind` (`serde(default,
skip_serializing_if)`, so stored pointers round-trip unchanged), `EndpointSpec::Foreign.kind`, and
top-level `source_kind` / `target_kind` on `knowledge_add_cross_group_edge` (foreign endpoints
only; supplying one for a `{uuid}` endpoint is a validation error, leaving `parse_endpoint_spec`'s
strict endpoint shape untouched). `resolve_endpoint_kind`: with a kind it resolves exactly (same-kind
duplicates are still `Ambiguous`, a missing kind is `Unbound`); without one it resolves across all
kinds and is `Ambiguous` once two kinds share the name — correct under D2, and the pointer has no
hop until re-created with a kind. Rebind uses the stored `endpoint_kind`.

**D5 — merges never cross kinds.** `follow_merged_into_chain` must never let a pointer asserting
kind `Topic` land on a `KnowledgeChannel`. `MergeEntitiesParams` gains an optional `kind`
(disambiguates `canonical_name` / `alias_names`). A resolved canonical+alias set spanning more than
one kind refuses the **whole call** — dry-run included — with an error naming the kinds, before any
mutation. `merge_all_by_name` is limited to the canonical's own kind.

**Extraction (FR-010) stays in the `Entity` namespace.** Phase B, Phase C endpoint authority and
the insert path all use kind `Entity`; the embedding-dedup candidate queries add `(kind IS NULL OR
kind = 'Entity')`. Extraction therefore never merges its summary into an asserted `Topic "adr"`;
it creates or merges its own `Entity "adr"`. On all-`Entity` data this is exactly today's
behaviour, and it keeps the indexed extraction hot path at one probe per name. Ontology-driven
kinds are the follow-up that lifts this.

> **Amended by [ADR-0616](0616-ontology-driven-kind-assignment.md) (#616).** Extraction now
> assigns a non-`Entity` kind to entities whose primary type the ontology marks `identity: true`,
> and Phase B / Phase C resolve within the entity's own kind. Extraction still never touches an
> *asserted* kind it does not create; a group with no identity flags behaves exactly as described
> here.

**Label rewriters preserve kind (FR-001).** `apply_entity_type_labels` and the restamp phase
derive labels from the ontology alone; both append the entity's kind when absent. The
`Merged`-only writers are untouched.

## Uniqueness

FR-002 said the ART index "MUST enforce uniqueness". It does not and cannot: ADR-0221's index
leaves are deliberately non-unique, and `Merged` tombstones deliberately share their canonical's
key, so the database cannot reject a duplicate. "Unique" here means **enforced by `write_lock` plus
kind-scoped resolution in both passes of `handle_assert_entity`** (#543's Pass-2 re-resolution is
the only guard against two concurrent same-key creates). The spec wording is corrected alongside
this ADR.

## Consequences

- Same-named entities of different kinds coexist; existing callers are unchanged (all existing
  data is kind `Entity`).
- `EntityRow` gains `kind` (additive on the wire); every entity reader returns it, and a NULL
  column reads as `Entity`.
- The tool registry gains `knowledge_resolve_entity` (45 tools; `read` 15).
- A kind-less pointer flips to `ambiguous` when a same-named second kind appears in its source
  group. Intended; kind-pinned pointers are the escape hatch.
- Extraction does not merge into asserted non-`Entity` entities. (ADR-0616 adds ontology-driven kinds, but extraction still only ever resolves within `Entity` and the group's own identity-bearing kinds.)
- First start after upgrade pays a one-time O(N) backfill.
- The Python-side `service_protocol.py` (the liminis app repo) needs a follow-up for the additive
  wire changes; nothing here breaks it.
