# Research: WebBrain retrieval suggestions (#843, #847, #848, #849)

Issue [#863](https://github.com/liminisapp/liminis-context-graph/issues/863). Findings and recommendations only; no product code changed. The one measurement is recorded as **E15** in the [experiments master record](../retrieval-experiments.md).

## Summary

| Suggestion | Question | Finding | Recommendation |
|---|---|---|---|
| [#843](https://github.com/liminisapp/liminis-context-graph/issues/843) | Add an any-term (OR) BM25 leg, with per-leg attribution? | The leg **is already any-term** (verified empirically). E3 never tried an all-terms (AND) leg or a "BM25 only when it agrees" gate. Attribution is cheap and half-present. | **Probe** (E16 design below) |
| [#848](https://github.com/liminisapp/liminis-context-graph/issues/848) | Are scores relevance? Is #812's floor safe in hybrid mode? | Hybrid `score` is a rank-only fusion value, not a relevance. The 0.5 floor is **safe** (keeps 139/140 answerable top-1s) but is **not an abstention mechanism**: hybrid still answers 25 of 34 off-topic queries. | **Work issue** (draft spec below) |
| [#849](https://github.com/liminisapp/liminis-context-graph/issues/849) | Centroid-gate merges by embedding similarity? | LCG has no cluster, label or centroid merging. Ingest compares to one fixed anchor embedding. No false merge found in the v2 graphs. | **Decline** |
| [#847](https://github.com/liminisapp/liminis-context-graph/issues/847) | `lcg-eval` report hygiene | Grouping by type is present. Side-by-side is partial, input digests are partial, and `retrieval` has no `compare` at all. | **Work issue** (draft spec below) |

## Revisions read and how this was measured

- **Code and docs:** branch `fabrik/issue-863` at `e9d5177f` = `origin/main` `bebb259964849986d59059c9e4cb32a1f27a1718` plus the #863 spec commit. [`docs/retrieval-experiments.md`](../retrieval-experiments.md) was read at `d27facad` (through E14); PR #814 has merged, so it is on `main`. Citations below are `file:line` on that revision.
- **Service under test:** `liminis-context-graph 0.19.1`, debug build of this branch. Corpus `meridian-notebook-2` (hash `da41ea92487ba4fc`), questions `meridian-questions-3` (hash `c110fc2cb5090cea`). The question labels were human-reviewed (`review.json`: `status: reviewed`, error rate 0, 2026-11-18), so the "provisional labels" caveat in ADR-0789 no longer applies.
- **Embedder:** `coreml-bge-base-en-v1.5` (768-d) in the `LocalInference` sidecar, embedding profile `bge-v1.5`, the standard setup. The sidecar socket was one a separate maintainer session had running; this work only sent embedding requests to it.
- **Ingest for the #848 measurement:** the committed free-form cassette **no longer replays** on this branch. The first chunk misses (`cassette miss … key 91bede58…`); I did not investigate why. Passage search reads only Episodic nodes, so the probe ingested all 408 notes through a stub extractor that extracts nothing ([`crates/eval/scripts/null-extractor.py`](../../crates/eval/scripts/null-extractor.py)). Everything that reads the graph (entities, relationships, walk) was therefore **not** measured on a fresh ingest.
- **Graph evidence for #849:** the recorded E13 graph dumps (free-form: 769 entity nodes, 2,016 edges; ontology v3.1: 1,202 nodes, 2,620 edges), read-only. They are a maintainer's local artifacts, not in the repository.
- **Eval-only instrumentation added** (FR-013; no `crates/core` or `crates/service` change): `lcg-eval retrieval-score-probe` ([`crates/eval/src/retrieval/score_probe.rs`](../../crates/eval/src/retrieval/score_probe.rs), unit-tested), `crates/eval/fixtures/retrieval-v2/offtopic-queries.jsonl` (34 constructed queries), `crates/eval/scripts/name-pair-scan.py`, `crates/eval/scripts/null-extractor.py`.

### Reproducing the measurements

```bash
cargo build -p lcg-eval -p lcg-service
python3 -I crates/eval/scripts/null-extractor.py 18765 &      # stop it when done
target/debug/lcg-eval retrieval-score-probe \
  --embedder-uds <sidecar embed socket> --embedding-profile bge-v1.5 \
  --extractor-http http://127.0.0.1:18765/v1/chat/completions \
  --service-bin $PWD/target/debug/liminis-context-graph \
  --modes vector,hybrid,hybrid-belief,vector@0.5,hybrid@0.5,hybrid-belief@0.5 \
  --fts-check cedar,zzqxvk --out probe-obs.jsonl > probe-summary.json
python3 -I crates/eval/scripts/name-pair-scan.py <graph-dump.json> \
  crates/eval/fixtures/retrieval-v2/skeleton.json <sidecar embed socket>
```

The probe takes about two minutes against a warm sidecar.

---

## #843: an any-term BM25 leg, and per-leg attribution

### Does the lexical leg match any term or all terms?

**Any term.** Code path:

- `sanitize_fts_query` (`crates/core/src/search.rs:360`) splits on non-alphanumerics, keeps at most 64 tokens, and joins them with spaces. It adds no `AND`, `OR` or quoting.
- `fts_search_episodic` (`crates/core/src/db.rs:3371`) runs `CALL QUERY_FTS_INDEX('Episodic','episode_content',$q)` with no `conjunctive` argument.
- The LadybugDB docs say `conjunctive` defaults to false, which reads as any-term. The extension source is a prebuilt bundle, so I checked it **empirically** through the service's raw-Cypher escape hatch on the ingested v2 corpus:

| FTS query | default call (what `fts_search_episodic` sends) | `conjunctive := true` |
|---|---|---|
| `cedar` | 34 matches | 34 |
| `zzqxvk` (nonsense) | 0 | 0 |
| `cedar zzqxvk` | **34** | **0** |

A multi-term query returns every episode containing any one term, exactly as an OR leg would. So the "add an any-term leg" half of #843 describes what `main` already does.

The index covers `content`, `source` and `source_description` (ADR-0789 §3), so a term in a note id also matches.

### Was an OR leg among the E3 (#789) variants?

No variant in E3 or ADR-0789 changed the leg's term semantics. The variants tried were:

| Variant | v2 nDCG@10 | Source |
|---|---|---|
| unweighted RRF | 0.527 | E3, ADR-0789 |
| RRF with a top-rank bonus | 0.531 | E3, ADR-0789 |
| weighted RRF (lexical 0.4), hybrid | 0.572 (vector 0.568) | E3 |
| weighted RRF + belief-aware | 0.652 (vector + belief 0.649) | E3 |

Hybrid is flat against vector. ADR-0789 diagnoses the early losses as OR-leg decoys (on paraphrase questions BM25's top hits are lexical decoys) and names "counting BM25 only when it agrees with a vector hit" as the next experiment (ADR-0789 line 144). That experiment, and an all-terms variant, have not been run. E14 (LLM note summaries) is a paraphrase probe in the same family; it is cross-referenced here and was not re-measured.

New datapoint (E15, v2, floor 0.5, answerable queries): under the floor, hybrid returns 140 lexical-only hits across 30 queries, of which **6 are relevant** (4%). The 1,260 vector-backed hits contain 139 relevant (11%). The lexical leg contributes few useful hits once the vector leg is floored, but this counts hits, not nDCG, so it motivates a probe rather than settling it.

### Per-leg attribution: where, how cheap

- **The data exists at fusion time.** `FusedHit.raw` (`search.rs:106-110`) is `Vec<Option<f64>>`, one entry per input list, `None` when a list missed the item. It is discarded where the hybrid response is built (about `search.rs:715-722`), which keeps only `rrf_score` and the vector cosine.
- **Half of it is already on the wire.** `PassageResult.similarity` (`types.rs:439-444`) is set only where the vector leg matched, so `similarity == null` means lexical-only. Dense-only against both needs one more bit.
- **Cost of the rest:** an additive `legs` field (`dense | lexical | both`) and optionally the raw BM25 value on `PassageResult`, populated in `search_passages_hybrid` and the variants path. `#[serde(skip_serializing_if)]` keeps vector-mode responses byte-identical, as `similarity` already does. It touches `types.rs`, `search.rs`, the `knowledge_search_passages` `ToolSpec` description in `crates/service/src/mcp/tools.rs`, and the Python `service_protocol.py` docs. No existing response contract changes. About a day including tests and docs.
- **In `lcg-eval`:** `map_passages` (`crates/eval/src/retrieval/mapping.rs`) keeps only the note id and text, and reports drop ranked lists (`metrics.rs:135`). Showing attribution in reports therefore needs the #847 side-by-side work as well. Without a service change, an eval can already infer "lexical-only" from `similarity == null`, which is what `retrieval-score-probe` does.
- **Placement of the work:** the attribution field is proposed in the #848 work issue below, where it also serves floor semantics, rather than as a separate #843 item.

### Recommendation: **probe** (E16)

Do not add an OR leg (it exists). Run one experiment on the open questions.

- **Question:** does restricting the lexical leg (all-terms, or "lexical-only hits must be vector-backed") improve v2 retrieval or abstention against the current any-term leg?
- **Arms** (all `mode: hybrid`, `belief_aware: true`, `min_score: 0`, k = 10, standard setup, `--no-grounded`):
  - **A:** the shipped any-term leg (the baseline).
  - **B:** `conjunctive := true` in `fts_search_episodic`.
  - **C:** any-term leg, but lexical-only hits are dropped (the ADR-0789 "agree" gate).
- **How:** a throwaway patch in a probe worktree (the E13 pattern); no product change in this issue. Needs the cassette or a re-extraction only if entity configs are included; passage arms do not (see the null extractor).
- **Data:** v2, all 244 questions (210 scored), plus the E15 off-topic set for abstention.
- **Success:** arm B or C has paired nDCG@10 within noise of A overall (interval includes zero) **and** either a gain on paraphrase whose interval excludes zero, or at least a one-third reduction in off-topic queries that return a hit under the 0.5 floor (E15 baseline: 25 of 34).
- **Failure (decline the gate):** both arms are significantly worse overall, or equal on nDCG and no better on abstention.
- **Prior:** low upside. E3 hybrid is flat, and lexical-only hits are 4% relevant. The probe is cheap enough to close the question with evidence either way.

---

## #848: scores as relevance, and whether #812's floor is safe in hybrid mode

### What each method returns

| Method | `score` is | Presented as relevance? | Source |
|---|---|---|---|
| `knowledge_search_passages`, `mode: vector` | raw cosine, `1 − distance`. With belief-aware ranking a demoted passage's score is lowered to just below its anchor. | Yes, and it is one: a cosine. | `search.rs:762`, `search.rs:733` |
| `knowledge_search_passages`, `mode: hybrid` | weighted RRF value, at most `(0.4 + 1.0)/60 ≈ 0.0233`. `similarity` carries the vector cosine, `null` for lexical-only hits. | **No, and it reads like one.** It is rank-only and saturates when both lists rank an item first. | `search.rs:692`, `types.rs:439` |
| passages with `rerank` | adds `rerank_score`, the cross-encoder logit, unbounded; `score` stays the first-stage value | `rerank_score` is a relevance logit; not normalised | `passage_rerank.rs`, `types.rs:454` |
| passages with `queries` (variants) | cross-query fused RRF; `similarity` is the best cosine | rank-only, as hybrid | `search.rs:840-960` |
| `knowledge_find_entities` / `knowledge_find_relationships` | fused `rrf_score` plus a `search` object with the raw per-leg values (`name_similarity`, `summary_similarity`, `fact_similarity`, BM25 `text_match`) | The fused score is explicitly **not** relevance; per-leg evidence is already exposed. | ADR-0629, `types.rs:471` |
| `knowledge_walk` | per-edge raw cosine of hop phrase against fact embedding, plus `DIRECTION_BONUS = 0.05` in soft mode; no floor | raw similarity; the bonus makes it a ranking value | `walk.rs:37`, `walk.rs:602` |
| "graph search" | **does not exist on `main`**: #797 (E4, E11, E12) was closed unmerged; there is no `knowledge_search_graph` in `crates/service/src/mcp/tools.rs` | n/a | |

So the suggestion's "expose per-leg scores" is already done for entities and relationships (ADR-0629). The gap is passages in hybrid mode.

### Provenance of the defaults

| Default | Value | Calibrated? |
|---|---|---|
| `LCG_SEARCH_MIN_SCORE` / `min_score` (`search_config.rs:18`) | 0.5 | **No.** ADR-0812 says the value is uncalibrated. Its only evidence is the #811 report: relevant 0.73–0.77 against unrelated 0.58–0.62, on one CPU bge-base run. |
| `min_similarity` (entities, relationships) | none, unset means no floor (ADR-0629) | n/a, deliberately no default |
| canonicalise `embedding_threshold` (`canonicalize.rs:31`) | 0.7 | **No** |
| ingest `DEDUP_THRESHOLD` (`episode.rs:267`) | 0.85 | **No**; see #849 |
| lint near-duplicate threshold (`lint/mod.rs:38`) | 0.90 | **No**; lexical, see #849 |

`lcg-eval` pins `min_score: 0.0` per config (`crates/eval/src/retrieval/lcg.rs`) and removes `LCG_SEARCH_MIN_SCORE` from the spawned service. Eval therefore never exercised #812's floor, and its `false_positive` metric on negatives counts "returned anything", not score separation.

### Where #812's floor acts in hybrid mode

It floors only the **vector** list's cosine, before fusion (`search.rs:638-642`, ADR-0812 §5). Lexical-only hits are never floored, and the fused score is never floored. So two separate questions: does the vector cosine separate relevant from irrelevant, and does the BM25 leg leak irrelevant hits past the floor?

### Measurement (E15)

Query sets on v2 (service `0.19.1`, BGE-base v1.5 Core ML, `min_score = 0` unless stated, k = 10):

- **answerable:** 140 questions (direct, paraphrase, answer, passage).
- **hard-negative:** the 34 v2 `negative` questions (real-sounding entities or attributes that do not exist).
- **off-topic:** 34 constructed queries (`offtopic-queries.jsonl`): 16 unrelated-domain, 8 gibberish, 10 common-words-only. Constructed by hand, not sampled; small.

**Top-1 score by set.** Vector cosine (hybrid mode's best vector cosine is within 0.003 of this):

| set | n | min | p10 | median | p90 | max |
|---|---|---|---|---|---|---|
| answerable | 140 | 0.475 | 0.554 | 0.668 | 0.790 | 0.867 |
| hard-negative | 34 | 0.582 | 0.616 | **0.690** | 0.732 | 0.764 |
| off-topic, all | 34 | 0.407 | 0.429 | 0.469 | 0.567 | 0.601 |
| off-topic: unrelated-domain | 16 | | | 0.442 | | 0.515 |
| off-topic: gibberish | 8 | | | 0.469 | | 0.487 |
| off-topic: common-words | 10 | | | 0.544 | | **0.601** |

**Fused hybrid `score`** (top-1), for the same sets: answerable median 0.0230 (p10 0.022, saturating at 1.4/60 = 0.0233), hard-negative median 0.0230, off-topic median 0.0167 (= 1.0/60, a vector-only rank-1).

**Separation of each set from the answerable set** (AUC = probability a random answerable query scores above a random irrelevant one; 0.5 is none):

| statistic | vs off-topic | vs hard-negative |
|---|---|---|
| vector cosine (top-1) | AUC **0.961** | AUC **0.441** (worse than chance) |
| fused hybrid score (top-1) | AUC 0.947 | AUC 0.410 |

**The 0.5 floor, query by query:**

| | vector `@0.5` | hybrid `@0.5` |
|---|---|---|
| answerable queries still returning a top-1 | 139/140 | 140/140 |
| hard-negative queries returning hits | 34/34 | 34/34 |
| off-topic queries returning hits | **13/34** | **25/34** |
| ... of which common-words | 10/10 | 10/10 |
| ... of which unrelated-domain | 3/16 | 15/16 |
| ... of which gibberish | 0/8 | 0/8 |
| lexical-only hits returned for off-topic queries | n/a | 115, all on unrelated-domain queries |

- The floor loses one answerable query in vector mode (`paraphrase-39`, top-1 0.475). In hybrid mode BM25 supplies hits for it, so none is lost.
- **Best single cosine threshold** for answerable against off-topic is 0.548: it keeps 91.4% of answerable queries and admits 11.8% of off-topic ones. Raising the floor above ~0.55 starts costing recall (answerable p10 is 0.554), and **no threshold separates the hard negatives** (they sit above the answerable median).
- Unfloored hybrid returned no lexical-only hit in the top 10 for any of the 208 queries: the vector leg's 3 × k over-draw fills the list, and BM25-only items rank below it. Lexical-only hits appear when the floor thins the vector list.

### Verdict on #812's absolute floor in hybrid mode

- **Safe against false rejection.** On v2 with BGE-base v1.5 the 0.5 floor keeps 99.3% of answerable top-1s, and hybrid's BM25 leg covers the one it loses. It acts on the same cosine distribution as vector mode.
- **Not an abstention mechanism, and weaker in hybrid than in vector.** Hybrid answers 25 of 34 off-topic queries against 13 in vector mode, because unfloored BM25 refills the list for function-word and generic queries. Those lexical-only hits are tagged by `similarity == null` and a fused score of about 0.0067 (`0.4/60`), so a client could discard them, but nothing in the response says so.
- **The margin is thin and embedder-specific.** The lowest answerable top-1 (0.475) is already below the floor, and the answerable 10th percentile (0.554) is only 0.054 above it. 0.5 came from a different embedder run (#811: unrelated 0.58–0.62 there, against 0.41–0.60 here), so do not carry it to another embedder without remeasuring.
- **Do not raise it, and do not floor the fused score.** A floor on the fused hybrid score is meaningless: it is rank-only (ADR-0629), and a 0.5 floor on it would drop every result.
- **Near-domain negatives are not separable by score in any mode.** Whether an absent entity or attribute exists needs a different signal (the graph, or a reader), not a threshold.

Limits: v2 is synthetic and near one-to-one, so separation probably overstates real-note separation; the off-topic set is 34 hand-written queries; one embedder; debug build (scores are unaffected); the rerank leg was not measured.

### Recommendation: **work issue** (draft spec below)

No default changes. The measured gap is that a hybrid passage hit does not say what backed it, and `score` reads as a relevance when it is not.

#### Draft spec: Hybrid passage evidence (per-leg fields, honest `score` docs)

**User Scenarios & Testing**

- **Story 1 (P1): a client can tell what backed a hybrid hit.** A caller of `knowledge_search_passages` with `mode: hybrid` sees, on each passage, which legs retrieved it. *Acceptance:* a passage retrieved only by BM25 reports `legs: "lexical"`; only by the vector leg `"dense"`; by both `"both"`. `similarity` is unchanged.
- **Story 2 (P1): `score` is documented as rank-only in hybrid.** The tool description, `docs/ipc-mcp-reference.md` and ADR-0812's text say a hybrid `score` is a fusion value (at most ≈ 0.0233) and not a relevance, and that `min_score` floors the vector leg's cosine only. *Acceptance:* the three places agree and name `similarity` as the relevance signal.
- **Story 3 (P2): `lcg-eval` records it.** The `retrieval` report shows, per hybrid config, the share of returned hits that are lexical-only (depends on the #847 work for per-question detail).

**Requirements**

- **FR-001:** `PassageResult` gains `legs` (`dense | lexical | both`), set by `search_passages_hybrid` and `search_passages_variants` in hybrid mode, derived from `FusedHit.raw`.
- **FR-002:** The field is omitted in `vector` mode, so a vector-mode response is byte-identical to today's.
- **FR-003:** Optionally, `lexical_score` carries the raw BM25 value when the lexical leg matched, omitted otherwise.
- **FR-004:** No default changes: `LCG_SEARCH_MIN_SCORE`, `LEXICAL_RRF_WEIGHT`, ranking and fusion are untouched.
- **FR-005:** The `knowledge_search_passages` `ToolSpec` description, the IPC reference, `service_protocol.py` docs and an ADR-0812 addendum state the semantics in Story 2.
- **FR-006:** Unit tests cover the three legs values and the vector-mode omission; the `crates/service/src/mcp/tools.rs` registry counts are unchanged (no new method).

**Success Criteria**

- **SC-001:** On the v2 probe, `legs` agrees with the E15 `similarity == null` count of lexical-only hits (115 off-topic, 140 answerable under the 0.5 floor).
- **SC-002:** Every existing parity test (`ipc_parity.rs` Tier 1a/1b/1c) passes unchanged.
- **SC-003:** The vector-mode response is byte-identical on a fixture query.

**Edge cases:** a passage in both lists with a different rank in each; variants mode (a passage retrieved by several queries: report the union of legs); the lexical index unavailable (hybrid falls back to vector-only: every hit is `dense`); `rerank` on (the field survives reranking).

**Assumptions:** additive JSON fields are non-breaking for the Python client (`graphiti_service.py`); the probe in #843 (E16) does not depend on this field but reads it more cleanly.

**Not in this issue:** an abstention threshold, gating lexical-only hits (that is E16), score normalisation.

---

## #849: merge discipline in entity and label resolution

### Inventory: every place LCG merges or resolves by similarity

| Site | Location | Trigger | Rule | Threshold | Provenance |
|---|---|---|---|---|---|
| Ingest entity resolution | `episode.rs:472-522`, `db.rs` (`hybrid_dedup_similar_entity_kind_for_name`, `brute_force_…`) | every extracted entity | Exact case-insensitive name match first. Otherwise cosine of the incoming **name embedding** against each stored entity's single `name_embedding` (HNSW + BM25 top 200, or brute force below the hybrid threshold), same group and same kind. Best candidate that passes the identifier veto. | `DEDUP_THRESHOLD = 0.85` (`episode.rs:267`) | Uncalibrated. ADR-0650 records that no threshold separates `ADR 2018`/`ADR 2019` (0.946) from `PostgreSQL`/`Postgres` (0.934). Backstops: identifier veto (always on) and the optional `LCG_DEDUP_LLM` judge (ADR-0652). |
| Edge-endpoint salvage | `episode.rs:1129-1163` | an edge endpoint missing from the batch's entity list | cosine of the endpoint name against **this batch's** entity name embeddings; best above threshold, with the veto | same 0.85 | same |
| Relation canonicalisation | `canonicalize.rs:462, 581-651` | residual edges the lexical pass could not type | argmax cosine of the edge **fact** embedding against each relation type's description embedding (a typing choice, not a merge) | `DEFAULT_EMBEDDING_THRESHOLD = 0.7`, per-call override | Uncalibrated |
| Lint near-duplicates | `lint/entities.rs:93,177`, `lint/mod.rs:38` | `knowledge_lint` | **Not embedding-based.** Sorted-token blocking plus normalised Levenshtein ratio, within one group and kind. The stored-embedding cosine is attached to each pair as `details.embedding_cosine` (`lint/mod.rs:467-487`), evidence only. | 0.90 | Uncalibrated, lexical |
| `knowledge_merge_entities` | `handlers.rs`, `corrections.rs` | explicit caller request | **Not similarity-driven.** Merges named uuid pairs. LCG produces no similarity-based merge suggestions; the lint pairs are the nearest thing. | n/a | n/a |
| `knowledge_resolve_entity`, cross-group endpoint resolution | `assert.rs:57`, `cross_group.rs` | read / endpoint lookup | Name-based, "with no embedding-similarity" step (`assert.rs:57`). `cross_group.rs` copies `fact_embedding`, merges nothing. | n/a | n/a |

I grepped `crates/core/src` and `crates/service/src` for `cosine`, `DEDUP_THRESHOLD`, `embedding_threshold`, `similar_entity` and `name_embedding`; the rows above are every hit that decides a merge or a resolution.

### Single-linkage or centroid?

**Neither.** Each entity has exactly one `name_embedding`, written when the node is created (`episode.rs:1280-1312`) and never updated: a merge refreshes only `summary` and `summary_embedding` (ADR-0651), and aliases never become members. Ingest therefore compares an incoming name to **one fixed anchor per entity, the first-seen spelling**. There is no cluster and no accumulated embedding, so:

- chaining (A≈B, B≈C, A≉C) cannot accumulate through merges, because B never contributes an embedding;
- it can still over-merge on one-hop similarity to the anchor, which is what the veto and the optional LLM judge guard;
- "label absorption" and cluster-level merging, the WebBrain failure, have no equivalent: LCG has no label-clustering pass. A "centroid gate" would introduce a new structure rather than change a linkage rule.

### Does the v2 graph show false merges or duplicate edges?

Method: `crates/eval/scripts/name-pair-scan.py` over the recorded E13 graph dumps, with node names embedded by the same BGE sidecar the dedup uses (names only, no prefix), threshold 0.85. A direct sample for false merges had not been run before.

| | free-form (769 nodes, 2,016 edges) | ontology v3.1 (1,202 nodes, 2,620 edges) |
|---|---|---|
| skeleton entities (48) resolving to a node by exact name | 48/48 | 48/48 |
| skeleton names with more than one node | 0 | 0 |
| nodes shared by two skeleton names (a merge) | **0** | **0** |
| highest same-kind cosine among the 48 skeleton names | 0.782 (`Juniper Supply`/`Cedar Supply`) | same |
| names carried by more than one node | 0 | 0 |
| distinct node pairs with name cosine ≥ 0.85 | 8 (6 same kind) | 28 (12 same kind) |
| node pairs in [0.80, 0.85) | 301 | 559 |
| exact-duplicate edge groups (same source, target, fact) | 13 (13 extra edges) | 17 (17 extra edges) |

Reading:

- **No false merge recorded.** The 48 skeleton entities, the corpus's confusable "X Core" / "X Supply" families, all resolve to distinct nodes, with a 0.07 margin below the 0.85 line. This agrees with E13 ("all 48 skeleton entities resolve to exactly one node") and extends it: no two skeleton entities share a node. The graph-integrity baseline's misattachments (value facts pinned to an unrelated entity: 2 free-form, 29 under ontology) are an extraction problem, not a merge problem.
- **The risk is visible, though.** Pairs above 0.85 that are genuinely different things exist as separate nodes: `99.5% uptime` / `99.0% uptime service level` (0.915; the identifier veto covers this one), `distance-based schedules` / `time-based schedules` (0.897, same kind, no digit for the veto), `paper calendar` / `digital calendar` (0.869). Why those two were not merged in the dumps I could not attribute: the ingest-time candidate scores and `dedup_paths` counters were not recorded, and the pair may have arrived in one batch (resolution is against the persisted graph). Treat it as an unexplained near miss, not as proof of safety.
- **Duplicate edges are by design.** Ingest does not deduplicate edges (`episode.rs:1872`, "edges are not deduplicated"); each carries its producing episode. They are the #820 ties (E13 step 6: 6 of 118 questions differ only by exact-duplicate edges). They come from the same fact stated in a note and in its digest, not from similarity merging. `corrections.rs:958` dedupes only in the correction path.
- Absence of evidence: this is the 48-entity skeleton plus a name-only pair scan on two graphs of one synthetic corpus. It does not sample facts attached to merged nodes, and no LLM adjudication was done.

### Recommendation: **decline**

Evidence: no cluster, centroid or label merging exists to gate; ingest compares against one fixed anchor, so chaining cannot accumulate; the identifier veto and optional LLM judge already guard the one-hop case; no false merge appears in either v2 graph; duplicate edges are a documented design choice, not a merge artefact; and the lint "near-duplicates" are Levenshtein, not embedding similarity. The honest residual is that 0.85 (and 0.7, 0.9) are uncalibrated and ADR-0650 shows no threshold can separate the confusable pairs; that is already recorded there. **Reopen if** a `dedup_paths` replay or a real-notebook sample shows a false merge among the unexplained near misses above (for example `distance-based schedules` against `time-based schedules`), at which point a calibration probe is the right shape.

---

## #847: `lcg-eval` report hygiene

### What reports already record

`lcg-eval retrieval` (`crates/eval/src/retrieval/report.rs:39`, `RunMeta`): `corpus_version`, `corpus_hash`, `questions_version`, `questions_hash` (the first 16 hex of a length-prefixed sha256, `fixture.rs:243-250`), `k`, `lcg_build` (a version string), qmd version and index, `embedder` (the endpoint or socket path, plus the embedding profile), `extractor` (cassette path or endpoint), `qmd_models`, `judge_model`, `category_counts`, and the bootstrap seed and resamples. Per-config JSON (`to_json`, `report.rs` about 1426-1550) carries a `by_category` block and per-question `recall`, `mrr`, `ndcg`, grounded metrics and so on. Paired bootstrap differences between configs within a run are reported.

`lcg-eval graph-integrity` (`crates/eval/src/graph_integrity/report.rs:33-35`) records `inputs.skeleton_sha256` and `mapping_sha256`; this is where the issue's "skeleton and mapping hashes" live, not in `retrieval`.

### The four proposed capabilities

| Capability | Status | Evidence |
|---|---|---|
| Question grouping by type (lexical against paraphrase) | **Present** | `by_category` in the report and per-category tables; v2's categories (direct, paraphrase, temporal, answer, multi-hop, passage, negative) are the type axis. "Lexical" is `direct`. |
| Dense and fused results side by side | **Partial** | The configs `lcg-passages-raw` (dense), `lcg-passages-hybrid`, `-hybrid-belief` are separate rows with paired differences. **Missing:** per-question ranked lists and scores are not persisted (`QuestionScore.ranked` is "not written to the JSON report", `metrics.rs:135`), and `map_passages` drops `score`/`similarity`. |
| Digests of every input | **Partial** | Present: corpus and questions hashes. **Missing:** the cassette (path only), `query-variants.jsonl` (stamped with the questions hash, no digest of its own), the embedder model (a socket path or URL is recorded, not the model identity or a digest of its files), the service binary (version string only), the ontology, the extraction prompt/request shape, and the env overrides in force. |
| `compare` refusing runs whose inputs differ | **Missing for `retrieval`** | `retrieval/cli.rs` has no `--compare`. The only comparison is paired diffs inside one run. The precedent exists in `graph-integrity`: `--compare <FILE>` and `compare()` return `CompareError::Incompatible` (exit 2) when the skeleton or mapping hash differs (`graph_integrity/report.rs:396-420`). |

One more finding that bears on every comparison: **eval pins `min_score: 0.0`** and strips `LCG_SEARCH_MIN_SCORE`, so retrieval reports say nothing about #812's default, and the committed v2 cassette currently misses on this branch (see "Revisions read"). A digest of the cassette plus the extraction request shape would have flagged that.

### Recommendation: **work issue** (draft spec below)

#### Draft spec: `lcg-eval retrieval` input digests and `--compare`

**User Scenarios & Testing**

- **Story 1 (P1): refuse to compare runs that are not comparable.** A maintainer runs `lcg-eval retrieval --compare <earlier report.json>`. *Acceptance:* if `corpus_hash`, `questions_hash`, or `k` differ, the command exits 2 naming the differing input and prints no deltas. If only a soft input differs (embedder model digest, service build, cassette digest, extractor), it prints the deltas with a banner listing what changed; `--strict` refuses on those too.
- **Story 2 (P1): digests of every input.** Each report records a digest for the corpus, questions, variants file, cassette, embedder model, extractor identity, service binary and effective service environment (a sorted list of `LCG_*` overrides). *Acceptance:* two runs with a different cassette file produce different digests in `report.json`; a run with no cassette records `none`.
- **Story 3 (P2): dense and fused side by side per question.** `--record-ranked` writes per-question ranked lists with `score`, `similarity` and (when the service supplies it, see the #848 draft) `legs` to `ranked.jsonl` beside the report. *Acceptance:* the dense and hybrid configs of one run can be joined by question id to show where fusion changed the top-k.
- **Story 4 (P3): per-type headline.** The Markdown report's first table groups by question category (already in JSON); add `lexical` (direct) against `paraphrase` as a named pair in the summary.

**Requirements**

- **FR-001:** `report.json` gains an `inputs` block (schema version 1) with the digests above, each `sha256` hex (first 16 for display, full in JSON).
- **FR-002:** `lcg-eval retrieval --compare <FILE>` parses an earlier `report.json`, checks `inputs`, and prints per-config and per-category nDCG@k deltas.
- **FR-003:** Hard inputs (`corpus_hash`, `questions_hash`, `k`) mismatch → exit 2. A report without an `inputs` block (older runs) → exit 2 with a message to re-run, never a silent pass.
- **FR-004:** The embedder model identity comes from the service's `knowledge_status` (or the embedder response), not from the socket path.
- **FR-005:** `--record-ranked` is off by default; a private run (`--private`) must not write it.
- **FR-006:** Reuse the `graph_integrity::compare` shape and exit codes; no behaviour change for runs without `--compare`.

**Success Criteria**

- **SC-001:** Comparing two reports from different `questions_hash` exits 2 and prints no table.
- **SC-002:** Changing only the cassette file changes exactly the `cassette` digest.
- **SC-003:** Existing report golden tests pass unchanged apart from the added `inputs` block.
- **SC-004:** The `ranked.jsonl` of a hybrid and a raw config join on question id with no missing ids.

**Edge cases:** an older report with no `inputs`; a run where a config is skipped (qmd absent); `--private`; a config present in only one report; a service that does not report its embedder model (record `unknown`, which counts as a mismatch under `--strict`).

**Assumptions:** `graph_integrity::compare` is the model; the digest format matches `fixture.rs` (`sha_hex`); the sidecar model files may be hashed lazily and cached.

**Not in this issue:** changing any metric, changing what a config sends to the service.
