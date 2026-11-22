#!/usr/bin/env python3
"""Eval-only (#863, #849): scan a retrieval-v2 graph dump for false-merge and duplicate-edge signs.

Read-only. Input is a `graph-dump.json` (`{"nodes": [...], "edges": [...]}`, as written by the
E13 probe, #815) plus the fixture's `skeleton.json`. Names are embedded with the same sidecar the
ingest-time dedup uses (BGE-base, UDS `/v1/embeddings`, no instruction prefix: the document role
is empty), so the cosines are the ones `DEDUP_THRESHOLD` (0.85, episode.rs) is compared against.

Reports, as JSON on stdout:
  * skeleton_resolution - each skeleton entity name -> the nodes carrying exactly that name
    (ci). A name with no node, or two skeleton names landing on one node, is a merge/split sign.
  * skeleton_pair_cosines - the highest same-kind cosines among the skeleton's own names, i.e.
    how close the confusable entities the corpus was built from sit to the 0.85 line.
  * node_pairs_ge_threshold / node_pairs_near - node pairs whose name cosine is >= 0.85 (these
    would have been merged at ingest unless vetoed or in a different kind) and in [0.80, 0.85).
  * duplicate_names - names carried by more than one node (entity splitting).
  * duplicate_edges - edge groups with the same (source, target, normalised fact).

usage: python3 -I name-pair-scan.py <graph-dump.json> <skeleton.json> <embedder.sock> [threshold]
"""
import collections
import http.client
import itertools
import json
import operator
import socket
import sys


class UDSConn(http.client.HTTPConnection):
    def __init__(self, path):
        super().__init__("localhost")
        self.path = path

    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(120)
        self.sock.connect(self.path)


def embed(sock, texts, batch=32):
    out = []
    for i in range(0, len(texts), batch):
        c = UDSConn(sock)
        c.request(
            "POST",
            "/v1/embeddings",
            json.dumps({"model": "x", "input": texts[i : i + batch]}),
            {"content-type": "application/json"},
        )
        r = json.loads(c.getresponse().read())
        out += [unit(d["embedding"]) for d in sorted(r["data"], key=lambda d: d["index"])]
        c.close()
    return out


def unit(v):
    n = sum(x * x for x in v) ** 0.5
    return [x / n for x in v]


def cos(a, b):
    """Cosine of two unit vectors (`embed` normalises)."""
    return sum(map(operator.mul, a, b))


def kind_of(node):
    ls = [l for l in node.get("labels", []) if l != "Entity"]
    return ls[0] if ls else "Entity"


def norm(s):
    return " ".join(s.lower().split())


def main():
    dump, skel, sock = sys.argv[1:4]
    thr = float(sys.argv[4]) if len(sys.argv) > 4 else 0.85
    g = json.load(open(dump))
    sk = json.load(open(skel))
    nodes = [n for n in g["nodes"] if n.get("kind", "Entity") == "Entity"]
    by_name = collections.defaultdict(list)
    for n in nodes:
        by_name[norm(n["name"])].append(n)

    # Skeleton resolution.
    sk_names = [e["name"] for e in sk["entities"]]
    resolution = {}
    for e in sk["entities"]:
        hits = by_name.get(norm(e["name"]), [])
        resolution[e["name"]] = {
            "kind": e["kind"],
            "nodes": [{"uuid": h["uuid"], "labels": kind_of(h)} for h in hits],
        }
    unresolved = [k for k, v in resolution.items() if not v["nodes"]]
    multi = {k: len(v["nodes"]) for k, v in resolution.items() if len(v["nodes"]) > 1}
    landing = collections.Counter(
        n["uuid"] for v in resolution.values() for n in v["nodes"]
    )
    shared = [u for u, c in landing.items() if c > 1]

    uniq = sorted({n["name"] for n in nodes})
    vec = dict(zip(uniq, embed(sock, uniq)))

    # Skeleton pair cosines, same kind.
    kinds = {e["name"]: e["kind"] for e in sk["entities"]}
    sk_pairs = sorted(
        (
            (cos(vec.get(a) or embed(sock, [a])[0], vec.get(b) or embed(sock, [b])[0]), a, b)
            for a, b in itertools.combinations(sk_names, 2)
            if kinds[a] == kinds[b]
        ),
        reverse=True,
    )[:8]

    # All node pairs (distinct nodes, any kind) at or above the near band.
    ge, near = [], []
    items = [(n, vec[n["name"]]) for n in nodes]
    for (a, va), (b, vb) in itertools.combinations(items, 2):
        if norm(a["name"]) == norm(b["name"]):
            continue
        c = cos(va, vb)
        if c >= 0.80:
            row = {
                "cos": round(c, 3),
                "a": a["name"],
                "a_kind": kind_of(a),
                "b": b["name"],
                "b_kind": kind_of(b),
                "same_kind": kind_of(a) == kind_of(b),
            }
            (ge if c >= thr else near).append(row)
    ge.sort(key=lambda r: -r["cos"])
    near.sort(key=lambda r: -r["cos"])

    dup_names = {
        k: sorted(kind_of(n) for n in v) for k, v in by_name.items() if len(v) > 1
    }

    # Duplicate edges.
    groups = collections.defaultdict(list)
    for e in g["edges"]:
        groups[(e["source_node_uuid"], e["target_node_uuid"], norm(e.get("fact", "")))].append(
            e["uuid"]
        )
    dup_edges = [v for v in groups.values() if len(v) > 1]

    print(
        json.dumps(
            {
                "dump": dump,
                "threshold": thr,
                "entity_nodes": len(nodes),
                "edges": len(g["edges"]),
                "skeleton_entities": len(sk_names),
                "skeleton_unresolved": unresolved,
                "skeleton_with_multiple_nodes": multi,
                "nodes_shared_by_two_skeleton_names": shared,
                "skeleton_pair_cosines_same_kind_top": [
                    {"cos": round(c, 3), "a": a, "b": b} for c, a, b in sk_pairs
                ],
                "node_pairs_ge_threshold": {"count": len(ge), "same_kind": sum(r["same_kind"] for r in ge), "top": ge[:25]},
                "node_pairs_near_080_085": {"count": len(near), "top": near[:15]},
                "duplicate_names": {"count": len(dup_names), "examples": dict(itertools.islice(dup_names.items(), 15))},
                "duplicate_edges": {
                    "groups": len(dup_edges),
                    "edges_in_groups": sum(len(v) for v in dup_edges),
                    "extra_edges": sum(len(v) - 1 for v in dup_edges),
                },
            },
            indent=1,
        )
    )


main()
