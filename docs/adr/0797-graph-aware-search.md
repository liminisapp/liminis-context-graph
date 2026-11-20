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

The maintainer later **dropped the 100 ms target** (steer of 2026-11-19): these are deliberate agent
searches, not thousands a second. The budget is now **p50 ≤ 500 ms including the rerank stage**
(§6, #796), and the read keeps reporting per-stage timings in `stats.timings_ms`.

## Decision

### 1. A new opt-in read, `knowledge_graph_search`

The working name `knowledge_search` was too close to `knowledge_search_passages`; the chosen name says
what is different. It is a `read`-scope MCP tool, `ReadOnly` in the write policy, holds no lock (the
search-read convention), and uses the shared missing-index retry. Its params reuse the existing names:
`query` (required, non-empty), `num_results` (default 10; 0 or a non-integer is rejected, values above 100 are capped at 100; never padded), `group_ids`
(the `_preserve_empty` semantics of `find_entities`: omitted = all, `[]` = nothing), `origins`,
`believed_only`, `as_of`, plus `stages` (§7).

### 2. Compose `Conn` primitives; do not refactor `search.rs`

Every hybrid function in `search.rs` embeds internally, so composing them would cost three or more
embeddings. Rather than thread a precomputed embedding through the hot paths that existing reads
share (and argue byte-identical output), the new module `graph_search.rs` embeds once with
`EmbedRole::Query` and calls the `Conn`-level primitives those functions already use
(`fts_search_entities_kind`, `vector_search_entities_kind`, `vector_search_entities_by_summary_kind`,
`fts_search_edges`, and the passage ANN query). FR-002 then holds by construction. The shared-code
changes are mechanical:
- the belief lookups of `handlers::apply_belief` moved into `belief::load_context`, so both reads
  evaluate belief from the same code (same queries, same order);
- `vector_search_episodic_origins` now delegates to `vector_search_episodic_probed`, with its probe
  size computed exactly as before.

Seven additive `Conn` primitives were needed:

- `entities_by_lookup_keys` — one `lookup_key IN $keys` probe with **no scan fallback and no
  self-heal write**. `resolve_entities_by_name_any_kind` scans the group and writes on a miss, so it
  cannot probe dozens of candidate n-grams on a read path. (Whether lbug's ART index serves `IN` is
  unverified. The key list is n-grams × `(group, kind)` pairs, hard-capped at 2,048 keys
  (`MAX_EXACT_KEYS`); at that size even a scan of the `Entity` table stays bounded. If it becomes a
  hotspot, a loop of indexed `=` probes is the drop-in alternative.)
- `entity_group_kinds` — the distinct `(group_id, kind)` pairs present among entities, one scan,
  used only when the caller gives no `group_ids`.
- `expand_frontier` — every fact incident to a set of entities, both directions, as two batched
  queries per hop (O(hops), not O(seeds) like `get_entity_neighbors`), returning `episodes`,
  `warrant`, and the far endpoint's name, group and tombstone status. Its scored form
  (`expand_frontier_scored`) ships no embedding: one further query per hop, `fact_similarities`,
  computes every fact's cosine in lbug (§4).
- `get_episode_passages` — passage fields plus origin, the full supersession/revision state,
  `ingested_at` and optionally `content_embedding`. Its scored form (`get_episode_passages_scored`)
  returns the passage's cosine to the query instead of the embedding.
- `vector_search_episodic_probed` — the passage ANN query with an explicit probe size, so list (b)
  can oversample under `group_ids` (§6).
- `entity_mention_counts` and `entity_mentions_ranked` — the per-entity mention bound (§6): a count
  per entity, and one hub's most query-relevant qualifying mentions, capped, with the filters in the
  query.

### 3. Seeds: four lists, RRF, then an evidence gate

Candidates (40 per list) come from four lists: **exact names**, **BM25** over entity names and
summaries, the **name vector** and the **summary vector** (non-finite similarities — zero-filled,
un-backfilled summaries — dropped). They are fused with `rrf_fuse_scored`.

Exact names are every 1–4-word n-gram of the query (punctuation and a trailing possessive stripped,
at most 128) × `(group, kind)` pairs, probed as `lookup_key`s and ranked longest name first. With
`group_ids` the pairs are those groups × every registered kind, at no query cost. Without, they are
the pairs actually present among entities (one `DISTINCT` scan of `Entity`): every group, not just
the groups the capped lists happened to fill from, and no key for a kind a group lacks. The key list
is capped at 2,048. A cut drops the longest n-grams across all pairs alike, never whole groups, and
is reported as an `exact_name_keys` truncation. Callers that need the latency budget should pass
`group_ids`; omitting them costs the extra scan. **Known trade-off:** because short n-grams go
first, a store with very many `(group, kind)` pairs probes generic single words before long
multi-word names, and one with more than 2,048 pairs probes none (exact seeding is skipped, and
only the truncation says so). Name BM25 and the name vector still link such queries. Ordering keys
by n-gram length the other way, or scoping the probe to the candidate lists' groups first, is a
candidate follow-up once a large multi-group store is measured.

Because an ANN probe always fills its k (#613, #629), RRF alone would always produce seeds and
gibberish would seed the walk. A fused candidate is admitted only on real evidence:

- an exact-name hit; or
- a BM25 rank below 10; or
- a best vector similarity ≥ **0.5** that is within **0.08** of the best vector similarity among all
  candidates.

Merged tombstones are dropped; under `origins`, a seed needs a matching live mention
(`origin_reads::entity_support`). The same check gates a far endpoint during expansion. Unlike the
per-edge and passage checks, this one always requires the mention to be live at `as_of`, even with
`believed_only: false`: an entity whose only matching mentions have been superseded is neither
seeded nor walked into. The check runs whenever `origins` is set; it is only *stricter than the
rest of the read* when belief filtering is also off, since under the default every check already
requires liveness at `as_of`. It is the conservative side. The top **5** by RRF are kept, scored relative to the best (1.0);
up to 3 summary-vector seeds may follow (below), so a response carries **at most 8** seeds.
There is **no fuzzy name matcher**: near-miss names are covered by BM25 and the name vector. A
vector list is reported in a seed's `matched` only at or above the 0.5 floor, so filler is never
presented as evidence. The one exception is a summary-vector seed (below): it was admitted on the
summary list's relative evidence, so it always names `summary_vector`, whatever its absolute
similarity (which is reported alongside).

**Summary-vector seeds, on relative evidence (#797 steer, 2026-11-19).** The first v2 run scored
`lcg-graph-seeds` at 0.000 on paraphrase. A seed trace (`explain: true`, below) over the 40
paraphrase questions showed why. The entity a question describes was a candidate in 34 of them,
usually near the top of the summary-vector list (rank ≤ 5 in 14, ≤ 10 in 21). In 3 it was seeded
but its note still ranked low (list (a) scoring, §6). In the other 31 it was rejected:
- **By the cap** in 19 questions. An entity matched only by its summary sits in one list, so its
  fused score is low (≈ 1/62). Generic concept entities ("induction sessions", "billing") that also
  match BM25 on common query words filled all five slots.
- **By the vector margin** in 12. That margin is measured against the best similarity of *either*
  vector list, and concept entities' *name* vectors set it.

So up to **3** more seeds are taken in summary-list order, beyond the 5, when the candidate's
summary similarity is both within **0.03** of the best summary similarity *and* at least 0.03 above
that list's median. Both tests are relative to the list, never an absolute cosine floor, whose right
value depends on the embedder. The median test is what keeps ANN filler out (#613): a query that
matches no summary well gives a flat list, whose best barely clears the median, so nothing
qualifies. Such a seed reports `summary_vector` in `matched` and in the trace. A candidate cut by
the RRF cap can return this way: the cap judges fused rank, this pass judges the summary list alone.

**These constants were tuned in-sample.** `SUMMARY_SEEDS` (tried 0, 2, 3), `SUMMARY_SEED_MARGIN`
(0.03, not varied) and `SUPPORT_SOURCE_EXP` (§6; tried 1, 0.5, 0.25, 0) were chosen on the **whole**
v2 question set, the same set the success criteria are judged on. There was no held-out split.
That was eight runs of a small grid, on a local harness that computes nDCG@10 as `lcg-eval` does,
without the judge. So v2 gains on paraphrase and multi-hop from these two changes are partly
in-sample. They are evidence the mechanism works, not an independent acceptance test. A held-out
check needs either a v2 split fixed before tuning, or a fresh corpus.

**The holdout rule from here on** (steer of 2026-11-19): any *new* constant — the rerank pool size,
blend weights, list weights — is tuned on v2 only, and **v1 is the holdout**: v1 is reported for the
final settings and never tuned on. The broad-collection change (§6(b)) adds no tuned constant: its
new lists enter the fusion unweighted, at the same `max(3k, 30)` cap as every other list.

**`explain: true`** adds `seed_trace` to the response: every fused candidate with its rank in each
list, its similarities, whether it was seeded, and why (`exact_name`, `bm25_rank`, `vector`,
`summary_vector`; or `no_evidence`, `merged`, `no_row`, `origin_filter`, `seed_cap`). It is a
diagnostic and changes nothing else. The eval sets it, and appends each response to a JSONL file,
when `LCG_EVAL_GRAPH_TRACE` names one.

**Known trade-off, open until v2 is measured: generic one-word names.** Every query word is probed
as an exact name, and an exact hit is admitted unconditionally, so an entity literally named
`Status`, `Price` or `Notes` is seeded by "what is the price status". Multi-word exact hits rank
above it in the exact list, but with no multi-word hit it takes that list's first place. Requiring a
one-word hit to also pass the BM25 gate would not help, since BM25 ranks such an entity highly for
the same word. Telling a generic name from a real one needs a specificity signal, such as damping
entities by mention count (HippoRAG's node specificity). That is a tuning change to judge on the v2
`direct` and `paraphrase` rows, not to guess at here, so the gate is left as it is.

### 4. Expansion: batched hops, relevance from the fact text, hop decay, bounded fan-out

Up to **2** hops. Per hop: one frontier lookup (both directions); belief context for the new edges;
one episode lookup for their producing episodes (origin and liveness); under `origins`, one
entity-support lookup for the far endpoints. Expansion is a few indexed lookups, not one Cypher
pattern: belief is deliberately evaluated in Rust (ADR-0705), pruning between hops needs scores and
belief state, and a four-step variable-length pattern through the fact node is unverified on lbug.

- **Edge relevance** = `max(0, cos(query, fact_embedding))` + `0.15 × (1 − r/100)` when the edge is at
  rank r of one global fact BM25 list (limit 100). The cosine is computed in lbug
  (`array_cosine_similarity`, the query vector bound and cast once per hop), not from shipped
  embeddings. Profiling on v2 showed the cost of each frontier query was binding the 768-float
  vector, not the rows. So the two frontier queries carry no vector, and one scoring query per hop
  scores them all. That cut expansion from 65 ms to 39 ms at p50 with identical rankings. lbug fails
  the whole query, rather than yielding `NULL`, on a vector of the wrong length, so on any error the
  primitive falls back to fetching the embeddings and scoring in Rust: same rows, same scores, only
  slower. Relation-type priors are rejected: v2 has 1,257 free-form relation types, and new stored
  data is out of scope.
- **Path score** = root seed score × **0.6**^(hop − 1) × relevance of the *last* edge. Multiplying
  relevances along the path was rejected: the bridging edge of a multi-hop question ("depends on") is
  usually weakly related to the question, and the beam already gates it.
- **Caps**: 20 edges per frontier entity per hop (by relevance), the next frontier is the 10 best
  newly reached far endpoints of the hop (an edge back to a reached entity takes no slot), at most 200 facts overall, 1,000 rows per direction
  per frontier query. That row cap is cut by uuid across the whole batched frontier, so when it is
  hit each frontier entity is refetched on its own with an equal share of the same budget
  (reported as `frontier_rows`, plus `entity_rows` for any entity that still overflows its share):
  a hub cannot starve the other frontier entities. Within one entity the cut stays by uuid, since
  the cut happens before relevance is scored (one scoring query over the surviving rows); the
  20-per-entity fan-out ranks by relevance next. The 200-fact trim keeps whole paths: a fact is kept, best score first, only
  together with every edge on its path, and an entity reached only through a trimmed edge is no
  longer reached. Cutting by score alone would orphan hop-2 facts, since hop decay can score their
  weakly worded bridge below them, and leave evidence naming an edge the response omits. Citation-only edges (every producer derived / answer) never take one of these slots. They have their own budget of 20 per entity, reported as `per_entity_citations`, which they share with facts not believed at `as_of` (returned only under `believed_only: false`, and never walked), so neither kind can starve a hub's walkable believed edges. They do not count toward the 200-fact cap, and they are kept only while the walk edges before them survive. They can neither rank nor carry the walk, so a hub full of on-topic agent restatements cannot crowd its primary edges out. Every cap that cut something is reported in `stats.truncations`; a direction holding exactly the row cap is not a cut (one row past the cap is fetched to tell). A self-loop fact is returned once per hop, not once per direction.
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

An edge that is not believed at `as_of` is never a traversal step, so a path breaks at a superseded
intermediate edge. That holds with `believed_only: false` too: the edge may then be **returned**,
with its belief state, but the walk does not continue through it, so nothing is reached or ranked
on the strength of a fact no longer believed (FR-013). Legacy edges and warranted edges are believed and traversable.
Facts carry the full `BeliefAnnotation`.

Passages had no liveness filter anywhere. This read defines one, `episode_live_at(ep, t) =
EpisodeSupersession::live_at(t) && ingested_at ≤ t` (visible revision, not superseded at t, ingested
by t), and uses it for both supporting and plain passages. `EpisodeOriginInfo::is_live` — which
ignores pending document revisions — is not used for passages. Document supersession folds into
`superseded_at = max(recorded, effective)`, so a future-effective supersession stays believed with no
special case. Passages carry a `{state, believed, supersession?}` annotation. Its state is one of
`believed`, `superseded` and `not_yet_ingested` (ingested after `as_of`), all in the edge
vocabulary, or the passage-only `pending_revision`. That last one means the passage was ingested by
`as_of`, but its document revision had not committed by then, so it is not yet evidence. An edge has
no revision of its own and keeps reporting that case as `not_yet_ingested` (#785). A passage can say
which case it is, so it does.

### 6. Fusion and the origin rule

RRF (k = 60) over up to five lists — (a), (b), (b′), (b″) and (c) below; `seeds` and `expand` use
fewer (§7) — each capped at `max(3k, 30)`, keyed `p:<episode>` / `f:<edge>`:

- **(a) `supporting`** — producing episodes of the selected facts (`rn.episodes` only, never ADR-0012
  `episode_uuids`) with the fact's path score, and MENTIONS episodes of the seeds and reached entities
  with the entity's score. Per episode the **maximum** source score is kept — never a sum, so a
  restatement adds no mass. A passage scores `source^0.25 × cosine(passage, query)`, so a hub
  entity's many mentions do not flood the list. The exponent was **1** until the #797 steer: the v2
  trace showed a summary-matched seed (source ≈ 0.33) whose own note had the highest cosine of all
  candidates, yet ranked 6th–15th behind a generic top seed's digest mentions. At 0.25 the
  passage's own relevance decides and the path only tilts it. The cosine is computed in lbug
  (`get_episode_passages_scored`). At most 300 candidates are scored. Episodes
  that can never enter the list — not live, outside `origins`, or derived / answer — are dropped
  **before** that cap, so they cannot use up slots that live primary passages would fill.
  Fact-produced episodes are checked exactly from the rows expansion already fetched. Mention
  episodes are checked on origin and supersession, which the mention lookup returns. A pending
  revision or a not-yet-ingested mention is only caught after the cap, and that is rare.

  **The mention lookup is bounded per entity.** One count query splits the reached entities.
  - **Entities with at most 50 mentions** are fetched in one batch.
  - **Hubs, mentioned more often,** get one query each that returns their **50 most query-relevant
    mentions** (`array_cosine_similarity` of `content_embedding` against the query, uuid
    tie-break). The query also filters, so only corroborating, in-`origins` mentions not superseded
    at `as_of` qualify.
  - **At most 10 hubs** are fetched, best score first.

  So no request loads every mention of a hub. A hub's superseded or derived mentions take none of
  its 50 slots. And the relevance that later scores a mention also decides which of a hub's mentions
  compete, which is what paraphrase questions need: the one relevant note among many mentions
  survives. Both cuts are reported in `stats.truncations` (`mentions_per_entity`, `mention_hubs`).
  The ranked query's supersession bound is rounded down to the millisecond, so it
  can only keep an episode the exact liveness check later drops.
- **(b) `passages`** — plain vector passage search with the same embedding, over-drawn 3×, then
  #789's **belief-aware ranking** evaluated at `as_of` (`knowledge_search_passages` with
  `mode: "vector", belief_aware: true`, the best passage list measured on v2; `stats.passage_mode`
  is `vector_belief`): a derived page ranks below the primary note it cites, a superseded passage
  below its successor. It runs over the whole over-drawn set before the liveness filter and the
  cut, so a demoted passage is replaced rather than reshuffled. #789's hybrid mode stays unused here:
  it measured below vector on v2 (ADR-0789). The group
  and origin filters run after the ANN probe. So whenever `group_ids` or `origins` is set, the
  probe is oversampled 10×: otherwise, in a multi-group database, other groups' passages crowd the
  requested group's out of the nearest neighbours and the list comes back short or empty. The
  ranking is the same as `knowledge_search_passages`', over a larger candidate pool. That read itself is
  unchanged (FR-002): it still oversamples only under `origins`. Its own minority-group recall is a
  candidate follow-up, through the new `vector_search_episodic_probed` primitive this read uses.
- **(b′) `passages_raw`** — the same vector list for the **bare** query, run only when the
  embedding profile has a query instruction (steer of 2026-11-19, "collect broadly"). On v2 the
  BGE instruction doubles vector paraphrase (0.150 → 0.281) but hurts named-entity passage lookups
  (0.477 → 0.343), so the read keeps both instead of choosing. Both vectors come from **one** model
  call: `embed_query` sends `[query, prefix + query]` through the new
  `Embedder::embed_batch_plain`, which no layer of the stack instruction-prefixes (the
  `InstructedEmbedder` decorator skips itself; `OaiEmbedder` in `embedder` mode sends no
  `input_type`). The instructed text is exactly what `embed(query, Query)` sends, so the instructed
  vector — and every graph stage built on it — is byte-identical to before. If the two vectors
  come back identical, the bare list is dropped rather than double-counted. Under profile `none`
  there is one vector and one `embed`, as before.
- **(b″) `passages_lexical`** — BM25 over passage content, for names: #789's lexical leg
  (`search::lexical_passage_list`, with `sanitize_fts_query`, the group and origin filters in the
  query, and an empty list when the FTS index is unavailable), followed by the same belief-aware
  ranking at `as_of` and the same liveness rule as the vector lists. Each passage still reports its
  cosine against the instructed query as its similarity.

  `stats.passage_lists` names the plain lists that ran, in fusion order, and `stats.query_vectors`
  is 2 when the bare vector was computed. `embed_calls` stays 1.
- **(c) `facts`** — the selected facts by path score.

A returned passage's `score` is always its cosine against the instructed query, whichever list
found it (each list still ranks by its own score, reported in `evidence.lists`). There is no
belief step after fusion: belief-aware ranking runs inside each plain list, and under the
believed-only default a superseded passage is in no list at all. With `believed_only: false` the
fused order across lists is not belief-adjusted; the rerank stage (Consequences) must re-apply belief
ordering after it reorders.

**Derived content cites but never corroborates (ADR-0752).** A `derived`/`answer` passage never
enters list (a); an edge whose surviving producers are all `derived`/`answer` is never ranked in list
(c) or used as a fact source for (a), and it carries the walk nowhere: it neither reaches its far
endpoint nor enters the beam. Were it traversed, an agent-written page asserting "A relates to B"
would pull B's primary notes into list (a) through B's mentions, ranking them on derived evidence
alone. An entity a primary edge also reaches is still reached that way. Such episodes are attached to the
same-fact sibling (ADR-0752's `(group, source, target, name)` class) as `supporting_episodes` with
`corroborating: false`. Under an `origins` filter derived content is excluded from every stage. With
no filter, a derived passage can still surface on its own merit through list (b). Returned facts
carry `support`/`corroboration` from `edge_support`, as `knowledge_find_relationships` does.

### 7. Evidence, stats, determinism and ablation

Every result carries `evidence`: the contributing lists with rank and raw score; for graph-reached
results the root seed (with match kinds and raw BM25/similarities), the ordered path in traversal
direction, supporting episodes (source and origin), `via` (fact or mention) and `warrant`. A result
from list (b) only has no seeds or path. `stats` reports the stage, passage mode, belief settings,
`passage_lists`, `query_vectors`, `embed_calls` (1), `db_queries`, candidate/seed/edge counts, hops,
truncations, `no_seeds`, and
`timings_ms` — wall-clock milliseconds for `embed`, `seeds`, `expand`, `supporting`, `passages`,
`fusion` and `total`. Timings are reported, never ranked on, so they are the one part of the
response that differs between two identical requests.

Every sort breaks ties by key or uuid; hash-map iteration never reaches output order.

`stages` (`seeds` / `expand` / `full`, default `full`) switches stages off for ablation (FR-018):
`seeds` fuses list (a) restricted to seed mentions; `expand` fuses (a) and (c); `full` adds the plain
lists (b), (b′) and (b″). It
is exposed on IPC and MCP alike, so the two transports stay aligned; the other constants stay
internal.

### 8. Eval

`lcg-eval retrieval` gains opt-in configurations, accepted by `--configs` but not in the default
set (the pattern #789 uses), so the default configuration set, its count tests and every existing
ranking are unchanged: `lcg-graph`, `lcg-graph-expand`, `lcg-graph-seeds`, and
`lcg-entities-facts-rrf`. Every
`lcg-*` id now routes explicitly; an unknown one is an error rather than silently becoming
`lcg-entities-facts-belief`. Results report service calls per query (every RPC, the main reads
included); `lcg-graph`'s model calls come from `stats.embed_calls`, not a constant.

The report gains two sections; every existing section and ranking stays as it was:
- **Headroom: per-question oracle** (#797 steer). For each question it takes the best nDCG@k any
  configuration in the run reached. It shows the oracle's mean overall and per category, each
  configuration's gap to it and the share it captures, and the best configuration per category.
  It appears in **any** run with two or more configurations, the default run included, because
  the headroom between retrievers is the point of the measurement, not a graph-only figure.
- **Graph-aware search**, only when a graph configuration or `lcg-entities-facts-rrf` ran.

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
- Cost is one embedding plus roughly 25–30 lbug lookups, all on one connection in one blocking task;
  the caps bound the worst case. Measured on the v2 graph (Apple silicon, the BGE sidecar shared
  with another run), `full` p50 went from 143 ms to 103 ms with the in-lbug scoring. That is just
  over the 100 ms target: embed 22, seeds 10, expand 39, supporting 23, passages 7. Where the time
  remains:
  - every query that binds the query vector costs several milliseconds;
  - the hub mention queries bind it once each.

  The beam size barely matters (5 vs 10: 109 vs 115 ms before the scoring change). Batching the hub
  mention queries into one is the next lever, at the cost of loading all of a hub's mention rows.

  The target is now p50 ≤ 500 ms including rerank (Context). The broad-collection lists (§6(b′),
  (b″)) add a second vector probe, one FTS query and their row fetches, a few milliseconds each,
  and no model call: the bare vector rides in the same embed batch. That cost is not yet measured
  on v2.
- The success criteria (SC-001–SC-007) need a run on v2 with a real embedder; they are not provable
  in unit tests, which use hand-chosen vectors.
- #789 has merged with hybrid passage search opt-in, because it scored below vector on v2
  (ADR-0789). List (b) therefore keeps vector retrieval, with #789's belief-aware ranking layered on
  top (`belief_aware: true`, evaluated at `as_of`; §6). Since #806 (ADR-0806) that *is*
  `knowledge_search_passages`' shipped default, which the `lcg-passages` eval row measures. So
  `lcg-graph`'s vector list `passages` matches what `lcg-passages` measures, but since the
  broad-collection change `full` also fuses the bare-query and BM25 lists, so `lcg-graph` against
  `lcg-passages` measures the graph stages *and* the broader passage collection together;
  `lcg-passages-raw` (cosine only) shows what the belief rules add on their own. List (b) still
  sends `as_of`, which the default passage read does not take. BM25 enters as its own RRF list
  rather than through #789's weighted-RRF hybrid list. Moving the seed and edge FTS legs' private
  `fts_terms` sanitiser to `sanitize_fts_query` is a follow-up.
- **Rerank stage (#796, steer of 2026-11-20).** `rerank: true` (or the workspace default,
  `LCG_RERANK`) adds #796's stage as the final arbiter, reused rather than re-implemented: the
  same `Reranker` client, `passage_rerank::blend` and `assemble`, with the same position-aware
  blend weights and candidate count (`LCG_RERANK_CANDIDATES`, 50 by default). The pipeline is run
  for the whole pool (`num_results` raised to the pool size, so the list caps grow with it), the
  cross-encoder scores the top fused candidates — a passage on its text, a fact on
  `from → to: fact` — and the result is cut back to `k`. Belief is applied **after** the
  cross-encoder, exactly as #796 does: each candidate carries the supersession row of its own
  episode (a passage) or its producing episode (a fact), the superseded factor scales its whole
  blended score, and a superseded item is moved below any replacement in the head, so it never
  ends above its replacement (qmd-rerank loses temporal, 0.529 → 0.335, because its reranker
  ignores time). Candidates are keyed by episode, so a replacement is recognised whether it
  arrives as a passage or as a fact it produced. `stats.rerank` reports `{applied, candidates,
  reason?, elapsed_ms?}`; a missing reranker (`not_configured`), a failed call or a failed
  supersession lookup (`unavailable`) leave the fused order, cut to `k`. Eval rows:
  `lcg-graph-rerank` and `lcg-graph-seeds-rerank`, read against `lcg-passages-belief-rerank`.
- **Caller `queries` (#802) are not passed into list (b) yet.** List (b) runs its vector and BM25
  lists directly, not through `knowledge_search_passages`, so adopting #802's weighted variant
  fusion means embedding the variants too (more model calls) and adding one list per variant. That
  is a follow-up, not a one-line change.

## Rejected alternatives

- **`*_with_embedding` variants of the hybrid searches** — refactors a shared hot path to save three
  short calls; puts byte-identical output at risk.
- **One Cypher query for the walk** — cannot prune between hops or evaluate belief, and the
  variable-length pattern is unverified on lbug.
- **Personalised PageRank (HippoRAG)** — principled, but needs the whole neighbourhood loaded and many
  iterations; decay + beam gives the same "near and relevant first" ordering within the latency budget.
- **Changing `lcg-entities-facts` in place** — would move historical numbers and break SC-008.
