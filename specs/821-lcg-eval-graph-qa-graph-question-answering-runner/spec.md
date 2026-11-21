# Feature Specification: lcg-eval graph-qa — graph question-answering runner, `--ontology` ingest and kept workspaces (E13 in the repo)

**Feature Branch**: `fabrik/issue-821`
**Created**: 2026-11-21
**Status**: Specified
**Input**: User description:

> ## Problem
>
> The E13 graph question-answering eval (#815, `docs/retrieval-experiments.md`) runs only as scratch Python outside LCG:
> - the planner, the walk and the typed reader;
> - answer scoring;
> - the dev/test split;
> - `qmd` and passage baselines.
>
> `lcg-eval` can't ingest under an ontology either. The probe used a local patch adding `LCG_EVAL_ONTOLOGY` and `LCG_EVAL_KEEP_WORKSPACE`. So none of E13's numbers can be reproduced from the repo, and the walk tool (#820) has no regression check.
>
> ## Change
>
> 1. **`lcg-eval retrieval --ontology <file>`:** ingest the corpus under that ontology. Record its hash in the report, and fail when a cassette was recorded under a different ontology hash, rather than missing every call. **`--keep-workspace <dir>`** keeps the ingested workspace for later runs, such as `graph-integrity` (#819) and repeated QA runs.
> 2. **A graph question set in the retrieval-v2 fixture,** generated deterministically from the skeleton by `retrieval-gen`, as E13's `gen-graph-questions.py` does: chain-2, chain-3, list, join and history families, with gold answers and a fixed per-family dev/test split (seed 815). Add **inverse-direction questions** ("who reports to X", "which projects does Y own"). v2 has almost none, so the E13 eval can't show whether direction is right (step 5).
> 3. **`lcg-eval graph-qa`** over a kept workspace:
>    - **Runners:**
>      - `passages` (the shipped default);
>      - `passages-belief-rerank`;
>      - `qmd` (optional, as `retrieval` handles it);
>      - `walk` (an LLM planner given the question and `knowledge_get_ontology`, then `knowledge_walk`, #820);
>      - `route` (the planner chooses walk or passages).
>    - **Reader:** one fixed prompt for every runner. The walk context uses `facts_text`.
>    - **Planner and reader models** come from the same provider configuration as the judge. That includes an Anthropic-compatible base URL, so the subscription bridge works. Responses are cached on disk, keyed by the full prompt, as the judge cache is.
>    - **Scoring:** first answer matches for single answers, set F1 for lists and joins, exact in-order match for history. Paired bootstrap between runners (seed 788). Every number is reported per family, overall and per dev/test half.
>
> ## Acceptance
>
> - On the kept v2 ontology workspace, with the recorded E13 plans and reader cache, `graph-qa` reproduces E13 step 4: route v5 0.91 ALL, 0.94 test, within 0.02.
> - A v2 ingest with `--ontology ontology-v3.1.yaml` replays the E13 cassette with zero misses.
> - A unit test covers the scorer (each mode) and the bootstrap. A smoke test runs `graph-qa` on the existing small fixture with a mock planner and reader.
> - Docs (the eval README and `docs/retrieval-experiments.md` standard setup) and a CHANGELOG entry.
>
> ## Out of scope
>
> - New corpora (beyond adding inverse-direction questions to v2's question set).
> - The walk itself (#820) and the integrity report (#819).

## Background

LCG's graph should answer questions that chunk search over single notes cannot: chains of relations, joins and inverse lookups. Experiment E13 (#815, recorded in `docs/retrieval-experiments.md` on the open docs PR #814) measured this on the v2 "Meridian" corpus.

- Chunk search answers almost no graph-shaped questions. `lcg-passages`, `lcg-passages-belief-rerank`, `qmd` and `qmd-rerank` score 0.30–0.37 mean answer accuracy, and 0/24 on 3-hop chains.
- A typed walk over a graph extracted under ontology v3.1 does much better. With an LLM planner and a typed reader, routed between walk and passages ("route v5"), it scores 0.91 on all 129 questions and 0.94 on the held-out test half. That is +0.54 to +0.62 over every chunk-search baseline.
- Step 6 showed that the shipped `knowledge_walk` (#820, merged in PR #825) matches the probe: 0.916 against 0.915.

**None of this can be reproduced from the repo.** The question set, planner, reader, scoring, dev/test split and baselines all exist only as the maintainer's scratch Python. Ontology-constrained ingest and kept workspaces existed only as a local patch (`LCG_EVAL_ONTOLOGY`, `LCG_EVAL_KEEP_WORKSPACE`). As a result:

- `knowledge_walk` has no regression check;
- the graph-integrity report (#819) has no supported way to obtain an ontology-built workspace;
- E13's numbers can't be re-derived when the extractor, ontology or walk changes.

**Direction is also unmeasured.** E13 step 5 found the question set has almost no question whose answer depends on edge direction. The #816 direction repair therefore showed no QA gain, even though it fixed 47 of 48 reversed known-true links. Inverse-direction questions ("who reports to X", "which projects does Y own") are needed to make direction measurable.

**Relevant existing state** (from the codebase and project docs):

- **Extraction eval already has an ontology flag.** `lcg-eval`'s extraction eval has `--ontology <PATH>` and `--ontology-mode open|strict`, defaulting to strict when an ontology is given. `lcg-eval retrieval` has no ontology or keep-workspace option. The new retrieval flags should follow the existing extraction-eval naming and semantics.
- **The judge is Anthropic-only.** `lcg-eval retrieval` has a judge (`--judge-model`, `--judge-cache`) that reads `ANTHROPIC_API_KEY`. Today it has no Anthropic-compatible base-URL option. The committed v2 cassette was recorded through a local Messages API → `claude -p` bridge.
- **`graph-integrity` already reads a kept workspace.** `lcg-eval graph-integrity --workspace <DIR>` (#819) reads an ingested workspace and never modifies it. It is the natural consumer of `--keep-workspace`.
- **Retrieval reports use a paired bootstrap.** Seed 788 and its interval conventions come from ADR-0788. graph-qa should report the same way.
- **#817 invalidated older cassettes.** PR #817 (#816, merged 2026-11-21) changed the edge-extraction prompt. Its own notes say this invalidates recorded cassettes, including `crates/eval/fixtures/retrieval-v2/cassette.jsonl`. The E13 `v2-haiku45-onto31` cassette was recorded before #817, so it is stale on current `main` too (see Open Questions).
- **No eval README exists.** There is no `crates/eval/README.md`. The eval's user-facing docs are `docs/retrieval-quality-evaluation.md` and `crates/eval/fixtures/retrieval-v2/README.md`.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Ingest under an ontology and keep the workspace (Priority: P1)

A maintainer runs `lcg-eval retrieval` on the v2 corpus with `--ontology <file>` and `--keep-workspace <dir>`.

- The corpus is extracted under that ontology.
- The report records the ontology's hash.
- The ingested workspace is left in `<dir>` for later `graph-integrity` and `graph-qa` runs, without re-ingesting.

If the cassette being replayed was recorded under a different ontology, the run stops at once with a clear message. It does not miss every call.

**Why this priority**: Every other story needs an ontology-built workspace. It is also the smallest useful slice: `graph-integrity` (#819) can use it immediately.

**Independent Test**: Run `retrieval` with `--ontology` and `--keep-workspace` on the small v1 fixture with a mock or replay backend. Check three things: the report carries the ontology hash, the kept directory opens as a workspace, and `graph-integrity --workspace <dir>` runs on it.

**Acceptance Scenarios**:

1. **Given** an ontology file and a matching cassette, **When** `retrieval --ontology F --replay-cassette C --keep-workspace D` runs, **Then** extraction uses the ontology, the report records its hash and mode, and `D` holds the ingested workspace after the run exits.
2. **Given** a cassette recorded under ontology hash H1, **When** a run replays it with an ontology of hash H2 (or with no ontology), **Then** the run fails before ingest with an error naming both hashes, and makes no extraction calls.
3. **Given** `--record-cassette` together with `--ontology`, **When** the run completes, **Then** the recorded cassette carries the ontology hash, so a later replay can be checked against it.
4. **Given** `--keep-workspace D` where `D` already exists and is not empty, **When** the run starts, **Then** it refuses with a usage error rather than overwriting.
5. **Given** no `--keep-workspace`, **When** the run completes, **Then** the temporary workspace is cleaned up, as today.
6. **Given** a cassette just recorded with `--ontology` under the current extraction prompts, **When** the same corpus is re-ingested replaying it with the same ontology, **Then** there are zero cassette misses and the resulting graph and report metrics are identical to the recording run.

---

### User Story 2 - A deterministic graph question set in the v2 fixture (Priority: P1)

A maintainer runs `retrieval-gen` and gets a graph question set in the retrieval-v2 fixture alongside the existing questions.

- The set is generated deterministically from the skeleton.
- It covers the chain-2, chain-3, list, join and history shapes, plus a new inverse-direction family.
- Each question has gold answers and a fixed dev/test assignment (seed 815).

`retrieval-gen --check` verifies the committed set.

**Why this priority**: graph-qa has nothing to score without it. It is also the only way to make edge direction measurable.

**Independent Test**: Run `retrieval-gen --check` and confirm the committed graph question file is byte-identical to a fresh generation. Then compare the E13 questions with the maintainer's reference set (`graph-questions-v2.jsonl`, 129 questions) and dev split (`split-dev.json`, 67 dev ids), and confirm they match exactly.

**Acceptance Scenarios**:

1. **Given** the committed skeleton, **When** `retrieval-gen` runs twice, **Then** the graph question set is byte-identical both times.
2. **Given** the generated set, **When** its E13 questions are compared with the reference `graph-questions-v2.jsonl`, **Then** every question id, text, shape, family, gold answer and hop record matches.
3. **Given** the generated set, **When** its dev/test assignment is compared with the reference `split-dev.json`, **Then** the same 67 ids are dev and the other 62 are test.
4. **Given** the generated set, **When** inverse-direction questions are inspected, **Then** each asks for the source side of a relation given its target (e.g. "who reports to X", "which projects does Y own"), and its gold answer comes from the skeleton.
5. **Given** a hand edit to the committed graph question file, **When** `retrieval-gen --check` runs, **Then** it fails and reports the drift.

---

### User Story 3 - Run graph-qa over a kept workspace and compare runners (Priority: P1)

A maintainer runs `lcg-eval graph-qa` over a kept workspace and chooses the runners: `passages`, `passages-belief-rerank`, `qmd` (optional), `walk` and `route`.

- Every runner's retrieved context goes to the same reader prompt.
- Answers are scored per shape.
- The report gives every number per shape, overall and per dev/test half, with paired-bootstrap intervals between runners.

**Why this priority**: This is the regression check for `knowledge_walk` and the reproduction of E13, which is the main point of the issue.

**Independent Test**: Run `graph-qa` on the small existing fixture with a mock planner and a mock reader. Check that every requested runner produces a scored row and that the report has per-shape, overall and dev/test sections with bootstrap intervals. Separately, load the recorded v5 plans file and confirm every walk-routed plan becomes a valid `knowledge_walk` request, with no planner call.

**Acceptance Scenarios**:

1. **Given** an ontology-v3.1 v2 workspace, the recorded E13 v5 plans and the ported reader prompt, **When** `graph-qa --runners route` runs, **Then** route scores 0.91 ± 0.02 on all E13 questions and 0.94 ± 0.02 on their test half. The maintainer verifies this after a full re-extraction (see SC-001).
2. **Given** the `walk` runner, **When** a question is run, **Then** the planner sees only the question and the workspace's `knowledge_get_ontology` output (no family, no gold). It returns a plan with a start entity, typed hops with direction and the answer type. The plan is executed through `knowledge_walk`, and the reader gets the walk's `facts_text`.
3. **Given** the `route` runner, **When** the planner chooses passages for a question, **Then** that question is answered from the `passages` context; otherwise it is answered from the walk.
4. **Given** the recorded v5 plans file, **When** graph-qa runs with it, **Then** no planner call is made for the 129 questions it covers. Its 118 graph-routed plans are executed through `knowledge_walk` and its 11 passages-routed plans through `passages`.
5. **Given** the `qmd` runner and no qmd binary or index, **When** graph-qa runs, **Then** qmd is skipped with a notice, as `retrieval` handles it, and the other runners still report.
6. **Given** two runners, **When** the report is written, **Then** it shows the paired-bootstrap difference (seed 788) with a 95% interval: overall, per shape and per dev/test half.
7. **Given** a second identical run, **When** all planner and reader responses are cached, **Then** it makes zero model calls and produces identical scores.
8. **Given** a workspace that `graph-qa` reads, **When** the run completes, **Then** the workspace (including its WAL) is unchanged.

---

### User Story 4 - Planner, reader and judge through one provider configuration (Priority: P2)

A maintainer without an API key points the planner, reader and judge at an Anthropic-compatible base URL, such as the local subscription bridge. They choose each model through the same configuration surface. Responses are cached on disk keyed by the full prompt, so a repeated run is free and deterministic.

**Why this priority**: Without it, the eval can only run with an API key, and repeated runs cost model calls. The core scoring still works on cached or mock responses without it.

**Independent Test**: Point the provider configuration at a local mock Messages endpoint. Run graph-qa once and confirm requests arrive there. Run it again and confirm zero requests reach the endpoint.

**Acceptance Scenarios**:

1. **Given** a configured Anthropic-compatible base URL, **When** graph-qa or a judged retrieval run makes a model call, **Then** the call goes to that URL.
2. **Given** a cache entry for a prompt, **When** the same prompt is needed again, **Then** the cached response is used and no call is made.
3. **Given** a change to any byte of a prompt or to the model, **When** the run needs that response, **Then** it is a cache miss.

---

### User Story 5 - Documentation of the reproducible setup (Priority: P3)

A maintainer reading the eval docs can reproduce E13. The docs explain:

- how to ingest under an ontology and keep the workspace;
- how to run graph-qa;
- how the runners, reader, scoring and dev/test split work;
- how to reuse plans and caches.

The experiments record's standard setup names this as the way graph-QA results are produced.

**Why this priority**: The work is useful without docs, but other people can only reproduce the results with them.

**Independent Test**: Follow only the documented steps on a clean checkout to ingest v2 under ontology v3.1, keep the workspace and run graph-qa with the recorded v5 plans.

**Acceptance Scenarios**:

1. **Given** the docs, **When** a reader looks up graph-qa, **Then** they find its flags, runners, scoring modes, report layout, plans-file format and the dev/test convention.
2. **Given** the CHANGELOG, **When** the release is cut, **Then** it has an entry for `--ontology`, `--keep-workspace`, the graph question set and `graph-qa`.

---

### Edge Cases

- **Unknown start entity:** the planner names an entity that matches no node. The walk returns no context, the reader answers from nothing, and the question scores 0. The report counts unresolved starts per shape.
- **Malformed plan:** planner output is unparseable or names a relation type that is not in the ontology. The question is scored as answered from empty context and counted as a plan error. It is never silently retried with a different prompt.
- **Plans file doesn't cover a question:** for example, the new inverse questions are absent from the E13 v5 plans. The planner is called for that question if one is configured. If none is configured, the run fails before any reader call and names the uncovered question ids.
- **Plans file names an unknown question id:** the entry is ignored and the report counts it as a warning.
- **Walk returns nothing:** the reader is still called with empty context, so every runner is scored on the same terms.
- **Empty reader answer** ("unknown", "none"): scores 0 for single and list questions.
- **List or join with an empty gold set:** this must not exist in the generated set. The generator rejects it.
- **History question with a partially correct order:** scores 0 (exact in-order match only).
- **Answer-match normalisation:** case, surrounding punctuation and whitespace differences between the reader answer and gold are not counted as wrong.
- **Missing or unreadable workspace,** or one not built under an ontology: graph-qa fails before any model call with a distinct exit code. A free-form workspace is allowed (E13 compares both), and the report records which kind of workspace it was.
- **Cassette with no recorded ontology hash** (recorded before this change), replayed with `--ontology`: the run warns that the hash cannot be verified. It is not treated as a mismatch, so existing cassettes keep working.
- **Model provider fails mid-run:** the run aborts rather than scoring the remaining questions as wrong. Responses already received stay cached, so a rerun resumes cheaply.
- **Dev/test reporting with one runner:** per-half numbers are still reported. Bootstrap differences need two runners.
- **Inverse-direction questions** are scored like their forward counterparts: single-answer for a functional inverse, set F1 for a many-valued inverse.
- **Pre-#816 ontology schema:** ontology v3.1 uses the pre-#816 schema, with no signature lists or inverse aliases. It must load unchanged under the current ontology loader, which #817 kept additive.

## Requirements *(mandatory)*

### Functional Requirements

**Ontology ingest and kept workspaces**

- **FR-001**: `lcg-eval retrieval` MUST accept `--ontology <file>`. The corpus is then ingested with every extraction call constrained by that ontology. An `--ontology-mode open|strict` option MUST be available, defaulting to strict, consistent with the extraction eval's existing flags.
- **FR-002**: The retrieval report (markdown and JSON) MUST record the ontology's hash and mode, or record that none was used.
- **FR-003**: A cassette recorded with an ontology MUST record that ontology's hash.
- **FR-004**: When a replayed cassette records an ontology hash that differs from the run's (including a run with no ontology), the run MUST fail before ingest. The error names both hashes, and no extraction calls are made.
- **FR-005**: When a replayed cassette predates hash recording, a run with `--ontology` MUST warn that the hash cannot be verified and proceed.
- **FR-006**: `lcg-eval retrieval` MUST accept `--keep-workspace <dir>`. The ingested workspace (DB, WAL and ontology) is then left at `<dir>` after the run, in a form `graph-integrity --workspace` and `graph-qa` can open.
- **FR-007**: `--keep-workspace` MUST refuse a non-empty existing directory with a usage error. Without the flag, the workspace is cleaned up as today.

**Graph question set**

- **FR-008**: `retrieval-gen` MUST generate a graph question set for the retrieval-v2 fixture deterministically from the skeleton. It is committed beside the existing question files and verified by `retrieval-gen --check`.
- **FR-009**: The set's E13 questions MUST match the maintainer's reference set `e13-probe/graph-questions-v2.jsonl` (branch `maintainer/e13-probe-files` @ `3e1f17f2`, produced by `gen-graph-questions.py`) in id, text, shape, family, gold answers and hop records. That is 129 questions:
  - by shape: chain-2 48, chain-3 24, list 31, join 14, history 12;
  - across 13 families, such as `supplier-contact`, `owner-manager-location` and `change-history`.
- **FR-010**: The set MUST add an inverse-direction family. Each question asks for the source side of a typed relation given the target, with gold from the skeleton, and covers at least the `REPORTS_TO` and `OWNS` relations.
- **FR-011**: Every question MUST carry:
  - a shape;
  - a family;
  - a scoring mode (single, set or ordered);
  - its gold answer or answers;
  - a dev/test assignment from a fixed split with seed 815.

  On the E13 questions the split MUST reproduce the reference `e13-probe/split-dev.json`: the same 67 dev ids, with the remaining 62 in the test half.
- **FR-012**: Each question SHOULD record its expected relation path, as for the v2 multi-hop questions, so path-level analysis is possible later. Graph-qa scoring does not depend on it.

**graph-qa runners**

- **FR-013**: A new `lcg-eval graph-qa` subcommand MUST run over a kept workspace and the graph question set. It never modifies the workspace.
- **FR-014**: graph-qa MUST support these runners, selectable per run:
  - `passages`: the shipped default passage search, top 10;
  - `passages-belief-rerank`;
  - `qmd`: optional, available and skipped under the same conditions as `retrieval`. The reader gets the whole note for each hit, as in E13 step 3;
  - `walk`;
  - `route`.
- **FR-015**: The `walk` runner MUST:
  1. give an LLM planner only the question and the workspace's `knowledge_get_ontology` output, using the v5 planner prompt and output schema (`PLAN3_SYS` / `PLAN3_SCHEMA` in the reference `answer-eval.py`);
  2. receive a plan of the shape `{route, start, hops: [{types, phrase, direction}], list, history, answer_type}`;
  3. execute the plan through the shipped `knowledge_walk` (#820) with soft direction. Each hop's `types` maps to `relation_types`, and the walk mode is history, list or single according to the plan's flags;
  4. pass the walk's `facts_text` to the reader.

  The probe's own walk (`walk3`) is not ported.
- **FR-016**: The `route` runner MUST let the same planner choose between the walk and `passages` for each question. The chosen route is recorded per question in the report.
- **FR-017**: graph-qa MUST accept a file of previously recorded plans and use it in place of planner calls for the questions it covers. It MUST read the reference `e13-probe/v5-plans.json` format: top-level `model`, `ontology`, `planner_prompt`, `plans` keyed by question id, and `route_planned5_scores`.

**Reader, models and caching**

- **FR-018**: Every runner's context MUST go to the same reader prompt, ported from the reference reader (`READ_SYS` / `READ_SCHEMA` in `answer-eval.py`). The prompt is versioned, and its version is recorded in the report.
- **FR-019**: Planner and reader models MUST be chosen through the same provider configuration as the retrieval judge. That configuration MUST support an Anthropic-compatible base URL, and the judge MUST gain the same ability.
- **FR-020**: Planner and reader responses MUST be cached on disk, keyed by model plus the full prompt, as the judge cache is. A fully cached run makes zero model calls.
- **FR-021**: graph-qa MUST support a mock planner and reader for tests, so that it runs with no network.

**Scoring and reporting**

- **FR-022**: Scoring MUST follow the reference `score()` in `answer-eval.py`:
  - single-answer questions: 1 if the reader's first answer matches gold after normalisation, else 0;
  - lists and joins: set F1 against the gold set;
  - history: exact in-order match, 1 or 0.
- **FR-023**: The report MUST give the mean score for every runner per shape (chain-2, chain-3, list, join, history, inverse), overall, and per dev/test half. Per-family means are included as a secondary table.
- **FR-024**: For every pair of runners, the report MUST give the paired-bootstrap mean difference and 95% interval (seed 788, resample count as in `retrieval`): overall, per shape and per dev/test half.
- **FR-025**: The report MUST separate the E13 questions from the inverse family. It reports an "E13" overall (the 129 questions) as well as an overall that includes inverse questions, so E13 numbers stay comparable.
- **FR-026**: The report MUST include per-question detail:
  - the route taken and the plan, and whether the plan came from the plans file or the planner;
  - whether the start entity resolved;
  - the reader's answer, the gold answer and the score.

  It MUST also record the workspace's ontology hash, the model identities, the prompt versions and the cache hit counts.
- **FR-027**: graph-qa MUST be documented in the eval's user-facing docs (`docs/retrieval-quality-evaluation.md` and the retrieval-v2 fixture README), covering:
  - `--ontology` and `--keep-workspace`;
  - the graph question set and its dev/test convention;
  - runners, reader, scoring, the plans-file format and caching.

  The standard setup in `docs/retrieval-experiments.md` MUST say how graph-QA results are produced, and the CHANGELOG MUST have an entry.
- **FR-028**: Ontology v3.1 (the reference `e13-probe/ontology-v3.1.yaml`) MUST be committed to the retrieval-v2 fixture as the ontology for graph-QA ingests, and the docs MUST name it.

### Key Entities *(if applicable)*

- **Graph question:** id, question text, shape (chain-2, chain-3, list, join, history, inverse), family (one of E13's 13 finer families, or an inverse family), scoring mode, gold answers (ordered for history), dev/test half, and hop records or an expected relation path.
- **Plan:** for one question, `{route, start, hops: [{types, phrase, direction}], list, history, answer_type}`. Route is `graph` or `passages`.
- **Plans file:** recorded plans keyed by question id. It also records the model, ontology and planner prompt they were produced with, and optionally reference scores per question.
- **Runner result:** per question and runner, the context given to the reader, the reader's answer, and the score.
- **Ontology hash:** the hash of the ontology a workspace or cassette was built under, as LCG already computes it.
- **Kept workspace:** an ingested workspace left on disk by `retrieval --keep-workspace`, read by `graph-integrity` and `graph-qa`.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: After #823 and #824 land, the maintainer does one full v2 re-extraction under ontology v3.1, then runs `graph-qa` with the recorded v5 plans. The `route` runner scores 0.91 ± 0.02 on the 129 E13 questions and 0.94 ± 0.02 on their test half, and the numbers are posted on this issue. This is a maintainer verification step, not a merge gate for this issue's PR.
- **SC-002**: A cassette recorded with `--ontology` under the current extraction prompts, then replayed with the same ontology, gives zero cassette misses and identical graph and report metrics. This round trip runs as an automated test on a small fixture.
- **SC-003**: Replaying a cassette under a different ontology hash fails in under 5 seconds, with no extraction calls made.
- **SC-004**: `retrieval-gen --check` passes on the committed graph question set. Its E13 questions match the reference `graph-questions-v2.jsonl` (129 questions) and `split-dev.json` (67 dev / 62 test) exactly.
- **SC-005**: A second identical graph-qa run makes zero model calls and produces byte-identical scores.
- **SC-006**: Unit tests cover each scoring mode (single, set F1, ordered) and the paired bootstrap. A smoke test runs `graph-qa` end to end on the small existing fixture with a mock planner and reader, with no network. Both run in the standard test suite.
- **SC-007**: `graph-integrity --workspace` runs successfully on a workspace produced by `retrieval --keep-workspace`.
- **SC-008**: The recorded v5 plans file loads in graph-qa with no planner call. All 118 graph-routed plans translate to valid `knowledge_walk` requests, and all 11 passages-routed plans route to `passages`. An automated test checks this without a fresh extraction.

## Assumptions

- **#820 has landed.** `knowledge_walk` and `knowledge_get_ontology` (#820, merged in PR #825) are on `main`, and their behaviour is fixed for this issue.
- **The E13 probe files are reference only.** They are on branch `maintainer/e13-probe-files` @ `3e1f17f2` under `e13-probe/`. Implement ports what it needs into `lcg-eval` and the fixture, and never merges that branch.
- **SC-001 is maintainer-verified after this issue.** No new v2 cassette is committed here, because #823 and #824 will change the extraction prompt again within days. Implement and Validate check SC-002 to SC-008 locally.
- **Reader responses are fresh in the reproduction.** The probe's Python reader cache is not ported. SC-001's reproduction makes fresh reader calls through graph-qa's own cache, and its tolerance absorbs reader variance.
- **Default planner and reader model is `claude-haiku-4-5-20251001`,** as in E13, and is overridable through the provider configuration.
- **Ontology v3.1 is the fixture's reference ontology,** because E13's kept numbers were measured on it. Ontology v3.2 (the #816 test ontology, also on the probe branch) is not added here.
- **Reproduction is scored on E13 questions only.** The inverse family is new and is excluded from the E13 overall used for SC-001.
- **The inverse family is small,** sized like history (about 12 questions), and split with the same seed. Its exact size is an Implement choice within FR-010. It has no recorded plans, so a planner (live, cached or mock) handles it.
- **"Family" has two levels.** The issue's "families" (chain-2, chain-3, list, join, history) are the reference set's *shapes*. Each question also has one of 13 finer families. The headline report is per shape.
- **"The eval README" means the eval's user-facing docs.** There is no `crates/eval/README.md`, so this is taken to mean `docs/retrieval-quality-evaluation.md` plus `crates/eval/fixtures/retrieval-v2/README.md`.
- **The experiments record may not be on `main` yet.** `docs/retrieval-experiments.md` is in open PR #814. If #814 has not merged when this is implemented, the standard-setup text goes into `docs/retrieval-quality-evaluation.md`, and a note on #814 asks for it to be carried over.
- **Answer normalisation follows the reference `score()`.**
- **Bootstrap settings match `retrieval`.** The resample count is `retrieval`'s default, and the interval is the 95% percentile interval.
- **Routing is per question.** The planner's route decision is made per question, and `route` uses `passages` (not belief-rerank) as its passage arm, matching route v5.

## Out of Scope *(optional)*

- New corpora (beyond adding inverse-direction questions to v2's question set).
- The walk itself (#820) and the integrity report (#819).
- Re-recording or committing a new v2 cassette (deferred until #823 and #824 land).
- Porting the probe's own walk (`walk3`) or its Python caches.
- Re-tuning the planner or walk for ontology v3.2, or adding v3.2 to the fixture.
- Path-correctness or evidence-nDCG metrics for graph-qa. Recording expected paths (FR-012) only enables them later.
- Fixing extraction issues E13 surfaced: note-ID junk nodes (#824), misattached value facts, and value facts missing from their entity's edges.
- A planner or reader prompt search, or any new walk or planner fixes beyond reproducing route v5.

## Source References *(optional)*

- #815: graph retrieval, prove multi-hop value (E13 parent).
- #820 / PR #825: `knowledge_walk` and `knowledge_get_ontology`. E13 step 6 parity is 0.916 against 0.915.
- #816 / PR #817: edge direction. The prompt change invalidates recorded cassettes.
- #823 and #824: upcoming extraction-prompt changes, which gate the maintainer's SC-001 reproduction.
- #819: `lcg-eval graph-integrity` (reads kept workspaces).
- PR #814: `docs/retrieval-experiments.md` master record (E13 steps 1–6), not yet on `main`.
- Branch `maintainer/e13-probe-files` @ `3e1f17f2`, `e13-probe/` (reference only, do not merge):
  - `ontology-v3.1.yaml` and `ontology-v3.2.yaml`;
  - `gen-graph-questions.py`, `graph-questions-v2.jsonl` and `split-dev.json`;
  - `answer-eval.py` (`PLAN3_SYS` / `PLAN3_SCHEMA`, `READ_SYS` / `READ_SCHEMA`, `score()`);
  - `v5-plans.json`;
  - `llm.py` and `traverse_lib.py`.
- ADR-0788: retrieval corpus v2 and bootstrap reporting (seed 788).
- ADR-0754: retrieval eval architecture.
- ADR-0044: LLM cassette record/replay seam.
- `docs/retrieval-quality-evaluation.md` and `crates/eval/fixtures/retrieval-v2/README.md`: the eval's current user-facing docs.
