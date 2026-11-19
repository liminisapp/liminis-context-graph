# ADR-0797: Graph-Aware Search — One Read That Links, Walks, Believes and Fuses

**Status**: Accepted
**Date**: 2026-11-18
**Issue**: #797 (composes ADR-0629's evidence-carrying RRF, ADR-0705/0706/0707's belief time, ADR-0752's origin rule and ADR-0790's query role; measured with ADR-0754/0788's retrieval eval)

## Context

LCG holds an entity/relationship graph with belief time, HNSW indexes over passages, entity names,
entity summaries and facts, and full-text indexes. Every read used one of these on its own:
`knowledge_find_entities` (BM25 + name/summary vectors), `knowledge_find_relationships` (BM25 + fact
vector, belief opt-in), `knowledge_search_passages` (vector), `knowledge_get_entity_neighbors` (one
entity, truncated by uuid). On the v2 retrieval corpus the graph configuration lost to plain passage
search exactly where a graph should win — multi-hop (0.154 vs 0.655 nDCG@10) and paraphrase (0.009 vs
0.150) — because nothing ever *followed* a relationship, and nothing composed the reads. A client
would have to orchestrate five RPCs and three embeddings itself.

Constraints: no query-time LLM; one embed call (FR-006); p50 ≤ 100 ms at k = 10; no change to any
existing read, response, default, schema or WAL format (FR-002); deterministic (FR-005).

## Decision

### 1. A new opt-in read, `knowledge_graph_search`

The working name `knowledge_search` was too close to `knowledge_search_passages`; the chosen name says
what is different. It is a `read`-scope MCP tool, `ReadOnly` in the write policy, holds no lock (the
search-read convention), and uses the shared missing-index retry. Its params reuse the existing names:
`query` (required, non-empty), `num_results` (default 10, clamped 1–100, never padded), `group_ids`
(the `_preserve_empty` semantics of `find_entities`: omitted = all, `[]` = nothing), `origins`,
`believed_only`, `as_of`, plus `stages` (§7).

### 2. Compose `Conn` primitives; do not refactor `search.rs`

Every hybrid function in `search.rs` embeds internally, so composing them would cost three or more
embeddings. Rather than thread a precomputed embedding through the hot paths that existing reads
share (and argue byte-identical output), the new module `graph_search.rs` embeds once with
`EmbedRole::Query` and calls the `Conn`-level primitives those functions already use
(`fts_search_entities_kind`, `vector_search_entities_kind`, `vector_search_entities_by_summary_kind`,
`fts_search_edges`, `vector_search_episodic_origins`). FR-002 then holds by construction. The only
shared-code change is mechanical: the belief lookups of `handlers::apply_belief` moved into
`belief::load_context`, so both reads evaluate belief from the same code (same queries, same order).

Three additive `Conn` primitives were needed:

- `entities_by_lookup_keys` — one `lookup_key IN $keys` probe with **no scan fallback and no
  self-heal write**. `resolve_entities_by_name_any_kind` scans the group and writes on a miss, so it
  cannot probe dozens of candidate n-grams on a read path. (Whether lbug's ART index serves `IN` is
  unverified; at the probe sizes here — n-grams × kinds × groups, a few hundred keys — a scan of the
  `Entity` table is still well within budget. If it becomes a hotspot, a loop of indexed `=` probes
  is the drop-in alternative.)
- `expand_frontier` — every fact incident to a set of entities, both directions, as two batched
  queries per hop (O(hops), not O(seeds) like `get_entity_neighbors`), returning `episodes`,
  `warrant`, `fact_embedding`, and the far endpoint's name, group and tombstone status.
- `get_episode_passages` — passage fields plus origin, the full supersession/revision state,
  `ingested_at` and optionally `content_embedding`.

### 3. Seeds: four lists, RRF, then an evidence gate

Candidates (40 per list): **exact names** (every 1–4-word n-gram of the query, punctuation and a
trailing possessive stripped, at most 128, × requested groups — or the candidates' groups when none
are given — × known kinds, probed as `lookup_key`s; ranked longest name first), **BM25** over entity
names and summaries, the **name vector** and the **summary vector** (non-finite similarities —
zero-filled, un-backfilled summaries — dropped). They are fused with `rrf_fuse_scored`.

Because an ANN probe always fills its k (#613, #629), RRF alone would always produce seeds and
gibberish would seed the walk. A fused candidate is admitted only on real evidence:

- an exact-name hit; or
- a BM25 rank below 10; or
- a best vector similarity ≥ **0.5** that is within **0.08** of the best vector similarity among all
  candidates.

Merged tombstones are dropped; under `origins`, a seed needs a matching live mention
(`origin_reads::entity_support`). The top **5** by RRF are kept, scored relative to the best (1.0).
There is **no fuzzy name matcher**: near-miss names are covered by BM25 and the name vector. A
vector list is reported in a seed's `matched` only at or above the 0.5 floor, so filler is never
presented as evidence.

### 4. Expansion: batched hops, relevance from the fact text, hop decay, bounded fan-out

Up to **2** hops. Per hop: one frontier lookup (both directions); belief context for the new edges;
one episode lookup for their producing episodes (origin and liveness); under `origins`, one
entity-support lookup for the far endpoints. Expansion is a few indexed lookups, not one Cypher
pattern: belief is deliberately evaluated in Rust (ADR-0705), pruning between hops needs scores and
belief state, and a four-step variable-length pattern through the fact node is unverified on lbug.

- **Edge relevance** = `max(0, cos(query, fact_embedding))` + `0.15 × (1 − r/100)` when the edge is at
  rank r of one global fact BM25 list (limit 100). Cosine is computed in Rust from the fetched
  embedding (deterministic; `array_cosine_similarity` is unverified on lbug). Relation-type priors are
  rejected: v2 has 1,257 free-form relation types, and new stored data is out of scope.
- **Path score** = root seed score × **0.6**^(hop − 1) × relevance of the *last* edge. Multiplying
  relevances along the path was rejected: the bridging edge of a multi-hop question ("depends on") is
  usually weakly related to the question, and the beam already gates it.
- **Caps**: 20 edges per frontier entity per hop (by relevance), the 10 best edges of a hop choose the
  next frontier (newly reached entities only), at most 200 facts overall, 1,000 rows per direction
  per frontier query. Every cap that cut something is reported in `stats.truncations`.
- **No revisits**: an edge walked on an earlier hop is never walked again; reaching it again (or from
  both endpoints in one hop) increments `alternate_paths` and keeps the better path. Meeting it again
  from the entity it led to (A → B, then B → A) is the walk doubling back, not a second path, and is
  not counted. An entity keeps
  its earliest-hop best path.
- **Scope**: an edge is followed only if its own group and its far endpoint's group are requested, so
  traversal never leaves the requested groups through a cross-group edge. Tombstoned endpoints are
  not followed.

### 5. Belief: believed-only by default, one liveness definition

ADR-0705 kept belief opt-in on every existing read and anticipated a believed-by-default read as "a
later, documented behaviour change". This is that read; the default is specific to it and documented
on the tool. With neither `believed_only` nor `as_of`, the read filters at "now"; otherwise
`parse_belief_query`'s rules apply unchanged (`as_of` alone implies filtering; `believed_only: false`
annotates without filtering).

When filtering, an edge that is not believed at `as_of` is never a traversal step, so a path breaks
at a superseded intermediate edge. Legacy edges and warranted edges are believed and traversable.
Facts carry the full `BeliefAnnotation`.

Passages had no liveness filter anywhere. This read defines one, `episode_live_at(ep, t) =
EpisodeSupersession::live_at(t) && ingested_at ≤ t` (visible revision, not superseded at t, ingested
by t), and uses it for both supporting and plain passages. `EpisodeOriginInfo::is_live` — which
ignores pending document revisions — is not used for passages. Document supersession folds into
`superseded_at = max(recorded, effective)`, so a future-effective supersession stays believed with no
special case. Passages carry a `{state, believed, supersession?}` annotation in the edge vocabulary.

### 6. Fusion and the origin rule

RRF (k = 60) over three lists, each capped at `max(3k, 30)`, keyed `p:<episode>` / `f:<edge>`:

- **(a) `supporting`** — producing episodes of the selected facts (`rn.episodes` only, never ADR-0012
  `episode_uuids`) with the fact's path score, and MENTIONS episodes of the seeds and reached entities
  with the entity's score. Per episode the **maximum** source score is kept — never a sum, so a
  restatement adds no mass — then multiplied by the passage's own cosine to the query, so a hub
  entity's many mentions do not flood the list. At most 300 candidates are scored.
- **(b) `passages`** — plain vector passage search with the same embedding, over-drawn 3×, liveness
  applied. Vector-only until #789's hybrid passage list lands (`stats.passage_mode`).
- **(c) `facts`** — the selected facts by path score.

**Derived content cites but never corroborates (ADR-0752).** A `derived`/`answer` passage never
enters list (a); an edge whose surviving producers are all `derived`/`answer` is never ranked in list
(c) or used as a fact source for (a) — it may still be traversed. Such episodes are attached to the
same-fact sibling (ADR-0752's `(group, source, target, name)` class) as `supporting_episodes` with
`corroborating: false`. Under an `origins` filter derived content is excluded from every stage. With
no filter, a derived passage can still surface on its own merit through list (b). Returned facts
carry `support`/`corroboration` from `edge_support`, as `knowledge_find_relationships` does.

### 7. Evidence, stats, determinism and ablation

Every result carries `evidence`: the contributing lists with rank and raw score; for graph-reached
results the root seed (with match kinds and raw BM25/similarities), the ordered path in traversal
direction, supporting episodes (source and origin), `via` (fact or mention) and `warrant`. A result
from list (b) only has no seeds or path. `stats` reports the stage, passage mode, belief settings,
`embed_calls` (1), `db_queries`, candidate/seed/edge counts, hops, truncations and `no_seeds`.

Every sort breaks ties by key or uuid; hash-map iteration never reaches output order.

`stages` (`seeds` / `expand` / `full`, default `full`) switches stages off for ablation (FR-018):
`seeds` fuses list (a) restricted to seed mentions; `expand` fuses (a) and (c); `full` adds (b). It
is exposed on IPC and MCP alike, so the two transports stay aligned; the other constants stay
internal.

### 8. Eval

`lcg-eval retrieval` gains opt-in configurations, accepted by `--configs` but not in the default
set (the pattern #789 uses), so the default report, its count tests and every existing ranking are
unchanged: `lcg-graph`, `lcg-graph-expand`, `lcg-graph-seeds`, and `lcg-entities-facts-rrf`. Every
`lcg-*` id now routes explicitly; an unknown one is an error rather than silently becoming
`lcg-entities-facts-belief`. Results report service calls per query (every RPC, the main reads
included); `lcg-graph`'s model calls come from `stats.embed_calls`, not a constant.

Research found `lcg-entities-facts` unfair to the graph independently of this read (hub flooding in
note-id order, empty groups taking turns, `ceil(k/2)` per read with heavy overlap, strict
alternation). Changing it would break SC-008's "existing configurations rank identically", so the
fix is a new labelled variant, `lcg-entities-facts-rrf`: k per read, at most 2 notes per entity,
unmapped results skipped, facts believed-only, note lists fused with RRF. The report states the
difference.

## Consequences

- One read now does what needed five RPCs and three embeddings, with an evidence trail an agent can
  cite.
- The default differs from `knowledge_find_relationships` (believed-only); the tool description and
  the IPC reference say so.
- Cost is one embedding plus roughly 20–25 lbug lookups, all on one connection in one blocking task;
  the caps bound the worst case. Latency is measured by the eval, not asserted here. If p50 exceeds
  the budget, the first levers are the frontier row cap and the supporting-candidate cap.
- The success criteria (SC-001–SC-007) need a run on v2 with a real embedder; they are not provable
  in unit tests, which use hand-chosen vectors.
- When #789 merges, list (b) should switch to its hybrid passage list, and the private `fts_terms`
  sanitiser to `sanitize_fts_query`.
- #796's reranker can sit on top as a final stage.

## Rejected alternatives

- **`*_with_embedding` variants of the hybrid searches** — refactors a shared hot path to save three
  short calls; puts byte-identical output at risk.
- **One Cypher query for the walk** — cannot prune between hops or evaluate belief, and the
  variable-length pattern is unverified on lbug.
- **Personalised PageRank (HippoRAG)** — principled, but needs the whole neighbourhood loaded and many
  iterations; decay + beam gives the same "near and relevant first" ordering within the latency budget.
- **Changing `lcg-entities-facts` in place** — would move historical numbers and break SC-008.
