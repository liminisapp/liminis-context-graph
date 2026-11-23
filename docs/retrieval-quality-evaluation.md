---
layout: default
title: Retrieval-Quality Evaluation
---

# Retrieval-quality evaluation: LCG vs qmd on a notebook corpus

`lcg-eval retrieval` answers a question the extraction eval cannot: **given a real question,
does the read side return the right notes and the right evidence?** It scores LCG's
`knowledge_search_passages`, `knowledge_find_entities` and `knowledge_find_relationships`
against a checked-in question set, and compares them with [qmd](https://github.com/tobi/qmd) —
local markdown search (SQLite FTS5 + vectors, LLM query expansion, RRF, LLM reranker) and the
strongest local stand-in for "raw top-k retrieval".

It is a separate mode of the existing `lcg-eval` binary. The extraction eval
([testing-and-evaluation.md](testing-and-evaluation.md)) is unchanged.

> **Status: no numbers published yet.** The harness and both fixtures (the 31-note v1 smoke corpus
> and the 408-note v2 skeleton, whose prose a maintainer still has to render) are in the repository; results appear here only after a maintainer
> has run the comparison on a real embedder. Until then this page deliberately contains no scores.

> Passage ranking never reads an edge's type, direction or endpoints. To score the extracted graph
> against a fixture's `skeleton.json`, use `lcg-eval graph-integrity`
> ([graph-integrity-evaluation.md](graph-integrity-evaluation.md)).

## What it measures

| Configuration | What runs |
|---|---|
| `lcg-passages` | `knowledge_search_passages` sending neither `mode` nor `belief_aware`: **the service's shipped default** (#806), i.e. belief-aware vector, no FTS and no RRF. It tracks the default, so a change to the default shows up here. The harness removes `LCG_PASSAGE_SEARCH_MODE` and `LCG_PASSAGE_BELIEF_AWARE` from the spawned service's environment so an operator's overrides cannot change what it measures. It is the same request as `lcg-passages-belief` today. |
| `lcg-passages-raw` | *Opt-in (#806), not in the default run.* `mode: "vector"`, `belief_aware: false`: cosine only, the pre-#806 `lcg-passages` baseline, so the belief-aware gain stays measurable. |
| `lcg-passages-variants` | *Extra.* `lcg-passages` (the shipped default, belief-aware vector) plus the question's variants from `query-variants.jsonl` passed as `queries` (#802): one batched embed, one vector probe per query (concurrent), weighted RRF with the primary query weighing 2 and the variants sharing a total of 1. Skipped, not failed, when `query-variants.jsonl` is absent. |
| `lcg-passages-belief-variants` | *Opt-in.* `lcg-passages-belief` plus the same variants: every query runs the vector pipeline, belief-aware ranking runs once on the fused list. Read against `lcg-passages-belief`. |
| `lcg-passages-hybrid-belief-variants` | *Opt-in.* `lcg-passages-hybrid-belief` plus the same variants: every query runs BM25 + vector (so a variant's keywords reach BM25), belief-aware ranking runs once on the fused list. Read against `lcg-passages-hybrid-belief`. |
| `lcg-entities-facts` | `knowledge_find_entities` + `knowledge_find_relationships` (hybrid FTS + vector, RRF), default reads. k is split across the two reads (`ceil(k/2)` entities + `ceil(k/2)` facts) and the ranks are interleaved. |
| `qmd` | `qmd query --no-rerank` (query expansion on). |
| `qmd-rerank` | `qmd query` with its LLM reranker. |
| `lcg-passages-hybrid` | *Opt-in (#789), not in the default run.* `knowledge_search_passages` with `mode: "hybrid"`, `belief_aware: false`: BM25 over episode text + vector, weighted RRF (lexical weight 0.4, vector 1.0). |
| `lcg-passages-hybrid-belief` | *Opt-in (#789).* As above with `belief_aware: true` (the hybrid default): a derived page ranks below the primary note it cites, and a superseded passage below its successor. |
| `lcg-passages-belief` | *Opt-in (#789).* `mode: "vector"` with `belief_aware: true`: the vector ranking plus the same rules. Comparing the `lcg-passages*` rows shows what weighted fusion and the belief rules each add, per category. Unweighted RRF and the top-rank bonus were measured and dropped (ADR-0789). |
| `lcg-entities-facts-belief` | *Extra, clearly labelled variant.* As `lcg-entities-facts`, but facts are read with `believed_only=true`. |

Fact reads carry no `episodes` field: the harness maps each fact to its producing notes through
`belief.producing_episode_uuids`, which the service returns only when a belief parameter is sent. Both
LCG fact configurations therefore send `believed_only` (`false` for `lcg-entities-facts`, which annotates
without filtering). Only the fact read can express belief (ADR-0705); the entity read of
`lcg-entities-facts-belief` is unfiltered and can still credit a superseded note. During ingest the
harness validates every `knowledge_supersede_document` response and aborts, naming both note ids, if a
call ended belief for zero episodes (replays and future-dated supersessions excepted); the report's
ingest block shows "Belief ended for N episode(s) across M document supersession(s)".

The first four are the headline table. The fifth exists because only edge reads can express
belief time ([ADR-0705](adr/0705-belief-time-separate-from-validity.md)): passages and entities
have no belief filter and return superseded text by default, so without it the temporal
hypothesis would test the configuration, not the architecture.

Every configuration runs over the same corpus and question set with the **same k**
(`--k`, default 10). Score floors are set to zero (`min_score=0`, `min_similarity=0`) so a floor
never truncates a result list below k.

For `lcg-entities-facts*` the shared k is a *retrieval budget* split across the entity and fact
reads, so the configuration does not get 2k results against the other configurations' k. One
asymmetry remains by design: an entity maps to every note it was extracted from, so a single
entity result can credit several notes before the list is cut to k notes.

### Metrics

- **recall@k, MRR, nDCG@k** at note level. The ranked note list is de-duplicated (first
  occurrence wins) and cut at k. nDCG uses graded gain `2^gain − 1`.
- **grounded-recall@k / grounded-MRR** for answer questions: a blind LLM judge sees only a *key
  fact* and one returned text and says whether the text supports the fact. It never sees which
  system produced the text, and chunk boundaries do not matter, so neither system's chunker is
  favoured. Finding the right note but returning the wrong passage counts as a note hit and a
  grounded miss.
- **stale>current** and **stale-in-top-k** for temporal questions: whether the best-ranked
  superseded note outranks the best-ranked current note (or appears in the top k at all). Lower
  is better. A stale hit with no current hit counts as stale-above-current.
- **false-pos@k** for negative questions (no relevant note exists): 1 if anything was returned.
  Negatives are *excluded* from recall/MRR/nDCG means. Because every configuration runs with its
  score floor at 0, a system normally returns results for any query, so this column is usually
  pinned at 1.0 for all of them; the report says so ("non-discriminating") rather than marking a
  winner. It only separates systems that return fewer than k results on their own (e.g. qmd).
- **Hits without evidence text.** If a qmd hit carries no snippet there is no passage to judge. It
  is counted as ungrounded (empty evidence; the judge is not called) and the report lists how many
  hits had no snippet per configuration, so qmd's grounded figures are a lower bound rather than
  quietly equal to its note-level recall. There is deliberately no fallback to the whole note.
- **Latency** p50/p95 per query, **warm only**: indices are built explicitly and one unmeasured
  pass runs before the timed pass. qmd runs as one process per query, so its latency is
  per-process (it includes model load) and it gets a single warm-up query rather than a full pass.
- **Query-time model calls.** LCG: one embedder call per read (a fallback `get_edges_by_uuids`
  lookup, if a fact read omits provenance, is counted separately; it should now be 0). qmd does not expose its
  expansion/rerank call counts, so they are reported as `n/a (not exposed)`, never as 0.
- **Ingestion cost** is reported separately: LCG ingest wall time and number of extraction
  requests (each is at least one LLM call); qmd index time.

### Confidence intervals and noise (#788)

The first live run on the small v1 corpus (#786) showed why point estimates are not enough: with
40 judged questions a single question moves recall by 0.025, so a headline gap of 0.028 nDCG was
one to three questions' worth and could not be told from noise. The report therefore carries:

- **A 95% bootstrap interval on every headline metric** (recall@k, MRR, nDCG@k, grounded-recall@k,
  stale>current, false-pos@k), overall and per category. The method is a *percentile bootstrap
  over questions*: resample the scored questions with replacement 10,000 times, take the mean of
  each resample, and report the 2.5th and 97.5th percentiles. Each metric is resampled over the
  questions where it is defined (negatives are not part of nDCG; only temporal questions have
  stale>current), so `n` differs by metric.
- **Paired comparisons.** For every pair of configurations that ran, the difference in per-question
  nDCG@k (a − b) gets its own bootstrap interval, joined **by question id** (a configuration with
  failed queries has fewer rows, so positional pairing would be wrong). If the interval includes
  zero the row reads **"not distinguishable from noise"**; a winner is only named when the top
  configuration's paired interval is wholly above zero against every other configuration. The JSON
  carries the same comparison for recall@k. No multiplicity correction is applied across the many
  pairs shown; treat one isolated "significant" pair among many with suspicion.
- **Hypotheses H1/H2** use the same rule: the best `lcg-*` and best `qmd*` configuration (by point
  estimate) are compared on the category's nDCG@k, and the reading is *YES*, *NO* (the opposite is
  established), *not distinguishable from noise*, or *cannot be read*.
- **Low-n and ceiling labels.** A category with fewer than 20 questions is flagged "low n", and its
  (visibly wide) interval is printed anyway. When every configuration scores nDCG@k ≥ 0.98 in a
  scope, the report says it is **at the ceiling** and does not rank the configurations there.
  Where fewer than two questions are scored the interval is `n/a`, never a zero-width interval.
- **Reproducibility.** The seed (default 788), resample count and level are printed in the report
  and in `report.json` (`bootstrap`). Each interval is seeded from the base seed XOR an FNV-1a hash
  of `config|category|metric`, so adding a configuration changes no other interval, and two runs
  over the same per-question scores are byte-identical. Change them with `--bootstrap-seed` and
  `--bootstrap-resamples` (e.g. `--bootstrap-resamples 1000` for a quick look).

Bold in the tables marks the best point estimate. It is not by itself a finding; read the paired
comparison beneath each table.

### By origin of returned notes (fixtures that declare origins)

Real notebooks contain agent-maintained pages that restate human notes. Origin
([ADR-0752](adr/0752-episode-origin-and-corroboration.md)) is stamped on each note at ingest, so a
`derived` page cannot corroborate itself. When a fixture declares origins, the report adds a section
that shows how each configuration treats those restatements. The definitions are fixed so the
numbers are comparable between configurations:

- A *restating note* for a question is a `derived` note whose `cites` contains one of the question's
  relevant `primary` notes and which is not itself in the question's `relevant_notes`.
- A question is *eligible* if it is not a negative and has at least one restating note.
- **Restating share@k**: the mean, over eligible questions, of the fraction of the returned top-k
  that are restating notes.
- **Outrank rate**: over eligible questions where both a relevant primary note and a restating note
  were returned, how often the best-ranked restating note sits above the best-ranked primary note
  (the pair count is shown).
- **Derived-only rate**: over eligible questions, how often a restating note was returned and no
  relevant primary note was.
- **Returned top-k by origin**: the share of all returned results that are `primary`, `external`,
  `derived`, `answer` or undeclared, over non-negative questions.

Each rate carries a bootstrap interval. v1 declares no origins, so it never prints this section.

### Result mapping (identical for every configuration)

- Passage → note: `source_description` is `"{note}:{chunk}"`.
- Entity → notes: every note whose episodes mention the entity (built once at ingest).
- Fact → notes: the fact's *producing* episodes (`episodes`). ADR-0012's either-endpoint
  inference is never used.
- qmd result → note: the result path's file stem.

## Query variants (`lcg-passages-variants`)

A paraphrase question describes a thing instead of naming it, so it shares few words with the answer
note. An agent calling `knowledge_search_passages` can supply its own rewrites as `queries`
([IPC reference](ipc-mcp-reference.md)). The eval simulates that agent from a fixture,
`query-variants.jsonl`, with up to 5 variants per question id. An LLM writes them seeing **only the
question text** (no corpus, notes or relevance labels) under the prompt committed beside the fixture,
`query-variants.prompt.md`. It is a one-time maintainer step, never run in CI:

```bash
lcg-eval retrieval-variants --dir crates/eval/fixtures/retrieval-v2 --export-prompts prompts.jsonl
# write each reply (a JSON array of up to 5 strings) to <replies>/<key>.md
lcg-eval retrieval-variants --dir crates/eval/fixtures/retrieval-v2 --replies <replies>
lcg-eval retrieval-variants --dir crates/eval/fixtures/retrieval-v2 --check
```

The fixture is stamped with the questions hash; the eval refuses a stale one. **Leak check:** a variant that
contains, verbatim, the title (first `# ` heading, whole or without `Notes on ` / `Q&A: `) or a key-fact
string of any note relevant to its question fails, naming the question and variant. Matching is
case-insensitive, needles under 8 characters are skipped, and text the question itself already contains is not
a leak. It catches verbatim leaks only, not paraphrased ones. With no `query-variants.jsonl`, the
`lcg-passages-variants` row is reported as skipped.

The committed v2 fixture (244 questions, 1,194 variants) was written by Sonnet 4.6 from each question's text
alone through this exchange; `--check` finds no leaks. About one variant in ten is a hypothetical answer with a
placeholder (`[date]`, `[Name]`), which adds a noise token and will not match note text.

That fixture **predates the current `query-variants.prompt.md`**, which since added the rules to keep every
named entity and to send no placeholders. The fixture does not follow them (about 41% of multi-hop variants
omit the anchor entity), and it is not reproducible from the committed prompt, because replies are keyed by a
hash of the system prompt. It will be regenerated and re-measured with #797's composite search.

Every variants config sends the *same request as its base config* plus `queries`, so each query runs that
pipeline (vector, or BM25 + vector) and the per-query rankings are fused (primary weight 2, the variants
sharing a total of 1). The report's "Query variants vs `<base>`" section, one per variants config
(`lcg-passages-variants` against `lcg-passages`, `lcg-passages-hybrid-belief-variants` against
`lcg-passages-hybrid-belief`), gives per-category paired nDCG@k differences with intervals, p50/p95, and the
merge-bar reading: paraphrase interval above zero, no category wholly below zero. Latency is reported, not gated: variants are deliberate agent searches.

First v2 result (vector-only, per-variant weight 1, head `a0c7e95f`): variants lowered overall nDCG@10 from
0.568 to 0.522 (paraphrase 0.150 to 0.085, multi-hop 0.662 to 0.552, p50 24 ms to 141 ms). That is why variants
now run the request's whole pipeline, share a fixed weight budget and fuse concurrently (ADR-0802).

Second v2 result (head `87c85525`, that design): overall +0.011 (vector) and +0.017 (hybrid), within noise;
passage +0.117 and +0.112, hybrid paraphrase +0.082 [+0.017, +0.153], and multi-hop -0.062 [-0.107, -0.026]
(vector) and -0.059 [-0.097, -0.027] (hybrid). The multi-hop loss is a known regression with the committed
fixture, caused mostly by correlated variants voting together for generic distractors; entity-preserving
variants are the planned mitigation (ADR-0802).

## Caveats the report repeats

- LCG and qmd use **different embedding (and reranker) models**, so this compares systems, not
  retrieval algorithms.
- Both corpora are synthetic. v1 is small and hand-built (results are indicative, not general). v2
  is larger and synthetic: its structure is generated and its prose is written by an LLM, so it is
  **more uniform than a real notebook's** and direct questions may sit near the ceiling. The
  report prints the matching caveat, and says explicitly when a category is at the ceiling.
- LCG ingestion is only deterministic with a replayed extraction cassette; qmd's expansion and
  reranking depend on their models. Relevance metrics are exactly reproducible for a fixed LCG
  graph and embedder.
- The hypotheses ("LCG beats qmd on temporal/superseded questions"; "qmd beats LCG on
  paraphrase questions") are read from the data with the paired-interval rule above (yes, no, not
  distinguishable from noise, or cannot be read). They are not assumed.
- Temporal questions about a decision revised by a plain later note (no supersession) measure
  retrieval-level recency handling only: LCG's supersession state cannot help there, so those
  questions can favour systems that rank by text alone. That is the intended signal.

## The fixtures

Two fixtures live side by side. Both are **synthetic**, contain no real or private data, and are
versioned: the report prints each corpus and question-set version and content hash.

| | v1 `crates/eval/fixtures/retrieval/` | v2 `crates/eval/fixtures/retrieval-v2/` |
|---|---|---|
| Purpose | Fast smoke corpus; frozen | Realistic corpus that can resolve differences |
| Corpus | `fernhill-notebook-1`, 31 notes, ~1,200 words, hand-written | `meridian-notebook-2`, 408 notes, about 155,000 target words, structure generated and prose LLM-rendered |
| Questions | 43 (40 judged, 3 negative) in four categories | 244 in seven categories (≥ 30 each) |
| Origins | none declared | primary 286, external 36, derived 56, answer 30 |
| Chunks at 900 tokens | one per note | 63 long notes are sectioned (254 sections, each its own chunk or more) |

```text
manifest.json         corpus_version, chunking, notes in ingest order (id, reference_time, and in v2
                      origin + cites), supersedes relations (explicit supersession only)
notes/<id>.md         one markdown note per manifest entry
questions.jsonl       one question per line
questions.meta.json   {"questions_version": "..."}
scenario.json         (v2) the generator input: seed and quotas
skeleton.json         (v2) generated: everything except the prose (facts, origins, citations,
                      sizes, supersessions, question ground truth)
render.json           (v2) written by the render step: render model, prompt version, skeleton hash
review.json           (v2) the human-review record for a stratified sample of the questions
```

Note ids are lowercase slugs (`[a-z0-9-]`) because they double as the file names qmd indexes.
`scenario.json`, `skeleton.json`, `render.json` and `review.json` are not part of the corpus or question hashes.

### v1 (smoke)

`fernhill-notebook-1` is a notebook for a fictional bike-share co-operative with 8 decisions that
a later note explicitly replaces. It stays as a fast smoke test: every note fits in one chunk, so it
cannot exercise chunking, long-note recall or passage choice, and with 40 judged questions one
question is 0.025 of recall. Its content and hashes never change.

### v2 (realistic)

`meridian-notebook-2` is a fictional cooperative's notebook shaped like a Liminis workspace:

- **Size mix (target words):** 302 notes under 250 words, 22 of 250–500, 78 of 500–2,000 and 6
  over 5,000. The long notes are weekly digests and journals with a handful of facts buried
  in otherwise generic text, so **choosing the right passage inside a note** is exercised.
- **Origins:** human notes (`primary`), clipped articles (`external`, some carrying a vendor's
  public list price), agent-maintained entity and decision pages (`derived`, each citing the notes
  it restates), and filed answers (`answer`). 56 derived pages restate facts from primary notes, so
  "the wiki citing itself" is present; some lag and restate a stale decision.
- **Near-duplicates:** each entity's facts are restated by its derived page and, for 16 facts, by a
  primary standup recap. Ground truth lists the recap at gain 1 beside the source at gain 2; the
  derived restatements are never relevant (the per-origin report shows how often they outrank the
  source). The generator completes every non-negative question's relevant set from the skeleton: any
  other non-derived, non-stale note that states one of the question's basis facts (any hop, for
  multi-hop) is added at gain 1, so a system returning a correct near-duplicate (a recap, a digest, a
  filed answer) is not scored wrong for it. The check step fails a rendered fixture in which such a
  note is left out.
- **Time:** 26 decisions, each revised 2–4 times (3–5 notes), with explicit
  `knowledge_supersede_document` revisions (45 supersessions) and plain later notes that contradict
  the earlier one without superseding it. Several effective dates differ from the recording date.
  Explicit supersessions are primary→primary only, because ADR-0752 refuses a derived page
  superseding a primary note.
- **Questions** (target and minimum 30 per category):

  | Category | Count | What it asks |
  |---|---|---|
  | `direct` | 40 | A fact stated in one note. 6 are about a clipped article's figure. |
  | `paraphrase` | 40 | The same kind of fact, asked without lexical overlap with the source (see below). |
  | `temporal` | 40 | The current value of a revised decision (20 chains end in an explicit supersession, 20 in an implicit contradiction; 14 are re-asked with different wording). |
  | `answer` | 30 | A fact with a key-fact `evidence` entry for grounded judging; the filed answer is a gain-1 relevant note for half of them. |
  | `multi-hop` | 30 | Needs two notes: a project's supplier then the supplier's attribute (15), or compare two projects' budgets (15). |
  | `passage` | 30 | One fact buried in a paragraph of a long note, always beyond the note's first chunk, with the key fact as `evidence`. |
  | `negative` | 34 | No note answers. 17 ask about an entity that does not exist but is named like ones that do (`Cedar Labs` beside `Cedar Core`); 17 ask a real entity for an attribute its kind never has. |

  At 244 questions a single question moves recall by at most 0.0125 in aggregate (v1: 0.025).

**Paraphrase rule.** The checks count a question's content words (lowercase, longer than three
letters, minus glue words like *what* and *does*) and requires at most **25 %** of them to appear in
the fact sentence and at most **50 %** in the whole relevant note. The skeleton builder replaces a
candidate whose plain wording fails from a reserve; the same bound is checked again on the rendered
wording and the rendered note. Note-side and question-side wordings of
every domain, kind and attribute share no content word by construction (*billing* vs *invoicing and
payment collection*).

**Negatives are unanswerable by construction and verified.** A fake entity is checked to appear in
no note; a real entity asked for an attribute its kind never has is checked never to share a note
with that attribute's marker words. Both are checked against the skeleton when it is built and
again against the rendered text. The near-miss distractors (same family name, same attribute)
are present in the corpus.

**Limitation.** An LLM writes the prose, so notes read naturally and paraphrase questions are not
template paraphrases, but one model with one prompt still writes everything: the corpus is more
uniform than a real notebook, and direct questions may be at the ceiling for both systems. That is
reported, with intervals, not hidden. Use the private-notebook run below as a cross-check.

### How v2 is built: skeleton, then render, then check

v2 is built in two steps so that structure is reproducible and prose is natural.

1. **Skeleton (deterministic, free).** `lcg-eval retrieval-gen` turns the checked-in `scenario.json`
   (seed and quotas) and the fact model in code into `skeleton.json`: entities and facts, which note
   carries which facts (and, for long notes, in which section), each note's origin, reference time,
   target length and citations, the explicit supersessions with effective dates, and every
   question's ground truth (relevant notes, gains, evidence key facts, current/stale). Same
   scenario and seed give a byte-identical skeleton (hand-rolled SplitMix64, no clock, no network,
   no model).
2. **Render (explicit, paid, once; a maintainer step).** `lcg-eval retrieval-render` has an LLM write
   every note in the voice its origin implies (journal, decision record, standup recap, clipped
   article, agent topic page with citations, filed answer) and every question's wording, paraphrase
   questions included. It writes `manifest.json` (generated from the skeleton), `notes/*.md`,
   `questions.jsonl`, `questions.meta.json` and `render.json` (render model, prompt version, skeleton
   hash). **The rendered output is checked in; CI and normal eval runs never call a model, and there
   is no template fallback for the checked-in fixture.** A long note is rendered section by section
   so each fact lands in the section the skeleton assigned it.
3. **Checks (free, in `cargo test` and `retrieval-gen --check`).** They verify the checked-in text
   against the skeleton rather than regenerating it: every skeleton fact appears in its note (entity
   name and value verbatim); evidence key facts are findable in their notes; a passage fact sits in
   a later chunk of a multi-chunk note; a note names no entity other than its own; an implicit
   revision does not mention the earlier value and an explicit one states its effective date;
   derived and answer pages carry their `[[citations]]`; questions keep the names they must (and a
   paraphrase names none); the paraphrase lexical-overlap bound holds on the *rendered* wording
   against the sentence carrying the fact and the whole note; and negatives remain unanswerable.
   `manifest.json` and the question ground truth must equal what the skeleton generates (only the
   wording may change), and `render.json` must name the current skeleton.

```bash
# 1. skeleton (deterministic; a pending review.json template is written only when absent)
target/release/lcg-eval retrieval-gen \
  --spec crates/eval/fixtures/retrieval-v2/scenario.json --out crates/eval/fixtures/retrieval-v2
# check the skeleton against the scenario, and (once rendered) the text against the skeleton
target/release/lcg-eval retrieval-gen --check \
  --spec crates/eval/fixtures/retrieval-v2/scenario.json --out crates/eval/fixtures/retrieval-v2
# 2. render (needs ANTHROPIC_API_KEY; ~630 model calls; --dry-run prints the count and needs no key)
ANTHROPIC_API_KEY=… target/release/lcg-eval retrieval-render --dir crates/eval/fixtures/retrieval-v2
# re-render only what the checks flagged
target/release/lcg-eval retrieval-render --dir crates/eval/fixtures/retrieval-v2 --only note-id,question-id
```

`--model` (default the judge model) is recorded in `render.json`. A crashed run continues with
`--resume`. **Re-rendering changes the corpus: bump `corpus_version` and `questions_version` in
`scenario.json`, regenerate the skeleton, then render.** A change to the skeleton makes a test fail
until `skeleton.json` is regenerated. The order is: **freeze the generator → generate the skeleton →
render → review → record the cassette.**

**Current status.** No model key was available when this was implemented, so **the checked-in v2 is
the skeleton only: it has not been rendered**, and `lcg-eval retrieval --corpus …/retrieval-v2`
stops with a message saying so. Rendering, the question review and the cassette are maintainer
steps. The Rust tests exercise the whole pipeline (checks, loader, ingest, report) against a
template-prose test double built in a scratch directory; that prose is never checked in, and a test
fails if a checked-in `render.json` names it.

### Human review of the questions

`review.json` is a template that lists a seeded, stratified sample (6 questions per category, 42 in all, at least 10 %
of the set) with three verdicts per question: *ground truth correct*, *note relevance correct*, *key
fact correct*, plus the reviewer, date, error rate and the question-set version and hash. **The
checked-in record is `pending` with empty verdicts: no human has reviewed it, and neither the
generator nor the renderer ever writes verdicts.** After rendering, `retrieval-render` fills
`questions_hash` and each sampled question's text (only while the record is pending; a record marked
reviewed is never modified) so the reviewer can read the questions in one place. The fixture counts as reviewed only
once a maintainer fills every verdict, sets `status` to `reviewed`, and records `reviewer`,
`reviewed_on` and `error_rate` (the share of sampled questions with any `false` verdict). If the rate
exceeds `error_rate_threshold` (default 5 %), correct or regenerate the affected categories and
re-review. A test checks that the sampled ids exist, every category is covered, the record names the current
skeleton and (once rendered) the question set's hash, that a pending record has no verdicts, and
(when reviewed) that every verdict is filled and the error rate agrees with them.

### Recording the v2 cassette (free re-runs)

Ingesting v2 means one `knowledge_process_chunk` call per chunk (several hundred once rendered; the
dry run prints the exact number), each at least one extraction-LLM call.
Record them once and replay forever (cassettes: [ADR-0044](adr/0044-llm-cassette-record-replay-seam.md)):

```bash
# once the prose is rendered, with a local or live extractor and a real embedder
LCG_EVAL_EMBEDDER_HTTP=http://127.0.0.1:8080 \
LCG_EVAL_RECORD_CASSETTE=crates/eval/fixtures/retrieval-v2/cassette.jsonl \
  crates/eval/scripts/07-retrieval-eval.sh --corpus crates/eval/fixtures/retrieval-v2
```

After that, `07-retrieval-eval.sh --corpus crates/eval/fixtures/retrieval-v2` replays
`<corpus>/cassette.jsonl` automatically (or set `LCG_EVAL_REPLAY_CASSETTE`) and makes **zero
extraction requests**. The cassette is keyed by extraction input, not by corpus hash, so if the
corpus changes, the changed chunks **fail loudly with a cassette miss that names the note and
chunk** and the run stops; nothing falls back to a live model. The cassette is recorded against the
corpus hash the report prints (hash in the report header, after rendering); write that hash
next to the file when committing it. Expect a few MB; if it is too large for the repository, keep it
in external storage and set `LCG_EVAL_REPLAY_CASSETTE` instead of using the sidecar path.
**The recording needs a rendered corpus and an extraction endpoint (a local extractor is fine); it is a maintainer step and has not been done yet.**

### Question format

```json
{"id": "temporal-01", "category": "temporal", "text": "Which company hosts the booking API?",
 "relevant_notes": [{"note": "decision-hosting-tern", "gain": 2}],
 "current": ["decision-hosting-tern"], "stale": ["decision-hosting-harbor"]}
```

| Field | Rules |
|---|---|
| `id` | Required, unique. Every load error names it. |
| `category` | Exactly one of `direct`, `paraphrase`, `temporal`, `answer`, `multi-hop`, `passage`, `negative`. |
| `text` | The query, verbatim. |
| `relevant_notes` | `[{note, gain}]`, `gain` 1–3 (default 1). Notes must exist in the manifest. Empty only with `"negative": true`. |
| `negative` | `true` for a question no note answers. Scored on false-positive rate. Required with category `negative`; v1 uses `direct` plus the flag, which still loads. |
| `evidence` | **Answer and passage questions only.** Required on answer questions; a passage question has exactly one. `[{note, key_fact}]`: `key_fact` is a short sentence the judge checks against returned text. The note must be in `relevant_notes`. |
| `current` / `stale` | **Temporal questions only**, both required. `current` ⊆ `relevant_notes`; `stale` must not be relevant. The question is the ground truth: the loader does **not** require the manifest to record a supersession for the pair (v1 happens to record every pair; v2 deliberately leaves about half unrecorded, the "plain later note" case). |
| `multi-hop` | At least two `relevant_notes`; both facts are required. Partial coverage scores below full coverage under graded gain. |

Manifest note entries may also carry `origin` (`primary`, `external`, `derived`, `answer`) and
`cites` (earlier notes a `derived` page restates; required on `derived`, optional on `answer`,
invalid elsewhere). The loader rejects an unknown origin, a citation of a missing or later note,
and an explicit supersession of a `primary`/`external` note by a `derived`/`answer` one
(ADR-0752 would refuse it at ingest). A note with no `origin` ingests exactly as before.

Paraphrase questions deliberately share little vocabulary with the relevant text.

### Adding a question (v1 only; v2 is generated)

Do not edit the v2 files by hand: change `scenario.json` or the generator, regenerate the skeleton and render.

1. Add a line to `questions.jsonl` in the format above; pick a unique `id`.
2. If it needs new content, add `notes/<id>.md` and a manifest entry (in ingest order, with a
   `reference_time`); for a replaced decision also add a `supersedes` entry
   (`old_note`, `new_note`, `effective_at`). Bump `corpus_version` / `questions_version`.
3. Run `lcg-eval retrieval --dry-run`. A malformed entry, an unknown category, or a reference
   to a note that is not in the corpus fails the load and names the question.

## Running it

Not run in CI: it needs a real embedder and, for the qmd columns, an out-of-band qmd.

### 1. Install qmd (out of band — not a dependency of any lcg crate)

```bash
npm install -g @tobilu/qmd        # or: bun install -g @tobilu/qmd
qmd --version
```

The eval drives qmd only through its CLI, using a named index so your own qmd index is
untouched:

```text
qmd --index <name> collection add <corpus>/notes --name <name>
qmd --index <name> update
qmd --index <name> embed
qmd --index <name> query --json -n <k> [--no-rerank] "<question>"
```

qmd downloads its models on first use (`embed` and `query`); if that is impossible the qmd
configurations are reported as **skipped with the reason**, never as zero scores, and the LCG
configurations still run. The qmd version and `qmd status` summary are recorded in the report.

### 2. Build and start from a real embedder

```bash
cargo build --release -p lcg-eval -p lcg-service
# any OpenAI-compatible embedding endpoint (or --embedder-uds <socket>):
export LCG_EVAL_EMBEDDER_HTTP=http://127.0.0.1:8080
```

The eval refuses to run LCG configurations without an explicit embedder (or
`LCG_EMBEDDING_URL`): mock and hash embedders produce meaningless vectors.

### 3. Preview, then run

```bash
DRY_RUN=1 crates/eval/scripts/07-retrieval-eval.sh   # resolved plan: configs, k, hashes; spends nothing
crates/eval/scripts/07-retrieval-eval.sh             # v1 (default): report.md + report.json in ./retrieval-report

# choose the fixture with --corpus (or LCG_EVAL_CORPUS; the flag wins)
DRY_RUN=1 crates/eval/scripts/07-retrieval-eval.sh --corpus crates/eval/fixtures/retrieval-v2
crates/eval/scripts/07-retrieval-eval.sh --corpus crates/eval/fixtures/retrieval-v2
```

Without `--corpus` the script runs v1, as before. An explicit `--corpus` that is not a fixture
directory fails with exit 2, naming the path, before anything is built or spent. The dry run of v2
prints its note count, size mix, origin counts, per-category question counts and both hashes.

Or directly:

```bash
target/release/lcg-eval retrieval \
  --service-bin target/release/liminis-context-graph \
  --embedder-http http://127.0.0.1:8080 \
  --k 10 --out retrieval-report
```

| Flag | Meaning |
|---|---|
| `--corpus <DIR>` | Fixture directory (default `crates/eval/fixtures/retrieval`, v1; the full corpus is `crates/eval/fixtures/retrieval-v2`). |
| `--k <N>` | Shared cut-off, 1–100 (default 10). |
| `--configs <LIST>` | Subset of the six configuration ids. |
| `--service-bin`, `--embedder-http/--embedder-uds`, `--extractor-http/--extractor-uds` | How the spawned LCG service is configured. |
| `--embedding-profile <LIST>` | Embedding [instruction profile(s)](configuration.md#embedding-instruction-profiles) to measure, e.g. `none,bge-v1.5`. See [Comparing embedding profiles](#comparing-embedding-profiles). |
| `--replay-cassette <PATH>` / `--record-cassette <PATH>` | Replay or record ingest extraction ([cassettes](testing-and-evaluation.md#recordreplay-cassettes)) for a free, repeatable LCG ingest. |
| `--qmd-bin`, `--qmd-index` | qmd executable and named index. The default index name includes the corpus hash (`lcg-eval-<corpus_version>-<hash8>`), so a changed corpus gets a fresh index. qmd keeps a collection's original directory: if you pass your own `--qmd-index` and reuse it after changing `--corpus` or editing notes, the run warns, and you must remove the old collection or pick a new name. |
| `--judge-model`, `--judge-cache`, `--no-grounded` | Grounded-evidence judge (needs `ANTHROPIC_API_KEY`); verdicts are cached so re-runs cost nothing. Without a key the grounded columns are **skipped**, not zero. |
| `--out <DIR>` | Write `report.md` and `report.json` (and `compare.md` / `ranked.jsonl` when those flags are used). A stale `compare.md` or `ranked.jsonl` left by an earlier run in the same directory is removed first, so the directory never mixes runs. |
| `--compare <FILE>` | Compare against an earlier `report.json`; see [Comparing two runs](#comparing-two-runs). |
| `--strict` | With `--compare`, also refuse a run whose soft inputs differ. Requires `--compare`. |
| `--record-ranked` | Write `ranked.jsonl` beside the report; see [Per-question ranked lists](#per-question-ranked-lists). Requires `--out`; refused with `--private`. |
| `--bootstrap-seed <N>`, `--bootstrap-resamples <N>` | Confidence-interval settings (default seed 788, 10,000 resamples). Recorded in the report. |
| `--private` | Private-notebook run; see below. |
| `--dry-run` | Print the resolved plan and exit. |

The extraction backend for LCG ingestion (live Anthropic, a local OpenAI-compatible endpoint,
or a cassette) and the ontology are whatever the spawned service uses; the report records the
choice.

## Input digests (`inputs`)

Every `report.json` carries a top-level `inputs` block (schema version 1) so a later reader can tell
whether two runs measured the same thing. Each entry is a full-length sha256 hex string in JSON (the
Markdown report's `## Inputs` table shows the first 16), or one of two literals: `none` (the run does
not use that input) and `unknown` (it does, but its identity could not be determined). The existing
`meta.corpus_hash` / `meta.questions_hash` are unchanged and are the first 16 hex of
`inputs.corpus` / `inputs.questions`.

| Entry | What is digested |
|---|---|
| `corpus`, `questions` | The fixture, with the same length-prefixed sha256 as `meta.*_hash`. |
| `variants` | Bytes of `<corpus>/query-variants.jsonl`; `none` unless a `*-variants` config ran. |
| `cassette` | Bytes of the `--replay-cassette` / `--record-cassette` file (after ingest, so a recorded cassette is hashed once written); `none` without one. The `.meta.json` sidecar is not hashed. |
| `embedder_model` | The model name, dimension and effective instruction profile the service reports in `knowledge_status`, not the socket path or URL. `unknown` when the service reports no model; `none` for a qmd-only run. With several `--embedding-profile` values, one digest over the sorted set, so profile order does not matter. |
| `extractor` | A path-free identity: `cassette-replay`, the endpoint with credentials stripped, or the service default. The cassette's path and contents belong to `cassette` alone. |
| `service_binary` | Streamed sha256 of the resolved `--service-bin` (a bare name is looked up on `PATH`); the `--version` string if the file cannot be read; else `unknown`. |
| `service_env` | The `LCG_*` variables, and the legacy `GRAPHITI_*` spellings the service still reads as fallbacks, in the spawned service's *effective* environment (the inherited one after the eval's own removals and settings, e.g. `LCG_PASSAGE_*`, `LCG_DB_PATH`). Names ending `_API_KEY`, `_TOKEN`, `_SECRET` or `_PASSWORD`, and names containing `URL` or `ENDPOINT`, contribute their name only, so no credential is hashed into a shared report (a changed key or URL is therefore not detected). `LCG_REPLAY_LLM` / `LCG_RECORD_LLM` are left to `cassette`. `service_env_names` lists the sorted names so a mismatch is explainable. |

`--private` runs record the digests too: they are hashes, not content.

## Comparing two runs

```sh
lcg-eval retrieval --corpus fixtures/retrieval-v2 ... --out run-b --compare run-a/report.json
```

`--compare` reads the earlier report before anything is started or spent, runs the new evaluation,
writes this run's `report.md` / `report.json`, and then judges comparability (so a refusal never
discards a finished run). It prints per-config and per-category nDCG@k deltas, and writes them to
`compare.md` beside the report when `--out` is set.

| Kind | Inputs | When they differ |
|---|---|---|
| Hard | `inputs.corpus`, `inputs.questions`, `k` | Exit 2 naming **every** differing hard input. No deltas are printed and no `compare.md` is written. |
| Soft | `variants`, `cassette`, `embedder_model`, `extractor`, `service_binary`, `service_env`, `meta.lcg_build` (the service build), `meta.ontology_hash` | The deltas are printed under a banner listing each changed input. With `--strict`, exit 2 instead. |

`none` equals `none`. `unknown` against a known value is a difference; `unknown` against `unknown` is
a gap rather than a change, except under `--strict`, where an unidentifiable input always counts as a
mismatch. A config (or category) present in one report only, or skipped in one, shows `n/a` for the
missing side and is not itself a mismatch.

An earlier report with no `inputs` block (a run from before this existed) is refused with exit 2 and a
message to re-run it; it is never compared silently. A `--compare` file that cannot be read or is not a
retrieval report exits 1. `--strict` without `--compare` is a usage error. `--private` runs may use
`--compare`. The comparison policy is recorded in
[ADR-0872](adr/0872-retrieval-run-comparability.md).

Exit codes of `lcg-eval retrieval`: 0 success; 1 runtime error (including an unreadable or invalid
`--compare` file); 2 usage error, or incompatible inputs under `--compare` (including an earlier
report with no `inputs`). A run without `--compare` behaves as before.

## Per-question ranked lists

`--record-ranked` writes `ranked.jsonl` beside the report: one line per config and scored question,

```json
{"config":"lcg-passages-hybrid","question":"para-03","category":"paraphrase",
 "hits":[{"rank":1,"note":"policy-marlin","uuid":"…","score":0.031,"similarity":0.82,"legs":"both"}]}
```

`hits` are the service's passages in response order, before the note de-duplication the metrics use.
`score` is the config's own ranking score, `similarity` is present only in hybrid mode (absent for a
lexical-only hit), and `legs` (`dense`, `lexical` or `both`) is present when the service supplies it
(hybrid mode, service build with #871 or later); a field the service did not supply is omitted, never
`null`. Evidence text is not written. Only `lcg-passages*` configs write lines (entities/facts and qmd
expose no per-hit scores). A question that returned no passages has an empty `hits` list, and a
question whose query failed is absent, so the dense (`lcg-passages-raw`) and fused
(`lcg-passages-hybrid`) lines of one run join on `question` with no gaps and show where fusion changed
the top-k. The flag needs `--out` and is refused with `--private` (a usage error), since the file
holds per-question note pointers and scores.

## Comparing embedding profiles

Asymmetric embedders (BGE, nomic, e5) are trained with role prefixes. `--embedding-profile` runs
the LCG configurations under each named profile so the report shows the difference side by side:

```sh
# Bundled sidecar (BGE): none vs bge-v1.5
lcg-eval retrieval --embedder-uds /tmp/liminis-inference.sock \
  --configs lcg-passages,lcg-entities-facts --embedding-profile none,bge-v1.5 --out report-bge

# Ollama (nomic): none vs nomic-v1.5
lcg-eval retrieval --embedder-http http://127.0.0.1:11434/v1/embeddings \
  --configs lcg-passages,lcg-entities-facts --embedding-profile none,nomic-v1.5 --out report-nomic
```

With more than one profile, each LCG configuration appears once per profile as
`<id>@<profile>` (for example `lcg-passages@none` and `lcg-passages@bge-v1.5`), and every profile
runs in its **own fresh workspace with a fresh ingest** — a document-prefix change means a
re-embed, so vectors are never reused across profiles. (This is the same rebuild a real workspace
needs; see the [re-embed rule](configuration.md#when-you-need-to-re-embed).) qmd configurations
run once. With one profile the ids are unchanged and the profile is recorded in the report's
embedder line. `--dry-run` prints the expanded rows. When `--embedding-profile` is given, the harness
removes any exported `LCG_EMBEDDING_QUERY_PREFIX` / `LCG_EMBEDDING_DOCUMENT_PREFIX` from the spawned
service's environment, so each row measures exactly the named profile.

These are on-demand runs: they need a real embedder, an extraction backend (a cassette makes the
ingest free and repeatable) and, for the grounded columns, a judge key. Differences on a small
question set are within noise; read them with the confidence caveats above.

## Comparing the passage stages (rerank and query expansion)

Four opt-in configurations measure the optional stages of `knowledge_search_passages`
([ADR-0796](adr/0796-passage-rerank-and-query-expansion.md)); each is switchable and measured on its own:

| Config | Stages |
|--------|--------|
| `lcg-passages-rerank` | rerank only |
| `lcg-passages-expand` | query expansion only |
| `lcg-passages-belief-rerank` | rerank over a belief-aware first stage (`belief_aware: true` with `rerank: true`) |
| `lcg-passages-expand-rerank` | expansion, RRF fusion, then rerank |

Since #806 `lcg-passages` is the shipped belief-aware default, while `lcg-passages-rerank`, `-expand` and
`-expand-rerank` pin a raw cosine first stage (`belief_aware: false`); `lcg-passages-raw` is the matching
un-staged baseline and `lcg-passages-belief-rerank` is the stage over the belief-aware first stage. The
report's *Passage stages* section pairs each stage with the baseline that has the same first stage
(`lcg-passages-raw`, or `lcg-passages-belief` for `lcg-passages-belief-rerank`) when that baseline ran, and falls back to
`lcg-passages` otherwise, so include `lcg-passages-raw` (and `lcg-passages-belief`) in `--configs` for an isolated
stage delta. All baselines send `rerank: false, expand: false`.

They are **not** in the default configuration list: they need a reranker (and for expansion an LLM)
reachable from the spawned service, which is normally the CoreML sidecar started with a rerank model
(see [`native/local-inference/README.md`](../native/local-inference/README.md#reranker-v1rerank)).

```sh
lcg-eval retrieval --corpus crates/eval/fixtures/retrieval-v2 \
  --embedder-uds /tmp/liminis-inference.sock \
  --configs lcg-passages,lcg-passages-rerank,lcg-passages-expand,lcg-passages-expand-rerank,qmd-rerank \
  --out report-stages
# a different expansion LLM (e.g. a local Qwen): add --expander-http <url>; a separate reranker:
# --reranker-uds <socket>
```

A stage that was requested but did not apply (sidecar down, model absent) is reported as a **failed
query**, never scored as if it had run — the service degrades to first-stage results, and scoring those
under the stage's name would silently report `lcg-passages` numbers. The report's *Passage stages* section
gives, for each stage, the paired nDCG@k difference against the matching un-staged baseline (`lcg-passages-raw` / `lcg-passages-belief`, else `lcg-passages`) per category with a 95%
bootstrap interval (and expansion on top of `lcg-passages-rerank`), plus the four rerank acceptance checks:
paraphrase gain with an interval above zero, overall not lower, temporal interval including or exceeding
zero, and p50 ≤ 150 ms. Take acceptance numbers only after #790 (embedding roles) is in the build under
test, and hybrid BM25 + vector (#789) has landed, but the eval has no configuration for rerank or expansion on top of hybrid yet, so that half of the comparison is still to be done.

### When to use each stage

Rerank is measured; expansion is not yet. The maintainer's v2 and v1 numbers (Apple silicon, mxbai Core ML
model) are in [ADR-0796](adr/0796-passage-rerank-and-query-expansion.md#belief-aware-first-stage-maintainer-head-2471ad5d):
`lcg-passages-belief-rerank` scores 0.759 nDCG@10 on v2 against 0.653 for `lcg-passages-belief` (+0.106
[+0.073, +0.142]) and 0.935 against 0.894 on the v1 holdout, at about 1.9 s p50 (not the 150 ms budget).

- **Rerank** is aimed at paraphrase questions (a role or description instead of a name). Its blend keeps
  superseded notes below their replacements, so it is designed not to repeat qmd's temporal collapse
  (`qmd-rerank` 0.293 vs `lcg-passages` 0.628). Measured on v2 with the belief-aware first stage, temporal
  is 0.800 against 0.745 for `lcg-passages-belief` (the 0.85 target was not reached; accepted as a
  follow-up), so it does not collapse. Use it for deliberate searches by role or description, not
  search-as-you-type (about 1.9 s p50 at the default 50 candidates).
- **Query expansion** is expected to cost on the order of a second (unmeasured). It is recommended **only if** the measurements show it improving
  some category with a paired interval above zero beyond what rerank already gives; if no category
  justifies it, the ADR says so and it should not be used. Either way it is for deliberate agent searches,
  never search-as-you-type.

## Reading the report

- **Overall** and **per-category** tables: every configuration's metrics as `point [lo, hi]`, with
  the best point estimate per column in bold and the number of questions behind each mean. Under
  each table, a paired nDCG@k comparison, and a line that names a winner only if it is
  distinguishable from every other configuration, else says "not distinguishable from noise". Low-n
  and at-the-ceiling categories are labelled.
- **By origin of returned notes** (fixtures that declare origins): how often derived restatements
  are returned and outrank the primary source.
- **Hypotheses**: *YES*, *NO*, *not distinguishable from noise* or *cannot be read* (a side was
  skipped) for "LCG beats qmd on temporal/superseded" and "qmd beats LCG on paraphrase".
- **Cost**, **Reproduction** (corpus/question versions and hashes, LCG build, qmd version and
  models, embedder, extractor, judge model, k, latency convention).

## Running on a private real notebook

Synthetic corpora approximate a real notebook. To sanity-check on yours, run the same harness over
your own notes with a question set you write locally, and **report results only: nothing from the
notebook or the questions is checked in or written into the report.**

1. **Make a fixture directory outside the repository** (for example `~/eval-private/mynotebook`)
   in the same format: `notes/<id>.md` (copy or symlink your markdown files; ids are lowercase
   slugs), a `manifest.json` listing the notes in the order to ingest them, and a `questions.jsonl`
   in the [format above](#question-format). A minimal manifest needs only
   `{"corpus_version": "private-1", "notes": [{"id": "..."}, ...]}`; add `reference_time` and, if you
   want the origin analysis, `origin`/`cites`. Write the questions yourself, including a few
   negatives.
2. **Run with `--private`** and with the output and judge cache outside any git tree:

   ```bash
   LCG_EVAL_PRIVATE=1 \
   LCG_EVAL_EMBEDDER_HTTP=http://127.0.0.1:8080 \
   LCG_EVAL_OUT=$HOME/eval-private/out \
   LCG_EVAL_JUDGE_CACHE=$HOME/eval-private/judge-cache.jsonl \
     crates/eval/scripts/07-retrieval-eval.sh --corpus $HOME/eval-private/mynotebook
   ```

   or `lcg-eval retrieval --private --corpus ... --out ... --judge-cache ...` (use `--no-grounded`
   to skip the judge and its cache).
3. **What `--private` does.** It requires an explicit `--out` (and `--judge-cache` unless
   `--no-grounded`) and **refuses any such path, and any `--record-cassette` or `--keep-workspace` path,
   that is inside a git working tree** (it looks for a `.git` above the path), so private text cannot land in a
   commit. The report's disclaimer says the corpus is a private notebook, not a synthetic one. The report contains aggregate
   metrics, counts, versions and hashes only: it never prints note or question text, and the detail
   of a failed query is withheld (`id: error (detail withheld)`) because a service error can echo the
   query. The judge cache *does* hold key facts and returned note text, which is why it must live
   outside the repository, and why running without `--private` from the repository root would write
   it there by default (`retrieval_evidence_cache.jsonl` in the working directory).
4. qmd keeps its own index of your notes under its usual location; remove it afterwards
   (`qmd --index <name> collection remove <name>`) if you do not want it to persist. Nothing in the
   repository is created or modified by a private run.

The same statistics apply: with a few dozen of your own questions the intervals will be wide, and
the report will say so.

## Ingesting under an ontology and keeping the workspace (#821)

Graph-shaped questions are answered from a graph extracted under an ontology, and graph-quality
tools (`graph-integrity`, `graph-qa`) read an ingested workspace. `lcg-eval retrieval` does both:

```bash
target/release/lcg-eval retrieval --corpus crates/eval/fixtures/retrieval-v2 \
  --ontology crates/eval/fixtures/retrieval-v2/ontology-v3.1.yaml \
  --keep-workspace ~/lcg-ws/v2-onto31 \
  --embedder-uds <socket> --record-cassette ~/lcg-ws/v2-onto31.cassette.jsonl
```

| Flag | Meaning |
|---|---|
| `--ontology <FILE>` | Every extraction call is constrained by this ontology. It is written into the workspace as `.lcg/ontology.yaml`, which is where the service reads it. |
| `--ontology-mode open\|strict` | The mode applied to the ontology (default `strict`, as in the extraction eval). It needs `--ontology`. The mode is part of the ontology hash. |
| `--keep-workspace <DIR>` | After the run, the service is shut down gracefully (`knowledge_close`) and the ingested workspace (DB, WAL, ontology) is moved to `DIR`, with an `lcg-eval-workspace.json` manifest. `DIR` must not exist or must be empty. It takes one `--embedding-profile` at most, and needs at least one `lcg-*` config in `--configs` (a qmd-only run ingests nothing into LCG). Without the flag the temporary workspace is removed, as before. |

**The ontology hash.** The report records the ontology's hash and mode in its Reproduction block
and in `meta.ontology_hash` / `meta.ontology_mode`. It is `lcg_core::ontology::content_hash`, the
hash LCG already computes; a run without an ontology records `none`. The value is the one the
**service reports** (`knowledge_get_ontology.ontology_hash` for the eval group), not the staged
file's: the spawned service is always given the eval workspace through `LIMINIS_WORKSPACE_ROOT`
(overriding any value in your shell), and right after startup `lcg-eval` checks that the service
loaded the staged ontology. If the service has no ontology, a different one, or is an older build
that reports no hash, the run exits non-zero before any ingest with an error naming both hashes
(#881). `graph-qa` applies the same check when its workspace has an ontology (the eval group's
`.lcg/ontology/retrieval-eval.yaml`, else `.lcg/ontology.yaml`). The cassette sidecar and the
kept-workspace manifest also record the service-reported hash.

**The cassette check.** A cassette's records carry no ontology, so `--record-cassette C` also
writes `C.meta.json` with the hash it was recorded under (`none` without an ontology). Replaying
`C` under a different hash (another ontology, another mode, or no ontology at all) fails before the
service starts. The error names both hashes, and no extraction call is made: without the check every
call would miss. A cassette recorded before this change has no sidecar. Replaying it with
`--ontology` prints a warning that the hash cannot be verified and carries on.

**The manifest.** `lcg-eval-workspace.json` records how the workspace was built: group, corpus
version and hash, ontology hash and mode, embedder (any userinfo, query or fragment stripped from its
URL, so no credential is written to disk), embedding profile, extractor, cassette and LCG build.
`graph-qa` refuses a workspace whose embedder differs from the one it is given (exit 3). The
addresses are compared after normalising loopback aliases (`localhost`, `127.0.0.1`, `[::1]`), case
and a trailing slash; `--allow-embedder-mismatch` downgrades a remaining mismatch to a warning, for
the same embedding model reached at a new address (a relocated socket, say). A genuinely different
embedder would make the stored vectors incomparable and the numbers silently meaningless.

## Model provider (judge, planner, reader)

The retrieval judge, the extraction judge and graph-qa's planner and reader all call an
Anthropic-compatible Messages endpoint. They resolve it the way the service does (ADR-0346):

| Setting | Meaning |
|---|---|
| `--anthropic-base-url <URL>` / `ANTHROPIC_BASE_URL` | Base URL; `/v1/messages` is appended. Use this for the local subscription bridge or a gateway. |
| `--anthropic-endpoint <URL>` / `LCG_ANTHROPIC_ENDPOINT` | The full endpoint, used verbatim. |
| `ANTHROPIC_API_KEY` | Sent as `x-api-key`. |
| `ANTHROPIC_AUTH_TOKEN` | Sent as `Authorization: Bearer` when a base URL or endpoint is configured. |

Any one of these enables the judge, so a bridge that needs no credential needs no dummy key. The
flags are on `retrieval` and `graph-qa`; the extraction eval reads the environment variables.

## Graph question answering (`graph-qa`, #821)

`lcg-eval graph-qa` answers the retrieval-v2 graph question set over a kept workspace and compares
how each retrieval route serves a reader. It is the reproducible form of experiment E13 (#815) and
the regression check for `knowledge_walk` (#820).

### The graph question set

`crates/eval/fixtures/retrieval-v2/graph-questions.jsonl` is generated from the skeleton by
`retrieval-gen`, and `retrieval-gen --check` reports any drift. It has 144 questions:

- **E13** (`set: "e13"`, ids `g000`–`g128`): 129 questions, the same as the E13 reference
  question for question. By shape: chain-2 48, chain-3 24, list 31, join 14, history 12. They fall
  into 13 finer families, such as `supplier-contact`, `owner-manager-location` and `change-history`.
- **Inverse** (`set: "inverse"`, `shape: "inverse"`, ids `i000`–`i014`): 15 new
  questions that ask for the *source* side of a relation given its target, so the answer depends
  on edge direction:
  - `reports-to-single`: "Who reports to X?", for managers with one report;
  - `account-manager-of`: "Which suppliers is X the account manager for?";
  - `projects-of-reports`: "Which projects are owned by someone who reports to X?".

Each line carries `id`, `set`, `family`, `cq`, `shape`, `text`, `answers` (ordered for history),
`scoring` (`single`, `set` or `ordered`), `split` (`dev` or `test`), `hops` (the facts a complete
answer rests on, with every primary note stating each value) and `path` (the expected relation
types and directions, for later path-level analysis).

**Dev/test split.** The split is fixed, with seed 815. For each family, in order of first
appearance, its ids are shuffled with one shared `random.Random(815)`, and the first half (rounded
up) is dev. The E13 part gives the reference 67 dev / 62 test. The inverse family is split the
same way, from its own `Random(815)`. Questions are chosen with Python-compatible RNGs (a Rust
MT19937) so the E13 set matches the probe exactly.

### Running it

```bash
target/release/lcg-eval graph-qa --workspace ~/lcg-ws/v2-onto31 --embedder-uds <socket> \
  --plans crates/eval/fixtures/retrieval-v2/e13-v5-plans.json \
  --runners passages,passages-belief-rerank,walk,route --out /tmp/graph-qa
```

The workspace is **copied** to a temporary directory and the service runs on the copy. The
workspace itself, including its WAL, is never modified.

| Flag | Default | Meaning |
|---|---|---|
| `--workspace <DIR>` | required | An ingested workspace (for example from `retrieval --keep-workspace`). A free-form workspace is allowed; the report records which kind it is. |
| `--corpus <DIR>` | `crates/eval/fixtures/retrieval-v2` | The fixture; `qmd` reads its notes. |
| `--questions <FILE>` | `<corpus>/graph-questions.jsonl` | The question set. |
| `--runners <LIST>` | `passages,walk,route` | Any of `passages`, `passages-belief-rerank`, `qmd`, `walk`, `route`. |
| `--plans <FILE>` | none | Recorded plans, used instead of planner calls for the questions they cover. |
| `--plans-only` | off | Never call the planner. A question with no recorded plan fails the run before any reader call (exit 4). |
| `--allow-reader-errors` | off | Report a runner whose reader replies fail to parse more than 10% of the time as a warning instead of failing the run (exit 1). |
| `--planner-model`, `--reader-model` | `claude-haiku-4-5-20251001` | Through the provider configuration above. |
| `--llm-cache <FILE>` | `graph-qa-llm-cache.jsonl` | The response cache. |
| `--embedder-*`, `--embedding-profile`, `--reranker-*`, `--service-bin` | | As for `retrieval`; the embedder must be the ingest embedder (exit 3 otherwise, unless `--allow-embedder-mismatch`). `--embedding-profile` defaults to the one recorded in the workspace manifest, and a different one is refused (exit 3). |
| `--qmd-bin`, `--qmd-index` | `qmd`, `lcg-eval-<corpus>-<hash8>` | For the `qmd` runner. |
| `--k`, `--concurrency` | 10, 6 | Passages per question; parallel model calls. |
| `--bootstrap-seed`, `--bootstrap-resamples` | 788, 10000 | Paired bootstrap, as in `retrieval` (ADR-0788). |
| `--out <DIR>` | none | Writes `graph-qa.md` and `graph-qa.json`. |

Exit codes:

- 0: the run succeeded.
- 1: the run failed (service, model provider or I/O), or a runner's reader replies failed to parse
  more than 10% of the time. Unparseable replies score 0, so a reader that answers in prose (a
  provider or bridge that ignores `output_config.format`) would otherwise yield an all-zero report
  that looks like a measurement. The report is still written; its Overall section and stderr give
  the parse-error count per runner. `--allow-reader-errors` turns this failure into a warning.
- 2: usage error.
- 3: unusable workspace (missing, not ingested, or an embedder mismatch).
- 4: questions without a plan, and no planner may be called. With no model provider configured, a planner response already in `--llm-cache` still counts as a plan, so a fully cached rerun works offline; only questions with no recorded or cached plan are uncovered.

### Runners

| Runner | Context given to the reader |
|---|---|
| `passages` | The shipped default `knowledge_search_passages`, top k, as `Passages from the notes:` followed by `[note, date]` blocks. |
| `passages-belief-rerank` | The same, reranked over a belief-aware first stage. It needs a reranker, and a stage that did not apply fails the run rather than being scored as first-stage results. |
| `qmd` | The whole note for each qmd hit (E13 step 3). It is skipped with a notice when qmd cannot run, as `retrieval` does. |
| `walk` | An LLM planner sees only the question and the workspace's `knowledge_get_ontology` menu (no family, no gold). It returns `{route, start, hops: [{types, phrase, direction}], list, history, answer_type}`. The plan runs through `knowledge_walk`, mapped as follows: `types` → `relation_types`, mode history / list / single, `direction_mode: soft`, and `query` set to the question. The reader gets `Facts from the knowledge graph:` followed by the walk's `facts_text`, or `(none found)`. |
| `route` | The planner's route for each question: `passages` context when it routes to passages, otherwise the walk's context. This is E13's "route v5". |

One plan per question serves both `walk` and `route`. A plan is a **plan error** in these cases:

- it does not parse;
- it names an unknown route or direction;
- a hop's relation type is not in the ontology;
- its `answer_type` is neither an ontology entity type nor `value`;
- a graph route has no hops.

The question is then answered from an empty walk and counted; it is never re-planned. A start
entity that matches no node is counted as unresolved, and the question scores 0.

### The reader and scoring

Every runner's context goes to one reader prompt, ported word for word from the probe
(`e13-read-v1`); the user turn is `<context>\n\nQuestion: <text>`. The planner prompt is the probe's
v5 (`e13-plan-v5`). Both versions are recorded in the report. Identical prompts are sent once, so
the same context always gets the same answer.

Scoring is the probe's `score()`. Answers are normalised first: lowercased, runs of characters
other than letters, digits, `%`, `$` and `.` become a space, and spaces and dots are stripped from
the ends. An answer matches a gold value when it contains it, or when the answer has 3 or more
characters and the gold contains the answer.

- `single`: 1 when the first answer matches, else 0.
- `set` (lists, joins, many-valued inverses): set F1.
- `ordered` (history): 1 only for the same number of values, each matching in order.

### Plans files and caching

A plans file has the probe's `v5-plans.json` format:

- top-level `model`, `ontology` and `planner_prompt`;
- `plans`, keyed by question id;
- optional `route_planned5_scores`, which the report shows as a reference.

Plan ids that name no question are ignored and listed as warnings. Questions the file does not
cover (the inverse family, for `e13-v5-plans.json`) go to the planner, unless `--plans-only` is set.

Planner and reader responses are cached in `--llm-cache`. Each entry is keyed by
`sha256` of the model, system prompt, user prompt and schema, and is written as it arrives. Any
change to a prompt byte or the model is a cache miss. A fully cached run makes no model call and
gives byte-identical `scores`. A provider failure aborts the run rather than scoring the remaining
questions as wrong, and a rerun resumes from the cache.

### The report

`graph-qa.md` and `graph-qa.json` contain:

- **Overall:** the mean score per runner on E13, its dev and test halves, and all questions
  (E13 plus inverse) with their halves. The E13 numbers stay comparable with E13.
- **By shape:** chain-2, chain-3, list, join, history and inverse, for all questions and for each
  half.
- **By family:** a secondary table.
- **Paired differences:** for every pair of runners, the mean difference with a 95% paired
  bootstrap interval, over E13, all, each half and each shape.
- **Diagnostics:** routes by shape, plan sources (file or planner), plan errors and unresolved
  starts by shape, reader responses that did not parse, plans-file warnings and the plans file's
  reference scores.
- **Per question:** route, plan and its source, whether the start resolved, answers, gold and
  score.
- **Reproduction:** the workspace's ontology hash and manifest, the models, prompt versions and
  cache counts.

In `graph-qa.json`, everything the questions, plans and responses determine is under `scores`.
Run facts such as cache counts are under `meta`.

## Standard setup for graph-QA results

Graph-QA numbers (E13 and later) are produced this way, and only this way:

1. **Ingest v2 under ontology v3.1 and keep the workspace**, with the BGE embedder the passage
   numbers use:

   ```bash
   target/release/lcg-eval retrieval --corpus crates/eval/fixtures/retrieval-v2 \
     --ontology crates/eval/fixtures/retrieval-v2/ontology-v3.1.yaml \
     --keep-workspace <ws> --embedder-uds <socket> --record-cassette <ws>.cassette.jsonl
   ```

   Replaying that cassette later reproduces the same graph without extraction calls.

2. **Run graph-qa with the recorded E13 v5 plans:**

   ```bash
   target/release/lcg-eval graph-qa --workspace <ws> --embedder-uds <socket> \
     --plans crates/eval/fixtures/retrieval-v2/e13-v5-plans.json \
     --runners passages,passages-belief-rerank,walk,route --out <dir>
   ```

   A question the plans file covers never reaches the planner, so every E13 plan is the recorded
   one. Only the inverse family is planned, live and then cached. Add `--plans-only` when no
   planner call may be made at all. It needs a question file that the plans cover, since an
   uncovered question then fails the run (exit 4).

3. **Report `route` on E13 and on E13/test** from the Overall table, with the paired differences
   against `passages`. The E13 reference is route v5 at 0.915 (E13) and 0.940 (E13 test). It was
   measured before the #817 prompt change, so a fresh extraction is needed to compare
   (#823 and #824).

`graph-integrity --workspace <ws>` reads the same workspace (see
[graph-integrity-evaluation.md](graph-integrity-evaluation.md)).

## Out of scope

This eval measures retrieval; it does not change it. Query expansion,
position-aware reranker blending and an explain trace are follow-up work that plugs in as new
configurations. Context assembly baselines (#734), document mode (#733) and retrieval
restricted to primary sources (#752) are separate. Design record:
[ADR-0754](adr/0754-retrieval-eval-architecture.md).
