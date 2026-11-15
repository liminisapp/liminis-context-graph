# ADR-0755: `knowledge_lint` — A Deterministic, LLM-Free Notebook Health Report

**Status**: Accepted
**Date**: 2026-11-15
**Issue**: #755 (numbered by issue, per the ADR numbering rule)
**Builds on**: ADR-0667 (read-path paging cursor), ADR-0705 (belief time), ADR-0707 (document
supersession), ADR-0752 (episode origin and corroboration), ADR-0615 (kind-scoped identity),
ADR-0009 (degraded mode), ADR-0316 (one bench target per case).

## Context

"LLM wiki" systems (Karpathy's gist, claude-second-brain `/lint`, agent-second-brain, COG) all include
a health pass, and all implement it by having an LLM read the notes: slow, nondeterministic and
growing with the notebook. Liminis Notes is that pattern over LCG, and LCG already records the structure
those passes rediscover — belief time, document supersession, episode origin with per-document
`support` — so most notebook-health questions are graph queries. LCG holds no stored contradiction
signal (ingest never invalidates one fact because of another), no notion of a note "page", and no
document links, which shapes what can be deterministic.

## Decision

**1. One read-only method.** `knowledge_lint` (MCP `read` scope, `ReadOnly` in the write policy) returns
`{findings, count, next_cursor, summary, effective}`. It makes no LLM call and no embedding call, writes
nothing (graph, WAL, indices), and never queues or suggests a fix. Reporting a `near_duplicate_entity`
is not a merge instruction.

**2. Three phases.** *Snapshot*: chunked keyset scans (`uuid > $after ORDER BY uuid LIMIT n`) load only
the tables the selected categories need — episodes without content or embeddings, edges with endpoints
and their groups, entities without summary or embedding, and `MENTIONS`. Aggregation happens in Rust, as
in `origin_reads`; no aggregating Cypher is used, and the quadratic per-page `sibling_edge_rows`
cross-product is avoided. *Analyse*: each category is a pure function of the snapshot and an injected
`as_of_us` — no DB, no clock — returning findings sorted by id. *Page*: categories are concatenated in a
fixed order and sliced by a cursor `{c: category index, id: last id}`; only the returned page is hydrated
with entity names (and stored-embedding cosines). Rejected: calling `edge_support` per page (quadratic over
a whole group).

**3. Exact counts cost a full pass on every call.** The `summary` must be the same on every page and equal
the number of findings, so every call analyses the whole selected scope; the cursor only picks a slice.
The spec's "first page without a full pass" therefore cannot hold for the summary and is replaced by a
near-linear full-pass bound (SC-004), measured by `benches/lint.rs` at 1k/10k/50k episodes and reported,
not gated, because a timing gate on shared runners is flaky. A category that is not selected loads
nothing. The read lock is held only while the snapshot loads (a consistent view) and re-taken briefly to
hydrate, so writers never queue behind CPU-bound analysis; the page can reflect a slightly newer graph
than the summary. (ADR-0030's batched lock was the alternative; releasing after the load is simpler and
loses nothing the report promises.)

**4. Stable, content-derived ids.** `id = sha256("lint:v1" ␟ category ␟ group ␟ subject keys…)` over
stored uuids, `doc_key`s and designation strings only — never the run time, page position or row order —
so it survives unrelated ingests, WAL rebuild (uuids replay verbatim) and checkpoint restore, and a
resurfaced finding carries the same id. Subjects: the same-fact class `(src, dst, name)` for weak support;
the derived document key for a stale page (stable as more of its facts go stale); the entity uuid for an
orphan; the ordered target pair `(src, designation, dst_a, dst_b)` for a contradiction (a re-ingested
restatement keeps its id); the uuid pair for a near-duplicate. A weak finding's `reason` is not in its id,
so `derived_only` gaining a primary source remains the same finding as `single_source`. Dismissal state
lives in the app; LCG persists nothing.

**5. The cursor reuses ADR-0667.** `read_page::lint_shape_hash` binds groups, categories, the
near-duplicate threshold, `min_mentions` and the normalised single-valued set (the page `limit` is not
part of the shape). It is a separate function from `shape_hash`, so every live `episodes` / `entities`
cursor keeps decoding. Unlike the older reads, `next_cursor` is always present (`null` on the last page).

**6. Category definitions.**

- **Contradiction** is deliberately narrow: two currently-believed facts (ADR-0705) in one group with the
  same source and a relation designated single-valued, but different targets. The designation is a
  **request parameter only** (`single_valued_relations`, matched against a fact's `name` or
  `relation_type`, trimmed and case-insensitive); without one the category is `unavailable`. Putting it in
  the ontology (a `single_valued` flag on a relation type) is additive follow-up work but needs a
  conditional content-hash segment (ADR-0032) or every workspace shows spurious drift after upgrade. A
  side is all believed sibling edges to one target; one finding per unordered pair of sides.
- **Stale derived page**: a document whose episodes (live and superseded) are all `derived` and at least
  one is live, restating a same-fact class that had corroborating (primary/external) support, none of it
  live now. Cause from the superseded episode: document supersession, `revision` → `corrected`,
  `chunk_removed` → `removed`. A class that never had corroboration is `weak_support`, not stale. A
  hard-deleted corroborating episode has no readable origin and is ignored. LCG records no "derived from"
  link, so the relation is inferred through shared facts (ADR-0752's key). Detection is read-time, so a
  correction or supersession is reflected immediately with no re-ingest of the page.
- **Weak support**: a believed class with corroboration (distinct primary + external documents) 0
  (`derived_only`) or 1 (`single_source`). Classes containing a believed edge with no producing episode
  (asserted, cross-group warrant, legacy) are excluded — ADR-0752 treats them as `primary`, and a zero
  tally would misreport them.
- **Orphan entity**: no live mention (any origin — orphanhood is about footing in the graph, not
  corroboration) and no believed edge endpoint. `support_lost` cites the superseded episodes;
  `never_supported` had none.
- **Near-duplicate entity**: normalised-name (lowercase, punctuation stripped, tokens sorted) Levenshtein
  ratio at or above the threshold (default 0.9), same group and kind (ADR-0615). The stored
  `name_embedding` cosine is reported on the page but never gates membership: cosine of different names is
  noisy, and an embedding gate needs all-pairs or LSH. Candidates come from threshold-independent blocking
  (shared token of ≥ 4 characters, or shared 3-character prefix of the sorted-token key), so raising the
  threshold can only shrink the result. A block over 200 entities is skipped and counted
  (`skipped_blocks`), never silently; near-duplicates sharing no block are not found.
- **Contradiction bound**: findings are per unordered pair of targets, so k targets give k(k-1)/2 findings.
  A `(group, source, designation)` with more than 50 distinct believed targets is skipped and counted
  (`skipped_subjects`) rather than expanded; it almost always means a multi-valued relation was designated
  single-valued by mistake.
- **Unlinked mention / missing page**: reads a convention in episode `attributes` (ADR-0528) — `page_for`
  (entity uuid, or a unique case-insensitive name in the group) marks a page (its episode must carry a `doc_id`, else the declaration is ignored); `links: [doc_id…]` opts a
  document into link tracking. `unavailable` (`no_page_data` / `no_link_data`) until the scope carries both.
  No text matching at lint time. The convention is a contract to agree with the notes app; until the app
  writes it, the category reports `unavailable`.

**7. Origin discipline.** Derived and answer content never corroborates in any category (FR-016). Origin
remains the caller's unauthenticated claim; the reference docs say so.

## Consequences

- The app gets a pollable, citable, trackable health report with no model cost, scaling near-linearly.
- A call is a full pass over the selected categories even for page two; callers wanting only one
  category should select it.
- Findings depend on the evaluated instant (a future-dated document supersession takes effect when its
  time arrives), so determinism holds for an unchanged graph at the same instant. Tests inject `as_of_us`.
- The contradiction category is useful only once the app tells LCG which relations are single-valued;
  broader semantic contradictions remain out of reach without an LLM.
- Out of scope: fix actions, persisted findings/dismissals, a single health score, authoring pages or links.
