# retrieval-v2: a realistic, generated notebook for `lcg-eval retrieval`

A synthetic notebook (fictional "Meridian" cooperative) shaped like a Liminis workspace. It contains
no real or private data. Design record: [ADR-0788](../../../../docs/adr/0788-retrieval-corpus-v2-and-bootstrap-reporting.md);
usage and the full description: [docs/retrieval-quality-evaluation.md](../../../../docs/retrieval-quality-evaluation.md).
The 31-note `../retrieval/` fixture (v1) stays beside it as the fast smoke corpus and never changes.

| | |
|---|---|
| Corpus | `meridian-notebook-2`: 408 notes, about 165,000 words |
| Size mix | 302 notes under 250 words, 21 of 250–500, 75 of 500–2,000, 4 of 2,000–5,000, 6 over 5,000 |
| Chunks at 900 tokens | about 680; 84 notes span several chunks |
| Origins | primary 286, external 36, derived 56 (each cites the notes it restates), answer 30 |
| Decisions | 26 revised 2–4 times: 45 explicit supersessions plus plain later notes that contradict without superseding; some effective dates differ from the recording date |
| Questions | `meridian-questions-2`: 244 — direct 40, paraphrase 40, temporal 40, answer 30, multi-hop 30, passage 30, negative 34 (minimum 30 per category) |

## Files

```text
scenario.json         generator input: seed and quotas (the only hand-edited file)
manifest.json         generated: notes in ingest order with origin/cites, supersessions
notes/*.md            generated
questions.jsonl       generated
questions.meta.json   generated: question-set version
review.json           human-review record (written once by the generator; see below)
cassette.jsonl        (optional, maintainer-recorded) ingest extraction cassette
```

**Do not edit the generated files by hand.** Change `scenario.json` or the generator
(`crates/eval/src/retrieval/generator/`) and regenerate:

```bash
cargo build --release -p lcg-eval
target/release/lcg-eval retrieval-gen --spec crates/eval/fixtures/retrieval-v2/scenario.json \
  --out crates/eval/fixtures/retrieval-v2
target/release/lcg-eval retrieval-gen --check --spec crates/eval/fixtures/retrieval-v2/scenario.json \
  --out crates/eval/fixtures/retrieval-v2        # exit 1 if the checked-in files drift
```

`cargo test -p lcg-eval --test retrieval_v2` runs the same drift check plus the shape, ground-truth
and review checks, so a stale fixture fails the tests.

## Status of the two maintainer steps

- **Human review: PENDING.** `review.json` lists a stratified sample (6 per category) with empty
  verdicts. Fill every verdict, set `status` to `reviewed`, and record `reviewer`, `reviewed_on` and
  `error_rate`. Above 5 % errors, regenerate or correct the affected categories and re-review.
  Changing the questions changes `questions_hash` and the test will say the review is stale.
- **Cassette: NOT RECORDED.** Recording needs a live extractor:
  `LCG_EVAL_EMBEDDER_HTTP=… LCG_EVAL_RECORD_CASSETTE=crates/eval/fixtures/retrieval-v2/cassette.jsonl
  crates/eval/scripts/07-retrieval-eval.sh --corpus crates/eval/fixtures/retrieval-v2`.
  Afterwards `07-retrieval-eval.sh --corpus …/retrieval-v2` replays it automatically with zero
  extraction requests. Record it against the corpus hash in the report header
  (`meridian-notebook-2`) and note that hash beside the file.
