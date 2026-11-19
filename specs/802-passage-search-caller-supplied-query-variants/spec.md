# Feature Specification: Passage search: caller-supplied query variants, fused with weighted RRF

**Feature Branch**: `fabrik/issue-802`
**Created**: 2026-11-18
**Status**: Specified
**Input**: User description: "## Problem

Paraphrase questions describe a thing instead of naming it, so they share few words with the note that answers them. On the v2 retrieval corpus (#788), paraphrase nDCG@10 is:
- `lcg-passages`: **0.150**
- `qmd`: 0.347
- `qmd-rerank`: 0.475

qmd closes most of that gap by rewriting each query with a local 1.7B LLM before searching.

LCG's main caller is already an LLM. The Liminis Notes agent, and any MCP client, can write those rewrites itself as part of the turn it is already running, at no extra model cost to LCG. Today the read APIs take exactly one `query` string, so the agent has nowhere to put them.

Example (v2 `paraphrase-03`): \"Whose sign-off lets someone get around the rule covering new-hire induction?\" The answer note says \"Marlin Policy is the policy responsible for onboarding… The Marlin Policy exception approver is Callum Brennan.\" An agent that rewrites the question as \"onboarding policy exception approver\" finds it. The question as written does not.

## Proposal

Let callers pass their own query variants, and fuse them deterministically.

1. **API.** `knowledge_search_passages` accepts an optional `queries: [string]` alongside `query`:
   - `query` stays required and is the primary query.
   - `queries` holds up to 5 extra variants (validated: non-empty strings, at most 5, total length bounded).
   - With no `queries`, behaviour is byte-for-byte unchanged.
2. **Execution.**
   - Embed all queries in one batched embedder call (`EmbedRole::Query`).
   - Run the mode's normal retrieval per query: vector today, plus whatever stages are enabled (hybrid #789, rerank #796).
   - Fuse with weighted RRF, giving the primary query a higher weight. qmd weights the original query ×2. Expose the weight as a constant.
   - Rerank, when on, runs once on the fused candidates, against the **primary** query.
3. **Belief time and origin are unchanged.** Variants only widen the candidate set. Every existing filter (`believed_only`, `as_of`, origins, groups) applies to every variant.
4. **Evidence.** Each result says which queries matched it (indices into `[query, ...queries]`), so an agent can see what each rewrite contributed.
5. **MCP tool description.** Tell agents when and how to supply variants. Use it when a direct search came back thin, or the question describes something rather than naming it. Good variants are keywords the note would use, a likely name for the thing described, and a one-sentence hypothetical answer.
6. **Graph search.** Once #797 (PR #801) merges, give `knowledge_graph_search` the same `queries` parameter, to seed entity linking from every variant. That can be a follow-up if #801 isn't merged when this lands.

## Measuring it

The eval needs to simulate the agent without leaking answers.

- **Fixture.** Add `crates/eval/fixtures/retrieval-v2/query-variants.jsonl`, keyed by question id and the questions hash, holding up to 5 variants per question.
  - The variants are written by an LLM that sees **only the question text**: no corpus, no notes, no relevance labels. It uses a fixed prompt committed next to the fixture.
  - Generation is a maintainer step done once, out of band, like the corpus render (#788). Workers don't need an LLM key.
  - Add an `--export-prompts` / `--replies` exchange like `retrieval-render`'s so the maintainer can generate it.
- **Configs.** `lcg-passages-variants` (vector + variants), plus `lcg-passages-variants-rerank` once #796 merges, next to `lcg-passages`, qmd and qmd-rerank.
- **Check.** A fixture check fails if any variant contains a relevant note's title or its key-fact string verbatim. That is an answer leak, not a rewrite.
- **Report.** Per-category nDCG with paired intervals against `lcg-passages`, plus p50 and p95. Expected cost: one batched embed and k×N ANN probes, well under 100 ms at N = 5.

## Acceptance

- With `queries` absent, existing tests pass unchanged; this is the regression guard.
- Unit tests:
  - weighted fusion ranks a result found only by the primary query, ahead of one found only by a variant at the same rank;
  - validation errors;
  - every filter applies to every variant;
  - evidence indices are reported.
- The fixture, its prompt and the leak check are committed. The maintainer generates the variants and commits them before Validate.
- Merge bar: on v2, paraphrase nDCG improves over `lcg-passages` with its paired interval above zero, no other category regresses beyond noise, and p50 ≤ 100 ms.

## Context

- Alternatives under evaluation, compared on the same v2 corpus:
  - LLM expansion inside LCG (#796, opt-in, Apple Foundation Models);
  - graph-aware search (#797).
- The maintainer prefers LCG not to run an LLM at query time by default. Caller-supplied variants are the cheapest way to test whether expansion pays."

## Background

Paraphrase-style questions describe a thing rather than naming it, so they share few words with the note that answers them. On the v2 retrieval corpus (#788), passage search (`lcg-passages`) scores paraphrase nDCG@10 of 0.150, against 0.347 for `qmd` and 0.475 for `qmd-rerank`. qmd closes most of that gap by rewriting each query with a local 1.7B LLM before searching.

LCG's main caller is already an LLM: the Liminis Notes agent, or any MCP client. It can write those rewrites itself during the turn it is already running, at no extra model cost to LCG. Today the read APIs take one `query` string, so the caller has nowhere to put them. Example (v2 `paraphrase-03`): the question "Whose sign-off lets someone get around the rule covering new-hire induction?" misses the answer note, which says "Marlin Policy is the policy responsible for onboarding… The Marlin Policy exception approver is Callum Brennan." The rewrite "onboarding policy exception approver" finds it.

The maintainer prefers that LCG not run an LLM at query time by default. Caller-supplied variants are the cheapest way to test whether query expansion pays, and they are compared on the same v2 corpus against two alternatives: LLM expansion inside LCG (#796, opt-in) and graph-aware search (#797).

Related work is at different stages: hybrid retrieval (#789), the reranker (#796) and graph search (#797, PR #801) are all still open. The parts of this issue that depend on them are conditional (see FR-007, FR-014 and Assumptions).

## User Scenarios & Testing *(mandatory)*

### User Story 1 - An agent widens a thin passage search with its own rewrites (Priority: P1)

A calling agent runs `knowledge_search_passages` with the user's question, gets thin or irrelevant results, and calls again with the same `query` plus up to 5 `queries` variants (keywords the note would use, a likely name for the thing, a one-sentence hypothetical answer). The results come from all variants fused into one ranked list, with the primary query favoured.

**Why this priority**: This is the whole feature. It is the capability that lets callers close the paraphrase gap without LCG running an LLM.

**Independent Test**: Against the v2 corpus, search `paraphrase-03` with and without the variant "onboarding policy exception approver". The Marlin Policy note is absent or low-ranked without the variant and ranked near the top with it.

**Acceptance Scenarios**:

1. **Given** a request with `query` and no `queries`, **When** it is served, **Then** the response is byte-for-byte identical to the response before this feature.
2. **Given** a request with `query` and `queries` of 1–5 valid strings, **When** it is served, **Then** the result list is the weighted fusion of the per-query results, and each result carries the indices of the queries that matched it (0 is `query`, 1.. are `queries` in order).
3. **Given** a document found by the primary query at rank r and another found only by a variant at the same rank r, **When** results are fused, **Then** the primary-only document ranks ahead.
4. **Given** a document returned by several queries, **When** results are fused, **Then** it appears once in the output, with every matching query index listed.

---

### User Story 2 - Variants never widen what the caller is allowed to see (Priority: P1)

Variants add candidates, but they never bypass belief-time, origin, or group scoping.

**Why this priority**: Leaking superseded, out-of-scope or other-group content through a variant would be a correctness and privacy bug, and would invalidate the evaluation.

**Independent Test**: Run a search with an origin filter, a group filter and a `min_score` floor, plus variants chosen to match excluded notes. None of the excluded notes appear.

**Acceptance Scenarios**:

1. **Given** `group_ids`, `origins` or `min_score`, **When** a search has variants, **Then** every per-query retrieval applies `group_ids` and `origins`, and the `min_score` floor to its vector list, so no result excluded by the group or origin filters in the single-query case is present in the variant case. In `hybrid` mode a variant's BM25 hits are not subject to the cosine floor, as in single-query hybrid.
2. **Given** a variant that closely matches a superseded note, **When** `belief_aware` ranking is on (the default in `hybrid` mode), **Then** that note is demoted exactly as it would be for the primary query, because belief-aware ranking runs once on the fused candidates.

*Scope note: `believed_only` and `as_of` are edge-read parameters, not passage-search parameters (ADR-0705), so they do not apply here. Belief handling for passages is `belief_aware` (ADR-0789). See ADR-0802, "Deferred".*

---

### User Story 3 - Invalid variant input is rejected clearly (Priority: P2)

A caller passes malformed `queries` and gets a clear error rather than silently degraded results.

**Why this priority**: Callers are LLMs that will sometimes produce malformed arguments. Clear errors let them self-correct. Silent truncation would hide the problem.

**Independent Test**: Send `queries` that are empty, contain an empty or whitespace-only string, have 6 entries, or exceed the total-length bound. Each returns a validation error that names the problem.

**Acceptance Scenarios**:

1. **Given** `queries` with 6 or more entries, **When** the request is made, **Then** it is rejected with a validation error.
2. **Given** `queries` containing an empty or whitespace-only string, **When** the request is made, **Then** it is rejected.
3. **Given** `queries` whose combined length exceeds the bound, **When** the request is made, **Then** it is rejected.
4. **Given** `queries` that is not an array of strings, **When** the request is made, **Then** it is rejected.

---

### User Story 4 - Agents know when and how to supply variants (Priority: P2)

An MCP client reading the `knowledge_search_passages` tool description learns that `queries` exists, when to use it, and what good variants look like.

**Why this priority**: The capability is useless if agents don't use it well. Agents learn it from the tool description, not from the code.

**Independent Test**: Read the tool listing from the MCP-over-stdio surface. The description and input schema for `knowledge_search_passages` document `queries`.

**Acceptance Scenarios**:

1. **Given** the MCP tool list, **When** an agent reads `knowledge_search_passages`, **Then** its schema lists `queries` as an optional array of at most 5 strings, and its description says to use it when a direct search came back thin or the question describes something rather than naming it.
2. **Given** the description, **When** read, **Then** it names three good variant kinds: keywords the note would use, a likely name for the thing described, and a one-sentence hypothetical answer.

---

### User Story 5 - The maintainer measures whether variants pay, without answer leakage (Priority: P1)

The maintainer generates a committed fixture of per-question query variants from an LLM that sees only the question text, and the eval reports whether passage search with variants beats plain passage search on v2.

**Why this priority**: The merge bar is a measured improvement. Without a leak-free fixture and report, the feature can't be accepted.

**Independent Test**: Run the variants-generation exchange offline (export prompts, write replies, import), run the leak check, then run the retrieval eval with the new config and read the per-category report with paired intervals and latency.

**Acceptance Scenarios**:

1. **Given** the v2 questions, **When** the maintainer exports prompts, **Then** each prompt contains only that question's text and the fixed committed instructions: no corpus, notes, or relevance labels.
2. **Given** replies written outside LCG, **When** they are imported, **Then** `query-variants.jsonl` is written with up to 5 variants per question id, stamped with the questions hash.
3. **Given** a variant containing a relevant note's title or key-fact string verbatim, **When** the fixture check runs, **Then** it fails and names the question and the leaking variant.
4. **Given** a fixture whose questions hash doesn't match the current questions, **When** the eval loads it, **Then** it fails clearly rather than using stale variants.
5. **Given** the committed fixture, **When** the eval runs, **Then** `lcg-passages-variants` is reported next to `lcg-passages`, `qmd` and `qmd-rerank`, with per-category nDCG, paired intervals against `lcg-passages`, and p50 and p95 latency.

---

### User Story 6 - Graph search accepts the same variants (Priority: P3)

Once graph search (#797) has landed, `knowledge_graph_search` accepts `queries` and seeds entity linking from every variant.

**Why this priority**: It is useful, but it depends on an unmerged feature. The issue allows it as a follow-up.

**Independent Test**: With graph search present, supply a variant naming an entity the primary query doesn't mention. That entity is linked.

**Acceptance Scenarios**:

1. **Given** graph search is on `main` when this work lands, **When** `knowledge_graph_search` receives `queries`, **Then** the same validation applies and entity linking is seeded from `query` and every variant.
2. **Given** graph search is not yet on `main`, **When** this work lands, **Then** no graph-search change is made and a follow-up issue is recorded.

---

### Edge Cases

- `queries` is present but an empty array: treated the same as absent. The response is byte-identical to the no-`queries` response.
- A variant is identical to `query`, or to another variant: it adds no information. Behaviour must be deterministic, and the evidence indices must still be well-defined. Whether duplicates are dropped or kept is a design choice, but the result must not rank any document higher merely because a duplicate was repeated.
- A variant returns no results: it contributes nothing, and the search succeeds.
- Every variant's results are a subset of the primary's: the order is determined by the weights, and the primary query's own ordering is not degraded.
- A document is matched only by variants: it can still appear, and its evidence lists only the variant indices.
- Ties in fused score are broken deterministically, so that repeated runs give identical output.
- `queries` supplied while the mode is not vector-only: each enabled stage (hybrid, rerank) runs per its own rules. Rerank, when on, runs once on the fused candidates against the primary query only.
- Whitespace around variants: validation treats whitespace-only as empty. Whether other strings are trimmed must be consistent and documented.
- Non-ASCII and very long single variants: length is measured consistently, and over-long input is rejected rather than truncated.
- A question with fewer than 5 variants (or none) in the fixture: valid. The eval runs with whatever the fixture holds.
- Fixture variants for a question id that no longer exists in the questions file: the fixture check fails.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: `knowledge_search_passages` MUST accept an optional `queries` parameter, an array of strings, alongside the existing required `query`. `query` remains the primary query.
- **FR-002**: When `queries` is absent (or an empty array), the response MUST be byte-for-byte identical to the current behaviour, including result order, fields and scores, and all existing tests MUST pass unchanged.
- **FR-003**: `queries` MUST be validated: at most 5 entries, each a non-empty (non-whitespace) string, with a bounded total length. Violations MUST produce a validation error identifying which rule was broken. Values are never silently truncated or dropped.
- **FR-004**: When variants are supplied, all queries (primary first) MUST be embedded in a single batched call as query-role embeddings.
- **FR-005**: For each query, the mode's normal retrieval MUST run, including any stages enabled for that mode.
- **FR-006**: Per-query results MUST be combined with weighted reciprocal-rank fusion in which the primary query has a higher weight than a variant. The primary weight MUST be a single named, documented constant (`PRIMARY_QUERY_WEIGHT`, initial value 2, as in qmd). The variants MUST share a fixed total budget (`VARIANT_BUDGET_WEIGHT`, initial value 1) split equally across the variants actually searched, so that together they never outweigh the primary query (its share of the total weight is at least 2/3 for 1–5 variants). *Revised after the first v2 measurement: the original wording gave every variant weight 1, which let 5 variants outweigh the primary 5:2 and cost 0.046 overall nDCG@10; see ADR-0802, decision 4.*
- **FR-007**: When a rerank stage is enabled (#796), it MUST run once on the fused candidate set, scored against the primary query only, never once per variant. If the rerank stage is not present in the code at implementation time, this requirement is deferred to that stage.
- **FR-008**: The `group_ids` and `origins` filters, and the `min_score` cosine floor on the vector list, MUST apply to every per-query retrieval. Variants MUST NOT surface any result from a group or origin that those filters would exclude for the primary query. In `hybrid` mode the BM25 list is not subject to `min_score` for any query, as in single-query hybrid, so a variant's lexical-only hit may appear. Belief handling is `belief_aware` ranking, which runs once on the fused candidates. `believed_only` and `as_of` are not passage-search parameters, so they are out of scope here (ADR-0802, "Deferred").
- **FR-009**: Each result in a request that used variants MUST report which queries matched it, as indices into `[query, ...queries]`. A result matched by several queries lists all of them, in ascending order.
- **FR-010**: Fusion and tie-breaking MUST be deterministic: the same input and index state produce the same output order.
- **FR-011**: The `knowledge_search_passages` entry in the MCP tool registry MUST document `queries` in its input schema, and its description MUST say when to supply variants (a direct search came back thin, or the question describes something rather than naming it) and what good variants are (keywords the note would use, a likely name for the thing described, a one-sentence hypothetical answer). The registry's own count assertions MUST remain correct.
- **FR-012**: The IPC contract between the service and its Python-side client MUST remain compatible: requests without `queries` behave exactly as before, and the parity tests cover the new optional parameter and the evidence field.
- **FR-013**: A fixture `crates/eval/fixtures/retrieval-v2/query-variants.jsonl` MUST hold up to 5 variants per question, keyed by question id, and stamped with the hash of the questions it was generated for. The eval MUST refuse a fixture whose hash does not match the current questions.
- **FR-014**: The variants MUST be generated from prompts containing only the question text and a fixed instruction text, and that text MUST be committed next to the fixture. The prompts MUST NOT contain corpus content, note text or relevance labels.
- **FR-015**: The eval tooling MUST support an offline generation exchange like `retrieval-render`'s: export prompts to a file, accept replies from a directory, and write the fixture. No API key may be required for the checked-in workflow. Generation is a one-time maintainer step.
- **FR-016**: A fixture check MUST fail when any variant contains, verbatim, the title or the key-fact string of any note that is relevant to that question. The failure message MUST identify the question and the variant.
- **FR-017**: The eval MUST provide a `lcg-passages-variants` config (vector retrieval plus the fixture's variants), reported alongside `lcg-passages`, `qmd` and `qmd-rerank`. A `lcg-passages-variants-rerank` config MUST be added if and only if the rerank stage (#796) is present at implementation time.
- **FR-018**: The eval report MUST show per-category nDCG for the variants config with paired intervals against `lcg-passages`, and p50 and p95 latency.
- **FR-019**: If graph search (#797) is on `main` when this work lands, `knowledge_graph_search` MUST accept the same `queries` parameter with the same validation and seed entity linking from every variant. Otherwise this is delivered as a follow-up issue.
- **FR-020**: A request that uses variants MUST NOT add any LLM call inside LCG. Variants are supplied by the caller only.

### Key Entities *(if applicable)*

- **Query variant**: A caller-written alternative phrasing of the primary query (keywords, a likely name, a hypothetical answer). It widens the candidate set and never replaces the primary query.
- **Primary query**: The required `query`; always index 0 in the evidence list and the only query a reranker scores against.
- **Evidence (matched query indices)**: Per-result list of indices into `[query, ...queries]` that retrieved that result.
- **Variants fixture**: `query-variants.jsonl`, per-question-id variants generated from the question text alone, stamped with the questions hash.
- **Variant-generation prompt**: The fixed, committed instruction text used for generation. It sees the question and nothing else.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: With `queries` absent, 100% of the existing test suite passes unchanged and responses are byte-identical.
- **SC-002**: On v2, paraphrase nDCG@10 for `lcg-passages-variants` is higher than `lcg-passages`, with the paired interval's lower bound above zero.
- **SC-003**: On v2, no other question category regresses beyond noise, meaning no category's paired interval lies wholly below zero against `lcg-passages`.
- **SC-004**: p50 and p95 latency of `lcg-passages-variants` at 5 variants on the v2 corpus are reported. *The original "p50 at most 100 ms" bar was dropped by the maintainer on 2026-11-19, since these are deliberate agent searches; the measured p50 is about 140 ms.*
- **SC-005**: The leak check passes on the committed fixture: 0 variants contain a relevant note's title or key-fact string verbatim.
- **SC-006**: In unit tests, a primary-only result ranks ahead of a variant-only result at the same rank, 100% of the time, and every filter is shown to apply to every variant.
- **SC-007**: Every validation rule (more than 5, empty or whitespace entry, over the length bound, wrong type) has a test that asserts an error.
- **SC-008**: A reader of the MCP tool description alone can tell when to supply variants and what to put in them.

## Assumptions

- Variants are provided by the calling agent. LCG never generates them, which satisfies the maintainer's preference for no query-time LLM by default.
- The primary-query weight is 2, taken from qmd, and the variants share a fixed total budget of 1, split equally (see FR-006 and ADR-0802, decision 4; the original "each variant weighs 1" was superseded). Both are named constants so they can be tuned without an API change.
- The initial total-length bound (all variants combined) is generous enough for a short sentence per variant, so around 2,000 characters. The exact value is a design choice and is documented where the validation is defined.
- "Regresses beyond noise" (SC-003) means a category's paired interval against `lcg-passages` lies wholly below zero. Categories with intervals spanning zero count as noise.
- #789 (hybrid) and #796 (rerank) are not yet merged. This work targets whatever retrieval modes exist when it is implemented; the hybrid and rerank parts apply only if those stages are present. The merge bar is measured on the vector configuration.
- #797 / PR #801 is not merged. Graph-search `queries` (FR-019) ships here only if it has merged by the time implementation starts; otherwise it is a follow-up.
- The maintainer generates and commits the variants fixture before Validate, in the same way the v2 corpus render and cassette were produced (offline exchange on a subscription, no API key). Implement and Review work without the fixture's contents, using a small test fixture for the checks. The merge-bar measurement can only run after the maintainer commits it.
- The variants LLM sees only the question; the leak check is the safety net for answers leaking through.
- Duplicate or empty-result variants are handled harmlessly by fusion (see Edge Cases). Dropping versus keeping duplicates is a design decision.

## Out of Scope *(optional)*

- Generating variants inside LCG (that is #796's opt-in LLM expansion).
- Any LCG-side query-time LLM, in any mode, by default.
- Changing the ranking of requests that have no `queries`.
- More than 5 variants, per-variant weights chosen by the caller, or a caller-supplied primary weight.
- Variants for read APIs other than `knowledge_search_passages` (and `knowledge_graph_search`, per FR-019).
- Tuning the RRF constant or the weight to improve the benchmark, beyond reporting the result with the documented constant.
- Hybrid retrieval (#789), the reranker (#796) and graph search itself (#797).
- Running the one-time LLM variant generation inside CI or Fabrik workers.

## Source References *(optional)*

- #788 — retrieval corpus v2 and the render exchange (`retrieval-render --export-prompts` / `--replies`) this feature mirrors
- #789 — hybrid BM25+vector with RRF
- #796 — local reranker and opt-in LLM query expansion
- #797 / PR #801 — `knowledge_graph_search`
- `crates/eval/fixtures/retrieval-v2/README.md` — v2 corpus, cassette and render provenance
- qmd query expansion, which weights the original query ×2 in fusion
