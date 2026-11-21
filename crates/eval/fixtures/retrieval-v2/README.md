# retrieval-v2: a realistic notebook for `lcg-eval retrieval`

A synthetic notebook (fictional "Meridian" cooperative) shaped like a Liminis workspace. It contains
no real or private data. Design record: [ADR-0788](../../../../docs/adr/0788-retrieval-corpus-v2-and-bootstrap-reporting.md);
usage and the full description: [docs/retrieval-quality-evaluation.md](../../../../docs/retrieval-quality-evaluation.md).
The 31-note `../retrieval/` fixture (v1) stays beside it as the fast smoke corpus and never changes.

**Status: rendered, human review pending.** Structure is generated deterministically; the prose was
rendered once by an LLM and is checked in (`render.json`: `claude-sonnet-5-5`, prompt
`retrieval-v2-render-1`, 631 calls, 0 check errors). It was rendered through the offline exchange
(`retrieval-render --export-prompts`, replies written by Claude Code subagents on a Claude subscription,
then `--replies`), so no API key was used; the prompts are byte-identical to an API render. Three
incidental sentences were reworded by hand after the first check run because they tripped the
negative-question rule ("renewed the certificate", "people joined" ×2). Template prose is never checked in.

**Cassette:** recorded 2026-11-18 with lcg's default production extractor, `claude-haiku-4-5-20251001`
(what Liminis Notes ships), and the BGE-base-en-v1.5 sidecar embedder. It took 3,350 extraction calls
and had zero failures. The calls were served through a local Messages API → `claude -p` bridge on a
Claude subscription. Replay makes zero extraction requests. The cassette is keyed by prompt, so a run
with a different embedder can see different dedup prompts and miss the cassette; record a new one for
that embedder.

| | |
|---|---|
| Corpus | `meridian-notebook-2`: 408 notes (about 155,000 target words) |
| Size mix (targets) | 302 notes under 250 words, 22 of 250–500, 78 of 500–2,000, 6 over 5,000 |
| Origins | primary 286, external 36, derived 56 (each cites the notes it restates), answer 30 |
| Decisions | 26 revised 2–4 times: 45 explicit supersessions plus plain later notes that contradict without superseding; some effective dates differ from the recording date |
| Questions | `meridian-questions-2`: 244 — direct 40, paraphrase 40, temporal 40, answer 30, multi-hop 30, passage 30, negative 34 (minimum 30 per category) |

## Files

```text
scenario.json         generator input: seed and quotas (hand-edited)
skeleton.json         generated: facts, origins, citations, sizes, supersessions, question ground truth
graph-mapping.json    hand-edited: skeleton kind/attribute -> expected node type / relation type + direction,
                      read by `lcg-eval graph-integrity` (not touched by `retrieval-gen`)
review.json           human-review template (pending, empty verdicts)
--- written by `retrieval-render` ---
manifest.json  notes/*.md  questions.jsonl  questions.meta.json  render.json
cassette.jsonl        maintainer-recorded ingest extraction (claude-haiku-4-5-20251001, BGE-base-en-v1.5
                      embedder; corpus hash da41ea92487ba4fc); replayed by default
```

## Maintainer steps

```bash
cargo build --release -p lcg-eval
# deterministic skeleton (already checked in; --check verifies it)
target/release/lcg-eval retrieval-gen --check --spec crates/eval/fixtures/retrieval-v2/scenario.json \
  --out crates/eval/fixtures/retrieval-v2
# render the prose with an LLM (needs ANTHROPIC_API_KEY, ~630 calls; --dry-run shows the count)
target/release/lcg-eval retrieval-render --dir crates/eval/fixtures/retrieval-v2
# or, with no API key: export the prompts, write each reply to <replies>/<key>.md (e.g. with agents), import
target/release/lcg-eval retrieval-render --dir crates/eval/fixtures/retrieval-v2 --export-prompts prompts.jsonl
target/release/lcg-eval retrieval-render --dir crates/eval/fixtures/retrieval-v2 --replies <replies> --model <model that wrote them>
# fix anything the checks flag by re-rendering just those ids
target/release/lcg-eval retrieval-render --dir crates/eval/fixtures/retrieval-v2 --only <id>,<id>
```

1. **Render** as above, then commit the rendered files. `render.json` records the model and prompt
   version. Re-rendering later means bumping `corpus_version`/`questions_version` in `scenario.json`
   and regenerating the skeleton.
2. **Human review: PENDING, and the reviewer is Brett.** `review.json` holds a stratified sample
   (6 per category) with empty verdicts. After rendering it also carries the question texts and the
   question-set hash. Fill every verdict, set `status` to `reviewed`, and record `reviewer`,
   `reviewed_on` and `error_rate`. Above 5 % errors, correct or re-render the affected categories
   and re-review. Nobody else writes verdicts.
3. **Record the cassette** with a local extractor, not in CI:
   `LCG_EVAL_EMBEDDER_HTTP=… LCG_EVAL_RECORD_CASSETTE=crates/eval/fixtures/retrieval-v2/cassette.jsonl
   crates/eval/scripts/07-retrieval-eval.sh --corpus crates/eval/fixtures/retrieval-v2`.
   Afterwards the script replays it automatically with zero extraction requests. Note the corpus
   hash from the report header beside the file.

4. **Query variants (#802), after the render is final.** `query-variants.prompt.md` is the fixed
   prompt; the LLM sees each question's text and nothing else. No API key:
   ```bash
   target/release/lcg-eval retrieval-variants --dir crates/eval/fixtures/retrieval-v2 --export-prompts prompts.jsonl
   # write each reply (a JSON array of up to 5 strings) to <replies>/<key>.md, e.g. with agents
   target/release/lcg-eval retrieval-variants --dir crates/eval/fixtures/retrieval-v2 --replies <replies>
   target/release/lcg-eval retrieval-variants --dir crates/eval/fixtures/retrieval-v2 --check
   ```
   Commit `query-variants.jsonl`. It is stamped with the questions hash, so any edit to
   `questions.jsonl` makes it stale (regenerate). `--replies` refuses to write if a reply is missing or
   invalid or any variant contains a relevant note's title or key fact verbatim. Then run the eval with
   `lcg-passages-variants`, and the opt-in `lcg-passages-belief-variants` and
   `lcg-passages-hybrid-belief-variants`, to measure the merge bar.

   **Committed fixture:** 244 questions, 1,194 variants, written by Sonnet 4.6 from the question text only
   (`--export-prompts` / `--replies` exchange, no API key); `--check` reports no leaks against questions hash
   `c110fc2cb5090cea`.

   **The committed fixture predates the current prompt.** It was generated with the earlier
   `query-variants.prompt.md`, before the entity-preserving and no-placeholder rules were added, so it does
   not follow them: about 41% of multi-hop variants omit the question's named entity and about 10% of all
   variants carry placeholders such as `[Name]` or `[date]` (ADR-0802). `call_key` hashes the system prompt,
   so re-running `--replies` against the current prompt finds none of the existing replies, and the
   fixture is not reproducible from the committed prompt. The maintainer will regenerate it with the current
   prompt and re-measure together with #797's composite search; until then the multi-hop result is a
   known regression.

`cargo test -p lcg-eval --test retrieval_v2` checks the skeleton against the scenario, the pipeline
against a template-prose test double, and (once rendered) the checked-in prose against the skeleton.
