# Feature Specification: Graph-aware search — one read that fuses entity linking, neighbourhood expansion, belief time, vector and FTS

**Feature Branch**: `fabrik/issue-797`
**Created**: 2026-11-18
**Status**: Specified
**Input**: User description: "Graph-aware search: one read that fuses entity linking, neighbourhood expansion, belief time, vector and FTS

## Problem

LCG is a context graph. It combines an entity and relationship graph with belief time, vector indexes (HNSW over passages, entity summaries and facts) and full-text indexes. Its retrieval still uses each of these on its own. On the v2 retrieval corpus (#788, baseline on #789), the graph read loses to plain passage search in exactly the categories a graph should win:

| nDCG@10 | `lcg-entities-facts` | `lcg-passages` | `qmd-rerank` |
|---|---|---|---|
| overall | 0.275 | **0.567** | 0.455 |
| **multi-hop** (\"what does Cedar Core's supplier charge?\") | 0.154 | **0.655** | 0.397 |
| **paraphrase** (\"the initiative covering shift rostering\") | 0.009 | 0.150 | **0.492** |
| temporal | 0.391 | **0.628** | 0.293 |

The graph config doesn't traverse. It runs two flat similarity reads (`knowledge_find_entities` and `knowledge_find_relationships`) against the query string and interleaves them:
- **Multi-hop:** the question names Cedar Core, so we find Cedar Core and its facts, but we never follow `SUPPLIED_BY` to Fable Supply and its price.
- **Paraphrase:** the question names nothing. The graph already knows \"Marlin Core is the project for scheduling\" in the entity summary, but a raw query-to-name or query-to-fact similarity never reaches it.

The pieces all exist as separate RPCs: `knowledge_find_entities` (hybrid BM25 + HNSW, RRF), `knowledge_resolve_entity`, `knowledge_get_entity_neighbors`, entity summary embeddings, `knowledge_find_relationships` (belief-time aware) and `knowledge_search_passages`. Nothing composes them into one query path. A client such as the Liminis Notes agent has to orchestrate them itself, and it doesn't.

This is LCG's real advantage over qmd-style search. qmd buys paraphrase with a 1.7B expansion LLM and a reranker at 1.7–3.3 s per query, and has no graph at all.

## Desired behaviour

A composite read, working name `knowledge_search`, that uses all of LCG's modes in one deterministic call. There is no LLM at query time, the p50 target is ≤ 100 ms at k = 10, and it is opt-in alongside the existing reads.

1. **Seed entities (entity linking).** Find the entities the query is about, combining:
   - exact and fuzzy name matching (`knowledge_resolve_entity`);
   - BM25 over entity names and summaries;
   - vector similarity of the query to entity summaries, using the query role from #790.

   Matching on summaries is what resolves role descriptions (\"the initiative covering shift rostering\" → Marlin Core). Score and cap the seeds.
2. **Neighbourhood expansion.** Walk 1–2 hops from the seeds along relationships, scoring each edge by its fact's relevance to the query (vector + BM25) and decaying with hop distance. This is what multi-hop needs: Cedar Core → supplier → Fable Supply → price.
3. **Belief time.** By default traverse and return only currently believed facts. Respect `believed_only`, `as_of` and supersession (ADR-0705/0706/0707) so the current decision wins (temporal).
4. **Fusion.** RRF over three lists:
   - (a) passages that support the selected facts and seed entities (their episodes);
   - (b) plain passage search (vector, plus BM25 once #789 lands);
   - (c) the facts themselves.
5. **Evidence trail.** Each result says why it was returned: the seed entity, the edge path and the supporting episode. An agent can then cite \"Cedar Core → Fable Supply → $87/unit (from note X)\".
6. **Origin aware** (ADR-0752): derived pages cite but never corroborate. The origin filter applies to every list.

## Acceptance (on v2 with `lcg-eval retrieval`; per category, bootstrap intervals; new config `lcg-graph`)

- **Multi-hop** and **paraphrase** nDCG@10 beat `lcg-passages`, with paired intervals above zero.
- **Overall** nDCG beats `lcg-passages`.
- **Temporal** is no worse than `lcg-passages` (paired interval includes or exceeds zero).
- **Direct, answer and passage** don't regress.
- p50 ≤ 100 ms; the report shows calls per query.
- The existing reads are unchanged; clients that don't call `knowledge_search` see no difference.
- Report the gain from each stage separately: seeds only, plus expansion, plus fusion.

## Sequencing

- Measuring needs **#788**, the v2 corpus with corrected relevance labels.
- It gains from **#790** (query role for summary matching) and from **#789** (BM25 for passages in list b). It can start on what exists, but should be measured after both.
- **#796**, the local reranker, can sit on top as a final stage. Measure it with and without.
- Notes-side follow-up after an lcg release: the agent's search tool uses `knowledge_search`.

## Questions for Research
- How to score seeds and combine name, BM25 and summary-vector evidence; how many seeds to keep.
- Hop limit, fan-out cap and edge scoring: fact vector against query, relation-type priors, hop decay.
- Should expansion run inside one Cypher/lbug query, or as a few indexed lookups? The latency budget decides.
- Is `lcg-entities-facts`'s current interleaving unfair to the graph in the eval regardless? If so, fix the config too."

## Background

LCG is a *context graph*: an entity and relationship graph with belief time (ADR-0705/0706/0707), vector indexes (HNSW over passages, entity summaries and facts) and full-text indexes. Today every read uses one of these on its own, and the eval shows the cost. On the v2 retrieval corpus (#788, ADR-0788; provisional baseline posted on #789; BGE-base embedder, k = 10), the graph configuration loses to plain passage search in exactly the categories where a graph should win:

| nDCG@10 | `lcg-entities-facts` | `lcg-passages` | `qmd-rerank` |
|---|---|---|---|
| overall | 0.275 | **0.567** | 0.455 |
| multi-hop ("what does Cedar Core's supplier charge?") | 0.154 | **0.655** | 0.397 |
| paraphrase ("the initiative covering shift rostering") | 0.009 | 0.150 | **0.492** |
| temporal | 0.391 | **0.628** | 0.293 |

The graph configuration never traverses. It runs two flat similarity reads against the query string — entity search and relationship search — and interleaves their results:

- **Multi-hop fails** because the question names Cedar Core, so Cedar Core and its own facts are found, but nothing follows the `SUPPLIED_BY` edge to Fable Supply and on to its price.
- **Paraphrase fails** because the question names no entity. The graph already holds "Marlin Core is the project for scheduling" in an entity summary, but neither query-to-name nor query-to-fact similarity reaches it.

Every building block already exists as a separate read: hybrid entity search (BM25 + HNSW fused with RRF), exact/fuzzy entity resolution, entity-neighbour lookup, entity summary embeddings, belief-time-aware relationship search, and passage search. Nothing composes them into one query path. A client such as the Liminis Notes agent would have to orchestrate them itself, and it doesn't.

This is LCG's structural advantage over qmd-style search. qmd buys paraphrase recall with a 1.7B query-expansion LLM and a cross-encoder reranker, at 1.7–3.3 s per query, and has no graph at all. A deterministic graph-aware read with no query-time LLM could beat it on paraphrase and multi-hop questions at a fraction of the latency, and its evidence trail lets an agent cite *why* an answer holds.

**Related work and sequencing**

- **#788** (closed): the v2 corpus with corrected relevance labels, needed for measurement. The baseline table above was taken before its label corrections were finished, so the numbers to beat are those of a fresh run on the final corpus.
- **#790** (closed, ADR-0790): embedder roles. Queries are embedded with the `Query` role, which matters for matching queries against entity summaries.
- **#789** (open): hybrid BM25 + vector passage search. It provides the BM25 half of the plain-passage list this read fuses. This work can start without it but should be measured after it lands.
- **#796** (open): an optional local reranker and opt-in LLM query expansion. It can sit on top of this read as a final stage.
- **Notes follow-up**: after an LCG release, the Notes agent's search tool switches to this read. That is tracked separately.

**Prior art** (context only, not a prescription)

- *Microsoft GraphRAG "local search"* links the query to entities by embedding entity descriptions, expands to their neighbours, relationships and source text units, and ranks the mix into one context window. It is the closest analogue to seeds → expansion → supporting passages.
- *HippoRAG* seeds Personalized PageRank from entities linked to the query and scores passages by the mass that reaches them. That is one principled way to get hop decay and fan-out control.
- *LightRAG* runs dual-level retrieval: low-level (entities) and high-level (relations/themes), merged.
- *Graphiti* (whose Kuzu schema LCG tracks) combines hybrid search with BFS from seed nodes and graph-distance reranking, fused by RRF.
- *qmd* fuses BM25 and vector with RRF plus a top-rank bonus, then adds LLM expansion and reranking. The bonus is being adopted for passages in #789.

**Questions for the Research stage** (from the original issue; these are design questions, not product questions):

- How should seeds be scored, combining name-match, BM25 and summary-vector evidence, and how many should be kept?
- What hop limit, fan-out cap and edge scoring work best: fact vector against the query, relation-type priors, hop decay?
- Should expansion run as one graph query or as a few indexed lookups? The latency budget decides.
- Is the eval's current `lcg-entities-facts` interleaving unfair to the graph regardless of this feature? If so, fix that configuration too (FR-019).

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Multi-hop answers by following relationships (Priority: P1)

An agent asks "what does Cedar Core's supplier charge?". One call finds Cedar Core, follows its supplier relationship to Fable Supply, and returns Fable Supply's price fact and the note that states it near the top. The evidence trail shows the path.

**Why this priority**: Multi-hop is the category where a graph should beat passage search most clearly, and today it loses badly (0.154 against 0.655). Traversal is the core capability this issue adds.

**Independent Test**: On a small fixture where entity A relates to entity B, B has a fact F, and F is stated only in a note that never mentions A, ask a question that names A and asks for F. The note stating F ranks in the top k, and its result's evidence shows A → B → F.

**Acceptance Scenarios**:

1. **Given** a graph where Cedar Core is supplied by Fable Supply and Fable Supply's price is recorded in note X, which does not mention Cedar Core, **When** the agent searches "what does Cedar Core's supplier charge?", **Then** note X (or the price fact) is returned in the top k, with evidence naming Cedar Core as the seed and the Cedar Core → Fable Supply → price path.
2. **Given** a seed entity with many relationships, only one of them relevant to the query, **When** the search runs, **Then** facts on the relevant edge outrank facts on the unrelated ones.
3. **Given** a fact two hops from the seed and an equally relevant fact one hop away, **When** the search runs, **Then** the one-hop fact ranks at least as high (hop decay).

---

### User Story 2 - Paraphrase: find an entity described by its role (Priority: P1)

An agent asks about "the initiative covering shift rostering" without naming it. The read links the description to Marlin Core through Marlin Core's entity summary and returns Marlin Core's facts and supporting notes.

**Why this priority**: Paraphrase is LCG's weakest category (0.150 for passages, 0.009 for the graph read) and the one qmd wins by spending an LLM. Matching on summaries is the graph's cheaper answer.

**Independent Test**: On a fixture where entity E's summary describes its role in words that appear in no note's text, a query using only the role description returns E's supporting notes in the top k, with evidence showing E was seeded by summary match.

**Acceptance Scenarios**:

1. **Given** Marlin Core's summary says it is the project for scheduling, **When** the agent searches "the initiative covering shift rostering", **Then** Marlin Core is among the seeds and its supporting notes appear in the top k.
2. **Given** a query that matches an entity's name exactly, **When** the search runs, **Then** that entity is seeded and its name match is reported in the evidence.
3. **Given** a query that links to no entity at all, **When** the search runs, **Then** it still returns plain passage results (no error and no empty result while passages match).

---

### User Story 3 - The current belief wins (Priority: P1)

An agent asks about a decision that was later revised or superseded. By default the read traverses and returns only facts believed now, so the current decision ranks above the stale one. A caller can instead ask what was believed at an earlier time, or include facts no longer believed.

**Why this priority**: Temporal is a strength of passage search today (0.628) and must not regress. Belief time is something qmd cannot express at all.

**Independent Test**: On a fixture where note A states a decision and a later note B corrects it (an episode revision) or supersedes it (document supersession), the default search ranks B's fact and note above A's, and A's fact is not returned as a believed fact.

**Acceptance Scenarios**:

1. **Given** a fact whose producing episode was superseded by a correction, **When** a default search runs, **Then** that fact is neither returned nor used as a traversal step.
2. **Given** the same graph, **When** the search is run with an `as_of` before the correction, **Then** the old fact is believed and can be returned, and the newer fact is not.
3. **Given** the same graph, **When** the caller turns `believed_only` off, **Then** facts no longer believed may be returned, and each one's evidence reports its belief/supersession state.
4. **Given** a document superseded with a future effective date, **When** a default search runs before that date, **Then** its facts are still treated as believed (matching ADR-0707).

---

### User Story 4 - Every result explains itself (Priority: P2)

Each result carries an evidence trail: which seed entities it came from and how each was matched, the edge path walked, the supporting episode or note, and which fused lists contributed. An agent can then write "Cedar Core → Fable Supply → $87/unit (from note X)".

**Why this priority**: It is what lets an agent cite answers, and it makes ranking debuggable. Ranking quality (Stories 1–3) is still the headline.

**Independent Test**: For every result of a search on the fixture, the evidence names at least one contributing list, and every result reached through the graph names its seed, its path and its supporting episode, all of which resolve to real graph objects.

**Acceptance Scenarios**:

1. **Given** a result reached by traversal, **When** the caller inspects it, **Then** the evidence lists the seed entity (with its match kind: name, BM25 or summary vector), the ordered edge path, and the supporting episode with its note source.
2. **Given** a result returned only by plain passage search, **When** the caller inspects it, **Then** the evidence says so and has no seed or path.
3. **Given** a result that several lists returned, **When** the caller inspects it, **Then** the evidence names all of them.

---

### User Story 5 - Origin-aware: derived pages cite but never corroborate (Priority: P2)

In a notebook where an agent has written summary pages back alongside human notes, a derived page restating a human fact never counts as independent corroboration. A caller filtering to primary/external origins gets no derived content from any part of the read.

**Why this priority**: ADR-0752 makes this a cross-cutting invariant. Without it the read would re-read the agent's own pages as evidence.

**Independent Test**: On a fixture where a derived page restates a fact from a primary note, (a) with `origins = [primary, external]` the derived page and its edges appear in no result, no traversal step and no evidence trail; (b) with no filter, the derived page's restatement does not raise the fact's rank above what the primary note alone gives it.

**Acceptance Scenarios**:

1. **Given** an `origins` filter, **When** the search runs, **Then** seeds, traversed edges, fact results, supporting passages and plain passage results all honour it.
2. **Given** a fact supported by one primary note and one derived page, and no filter, **When** the search runs, **Then** the fact's score reflects one corroborating source, and the derived page may appear only as a citation.

---

### User Story 6 - Measured in the eval, stage by stage (Priority: P2)

A maintainer runs `lcg-eval retrieval` on v2 and gets a new `lcg-graph` configuration beside the existing ones, with per-category nDCG, bootstrap and paired intervals against `lcg-passages`, p50 latency, and calls per query. Ablation rows show what each stage adds.

**Why this priority**: The issue is only done when the gain is measured; per-stage rows keep a stage that doesn't help from riding along.

**Independent Test**: Running the retrieval eval with the new configuration enabled produces a report with the `lcg-graph` row, per-category numbers, paired intervals against `lcg-passages`, p50, calls per query, and one row per ablation stage.

**Acceptance Scenarios**:

1. **Given** the v2 corpus and a real embedder, **When** the eval runs, **Then** the report includes `lcg-graph` with per-category nDCG@10 and bootstrap intervals, and paired differences against `lcg-passages`.
2. **Given** the same run, **When** the report is read, **Then** it shows separate rows for seeds only, seeds + expansion, and full fusion.
3. **Given** the same run, **When** the report is read, **Then** it shows p50 latency and both service calls and model calls per query for `lcg-graph`.

---

### User Story 7 - Existing clients see no difference (Priority: P1)

A client that never calls the new read — the Notes app over the socket, an MCP client, the WAL, every existing eval configuration — sees the same results, schema and behaviour as before.

**Why this priority**: The read is opt-in. Breaking the IPC contract would break the Notes app (see CLAUDE.md, "Build artifact").

**Independent Test**: The existing read and IPC-parity test suites pass unchanged, and the existing eval configurations produce identical rankings on the same build and corpus.

**Acceptance Scenarios**:

1. **Given** a client calling only existing methods, **When** this feature ships, **Then** their requests and responses are unchanged.
2. **Given** the graph store, **When** this feature ships, **Then** no schema migration, WAL format change or re-ingest is required.

### Edge Cases

- **Empty graph, or a group with episodes but no entities**: the read returns plain passage results, or an empty list when nothing matches; never an error.
- **No seeds found**: the graph lists are empty and the result equals plain passage search, flagged in evidence.
- **Hub entities** with hundreds of relationships: fan-out is capped so latency stays within budget. The cap is reported when it truncated the walk.
- **Cycles and back-edges** (A → B → A): an edge or entity is never counted twice along one path, and traversal terminates.
- **Several paths to the same fact or passage**: the item appears once in the results; its evidence keeps the best path, and may list the others.
- **Legacy edges** with neither producing episodes nor a warrant (ADR-0705): believed, so traversable, but they have no supporting passage; the fact can be returned without one.
- **Warranted edges** (`assertion`, `cross_group`) with no producing episode: as legacy edges, traversable, with evidence naming the warrant instead of an episode.
- **Cross-group edges**: traversal stays within the requested group(s); an edge to an entity outside them is not followed (consistent with existing group scoping).
- **Belief changes mid-path**: a path is valid only if every edge on it is believed at the evaluated time; a superseded intermediate edge breaks the path.
- **Future-effective document supersession**: believed until the effective date (ADR-0707).
- **Entities whose only mentions are derived** under a primary/external filter: they cannot be seeds and are not traversed through.
- **Missing summary embeddings** (an older graph before summary backfill): summary-vector seeding contributes nothing for those entities; name and BM25 seeding still work.
- **Embedder unavailable**: the read fails with a clear error the same way other vector reads do, rather than silently degrading.
- **Indexes not yet built** (lazy first-search build): behaves as the existing reads do.
- **Very long or empty query string**: an empty query is an input error, as with existing search reads.
- **`k` larger than the candidate pool**: returns what exists, never padded.

## Requirements *(mandatory)*

### Functional Requirements

**Surface**

- **FR-001**: LCG MUST offer a new composite read (working name `knowledge_search`) that takes a query string and returns one ranked list of results combining graph-derived and passage-derived evidence.
- **FR-002**: The read MUST be additive and opt-in. No existing read, response shape, default, stored schema or WAL format may change.
- **FR-003**: The read MUST be reachable over every transport that serves the existing reads (the Unix-socket JSON-RPC surface and MCP over stdio), classed as a read operation, and documented in the IPC/MCP reference alongside the existing reads.
- **FR-004**: The read MUST accept the same scoping and filter parameters as the existing search reads, with the same names and meanings: group scoping, result count (`num_results`, consistent with `knowledge_find_entities`), `origins`, `believed_only` and `as_of`.
- **FR-005**: The read MUST be deterministic: the same graph state, parameters and embedder output yield the same results in the same order.
- **FR-006**: The read MUST make no LLM call at query time. Its only model call is embedding the query (one per request, with the `Query` role from ADR-0790).

**Stage 1 — seed entities (entity linking)**

- **FR-007**: The read MUST identify seed entities for the query by combining at least three kinds of evidence: exact/fuzzy name matching, BM25 over entity names and summaries, and vector similarity between the query and entity summaries.
- **FR-008**: Seeds MUST be scored and capped, so that a query matching many entities still yields a bounded seed set.

**Stage 2 — neighbourhood expansion**

- **FR-009**: The read MUST walk relationships outward from the seeds up to a bounded hop limit of at least 2.
- **FR-010**: Each traversed edge MUST be scored by its fact's relevance to the query, combining vector and lexical evidence, and that score MUST decay with hop distance from the seed.
- **FR-011**: Expansion MUST cap fan-out per entity and in total, so that hub entities cannot blow the latency budget, and MUST never revisit an edge along one path.

**Stage 3 — belief time**

- **FR-012**: By default the read MUST return and traverse only facts that are currently believed. Where `believed_only` and `as_of` mean something for the existing belief-aware reads (ADR-0705/0706/0707), they MUST mean the same here. Note that this read defaults to believed-only, while `knowledge_find_relationships` does not (see Assumptions).
- **FR-013**: A path MUST be valid only if every edge on it is believed at the evaluated time. Supporting passages MUST come from episodes live at that time, unless the caller has turned `believed_only` off.
- **FR-014**: When the caller turns `believed_only` off, results that are no longer believed MUST report their belief and supersession state, in the same vocabulary as the existing belief-aware reads.

**Stage 4 — fusion**

- **FR-015**: The read MUST fuse at least three ranked lists with reciprocal rank fusion: (a) passages that support the selected facts and seed entities, (b) plain passage search, using whatever ranking `knowledge_search_passages` provides on the same build (hybrid once #789 lands), and (c) the facts themselves.

**Evidence and origin**

- **FR-016**: Every result MUST carry an evidence trail naming which fused lists returned it. A result reached through the graph MUST also name its seed entity or entities (with how each was matched), the ordered edge path and the supporting episode with its note source.
- **FR-017**: When `origins` is given, it MUST apply to every list and every stage: seeding, traversal, fact results, supporting passages and plain passage results. Under ADR-0752, `derived` and `answer` content MUST never count as corroboration in ranking, though it may appear as a cited source when no filter excludes it.

**Ablation and eval**

- **FR-018**: The stages MUST be individually switchable (seeds only; seeds + expansion; full fusion), so their separate contributions can be measured. The full pipeline is the default.
- **FR-019**: `lcg-eval retrieval` MUST gain an `lcg-graph` configuration that drives the new read, plus ablation configurations for the stages in FR-018. Its report MUST show per-category nDCG@10 with bootstrap intervals, paired intervals against `lcg-passages`, p50 latency, and service and model calls per query. If Research finds the existing `lcg-entities-facts` interleaving unfair to the graph independently of this feature, that configuration MUST be fixed too, and the change MUST be stated in the report.
- **FR-020**: If #796's reranker has landed by the time of measurement, the report MUST show `lcg-graph` both with and without it. Otherwise the report MUST say it was not measured.

### Key Entities *(if applicable)*

- **Seed entity**: an entity the query is judged to be about, with a score and the kinds of evidence that matched it (name, BM25, summary vector).
- **Path**: an ordered sequence of believed edges from a seed to a reached entity or fact, with its hop count and decayed score.
- **Search result**: one ranked item, either a fact (relationship) or a passage (episode text), with a fused score and an evidence trail.
- **Evidence trail**: per result, the contributing lists, seeds, path(s), supporting episode(s) with note source and origin, and, where applicable, belief/supersession state.

## Success Criteria *(mandatory)*

### Measurable Outcomes

All measured with `lcg-eval retrieval` on the v2 corpus (final, corrected labels), k = 10, with a real embedder and the same embedding profile for every configuration, against `lcg-passages` from the same build and run.

- **SC-001**: `lcg-graph` multi-hop nDCG@10 exceeds `lcg-passages`, with a paired bootstrap interval entirely above zero.
- **SC-002**: `lcg-graph` paraphrase nDCG@10 exceeds `lcg-passages`, with a paired bootstrap interval entirely above zero.
- **SC-003**: `lcg-graph` overall nDCG@10 exceeds `lcg-passages`.
- **SC-004**: `lcg-graph` temporal nDCG@10 is no worse than `lcg-passages`: the paired interval includes or exceeds zero.
- **SC-005**: Direct, answer and passage nDCG@10 do not regress against `lcg-passages`: each paired interval includes or exceeds zero.
- **SC-006**: `lcg-graph` warm p50 latency is ≤ 100 ms per query at k = 10, including the query embedding. The report shows service calls per query (target: 1) and model calls per query (target: 1).
- **SC-007**: The report shows ablation rows (seeds only, + expansion, + fusion), so each stage's gain per category is visible.
- **SC-008**: Clients that don't call the new read see no difference: existing read, IPC-parity and MCP registry tests pass with only additive changes, and existing eval configurations produce unchanged rankings on the same corpus and build.
- **SC-009**: Every `lcg-graph` result in the v2 run carries an evidence trail, and every graph-reached result names a seed, a path and (unless the edge is legacy or warranted) a supporting episode that exist in the graph.

## Assumptions

- **Single ranked list of mixed items.** The read returns one list whose items are facts or passages, each tagged with its kind. The eval maps both to notes, as it already does: facts through their producing episodes, passages through their source. The exact response shape is a Plan decision.
- **`knowledge_search` is a working name.** The final method name is chosen in Plan. It must follow the `knowledge_*` convention and not be confusable with `knowledge_search_passages`.
- **Believed-only is the default for this read,** as the issue requests, although `knowledge_find_relationships` defaults to off. This is a deliberate, documented difference for a new read, not a change to the existing one.
- **The baseline to beat is re-measured.** The table in Background predates #788's final label corrections, and `lcg-passages` will improve when #789 lands. Success criteria are judged against `lcg-passages` from the same build and run, not against the numbers quoted above.
- **Measurement timing.** Implementation proceeds on what exists. If #789 has not merged by Validate, list (b) is vector-only, and the report says so. The success criteria still apply against `lcg-passages` on that build.
- **A missed target is reported, not hidden.** If a success criterion is missed after reasonable tuning, the report states which one and by how much, and the maintainer decides whether to merge. Because the read is opt-in, shipping it cannot regress existing clients.
- **Latency is warm latency,** measured as ADR-0754 defines it (indexes built, one unmeasured warm-up pass). It is measured on the eval machine, which should be Apple silicon, like the #789 baseline.
- **No new storage.** All signals already exist: entity summaries and their embeddings, fact embeddings, FTS indexes, `RelatesToNode_.episodes`, belief columns and origin. No schema change or re-ingest is expected. If Research finds one is needed, that is escalated rather than assumed.
- **Tuning constants** (seed cap, hop limit, fan-out caps, decay, RRF constant) are internal defaults chosen by Research/Plan and validated on v2. They may be exposed as optional parameters, but the defaults must be sane without them.
- **The Python-side `service_protocol.py`** (Liminis app repo) adopts the new method in the Notes follow-up. This issue only has to keep the existing IPC contract intact.

## Out of Scope *(optional)*

- Any change to existing reads (`knowledge_find_entities`, `knowledge_find_relationships`, `knowledge_search_passages`, `knowledge_get_entity_neighbors`, `knowledge_resolve_entity`) or their defaults. Hybrid passage search itself is #789.
- Query-time LLM use of any kind: query expansion, HyDE, or LLM reranking. These belong to #796.
- Building the local cross-encoder reranker (#796); this issue only measures with it if it already exists (FR-020).
- The Notes agent switching its search tool to the new read (a Notes-side follow-up after an LCG release).
- New ingest-time signals, such as new embeddings, community summaries or relation-type priors that need new stored data.
- Changes to the v2 corpus or its labels (#788). Changes to the eval harness are limited to the new configurations, the ablation rows, the calls-per-query reporting and, if warranted, the `lcg-entities-facts` fix (FR-019).
- Global or "community" summarisation queries in the GraphRAG global-search sense.

## Source References *(optional)*

- Issue #788 / ADR-0788 — retrieval corpus v2 and bootstrap reporting
- Issue #789 — hybrid BM25 + vector passage search (provides list (b)'s BM25 half; holds the provisional v2 baseline)
- Issue #790 / ADR-0790 — embedder roles (query role for summary matching)
- Issue #796 — local reranker and opt-in LLM query expansion
- ADR-0754 — retrieval eval architecture (configurations, metric rules, warm latency)
- ADR-0705 / ADR-0706 / ADR-0707 — belief time, correction reads, document supersession
- ADR-0752 — episode origin: derived content cites but never corroborates
- ADR-0629 — search evidence carried through RRF; `min_similarity` floor
- ADR-0012 — `episode_uuids` either-endpoint inference (not to be used as provenance)
- Microsoft GraphRAG, local search: https://microsoft.github.io/graphrag/query/local_search/
- HippoRAG (Gutiérrez et al., 2024), Personalized PageRank over a knowledge graph for multi-hop retrieval: https://arxiv.org/abs/2405.14831
- LightRAG (Guo et al., 2024), dual-level graph retrieval: https://arxiv.org/abs/2410.05779
- Graphiti search (hybrid + BFS + node-distance reranking, RRF): https://github.com/getzep/graphiti
- qmd: https://github.com/tobi/qmd
