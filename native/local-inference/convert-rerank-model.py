#!/usr/bin/env python3
"""
Convert a HuggingFace cross-encoder reranker to Core ML (.mlpackage) for the sidecar's /v1/rerank.

Issue #796. Defaults to mixedbread-ai/mxbai-rerank-large-v1 (DeBERTa-v3-large sequence
classifier, Apache-2.0). BAAI/bge-reranker-base (XLM-RoBERTa-base) stays a supported alternative.
Also works with BAAI/bge-reranker-v2-m3 and any `AutoModelForSequenceClassification` with one
output logit. NOTE: the mxbai export needs the DeBERTa-v2 patches in `patch_deberta_v2` below and was verified by the
maintainer (sidecar scores within 0.037 of transformers on real pairs); step 5 fails the conversion if the
Core ML logits drift from PyTorch, on an unpadded and a padded batch.
Qwen3-Reranker-0.6B is a decoder-only yes/no scorer and needs a different export path; it is not
handled here (see docs/adr/0796-passage-rerank-and-query-expansion.md).

The exported model has FIXED shapes [batch, seq_len] — the Swift actor reads them from the model
and scores documents in groups of `batch`, padding the last group. Batch size is the main latency
lever (the embedder is batch=1): larger batches amortise per-call overhead on the Neural Engine.
Use `benchmark/bench_rerank.py` to choose it.

Inputs : input_ids, attention_mask (int32) [+ token_type_ids with --token-types, for BERT-style]
Output : logits  [batch, 1]  (raw logit; higher = more relevant — LCG applies the sigmoid itself)

Usage:
    uv run convert-rerank-model.py [--model mixedbread-ai/mxbai-rerank-large-v1] [--batch 8]
                                   [--max-seq-length 512] [--output mxbai-rerank-large-v1-b8.mlpackage]
                                   [--revision <commit-hash>]
"""
# /// script
# requires-python = ">=3.11,<3.13"
# dependencies = [
#     "torch==2.4.0",
#     "transformers>=4.36.2,<5.0",
#     "coremltools==8.1",
#     "numpy<2.4",
#     "sentencepiece",
# ]
# ///

import argparse
import os
import shutil

# Pinned HuggingFace commits. Update deliberately; the converted asset is not committed, so an
# unpinned revision would make the measured numbers irreproducible.
DEFAULT_RERANK_MODEL = "mixedbread-ai/mxbai-rerank-large-v1"
PINNED_REVISIONS = {
    "mixedbread-ai/mxbai-rerank-large-v1": "98f655841d5caf0b16eaff79c2b4ca109d920d17",
    "BAAI/bge-reranker-base": "2cfc18c9415c912f9d8155881c133215df768a70",
}


def patch_deberta_v2(torch):
    """Make DeBERTa-v2/v3 traceable by coremltools and fp16-safe (mxbai-rerank-large-v1).

    Without these the export fails (issue #796):
      * the model wraps helpers in torch.jit.script (make_log_bucket_position, build_rpos,
        c2p/p2c/pos_dynamic_expand); coremltools then sees script control flow and fails with a
        branch type mismatch on `rel_pos_ids`. Plain-Python equivalents trace to static shapes;
      * `scaled_size_sqrt` traces as an int32 sqrt, which coremltools rejects. A float32 constant
        replaces it;
      * `DisentangledSelfAttention.forward` fills masked scores with finfo(float32).min, which is
        -inf at fp16, so fully masked (padded) query rows softmax to NaN and the NaN spreads to every
        row of the next layer. -1e4 is representable in fp16.
    Applied only for `deberta-v2` models, before tracing.
    """
    import inspect
    import math
    import textwrap

    import transformers.models.deberta_v2.modeling_deberta_v2 as dv2

    def make_log_bucket_position(relative_pos, bucket_size, max_position):
        sign = torch.sign(relative_pos)
        mid = bucket_size // 2
        abs_pos = torch.where(
            (relative_pos < mid) & (relative_pos > -mid),
            torch.tensor(mid - 1).type_as(relative_pos),
            torch.abs(relative_pos),
        )
        log_pos = (
            torch.ceil(torch.log(abs_pos.float() / mid) / math.log((max_position - 1) / mid) * (mid - 1))
            + mid
        )
        return torch.where(abs_pos <= mid, relative_pos.type_as(log_pos), log_pos * sign)

    def build_rpos(q, k, relative_pos, position_buckets, max_relative_positions):
        if k.size(-2) != q.size(-2):
            return dv2.build_relative_position(
                k, k, bucket_size=position_buckets, max_position=max_relative_positions
            )
        return relative_pos

    dv2.make_log_bucket_position = make_log_bucket_position
    dv2.build_rpos = build_rpos
    dv2.c2p_dynamic_expand = lambda c, q, r: c.expand([q.size(0), q.size(1), q.size(2), r.size(-1)])
    dv2.p2c_dynamic_expand = lambda c, q, k: c.expand([q.size(0), q.size(1), k.size(-2), k.size(-2)])
    dv2.pos_dynamic_expand = lambda p, a, k: p.expand(a.size()[:2] + (p.size(-2), k.size(-2)))
    dv2.scaled_size_sqrt = lambda layer, f: torch.tensor(
        math.sqrt(layer.size(-1) * f), dtype=torch.float32
    )

    src = textwrap.dedent(inspect.getsource(dv2.DisentangledSelfAttention.forward))
    marker = "torch.finfo(query_layer.dtype).min"
    if marker not in src:
        raise SystemExit(
            "transformers' DeBERTa-v2 attention no longer matches the expected source "
            f"(missing {marker!r}); update patch_deberta_v2 for this transformers version"
        )
    namespace = {}
    exec(compile(src.replace(marker, "-1e4"), "deberta_v2_patched_forward", "exec"), dv2.__dict__, namespace)
    dv2.DisentangledSelfAttention.forward = namespace["forward"]
    print("  Applied DeBERTa-v2 export patches (scripted helpers, float sqrt, fp16-safe mask fill)")


def convert(model_name, output_path, revision, batch, max_seq_length, token_types):
    import numpy as np
    import coremltools as ct
    import torch
    import torch.nn as nn
    from transformers import AutoModelForSequenceClassification

    rev_display = revision[:8] + "..." if revision else "latest"
    print(f"Step 1: Load {model_name} (revision={rev_display})...")
    base = AutoModelForSequenceClassification.from_pretrained(model_name, revision=revision)
    base.eval()
    if getattr(base.config, "model_type", "") == "deberta-v2":
        patch_deberta_v2(torch)
    if base.config.num_labels != 1:
        raise SystemExit(f"{model_name} has {base.config.num_labels} labels; a reranker must have 1")

    class Wrapper(nn.Module):
        """Plain-tensor forward (no ModelOutput) so torch.jit.trace works."""

        def __init__(self, model):
            super().__init__()
            self.model = model

        def forward(self, input_ids, attention_mask, token_type_ids=None):
            kwargs = dict(input_ids=input_ids, attention_mask=attention_mask, return_dict=False)
            if token_type_ids is not None:
                kwargs["token_type_ids"] = token_type_ids
            return self.model(**kwargs)[0]  # [batch, 1]

    model = Wrapper(base).eval()
    shape = (batch, max_seq_length)
    ids = torch.ones(shape, dtype=torch.long)
    mask = torch.ones(shape, dtype=torch.long)
    types = torch.zeros(shape, dtype=torch.long)

    print(f"Step 2: Trace (batch={batch}, seq_len={max_seq_length})...")
    example = (ids, mask, types) if token_types else (ids, mask)
    with torch.no_grad():
        traced = torch.jit.trace(model, example, strict=False)

    print("Step 3: Convert traced model -> Core ML (float16)...")
    inputs = [
        ct.TensorType(name="input_ids", shape=shape, dtype=np.int32),
        ct.TensorType(name="attention_mask", shape=shape, dtype=np.int32),
    ]
    if token_types:
        inputs.append(ct.TensorType(name="token_type_ids", shape=shape, dtype=np.int32))
    cml = ct.convert(
        traced,
        inputs=inputs,
        outputs=[ct.TensorType(name="logits")],
        compute_precision=ct.precision.FLOAT16,
        compute_units=ct.ComputeUnit.ALL,
        minimum_deployment_target=ct.target.macOS14,
    )

    print(f"Step 4: Save to {output_path}...")
    if os.path.exists(output_path):
        shutil.rmtree(output_path)
    cml.save(output_path)
    size_mb = sum(
        os.path.getsize(os.path.join(dp, f)) for dp, _, fs in os.walk(output_path) for f in fs
    ) / (1024 * 1024)
    print(f"  Done: {output_path} ({size_mb:.1f} MB)")

    print("Step 5: Verify against the PyTorch model (unpadded, then padded input)...")
    # ComputeUnit.ALL crashes at load for DeBERTa-v3; CPU+GPU is what the sidecar uses for it.
    loaded = ct.models.MLModel(output_path, compute_units=ct.ComputeUnit.CPU_AND_GPU)

    def check(label, ids_arr, mask_arr):
        feed = {"input_ids": ids_arr, "attention_mask": mask_arr}
        if token_types:
            feed["token_type_ids"] = np.zeros(shape, dtype=np.int32)
        got = loaded.predict(feed)["logits"].reshape(batch)
        with torch.no_grad():
            want = model(
                torch.from_numpy(feed["input_ids"]).long(),
                torch.from_numpy(feed["attention_mask"]).long(),
                *([torch.from_numpy(feed["token_type_ids"]).long()] if token_types else []),
            ).reshape(batch).numpy()
        if not np.all(np.isfinite(got)):
            raise SystemExit(f"{label}: Core ML produced non-finite logits {got}")
        err = float(np.max(np.abs(got - want)))
        print(f"  {label}: max |coreml - torch| logit error: {err:.4f}")
        return err

    rng = np.random.default_rng(0)
    ids_arr = rng.integers(5, 1000, size=shape).astype(np.int32)
    full = np.ones(shape, dtype=np.int32)
    # Real inputs are padded: random all-ones masks hide the fully-masked-row NaN failure.
    padded = np.zeros(shape, dtype=np.int32)
    for row in range(batch):
        padded[row, : int(rng.integers(8, max_seq_length // 2))] = 1
    err = max(check("unpadded", ids_arr, full), check("padded", ids_arr, padded))
    print(f"  max |coreml - torch| logit error: {err:.4f}")
    if err > 0.5:
        raise SystemExit("float16 conversion drifted more than 0.5 logits from the PyTorch model")
    print("  Sanity check passed.")


def main():
    p = argparse.ArgumentParser(description="Convert a HuggingFace cross-encoder to Core ML")
    p.add_argument("--model", default=DEFAULT_RERANK_MODEL)
    p.add_argument("--output", default=None, help="Output .mlpackage path")
    p.add_argument(
        "--revision",
        default=None,
        help="HF commit hash. Defaults to the pinned revision for mixedbread-ai/mxbai-rerank-large-v1 "
        "and BAAI/bge-reranker-base; any other --model loads its latest revision unless one is given.",
    )
    p.add_argument("--batch", type=int, default=8, help="Fixed batch size of the exported model")
    p.add_argument("--max-seq-length", type=int, default=512)
    p.add_argument(
        "--token-types",
        action="store_true",
        help="Export a token_type_ids input (BERT-style cross-encoders; not XLM-RoBERTa)",
    )
    a = p.parse_args()
    if a.output is None:
        a.output = f"{a.model.split('/')[-1]}-b{a.batch}-l{a.max_seq_length}.mlpackage"
    revision = a.revision or PINNED_REVISIONS.get(a.model)
    convert(a.model, a.output, revision, a.batch, a.max_seq_length, a.token_types)


if __name__ == "__main__":
    main()
