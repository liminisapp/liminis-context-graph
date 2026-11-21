# Feature Specification: Summary units as rerank candidates in `knowledge_search_passages` (opt-in; E14 implementation probe)

**Feature Branch**: `fabrik/issue-850`
**Created**: 2026-11-21
**Status**: Specified
**Input**: User description: "## Problem

Passage search is weakest on **paraphrased** questions, where the question describes a note's content in words the note doesn't use. On v2, `lcg-passages-belief-rerank` scores 0.328 nDCG@10 on paraphrase, against 0.475 for qmd with rerank and 0.758 for LCG overall.

E14 (`docs/retrieval-experiments.md`, offline probe, no LCG change) indexed one LLM-written summary per note beside the passages:
- **As rerank candidates,** summaries lifted paraphrase by **+0.129 [+0.029, +0.233]** against the same reranker without them, to 0.526, which is above qmd.
- **Folded into LCG's belief-aware order,** they gave **+0.084 [+0.002, +0.181]** on paraphrase and +0.014 overall. Temporal was unchanged, but multi-hop lost 0.023 because summary hits pushed passages out of the top 10.
- **Plain RRF fusion** lost 0.081 overall. Summaries carry no supersession signal, so used alone they lose 0.22 on temporal.

The probe merged results crudely, outside LCG. **This issue is an implementation probe. A null result is an acceptable outcome**, and in that case it ships off by default or not at all.

## Change

**1. Generate summaries at ingest (opt-in).** When summary units are enabled for a group or workspace (config, default **off**), each ingested document gets one summary unit:
- 3–6 plain sentences plus one key-concepts sentence;
- **no fixed headings** (\"Topics:\", \"Entities:\") in the embedded text, because they inflate similarity between unrelated items;
- one LLM call per document, through the existing extractor provider, with its own `call_type` (`summarize`) so cassettes record and replay it;
- the unit is embedded with the passage embedder.

Research should pick the granularity: per document (as in E14) or per episode for long documents, the storage shape, and how re-ingest or supersession replaces a stale summary.

**2. Retrieval.** In `knowledge_search_passages`, behind a request flag (`summaries: true`, default off) and only when units exist:
- Summary units join the candidate pool **before** the belief-aware rerank. They are not fused afterwards, and they are not ranked by the reranker alone.
- A summary unit inherits its source episode's validity, so a superseded note's summary is demoted exactly like its passages.
- **Results stay passages.** A summary hit is replaced by its source note's best passage for the query, so citations stay exact. The response marks it with `via: \"summary\"`.
- At most `max_summary_slots` (default 3) of the top results may come in via summaries. This protects multi-hop questions, which lost slots in E14.

**3. Eval.** An `lcg-eval` runner config `lcg-passages-belief-rerank-summaries` on v2, and the same paired report by category as E1–E14.

**Docs:** configuration, the IPC/MCP reference for the flag and `via`, an ADR explaining why summaries are rerank candidates and not a fused list (E14), and a CHANGELOG entry.

## Acceptance

- **Off by default.** With summaries disabled, ingest and search are byte-identical to before: no extra LLM call, and the same results.
- **Unit tests:**
  - summary units join the candidate pool only when the flag is set;
  - a superseded note's summary is demoted;
  - a summary hit resolves to a passage of its own note and is marked `via: \"summary\"`;
  - the slot cap holds;
  - summarize calls go through the cassette (record and replay).
- **Eval (maintainer-run, v2).** The bar to make it a recommended setting, against `lcg-passages-belief-rerank`:
  - paraphrase at least +0.05 with the 95% CI excluding 0;
  - overall CI lower bound above −0.01;
  - temporal and multi-hop each no worse than −0.01.

  If the bar isn't met, the outcome is a recorded null result (E15), and the feature stays opt-in or is reverted. The maintainer decides at Validate.
- Read path: still no LLM call at query time (ADR-0820).

## Out of scope

- Cluster or community summary pages (#845).
- Showing summary text to the reader instead of passages.
- Changing the reranker or belief blend."

## Background

Passage search (`knowledge_search_passages`) is weakest on **paraphrased** questions, where the question describes a note's content in words the note itself doesn't use. On the v2 retrieval fixture, the `lcg-passages-belief-rerank` configuration (belief-aware ordering plus the local cross-encoder rerank) scores **0.328 nDCG@10 on paraphrase**, against 0.475 for qmd with rerank, even though LCG scores 0.758 overall.

**E14** (recorded in the retrieval experiments record, `docs/retrieval-experiments.md`; offline probe, no LCG change, 2026-11-21) tested whether one LLM-written summary per note, indexed beside the raw passages, closes that gap. Haiku 4.5 wrote one summary per note (408 calls): 3–6 plain sentences plus one key-concepts sentence and no headings, embedded with the same embedder as passages (BGE). The idea came from a review of WebBrain's retrieval design (#843–#849 are the related suggestions).

| v2 nDCG@10 | overall | direct | paraphrase | temporal | answer | multi-hop | passage |
|---|---|---|---|---|---|---|---|
| (a) passages, belief + rerank (shipped) | 0.758 | 0.910 | 0.328 | 0.800 | 0.935 | 0.740 | 0.913 |
| (b) summaries only, dense | 0.540 | 0.716 | 0.346 | 0.580 | 0.747 | 0.621 | 0.223 |
| (c) RRF(a, b) | 0.677 | 0.855 | 0.366 | 0.707 | 0.878 | 0.721 | 0.568 |
| (d0) a's passages re-scored by the reranker alone (control) | 0.710 | 0.863 | 0.397 | 0.716 | 0.784 | 0.596 | 0.954 |
| (d) reranker over a's passages + top-10 summaries | 0.731 | 0.862 | **0.526** | 0.682 | 0.784 | 0.616 | 0.954 |
| (d2) a's order, summary notes inserted by rerank score | **0.771** | 0.914 | 0.412 | 0.800 | 0.935 | 0.717 | 0.913 |

What E14 found:

- **Plain RRF fusion loses**: −0.081 [−0.109, −0.054] overall and −0.093 on temporal. Summaries carry no supersession signal, so summaries alone lose 0.22 on temporal.
- **As rerank candidates, summaries help paraphrase**: (d) against its own control (d0) is +0.129 [+0.029, +0.233] on paraphrase, to 0.526, above qmd-rerank.
- **Kept inside LCG's belief-aware order (d2)**: +0.084 [+0.002, +0.181] on paraphrase (7 wins, 6 losses of 40, so fragile), +0.014 overall, temporal unchanged. Multi-hop dropped 0.023 [−0.046, −0.003] because summary hits pushed passages out of the top 10.
- Re-scoring with the raw reranker alone (d0) loses about 0.05 overall against the shipped order, so LCG's belief blend is worth keeping.
- The (d2) merge was crude and done outside LCG. The record's own conclusion was that a proper version puts summary units into LCG's candidate pool **before** the belief-aware rerank and maps each citation back to a source passage.
- There is no dev/test split for these questions, so the result is tuned on v2.

**This issue is an implementation probe, and a null result is acceptable.** If the measured gain does not clear the bar (SC-004–SC-006), the feature ships off by default, as it does in every case, and is recorded as a null result (E15), or it is reverted. The maintainer decides at Validate.

Constraints from existing features this must respect:

- **Belief-aware ordering** ([ADR-0806](docs/adr/0806-belief-aware-passage-ranking-default.md)), the default since #806, demotes a passage whose episode is no longer believed below its successor, and a `derived`/`answer` passage below the primary note it cites. Demoted passages are never dropped.
- **Optional rerank/expand stages** ([ADR-0796](docs/adr/0796-passage-rerank-and-query-expansion.md)) are opt-in request flags with workspace defaults. When a requested stage cannot run, the response reports it with an `{applied, reason}` block and never fails. With every new flag omitted, the call is byte-identical to before.
- **No LLM call on the read path** ([ADR-0820](docs/adr/0820-caller-plans-walk-no-lcg-llm.md)). Query-time model use is limited to the existing embedder and the local cross-encoder.
- **LLM cassettes** (`LCG_RECORD_LLM` / `LCG_REPLAY_LLM`) key each extractor call by content and record a `call_type`. A replay miss is an identifiable error, never a silent fall-through to a live provider.
- **ADR numbering** uses the issue number, so this issue's ADR is `0850-<slug>`.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Paraphrased questions find the right note (Priority: P1)

A caller (an agent or the Liminis app) asks `knowledge_search_passages` a question that describes a note's content in different words, and sets `summaries: true`. The note's summary unit matches the paraphrase even though none of its passages do. It enters the candidate pool, survives belief-aware ordering and rerank, and the caller receives that note's best passage for the query, marked `via: "summary"`. The caller can cite it exactly, as it would any other passage.

**Why this priority**: This is the whole point of the feature. Paraphrase is the weakest category, and E14 showed summaries can lift it above qmd.

**Independent Test**: In a workspace with summaries enabled, ingest a note whose summary paraphrases its content. Query with wording that matches the summary but not the passages. With `summaries: true`, a passage from that note is in the results with `via: "summary"`. With the flag omitted, the results are unchanged.

**Acceptance Scenarios**:

1. **Given** a workspace with summary units for its documents, **When** a caller sends `knowledge_search_passages` with `summaries: true`, **Then** summary units are considered as candidates alongside passages before belief-aware ordering and any rerank.
2. **Given** a summary unit that ranks within the returned results, **When** the response is built, **Then** that slot holds a passage from the summary's source note (the note's best passage for the query), carrying the same fields as any other passage plus `via: "summary"`, and no summary text appears in the result.
3. **Given** a summary hit whose source note already has a passage in the results through normal passage retrieval, **When** the response is built, **Then** the note's passage appears only once, at the better of the two positions, and the duplicate slot is filled by the next candidate.
4. **Given** the same request without `summaries` (or `summaries: false`/`null`), **When** it runs, **Then** the response is byte-identical to the response before this feature.

---

### User Story 2 - Superseded notes stay demoted (Priority: P1)

A note has been superseded or revised. Its summary unit still exists, but it must not resurrect the stale note above its successor. A summary unit carries its source episode's belief state, so belief-aware ordering demotes it exactly as it demotes that episode's passages.

**Why this priority**: E14's summaries-only and RRF variants lost up to 0.22 on temporal questions because summaries carried no supersession signal. Without this, the feature regresses temporal answers.

**Independent Test**: Ingest a note, then a superseding version, both with summaries. Query with `summaries: true` and `belief_aware` on. The superseded note's summary-sourced passage ranks below its successor.

**Acceptance Scenarios**:

1. **Given** a superseded note whose summary unit matches the query, **When** the search runs with `summaries: true` and belief-aware ordering on, **Then** the result coming in via that summary ranks below the successor's passage, provided the successor is in the candidate set.
2. **Given** a `derived` or `answer` note that cites a primary note, **When** its summary matches, **Then** the citation-demotion rule applies to it exactly as it does to its passages.
3. **Given** `belief_aware: false`, **When** the search runs with `summaries: true`, **Then** summary units join the pool without belief rules, consistent with how passages are treated.

---

### User Story 3 - Multi-hop answers keep their passage slots (Priority: P2)

A multi-hop question needs several distinct passages in the top results. Summary hits are limited to at most `max_summary_slots` (default 3) of the returned results, so they cannot crowd out the passages multi-hop answers need.

**Why this priority**: In E14, multi-hop lost 0.023 because summary hits pushed passages out of the top 10. The cap is the guard against that regression.

**Independent Test**: Build a fixture where more than three summary units would outrank passages. With `summaries: true`, at most three results carry `via: "summary"`. With `max_summary_slots: 1`, at most one does.

**Acceptance Scenarios**:

1. **Given** more summary hits than `max_summary_slots` above the passage candidates, **When** results are truncated to `num_results`, **Then** no more than `max_summary_slots` results carry `via: "summary"`, and the remaining slots go to the next-best ordinary passages in order.
2. **Given** `max_summary_slots: 0`, **When** the search runs with `summaries: true`, **Then** no result comes in via a summary.
3. **Given** an invalid `max_summary_slots` (non-integer, negative), **When** the effective request has summaries on, **Then** the call returns an invalid-params error consistent with the existing stage parameters.

---

### User Story 4 - Operator opts in to summary generation at ingest (Priority: P2)

An operator enables summary units for a workspace (and optionally for specific groups). From then on, each ingested document gets exactly one summary unit: one LLM call through the configured extractor provider, recorded under its own `summarize` call type, embedded with the passage embedder. With the setting off (the default), ingest is unchanged: no extra LLM call and no extra stored data.

**Why this priority**: Retrieval (Stories 1–3) needs units to exist, but the generation step is plumbing with no user-facing value on its own.

**Independent Test**: With summaries disabled, ingest a document while recording a cassette. The cassette contains no `summarize` entries and the stored graph is unchanged. Enable summaries and ingest again. Exactly one `summarize` entry appears per document, and replaying that cassette reproduces the summary with no live provider.

**Acceptance Scenarios**:

1. **Given** summaries disabled (default), **When** a document is ingested, **Then** no summarize call is made and nothing beyond today's data is stored.
2. **Given** summaries enabled, **When** a document is ingested, **Then** one summary unit is created: 3–6 plain sentences plus one key-concepts sentence, with no fixed headings such as "Topics:" or "Entities:" in the embedded text.
3. **Given** a recorded cassette, **When** the same ingest runs under replay, **Then** the summary comes from the cassette, and a missing entry fails with the existing identifiable cassette-miss error.
4. **Given** a document that is re-ingested with changed content, or superseded, **When** the new version is stored, **Then** retrieval never uses the stale summary as if it described the new content.
5. **Given** the summarize call fails (provider error, timeout, malformed output), **When** the document is ingested, **Then** the document and its passages are still ingested, the failure is visible in the same way other extraction failures are, and the document simply has no summary unit.

---

### User Story 5 - Maintainer measures the probe (Priority: P3)

The maintainer runs the `lcg-eval` retrieval eval on v2 with a new configuration, `lcg-passages-belief-rerank-summaries`, and gets the same paired, per-category report (with 95% CIs) that E1–E14 used, against `lcg-passages-belief-rerank`. They use it to decide whether the feature becomes a recommended setting, stays opt-in, or is reverted.

**Why this priority**: The decision rests on it, but it is maintainer-run and comes after the feature works.

**Independent Test**: The configuration is listed and runs on the v1 smoke fixture without error. Its report includes a paired comparison against `lcg-passages-belief-rerank` for every category.

**Acceptance Scenarios**:

1. **Given** the eval runner, **When** the maintainer selects `lcg-passages-belief-rerank-summaries`, **Then** it runs `knowledge_search_passages` with belief-aware ordering, rerank and `summaries: true` on a workspace whose documents have summary units, and nothing else differs from `lcg-passages-belief-rerank`.
2. **Given** a completed run, **When** the report is produced, **Then** it shows nDCG@10 by category with paired 95% CIs against `lcg-passages-belief-rerank`, in the same format as the earlier experiments.
3. **Given** a recorded summarize cassette, **When** the eval re-runs, **Then** no live LLM calls are needed.

---

### Edge Cases

- **Flag set, no units exist** (summaries never enabled, or the matching groups have none): the search returns the ordinary results. If the response reports stage status the way rerank and expand do, it says the summary stage was not applied and gives a reason. The call never fails.
- **Summaries enabled after documents were already ingested**: earlier documents have no summary units until they are re-ingested (see Assumptions on backfill).
- **The source note's passages all fall below `min_score`, or are filtered out**: the summary hit still resolves to a passage of that note when one exists that passes the request's `group_ids` and `origins` filters. If none exists, the summary hit is dropped and does not count against the slot cap.
- **`group_ids` / `origins` filters**: a summary unit is eligible only if its source episode would be. A summary can never bring in a passage the filters exclude.
- **A summary hit and a direct passage hit for the same note**: the note is returned once (User Story 1, scenario 3). The result is marked `via: "summary"` only if the summary was the reason it holds its slot.
- **Long documents split into several episodes**: which episodes a summary inherits validity from, and which passages it may resolve to, follow the granularity Research picks. A summary must never resolve to a passage outside its own source note.
- **A document deleted or a chunk removed**: its summary unit no longer yields results.
- **Composition with `expand`, `queries` and `hybrid` mode**: summaries must compose with them without errors. They are measured only in the belief-aware rerank configuration.
- **`summaries: true` with `rerank` off**: summary units still join the pool before belief-aware ordering. That combination is not the measured configuration.
- **Non-boolean `summaries`**: an invalid-params error, consistent with `rerank` and `expand`. `null` or an omitted key means "not specified".
- **Empty or degenerate document** (whitespace, very short): no summarize call, or one whose empty output is handled without failing ingest.
- **WAL rebuild / recovery** (`knowledge_rebuild_from_wal`): summary units must not be silently lost and must not silently diverge from what was ingested.
- **Summary units must not leak into other surfaces**: entity/edge extraction, `knowledge_get_episodes`, entity and relationship search, and episode counts in `knowledge_status` do not treat summary units as episodes or passages.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: Summary-unit generation MUST be controlled by configuration and MUST default to **off**. It MUST be possible to enable it for a whole workspace. Enabling it per group is desirable; Research decides whether it is in scope for this probe.
- **FR-002**: With generation off, ingest MUST be byte-identical to current behaviour: no summarize LLM call, no additional stored data, the same WAL contents.
- **FR-003**: With generation on, each ingested document MUST get one summary unit (or, if Research chooses per-episode granularity for long documents, one per episode, documented in the ADR). Its text is 3–6 plain sentences plus one key-concepts sentence, with no fixed headings or labels in the embedded text.
- **FR-004**: Each summary MUST be produced by a single LLM call through the existing extractor provider configuration, recorded with its own `call_type` of `summarize`, so that cassette record and replay cover it. A replay miss MUST raise the existing identifiable cassette-miss error.
- **FR-005**: Each summary unit MUST be embedded with the same embedder used for passages, so it is comparable with passage candidates.
- **FR-006**: A summary unit MUST be linked to its source note, so it can inherit that note's group, origin and belief/validity state and be resolved to that note's passages.
- **FR-007**: When a document is re-ingested with changed content, superseded or revised, retrieval MUST NOT use a stale summary as a description of the current content. A superseded note's summary MUST instead be demoted with that note, per FR-011. Research selects the replacement mechanism.
- **FR-008**: A failed summarize call MUST NOT fail the ingest of the document or its passages. The failure MUST be visible through the existing extraction-failure reporting.
- **FR-009**: `knowledge_search_passages` MUST accept an optional boolean request parameter `summaries` (default off). An omitted or `null` value means "use the default". Any other non-boolean value is an invalid-params error.
- **FR-010**: When `summaries` is effective and summary units exist for the searched groups, summary units MUST join the candidate pool **before** belief-aware ordering and before any rerank stage. They MUST NOT be fused into the result list afterwards, and they MUST NOT be ordered by the reranker alone, bypassing belief-aware ordering.
- **FR-011**: A summary candidate MUST carry its source episode's belief state and origin, so the existing belief-aware rules (supersession demotion and derived-below-cited-source demotion) apply to it exactly as they do to that episode's passages.
- **FR-012**: The returned results MUST remain passages. Every summary candidate that lands in the returned results MUST be replaced by the best passage of its own source note for the query, and that result MUST carry `via: "summary"`. Results that did not come in through a summary MUST NOT carry a `via` field, so the shape of non-summary results is unchanged.
- **FR-013**: A note MUST appear at most once in the results because of FR-012. When a summary-resolved passage is already present, the duplicate is removed and the next candidate fills the slot.
- **FR-014**: `knowledge_search_passages` MUST accept an optional integer `max_summary_slots` (default 3) that caps how many returned results may carry `via: "summary"`. `0` disables summary results. Invalid values follow the validation convention of the existing stage parameters.
- **FR-015**: Summary candidates MUST respect the request's `group_ids` and `origins` filters through their source episode.
- **FR-016**: When `summaries` is requested but cannot apply (no units for the searched groups), the search MUST NOT fail. It returns the ordinary results, and if a stage-status block is added for summaries it MUST follow the `{applied, reason}` convention of `rerank` and `expand`.
- **FR-017**: With `summaries` omitted, `null` or `false`, `knowledge_search_passages` MUST return byte-identical responses to the current behaviour for every existing parameter combination.
- **FR-018**: The read path MUST make no LLM call (ADR-0820). Query-time model use stays limited to the existing embedder and the optional local cross-encoder.
- **FR-019**: Summary units MUST NOT be treated as episodes or passages by any other surface: entity/edge extraction, episode listing, entity and relationship search, and episode counts.
- **FR-020**: Summary units MUST survive, or be faithfully reproduced by, a rebuild from the WAL, without requiring a live LLM call when a cassette or a recorded value is available.
- **FR-021**: The MCP tool registry's schema for `knowledge_search_passages` MUST describe `summaries`, `max_summary_slots` and the `via` result field, and the IPC/MCP reference MUST document them.
- **FR-022**: `lcg-eval` MUST provide a runner configuration `lcg-passages-belief-rerank-summaries`. It is identical to `lcg-passages-belief-rerank` except that summary units are generated at ingest and `summaries: true` is sent. It MUST produce the same paired, per-category report with 95% CIs as E1–E14.
- **FR-023**: Documentation MUST include: the configuration reference for enabling summary units; the IPC/MCP reference for `summaries`, `max_summary_slots` and `via`; an ADR numbered `0850-<slug>` explaining, from E14, why summaries are rerank candidates inside belief-aware ordering rather than a fused list; and a CHANGELOG entry.

### Key Entities *(if applicable)*

- **Summary unit**: One LLM-written description of a source note. It holds plain text and its embedding and is linked to its source note or episode. It inherits the source's group, origin and belief state. It is a retrieval candidate only, never returned to callers and never extracted.
- **Source note**: The ingested document (or episode, depending on granularity) that a summary describes. Its passages are what a summary hit resolves to.
- **Summary-sourced result**: An ordinary passage result that holds its slot because its note's summary matched. It is marked `via: "summary"`.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: With summaries disabled (the default), ingest makes zero `summarize` calls, and every existing `knowledge_search_passages` request returns a byte-identical response, verified by the existing test suite plus a no-flag parity test.
- **SC-002**: Unit tests demonstrate each acceptance check in the issue: summary units join the pool only with the flag set; a superseded note's summary is demoted; a summary hit resolves to a passage of its own note marked `via: "summary"`; the slot cap holds; and summarize calls record to and replay from a cassette.
- **SC-003**: Profiling or tests show that a `knowledge_search_passages` call with `summaries: true` triggers no LLM call.
- **SC-004** *(recommendation bar, maintainer-run on v2)*: Paraphrase nDCG@10 for `lcg-passages-belief-rerank-summaries` is at least +0.05 above `lcg-passages-belief-rerank`, with the paired 95% CI excluding 0.
- **SC-005** *(recommendation bar)*: The lower bound of the overall nDCG@10 paired 95% CI is above −0.01.
- **SC-006** *(recommendation bar)*: Temporal and multi-hop nDCG@10 are each no worse than −0.01 against `lcg-passages-belief-rerank`.
- **SC-007**: The outcome is recorded either way. If SC-004–SC-006 are all met, the feature may be documented as a recommended setting. If not, it is recorded as null result E15, and it stays opt-in or is reverted at the maintainer's decision at Validate.

## Assumptions

- **"Note" means an ingested document.** The default granularity is one summary per document, as in E14. Research may choose per-episode summaries for long documents and must record that choice in the ADR.
- **The summarize prompt follows E14's instruction** (3–6 plain sentences, one key-concepts sentence, no headings). The provider and model are whatever the extractor is configured to use. E14 used Haiku 4.5.
- **No backfill.** Enabling summaries affects documents ingested afterwards. Existing documents get summaries only when they are re-ingested. A backfill command is follow-up work. The eval workspace is ingested fresh with summaries enabled.
- **"Best passage for the query"** means the note's passage with the highest score against the query, in the same ranking the request uses. It may be below `min_score`, since the summary is what qualified the note. Research may refine this.
- **The slot cap applies to the returned `num_results`**, not to the wider internal candidate pool.
- **`max_summary_slots` is a request parameter** with a default of 3. A workspace-level default is optional.
- **Summary candidates enter through the vector retriever.** E14 measured dense summaries only. Whether they also join the lexical list in `hybrid` mode is left to Research, and the default is vector only.
- **A response-level stage block for summaries** (`{applied, reason}`) is a convention-following nicety, not a requirement beyond FR-016.
- **The recorded v2 cassette needs a `summarize` re-record** for the new configuration. The maintainer does this, because it needs a live provider.
- **The E15 null-result record**, if needed, is written by the maintainer at Validate in the retrieval experiments record, not by the implementing stage.

## Out of Scope

- Cluster or community summary pages (#845).
- Showing summary text to callers instead of passages.
- Changing the cross-encoder reranker, its model, or the belief blend weights.
- Backfilling summaries for documents ingested before the feature was enabled.
- Making summaries a default-on setting. That is a maintainer decision after the eval, and it is not part of this issue's implementation.
- Any LLM call at query time.
- Summary units in other read methods (`knowledge_find_entities`, `knowledge_graph_search`, `knowledge_walk`).

## Source References

- E14 record: `docs/retrieval-experiments.md` (on branch `docs/retrieval-experiments-record`, commit `4f12b637`), "E14: LLM note summaries as extra retrieval units".
- Related WebBrain-review suggestions: #843–#849; cluster/community summaries: #845.
- [ADR-0806](docs/adr/0806-belief-aware-passage-ranking-default.md): belief-aware passage ranking default.
- [ADR-0796](docs/adr/0796-passage-rerank-and-query-expansion.md): optional rerank and expansion stages.
- [ADR-0820](docs/adr/0820-caller-plans-walk-no-lcg-llm.md): no LCG LLM call on the read path.
- [ADR-0752](docs/adr/0752-episode-origin-and-corroboration.md): episode origin and citation demotion.
- ADR-0705 / ADR-0707: belief time and document supersession.
- `docs/ipc-mcp-reference.md`: "Hybrid passage search" and "Optional rerank and expansion stages".
- `docs/testing-and-evaluation.md`: record/replay cassettes and `call_type`.
- `docs/retrieval-quality-evaluation.md`: the v2 fixture, paired CIs, and runner configurations.
