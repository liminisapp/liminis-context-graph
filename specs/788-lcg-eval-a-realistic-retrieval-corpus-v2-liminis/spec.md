# Feature Specification: lcg-eval retrieval corpus v2 — Liminis-shaped, 300+ notes, 200+ stratified questions, origins, confidence intervals

**Feature Branch**: `fabrik/issue-788`
**Created**: 2026-11-18
**Status**: Specified
**Input**: User description: "## Problem

The #754 Fernhill fixture is too small to separate LCG from qmd or to steer the retrieval work:

- **Size:** 31 notes and about 1,200 words in total (40 words per note on average; the smallest is 21). Every note fits in one chunk, so chunking, long-note recall and passage choice within a note are never exercised.
- **Questions:** 43, of which 40 are judged. One question moves recall by 0.025, so every gap in the first run (#786: qmd-rerank nDCG 0.817 vs lcg-passages 0.789) is one to three questions' worth.
- **Ceiling:** direct questions score 1.000 vs 0.994. Both systems are at the ceiling, so the category carries no signal.
- **Negatives:** with only 3, false-pos@k is 1.0 for every config and tells nothing apart.
- **Not shaped like Liminis Notes:** there are no agent-written pages, filed answers or clipped articles. Origin (ADR-0752) and the self-corroboration case aren't tested, and there are no near-duplicate pages, which are what a wiki actually accumulates.

## Desired behaviour

A second fixture, `fixtures/retrieval-v2`, kept beside the first, which stays as a fast smoke corpus. It is synthetic so it can be checked in, and shaped like a real Liminis notebook:

1. **Scale:** about 300–500 notes in a realistic size mix: many short notes, a fair number of 500–2,000-word notes, and a few over 5,000 words.
2. **Origins:** human notes (`primary`), clipped articles (`external`), agent-maintained topic and person pages (`derived`, citing their sources), and filed answers (`answer`). Agent pages restate facts from primary notes, so that \"the wiki citing itself\" is present.
3. **Time:** decisions revised several times, with a mix of explicit `knowledge_supersede_document` supersession and plain later notes that contradict earlier ones without superseding them. Effective dates differ from recording dates.
4. **Questions:** 200 or more, stratified by category:
   - direct, paraphrase (no lexical overlap with the source), temporal, answer;
   - multi-hop (needs facts from two notes);
   - within-note passage (the answer is one paragraph of a long note);
   - at least 30 negatives with near-miss distractors.

   Every question carries `relevant_notes` and, for answers, `evidence` key facts.
5. **Generated, then checked by a human.** A script (checked in with its seed) generates notes and questions from a scenario spec. A human then checks a sample of the questions. Record corpus and question versions and hashes, as v1 does.
6. **Statistical reporting:** the report gives bootstrap 95% confidence intervals on the headline metrics, so a difference that is only noise is labelled as such.
7. **Running on a real notebook:** document how a maintainer can run the eval against a private real notebook with a local question set, reporting results only and checking nothing in.

## Acceptance

- Both fixtures run through `07-retrieval-eval.sh`, selected with `--corpus`.
- On v2, direct questions are no longer at the ceiling for both systems, or the report says they are, with CIs.
- The run records a cassette for v2, so later runs replay at no cost.

## Related
#754 (harness), #786, #787 (believed_only), and the retrieval improvement issue that measures itself on this corpus."

## Background

`lcg-eval retrieval` (#754, ADR-0754) compares LCG's read side with qmd over a checked-in synthetic notebook, the 31-note "Fernhill" fixture. The first live run (#786) showed that fixture cannot do the job it was built for:

- **Too small to resolve differences.** 40 judged questions means one question is 0.025 of recall. The headline gap (qmd-rerank nDCG 0.817 vs lcg-passages 0.789) is one to three questions and cannot be told from noise.
- **No signal in some categories.** Direct questions are at the ceiling for both systems; with 3 negatives, false-pos@k is 1.0 for every configuration.
- **Never exercises the hard parts.** Every note is a single chunk, so long-note recall and choosing the right passage within a note are untested.
- **Not shaped like Liminis Notes.** Real notebooks hold human notes, clipped articles, agent-maintained pages that restate human facts (with citations), filed answers, and near-duplicate pages. Episode origin (ADR-0752: derived content cites but never corroborates) and the "wiki citing itself" case are not represented, and temporal cases are limited to clean explicit supersession.

Upcoming retrieval-improvement work (and the `believed_only` configuration from #787) needs a corpus that can *steer* it and say when a difference is real. This issue provides that corpus (`retrieval-v2`) and the statistical reporting to read it. v1 stays as the fast smoke corpus.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Run the eval on a realistic v2 corpus (Priority: P1)

A maintainer evaluating a retrieval change selects the v2 fixture and gets a report over a notebook that looks like a real Liminis workspace: hundreds of notes of mixed sizes and origins, with revised decisions, and 200+ questions across all categories.

**Why this priority**: Without the corpus and questions there is nothing for the rest of the work to measure. It is the core deliverable.

**Independent Test**: Run the eval's dry-run plan against v2 and check the printed counts, versions and hashes; run the loader's validation over the fixture and confirm every manifest, note and question reference resolves.

**Acceptance Scenarios**:

1. **Given** the v2 fixture directory, **When** the retrieval eval resolves the plan with `--dry-run`, **Then** it prints v2's corpus version, question-set version, content hashes, note count and per-category question counts, and spends nothing.
2. **Given** v2, **When** its notes are measured, **Then** there are 300–500 notes, with many short notes, a substantial group of 500–2,000-word notes, and at least a few over 5,000 words.
3. **Given** v2's questions, **When** counted by category, **Then** there are at least 200 in total, covering direct, paraphrase, temporal, answer, multi-hop and within-note-passage categories, with at least 30 negatives that have near-miss distractors in the corpus.
4. **Given** the long notes in v2, **When** the corpus is chunked at the manifest's chunking setting, **Then** many notes span multiple chunks, so passage choice within a note is exercised.

---

### User Story 2 - Select either fixture from the standard script (Priority: P1)

A maintainer runs `07-retrieval-eval.sh` with `--corpus` to pick v1 (fast smoke) or v2 (full), without editing the script.

**Why this priority**: The issue's acceptance requires both fixtures to run through the script; today it is hardwired to v1.

**Independent Test**: Run the script in dry-run mode once per fixture and confirm each resolves its own corpus; run it with no `--corpus` and confirm the documented default.

**Acceptance Scenarios**:

1. **Given** the script, **When** run with `--corpus` pointing at v1 or v2 (dry-run), **Then** the plan reflects that fixture.
2. **Given** an invalid or missing corpus path, **When** the script runs, **Then** it fails with a message naming the path and spends nothing.
3. **Given** no `--corpus`, **When** the script runs, **Then** behaviour is unchanged from today (v1), so existing invocations keep working.

---

### User Story 3 - Test origin and self-corroboration (Priority: P1)

A maintainer measuring read-side behaviour needs v2 to contain the origins a real notebook has, so retrieval is tested against agent pages that restate primary facts and against filed answers and clipped articles — and results are reported per origin.

**Why this priority**: Origin-awareness (ADR-0752) is a core LCG differentiator and the "wiki citing itself" case is a main motivation for v2.

**Independent Test**: Inspect the v2 manifest for origin labels and source citations, ingest it through a canned responder and confirm each note is submitted with its declared origin; check the report for a per-origin breakdown.

**Acceptance Scenarios**:

1. **Given** v2, **When** its notes are classified, **Then** all four origins (`primary`, `external`, `derived`, `answer`) are present in realistic proportions, and every `derived` page cites the notes it restates.
2. **Given** a `derived` page that restates a fact from a `primary` note, **When** the corpus is ingested, **Then** the page is ingested with origin `derived`, so it cannot corroborate itself (ADR-0752).
3. **Given** questions whose relevant note is a `primary` note that a `derived` page also restates, **When** the report is produced, **Then** results can be read by the origin of the returned notes (e.g. how often a derived restatement outranks the primary source).
4. **Given** v1, **When** it is run, **Then** it still ingests exactly as before (v1 declares no origins).

---

### User Story 4 - Temporal realism: explicit and implicit revision (Priority: P2)

Decisions in v2 are revised several times. Some revisions are recorded through explicit supersession; others are plain later notes that contradict earlier ones with no supersession. Effective dates differ from recording dates.

**Why this priority**: Real notebooks contain both; the v1 corpus covers only clean explicit pairs. Needed to tell whether belief-time mechanisms actually help.

**Independent Test**: Load the fixture and confirm decision chains of length ≥ 3 exist, with both kinds of revision, and that every temporal question names current and stale notes consistently with the manifest.

**Acceptance Scenarios**:

1. **Given** v2, **When** the manifest is read, **Then** it contains decisions revised at least three times, with explicit supersessions and un-superseded contradicting later notes both present.
2. **Given** a temporal question over an implicitly contradicted decision, **When** it is loaded, **Then** the note that is current and the note that is stale are marked per the question's declared ground truth, even though the manifest has no supersession for that pair.
3. **Given** a supersession with an effective date different from the recording date of the new note, **When** ingested, **Then** the effective date is the one used.

---

### User Story 5 - Know whether a difference is real (Priority: P1)

A reader of the report sees bootstrap 95% confidence intervals on the headline metrics, and for each pair of configurations a paired comparison, so a gap that is only noise is labelled as such.

**Why this priority**: The motivating failure of v1 is that gaps of one to three questions were read as findings. Statistics are what make a larger corpus usable.

**Independent Test**: Feed the report builder fixed per-question scores with a known distribution and confirm interval bounds, the paired-difference interval and the "within noise" label are reproduced exactly for a given seed.

**Acceptance Scenarios**:

1. **Given** per-question scores for a configuration, **When** the report is built, **Then** each headline metric (recall@k, MRR, nDCG@k, grounded-recall, stale-above-current, false-positive rate) shows a point estimate and a bootstrap 95% CI.
2. **Given** two configurations whose paired per-question difference has a CI that includes zero, **When** the report is built, **Then** the comparison is labelled "not distinguishable from noise" rather than named a winner.
3. **Given** the same inputs and the same bootstrap seed, **When** the report is built twice, **Then** the output is byte-identical.
4. **Given** per-category breakdowns, **When** a category has few questions, **Then** its interval is reported and visibly wide (and the report flags categories below a minimum sample size).
5. **Given** direct questions for which both systems score near the ceiling, **When** the report is built, **Then** the report either shows them below the ceiling with CIs or states explicitly that they are at the ceiling, with CIs.

---

### User Story 6 - Regenerate the corpus reproducibly and have it checked (Priority: P2)

A maintainer can regenerate notes and questions from a scenario spec and a seed, and a human has checked a sample of questions; the fixture records versions and hashes so changes are visible.

**Why this priority**: Reproducibility and human validation make the corpus trustworthy, but the corpus itself (Story 1) has more immediate value.

**Independent Test**: Run the generator twice with the same spec and seed and compare outputs byte-for-byte; run the fixture's checks and confirm the review record is present and consistent with the question set.

**Acceptance Scenarios**:

1. **Given** the checked-in scenario spec and seed, **When** the generator runs twice, **Then** the notes, manifest and questions are identical to each other and to the checked-in fixture.
2. **Given** a different seed, **When** the generator runs, **Then** it produces a different, equally valid fixture without hand editing.
3. **Given** the generated paraphrase questions, **When** checked automatically, **Then** each shares less than a stated lexical-overlap threshold with its relevant note(s).
4. **Given** the fixture, **When** it is inspected, **Then** a human-review record is present listing the sampled questions, the reviewer's verdict per question, and the overall error rate, and the question-set version/hash recorded in the report matches the reviewed set.
5. **Given** any inconsistency (a question referencing a missing note, evidence not in the relevant notes, a temporal question with an unspecified stale note), **When** the fixture is loaded, **Then** loading fails naming the question.

---

### User Story 7 - Run on a private real notebook without leaking it (Priority: P3)

A maintainer points the eval at their own real notebook with a local question set and gets results only; nothing from the notebook or the questions is checked in or written into the report.

**Why this priority**: Synthetic corpora are an approximation; a maintainer needs a way to sanity-check on real data, but this is a documentation-level capability for a few users.

**Independent Test**: Follow the documented procedure against a small stand-in directory and confirm the report contains only metrics (no note text, no question text, no private paths) and that no file under the repository is created or modified.

**Acceptance Scenarios**:

1. **Given** the documentation, **When** a maintainer follows it with a private notebook and local question file, **Then** the eval runs end to end using the same question format.
2. **Given** a private-notebook run, **When** the report is written, **Then** it contains aggregate metrics, counts and hashes only, and omits note content and question text unless explicitly requested.
3. **Given** the documented procedure, **When** followed, **Then** it directs outputs to a location outside the repository (or an ignored path) and states that nothing is committed.

---

### User Story 8 - Record a cassette so later runs are free (Priority: P2)

The v2 run records an ingest cassette so that subsequent runs replay at no extraction cost.

**Why this priority**: v2 has ~10× the notes of v1; paying live extraction for every run would make iteration impractical.

**Independent Test**: Run with a replayed cassette against a canned responder and confirm zero extraction requests and identical ingest results.

**Acceptance Scenarios**:

1. **Given** a live run of v2 with recording enabled, **When** it finishes, **Then** a cassette covering every v2 extraction call exists and its location and the corpus hash it was recorded against are documented.
2. **Given** that cassette and an unchanged v2, **When** the eval is run again, **Then** it makes zero extraction requests.
3. **Given** a changed v2 (new corpus hash) and an old cassette, **When** the eval is run, **Then** it fails loudly on the cassette miss rather than silently calling a live model.

---

### Edge Cases

- A note longer than the chunking limit whose answer sits in only one chunk: relevance is at note level, but grounded evidence must be judged by content, so a note hit with the wrong passage still counts as a grounded miss.
- Near-duplicate pages (a primary note and several derived restatements): the corpus must state which is the relevant note for each question and the report must not credit a restatement as the primary source unless the question's ground truth says it is relevant.
- A question whose relevant notes are all `derived`: allowed; ground truth must name the cited sources too where they are also acceptable.
- Multi-hop questions need both notes; partial coverage (only one of the two) must score below full coverage under graded gain.
- A negative question's near-miss distractors must really be near-misses (same entities or vocabulary, different fact), or the negative tests nothing; a negative must never be answerable by a note that exists in the corpus.
- An implicitly contradicted decision with no supersession: the "current" note can only be determined by the question's declared ground truth, not by LCG's supersession state.
- Notes over 5,000 words must still ingest within the eval's request limits; if extraction cannot handle a note, that must surface as a visible ingest failure, not a silent skip.
- Bootstrap CIs on a metric with very few eligible questions (e.g. a category with a handful of items) must not crash or report a degenerate zero-width interval as meaningful.
- A configuration that is skipped or has failed queries must still produce a coherent report (CIs only over scored questions; failed questions listed, per ADR-0754).
- Note ids remain lowercase slugs so they can serve as qmd file stems; any generated id must obey that rule and be unique.
- Private-notebook mode must refuse to write note content into the report even when a question errors.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: A second fixture, `retrieval-v2`, MUST exist beside the unchanged v1 fixture (`retrieval`), in the same on-disk format (manifest, notes, questions, question-set metadata), extended only with additive fields needed for origins, citations and implicit-revision ground truth.
- **FR-002**: v2 MUST contain between 300 and 500 synthetic notes containing no real or private data, with a size mix that includes many short notes, a substantial group of 500–2,000-word notes, and at least three notes over 5,000 words.
- **FR-003**: Each v2 note MUST declare one origin from `primary`, `external`, `derived`, `answer`; all four MUST be present, with `primary` the largest share.
- **FR-004**: Every `derived` note in v2 MUST cite the notes it restates, and a substantial subset of `derived` notes MUST restate facts from `primary` notes so that the "wiki citing itself" case is present.
- **FR-005**: The eval's ingest MUST submit each note with its declared origin so that episode origin (ADR-0752) is stamped; notes without a declared origin (all of v1) MUST ingest exactly as they do today.
- **FR-006**: v2 MUST contain decisions revised at least three times, including both explicit supersession (recorded via the document-supersession mechanism) and later notes that contradict earlier ones with no supersession, and effective dates that differ from recording dates for some revisions.
- **FR-007**: v2 MUST contain near-duplicate pages (the same fact restated in several notes) in a realistic quantity, with the question ground truth stating which note(s) are relevant.
- **FR-008**: v2 MUST contain at least 200 questions in total across these categories: direct, paraphrase, temporal, answer, multi-hop, and within-note passage; every category MUST have enough questions that its confidence interval is meaningfully narrower than v1's, with a minimum per-category count stated in the fixture documentation.
- **FR-009**: v2 MUST contain at least 30 negative questions, each with near-miss distractor notes in the corpus, and no note in the corpus may answer a negative question.
- **FR-010**: Every v2 question MUST carry `relevant_notes` (with graded gain), and answer questions MUST carry `evidence` key facts that fall within their relevant notes; the loader MUST reject any question that violates the existing question rules.
- **FR-011**: Multi-hop questions MUST name at least two relevant notes whose facts are both required; within-note-passage questions MUST target a single paragraph of a note long enough to span multiple chunks, with the key fact recorded as evidence.
- **FR-012**: Paraphrase questions MUST share less than a documented lexical-overlap threshold with their relevant notes, and the generator MUST check and enforce this automatically.
- **FR-013**: Temporal questions over implicitly contradicted decisions MUST declare their `current` and `stale` notes in the question itself, and the loader MUST accept them without requiring a recorded supersession for that pair.
- **FR-014**: A generation script, checked in with its scenario spec and seed, MUST produce v2's notes, manifest and questions deterministically (same spec + seed ⇒ byte-identical output), without network access or paid model calls.
- **FR-015**: The checked-in v2 fixture MUST equal the generator's output for the checked-in spec and seed, and a check MUST detect drift between them.
- **FR-016**: A human-review record MUST be checked in listing a stratified sample of questions (covering every category), the reviewer's verdict on each (ground truth correct / note relevance correct / key fact correct), and the resulting error rate; the question-set version and hash MUST identify the reviewed set.
- **FR-017**: v2 MUST carry corpus and question-set versions and content hashes, printed by the report and the dry-run plan exactly as v1's are.
- **FR-018**: `07-retrieval-eval.sh` MUST accept `--corpus <DIR>` to select a fixture, default to v1 when omitted, and fail with a clear message on an invalid path; both fixtures MUST run through it.
- **FR-019**: The report MUST give a bootstrap 95% confidence interval for each headline metric per configuration (recall@k, MRR, nDCG@k, grounded-recall@k, stale-above-current, false-positive rate), using per-question resampling with a fixed, recorded seed and resample count so output is reproducible.
- **FR-020**: The report MUST compare configurations with a paired per-question bootstrap on the difference and label any difference whose 95% interval includes zero as not distinguishable from noise, never as a winner. The existing hypothesis readings (H1/H2) MUST use the same rule.
- **FR-021**: The report MUST present metrics per category with intervals, flag categories whose sample is below a stated minimum, and state when a category is at the ceiling (with its CI) rather than implying a ranking.
- **FR-022**: The report MUST break results down by the origin of the returned notes (at least the share of top-k results that are `derived` restatements of a primary source and how often a derived restatement outranks its primary source), for fixtures that declare origins.
- **FR-023**: Recording an ingest cassette for v2 MUST be supported using the existing record/replay mechanism, and replaying it MUST make zero extraction requests; a v2 whose corpus hash differs from the cassette's MUST fail loudly rather than fall back to a live call.
- **FR-024**: Documentation MUST describe the v2 fixture (shape, size mix, origins, question categories and counts, how to regenerate, how it was reviewed), and MUST replace v1-only statements in the retrieval-evaluation docs that no longer hold (e.g. "31 notes", "four categories").
- **FR-025**: Documentation MUST describe how a maintainer runs the eval against a private real notebook with a local question set in the same format, such that only results are reported, no note or question text is written into the report by default, and nothing is checked in.
- **FR-026**: v1 MUST remain unchanged in content, hashes and behaviour, and its existing tests MUST continue to pass.
- **FR-027**: Network-free tests MUST cover: v2 loading and validation, origin submission at ingest, un-superseded temporal ground truth, the generator's determinism and paraphrase-overlap check, bootstrap CI computation and the noise label, and per-origin reporting.
- **FR-028**: The existing metric definitions of ADR-0754 (deduplication, nDCG gain, negatives excluded from relevance means, failed queries listed not scored) MUST be unchanged; any new metric or label is additive.

### Key Entities *(if applicable)*

- **Note**: a markdown file with an id (lowercase slug), a reference time, and (v2) an origin and, for derived notes, a list of cited source notes.
- **Manifest**: ordered note list, chunking setting, supersession relations (explicit only), and corpus version.
- **Question**: id, category, text, graded relevant notes, evidence key facts (answer, within-note), declared current/stale notes (temporal), negative flag.
- **Scenario spec**: the checked-in description (entities, decision chains, note-type mix, size targets, question quotas) plus seed from which the generator produces the corpus.
- **Review record**: the checked-in human verdicts on a stratified question sample.
- **Cassette**: recorded ingest extraction for v2, keyed to a corpus hash.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: v2 loads with zero validation errors; it has 300–500 notes, ≥ 200 questions, ≥ 30 negatives, all four origins, ≥ 3 notes over 5,000 words, and every question category present.
- **SC-002**: Running the generator twice with the same spec and seed yields byte-identical output equal to the checked-in fixture; the drift check passes.
- **SC-003**: A human-review record exists covering every question category, with the error rate on the sampled questions stated; if the rate exceeds a maintainer-set threshold (default 5%) the affected categories are regenerated or corrected before the fixture is called reviewed.
- **SC-004**: Every headline metric in a v2 report carries a 95% bootstrap CI; two runs with the same seed produce identical reports; a pairwise comparison with a CI spanning zero is labelled "not distinguishable from noise".
- **SC-005**: On a v2 run, the report either shows direct-question scores below the ceiling for at least one configuration or states that direct questions are at the ceiling for both, in either case with CIs. With ≥ 200 questions, the per-question granularity is at most half of v1's (a single question moves recall by ≤ 0.0125 in aggregate).
- **SC-006**: `07-retrieval-eval.sh --corpus` resolves both fixtures in dry-run mode, and omitting `--corpus` behaves as today.
- **SC-007**: With the recorded v2 cassette, a re-run makes zero extraction requests; a changed corpus fails on the cassette miss.
- **SC-008**: A private-notebook run produces a report containing no note or question text and creates no files inside the repository.
- **SC-009**: The v1 fixture's hashes are unchanged and all existing retrieval eval tests pass.

## Assumptions

- The corpus is generated from templates and scenario data by a deterministic script (no LLM calls), so the same seed always reproduces it and generation costs nothing. Prose will be more regular than real notes; the scenario spec compensates with varied phrasings, filler paragraphs and distractors, and the limitation is stated in the docs.
- The default bootstrap is 10,000 per-question resamples at a fixed seed with a 95% percentile interval; these are recorded in the report so they can be changed explicitly.
- The paraphrase lexical-overlap threshold and the per-category minimum sample size are set during planning and documented; they must be numeric and checked automatically.
- The human review is performed by a maintainer on a stratified sample (default at least 10% of questions and at least 5 per category). The implementing change ships the review-record format and, where a human review has not yet happened, marks it as pending rather than claiming it has been done; the fixture is "reviewed" only once a maintainer fills the record.
- The v2 cassette needs a live extraction endpoint to record. If the implementing agent cannot make live calls, the recording is a documented maintainer step with the exact command, and the acceptance for that item is met once the cassette is committed (or its storage location documented if it is too large for the repository).
- Sizes in the 300–500-note range are targeted; the exact count and per-category quotas are set in the scenario spec.
- `answer`-origin notes are filed answers in the Liminis sense: their content is derived from the notebook, not independent evidence; they are relevant to questions only where the ground truth says so.
- Retrieval changes themselves (ranking, expansion, reranking) are not part of this issue; v2 measures them.
- Reports remain machine-readable (`report.json`) and human-readable (`report.md`); CI fields are additive so existing consumers of the JSON keep working.

## Out of Scope *(optional)*

- Any change to LCG retrieval, ranking or the qmd configurations; this issue only builds the corpus and the reporting.
- Using real or private notebook content in any checked-in file.
- LLM-generated corpus content or LLM-generated questions (a possible follow-up if template prose proves too regular).
- Publishing comparison numbers; running the comparison is a maintainer step after merge.
- Replacing v1 or removing it from the smoke path.
- Changing the metric definitions or the evidence-judge prompt.
- A CI job that runs v2 (it needs a real embedder; the eval stays on-demand).

## Source References

- #754 (retrieval eval harness), #786 (first run), #787 (`believed_only`)
- `docs/adr/0754-retrieval-eval-architecture.md`, `docs/adr/0752-episode-origin-and-corroboration.md`, `docs/adr/0705-belief-time-separate-from-validity.md`, `docs/adr/0707` (document supersession), ADR-0044 (cassettes)
- `docs/retrieval-quality-evaluation.md`, `crates/eval/fixtures/retrieval/`, `crates/eval/scripts/07-retrieval-eval.sh`
