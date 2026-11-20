#!/usr/bin/env bash
# Produces the artifacts the sidecar's /v1/rerank needs (issue #796):
#   1. native/local-inference/<model>-b<BATCH>-l<LEN>.mlpackage  (convert-rerank-model.py)
#   2. a local HuggingFace cache entry for the reranker's tokenizer, so the sidecar can load it
#      offline (same layout prepare-tokenizer.py writes for BGE:
#      <HF_CACHE>/models/<org>/<model>/).
#
# Not committed (the model is hundreds of MB). Re-running is safe.
#
# Env:  RERANK_MODEL   default mixedbread-ai/mxbai-rerank-large-v1 (BAAI/bge-reranker-base also works)
#       RERANK_BATCH   default 8        (see benchmark/bench_rerank.py)
#       RERANK_LENGTH  default 512
#       RERANK_REVISION  HF commit hash for RERANK_MODEL. Unset: the converter's pinned revision
#                        for the default model and BAAI/bge-reranker-base, and the latest revision
#                        for any other model. Applied to the tokenizer files too.
#       HF_CACHE       default ./resources/models/tokenizer  (match LOCAL_INFERENCE_HF_CACHE)
#
# Then run the sidecar with:
#   LOCAL_INFERENCE_RERANK_MODEL=./<printed .mlpackage> \
#   LOCAL_INFERENCE_RERANK_TOKENIZER_ID=$RERANK_MODEL \
#   LOCAL_INFERENCE_HF_CACHE=$HF_CACHE  .build/release/LocalInference

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

MODEL="${RERANK_MODEL:-mixedbread-ai/mxbai-rerank-large-v1}"
BATCH="${RERANK_BATCH:-8}"
LENGTH="${RERANK_LENGTH:-512}"
REVISION="${RERANK_REVISION:-}"
HF_CACHE="${HF_CACHE:-./resources/models/tokenizer}"
OUT="$(basename "$MODEL")-b${BATCH}-l${LENGTH}.mlpackage"

if ! command -v uv >/dev/null 2>&1; then
  echo "ERROR: uv is required. Install with: brew install uv" >&2
  exit 1
fi

echo "==> Staging tokenizer files for offline swift-transformers loading (before the conversion, so a failed conversion does not skip it)"
uv run --with huggingface_hub python - "$MODEL" "$HF_CACHE" "$REVISION" <<'PY'
import ast
import json
import sys
from huggingface_hub import snapshot_download

model, cache, revision = sys.argv[1], sys.argv[2], sys.argv[3] or None
if revision is None:
    # Same pin the converter uses (read from its source so the two cannot drift), so the
    # tokenizer files -- special-token layout and pad id -- match the model that was converted.
    tree = ast.parse(open("convert-rerank-model.py").read())
    for node in tree.body:
        if isinstance(node, ast.Assign) and any(
            getattr(t, "id", None) == "PINNED_REVISIONS" for t in node.targets
        ):
            revision = ast.literal_eval(node.value).get(model)
dest = f"{cache}/models/{model}"
snapshot_download(
    repo_id=model,
    revision=revision,
    local_dir=dest,
    allow_patterns=["tokenizer*", "sentencepiece*", "spiece.model", "vocab*", "merges.txt",
                    "special_tokens_map.json", "added_tokens.json", "config.json"],
)

# swift-transformers has no DeBERTa-v2 tokenizer class and rejects `DebertaV2Tokenizer`
# (unsupportedTokenizer). mxbai's tokenizer.json is Unigram + Metaspace, like XLM-R's, so the staged
# copy is relabelled; the sidecar then reads the real special-token layout and pad id from
# tokenizer.json itself (RerankTokenLayout.fromTokenizerJSON).
cfg_path = f"{dest}/tokenizer_config.json"
try:
    cfg = json.load(open(cfg_path))
except FileNotFoundError:
    cfg = None
if cfg and cfg.get("tokenizer_class") == "DebertaV2Tokenizer":
    cfg["tokenizer_class"] = "XLMRobertaTokenizer"
    json.dump(cfg, open(cfg_path, "w"), indent=2)
    print("  relabelled tokenizer_class DebertaV2Tokenizer -> XLMRobertaTokenizer in the staged copy")
print(f"  tokenizer files staged in {dest}")
PY

echo
echo "==> Converting $MODEL -> $OUT (batch $BATCH, length $LENGTH; takes a few minutes)"
REV_ARGS=()
[ -n "$REVISION" ] && REV_ARGS=(--revision "$REVISION")
uv run convert-rerank-model.py --model "$MODEL" --batch "$BATCH" --max-seq-length "$LENGTH" \
  --output "$OUT" ${REV_ARGS[@]+"${REV_ARGS[@]}"}

echo
echo "Done. Start the sidecar with:"
echo "  LOCAL_INFERENCE_RERANK_MODEL=./$OUT LOCAL_INFERENCE_RERANK_TOKENIZER_ID=$MODEL \\"
echo "  LOCAL_INFERENCE_HF_CACHE=$HF_CACHE .build/release/LocalInference"
