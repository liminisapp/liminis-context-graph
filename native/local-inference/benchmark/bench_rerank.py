#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11,<3.13"
# dependencies = []
# ///
"""
Latency spike for the sidecar's /v1/rerank (issue #796).

Measures wall-clock latency of one rerank call for several candidate counts N and passage
lengths, over the sidecar's Unix socket, so the candidate model (bge-reranker-base, -v2-m3, ...),
its exported batch size and sequence length can be compared against the 150 ms p50 budget for a
whole rerank-enabled search at k = 10. This is the *latency half* of the quality-per-millisecond
comparison; the quality half is `lcg-eval retrieval --configs lcg-passages,lcg-passages-rerank`
run once per candidate model (see docs/retrieval-quality-evaluation.md).

Usage:
    # Start the sidecar with the candidate model first, e.g.
    #   LOCAL_INFERENCE_RERANK_MODEL=./bge-reranker-base-b8-l512.mlpackage \
    #     LOCAL_INFERENCE_RERANK_TOKENIZER_ID=BAAI/bge-reranker-base \
    #     LOCAL_INFERENCE_HF_CACHE=./resources/models/tokenizer .build/release/LocalInference &
    uv run benchmark/bench_rerank.py [--socket /tmp/liminis-inference.sock] \
        [--candidates 10,25,50] [--doc-words 80,250,600] [--iterations 30] [--warmup 5] \
        [--label bge-reranker-base-b8-l512] [--out results/]

Prints a markdown table (candidates x passage length: p50 / p95 ms) and writes it, with the raw
samples, to results/rerank-<label>-<timestamp>.{md,json}.
"""

from __future__ import annotations

import argparse
import json
import socket
import statistics
import time
from datetime import datetime, timezone
from pathlib import Path

QUERY = "the initiative covering shift rostering for the support team"
VOCAB = (
    "rostering shift schedule team support initiative planning quarterly review capacity "
    "hiring onboarding handover on-call rota coverage staffing policy decision notes meeting "
    "agenda owner deadline milestone budget risk dependency migration release"
).split()


def passage(n_words: int, seed: int) -> str:
    return " ".join(VOCAB[(seed * 7 + i * 3) % len(VOCAB)] for i in range(n_words))


def post(sock_path: str, body: dict, timeout: float = 30.0) -> tuple[int, bytes]:
    payload = json.dumps(body).encode()
    req = (
        b"POST /v1/rerank HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n"
        b"Connection: close\r\nContent-Length: " + str(len(payload)).encode() + b"\r\n\r\n" + payload
    )
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(timeout)
    s.connect(sock_path)
    s.sendall(req)
    chunks = []
    while True:
        c = s.recv(65536)
        if not c:
            break
        chunks.append(c)
    s.close()
    raw = b"".join(chunks)
    head, _, rest = raw.partition(b"\r\n\r\n")
    status = int(head.split(b" ", 2)[1])
    return status, rest


def scores(sock_path: str, query: str, docs: list[str]) -> list[float]:
    """Scores in input order (the route returns results best-first with the original index)."""
    status, body = post(sock_path, {"query": query, "documents": docs})
    if status == 503:
        raise SystemExit(
            "/v1/rerank returned 503: the rerank model is still loading (a large model takes minutes "
            "on a cold start) or failed to load; check the sidecar log and retry"
        )
    if status == 404:
        raise SystemExit("/v1/rerank returned 404: is LOCAL_INFERENCE_RERANK_MODEL set?")
    if status != 200:
        raise SystemExit(f"/v1/rerank returned {status}")
    out = [None] * len(docs)
    for r in json.loads(body)["results"]:
        out[r["index"]] = r["relevance_score"]
    return out


def verify_batch_consistency(sock_path: str) -> None:
    """Fails loudly if a document's score depends on its position in a batch.

    Issue #796: a broken export / output read-back scored only row 0 of a batch and returned
    exactly 0 for the rest, which silently mis-ranks every multi-candidate search. Latency numbers
    from such an asset are meaningless, so this runs before any timing.
    """
    query = "what is the capital of France"
    docs = [
        "Paris is the capital of France.",
        "Bananas are yellow and grow in bunches.",
        "Berlin is the capital of Germany.",
        "The Eiffel Tower is in Paris.",
    ]
    batched = scores(sock_path, query, docs)
    for i, doc in enumerate(docs):
        alone = scores(sock_path, query, [doc])[0]
        allowed = max(0.5, 0.05 * abs(alone))
        if abs(batched[i] - alone) > allowed:
            raise SystemExit(
                f"batch self-check FAILED: document {i} scores {batched[i]:.4f} in a batch of "
                f"{len(docs)} but {alone:.4f} alone (allowed +/-{allowed:.2f}). The model asset or "
                "its output read-back is not batch-consistent; do not trust its quality or latency."
            )
    print("batch self-check passed (batched scores match solo scores)")


def percentile(xs: list[float], p: float) -> float:
    xs = sorted(xs)
    return xs[min(len(xs) - 1, max(0, round(p / 100 * (len(xs) - 1))))]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--socket", default="/tmp/liminis-inference.sock")
    ap.add_argument("--candidates", default="10,25,50")
    ap.add_argument("--doc-words", default="80,250,600", help="words per passage (~1.3 tokens/word). The model has fixed [batch, length] shapes, so Core ML time does not vary with passage length; only tokenisation and truncation to 512 tokens do")
    ap.add_argument("--iterations", type=int, default=30)
    ap.add_argument("--warmup", type=int, default=5)
    ap.add_argument("--label", default="model")
    ap.add_argument("--out", default="results")
    a = ap.parse_args()

    cands = [int(x) for x in a.candidates.split(",")]
    lens = [int(x) for x in a.doc_words.split(",")]
    verify_batch_consistency(a.socket)
    results: dict[str, dict] = {}
    for n in cands:
        for w in lens:
            docs = [passage(w, i) for i in range(n)]
            body = {"query": QUERY, "documents": docs}
            for _ in range(a.warmup):
                status, _ = post(a.socket, body)
                if status != 200:
                    raise SystemExit(f"/v1/rerank returned {status}; is LOCAL_INFERENCE_RERANK_MODEL set?")
            samples = []
            for _ in range(a.iterations):
                t0 = time.perf_counter()
                status, _ = post(a.socket, body)
                if status != 200:
                    # An error response is not a latency sample; stop rather than report it.
                    raise SystemExit(f"/v1/rerank returned {status} during timing (N={n}, words={w})")
                samples.append((time.perf_counter() - t0) * 1000)
            results[f"{n}x{w}"] = {
                "candidates": n,
                "doc_words": w,
                "p50_ms": statistics.median(samples),
                "p95_ms": percentile(samples, 95),
                "samples_ms": samples,
            }
            print(f"N={n:>3} words={w:>4}: p50 {results[f'{n}x{w}']['p50_ms']:.1f} ms")

    lines = [f"# /v1/rerank latency — {a.label}", "", "| candidates | " + " | ".join(f"{w} words p50 / p95 ms (tokenisation + truncation vary; Core ML time is fixed-shape)" for w in lens) + " |",
             "|---|" + "---|" * len(lens)]
    for n in cands:
        cells = [f"{results[f'{n}x{w}']['p50_ms']:.0f} / {results[f'{n}x{w}']['p95_ms']:.0f}" for w in lens]
        lines.append(f"| {n} | " + " | ".join(cells) + " |")
    lines += ["", f"{a.iterations} iterations after {a.warmup} warm-up calls; one UDS connection per call "
              "(so slightly pessimistic against LCG's pooled connections). Add ~15-25 ms for the "
              "first-stage embed + HNSW probe to estimate total search latency.", ""]
    md = "\n".join(lines)
    print("\n" + md)
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    (out / f"rerank-{a.label}-{stamp}.md").write_text(md)
    (out / f"rerank-{a.label}-{stamp}.json").write_text(json.dumps({"label": a.label, "results": results}, indent=2))


if __name__ == "__main__":
    main()
