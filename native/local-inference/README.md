# local-inference — macOS Swift sidecar for liminis-context-graph

A small HTTP service that runs on-device CoreML embeddings (BGE-base-en-v1.5) and Apple Foundation Models chat completions. Exposes both behind an OpenAI-compatible API so `liminis-context-graph` (and any other OpenAI-compatible client) can consume them over UDS or HTTP.

**macOS 26+ only.** Foundation Models is a macOS 26 framework; the package's `Package.swift` requires it. There is no Linux or Windows equivalent in this package — non-Mac users should run a different OpenAI-compatible embedder / LLM (Python sentence-transformers, Ollama, vLLM, a cloud API, etc.) and point `liminis-context-graph` at it via `--embedder-http` and the OpenAI-compatible LLM env vars.

## What it provides

- `POST /v1/embeddings` — CoreML BGE-base-en-v1.5, 768-dim, batched
- `POST /v1/chat/completions` — Apple Foundation Models on-device LLM

### Role-aware embeddings (optional)

`/v1/embeddings` is a plain OpenAI-compatible embedder: a client sending only `{input, model}`
gets the same vectors it always did. Two optional, backward-compatible additions let
liminis-context-graph (LCG) use the model's own query/document instruction (issue #790):

- **Request:** an optional `input_type` of `"query"` or `"document"`. When the loaded model has
  instructions, the sidecar applies the one for that role to each non-empty input (for BGE
  v1.5: the query instruction `Represent this sentence for searching relevant passages: `, and
  nothing for documents). An unknown `input_type` is a `400`. An empty input still returns the
  zero vector. For a model with no instructions `input_type` is accepted and ignored.
- **Response:** `model` is the *real* loaded model id — `coreml-<model file stem>`, so the default
  `bge-base-en-v1.5.mlpackage` still reports `coreml-bge-base-en-v1.5` — and, when the model has
  instructions, an `instruction_roles` object (`{"query": "...", "document": ""}`) advertising
  them. Set `LOCAL_INFERENCE_EMBEDDING_MODEL_ID` to pin the reported id if you rename the model
  file (the id is part of an LCG workspace's recorded embedding identity).

The advertisement changes nothing by itself: LCG only sends `input_type` when the operator selects
`LCG_EMBEDDING_PROFILE=embedder`, and then adds no prefix of its own, so a text carries exactly one
instruction. See [Embedding instruction profiles](../../docs/configuration.md#embedding-instruction-profiles).

Both endpoints follow OpenAI's wire shapes. The embeddings contract is documented in [`docs/adr/0006-embedder-http-contract.md`](../../docs/adr/0006-embedder-http-contract.md) and the UDS transport in [`docs/adr/0016-oai-embedding-contract-uds-transport.md`](../../docs/adr/0016-oai-embedding-contract-uds-transport.md).

## Requirements

- macOS 26+ (Foundation Models framework)
- Swift 6.2+ (ships with current Xcode)
- Xcode command-line tools (`xcode-select --install`)
- ~500 MB of disk for the BGE-base CoreML model (downloaded on first setup, not committed)

## Selecting a mode

The sidecar serves both `/v1/embeddings` and `/v1/chat/completions` by default. Set
`LOCAL_INFERENCE_MODE` to run only one:

| `LOCAL_INFERENCE_MODE` | Serves | Setup required |
|---|---|---|
| `embeddings` | `/v1/embeddings` only | CoreML model (see "First-time setup" below) |
| `completions` | `/v1/chat/completions` only | **none** |
| `both` (default, or unset) | Both endpoints | CoreML model (see "First-time setup" below) |

`completions`-only mode needs no setup step at all: it skips the embedding-model
presence check, never loads a tokenizer, never compiles a `.mlmodelc`, and never
requires `LOCAL_INFERENCE_HF_CACHE`. A request to a route disabled by the selected
mode returns `404` with `error.type: "invalid_request_error"` — distinguishable from
the `503`/`server_error` a route returns when it's enabled but currently unavailable
(e.g. a missing embedding model in `embeddings`/`both` mode, or Apple Intelligence
being off in `completions`/`both` mode).

An invalid `LOCAL_INFERENCE_MODE` value fails fast at startup with a clear error,
rather than silently falling back to `both`.

**Foundation-Models-backed completions are not a substitute for the extraction
backend.** `LOCAL_INFERENCE_MODE=completions` only affects this sidecar's own
routes; it has no bearing on `liminis-context-graph`'s `--extractor-uds` flag, which
has no default-socket auto-detection by design (see
[ADR-0041](../../docs/adr/0041-local-openai-compatible-extraction-adapter.md)) —
this sidecar's Foundation Models backend is not recommended for extraction quality
(see [Extractor: local or hosted](../../docs/configuration.md#extractor-local-or-hosted)).

## First-time setup

Required for `embeddings` and `both` modes only — `completions`-only mode needs no
setup step at all (see "Selecting a mode" above).

The CoreML model file (`bge-base-en-v1.5.mlpackage`, ~400 MB) is not committed to the repo. It is downloaded and converted on first use:

```bash
cd native/local-inference
./prepare-embedding-assets.sh
```

This downloads the BGE-base-en-v1.5 model from HuggingFace and converts it to the CoreML format the sidecar expects. The result lives in `native/local-inference/bge-base-en-v1.5.mlpackage/` and is `.gitignore`-d.

Subsequent runs reuse the cached model.

## Reranker (`/v1/rerank`)

An **optional** cross-encoder route (issue #796) lets `liminis-context-graph` rescore the top passage
candidates of `knowledge_search_passages`. It is enabled iff `LOCAL_INFERENCE_RERANK_MODEL` is set; there
is no `LOCAL_INFERENCE_MODE` value for it, so `both` and every other mode behave exactly as before. Not
configured → the route returns `404`; configured but the model failed to load → `503` (the sidecar keeps
serving embeddings and completions). At load the sidecar runs a **batch self-check**: it scores a few
documents in one batch and again one at a time and refuses to load (so the route answers `503`) if any
score differs, which catches a broken export or output read-back before it can mis-rank searches.
`benchmark/bench_rerank.py` runs the same check before timing anything.

```
POST /v1/rerank
{"model": "...", "query": "...", "documents": ["...", ...], "top_n": 5}      # top_n optional
→ {"object": "list", "model": "coreml-...",
   "results": [{"index": 2, "relevance_score": 3.1}, {"index": 0, "relevance_score": -0.4}, ...]}
```

`relevance_score` is the cross-encoder's **raw logit** (higher = more relevant), best-first, `index` into
`documents`; LCG applies the sigmoid itself. At most 256 documents per request. The input pair is
`query` (kept up to half the token budget) + `document` (cut at its tail) at the model's fixed length —
deterministic and head-biased.

**Loading.** The rerank model loads in the background, so the sidecar listens and serves embeddings at once.
A large cross-encoder takes minutes to load on a cold start (the maintainer measured 214–274 s for
mxbai-rerank-large-v1, whatever the compute unit); until it is ready `/v1/rerank` answers a `503` whose message
says the model is "still loading", and a model that failed to load answers a `503` with the reason. The
effect of `LOCAL_INFERENCE_MLMODELC_CACHE` (the compiled `.mlmodelc` cache) on a warm start has **not been
measured**; do not assume it removes the load time.

Setup (not committed; the model is hundreds of MB):

```bash
cd native/local-inference
RERANK_BATCH=8 ./prepare-rerank-assets.sh   # default model: mixedbread-ai/mxbai-rerank-large-v1
LOCAL_INFERENCE_RERANK_MODEL=./mxbai-rerank-large-v1-b8-l512.mlpackage \
LOCAL_INFERENCE_RERANK_TOKENIZER_ID=mixedbread-ai/mxbai-rerank-large-v1 \
LOCAL_INFERENCE_HF_CACHE=./resources/models/tokenizer \
  .build/release/LocalInference
```

| Variable | Default | Meaning |
|----------|---------|---------|
| `LOCAL_INFERENCE_RERANK_MODEL` | unset (route disabled) | Path to the converted `.mlpackage`/`.mlmodelc` |
| `LOCAL_INFERENCE_RERANK_TOKENIZER_ID` | `mixedbread-ai/mxbai-rerank-large-v1` | Tokenizer, loaded offline from `LOCAL_INFERENCE_HF_CACHE` when set |
| `LOCAL_INFERENCE_RERANK_FAMILY` | unset | Unset: the pair layout (special tokens, pad id) is read from the tokenizer's own `tokenizer.json` post-processor, and segment ids are fed exactly when the exported model has a `token_type_ids` input. Set to `xlm-roberta` (`<s> q </s></s> d </s>`) or `bert` (`[CLS] q [SEP] d [SEP]`) to force a built-in layout |
| `LOCAL_INFERENCE_RERANK_MODEL_ID` | `coreml-<file stem>` | Model id reported in responses |
| `LOCAL_INFERENCE_RERANK_COMPUTE_UNITS` | unset | `cpuAndGPU`, `cpuAndNeuralEngine` or `cpuOnly`. Unset: the Neural Engine for `bge-reranker-*`, the GPU for every other model. Measured by the maintainer for mxbai-rerank-large-v1: about 3.8 s per batch of 8 on the Neural Engine, about 0.26 s on the GPU |

`LOCAL_INFERENCE_MLMODELC_CACHE`, when set, also caches the compiled rerank model, in its `rerank/`
subdirectory. The export has **fixed** `[batch, length]` shapes read from the model at startup;
batch size is the main latency lever, so choose it with `benchmark/bench_rerank.py` (p50/p95 by candidate
count and passage length) before settling on one. `mixedbread-ai/mxbai-rerank-large-v1` (Apache-2.0,
DeBERTa-v3-large, revision pinned in `convert-rerank-model.py`) is the default; `BAAI/bge-reranker-base` (MIT,
pinned) is the supported alternative and `bge-reranker-v2-m3` uses the same converter. **The mxbai Core ML export was verified by the maintainer** (fp16, batch 8, length 512, pinned `98f65584`): on
the 8 real pairs of `paraphrase-03` the sidecar's scores matched transformers within 0.037, and the full v2
sweep is in ADR-0796. The converter's own step 5 also fails the conversion if Core ML and PyTorch logits drift by
more than 0.5. Qwen-family rerankers (including Qwen3-Reranker-0.6B and `mxbai-rerank-*-v2`) have no export path
here and are not used.

**DeBERTa-v3 (mxbai) export and tokenizer.** `convert-rerank-model.py` patches DeBERTa-v2/v3 before tracing
(plain-Python replacements for the `torch.jit.script` helpers, a float `scaled_size_sqrt`, and a fp16-safe
`-1e4` mask fill instead of `finfo(float32).min`, which turned padded rows into NaN) and verifies the
export on an unpadded **and a padded** batch with `CPU_AND_GPU` (`ComputeUnit.ALL` crashes at load for this
model). swift-transformers rejects `tokenizer_class: DebertaV2Tokenizer`; `prepare-rerank-assets.sh` stages
the tokenizer first (so a failed conversion does not skip it) and relabels the staged copy
`XLMRobertaTokenizer` (mxbai's `tokenizer.json` is Unigram + Metaspace like XLM-R's); the special-token layout
and pad id still come from `tokenizer.json`. These steps were written from the maintainer's working local
patch; the converter and the relabel have **not been run end to end in CI** (they need the model and a Mac).

**Tokenizer fixtures.** `Tests/LocalInferenceTests/RerankTokenizerFixtures.swift` pins the sidecar's pair
layout against `transformers`. Each entry holds the `post_processor`, `padding` and the pad entry of
`added_tokens` from the model's real `tokenizer.json` (vocabulary omitted) plus `transformers`' output for one
fixed query/document pair. Regenerate it after changing a model or its pinned revision (revisions are in
`convert-rerank-model.py`):

```bash
uv run --with transformers --with sentencepiece --with huggingface_hub python - <<'PY'
import json
from huggingface_hub import hf_hub_download
from transformers import AutoTokenizer

MODEL, REV = "mixedbread-ai/mxbai-rerank-large-v1", "<pinned revision>"
q = "who owns Marlin Core?"
d = "Marlin Core is owned by Northwind Holdings. It was acquired in 2021."
tok = AutoTokenizer.from_pretrained(MODEL, revision=REV)
full = json.load(open(hf_hub_download(MODEL, "tokenizer.json", revision=REV)))
pad = [t for t in full["added_tokens"] if t["content"] in ("[PAD]", "<pad>")]
fragment = {"post_processor": full["post_processor"], "padding": full.get("padding"), "added_tokens": pad}
enc = tok(q, d, truncation="only_second")
print(json.dumps(fragment))
print("queryIds", tok(q, add_special_tokens=False)["input_ids"])
print("documentIds", tok(d, add_special_tokens=False)["input_ids"])
print("transformersPairIds", enc["input_ids"])
print("transformersTokenTypeIds", enc.get("token_type_ids"))  # nil in Swift when absent
PY
```

Paste the printed values into the matching `RerankTokenizerFixture` entry.

**Other cross-encoders.** Nothing outside `prepare-rerank-assets.sh` and `convert-rerank-model.py` is tied to
the default model: set `RERANK_MODEL` to any Hugging Face `AutoModelForSequenceClassification` cross-encoder
that emits **one logit per pair**, plus `LOCAL_INFERENCE_RERANK_TOKENIZER_ID` to its tokenizer (and
`RERANK_REVISION` to pin it; only the default model and bge-reranker-base have a built-in pin). What the
sidecar still assumes: the pair layout is read from the tokenizer's `tokenizer.json` (a `TemplateProcessing`
pair template, or `RobertaProcessing` / `BertProcessing`), so a tokenizer without one of those needs
`LOCAL_INFERENCE_RERANK_FAMILY`; the pad id comes from `padding.pad_id` or the added `[PAD]`/`<pad>` token;
the CoreML output is named `logits` with shape `[B, 1]` or `[B]`; inputs are fixed `[batch, length]`
`input_ids`, `attention_mask` (and `token_type_ids` only if the exported model declares it); and the tokenizer
is loaded through swift-transformers' `AutoTokenizer`, so a model it cannot load offline is not supported
(**swift-transformers' handling of the DeBERTa-v3 tokenizer is untested here**). Truncation caps the query at
half the post-special-token budget, whereas transformers' `truncation="only_second"` does not; the two agree
for any query shorter than that. Decoder-only
scorers (yes/no logits) and multi-label heads are not. A second CoreML model is
resident only when the variable is set. The budget (≤ 150 ms p50 for a whole k = 10 search) is **not met**
by the default model: the maintainer recorded about 1.1 s p50 at 25 candidates and 1.9 s at 50 (see
[ADR-0796](../../docs/adr/0796-passage-rerank-and-query-expansion.md#measurements)).

## Build and run

```bash
cd native/local-inference
swift build -c release

# Default: listens on UDS at /tmp/liminis-inference.sock
.build/release/LocalInference
```

`liminis-context-graph` discovers this socket automatically:

```bash
# In a separate terminal
liminis-context-graph
# Or explicitly:
liminis-context-graph --embedder-uds /tmp/liminis-inference.sock
```

## Tests

```bash
cd native/local-inference
swift test
```

Tests use stub `.mlpackage` fixtures (under `Tests/LocalInferenceTests/Fixtures/`) so the real model is not required to run them. Tokenizer fixtures live under `Tests/LocalInferenceTests/Fixtures/tokenizer-cache/` and ship in-tree.

`.github/workflows/swift.yml` runs `swift build` and `swift test` on every PR and push that touches `native/local-inference/**`, on a `macos-26` runner. Still run `swift test` locally before pushing — CI is a backstop, not a substitute for catching failures early.

If you need to regenerate the stub fixtures (after a real schema change), run:

```bash
./refresh-test-fixtures.sh
```

## Dependencies and licenses

All Swift package dependencies are Apache 2.0 (mostly `github.com/apple/swift-*`, plus [hummingbird](https://github.com/hummingbird-project/hummingbird) and [swift-transformers](https://github.com/huggingface/swift-transformers)) or MIT (jinja). The BGE-base-en-v1.5 model is MIT by [BAAI](https://huggingface.co/BAAI/bge-base-en-v1.5).

Apple Foundation Models is provided by the macOS SDK; usage from third-party software is permitted under standard SDK terms.

See `Package.resolved` for the pinned dep tree.

## Relationship to the main project

This sidecar is an **optional** component. The Rust binary (`liminis-context-graph`) does not depend on it being built or running — it only needs *some* OpenAI-compatible embedder reachable at startup. The sidecar happens to be the easiest such embedder on macOS.

`liminis-context-graph` is the **sole source of truth** for this sidecar's source. The Liminis app (`shadoworg/liminis-notes`) does not carry its own copy of `native/local-inference/` — it obtains a built binary via the distribution mechanism below. See [ADR-0503](../../docs/adr/0503-swift-sidecar-source-of-truth.md) for the consolidation history and rationale.

## Distribution

The Liminis app (`shadoworg/liminis-notes`), or anyone else who wants a `LocalInference` binary without building it themselves, has three options, in order of convenience:

1. **Download the latest release asset.** Every `local-inference-v*` [GitHub Release](../../releases) on this repo carries `local-inference-aarch64-apple-macos.tar.gz` (a single self-contained `local-inference` binary) and its `.sha256` checksum:

   ```bash
   curl -L https://github.com/liminisapp/liminis-context-graph/releases/download/<TAG>/local-inference-aarch64-apple-macos.tar.gz -o local-inference.tar.gz
   tar -xzf local-inference.tar.gz
   ```

   To discover the latest tag programmatically, filter client-side by tag prefix — this repo also publishes dotted-version Rust releases via `release.yml`/cargo-dist, so an unfiltered "most recent release" call (e.g. `GET /repos/.../releases?per_page=1`, which has no server-side tag-pattern filter) can just as easily return one of those instead:

   ```bash
   gh release list --repo liminisapp/liminis-context-graph --limit 50 | awk -F'\t' '$3 ~ /^local-inference-v/' | head -1
   # or: gh api /repos/liminisapp/liminis-context-graph/releases --jq '[.[] | select(.tag_name | startswith("local-inference-v"))][0].tag_name'
   ```

2. **Manually trigger the release workflow.** [`.github/workflows/swift-release.yml`](../../.github/workflows/swift-release.yml) is a `workflow_dispatch`-only GitHub Actions workflow: builds the sidecar in release mode on `macos-latest`, runs `swift test`, and publishes the binary to a new `local-inference-v<N>`-tagged Release. Trigger it from the Actions tab (or `gh workflow run swift-release.yml`) when you need a fresh build without waiting for someone else to cut one.

3. **Build locally.** Follow "Build and run" below on a macOS 26 / Swift 6.2+ machine.

This is deliberately **not** a tag-push-triggered release: this repo's `release.yml` (the Rust binary's cargo-dist pipeline) already triggers on any tag containing a `digit.digit.digit` substring, so a dotted version tag for the sidecar could collide with it. Release tags for this sidecar always use the undotted `local-inference-v<integer>` scheme instead — see ADR-0503 for the full reasoning.
