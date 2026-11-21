---
layout: default
title: Retrieval Experiments
---

# Retrieval experiments: master record

This is the record of every retrieval experiment and its result: what was tried, what it scored, and what was decided. Add a new entry for every eval run that informs a decision, including the ones that fail.

**Goal:** LCG retrieval should beat qmd for the Liminis Notes agent.

## Standard setup (unless an entry says otherwise)

- **Harness:** `lcg-eval retrieval --k 10 --embedding-profile bge-v1.5 --replay-cassette … --embedder-uds …`
  - Extraction is replayed from a committed Haiku 4.5 cassette, so ingest is deterministic.
  - Runs with `--no-grounded` use label-based nDCG and need no judge.
  - Judged runs use a Sonnet 4.6 judge with a judge cache.
- **Embedder:** BGE base v1.5, Core ML, in the LocalInference sidecar.
- **Corpora:**
  - **v2:** `meridian-notebook-2` / `meridian-questions-3`. 408 notes and 244 questions, of which 210 are scored.
    - Categories: direct, paraphrase, temporal, answer, multi-hop and passage, plus negatives.
    - **Tune on v2 only.**
  - **v1:** `fernhill-notebook-1`. 31 notes and 40 scored questions.
    - Categories: direct, paraphrase, temporal and answer. Direct questions are near the ceiling.
    - **v1 is the holdout:** report it for final settings and never tune on it.
- **Metric:** nDCG@10, with paired-bootstrap 95% intervals. Treat a difference as real only when its interval excludes 0.
- **Artifacts:** each run's `report.md` / `report.json` is kept by the maintainer, outside the repo. The PR or issue comment linked in each entry has the full tables.
- **Methodology:** see [Retrieval-Quality Evaluation](retrieval-quality-evaluation.md) for the harness, corpora and question format.

## Current standing (2026-11-20, main after [#806](https://github.com/liminisapp/liminis-context-graph/issues/806), [#796](https://github.com/liminisapp/liminis-context-graph/issues/796) and [#802](https://github.com/liminisapp/liminis-context-graph/issues/802))

| v2 nDCG@10 | overall | direct | paraphrase | temporal | answer | multi-hop | passage | p50 |
|---|---|---|---|---|---|---|---|---|
| qmd | 0.507 | 0.639 | 0.347 | 0.529 | 0.700 | 0.473 | 0.353 | 1.8 s |
| qmd-rerank | 0.476 | 0.623 | **0.475** | 0.335 | 0.573 | 0.430 | 0.422 | 3.5 s |
| `lcg-passages-raw` (pre-[#806](https://github.com/liminisapp/liminis-context-graph/issues/806) default) | 0.574 | 0.768 | 0.281 | 0.619 | 0.779 | 0.673 | 0.343 | 24 ms |
| `lcg-passages` (default, belief-aware) | 0.653 | 0.826 | 0.328 | 0.745 | 0.918 | 0.758 | 0.361 | 25 ms |
| **`lcg-passages-belief-rerank`** (opt-in) | **0.759** | 0.910 | 0.328 | **0.800** | **0.942** | 0.740 | **0.913** | 1.9 s |

- **v1:** the default scores 0.894 and belief plus rerank 0.935. The qmd row on v1 (0.834 / 0.836) is from the Ollama-embedder era, so it doesn't compare directly.
- **Per-question oracle on v2:** 0.814. The remaining gap is mostly **paraphrase**, where qmd-rerank still leads (0.475 against 0.328).

## Experiments

Newest first. **Δ** is against the stated baseline, paired on v2 unless marked.

### E13: graph multi-hop probes and an ontology ([#815](https://github.com/liminisapp/liminis-context-graph/issues/815), local probe on main @ `19759906`, v2). Promising, not shipped

The question: does the graph answer what chunk search can't (chains, joins, inverse lookups), and does an ontology make it usable?

- **Free-form graph audit (v2, standard cassette):**
  - 2,016 edges carry 1,223 distinct relation types.
  - Hub entities have 80–115 edges.
  - The facts for the 15 real chain questions are present: 15/15 entities resolve and hop 1 is there for 15/15, hop 2 for 12/15.
  - Question-guided walks found the answer in the top fact for at most 7/15, against 1/15 for flat fact search.
  - `knowledge_canonicalize_relations` typed only 26–28% of edges lexically.
- **Ontology v3 → v3.1.** It was written from a purpose statement and 25+4 competency questions (CQs), not from the corpus. Fit was judged by blind classifier agents given only the purpose and the type menu.
  - Bar: ≥ 85% of purpose-relevant ("core") facts typed on both corpora, and κ ≥ 0.8.
  - v3: 93–98% core typed on v2, κ 0.87, but only **45% on v1**. Missing: roles and titles, plans and schedules, proposals, replacement.
  - v3.1 (+`HAS_ROLE`, `SCHEDULED_FOR`, `PROPOSED`, wider `SUPERSEDES`; 27 types): **97% / 97%** on a fresh v2 sample and **98%** on v1, κ 0.82, with incidental typing at 17–24%. The v1 figure is optimistic, since those facts were read while revising.
- **Live re-extraction of v2 under v3.1 (strict), Haiku 4.5 via subscription:**
  - 3,675 calls with 0 errors. The new cassette is `v2-haiku45-onto31`.
  - Result: 2,612 edges over **28 types**, of which 74 are `UNCLASSIFIED` (3%). Nodes went from 769 to 1,202, so entity splitting needs a look.
  - `lcg-entities-facts` nDCG 0.283 → 0.294 (noise). Passages are unaffected.
- **Graph-native question set:** 129 questions in 13 families, shapes from the CQs and gold from the skeleton.
  - Metric "complete": every hop has a gold note among the returned evidence.
  - Metric "answer": every gold answer appears in the returned facts.
  - Passages are scored strictly: all gold notes in the top 10.
  - The walk is LLM-free: exact-name seed, then at each hop the top 3 edges by cosine (10 for lists and joins). "Typed" filters each hop to its CQ relation types.

| runner (graph) | chain-2 (48) | chain-3 (24) | list (31) | join (14) | history (12) | **all (129)** | answer found |
|---|---|---|---|---|---|---|---|
| `lcg-passages` top 10 | 9 | 0 | 20 | 0 | **12** | 41 | n/a |
| flat facts top 10 (typed graph) | 5 | 0 | 28 | 0 | 10 | 43 | 50 |
| walk, untyped (free-form) | 17 | 3 | 28 | 12 | 9 | 69 | 93 |
| walk, typed + fallback (free-form) | 18 | 4 | 24 | 4 | 1 | 51 | 97 |
| walk, untyped (typed graph) | 22 | 9 | 26 | 11 | 11 | 79 | 95 |
| **walk, typed (typed graph)** | **35** | **19** | **29** | 12 | 9 | **104** | **119** |

- **Learned:**
  - Chunk search can't do chains or joins: `lcg-passages` gets 0/24 on 3-hop and 0/14 on joins, with complete evidence for 41 of 129 questions.
  - A typed walk over the ontology graph gets complete evidence for 104/129 and finds the answer for 119/129, returning about 6 facts against 10 passages.
  - The ontology is the lever. Typing the free-form graph after the fact doesn't help (51, which falls below the untyped walk's 69). Extracting under the ontology does (104).
  - Passages still win change history (12/12 against 9/12), so route by question type.
- **Caveats:**
  - Step 1's walk plan is oracle and "answer found" is substring presence. Step 2 addresses both.
  - v2 is nearly one-to-one (each supplier serves one project), which understates aggregation.
  - The walk and the `--ontology` hook are local patches, not in the repo.
- **Step 2: answer accuracy with a planner and a reader**. Haiku 4.5 via the subscription is both the planner and the reader, and both are cached.
  - The planner sees only the question and the relation-type menu: no families, no gold. It returns a route (graph or passages), a start entity and typed hops.
  - Every runner's context goes to the same reader.
  - Scoring: first answer matches for single answers, set F1 for lists and joins, exact in-order match for history.

| runner (mean score) | chain-2 | chain-3 | list | join | history | **all** | vs passages (95% CI) |
|---|---|---|---|---|---|---|---|
| `lcg-passages` top 10 | 0.19 | 0.00 | 0.72 | 0.05 | **1.00** | 0.34 | n/a |
| walk, oracle path (typed graph) | 0.90 | 0.92 | 0.77 | 0.62 | 0.50 | 0.80 | +0.46 [+0.35, +0.57] |
| walk, planned path (typed graph) | 0.90 | 0.88 | 0.77 | 0.60 | 0.17 | 0.76 | +0.42 [+0.30, +0.54] |
| **route, planned (typed graph)** | 0.90 | 0.88 | 0.77 | 0.60 | **1.00** | **0.84** | **+0.50 [+0.40, +0.60]** |
| route, planned (free-form graph) | 0.71 | 0.62 | 0.92 | 0.55 | 0.83 | 0.74 | +0.40 [+0.29, +0.50] |

  - **Planning costs little.** The planned path comes within 0.04 of the oracle path. All 119 graph starts resolved to a node, and the planner sent 10 of 12 history questions to passages.
  - **The ontology matters for the final answer.** Typed against free-form, route-planned scores +0.10 [+0.02, +0.18] (22 wins, 12 losses); walk-oracle +0.08 [+0.01, +0.15]. The gain is in chains (0.90/0.88 against 0.71/0.62). The free-form graph does better on lists (0.92 against 0.77).
  - **Misses:**
    - Some chain-2 questions return nothing on either path. These are gaps in extraction or data, not in planning.
    - Supplier-contact joins stop at the vendor instead of following on to the project.
    - Owner-city joins pull in non-project entities through `OWNS`, because the walk doesn't check the type of the entity it lands on.
    - A history walk returns partial lists, so passages are the right route for history.
  - **Caveats:**
    - The planner's route rule (attribute or history → passages) was written after the step-1 finding, so the route row is not a blind test.
    - The reader is Haiku.
    - The data is synthetic and nearly one-to-one.
- **Step 3: the qmd and rerank baselines on the same set.** The setup:
  - Same planner, same reader, same scoring as step 2.
  - The mxbai-rerank-large-v1 Core ML model was re-staged; its max logit error against PyTorch is 0.006.
  - qmd runs the way lcg-eval calls it, with and without its own reranker. The reader gets the **whole note** for each qmd hit, which is generous to qmd: about 2.3 KB per hit against about 350 characters per LCG passage.

| runner (mean score) | chain-2 | chain-3 | list | join | history | **all** |
|---|---|---|---|---|---|---|
| `qmd` | 0.19 | 0.00 | 0.72 | 0.24 | 0.67 | 0.33 |
| `qmd-rerank` | 0.17 | 0.00 | 0.89 | 0.15 | 0.08 | 0.30 |
| `lcg-passages` | 0.19 | 0.00 | 0.72 | 0.05 | **1.00** | 0.34 |
| `lcg-passages-belief-rerank` | 0.21 | 0.00 | 0.88 | 0.00 | 0.92 | 0.37 |
| **route, planned (typed graph + `lcg-passages`)** | 0.90 | 0.88 | 0.77 | 0.60 | **1.00** | **0.84** |
| route, planned (typed graph + belief-rerank) | 0.90 | 0.88 | 0.77 | 0.60 | 0.92 | 0.83 |

  - **The best chunk search scores 0.30–0.37 on graph-shaped questions.** None of the four passage or qmd configs answers a 3-hop question (0/24).
  - **Routed LCG beats qmd by a wide margin:** +0.51 [+0.41, +0.61] against `qmd` and +0.53 [+0.43, +0.63] against `qmd-rerank`.
  - **qmd's reranker hurts history** (0.08), because it ignores time. LCG's belief ranking keeps history at 0.92–1.00.
  - **Lists favour the rerankers** (0.88–0.89 against the walk's 0.77), so lists are the next place to look for a planner or walk gain.
- **Next:**
  - Planner fixes: vendor→project joins, and checking the type of the entity a hop lands on.
  - Entity splitting (769 → 1,202 nodes).
  - Lists: close the gap to the rerankers (0.77 against 0.88).
  - `lcg-eval --ontology`.
  - A second corpus with fan-out and the CQs v2 can't instantiate (P3, S2, meetings, provenance).

### E12: graph search plus rerank ([#797](https://github.com/liminisapp/liminis-context-graph/issues/797), PR [#801](https://github.com/liminisapp/liminis-context-graph/issues/801) @ `672c6de3`). Rejected, closed 2026-11-20

| config | v2 | v1 | paraphrase (v2) | temporal (v2) |
|---|---|---|---|---|
| `lcg-passages-belief-rerank` | **0.757** | **0.935** | 0.328 | 0.793 |
| `lcg-graph-rerank` | 0.745 | 0.890 | 0.235 | 0.789 |
| `lcg-graph-seeds-rerank` | 0.718 | 0.819 | 0.156 | 0.800 |
| `lcg-graph` (no rerank) | 0.664 | 0.867 | 0.214 | 0.806 |

- **Result:** graph plus rerank against passages plus rerank is −0.012 [−0.040, +0.015] on v2, and −0.045 [−0.109, +0.011] on v1. It met no part of the bar.
- **Learned:**
  - The reranker is the lever: about +0.08 on either first stage.
  - Graph candidates add nothing once the candidates are reranked.
  - Seed linking fails on described (paraphrased) entities.
  - Graph does win on direct questions (0.946) and passage lookups (0.951), which is a possible router signal.
- **Decision:** PR closed unmerged and the branch `fabrik/issue-797` kept. Results are on [#797](https://github.com/liminisapp/liminis-context-graph/issues/797).

### E11: graph search before rerank ([#797](https://github.com/liminisapp/liminis-context-graph/issues/797), PR [#801](https://github.com/liminisapp/liminis-context-graph/issues/801) @ `a3a1e155`). Not distinguishable

- **Result:** `lcg-graph` 0.661 against `lcg-passages-belief` 0.653 on v2: −0.008 [−0.033, +0.018]. On v1 it scored 0.863 against 0.894.
- **Oracle:** 0.795, of which `lcg-graph` captures 83%.
- **Decision:** hold until rerank lands (that became E12).

### E10: mxbai reranker shipped ([#796](https://github.com/liminisapp/liminis-context-graph/issues/796), PR [#800](https://github.com/liminisapp/liminis-context-graph/issues/800) @ `2471ad5d`). Accepted, merged 2026-11-20 as `d3f1477c`

| config | v2 | v1 | temporal (v2) |
|---|---|---|---|
| `lcg-passages` (raw then) | 0.574 | 0.829 | 0.619 |
| `lcg-passages-belief` | 0.653 | 0.894 | 0.745 |
| `lcg-passages-rerank` | 0.707 | 0.938 | 0.777 |
| `lcg-passages-belief-rerank` | **0.759** | 0.935 | 0.800 |

- **Result:**
  - Belief plus rerank against belief: +0.106 [+0.073, +0.142] on v2 and +0.041 [+0.007, +0.082] on v1.
  - Against raw: +0.185 on v2.
- **Merge bar:** met except temporal on v2 (0.800 against the 0.85 target). Accepted, since rerank is opt-in and temporal still improves on the default.
- **Shipped defaults:**
  - mxbai-rerank-large-v1, Core ML fp16, GPU;
  - blend 0.30 / 0.20 / 0.10;
  - 50 candidates, which is also the cap;
  - the superseded factor applies to the whole blended score.

### E9: belief-aware passage ranking as the default ([#806](https://github.com/liminisapp/liminis-context-graph/issues/806), PR [#807](https://github.com/liminisapp/liminis-context-graph/issues/807) @ `723bbb9f`). Accepted, merged 2026-11-19

- **Result:**
  - New default against `lcg-passages-raw`: +0.078 [+0.061, +0.097] on v2 and +0.065 [+0.027, +0.108] on v1.
  - Every category was level or better on both corpora.
  - Latency was unchanged.

### E8: mxbai blend and candidate-count sweep (Core ML in the PR [#800](https://github.com/liminisapp/liminis-context-graph/issues/800) sidecar @ `da2c2078`, v2)

| setting | overall | paraphrase | temporal | multi-hop | passage |
|---|---|---|---|---|---|
| baseline `lcg-passages` | 0.574 | 0.281 | 0.619 | 0.673 | 0.343 |
| cross-encoder only, N = 25 | 0.688 | 0.255 | **0.932** | 0.543 | 0.846 |
| cross-encoder only, N = 50 | 0.692 | 0.201 | 0.904 | 0.534 | **0.954** |
| blend 0.75 / 0.60 / 0.40, N = 50 | 0.649 | 0.283 | 0.742 | 0.678 | 0.613 |
| blend 0.50 / 0.40 / 0.30, N = 50 | 0.703 | 0.291 | 0.746 | **0.689** | 0.923 |
| **blend 0.30 / 0.20 / 0.10, N = 50** | **0.707** | 0.290 | 0.777 | 0.666 | 0.925 |

- **Learned:**
  - A light rank prior wins.
  - Pure cross-encoder ordering wins temporal and passage but loses multi-hop.
  - N = 100 silently ran at 50, the clamp, which is now a warning.
  - The Core ML scores matched the Python reference exactly (0.688 at N = 25).
- **Performance:**
  - On the Neural Engine, mxbai (DeBERTa-v3) is 15× slower than on the GPU (3.8 s against 0.26 s per batch of 8). The model loads in 3–4 minutes.
  - fp16 needed a −1e4 mask fill, otherwise padded pairs score NaN. The converter patches are in [#796](https://github.com/liminisapp/liminis-context-graph/issues/796).

### E7: reranker model probe (full corpus, paraphrase questions, transformers/MPS)

| model | paraphrase top-1 | top-10 | median rank | note |
|---|---|---|---|---|
| BAAI/bge-reranker-base | 4 | 15 | 15 | weak on paraphrase |
| ibm-granite reranker r2 | 5 | 24 | 9.5 | worse on passage |
| **mixedbread mxbai-rerank-large-v1** | **15** | **35** | **2** | chosen (Apache 2.0, DeBERTa-v3, 435M) |

- **Policy:** no Qwen-family models, which excludes mxbai v2 because it is Qwen-based. BGE is the acceptable fallback.

### E6: bge-reranker-base rerank and Foundation Models expansion (PR [#800](https://github.com/liminisapp/liminis-context-graph/issues/800) @ `5d3760fb`, v2)

| config | overall | paraphrase | temporal | passage |
|---|---|---|---|---|
| `lcg-passages` | 0.574 | 0.281 | 0.619 | 0.343 |
| `lcg-passages-rerank` (bge-reranker-base) | 0.618 | 0.064 | 0.899 | 0.752 |
| `lcg-passages-expand` | 0.545 | 0.213 | 0.563 | 0.374 |
| `lcg-passages-expand-rerank` | 0.626 | 0.047 | 0.899 | 0.797 |

- **Learned:**
  - bge-reranker-base wrecks paraphrase, and the first version ignored the first-stage rank. That led to the position-aware blend steer and to E7.
  - Before a positional fallback was added (`94d5ca05`), Apple Foundation Models omitted its `lex:` / `vec:` / `hyde:` labels on 79% of queries, and expansion at that head scored 0.465.
  - Expansion stays opt-in and unproven.

### E5: caller query variants ([#802](https://github.com/liminisapp/liminis-context-graph/issues/802), PR [#805](https://github.com/liminisapp/liminis-context-graph/issues/805)). Shipped opt-in, docs only, merged 2026-11-20

| config | v2 overall | paraphrase | multi-hop | passage |
|---|---|---|---|---|
| `lcg-passages-belief` | 0.653 | 0.328 | **0.757** | 0.361 |
| `lcg-passages-belief-variants` (`87c85525`) | 0.664 | **0.357** | 0.695 | 0.479 |
| `lcg-passages-hybrid-belief` | 0.647 | 0.214 | 0.748 | 0.443 |
| `lcg-passages-hybrid-belief-variants` | **0.665** | 0.296 | 0.689 | **0.554** |

- **History:**
  - First cut, `a0c7e95f`: weights of 2 for the primary and 1 for each variant, with no belief. Variants scored 0.522 against a 0.568 baseline, −0.046 [−0.077, −0.016].
  - Rebalanced: a variant budget of 1.0 against the primary's 2.0, belief applied after fusion, and variants run through the request's own pipeline.
- **Result:**
  - Passage lookups improve: +0.117 for vector and +0.112 for hybrid.
  - Hybrid paraphrase improves by +0.082.
  - Multi-hop drops: −0.062 for vector and −0.059 for hybrid.
- **Cause:** the variants (Sonnet 4.6, written from the question only) drop the named entity, and fusion dilutes notes the primary query already ranked well.
- **Decision:** shipped as opt-in, with guidance to keep named entities. Regenerate the fixture variants with that rule and re-measure together with rerank (open).

### E4: graph search, seed gate ([#797](https://github.com/liminisapp/liminis-context-graph/issues/797), PR [#801](https://github.com/liminisapp/liminis-context-graph/issues/801) @ `d7078ccd` / `e002f128`, v2)

| config | overall | direct | paraphrase | temporal | passage | p50 |
|---|---|---|---|---|---|---|
| `lcg-passages` (no instruction) | 0.568 | 0.756 | 0.150 | 0.628 | 0.477 | 25 ms |
| `lcg-graph-seeds` | 0.599 | 0.875 | **0.000** | 0.795 | 0.605 | 39 ms |
| `lcg-graph-expand` | 0.585 | 0.729 | 0.000 | 0.788 | 0.791 | 130 ms |
| `lcg-graph` | 0.648 | 0.861 | 0.078 | **0.834** | 0.613 | 140 ms |

- **Learned:**
  - The seed gate rejected every described entity, so paraphrase scored 0.
  - With the BGE query instruction (`e002f128`), `lcg-graph` reached 0.653, level with belief.
  - Per-category best configs would give an oracle of 0.842, which led to the composite steer (that became E11 and E12).

### E3: hybrid and belief-aware passages ([#789](https://github.com/liminisapp/liminis-context-graph/issues/789), PR [#798](https://github.com/liminisapp/liminis-context-graph/issues/798), v2). Merged

| config | v2 | v1 | temporal (v2) |
|---|---|---|---|
| `lcg-passages` | 0.568 | 0.789 | 0.628 |
| `lcg-passages-hybrid` | 0.572 | n/a | 0.630 |
| `lcg-passages-belief` | 0.649 | n/a | 0.802 |
| `lcg-passages-hybrid-belief` | 0.652 | n/a | 0.802 |

- **Result:**
  - Belief against raw: +0.081 [+0.063, +0.100].
  - Temporal: +0.174 [+0.114, +0.238].
  - Hybrid on its own is about level. The early hybrid-RRF / bonus variants scored 0.527–0.531 on v2 and lost paraphrase.
- **Decision:** belief as opt-in, which became the default in E9.

### E2: BGE query instruction (embedding profile `bge-v1.5` against `none`, v2)

- **Result:** it doubles paraphrase for vector search (0.150 → 0.281) and lifts the overall score slightly (0.568 → 0.574), but hurts named-entity passage lookups (0.477 → 0.343).
- **On v1:** 0.828–0.829 against 0.789 with the Ollama embedder.
- **Decision:** `bge-v1.5` is the standard profile. The composite steer asked for both embeddings fused.

### E1: baselines on v2 (corpus v2 built 2026-11-18, judged)

| config | overall | direct | paraphrase | temporal | answer | multi-hop | passage |
|---|---|---|---|---|---|---|---|
| `lcg-passages` | 0.568 | 0.756 | 0.150 | 0.628 | 0.789 | 0.662 | 0.477 |
| `lcg-entities-facts` | 0.283 | 0.284 | 0.009 | 0.389 | 0.360 | 0.186 | 0.529 |
| qmd | 0.507 | 0.639 | 0.347 | 0.529 | 0.700 | 0.473 | 0.353 |
| qmd-rerank | 0.476 | 0.623 | 0.475 | 0.335 | 0.573 | 0.430 | 0.422 |

- **Learned:**
  - v1 is too easy to separate systems: qmd scored 0.834 and LCG passages 0.789 with the Ollama embedder.
  - qmd's reranker helps paraphrase but costs temporal, because it ignores time.

## Open threads

1. **Paraphrase recall,** the main remaining gap: 0.328 against qmd-rerank's 0.475. Next step: regenerate the variants so they keep named entities, then feed them into `lcg-passages-belief-rerank`.
2. **Temporal with rerank:** 0.800 on v2 against the 0.85 target. The cross-encoder alone reaches 0.904–0.932 (E8). Consider a belief-aware first stage that is stronger on time, or ordering by belief after rerank.
3. **Router idea:** graph search wins on direct questions and passage lookups (E12). A per-question router could take some of the oracle headroom (0.814).
4. **Notes:** rerank is opt-in, and Notes needs a release, then to opt in. When it sends `queries`, its agent prompt needs the rule to keep named entities.
