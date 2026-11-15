#!/usr/bin/env bash
# Retrieval-quality eval (#754): LCG vs qmd over the checked-in Fernhill notebook fixture.
# See docs/retrieval-quality-evaluation.md. Not run in CI — it needs a real embedder and,
# for the qmd columns, a separately installed qmd.
#
#   LCG_EVAL_EMBEDDER_HTTP   (required) URL of a real embedding endpoint for the LCG service.
#                            Mock/hash embedders give meaningless numbers and are refused.
#   LCG_EVAL_EXTRACTOR_HTTP  OpenAI-compatible extraction endpoint (else the service default).
#   LCG_EVAL_REPLAY_CASSETTE Serve ingest extraction from a recorded cassette (free, repeatable).
#   LCG_EVAL_QMD_BIN         qmd executable (default: qmd). If it cannot run, the qmd columns
#                            are reported as skipped and the LCG columns still run.
#   LCG_EVAL_K               shared cut-off (default 10).
#   LCG_EVAL_OUT             report directory (default: retrieval-report).
#   DRY_RUN=1                print the resolved plan (configs, k, corpus hashes) and exit;
#                            spends nothing and starts nothing.
set -euo pipefail

HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO="${LCG_EVAL_REPO:-$(cd -- "$HERE/../../.." && pwd)}"
BIN="$REPO/target/release/lcg-eval"
SERVICE_BIN="$REPO/target/release/liminis-context-graph"

if [ ! -x "$BIN" ]; then
  echo "ERROR: $BIN not found. Build it first: cargo build --release -p lcg-eval -p lcg-service" >&2
  exit 1
fi

args=(retrieval --corpus "$REPO/crates/eval/fixtures/retrieval" --k "${LCG_EVAL_K:-10}" \
      --out "${LCG_EVAL_OUT:-retrieval-report}")
[ -n "${LCG_EVAL_QMD_BIN:-}" ] && args+=(--qmd-bin "$LCG_EVAL_QMD_BIN")
[ -n "${LCG_EVAL_REPLAY_CASSETTE:-}" ] && args+=(--replay-cassette "$LCG_EVAL_REPLAY_CASSETTE")
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
