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

## What it measures

| Configuration | What runs |
|---|---|
| `lcg-passages` | `knowledge_search_passages` with `mode: "vector"` sent explicitly. **Vector-only**: no FTS and no RRF. Pinned, so it stays a stable baseline whatever the service default is. |
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
| `--configs <LIST>` | Subset of the five configuration ids. |
| `--service-bin`, `--embedder-http/--embedder-uds`, `--extractor-http/--extractor-uds` | How the spawned LCG service is configured. |
| `--embedding-profile <LIST>` | Embedding [instruction profile(s)](configuration.md#embedding-instruction-profiles) to measure, e.g. `none,bge-v1.5`. See [Comparing embedding profiles](#comparing-embedding-profiles). |
| `--replay-cassette <PATH>` / `--record-cassette <PATH>` | Replay or record ingest extraction ([cassettes](testing-and-evaluation.md#recordreplay-cassettes)) for a free, repeatable LCG ingest. |
| `--qmd-bin`, `--qmd-index` | qmd executable and named index. The default index name includes the corpus hash (`lcg-eval-<corpus_version>-<hash8>`), so a changed corpus gets a fresh index. qmd keeps a collection's original directory: if you pass your own `--qmd-index` and reuse it after changing `--corpus` or editing notes, the run warns, and you must remove the old collection or pick a new name. |
| `--judge-model`, `--judge-cache`, `--no-grounded` | Grounded-evidence judge (needs `ANTHROPIC_API_KEY`); verdicts are cached so re-runs cost nothing. Without a key the grounded columns are **skipped**, not zero. |
| `--out <DIR>` | Write `report.md` and `report.json`. |
| `--bootstrap-seed <N>`, `--bootstrap-resamples <N>` | Confidence-interval settings (default seed 788, 10,000 resamples). Recorded in the report. |
| `--private` | Private-notebook run; see below. |
| `--dry-run` | Print the resolved plan and exit. |

The extraction backend for LCG ingestion (live Anthropic, a local OpenAI-compatible endpoint,
or a cassette) and the ontology are whatever the spawned service uses; the report records the
choice.

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
   `--no-grounded`) and **refuses any such path, and any `--record-cassette` path, that is inside a
   git working tree** (it looks for a `.git` above the path), so private text cannot land in a
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

## Out of scope

This eval measures retrieval; it does not change it. Query expansion,
position-aware reranker blending and an explain trace are follow-up work that plugs in as new
configurations. Context assembly baselines (#734), document mode (#733) and retrieval
restricted to primary sources (#752) are separate. Design record:
[ADR-0754](adr/0754-retrieval-eval-architecture.md).
