# Feature Specification: lcg-eval retrieval quality eval on a notebook corpus, with qmd as the external baseline

**Feature Branch**: `fabrik/issue-754`
**Created**: 2026-11-15
**Status**: Specified
**Input**: User description: "## Problem

lcg-eval measures **extraction** quality: blind pairwise judging, failure taxonomy, the ontology matrix. Nothing measures **retrieval** quality: whether `knowledge_find_entities`, `knowledge_find_relationships` and `knowledge_search_passages` return the right things for a real question. We have no number for how LCG's hybrid search (FTS + vectors, RRF) compares with the alternatives.

The obvious alternative for a Liminis Notes user is [qmd](https://github.com/tobi/qmd) (tobi/qmd, MIT, ~31k stars). It is local markdown search, and it is the retrieval half of the \"LLM wiki over Obsidian\" setups Liminis Notes competes with (e.g. claude-second-brain). It is also strong: SQLite FTS5 + sqlite-vec over ~900-token chunks, LLM query expansion into lexical and vector sub-queries, RRF (k=60) with a top-rank bonus, an LLM reranker, a blend that weights retrieval vs reranker by position, and an `--explain` score trace.

#734's acceptance criteria also call for comparison against \"raw top-k retrieval\". qmd is the strongest local version of that baseline.

## Desired capability

1. **A retrieval eval mode in lcg-eval** over a real notebook corpus (`demo-notebook`, or a fixture derived from it). It takes a question set with judged-relevant notes/passages, and reports recall@k, MRR/nDCG, latency and model calls.
2. **qmd as an external baseline**, run over the same corpus and question set. Install it out of band; it is not a dependency of lcg.
3. **Comparable configurations:** LCG passages, LCG entities+facts, qmd with and without its reranker. Use the same k for all.
4. Answer-producing questions should compare *grounded evidence*, not just document hits. Whether the right note was found is the minimum.
5. Questions must include **temporal and superseded content** (a decision that was later replaced), where a graph with belief time should beat plain text search, and **paraphrase** queries, where qmd's query expansion should help.

## Acceptance (request level)

- A reproducible run produces a side-by-side table for LCG vs qmd on one corpus.
- The report shows where each one wins, per question category.
- The question set and judgements are checked in and reusable for future retrieval changes.

## Why now

The 2026-11-15 agent-memory survey suggested several qmd techniques to borrow: query expansion, a top-rank bonus in RRF, position-aware reranker blending, and an explain trace. Without this eval there is no way to tell whether any of them helps LCG.

## Related

- #734 (context assembly baselines), #752 (origin; a later variant can measure retrieval restricted to primary sources), #733 (document mode)"

## Background

`lcg-eval` today measures **extraction** quality (blind pairwise judging, failure taxonomy, the ontology matrix; see `docs/extraction-quality-evaluation.md`). It has no way to say whether the read side (`knowledge_find_entities`, `knowledge_find_relationships`, `knowledge_search_passages`) returns the right things for a real question, so LCG's hybrid search (FTS + vectors fused with RRF) has no retrieval number to compare against alternatives.

The most relevant alternative for a Liminis Notes user is **qmd** (tobi/qmd, MIT): local markdown search combining SQLite FTS5 and sqlite-vec over ~900-token chunks, LLM query expansion into lexical and vector sub-queries, RRF (k=60) with a top-rank bonus, an LLM reranker with position-aware blending, and an `--explain` score trace. It is the retrieval half of the "LLM wiki over Obsidian" setups Liminis Notes competes with, and the strongest local stand-in for the "raw top-k retrieval" baseline that #734 asks for.

A 2026-11-15 agent-memory survey proposed borrowing several qmd techniques (query expansion, a top-rank bonus in RRF, position-aware reranker blending, an explain trace). Without a retrieval eval there is no way to tell whether any of them helps LCG, so this eval is the prerequisite measurement. LCG's distinguishing capability, belief time distinct from event time (ADR-0705), should show up as an advantage on temporal and superseded content; qmd's query expansion should show up as an advantage on paraphrase queries. The eval exists to confirm or refute both expectations.

Related: #734 (context assembly baselines), #752 (origin; a later variant can restrict retrieval to primary sources), #733 (document mode).

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Score LCG retrieval against a checked-in question set (Priority: P1)

A maintainer changing LCG's search (a ranking tweak, a fusion change) runs a retrieval eval mode of `lcg-eval` over the notebook corpus and gets recall@k, MRR, nDCG, latency and model-call counts per LCG configuration (passages, entities+facts), broken down by question category.

**Why this priority**: This is the core missing measurement and is useful with no baseline attached. It is what makes future retrieval changes testable.

**Independent Test**: Run the retrieval mode against the checked-in corpus fixture and question set with LCG configurations only; verify a metrics report is produced and that re-running on unchanged inputs reproduces the same relevance metrics.

**Acceptance Scenarios**:

1. **Given** the checked-in corpus, question set and relevance judgements, **When** the retrieval eval runs for the LCG passages configuration at k, **Then** the report lists recall@k, MRR and nDCG@k overall and per question category, plus latency and model-call counts.
2. **Given** the same inputs and an unchanged LCG build, **When** the eval is run twice, **Then** the relevance metrics are identical.
3. **Given** a question whose relevant evidence is a specific passage within a note, **When** LCG returns the right note but not the right passage, **Then** the note-level metric counts it as a hit and the grounded-evidence metric does not.

---

### User Story 2 - Compare LCG against qmd side by side (Priority: P1)

A maintainer installs qmd out of band, points the eval at the same corpus and question set, and gets one table comparing LCG passages, LCG entities+facts, qmd without reranker and qmd with reranker, all at the same k.

**Why this priority**: The comparison is the stated purpose; it answers whether LCG hybrid search is competitive with the strongest local alternative.

**Independent Test**: With qmd installed, run the comparison; verify a single table with four configurations and identical k. Without qmd installed, verify the eval fails the qmd configurations with a clear message and still runs the LCG ones.

**Acceptance Scenarios**:

1. **Given** qmd is installed and indexed over the same corpus, **When** the comparison runs, **Then** one table shows all four configurations on the same questions at the same k.
2. **Given** qmd is not installed, **When** the comparison runs, **Then** the LCG configurations still produce results and qmd columns are reported as skipped with the reason; lcg builds and tests never require qmd.
3. **Given** qmd's reranker is enabled versus disabled, **When** results are compared, **Then** the two are reported as separate configurations with their own latency and model-call counts.

---

### User Story 3 - See where each system wins, per question category (Priority: P1)

A maintainer reads the report and sees, per category (direct lookup, paraphrase, temporal/superseded, answer-producing), which configuration wins and by how much, so the expectation "graph with belief time beats plain text on superseded content; qmd query expansion helps on paraphrase" is confirmed or refuted from data.

**Why this priority**: A single aggregate number would hide exactly the differences the issue is about.

**Independent Test**: Run the comparison and verify the per-category breakdown exists and that the temporal/superseded and paraphrase categories each contain enough questions to be reported.

**Acceptance Scenarios**:

1. **Given** a completed comparison, **When** the report is read, **Then** each category lists every configuration's metrics and marks the best one.
2. **Given** a superseded-decision question where the old decision is stale, **When** a configuration returns the superseded note or fact ranked above the current one, **Then** it is penalised on the stale-evidence measure for that question.

---

### User Story 4 - Reuse the question set and judgements for future retrieval changes (Priority: P2)

A contributor evaluating a retrieval change (query expansion, rank bonus, reranker blending, explain trace) reuses the same checked-in question set and judgements, adds questions in the documented format, and re-runs without regenerating anything.

**Why this priority**: Reuse is an explicit acceptance criterion, but it depends on the P1 stories existing.

**Independent Test**: Add a new question with judgements in the documented format and verify the eval picks it up, and that a malformed entry is rejected with a message naming the offending question.

**Acceptance Scenarios**:

1. **Given** a documented question/judgement format, **When** a contributor adds a question, **Then** the next run includes it in the right category.
2. **Given** a question that references a note or passage absent from the corpus, **When** the set is loaded, **Then** loading fails and names the question and the missing reference.

---

### Edge Cases

- qmd is absent, the wrong version, or its model downloads are unavailable: qmd configurations are skipped with an explicit reason, never silently reported as zero.
- LCG returns passage or entity/fact results rather than whole notes; results must be mapped to notes/passages consistently to compare against qmd's document/chunk output.
- Different chunking (LCG passages vs qmd's ~900-token chunks): grounded-evidence judging must not favour one system's chunk boundaries.
- A question has multiple relevant notes, or none (negative questions where no relevant note exists): metrics must handle both without dividing by zero.
- Fewer than k results returned.
- LLM-dependent steps (qmd expansion/rerank, LCG embeddings/extraction) are non-deterministic or model-version-dependent: runs record model identities and versions so results are attributable.
- Corpus ingestion into LCG requires extraction model calls; the cost of ingestion must be reported separately from per-query cost.
- Superseded content where both old and new versions remain in the corpus: the judgement must say which is current.
- Latency measured cold versus warm: the report states which and applies it uniformly.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: `lcg-eval` MUST provide a retrieval eval mode, separate from the existing extraction eval, that runs a question set against a notebook corpus and reports results without affecting existing extraction-eval behaviour.
- **FR-002**: The mode MUST report recall@k, MRR and nDCG@k for each configuration, overall and per question category.
- **FR-003**: The mode MUST report per-configuration latency (per query, with the cold/warm convention stated) and the number of model calls made at query time, and MUST report corpus indexing/ingestion cost separately.
- **FR-004**: The mode MUST evaluate at least these configurations: LCG passages (`knowledge_search_passages`), LCG entities+facts (`knowledge_find_entities` and `knowledge_find_relationships`), qmd without its reranker, and qmd with its reranker.
- **FR-005**: All configurations in a run MUST use the same k, the same corpus and the same question set.
- **FR-006**: qmd MUST be installed and indexed out of band; it MUST NOT become a build, test or runtime dependency of any lcg crate, and the repo MUST document how to install and run it for this eval.
- **FR-007**: When qmd is unavailable, the eval MUST still run the LCG configurations and MUST report qmd configurations as skipped with a reason.
- **FR-008**: The corpus used MUST be a notebook corpus, either `demo-notebook` or a checked-in fixture derived from it, versioned so that a run can state exactly which corpus it used.
- **FR-009**: The question set MUST be checked in, with per-question relevance judgements at note level and, for answer-producing questions, at passage/evidence level, in a documented format that is validated on load.
- **FR-010**: Each question MUST carry exactly one category from at least: direct lookup, paraphrase, temporal/superseded, and answer-producing.
- **FR-011**: The question set MUST include temporal and superseded questions in which a decision was later replaced, with judgements identifying the current evidence and the superseded (stale) evidence.
- **FR-012**: The question set MUST include paraphrase questions whose wording deliberately shares little vocabulary with the relevant text.
- **FR-013**: For answer-producing questions the eval MUST assess grounded evidence (whether the returned passages or facts actually support the answer), not only whether the right note was returned.
- **FR-014**: The eval MUST report a stale-evidence measure for temporal/superseded questions, i.e. whether superseded content outranks current content.
- **FR-015**: The report MUST present a side-by-side table of all configurations and a per-category breakdown that marks the best configuration in each category.
- **FR-016**: A run MUST record what is needed to reproduce it: corpus version, question-set version, LCG build, qmd version and configuration, k, and the models used for embedding, expansion and reranking.
- **FR-017**: Relevance metrics on unchanged inputs and an unchanged system MUST be reproducible; where a component is non-deterministic, the report MUST say so.
- **FR-018**: Result mapping from each system's output (LCG passages, entities, facts; qmd documents/chunks) to judged notes/passages MUST be defined and applied identically across configurations.
- **FR-019**: A question set load error (malformed entry, reference to a note or passage not in the corpus, unknown category) MUST fail with a message naming the question.
- **FR-020**: The eval MUST be runnable without cloud services beyond what the configurations themselves require, and any model-judged step (e.g. grounded-evidence judging) MUST record the judge model and be cacheable so re-runs do not repeat paid calls.
- **FR-021**: The user-facing eval documentation MUST describe the retrieval mode, the question format, how to add questions, and how to run the qmd comparison.

### Key Entities *(if applicable)*

- **Corpus**: A versioned set of markdown notes derived from a real notebook, with stable note identifiers.
- **Question**: Text, category, and judgements (relevant notes; for answer-producing questions the supporting evidence; for superseded questions the current vs stale evidence).
- **Configuration**: A named retrieval setup under test (LCG passages, LCG entities+facts, qmd, qmd+rerank), with its parameters.
- **Run report**: Per-configuration, per-category metrics, latency, model-call counts, and reproduction metadata.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: One command sequence, documented, produces a side-by-side table for the four configurations on one corpus at one shared k.
- **SC-002**: The report shows per-category results and names the winning configuration for every category.
- **SC-003**: The checked-in question set contains enough questions in each category (including temporal/superseded and paraphrase) that per-category figures are reported for every category, with the per-category counts shown in the report.
- **SC-004**: Re-running on unchanged inputs reproduces recall@k, MRR and nDCG@k exactly for deterministic configurations.
- **SC-005**: A contributor can add a question and judgements using only the documentation and see it in the next run.
- **SC-006**: With qmd uninstalled, the LCG-only run completes and the repository builds and tests without qmd present.
- **SC-007**: The report supports a yes/no reading of both stated hypotheses: whether LCG beats qmd on temporal/superseded questions, and whether qmd beats LCG on paraphrase questions.

## Assumptions

- `demo-notebook` is not in this repository today. A checked-in fixture derived from it (cleaned of anything private) is the default corpus; the existing Wikipedia extraction corpus is not suitable because it is not notebook-style and has no superseded content.
- The question set is authored and judged by hand (possibly LLM-assisted drafting, with human review), checked in once, and treated as the gold standard; relevance judgements are not regenerated per run.
- k defaults to a single value shared by all configurations (for example 10) and is configurable per run.
- qmd results are consumed via qmd's own command-line or its documented interface; no qmd code is vendored.
- Ingesting the corpus into LCG uses the normal extraction path, so LCG entity/fact configurations depend on a chosen extraction backend and ontology, which are recorded in the run metadata.
- Grounded-evidence assessment may use an LLM judge, reusing the existing judge and judge-cache machinery in lcg-eval where practical.
- Results are comparative and indicative on a small corpus; they are not claimed to generalise beyond it.
- The eval is run on demand by maintainers, not in CI, because it needs models and an out-of-band qmd install.

## Out of Scope *(optional)*

- Changing LCG retrieval (query expansion, top-rank bonus, reranker blending, explain trace). This eval only measures them; implementations are follow-up issues.
- Context assembly baselines (#734), document mode (#733), and retrieval restricted to primary sources (#752 follow-up).
- Vendoring, forking or depending on qmd in any lcg crate; making qmd results part of CI.
- Extraction quality evaluation changes.
- Multi-corpus benchmarks or large-scale statistical significance testing.
- End-to-end answer-generation quality (an LLM writing the final answer); this eval stops at retrieved grounded evidence.

## Source References *(optional)*

- tobi/qmd (https://github.com/tobi/qmd): baseline system.
- `docs/extraction-quality-evaluation.md`, `docs/testing-and-evaluation.md`: existing lcg-eval documentation.
- ADR-0705: belief time separate from event time (basis for the temporal/superseded hypothesis).
- #734, #752, #733: related issues.
