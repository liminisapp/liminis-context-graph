# ADR-0850: Summary Units as Rerank Candidates, Inside Belief-Aware Order

**Status**: Accepted (implementation probe; off by default; the maintainer decides at Validate
whether it becomes a recommended setting, stays opt-in, or is reverted)
**Date**: 2026-11-21
**Issue**: #850 (follows the E14 offline probe; related to ADR-0796, ADR-0806, ADR-0820)

## Context

`knowledge_search_passages` is weakest on **paraphrased** questions, which describe a note in words
it does not use. On v2, `lcg-passages-belief-rerank` scores 0.328 nDCG@10 on paraphrase, against
0.475 for qmd with rerank and 0.758 for LCG overall.

E14 (an offline probe recorded in `docs/retrieval-experiments.md` on branch
`docs/retrieval-experiments-record`, commit `4f12b637`; no LCG change) indexed one LLM-written
summary per note (Haiku 4.5; 3–6 plain sentences plus one key-concepts sentence, no headings),
embedded with the passage embedder:

| v2 nDCG@10 | overall | paraphrase | temporal | multi-hop |
|---|---|---|---|---|
| (a) passages, belief + rerank (shipped) | 0.758 | 0.328 | 0.800 | 0.740 |
| (b) summaries only, dense | 0.540 | 0.346 | 0.580 | 0.621 |
| (c) RRF(a, b) | 0.677 | 0.366 | 0.707 | 0.721 |
| (d0) reranker alone over a's passages (control) | 0.710 | 0.397 | 0.716 | 0.596 |
| (d) reranker over a's passages + top-10 summaries | 0.731 | **0.526** | 0.682 | 0.616 |
| (d2) a's order, summary notes inserted by rerank score | **0.771** | 0.412 | 0.800 | 0.717 |

- **Fusing summaries as a separate list loses**: (c) is −0.081 [−0.109, −0.054] overall and −0.093
  on temporal. Summaries carry no supersession signal; alone, (b) loses 0.22 on temporal.
- **As rerank candidates they help paraphrase**: (d) against (d0) is +0.129 [+0.029, +0.233].
- **Inside LCG's belief-aware order** (d2) they give +0.084 [+0.002, +0.181] on paraphrase and
  +0.014 overall, temporal unchanged, but multi-hop −0.023 [−0.046, −0.003], because summary hits
  pushed passages out of the top 10.
- The (d2) merge was done crudely outside LCG. E14's own conclusion: put summary units into the
  candidate pool **before** the belief-aware rerank and map each hit back to a source passage.

## Decision

1. **Summary units are candidates inside the first stage, before belief-aware ranking and rerank —
   not a fused list.** With `summaries: true` the 10 summary units nearest the primary query
   (`summary_units::SUMMARY_CANDIDATES`, E14 (d)'s top 10) are merged into that query's **vector**
   candidates by cosine, under the same `min_score` floor, before `apply_belief_ranking` and before
   RRF in `hybrid` mode. They then pass through the rerank head (the cross-encoder scores the summary
   text, as in E14 (d)) and the belief/recency blend. E14 (c) is why there is no separate list; E14
   (b) is why a summary must carry belief. LLM expansion variants and caller `queries` variants get
   no summaries (one extra probe per call, as ADR-0802 keeps the primary query privileged).
2. **A summary's belief is its episode's.** Every supersession lookup on the read path keys on
   `PassageResult::belief_key()` — the source episode for a summary candidate — in
   `search::belief_facts` / `apply_belief_ranking` and in `staged_search`'s rerank lookup, and the
   derived-below-cited rule reads the **episode's** `[[wikilinks]]`. A revision successor is matched
   on the episode, so both the successor's passage and its summary anchor a stale candidate, and in
   the blend a summary and its episode's passage share replacement relations. Without this a summary
   would be belief-neutral and reproduce E14 (b)'s temporal loss.
3. **Results stay passages.** After ordering, `summary_units::resolve_results` replaces each summary
   candidate with **its own episode's passage** (the text it describes), marked `via: "summary"`,
   keeping the candidate's `score`/`similarity`/`rerank_score`/`belief` so `score` stays in rank
   order. A note appears once, at its better position (`via` only when the summary placed it). At most
   `max_summary_slots` (default 3, a request parameter) results come in this way — the guard against
   E14 (d2)'s multi-hop loss — and the next ordinary passages fill the remaining slots. With summaries
   on, the staged path keeps the full order (it no longer truncates in `assemble`) and draws
   `SUMMARY_CANDIDATES` extra candidates so de-duplication and the cap can backfill. Summary text
   never reaches a response (`summary_of` is `#[serde(skip)]`, and a test checks the wire).
4. **Granularity: one summary per episode.** Chunk-mode ingest (`knowledge_process_chunk`, what the
   app and the eval use) has no "document complete" signal, and a per-document summary would have
   needed a new method, a ToolSpec and a client change. Per episode, resolution is exact (a summary
   resolves to its own passage, never outside its note) and belief, filters and revision are
   inherited. On v2 all 40 paraphrase gold notes are single-chunk, so their summaries are exactly
   E14's input; the 84 multi-chunk notes get per-chunk summaries rather than one summary of up to
   40 KB — a deliberate difference from E14.
5. **Storage: a separate LCG-only `SummaryUnit` table**
   (`uuid, group_id, episode_uuid, content, content_embedding FLOAT[dim], created_at`) with its own
   HNSW index `summary_unit_content_embedding_idx`. Off by default is structural: nothing touches
   `Episodic`, its templates or WAL lines, and no other read can see the table (FR-019). Naming the
   columns `content`/`content_embedding` gives WAL vector stripping and replay recompute for free
   (ADR-0526), so a rebuild reproduces summaries with no LLM call; `knowledge_dump_wal` dumps the
   table in phase 1. `group_id` is denormalised (immutable, filtered beside the probe); origin and
   belief are read through `episode_uuid` (origin can change after ingest). Rows are only created and
   deleted, never `SET` (HNSW). Rejected: columns on `Episodic` (an `ALTER`, a zero-fill migration,
   changed chunk WAL templates and NULL vectors in an HNSW index).
6. **Stale and removed summaries.** A revised or superseded episode's summary stays and is demoted
   with its episode (rule 2). A summary whose episode has ended with no successor (a removed chunk)
   or is gone is excluded from the candidates. A pending document-mode episode's summary behaves
   exactly as its passage. Delete paths (`knowledge_delete_episode`, `_by_source`,
   `_chunk_episode`) and group purge delete the episode's units, probing first so a workspace that
   never had summaries gets no new WAL line.
7. **Generation is opt-in at ingest: `LCG_SUMMARY_UNITS`** (workspace-wide, read per ingest, no
   `AppState` field). `add_episode_with_opts` runs one `Extractor::summarize` call (its own
   cassette `call_type: "summarize"`, keyed on the body and rendered prompts only) concurrently with
   the embed + extract pair, as a third `try_join!` branch that always yields `Ok` with the summary's
   own result inside: a summarize failure cannot fail the ingest, and an embed or extract failure
   drops the in-flight summarize call rather than waiting for it. It embeds the cleaned text as a
   `Document`, and writes the
   unit in Phase C, in the same WAL flush as the episode. Labels and headings ("Topics:",
   "Summary:", `#`) are stripped defensively because they inflate similarity between unrelated items.
8. **Failure.** A summarize failure (provider error, timeout, empty or malformed reply) never fails
   the ingest; the provider emits `ExtractionFailure { call_type: "summarize" }` and the episode has
   no unit. **A cassette miss is the exception and fails the ingest**: it is a harness
   misconfiguration, and swallowing it would let a replay silently measure the baseline under the
   summaries configuration. For the same reason the eval runner refuses to score a response whose
   `summaries` block reports `no_units`.
9. **No LLM call on the read path** (ADR-0820): summaries are found with the query embedding alone.
10. **Response contract** (ADR-0796 conventions): `summaries` and `max_summary_slots` are optional;
    omitted, `null` or `false` leave every response byte-identical. A requested summaries stage adds
    `summaries: {applied, candidates, results?, reason?}`; `no_units`, `disabled`
    (`max_summary_slots: 0`) and `no_candidates` return the ordinary results, never an error.

## Consequences

- The measured configuration is `lcg-passages-belief-rerank-summaries` (belief-aware first stage +
  rerank + summaries), paired in the eval only with `lcg-passages-belief-rerank`. The bar to recommend
  it (spec SC-004–SC-006): paraphrase ≥ +0.05 with the paired interval above zero, overall interval
  lower bound above −0.01, temporal and multi-hop each no worse than −0.01. **A null result is an
  acceptable outcome**; it is then recorded as E15 and the feature stays opt-in or is reverted.
- v2 has no dev/test split, so a pass is optimistic.
- Ingest cost: one extra LLM call per episode (about 1,006 for v2 rather than E14's 408), concurrent
  with extraction. The v2 cassette needs `summarize` records before the summaries config can replay;
  the re-record procedure is in `docs/testing-and-evaluation.md`.
- Up to 10 summaries share the rerank head with passages (they displace passages rather than grow the
  head), so rerank latency is unchanged; each summaries search adds one ANN probe.
- A replay embed failure on a summary row is recorded in the zero-vector record under kind `content`,
  keyed by the summary uuid; a group purge of `SummaryUnit` clears no zero-vector kinds. Minor and
  repairable.

## Known gaps and follow-ups

- **The blend does not apply the derived-below-cited rule** (only the first stage does). That gap
  exists for passages already; a summary of a derived page that the cross-encoder favours widens it.
- With extra query lists (`expand`, `queries`), a note's passage can be fused in directly and outrank
  its own summary; it then holds the slot without `via`. Unmeasured.
- Per-group enabling (an allow-list), a backfill command for notes ingested before the switch, and
  summaries in `hybrid`'s lexical leg are follow-up work.
- Only the belief-aware rerank combination is measured; `hybrid`, `expand`, `queries` and rerank-off
  compose without errors but are unmeasured.
