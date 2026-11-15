# ADR-0752: Episode Origin — Derived Content Cites But Never Corroborates

**Status**: Accepted
**Date**: 2026-11-15
**Issue**: #752 (numbered by issue, per the ADR numbering rule)
**Builds on**: ADR-0705 (belief time), ADR-0707 (document supersession), ADR-0528 (episode
attributes), ADR-0012 (`episode_uuids` inference). **Consumed by**: #732 (observations), #733
(document-aware extraction).

## Context

LCG's main consumer follows the "LLM wiki" pattern: a human writes notes, and an agent writes summary
and synthesis pages back into the same notebook, which LCG indexes like any note. A fact the agent
*derived* from note A returns from the agent's page as a second, apparently independent episode.
Support overstates the evidence, search returns the agent's own earlier pages beside the primary notes
so an error in a page is re-read as evidence, and no read can ask "what do the primary sources say?".
`attributes.kind = "knowledge"` already half-tags derived content, but a free-form map is the wrong
carrier for something LCG must reason about. #732 needs the same guard: duplicate evidence must not
masquerade as independent corroboration.

## Decision

**1. Origin is a per-episode, validated, caller-declared label.** One nullable `Episodic.origin`
column holding `primary`, `external`, `derived` or `answer`. NULL or empty reads as `primary`, so
graphs and WALs written before this feature need no backfill and — unlike `attributes` (ADR-0528
Decision 3) — no post-replay zero-fill. New ingests write the resolved value explicitly. The column is
a logged parameter (it is *not* in `WalWriter`'s derived-param strip list), a dump column and a page
column, in lockstep (ADR-0028). `primary` and `external` are *corroborating*; `derived` and `answer`
are not. Origin is a claim by the caller and is never authenticated.

**2. Origin lives on episodes and is derived upward** (FR-016). It is never stored on entities or
relationships, so it cannot drift. A relationship's origins come from `RelatesToNode_.episodes` (never
the `episode_uuids` inference of ADR-0012), an entity's from `MENTIONS`.

**3. Per-episode column, per-document consistency.** The natural owner of origin is the document, but a
per-document record would need new schema, dump, replay and group-purge handling (ADR-0707 shows the
cost). Instead every episode carries origin and, when a `doc_id` is supplied, ingest refuses a
submission whose origin differs from the one already recorded for that `(group, doc_id)`. The check
runs only when a `doc_id` is present, so plain ingest pays no extra scan.

**4. Support counts distinct documents, over same-fact siblings.** Ingest never deduplicates edges
(ADR-0705), so a derived page restating a fact produces a *second edge*. Reversing that decision
(appending to `episodes[]`) would change `edges_inserted`, WAL volume and `valid_at` handling. Instead,
`support` for an edge is aggregated at read time over edges sharing `(group, source entity, target
entity, name)`; counts are per distinct document (`doc_id`, else the episode), over episodes live now.
The key is deliberately coarse: it can only move a count between origins for facts a reader would
already call the same, and never creates or merges anything. The `origins` filter looks at the edge's
**own** episodes, so a derived sibling is excluded under `[primary, external]`.

**5. Guards.** A `derived` or `answer` submission:

- cannot rewrite, or merge by embedding or LLM verdict into, an entity with a live corroborating
  mention (Phase B). Identity-level matches (exact or identical-normalized name) still link the
  episode, with no summary or embedding change; blocking them would fork "Postgres" and break
  provenance. Protection is by corroborating *mention*, so an entity with no mentions
  (`knowledge_assert_entity`) is unprotected — a known gap that follows the spec literally.
- cannot end belief in a corroborating episode. A `knowledge_process_chunk` whose `chunk_id` has a live
  `primary` or `external` episode is refused **regardless of `supersede`** (strict, so a multi-unit
  revision never half-writes); `knowledge_mark_chunk_removed` takes an optional `origin`; and
  `knowledge_supersede_document` refuses derived-supersedes-primary. Hard deletes keep ADR-0705's
  explicit-deletion contract.
- Refusals are `Error::Ipc` with an `origin_conflict:` message prefix. They are decided before the
  first mutation (lbug auto-commits per statement, ADR-0705) and re-checked under the write lock, so a
  refusal writes nothing and logs nothing.

The protection threshold is "primary **or** external", wider than the issue's "primary-only": clipped
third-party material is still a source, and protecting only primary would let a derived page overwrite
it. FR-009 (invalidation) has no ingest path to guard: ingest never invalidates another fact for any
origin, so SC-003 holds by construction and is pinned by tests. Any future fact-contradiction feature
(#732) must consult origin.

**6. Replay does not re-evaluate guards.** WAL replay executes recorded Cypher verbatim, so every guard
decision is baked into the logged statements (a blocked merge is logged as an entity `CREATE`).
Rebuild therefore cannot re-open a protected fact, and support breakdowns, being read-time, come out
identical. The Phase B guard is best-effort against a concurrent primary ingest — the same accepted
TOCTOU as ADR-0029 dedup; the hard refusals are re-verified under the write lock.

**7. Reads.** `support` (`{primary, external, derived, answer}`, all four keys always) and
`corroboration` (`primary + external`) are response-side annotations on `knowledge_find_relationships`
and `knowledge_find_entities`, not fields on `EntityRow` / `RelatesToEdge`, so other reads and the
WAL/dump paths are untouched and a no-origin client sees only additive keys. `origins` on those two
reads and on `knowledge_search_passages` is validated (non-empty, known values). It is a post-filter
over retrieval pools that know nothing about origin, so relationships and entities over-fetch ×3 (as
belief does) and passages oversample the ANN probe ×10 (as the `kind` filter does); a selective filter
can return fewer than `num_results`. Edges with no producing episode (asserted, cross-group, legacy)
and mention-less entities count as `primary`. Counts reflect live episodes at read time.

**8. Stamping.** `knowledge_stamp_origin` (admin scope, write-policy class `GroupRequired`) sets origin
on existing episodes by ordered rules `{origin, source_path_prefix?, attributes_kind?}` (selectors
ANDed, first match wins). The path prefix matches `source_description` (the only place the source file
survives), following `remove_episodes_by_source`; `attributes_kind` is matched Rust-side after parsing
the stored JSON. A selected episode with a `doc_id` expands to its whole document, and rules that would
split a document are refused. The write is a uuid-keyed, fixed-valued `SET`, so replay — including
over an overlapping tail — is idempotent, and it never revisits past supersessions. Existing
`attributes.kind = "knowledge"` episodes are not reinterpreted automatically.

## Consequences

- Facts a derived page restates are visible, cited and filterable, but cannot inflate corroboration,
  override primary content, or make an agent re-read its own output when it filters.
- `support` without a `doc_id` can still over-count a chunk-split note; callers who want the guarantee
  supply a `doc_id`.
- Origin is only as good as the caller's declaration. The companion app issue
  (shadoworg/liminis-notes#1327) stamps it on what the app sends.
- One new `Episodic` column and one admin method; every other change is additive. The MCP registry
  grows to 51 tools (admin 16).

## Alternatives considered

- **Ingest-time edge dedup** (append the derived episode to the primary edge's `episodes[]`): matches
  the issue's wording but reverses ADR-0705, changes ingest counts and WAL volume, and needs its own
  regression suite. Rejected for read-time sibling aggregation.
- **A per-document origin record** beside `DocSupersession`: the "right" owner, at the cost of schema,
  dump, replay and purge work. Rejected for the per-episode column plus a consistency check.
- **Storing origin on entities or relationships:** can drift from the episodes it summarises. Rejected.
- **Reinterpreting `attributes.kind`** automatically: would change behaviour silently. Rejected in
  favour of an explicit stamp.
