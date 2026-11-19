# ADR-0754: Retrieval-Quality Eval Architecture — Service-Spawn Driver, qmd Baseline, Metric Definitions

**Status**: Accepted
**Date**: 2026-11-15
**Issue**: #754 (named for the issue per the repo's ADR numbering rule)

## Context

`lcg-eval` (ADR-0048) measured extraction only. Nothing measured whether `knowledge_find_entities`,
`knowledge_find_relationships` and `knowledge_search_passages` return the right things for a real
question, so there was no number to compare LCG's read side with alternatives — chiefly
[qmd](https://github.com/tobi/qmd), the retrieval half of the "LLM wiki over Obsidian" setups
Liminis Notes competes with, and the strongest local stand-in for the "raw top-k retrieval"
baseline #734 asks for. A 2026-11-15 survey proposed borrowing qmd techniques (query expansion,
a top-rank RRF bonus, reranker blending, an explain trace); this eval is the measurement that
must exist before any of them can be judged.

## Decisions

1. **A `retrieval` subcommand of `lcg-eval`**, dispatched in `main.rs` *before* the flat
   extraction `parse_args`. One binary stays in cargo-dist tarballs; the extraction CLI is
   untouched (a test pins this). A separate `[[bin]]` was rejected for the extra artifact.
2. **Drive the shipped service, not an in-process `AppState`.** The eval spawns
   `liminis-context-graph` in a temp workspace and speaks newline-delimited JSON-RPC over
   `.lcg/service.sock`. This measures the real IPC surface and avoids duplicating ~22-field
   `AppState` wiring that breaks on every new field. The transport is a trait, so tests use a
   canned responder. No `lcg-core`/`lcg-service` change, and therefore no new dispatch method or
   MCP `ToolSpec`.
3. **qmd is an out-of-band CLI subprocess.** Never a build, test or runtime dependency. A missing
   binary, failed indexing (including unavailable model downloads) or unparsable output yields a
   *skipped* configuration with a reason — never a zero score. It uses a named index so a user's
   own qmd index is untouched. qmd documents `--no-rerank` but no switch for query expansion, so
   both qmd configurations include expansion; qmd does not expose model-call counts, reported as
   "not exposed".
4. **`lcg-passages` is measured as it is: vector-only.** (Since #806 / ADR-0806 it measures the shipped default, belief-aware vector; `lcg-passages-raw` is the vector-only baseline.) `search_passages` does no FTS and no RRF;
   the report says so. Changing retrieval is out of scope.
5. **A labelled fifth configuration, `lcg-entities-facts-belief`** (`believed_only=true` on the
   fact read). Only edge reads can express belief time (ADR-0705); without this variant the
   temporal hypothesis would test the configuration, not the architecture. The four spec-named
   configurations remain the headline table; the variant is an extra row.
6. **Notes are the unit of relevance; evidence is judged by content.** Note-level recall/MRR/nDCG
   use a shared mapping (passage `source_description`; entity → ingest-time `entity → notes`
   table; fact → *producing* episodes, never ADR-0012 either-endpoint inference; qmd → file
   stem). Grounded evidence is judged by a blind LLM against a short key-fact string, so neither
   system's chunk boundaries are favoured. The judge is a separate trait with its own prompt
   version, schema and JSONL cache, leaving the extraction judge's untagged-enum cache intact.
7. **Metric rules.** Ranked notes are de-duplicated (first occurrence) and cut at k, never
   padded. nDCG uses `2^gain − 1` with a log2 discount. Negative questions are excluded from
   recall/MRR/nDCG means and scored by a separate false-positive rate. *Stale above current* is
   true when the best-ranked stale note outranks the best-ranked current note, or a stale note is
   returned with no current one. A question whose query errors is listed and left out of the
   means, not scored as zero; if every query fails the configuration is reported as skipped.
8. **Warm latency only.** Indices are built explicitly and one unmeasured pass precedes the timed
   pass, so LCG's lazy first-search index build never lands on one question. Ingestion cost is
   reported separately from per-query cost.
9. **Refuse meaningless embedders.** The eval requires an explicit real embedder
   (`--embedder-http/--embedder-uds` or `LCG_EMBEDDING_URL`); `MockEmbedder` and `HashEmbedder`
   would produce meaningless numbers.
10. **The fixture is synthetic and hand-authored.** `demo-notebook` is private to the Liminis app
    workspace and is not in this repository. `crates/eval/fixtures/retrieval/` is a fictional
    small-company notebook written in the same style (31 notes, 8 superseded decision pairs, 43
    judged questions across direct / paraphrase / temporal / answer, plus negatives). Corpus and
    question set carry versions and content hashes that the report prints.

## Consequences

- Results compare *systems*, not algorithms: LCG and qmd use different embedding and reranker
  models. The report prints this caveat and that the corpus is small.
- Relevance metrics are exactly reproducible for a fixed LCG graph and embedder. LCG ingest is
  deterministic only with a replayed cassette (ADR-0044); qmd expansion and reranking are
  model-dependent.
- The eval is run on demand, not in CI. CI runs only the network-free unit and integration tests,
  which use a canned LCG responder, a fake qmd runner and a canned judge.
- Future retrieval changes (query expansion, rank bonus, reranker blending, explain trace) plug in
  as new `Retriever` configurations without reshaping the report.
- Published numbers are deliberately absent from the docs until a maintainer has run the
  comparison.

## Open items

- **qmd interface not verified against a live install.** The qmd commands and the `--no-rerank`
  toggle come from qmd's README; its JSON output shape and `--version` output are not documented
  there, so the parser in `retrieval/qmd.rs` is deliberately tolerant. Any qmd failure marks the
  qmd configurations skipped with a reason rather than scoring zero. The first maintainer run
  should confirm the shape against the pinned qmd version and record that version in the docs.
- **No end-to-end run yet.** The LCG service spawn, a real embedder and the grounded-evidence
  judge are covered only by canned-responder tests; the first live run is still ahead.

## References

ADR-0048 (eval harness), ADR-0052 (`--dry-run` shares the resolution path — `retrieval::resolve`
serves both), ADR-0044 (cassettes), ADR-0705 (belief time), ADR-0707 (document supersession, used
at ingest to record the replaced decisions), ADR-0629 (search evidence and the similarity floor).
