# ADR-0872: `lcg-eval retrieval` Run Comparability: Input Digests, Hard/Soft Inputs, `--compare`

**Status**: Accepted
**Date**: 2026-11-22
**Issue**: #872 (implements #847; research #863)

## Context

`lcg-eval retrieval` reported per-category metrics and paired bootstrap differences *within* one run,
but two reports could be read against each other even when their inputs differed, producing deltas
that meant nothing. The cassette and variants file were recorded by path or not at all, the embedder
by socket path rather than by the model behind it, the service by a version string, and the `LCG_*`
environment not at all. `graph-integrity --compare` already refuses incompatible runs (exit 2).

## Decision

- **An `inputs` block** (schema version 1) is added to `report.json`: full sha256 digests of the
  corpus, questions, variants file, cassette, embedder model, extractor identity, service binary and
  effective service environment. `none` means the run does not use the input; `unknown` means it
  does but it could not be identified. The existing `meta` keys and their 16-hex hashes are
  unchanged.
- **Each digest depends on its own input only**, so changing one input changes exactly one entry
  (the extractor identity carries no cassette path; the environment digest excludes
  `LCG_REPLAY_LLM`/`LCG_RECORD_LLM`).
- **The environment digest is computed from the child's effective environment**, through the same
  function (`lcg::service_env_overrides`) `spawn_service` applies, so it cannot drift from what runs.
  Credential-bearing names (`*_API_KEY`, `*_TOKEN`, `*_SECRET`, `*_PASSWORD`, and names containing
  `URL` or `ENDPOINT`) contribute their name only: a hashed low-entropy secret is still guessable and
  the report is shared. The cost is that a changed key or URL is not detected.
- **The embedder digest identifies the model by what the service reports** (`embedding_model`,
  `embedding_dim`, effective instruction profile), never the socket path. The service does not expose
  model files, so this is an identity digest, not a file digest.
- **`--compare <FILE>` has hard and soft inputs.** Hard: `inputs.corpus`, `inputs.questions`, `k`; any
  difference exits 2 naming all of them and prints no deltas. Soft: the remaining digests, the
  service build and the ontology hash; a difference prints the deltas under a banner, and `--strict`
  refuses instead. `none` equals `none`; `unknown` is always a mismatch under `--strict`, and is
  otherwise a gap rather than a change when both sides are `unknown`.
- **An earlier report with no `inputs` block exits 2** with a message to re-run it. It is never a
  silent pass. An unreadable or invalid file exits 1, as in `graph-integrity`.
- **Order of checks.** The file is read, and its `inputs` checked, before the service starts, so an
  old or unreadable report costs nothing. Hard and `--strict` mismatches are judged only after this
  run's own report is written, so a refusal never discards a finished (and possibly expensive) run.
  The price is that a hard mismatch is discovered only after a full run.
- **`--record-ranked`** writes passage-level `ranked.jsonl` (`score`, `similarity`, `legs` when the
  service supplies them; no evidence text), only for `lcg-passages*` configs. It needs `--out` and is a
  usage error with `--private`.

## Consequences

- Metrics, and what each config sends to the service, are unchanged. `report.json` gains only the
  additive `inputs` key; a run without `--compare` and `--record-ranked` exits as before.
- Reports from before this change cannot be compared and must be re-run.
- `compare.md` is Markdown only; there is no `compare.json`.
- With several `--embedding-profile` values the embedder and environment digests combine the
  per-profile values in sorted order, so profile order does not matter.
