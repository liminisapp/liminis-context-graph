#!/usr/bin/env python3
"""Drives a published v0.15.0 liminis-context-graph (lbug 0.20) over its Unix socket to write a
small notebook with non-ASCII terms in all three FTS-indexed tables."""
import json, socket, sys

sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.connect(sys.argv[1])
f = sock.makefile("r", encoding="utf-8")
n = 0
def call(method, **params):
    global n
    n += 1
    sock.sendall((json.dumps({"jsonrpc": "2.0", "id": n, "method": method, "params": params}) + "\n").encode())
    r = json.loads(f.readline())
    if "error" in r:
        raise SystemExit(f"{method} failed: {r['error']}")
    return r["result"]

G = "liminis"
for name, summary in [("Alice", "Alice lives in Zürich"), ("Bob", "Bob works with Alice"),
                      ("Zürich", "A city in Switzerland"), ("東京", "Capital of Japan")]:
    call("knowledge_assert_entity", name=name, summary=summary, group_id=G)
call("knowledge_assert_relationship", source_name="Alice", target_name="Zürich", predicate="LIVES_IN",
     fact="Alice → Zürich", group_id=G)
call("knowledge_assert_relationship", source_name="Alice", target_name="Bob", predicate="KNOWS",
     fact="Alice knows Bob", group_id=G)
for uuid, name, content in [
    ("ep-nonascii", "non-ascii episode", "Alice — moved to Zürich, then visited 東京"),
    ("ep-ascii", "ascii episode", "Bob met Alice at the office"),
]:
    call("knowledge_query_cypher", query=(
        f"CREATE (:Episodic {{uuid: '{uuid}', name: '{name}', group_id: '{G}', "
        f"created_at: timestamp('2026-01-01 00:00:00'), source: 'text', "
        f"source_description: 'fixture', content: '{content}', "
        f"valid_at: timestamp('2026-01-01 00:00:00'), entity_edges: [], attributes: '{{}}'}})"))
print(json.dumps(call("knowledge_status"), indent=1)[:600])
call("knowledge_prepare_checkpoint")
