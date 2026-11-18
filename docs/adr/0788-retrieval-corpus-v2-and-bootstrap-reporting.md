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
2. **The corpus is generated, deterministically, by checked-in code.** `lcg-eval retrieval-gen`
   turns `scenario.json` (seed, quotas) plus the vocabulary in code into the notes, manifest and
   questions. No LLM, network or clock is involved, and no hash-map iteration; randomness is a
   hand-rolled SplitMix64 with a forked stream per phase, so output is byte-stable across platforms
   and crate upgrades (`rand`'s stream is not). The generator is library code behind a subcommand so
   a test can run the drift check (`--check`) without a CLI. LLM-written prose was rejected for this
   issue: it would not be reproducible and would cost money on every regeneration; the limitation
   (template prose is more regular than real notes) is documented instead.
3. **A fact model is the single source of truth.** Entities, attribute facts, buried "log" facts and
   decision chains are built first; notes render them and questions and ground truth are read off the
   same objects, so evidence strings are verbatim substrings, negatives are unanswerable, and
   current/stale are right by construction. The generated output is re-validated through the
   production loader before it is returned.
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
7. **Paraphrase is enforced, not hoped for.** At most 25 % of a paraphrase question's content words
   (lowercase, longer than three letters, minus glue words) may appear in the fact sentence and at
   most 50 % in the whole note. Note-side and question-side wordings of every domain, kind and
   attribute share no content word by construction; failing candidates are replaced from a reserve.
8. **Negatives are verified against the corpus.** A fake entity must appear in no note; a real entity
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
12. **The human review is a checked-in record, never faked.** `review.json` holds a seeded stratified
    sample (6 per category) with three verdicts per question, the reviewer, the date, the error rate
    and the question-set version and hash. The generator writes it once as `pending` and never
    overwrites it. A test checks that it matches the question set (ids exist, every category is
    covered, the hash matches, a reviewed record's error rate agrees with its verdicts), so changing
    the questions forces a re-review. `scenario.json` and `review.json` are outside the corpus and
    question hashes.
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
- Any change to the generator or vocabulary changes the corpus hash and invalidates the review and
  any cassette. The order is: freeze the generator, regenerate, review, record the cassette.
- Template prose is more regular than real notes, so lexical systems can look better on v2 than on a
  real notebook. The docs say so and describe the private-notebook cross-check.
- Two steps need a human or a live endpoint and are not done by this change: the question review
  (`review.json` is `pending`) and the cassette recording.
- No LCG retrieval, ranking, or service change; no new dispatch method or MCP tool.
