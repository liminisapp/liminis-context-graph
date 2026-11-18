# ADR-0788: Retrieval Corpus v2 and Bootstrap Reporting — a Generated, Origin-Aware Fixture and Intervals That Label Noise

**Status**: Accepted
**Date**: 2026-11-18
**Issue**: #788 (named for the issue per the repo's ADR numbering rule)

## Context

The first live run of the retrieval eval (#786, ADR-0754) showed the 31-note Fernhill fixture cannot
steer retrieval work. It has 40 judged questions, so one question is 0.025 of recall and a headline
gap of 0.028 nDCG was one to three questions' worth. Direct questions were at the ceiling for both
systems; with three negatives, false-pos@k was 1.0 for every configuration. Every note fits in one
chunk, so chunking, long-note recall and passage choice were never exercised. And it was not shaped
like Liminis Notes: no agent-written pages that restate human facts (ADR-0752), no filed answers,
no clipped articles, no near-duplicates, and only clean explicit supersession.

## Decisions

1. **A second fixture, `crates/eval/fixtures/retrieval-v2/`, beside an unchanged v1.** Same on-disk
   format, extended only with additive fields (`origin`, `cites` on a manifest note; three question
   categories). v1 keeps its content, hashes and behaviour (a test pins its hashes) and remains the
   fast smoke corpus.
2. **Structure is generated deterministically; prose is rendered once by an LLM and checked in
   (scope change 2026-11-18).** `lcg-eval retrieval-gen` turns `scenario.json` (seed, quotas) and
   the fact model in code into `skeleton.json`: entities and facts, which note carries which facts,
   origins, citations, reference times, target sizes and sections, explicit supersessions, and the
   question ground truth. Same scenario and seed give a byte-identical skeleton: hand-rolled
   SplitMix64 with a forked stream per phase, no clock, no hash-map iteration, no model. A separate,
   explicitly invoked `lcg-eval retrieval-render` has an LLM write each note in the voice its origin
   implies and each question's wording (paraphrases included); the output is checked in, with the
   render model and prompt version recorded in `render.json`. CI and normal eval runs never call a
   model. Purely templated prose was rejected because it flatters BM25 and under-tests embeddings and
   paraphrase, the very things the corpus exists to measure. There is **no template fallback for the
   checked-in fixture**: template prose exists only as a test double that lets the pipeline run in
   tests (a test fails if `render.json` names it). Re-rendering is a deliberate maintainer action
   that bumps the corpus and question versions.
3. **A fact model is the single source of truth.** Notes carry facts and questions are derived from
   the same objects, so evidence key facts, negatives and currency are right by construction in the
   skeleton. Because the text is no longer ours, **checks** (`generator/check.rs`) verify the rendered
   prose against the skeleton instead of regenerating it: every fact present (entity and value
   verbatim), evidence findable, passage facts in a later chunk, no foreign entity names, implicit
   revisions not mentioning earlier values, explicit ones stating their effective date, citations
   present, required names in question wording, and the paraphrase and negative checks re-run on the
   rendered text. The manifest and the question ground truth must equal what the skeleton generates;
   only the wording may change.
4. **Origins are declared per note and stamped at ingest.** `ingest` sends `origin` on every chunk of
   a note that declares one; a note with none sends nothing (all of v1). Loader rules: the origin is
   in `primary|external|derived|answer`; `derived` requires `cites`; `cites` is valid only on
   derived/answer notes and must point at earlier manifest notes; and a `derived`/`answer` note may
   not supersede a `primary`/`external` one, because the service refuses it (ADR-0752) and failing at
   load is cheaper than an aborted paid ingest. "All four origins present" is a v2 test, not a loader
   rule, so a private notebook may declare only some.
5. **Implicit contradictions are declared by the question, not the manifest.** The manifest records
   explicit supersession only. A temporal question names its `current` and `stale` notes itself; the
   loader never required a recorded supersession for the pair, and v2 deliberately leaves about half
   unrecorded. v2 also gives some revisions an effective date different from the new note's recording
   date. Explicit supersessions are primary→primary.
6. **New categories.** `multi-hop` (≥ 2 relevant notes), `passage` (exactly one `evidence` entry; the
   fact sits beyond the first chunk of a multi-chunk note) and `negative` (requires the `negative`
   flag). v1's `direct` + flag negatives still load. `evidence` is therefore valid on `answer` and
   `passage` questions. The original four categories are always listed in plans and reports (so v1
   output is unchanged); the new ones appear only when a fixture has questions in them.
7. **Paraphrase is enforced, not hoped for.** On the rendered wording, at most 25 % of a paraphrase question's content words
   (lowercase, longer than three letters, minus glue words) may appear in the fact sentence and at
   most 50 % in the whole note; the skeleton builder already requires it of the plain wording and
   replaces failing candidates from a reserve, and the renderer is told which words to avoid.
8. **Negatives are verified twice, against the skeleton and the rendered text.** A fake entity must appear in no note; a real entity
   asked for an attribute its kind never has must never share a note with that attribute's marker
   words. Near-miss distractors (shared family names and attributes) exist by construction.
9. **Statistics are additive and exactly reproducible.** Percentile bootstrap over questions, 10,000
   resamples, base seed 788, 95 %. Each interval's seed is the base seed XOR an FNV-1a hash of
   `config|category|metric`, so adding a configuration, category or metric changes no other
   interval. Paired comparisons resample the per-question difference, **joined by question id**
   (failed queries drop rows, so index pairing would misalign). A pair whose interval includes zero
   is "not distinguishable from noise" and no winner is named; the hypotheses H1/H2 use the same
   rule (`Reading::{Yes, No, Noise, CannotRead}`; the JSON booleans stay, `null` for noise or
   unreadable, with `*_reading` strings added). Fewer than two values gives `n/a`, never a
   zero-width interval. No multiplicity correction is applied, and the report says so. Metric
   definitions are untouched (ADR-0754).
10. **Constants** (all in `retrieval/stats.rs` and `retrieval/generator/questions.rs`):
    `MIN_CATEGORY_N = 20` (below it a category is flagged "low n"), `CEILING = 0.98` (a scope where
    every ran configuration's nDCG@k point estimate is at or above it is stated to be at the ceiling
    and not ranked), `PARAPHRASE_MAX_FACT_OVERLAP = 0.25`, `PARAPHRASE_MAX_NOTE_OVERLAP = 0.5`. The
    documented per-category minimum for v2 is 30 questions.
11. **Per-origin report definitions** (so the numbers compare across configurations): a *restating
    note* for a question is a `derived` note citing one of its relevant `primary` notes and not
    itself relevant; a question is *eligible* if it is not negative and has one; *restating share@k*
    is the mean fraction of the returned top-k that restate; *outrank rate* is, over eligible
    questions where both appear, how often the best restating note outranks the best primary note;
    *derived-only rate* is how often a restating note was returned without any relevant primary
    note. `QuestionScore` carries the de-duplicated top-k list (not written to the JSON) so this
    needs no second query.
12. **The human review is a human's.** `review.json` is a checked-in template: a seeded stratified
    sample (6 per category) with three empty verdicts per question, the skeleton hash and, after
    rendering, the question-set hash and the question texts. The generator and renderer never write
    verdicts, never overwrite a record marked reviewed, and a test fails if a pending record holds
    verdicts. A reviewed record must have every verdict filled, an error rate that agrees with them,
    and the hash of the rendered questions, so re-rendering forces a re-review. `scenario.json`,
    `skeleton.json`, `render.json` and `review.json` are outside the corpus and question hashes.
13. **Cassette (ADR-0044) and private runs.** v2 ingest is about 680 extraction calls; the maintainer
    records them once and the script replays `<corpus>/cassette.jsonl` by default. A changed corpus
    already fails loudly (a non-dedup cassette miss aborts ingest, naming the note and chunk; a test
    pins it). `--private` is a code guard, not only documentation: it requires explicit `--out` and
    `--judge-cache`, refuses paths inside a git working tree, withholds failed-query detail, and
    prints metrics, counts and hashes only, because the default judge cache stores private text.
14. **`07-retrieval-eval.sh --corpus DIR`** (and `LCG_EVAL_CORPUS`, the flag winning) selects the
    fixture; with neither, v1 as before. An explicit invalid path exits 2 naming it, before anything
    is built.

## Consequences

- v2 can resolve differences of a few points of nDCG and says when it cannot; v1 remains for quick
  checks.
- Any change to the generator or vocabulary changes the skeleton; re-rendering changes the corpus hash
  and invalidates the review and any cassette. The order is: freeze the generator, generate the
  skeleton, render, review, record the cassette.
- LLM prose is more natural than templates but is still one model with one prompt, so the corpus is
  more uniform than a real notebook. The docs say so and describe the private-notebook cross-check.
- Three steps are not done by this change because they need a model key or a human: the render (no
  Anthropic key was available, so the checked-in fixture is the skeleton only and the loader says so),
  the question review (`review.json` is a pending template) and the cassette recording.
- No LCG retrieval, ranking, or service change; no new dispatch method or MCP tool.
