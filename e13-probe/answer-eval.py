"""Answer accuracy on the graph question set (E13 step 2): LLM path planner + fixed reader.

Runners (same reader, same question):
  passages      : top-10 shipped-default passages (lcg-passages)
  walk-oracle   : typed walk with the family's CQ path (as graph-walk.py)
  walk-planned  : typed walk with a path an LLM planned from the question and the ontology menu
  route-planned : the planner's route: passages, or the planned walk's facts
The planner sees the question and the relation-type menu only: no families, no gold.
Reader and planner: Haiku 4.5 on the Claude subscription (llm.py), cached.
Scores: single answer = first answer matches; list/join = set F1; history = exact in-order match.
Usage: answer-eval.py <questions.jsonl> <graph-dump.json> <passages.json> <embed.sock> <ontology.yaml> <out-prefix>
"""
import collections, json, os, random, re, sys
HERE = os.path.dirname(os.path.abspath(sys.argv[0]))
sys.path.insert(0, HERE); sys.path.insert(0, os.path.join(HERE, ".."))
import yaml
from llm import map_calls
from traverse_lib import embed, cos, QP

qp, gp, pp, sock, op, outp = sys.argv[1:7]
MODEL = os.environ.get("EVAL_MODEL", "claude-haiku-4-5-20251001")
Q = [json.loads(l) for l in open(qp)]
if os.environ.get("EVAL_N"):  # smoke test: first question of each family
    seen = set(); Q = [q for q in Q if not (q["family"] in seen or seen.add(q["family"]))]
G =json.load(open(gp)); PAS = json.load(open(pp)); ONTO = yaml.safe_load(open(op))
RT = {r["name"]: r["description"] for r in ONTO["relation_types"]}
ALL = G["edges"]
for e in ALL:
    if isinstance(e.get("episode_uuids"), str):
        e["episode_uuids"] = [x.strip() for x in e["episode_uuids"].strip("[]").split(",") if x.strip()]
by = {n["uuid"]: n for n in G["nodes"]}
names = collections.defaultdict(set)
for n in G["nodes"]:
    names[n["name"].lower()].add(n["uuid"])
adj_live, adj_all = collections.defaultdict(list), collections.defaultdict(list)
for i, e in enumerate(ALL):
    for u in (e["source_node_uuid"], e["target_node_uuid"]):
        adj_all[u].append(i)
        if not e.get("invalid_at"):
            adj_live[u].append(i)
other = lambda e, u: e["target_node_uuid"] if e["source_node_uuid"] == u else e["source_node_uuid"]
fv = embed([e["fact"] for e in ALL], sock)

# ---- oracle plans (same as graph-walk.py) ----
PHRASE = {"Supplier": "supplier vendor", "Account manager": "account manager point of contact",
          "Owner": "owner accountable for", "Reports to": "reports to manager", "Location": "based in city location",
          "Exception approver": "approves exceptions to policy", "Uptime SLA": "uptime service level commitment",
          "Contract price": "contract price per unit", "Renewal": "contract renewal date"}
TYPES = {"Supplier": ["SUPPLIED_BY"], "Account manager": ["CONTACT_FOR"], "Owner": ["OWNS"], "Reports to": ["REPORTS_TO"],
         "Location": ["LOCATED_IN"], "Exception approver": ["APPROVES"], "Uptime SLA": ["HAS_TERM"],
         "Contract price": ["HAS_TERM"], "Renewal": ["HAS_TERM"]}
HIST_TYPES = ["USES", "DECISION_ON", "DECIDED", "SUPERSEDES", "LOCATED_IN", "SUPPLIED_BY", "DEPENDS_ON"]
REVERSE = {"owned-by", "supplied-by", "approver-of", "reports", "projects-by-owner-city", "projects-by-supplier-contact"}


def oracle(q):
    h = q["hops"]
    if q["shape"] == "history":
        a = h[0]["attribute"]
        return {"start": h[0]["entity"], "hops": [{"types": HIST_TYPES, "phrase": a}], "list": True, "history": True}
    if q["family"] in REVERSE:
        attrs = []
        for x in h:
            if x["attribute"] not in attrs: attrs.append(x["attribute"])
        start = h[0]["value"] if q["shape"] == "list" else h[-1]["value"]
        return {"start": start, "hops": [{"types": TYPES[a], "phrase": PHRASE[a]} for a in attrs[::-1]], "list": True, "history": False}
    return {"start": h[0]["entity"], "hops": [{"types": TYPES[x["attribute"]], "phrase": PHRASE[x["attribute"]]} for x in h],
            "list": False, "history": False}


# ---- planner ----
PLAN_SYS = ("You plan how to answer a question from a team's notebook. The notebook has a knowledge graph whose "
            "edges are typed with the relation types below, and a passage index over the notes themselves.\n\n"
            "Relation types:\n" + "\n".join(f"- {k}: {v}" for k, v in RT.items()) + "\n\n"
            "Choose route 'graph' when answering means following relations from a named entity to other entities "
            "(one or more hops), and 'passages' when the answer is an attribute of the named entity itself, or how "
            "that attribute changed over time. For 'graph', give the start entity exactly as the question names it, "
            "then the hops in order from the start towards the answer. Each hop lists 1-3 relation types that could "
            "carry it (edge direction is ignored) and a short phrase for what that hop looks for. Set list=true when "
            "the question expects several answers, history=true when it asks about earlier values.")
PLAN_SCHEMA = {"type": "object", "properties": {
    "route": {"type": "string", "enum": ["graph", "passages"]}, "start": {"type": "string"},
    "hops": {"type": "array", "items": {"type": "object", "properties": {
        "types": {"type": "array", "items": {"type": "string"}}, "phrase": {"type": "string"}}, "required": ["types", "phrase"]}},
    "list": {"type": "boolean"}, "history": {"type": "boolean"}}, "required": ["route", "start", "hops", "list", "history"]}

ET = [e["name"] for e in ONTO["entity_types"]]
PLAN2_SYS = PLAN_SYS + (" Also give answer_type: the entity type of the answer, one of: " + ", ".join(ET) +
            ", or 'value' when the answer is a literal such as a price, date or percentage. The hops must lead all the "
            "way from the start to an entity of that type.")
PLAN2_SCHEMA = json.loads(json.dumps(PLAN_SCHEMA))
PLAN2_SCHEMA["properties"]["answer_type"] = {"type": "string", "enum": ET + ["value"]}
PLAN2_SCHEMA["required"].append("answer_type")
PLAN3_SYS = PLAN2_SYS + (" For each hop also give direction: 'out' when the current entity is the subject of the relation "
            "as its type reads (X REPORTS_TO its manager: from X to the manager is 'out'), 'in' when it is the object "
            "(from a manager to the people who report to them is 'in'), or 'any' when unsure.")
PLAN3_SCHEMA = json.loads(json.dumps(PLAN2_SCHEMA))
PLAN3_SCHEMA["properties"]["hops"]["items"]["properties"]["direction"] = {"type": "string", "enum": ["out", "in", "any"]}
PLAN3_SCHEMA["properties"]["hops"]["items"]["required"].append("direction")
label = lambda u: (by[u].get("labels") or ["", ""])[-1]

hv_cache = {}


def hvec(p):
    if p not in hv_cache:
        hv_cache[p] = embed([QP + p], sock)[0]
    return hv_cache[p]


def walk(plan):
    adj = adj_all if plan.get("history") else adj_live
    B = 10 if plan.get("list") or plan.get("history") else 3
    frontier = set(names.get(plan["start"].lower(), ())); got = []
    for h in plan["hops"]:
        cand = {i for u in frontier for i in adj[u]}
        typed = {i for i in cand if ALL[i].get("relation_type") in set(h["types"])}
        cand = typed or cand  # fall back to untyped at a hop with no typed edge
        v = hvec(h["phrase"])
        top = sorted(cand, key=lambda i: -cos(v, fv[i]))[:B]
        got += top
        frontier = {other(ALL[i], u) for i in top for u in frontier if u in (ALL[i]["source_node_uuid"], ALL[i]["target_node_uuid"])}
    return got


def walk2(plan, qtext):
    """walk + no backtracking, answer-type filter on the last hop, and one completion hop towards the
    answer type when the planned hops end elsewhere. Types come from node labels; every filter falls back."""
    adj = adj_all if plan.get("history") else adj_live
    B = 10 if plan.get("list") or plan.get("history") else 3
    at = plan.get("answer_type") if plan.get("answer_type") not in (None, "value") else None
    frontier = set(names.get(plan["start"].lower(), ())); seen = set(frontier); got = []
    hops = list(plan["hops"])
    for n, h in enumerate(hops):
        cand = {(i, u) for u in frontier for i in adj[u] if other(ALL[i], u) not in seen}
        typed = {c for c in cand if ALL[c[0]].get("relation_type") in set(h["types"])}
        cand = typed or cand
        if at and n == len(hops) - 1:
            cand = {c for c in cand if label(other(ALL[c[0]], c[1])) == at} or cand
        v = hvec(h["phrase"])
        top = sorted(cand, key=lambda c: -cos(v, fv[c[0]]))[:B]
        got += [i for i, _ in top]
        frontier = {other(ALL[i], u) for i, u in top}; seen |= frontier
    if at and frontier and not any(label(u) == at for u in frontier):
        cand = {(i, u) for u in frontier for i in adj[u] if other(ALL[i], u) not in seen and label(other(ALL[i], u)) == at}
        v = hvec(qtext)
        got += [i for i, _ in sorted(cand, key=lambda c: -cos(v, fv[c[0]]))[:B]]
    return list(dict.fromkeys(got))


def walk3(plan, qtext, soft_dir=False):
    """walk2 + direction preference per hop, and no answer-type filter for history questions."""
    adj = adj_all if plan.get("history") else adj_live
    B = 10 if plan.get("list") or plan.get("history") else 3
    at = plan.get("answer_type") if plan.get("answer_type") not in (None, "value") and not plan.get("history") else None
    frontier = set(names.get(plan["start"].lower(), ())); seen = set(frontier); got = []
    hops = list(plan["hops"])
    for n, h in enumerate(hops):
        cand = {(i, u) for u in frontier for i in adj[u] if other(ALL[i], u) not in seen}
        cand = {c for c in cand if ALL[c[0]].get("relation_type") in set(h["types"])} or cand
        d = h.get("direction", "any")
        end = {"out": "source_node_uuid", "in": "target_node_uuid"}.get(d) if not plan.get("_nodir") else None
        if end and not soft_dir:
            cand = {c for c in cand if ALL[c[0]][end] == c[1]} or cand
        bonus = (lambda c: 0.05 if ALL[c[0]][end] == c[1] else 0.0) if (end and soft_dir) else (lambda c: 0.0)
        if at and n == len(hops) - 1:
            cand = {c for c in cand if label(other(ALL[c[0]], c[1])) == at} or cand
        v = hvec(h["phrase"])
        top = sorted(cand, key=lambda c: -(cos(v, fv[c[0]]) + bonus(c)))[:B]
        got += [i for i, _ in top]
        frontier = {other(ALL[i], u) for i, u in top}; seen |= frontier
    if at and frontier and not any(label(u) == at for u in frontier):
        cand = {(i, u) for u in frontier for i in adj[u] if other(ALL[i], u) not in seen and label(other(ALL[i], u)) == at}
        v = hvec(qtext)
        got += [i for i, _ in sorted(cand, key=lambda c: -cos(v, fv[c[0]]))[:B]]
    return list(dict.fromkeys(got))


def facts_ctx_typed(idx):
    """facts_ctx with each endpoint's entity type, so the reader can tell a project from a supplier."""
    lines = []
    for i in idx:
        e = ALL[i]; s, o = e["source_node_uuid"], e["target_node_uuid"]
        when = f" [from {(e.get('valid_at') or '?')[:10]}" + (f" until {e['invalid_at'][:10]}]" if e.get("invalid_at") else "]")
        lines.append(f"- {e['fact']} ({by[s]['name']} [{label(s)}] -> {by[o]['name']} [{label(o)}]){when}")
    return "Facts from the knowledge graph:\n" + ("\n".join(lines) if lines else "(none found)")


def facts_ctx(idx):
    lines = []
    for i in idx:
        e = ALL[i]
        when = f" [from {(e.get('valid_at') or '?')[:10]}" + (f" until {e['invalid_at'][:10]}]" if e.get("invalid_at") else "]")
        lines.append(f"- {e['fact']} ({by[e['source_node_uuid']]['name']} -> {by[e['target_node_uuid']]['name']}){when}")
    return "Facts from the knowledge graph:\n" + ("\n".join(lines) if lines else "(none found)")


def pas_ctx(qid):
    return "Passages from the notes:\n\n" + "\n\n".join(f"[{p['note']}, {str(p.get('valid_at') or '')[:10]}]\n{p['text']}" for p in PAS[qid])


READ_SYS = ("Answer the question using only the context given. Reply with the answer items only: each a short string "
            "such as a name, a value or a date, with no explanation. If the question asks for several items, list them all; "
            "if it asks for values in order, list them oldest first. If the context does not contain the answer, return an empty list.")
READ_SCHEMA = {"type": "object", "properties": {"answers": {"type": "array", "items": {"type": "string"}}}, "required": ["answers"]}

norm = lambda s: re.sub(r"[^a-z0-9%$.]+", " ", s.lower()).strip(" .")
match = lambda g, p: bool(p) and (norm(g) in norm(p) or (len(norm(p)) >= 3 and norm(p) in norm(g)))


def score(q, preds):
    gold = q["answers"]
    if q["shape"] == "history":
        return float(len(preds) == len(gold) and all(match(g, p) for g, p in zip(gold, preds)))
    if q["shape"] in ("list", "join"):
        tp = sum(any(match(g, p) for p in preds) for g in gold)
        prec = sum(any(match(g, p) for g in gold) for p in preds) / len(preds) if preds else 0
        rec = tp / len(gold)
        return 2 * prec * rec / (prec + rec) if prec + rec else 0.0
    return float(bool(preds) and match(gold[0], preds[0]))


# 1. plans
plans = map_calls([(MODEL, PLAN_SYS, f"Question: {q['text']}", PLAN_SCHEMA) for q in Q])
plans2 = map_calls([(MODEL, PLAN2_SYS, f"Question: {q['text']}", PLAN2_SCHEMA) for q in Q])
plans3 = map_calls([(MODEL, PLAN3_SYS, f"Question: {q['text']}", PLAN3_SCHEMA) for q in Q])
# 2. contexts per runner
ctx = {}
for q, pl in zip(Q, plans):
    ctx[(q["id"], "passages")] = pas_ctx(q["id"])
    ctx[(q["id"], "walk-oracle")] = facts_ctx(walk(oracle(q)))
    planned = facts_ctx(walk(pl)) if pl and pl.get("hops") else facts_ctx([])
    ctx[(q["id"], "walk-planned")] = planned
    ctx[(q["id"], "route-planned")] = pas_ctx(q["id"]) if (pl and pl["route"] == "passages") else planned
for q, pl in zip(Q, plans2):
    w2 = facts_ctx(walk2(pl, q["text"])) if pl and pl.get("hops") else facts_ctx([])
    ctx[(q["id"], "walk-planned2")] = w2
    ctx[(q["id"], "route-planned2")] = pas_ctx(q["id"]) if (pl and pl["route"] == "passages") else w2
for q, pl in zip(Q, plans3):
    w3 = facts_ctx_typed(walk3(pl, q["text"])) if pl and pl.get("hops") else facts_ctx([])
    ctx[(q["id"], "walk-planned3")] = w3
    ctx[(q["id"], "route-planned3")] = pas_ctx(q["id"]) if (pl and pl["route"] == "passages") else w3
for q, p2, p3 in zip(Q, plans2, plans3):
    w4 = facts_ctx_typed(walk3(dict(p2, _nodir=True), q["text"])) if p2 and p2.get("hops") else facts_ctx([])
    w5 = facts_ctx_typed(walk3(p3, q["text"], soft_dir=True)) if p3 and p3.get("hops") else facts_ctx([])
    ctx[(q["id"], "walk-planned4")] = w4
    ctx[(q["id"], "route-planned4")] = pas_ctx(q["id"]) if (p2 and p2["route"] == "passages") else w4
    ctx[(q["id"], "walk-planned5")] = w5
    ctx[(q["id"], "route-planned5")] = pas_ctx(q["id"]) if (p3 and p3["route"] == "passages") else w5
RUN = ["passages", "walk-oracle", "walk-planned", "route-planned", "walk-planned2", "route-planned2", "walk-planned3", "route-planned3",
       "walk-planned4", "route-planned4", "walk-planned5", "route-planned5"]
# EXTRA_PAS="name=path,...": more passage runners (qmd, qmd-rerank, passages-rerank), plus
# route-planned+<name>: the planner's route with that runner's passages on the passages branch.
for spec in filter(None, os.environ.get("EXTRA_PAS", "").split(",")):
    nm, path = spec.split("=", 1); X = json.load(open(path))
    pc_ = lambda qid, X=X: "Passages from the notes:\n\n" + "\n\n".join(f"[{p['note']}, {str(p.get('valid_at') or '')[:10]}]\n{p['text']}" for p in X[qid])
    for q, pl in zip(Q, plans):
        ctx[(q["id"], nm)] = pc_(q["id"])
        ctx[(q["id"], "route+" + nm)] = pc_(q["id"]) if (pl and pl["route"] == "passages") else ctx[(q["id"], "walk-planned")]
    RUN += [nm, "route+" + nm]
keys = [(q["id"], r) for q in Q for r in RUN]
reads = map_calls([(MODEL, READ_SYS, f"{ctx[k]}\n\nQuestion: {next(q['text'] for q in Q if q['id'] == k[0])}", READ_SCHEMA) for k in keys])
ans = {k: (r or {}).get("answers", []) for k, r in zip(keys, reads)}

# 3. score
S = {r: {q["id"]: score(q, ans[(q["id"], r)]) for q in Q} for r in RUN}
shapes = ["chain-2", "chain-3", "list", "join", "history"]
print(f"\n== answer accuracy ({MODEL} planner and reader); mean score")
print(f"{'runner':22}" + "".join(f"{s:>9}" for s in shapes + ["ALL"]))
for r in RUN:
    row = [sum(S[r][q["id"]] for q in Q if q["shape"] == s) / sum(1 for q in Q if q["shape"] == s) for s in shapes]
    print(f"{r:22}" + "".join(f"{x:9.2f}" for x in row) + f"{sum(S[r].values()) / len(Q):9.2f}")
DEV = set(json.load(open(os.path.join(HERE, "split-dev.json"))))
for part, keep in (("dev", lambda i: i in DEV), ("test", lambda i: i not in DEV)):
    print(f"\n-- {part} half: mean score")
    for r in RUN:
        row = [(lambda xs: sum(xs) / len(xs) if xs else float("nan"))([S[r][q["id"]] for q in Q if q["shape"] == s and keep(q["id"])]) for s in shapes]
        xs = [S[r][q["id"]] for q in Q if keep(q["id"])]
        print(f"{r:22}" + "".join(f"{x:9.2f}" for x in row) + f"{sum(xs) / len(xs):9.2f}")
rng = random.Random(788); ids = [q["id"] for q in Q]
print("\npaired difference vs passages (95% bootstrap, 10000 resamples):")
for r in RUN[1:]:
    d = [S[r][i] - S["passages"][i] for i in ids]
    bs = sorted(sum(d[rng.randrange(len(d))] for _ in d) / len(d) for _ in range(10000))
    print(f"  {r:22} {sum(d) / len(d):+.3f} [{bs[250]:+.3f}, {bs[9750]:+.3f}]")
pc = collections.Counter(p["route"] if p else "fail" for p in plans)
ok = sum(1 for q, p in zip(Q, plans) if p and p["route"] == "graph" and p["start"].lower() in names)
print(f"\nplanner routes: {dict(pc)}; graph plans whose start resolves to a node: {ok}")
by_shape = collections.defaultdict(collections.Counter)
for q, p in zip(Q, plans): by_shape[q["shape"]][p["route"] if p else "fail"] += 1
print("routes by shape:", {k: dict(v) for k, v in by_shape.items()})
json.dump({"plans": plans, "plans2": plans2, "plans3": plans3, "ctx": {f"{k[0]}|{k[1]}": v for k, v in ctx.items()}, "answers": {f"{k[0]}|{k[1]}": v for k, v in ans.items()}, "scores": S},
          open(outp + ".json", "w"), indent=1)
