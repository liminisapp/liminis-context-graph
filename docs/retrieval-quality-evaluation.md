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

> **Status: no numbers published yet.** The harness and fixture are in the repository; results
> appear here only after a maintainer has run the comparison on a real embedder. Until then
> this page deliberately contains no scores.

## What it measures

| Configuration | What runs |
|---|---|
| `lcg-passages` | `knowledge_search_passages`. **Vector-only**: this method does no FTS and no RRF. |
| `lcg-entities-facts` | `knowledge_find_entities` + `knowledge_find_relationships` (hybrid FTS + vector, RRF), default reads. k is split across the two reads (`ceil(k/2)` entities + `ceil(k/2)` facts) and the ranks are interleaved. |
| `qmd` | `qmd query --no-rerank` (query expansion on). |
| `qmd-rerank` | `qmd query` with its LLM reranker. |
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

### Result mapping (identical for every configuration)

- Passage → note: `source_description` is `"{note}:{chunk}"`.
- Entity → notes: every note whose episodes mention the entity (built once at ingest).
- Fact → notes: the fact's *producing* episodes (`episodes`). ADR-0012's either-endpoint
  inference is never used.
- qmd result → note: the result path's file stem.

## Caveats the report repeats

- LCG and qmd use **different embedding (and reranker) models**, so this compares systems, not
  retrieval algorithms.
- The corpus is small and hand-built. Results are indicative, not general.
- LCG ingestion is only deterministic with a replayed extraction cassette; qmd's expansion and
  reranking depend on their models. Relevance metrics are exactly reproducible for a fixed LCG
  graph and embedder.
- The hypotheses ("LCG beats qmd on temporal/superseded questions"; "qmd beats LCG on
  paraphrase questions") get a yes/no reading from the data. They are not assumed.

## The fixture

`crates/eval/fixtures/retrieval/` (versioned: the report prints the corpus and question-set
versions and content hashes):

```text
manifest.json         corpus_version, chunking, notes in ingest order, supersedes relations
notes/<id>.md         one markdown note per manifest entry
questions.jsonl       one question per line
questions.meta.json   {"questions_version": "..."}
```

The corpus (`fernhill-notebook-1`) is a **synthetic** notebook for a fictional bike-share
co-operative, written in the style of a Liminis notebook. It contains no real or private data
and is not derived from any user's workspace. It has 31 notes, including 8 decisions that a
later note explicitly replaces. Note ids are lowercase slugs (`[a-z0-9-]`) because they double
as the file names qmd indexes.

### Question format

```json
{"id": "temporal-01", "category": "temporal", "text": "Which company hosts the booking API?",
 "relevant_notes": [{"note": "decision-hosting-tern", "gain": 2}],
 "current": ["decision-hosting-tern"], "stale": ["decision-hosting-harbor"]}
```

| Field | Rules |
|---|---|
| `id` | Required, unique. Every load error names it. |
| `category` | Exactly one of `direct`, `paraphrase`, `temporal`, `answer`. |
| `text` | The query, verbatim. |
| `relevant_notes` | `[{note, gain}]`, `gain` 1–3 (default 1). Notes must exist in the manifest. Empty only with `"negative": true`. |
| `negative` | `true` for a question no note answers. Scored on false-positive rate. |
| `evidence` | **Answer questions only**, required there: `[{note, key_fact}]`. `key_fact` is a short sentence the judge checks against returned text. The note must be in `relevant_notes`. |
| `current` / `stale` | **Temporal questions only**, both required. `current` ⊆ `relevant_notes`; `stale` must not be relevant. The manifest must record the stale note as superseded. |

Paraphrase questions deliberately share little vocabulary with the relevant text.

### Adding a question

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
crates/eval/scripts/07-retrieval-eval.sh             # report.md + report.json in ./retrieval-report
```

Or directly:

```bash
target/release/lcg-eval retrieval \
  --service-bin target/release/liminis-context-graph \
  --embedder-http http://127.0.0.1:8080 \
  --k 10 --out retrieval-report
```

| Flag | Meaning |
|---|---|
| `--corpus <DIR>` | Fixture directory (default `crates/eval/fixtures/retrieval`). |
| `--k <N>` | Shared cut-off, 1–100 (default 10). |
| `--configs <LIST>` | Subset of the five configuration ids. |
| `--service-bin`, `--embedder-http/--embedder-uds`, `--extractor-http/--extractor-uds` | How the spawned LCG service is configured. |
| `--embedding-profile <LIST>` | Embedding [instruction profile(s)](configuration.md#embedding-instruction-profiles) to measure, e.g. `none,bge-v1.5`. See [Comparing embedding profiles](#comparing-embedding-profiles). |
| `--replay-cassette <PATH>` / `--record-cassette <PATH>` | Replay or record ingest extraction ([cassettes](testing-and-evaluation.md#recordreplay-cassettes)) for a free, repeatable LCG ingest. |
| `--qmd-bin`, `--qmd-index` | qmd executable and named index. The default index name includes the corpus hash (`lcg-eval-<corpus_version>-<hash8>`), so a changed corpus gets a fresh index. qmd keeps a collection's original directory: if you pass your own `--qmd-index` and reuse it after changing `--corpus` or editing notes, the run warns, and you must remove the old collection or pick a new name. |
| `--judge-model`, `--judge-cache`, `--no-grounded` | Grounded-evidence judge (needs `ANTHROPIC_API_KEY`); verdicts are cached so re-runs cost nothing. Without a key the grounded columns are **skipped**, not zero. |
| `--out <DIR>` | Write `report.md` and `report.json`. |
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

- **Overall** and **per-category** tables: every configuration's metrics, with the best value
  per column in bold and the number of questions behind each mean. A winner line per category
  is by nDCG@k.
- **Hypotheses**: a yes/no reading of "LCG beats qmd on temporal/superseded" and "qmd beats LCG
  on paraphrase", or "cannot be read" if a side was skipped.
- **Cost**, **Reproduction** (corpus/question versions and hashes, LCG build, qmd version and
  models, embedder, extractor, judge model, k, latency convention).

## Out of scope

This eval measures retrieval; it does not change it. Query expansion, a top-rank bonus in RRF,
position-aware reranker blending and an explain trace are follow-up work that plugs in as new
configurations. Context assembly baselines (#734), document mode (#733) and retrieval
restricted to primary sources (#752) are separate. Design record:
[ADR-0754](adr/0754-retrieval-eval-architecture.md).
