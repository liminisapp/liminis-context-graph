# ADR-0796: Passage Rerank and Query Expansion — Two Opt-in Stages, a Belief-Aware Blend, and an Honest Evidence Gap

**Status**: Accepted (mechanism). **Rerank is measured** by the maintainer (v2 and the v1 holdout, Core ML
mxbai model); it improves overall nDCG@10 but takes about 1.9 s p50, so SC-004 is not met, and temporal on v2
is 0.800 against the 0.85 target. **Query expansion's verdict is not yet recorded.** Both stages stay off by
default. See [Measurements](#measurements) and [Evidence gap](#evidence-gap).
**Date**: 2026-11-18
**Issue**: #796 (builds on ADR-0705 belief time, ADR-0707 document supersession, ADR-0754/ADR-0788 retrieval eval, ADR-0790 embedder roles, ADR-0006/ADR-0016/ADR-0039/ADR-0040 sidecar transport)

## Context

On the v2 retrieval corpus (#788) `lcg-passages` beats qmd overall (nDCG@10 0.567 vs 0.499, paired
difference +0.068 [+0.017, +0.116]) and is far ahead on **temporal** questions (0.628 vs 0.293 for
`qmd-rerank`) because LCG knows which note is current. It loses on **paraphrase** questions, where the
user describes an entity by role without naming it: 0.150 vs 0.356 for qmd with LLM query expansion and
0.492 for qmd plus a cross-encoder.

qmd closes the gap with two stages LCG lacked: LLM **query expansion** and a **cross-encoder reranker**.
Both are slow (1.7 s and 3.3 s p50 against LCG's 24 ms), and the reranker is blind to which note is
current, which is exactly why it wrecks temporal questions. LCG already holds the signal that avoids
this: the belief state of every episode (ADR-0705).

Research found one factual correction to the issue: the sidecar's `/v1/chat/completions` is **Apple
Foundation Models**, not Qwen. The Qwen models in this repository's tooling are external endpoints.

## Decision

### 1. Two independent, opt-in stages on `knowledge_search_passages`

New optional parameters `rerank: bool`, `expand: bool`, `rerank_candidates: int`. A call that sends
neither stage flag, against a workspace that has not made rerank its default, takes the **original code
path untouched** (one exception: a `rerank` or `expand` key that is present, non-null and not a boolean
is rejected as invalid params on every call, because silently ignoring it could leave a workspace
default in force that the caller meant to override) — the staged path is a separate function (`staged_search::search_passages_staged`), so
byte-identity of the default response (SC-006) is structural, not merely tested. New response fields are
`skip_serializing_if`-gated; the envelope gains a `rerank` / `expansion` block only when that stage was
requested:

```json
{"passages": [...], "count": 10,
 "rerank":    {"applied": true,  "candidates": 25},
 "expansion": {"applied": false, "variants": 0, "reason": "unavailable"}}
```

`reason` is one of `not_configured`, `unavailable`, `no_candidates` (rerank), `no_variants` (expansion).
Reranked passages additionally carry `rerank_score` (the cross-encoder logit) and `belief`
(`current`/`superseded`, omitted when unknown). `score` **stays the first-stage cosine**: it is
documented, clients threshold on it, and only the *order* changes. With `expand: true` that holds for a passage
the original query found (it keeps the score computed against the user's own query). A passage found **only**
by an expansion variant carries that variant's score, and each variant search applies `min_score` to its own
query, so an `expand: true` call can return a passage whose similarity to the user's query alone is below
`min_score`. That is the point of expansion (it surfaces paraphrase matches); a client that needs a strict
floor on the user's query should not use `expand`, or should filter on its own.

Precedence: per-call value, then workspace default (`LCG_RERANK=on|off`), then off. Expansion has **no**
workspace default (FR-008): it can only be requested per call, and is meant for deliberate agent
searches. When both are on, expansion and RRF fusion run first and rerank scores the fused candidates
(FR-008).

Rejected: a new `knowledge_search_passages_v2` method (the MCP registry's counts and the Notes app's
Python parity both pin the method list; additive optional parameters need no coordination).

### 2. Degrade, never fail

An unreachable, slow, absent or misbehaving reranker or LLM returns the previous stage's results and a
`applied: false` report with a reason; only a failure of the original query's own first-stage search is an
error, exactly as before. A rerank request with no reranker configured returns `not_configured` rather
than an error (FR-006, FR-010, SC-007). Non-macOS hosts simply have no sidecar and get the same path.

### 3. Belief and recency blend (answers the research question)

The reranker emits a raw logit. For the top-N head (default N = 50, which is also the cap: larger values are clamped to 50 with a logged warning; configurable per call
and via `LCG_RERANK_CANDIDATES`; results beyond N follow in first-stage order, de-duplicated):

1. **First-stage prior + cross-encoder.** `s = a(r)·p(r) + (1 − a(r))·sigmoid(logit)`, where `r` is the
   candidate's 1-based first-stage rank, `p(r) = (1/r − 1/n) / (1 − 1/n)` is the reciprocal rank normalised
   across the head of `n` (1 at the top, 0 at the end), and `a(r)` is the first stage's weight: **0.30 for
   ranks 1–3, 0.20 for ranks 4–10, 0.10 beyond** (tuned on v2, see "Blend tuning"; the first values tried, 0.75 / 0.60 / 0.40, locked the first-stage top-1 and cost temporal) (`LCG_RERANK_RANK_WEIGHT_TOP|MID|TAIL`, `BlendConfig` fields).
   This was added after the first v2 measurement (head `5d3760fb`): a cross-encoder alone ordered the head by
   `sigmoid(logit)` and discarded the first-stage rank, and bge-reranker-base ranked the right note first for
   only 5 of 40 paraphrase questions on the Core ML sidecar (4 in the transformers probe tabulated below; top 10 for 16 and 15, median rank 15), so paraphrase fell 0.281 → 0.064 even
   when the first stage had the answer at rank 1. Mixing the rank in, most strongly at the top, protects a
   good first-stage ranking; a large logit gap can still promote a candidate from deep in the head, and the
   weights are deliberately light (0.30 / 0.20 / 0.10), so the cross-encoder dominates and the first-stage
   rank mostly breaks near-ties and protects paraphrase and multi-hop. (At the first values tried, 0.75 / 0.60 /
   0.40, no deeper candidate could displace the first-stage top-1.) The weights are step functions of rank, so with *equal* logits
   the order is preserved within a band and for neutral or negative logits, but a high shared logit can lift
   a candidate just past a band edge. `cross_encoder_only()` (all weights 0) restores the pure ordering for
   ablation.
2. **Demotion**: a superseded passage's whole blended `s` (rank prior included, before the recency term) is multiplied by `LCG_RERANK_SUPERSEDED_FACTOR` (default
   0.5). "Superseded" is `!live_at(now) || is_pending()` from the existing `EpisodeSupersession`
   (one batched `get_episode_supersession` over the head; no new signal is invented). A document
   supersession with a *future* effective date is still in force (ADR-0707) — `live_at` already handles it.
   A passage absent from the lookup is **neutral**.
3. **Recency prior** for *implicit* revisions (a plain later note with no supersession record, which
   demotion cannot see): `+ LCG_RERANK_RECENCY_WEIGHT (default 0.05) × r`, where `r` is the rank-normalised
   `valid_at` (falling back to `created_at`) within the head, `0.5` when unknown. Deliberately a
   near-tiebreak: a newer irrelevant note must not beat a clearly better text match.
4. **Hard guarantee** (FR-005): after sorting, if a superseded passage and its replacement are both in the
   head, the replacement is moved above it. Replacements are matched by `superseded_by` (episode kind) or by
   `doc_id` (document kind); pending revisions never count as replacements. This is the part that makes
   "a superseded note must not climb above the current one on text match alone" unconditional rather than
   dependent on tuned weights.
5. Ties break by first-stage rank, then uuid — fully deterministic.

Rejected: *dropping* superseded passages (a user may ask about history; ADR-0705 returns them by default);
*pure tiebreak* (too weak against a large text-match gap — the guarantee in step 4 exists for this);
*demotion alone* (blind to implicit revisions); *recency alone* (hurts non-temporal questions, SC-002).

The constants are env-tunable. **They must be chosen on v2 and the tuning is at risk of
overfitting**: v2 has about 40 temporal and 40 paraphrase questions. The shipped defaults are
principled starting values, not fitted ones; the tuning procedure and its overfitting caveat belong in
the Measurements section below.

### 4. Transport: a small shared client, not a refactor of the embedder

`sidecar_client.rs` is a pooled HTTP/UDS JSON client with the embedder's pool, `PoisonGuard`, bounded
slot-acquire and "retry only on a broken connection, never on a timeout" rules (ADR-0039/ADR-0040). It
deliberately **duplicates** `embedder.rs`'s pool code instead of generalising it, so the shipped embedder
hot path is untouched by this feature. Consolidating the two is a follow-up. The reranker endpoint is
`--reranker-uds|http`, else the embedder's own sidecar (flag, default socket, or a *loopback* `LCG_EMBEDDING_URL`; a non-loopback embedding URL is never reused, so passage text cannot reach a third-party embedding provider unnamed). A malformed stage setting fails startup.

### 5. `/v1/rerank` wire shape

Cohere/Jina-style — the de-facto standard for this route name:
`{model, query, documents:[str], top_n?}` → `{results:[{index, relevance_score}]}`.
`relevance_score` is the **raw logit**, not a probability; LCG applies the sigmoid. Results are best-first;
LCG reads them by `index` and rejects a response missing any document's score (a partially-scored head
cannot be ordered meaningfully).

### 6. Sidecar

`CoreMLRerankActor` mirrors the embedding actor (model load, `SetupCache` compile-and-cache in a
`rerank/` subdirectory so the two compiled models cannot collide, `swift-transformers` tokenizer,
fail-fast schema validation) with a fixed-batch `[B, L]` model that scores documents in groups of `B`.
The route is enabled iff `LOCAL_INFERENCE_RERANK_MODEL` is set — **no new `LOCAL_INFERENCE_MODE` value**,
so `both` and every existing mode behave as before. Not configured → 404; configured but failed to load →
503 (and the sidecar keeps serving embeddings: a rerank load failure must not take the product down).

**Truncation is deterministic and documented**: the query is kept up to half of the token budget left after
special tokens, the document gets the rest and is cut at its **tail** (head-biased — the start of a note is
its best summary). Chunk-and-max was rejected as multiplying latency by the number of windows.

### 7. Expansion LLM

Default: the sidecar's chat route (Apple Foundation Models) — "the model the sidecar already hosts"
(FR-009) — overridable through `--expander-uds|http`, `LCG_EXPANSION_URL`, `LCG_EXPANSION_MODEL`,
`LCG_EXPANSION_API_KEY` (the extractor configuration's precedent). The model emits `lex:`/`vec:`/`hyde:`
lines; each variant is searched separately (the HyDE text is embedded as a `Document`, since it reads like
stored text) and the per-variant lists are fused with the existing `rrf_fuse_scored`, the original query's
list always one input. Apple FM's suitability for query rewriting is **unmeasured** (its adequacy for
extraction was judged poor; rewriting is a much lighter task), so the eval compares it against an explicit
Qwen-class endpoint via `--expander-*`.

**Apple FM often drops the labels.** A first v2 run of `lcg-passages-expand` against this sidecar had 193 of
244 queries refused with `no_variants`, because the model answered without the `lex:`/`vec:`/`hyde:`
prefixes. `parse_variants` therefore falls back to a positional reading when the reply holds no labelled line
at all and at least two non-empty lines: line 1 is the lexical variant, line 2 the semantic one, the rest
(joined with spaces) the hypothetical answer, under the same dedupe and length rules. A partly labelled reply
is never re-read positionally, and a single unlabelled line is not treated as the three-part structure. The
service counts replies and replies without a usable variant (`query_expand::expansion_counts`) and logs every
empty reply with the running rate on stderr, so a model that ignores the format shows up in the service log
rather than silently degrading `expand: true` to the first stage. The rate is **not** added to
`knowledge_status`, to leave that response's shape untouched. Guided generation (a `response_format` /
JSON-schema on the sidecar's `/v1/chat/completions` mapped to Foundation Models' `@Generable`) would make the
structure guaranteed and is the better long-term fix; it is not implemented here and is left as a follow-up.

### 8. Scope: `find_relationships` and `find_entities` — not now (FR-017)

Rerank is **not** applied to `knowledge_find_relationships` / `knowledge_find_entities` in this issue.
Their candidates are short strings (names, summaries, facts) already fused by BM25 and vector RRF, edges
already have belief filtering through `believed_only`, so the belief-aware blend here is passage-specific,
and the eval's `lcg-entities-facts` config maps results to notes through an interleave that would make a
rerank effect unattributable. A follow-up issue should revisit this after the passage result is in.

### 9. Overlap with #789 and #797

#789 (hybrid BM25 + vector) and #797 (graph-aware search) touch the same handler. The new logic lives in new
modules (`staged_search`, `passage_rerank`, `rerank`, `query_expand`, `sidecar_client`) with a thin hook in
`handle_search_passages` and a one-line role parameter in `search.rs`. `search_passages_staged` takes its
first stage as a function argument, so whichever first stage #789 lands is what rerank and expansion sit on.

#789 has since landed (ADR-0789: `mode` `vector`|`hybrid`, `belief_aware`, default `vector`). The staged path
follows it: the original query and the lexical and semantic expansion variants run the call's `mode` and
`belief_aware` first stage, so a hybrid workspace keeps its BM25 leg. A hypothetical-answer (HyDE) variant is
embedded as a `Document`, which only the vector search supports, so it is always searched by vector. `score`
stays whatever the first stage produced (cosine in `vector`, the fused rank score in `hybrid`); only the order
changes. Since #806 belief-aware ranking is the **default first stage** (`belief_aware` defaults to `true`), so the
staged path's first stage is belief-aware unless the call or `LCG_PASSAGE_BELIEF_AWARE` says otherwise. The
`lcg-passages-rerank|expand|expand-rerank` eval configurations still pin `mode: "vector"` and
`belief_aware: false` (a raw cosine first stage, like `lcg-passages-raw`), so they isolate the stage over the
pre-#806 default; `lcg-passages-belief-rerank` sends `belief_aware: true` and is the stage over the shipped
default. All the baselines (`lcg-passages`, `-raw`, `-belief`, `-hybrid*`) send `rerank: false, expand: false`. Measuring a
stage on top of `hybrid` therefore needs extra configurations, which this issue does not add; it is the
"with #789" half of FR-014 and is left to the maintainer's measurement round.

**Belief ordering and `expand` without `rerank`.** `belief_aware` (ADR-0789) orders each first-stage list so a
superseded passage ranks below its successor. Expansion then fuses the original query's list with the variant
lists by rank alone, and a HyDE variant's list has no belief ordering at all, so the fused order can put a
superseded passage above its successor (for example when it ranks first in a variant list that does not hold
the successor). The hard replacement-above-superseded guarantee (FR-005) lives in the rerank blend, so it holds
whenever `rerank` runs, including `expand` + `rerank`. For `expand` **without** `rerank`, `belief_aware`
ordering is not guaranteed after fusion. This is deliberate for now (re-applying the belief rules after fusion
would be new behaviour on a stage that is opt-in, unmeasured and meant for deliberate agent searches); a client
that needs the guarantee should send `rerank: true` with `expand: true`.

## Measurements

> **Status: rerank recorded by the maintainer; expansion's verdict not yet.** The mechanism is implemented
> and unit/integration tested. The acceptance numbers need an Apple-silicon machine with the real model
> assets and a real embedder, which neither the implementing agent nor CI has, so every figure below was
> **reported by the maintainer** from their runs. Rerank's results are in "Belief-aware first stage" below;
> SC-004 (150 ms p50) is **not met** (about 1.9 s), and the 0.85 temporal target on v2 was not reached
> (0.800, accepted as a follow-up). Expansion's per-category verdict (SC-005) is not yet recorded. Both
> stages are off by default.

### Runbook

```sh
# 1. Build the sidecar and prepare each candidate model (not committed; ~hundreds of MB).
cd native/local-inference
RERANK_BATCH=8 ./prepare-rerank-assets.sh        # default model: mixedbread-ai/mxbai-rerank-large-v1
RERANK_MODEL=BAAI/bge-reranker-base RERANK_BATCH=8 ./prepare-rerank-assets.sh   # the alternative
swift build -c release
LOCAL_INFERENCE_RERANK_MODEL=./mxbai-rerank-large-v1-b8-l512.mlpackage \
  LOCAL_INFERENCE_RERANK_TOKENIZER_ID=mixedbread-ai/mxbai-rerank-large-v1 \
  LOCAL_INFERENCE_HF_CACHE=./resources/models/tokenizer .build/release/LocalInference &

# 2. Latency half of quality-per-millisecond (candidates x passage length, at batch/length of this export).
uv run benchmark/bench_rerank.py --label mxbai-rerank-large-v1-b8-l512

# 3. Quality half: the v2 corpus, after #790, with and without #789 where available.
lcg-eval retrieval --corpus crates/eval/fixtures/retrieval-v2 \
  --configs lcg-passages,lcg-passages-rerank,lcg-passages-expand,lcg-passages-expand-rerank,qmd-rerank \
  --embedder-uds /tmp/liminis-inference.sock --out <dir>
# Compare Apple FM and a Qwen endpoint for expansion: add --expander-http <url> and re-run.
```

The report's "Passage stages" section prints per-category paired intervals against the matching baseline (`lcg-passages-raw` for the raw-first-stage stages and `lcg-passages-belief` for `lcg-passages-belief-rerank`, when those ran; otherwise `lcg-passages`), expansion
on top of rerank, and the four rerank acceptance checks. A stage that did not apply is a *query error* in the
eval, never silently scored as first-stage results.

### Quality per millisecond, candidate rerankers at 512 tokens (partly filled)

| model | export (batch × length) | p50 ms for N=25 | paraphrase nDCG@10 [paired CI vs `lcg-passages`] | overall Δ | temporal Δ [CI] | meets 150 ms p50? |
|---|---|---|---|---|---|---|
| mxbai-rerank-large-v1 (default) | | | | | | |
| bge-reranker-base | | | | | | |
| bge-reranker-v2-m3 | | | | | | |
| Qwen3-Reranker-0.6B | _export path not implemented here (decoder-only yes/no scorer)_ | | | | | |

Decision rule: pick the highest-quality model that meets the 150 ms p50 at k = 10, else reduce N and sequence
length; if no quality-meaningful setting fits, record that and leave the stage opt-in with its measured latency.
`mixedbread-ai/mxbai-rerank-large-v1` (Apache-2.0, DeBERTa-v3-large) is the default and `bge-reranker-base` (MIT)
the alternative, both with a pinned revision in `convert-rerank-model.py`; `bge-reranker-v2-m3` uses the same
converter. Qwen-family rerankers (Qwen3-Reranker-0.6B, `mxbai-rerank-*-v2`) are ruled out by the maintainer and
have no export path. The Neural-Engine latency of 25 sequential or batched
512-token pairs is the principal risk to the budget and is unmeasured.

**Maintainer-reported comparison (not reproduced by the author of this change; Python `transformers` on MPS, not
Core ML).** Rank of the right note when each model scores all 408 v2 notes per question, with no first stage:

| model | paraphrase top-1 / top-10 / median rank | temporal top-1 / median | passage top-10 / median |
|---|---|---|---|
| `BAAI/bge-reranker-base` | 4 / 15 / 15 | 13 / 3 | 11 / 11.5 |
| `ibm-granite/granite-embedding-reranker-english-r2` | 5 / 24 / 9.5 | 9 / 3 | 7 / 17.5 |
| `mixedbread-ai/mxbai-rerank-large-v1` | 15 / 35 / 2 | 16 / 2 | 11 / 12.5 |

End to end on v2 (head `5d3760fb`, rerank only, mxbai served from a Python server with the same wire shape,
blend before the rank-prior change): overall 0.688 against 0.618 for bge-reranker-base and 0.574 for
`lcg-passages` (overall +0.114 [+0.069, +0.161] over `lcg-passages`); temporal 0.932; paraphrase 0.255 and
multi-hop 0.546, both still below the first stage; p50 587 ms. That is why mxbai-rerank-large-v1 is the default.
The remaining paraphrase loss was first attributed to the right note being absent from the 25-candidate head;
the later sweep (see "Blend tuning") moved the default to N = 50, the cap. Rerank-head recall at N = 40, 80 and
120 was not measured separately.

### Core ML export of mxbai-rerank-large-v1 (maintainer, head `da2c2078`)

`mixedbread-ai/mxbai-rerank-large-v1` (pinned `98f65584`) was converted at fp16, batch 8, length 512; on the 8
real pairs of `paraphrase-03` the sidecar's scores matched transformers within 0.037. It needed four export fixes
(DeBERTa-v2 `torch.jit.script` helpers, integer `sqrt`, fp16 mask NaN, and verifying with `CPU_AND_GPU`), a
tokenizer relabel for swift-transformers, and the GPU rather than the Neural Engine (about 3.8 s vs 0.26 s per
batch of 8). All are now in `convert-rerank-model.py`, `prepare-rerank-assets.sh` and the sidecar
(`LOCAL_INFERENCE_RERANK_COMPUTE_UNITS`, defaulting to the GPU for non-bge models); they were applied from the
maintainer's local patch and have not been re-run end to end in CI. Cold-start load takes 214–274 s whatever the
compute unit, so the model loads in the background and `/v1/rerank` answers a distinct "loading" 503 meanwhile; a
warm-start time with the compiled-model cache is not yet measured. End-to-end p50 with this model is 1.1 s at
N = 25 and 1.9 s at N = 50, so SC-004 (150 ms) is not met by this model on this hardware.

The default rerank timeout (`LCG_RERANK_TIMEOUT_MS`) is 10 s, up from 3 s, because the recorded Core ML p50 is 1.9 s at the default N = 50. p95 and behaviour under 2–3 simultaneous clients are **not measured** (`benchmark/bench_rerank.py` would give them); the sidecar scores serially and keeps computing after a client times out.

### Belief-aware first stage (maintainer, head `2471ad5d`)

With the mxbai Core ML model on GPU, default blend and N = 50 (BGE base v1.5, cassette replay, k = 10; nDCG@10):

| v2 | overall | paraphrase | temporal | multi-hop | passage | p50 |
|---|---|---|---|---|---|---|
| `lcg-passages-raw` (the pre-#806 `lcg-passages`) | 0.574 | 0.281 | 0.619 | 0.673 | 0.343 | 25 ms |
| `lcg-passages-belief` (the default since #806) | 0.653 | 0.328 | 0.745 | 0.758 | 0.361 | 26 ms |
| `lcg-passages-rerank` | 0.707 | 0.290 | 0.777 | 0.666 | 0.925 | 1.9 s |
| **`lcg-passages-belief-rerank`** | **0.759** | 0.328 | 0.800 | 0.740 | 0.913 | 1.9 s |

Against `lcg-passages-belief`, `lcg-passages-belief-rerank` is +0.106 [+0.073, +0.142] overall on v2. On the v1
holdout it is 0.935 against 0.894 (+0.041 [+0.007, +0.082]), with temporal 0.900. The maintainer accepts temporal
on v2 (0.800, against the 0.85 target) because rerank is opt-in and still beats the default there; closing that
gap is a follow-up. Per category, paraphrase is unchanged from the belief-aware first stage and temporal and
passage improve; the 1.9 s p50 still misses SC-004 (150 ms).

### Blend tuning

Record the `LCG_RERANK_SUPERSEDED_FACTOR` / `LCG_RERANK_RECENCY_WEIGHT` / `LCG_RERANK_RANK_WEIGHT_*`
values and the head size `N` tried, the categories evaluated, and that they were chosen on v2 (~40
questions per category) — i.e. at risk of overfitting. v1 is the holdout for the final settings.

| Settings (top / mid / tail, N) | Evaluated on | overall | paraphrase | temporal | Notes |
|---|---|---|---|---|---|
| cross-encoder only (0 / 0 / 0, N=25), head `5d3760fb` | v2 | 0.618 | 0.064 | 0.899 | measured by the maintainer; the baseline that motivated the position-aware blend |
Maintainer sweep on v2 (Core ML mxbai-rerank-large-v1 fp16, batch 8, length 512, pinned `98f65584`; BGE base v1.5 with the `bge-v1.5` instruction, cassette replay, `--no-grounded`; raw cosine first stage, i.e. what was `lcg-passages` before #806, now `lcg-passages-raw`: overall 0.574, paraphrase 0.281, temporal 0.619). The Core ML numbers reproduce the earlier Python measurement (cross-encoder only, N = 25: 0.688). Head `da2c2078` plus local changes:

| Settings (top / mid / tail, N) | overall | direct | paraphrase | temporal | answer | multi-hop | passage | stale>current | p50 |
|---|---|---|---|---|---|---|---|---|---|
| cross-encoder only, N = 25 | 0.688 | 0.797 | 0.255 | 0.932 | 0.780 | 0.543 | 0.846 | 0.100 | 1.1 s |
| cross-encoder only, N = 50 | 0.692 | 0.829 | 0.201 | 0.904 | 0.777 | 0.534 | 0.954 | 0.175 | 1.9 s |
| 0.75 / 0.60 / 0.40, N = 50 | 0.649 | 0.811 | 0.283 | 0.742 | 0.807 | 0.678 | 0.613 | 0.475 | 1.9 s |
| 0.50 / 0.40 / 0.30, N = 50 | 0.703 | 0.834 | 0.291 | 0.746 | 0.812 | 0.689 | 0.923 | 0.450 | 1.9 s |
| **0.30 / 0.20 / 0.10, N = 50** | **0.707** | 0.848 | 0.290 | 0.777 | 0.807 | 0.666 | 0.925 | 0.375 | 1.9 s |

`stale>current` counts temporal questions where a superseded note is ranked above its current replacement (lower is better); the un-reranked raw baseline (`lcg-passages-raw`, then `lcg-passages`) measured 0.725 in the same run (raw first stage, before #806), so every row above is an improvement on it, and 0.375 at the shipped defaults is worse than cross-encoder only (0.100) but still well under the baseline. The hard replacement-above-superseded rule only covers a replacement that is inside the reranked head; a replacement outside the top N, or an implicit revision with no supersession record (recency prior only), is not covered.

The light blend is the best overall and keeps paraphrase and multi-hop at the first stage's level; temporal is the remaining cost (0.904 → 0.777 against cross-encoder only). N = 25 → 50 helps passage lookups and hurts paraphrase and temporal. An attempt at N = 100 silently ran at 50 (the cap), so N beyond 50 was not measured. **Defaults chosen: 0.30 / 0.20 / 0.10 and N = 50, tuned on v2 only (~40 questions per category, so at risk of overfitting); the v1 holdout was then measured (0.935 against 0.894 for `lcg-passages-belief`, +0.041 [+0.007, +0.082]; see "Belief-aware first stage").** The sweep above was run before the superseded factor was applied to the whole blended score; the later "Belief-aware first stage" results include that change.

### Expansion verdict (not yet recorded)

Preliminary, maintainer-reported (v2, head `5d3760fb`, raw first stage, bge-reranker-base for the rerank rows, BGE base v1.5, nDCG@10; no
paired intervals recorded): `lcg-passages-expand` scored 0.545 overall against 0.574 for the raw baseline (paraphrase 0.213 vs
0.281, temporal 0.563 vs 0.619, answer 0.805 vs 0.779, passage 0.374 vs 0.343) at about 1.2 s p50, and
`lcg-passages-expand-rerank` 0.626 overall at about 1.9 s. Expansion alone was −0.029 overall; its gain on top of rerank
with the current mxbai model is not measured. No verdict is drawn here.

Per-category gain on its own and on top of rerank. Ship expansion as *recommended* only if it improves some
category with a paired interval above zero in a way that justifies its latency; otherwise record that no
category justified it.

## Evidence gap

CI cannot produce real-hardware acceptance evidence, so Validate can only rely on (a) the unit and
mock-server tests, which cover ordering, the belief guarantee, degradation, precedence, byte-identity of the
default call and transport pooling on Linux CI, and (b) numbers a maintainer records above. The rerank numbers
are recorded (maintainer-reported); expansion's verdict is not, and the documentation says so
(`docs/configuration.md`, `docs/retrieval-quality-evaluation.md`).

**Endpoints are resolved once, at startup.** The reranker and expander endpoints (and whether the default
sidecar socket exists) are read when the service installs the stages. A service started before the sidecar
(for example a standalone MCP process that begins degraded) has no reranker or expander for its lifetime:
opted-in calls report `not_configured` until it is restarted with the sidecar running, or given an explicit
`--reranker-*` / `--expander-*` endpoint. Re-resolving at call time is a possible follow-up.

## Consequences

- Clients that do not opt in see no change in results, ordering, response shape or latency.
- `AppState` gains one optional field (`search_stages`, empty by default); constructing it elsewhere needs
  `search_stages: Default::default()`.
- The sidecar's `both` mode and every existing route are unchanged; a second CoreML model is resident only
  when `LOCAL_INFERENCE_RERANK_MODEL` is set (memory and first-launch compile cost are paid only then).
- `sidecar_client.rs` duplicates the embedder's pool; consolidating is a follow-up.
- The Liminis Notes app enabling rerank, and shipping the reranker model asset, are follow-ups after an LCG
  release.
