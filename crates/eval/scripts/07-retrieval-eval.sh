#!/usr/bin/env bash
# Retrieval-quality eval (#754, #788): LCG vs qmd over a notebook fixture.
# See docs/retrieval-quality-evaluation.md. Not run in CI — it needs a real embedder and,
# for the qmd columns, a separately installed qmd.
#
#   07-retrieval-eval.sh [--corpus DIR]
#
#   --corpus DIR             Fixture directory (manifest.json, notes/, questions.jsonl).
#   --corpus=DIR             Default: crates/eval/fixtures/retrieval (v1, the fast smoke corpus).
#                            Use crates/eval/fixtures/retrieval-v2 for the full 400-note corpus.
#   LCG_EVAL_CORPUS          Same as --corpus (the flag wins). An explicit path that is not a
#                            fixture directory fails with exit 2, naming the path.
#   LCG_EVAL_EMBEDDER_HTTP   (required) URL of a real embedding endpoint for the LCG service.
#                            Mock/hash embedders give meaningless numbers and are refused.
#   LCG_EVAL_EXTRACTOR_HTTP  OpenAI-compatible extraction endpoint (else the service default).
#   LCG_EVAL_REPLAY_CASSETTE Serve ingest extraction from a recorded cassette (free, repeatable).
#                            If unset and <corpus>/cassette.jsonl exists, that file is replayed.
#   LCG_EVAL_RECORD_CASSETTE Record ingest extraction to this cassette (needs a live extractor).
#   LCG_EVAL_QMD_BIN         qmd executable (default: qmd). If it cannot run, the qmd columns
#                            are reported as skipped and the LCG columns still run.
#   LCG_EVAL_K               shared cut-off (default 10).
#   LCG_EVAL_OUT             report directory (default: retrieval-report).
#   LCG_EVAL_BOOTSTRAP_SEED / LCG_EVAL_BOOTSTRAP_RESAMPLES   confidence-interval settings.
#   LCG_EVAL_PRIVATE=1       private-notebook run (results only; see the docs). Requires
#                            LCG_EVAL_OUT, and LCG_EVAL_JUDGE_CACHE unless grounded judging is
#                            off, both outside any git tree.
#   LCG_EVAL_JUDGE_CACHE     grounded-judge verdict cache path.
#   LCG_EVAL_NO_GROUNDED=1   skip grounded-evidence judging.
#   DRY_RUN=1                print the resolved plan (configs, k, corpus hashes) and exit;
#                            spends nothing and starts nothing.
set -euo pipefail

HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO="${LCG_EVAL_REPO:-$(cd -- "$HERE/../../.." && pwd)}"
DEFAULT_CORPUS="$REPO/crates/eval/fixtures/retrieval"
BIN="$REPO/target/release/lcg-eval"
SERVICE_BIN="$REPO/target/release/liminis-context-graph"

CORPUS="${LCG_EVAL_CORPUS-}"
# "Explicit" means supplied at all, even empty: an unset shell variable passed as --corpus "$V2"
# must not silently select v1.
CORPUS_GIVEN=0
if [ "${LCG_EVAL_CORPUS+set}" = "set" ]; then CORPUS_GIVEN=1; fi
while [ $# -gt 0 ]; do
  case "$1" in
    --corpus)
      if [ $# -lt 2 ]; then echo "ERROR: --corpus requires a directory." >&2; exit 2; fi
      CORPUS="$2"; CORPUS_GIVEN=1; shift 2 ;;
    --corpus=*) CORPUS="${1#--corpus=}"; CORPUS_GIVEN=1; shift ;;
    -h|--help) sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d'; exit 0 ;;
    *) echo "ERROR: unknown argument '$1' (usage: 07-retrieval-eval.sh [--corpus DIR])." >&2; exit 2 ;;
  esac
done
if [ "$CORPUS_GIVEN" = "1" ]; then
  if [ -z "$CORPUS" ]; then
    echo "ERROR: --corpus (or LCG_EVAL_CORPUS) was given an empty path; pass a fixture directory or omit it to use the default (v1)." >&2
    exit 2
  fi
  # Explicit selection: fail here, naming the path, before anything is built or spent.
  if [ ! -f "$CORPUS/manifest.json" ] || [ ! -f "$CORPUS/questions.jsonl" ]; then
    echo "ERROR: --corpus '$CORPUS' is not a fixture directory (needs manifest.json and questions.jsonl)." >&2
    exit 2
  fi
else
  CORPUS="$DEFAULT_CORPUS"
fi

if [ "${LCG_EVAL_PRIVATE:-0}" = "1" ] && [ -z "${LCG_EVAL_OUT:-}" ]; then
  echo "ERROR: LCG_EVAL_PRIVATE=1 requires LCG_EVAL_OUT (a directory outside any git tree)." >&2
  exit 2
fi

if [ ! -x "$BIN" ]; then
  echo "ERROR: $BIN not found. Build it first: cargo build --release -p lcg-eval -p lcg-service" >&2
  exit 1
fi

args=(retrieval --corpus "$CORPUS" --k "${LCG_EVAL_K:-10}" \
      --out "${LCG_EVAL_OUT:-retrieval-report}")
[ -n "${LCG_EVAL_QMD_BIN:-}" ] && args+=(--qmd-bin "$LCG_EVAL_QMD_BIN")
if [ -n "${LCG_EVAL_RECORD_CASSETTE:-}" ]; then
  args+=(--record-cassette "$LCG_EVAL_RECORD_CASSETTE")
elif [ -n "${LCG_EVAL_REPLAY_CASSETTE:-}" ]; then
  args+=(--replay-cassette "$LCG_EVAL_REPLAY_CASSETTE")
elif [ -f "$CORPUS/cassette.jsonl" ]; then
  # A corpus that ships its recorded extraction replays it by default: zero extraction requests.
  args+=(--replay-cassette "$CORPUS/cassette.jsonl")
fi
[ -n "${LCG_EVAL_BOOTSTRAP_SEED:-}" ] && args+=(--bootstrap-seed "$LCG_EVAL_BOOTSTRAP_SEED")
[ -n "${LCG_EVAL_BOOTSTRAP_RESAMPLES:-}" ] && args+=(--bootstrap-resamples "$LCG_EVAL_BOOTSTRAP_RESAMPLES")
[ -n "${LCG_EVAL_JUDGE_CACHE:-}" ] && args+=(--judge-cache "$LCG_EVAL_JUDGE_CACHE")
[ "${LCG_EVAL_NO_GROUNDED:-0}" = "1" ] && args+=(--no-grounded)
[ "${LCG_EVAL_PRIVATE:-0}" = "1" ] && args+=(--private)
[ -n "${LCG_EVAL_EXTRACTOR_HTTP:-}" ] && args+=(--extractor-http "$LCG_EVAL_EXTRACTOR_HTTP")

if [ "${DRY_RUN:-0}" = "1" ]; then
  exec "$BIN" "${args[@]}" --dry-run
fi

if [ -z "${LCG_EVAL_EMBEDDER_HTTP:-}" ]; then
  echo "ERROR: set LCG_EVAL_EMBEDDER_HTTP to a real embedding endpoint." >&2
  echo "       Mock and hash embedders give meaningless retrieval numbers." >&2
  exit 1
fi
if [ ! -x "$SERVICE_BIN" ]; then
  echo "ERROR: $SERVICE_BIN not found. Build it: cargo build --release -p lcg-service" >&2
  exit 1
fi

exec "$BIN" "${args[@]}" --service-bin "$SERVICE_BIN" --embedder-http "$LCG_EVAL_EMBEDDER_HTTP"
