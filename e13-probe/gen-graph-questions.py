"""Graph-native question set for LCG (E13 / #815), generated from the ontology's competency
questions (ontology-v3.md), instantiated against the corpus skeleton (ground truth).

The question *shapes* come from the CQs, not from the corpus: each family names the CQ path it
tests. The skeleton supplies only the entities and the gold answers. A CQ the skeleton cannot
instantiate is reported as a corpus gap, never approximated.

Gold per question:
  answers   : the set of correct answer strings (list questions have several)
  hops      : the facts a complete answer rests on, each {entity, attribute, value, notes}
              where notes = every primary note stating that current value
Current value of (entity, attribute) = the value in the latest primary note (decisions supersede).

Usage: python3 -I gen-graph-questions.py <skeleton.json> <out.jsonl>
"""
import collections, json, random, sys

skel_p, out_p = sys.argv[1:3]
S = json.load(open(skel_p))
rng = random.Random(788)
CAP = 12  # questions per family

kind = {e["name"]: e["kind"] for e in S["entities"]}
by_kind = collections.defaultdict(list)
for e in S["entities"]:
    by_kind[e["kind"]].append(e["name"])

stated = collections.defaultdict(list)  # (entity, attr) -> [(time, value, note)] primary only
for n in S["notes"]:
    if n["origin"] != "primary":
        continue
    for f in n.get("facts", []):
        stated[(f["entity"], f["attribute"])].append((n["reference_time"], f["value"], n["id"]))


def cur(e, a):
    xs = sorted(stated.get((e, a), []))
    return xs[-1][1] if xs else None


def hop(e, a):
    v = cur(e, a)
    return {"entity": e, "attribute": a, "value": v,
            "notes": sorted({nid for _, val, nid in stated[(e, a)] if val == v})}


def history(e, a):
    seq = [v for _, v, _ in sorted(stated.get((e, a), []))]
    return [v for i, v in enumerate(seq) if i == 0 or v != seq[i - 1]]


Q = []
gaps = []


def add(family, cq, text, answers, hops, shape):
    Q.append({"family": family, "cq": cq, "shape": shape, "text": text,
              "answers": answers if shape == "history" else sorted(set(answers)), "hops": hops})


def fam(name, items):
    items = [x for x in items if x]
    rng.shuffle(items)
    for f in items[:CAP]:
        add(name, *f)


P, V, H, POL = by_kind["project"], by_kind["vendor"], by_kind["person"], by_kind["policy"]

# --- chains (2 and 3 hops) -------------------------------------------------------------------
fam("supplier-contact", [
    ("A4", f"Who is the account manager at the supplier for {p}?", [cur(cur(p, "Supplier"), "Account manager")],
     [hop(p, "Supplier"), hop(cur(p, "Supplier"), "Account manager")], "chain-2")
    for p in P if cur(p, "Supplier") and cur(cur(p, "Supplier"), "Account manager")])

TERMS = {"Uptime SLA": "What uptime does the supplier for {p} commit to?",
         "Contract price": "What is the contract price agreed with {p}'s supplier?",
         "Renewal": "When does the contract with {p}'s supplier renew?"}
fam("supplier-term", [
    ("A5", t.format(p=p), [cur(cur(p, "Supplier"), a)], [hop(p, "Supplier"), hop(cur(p, "Supplier"), a)], "chain-2")
    for p in P for a, t in TERMS.items() if cur(p, "Supplier") and cur(cur(p, "Supplier"), a)])

fam("owner-manager", [
    ("R1+R2", f"Who does the owner of {p} report to?", [cur(cur(p, "Owner"), "Reports to")],
     [hop(p, "Owner"), hop(cur(p, "Owner"), "Reports to")], "chain-2")
    for p in P if cur(p, "Owner") and cur(cur(p, "Owner"), "Reports to")])

fam("owner-location", [
    ("R1+L1", f"Where is the owner of {p} based?", [cur(cur(p, "Owner"), "Location")],
     [hop(p, "Owner"), hop(cur(p, "Owner"), "Location")], "chain-2")
    for p in P if cur(p, "Owner") and cur(cur(p, "Owner"), "Location")])

fam("supplier-contact-location", [
    ("A4+L1", f"Where is the account manager for {p}'s supplier based?",
     [cur(cur(cur(p, "Supplier"), "Account manager"), "Location")],
     [hop(p, "Supplier"), hop(cur(p, "Supplier"), "Account manager"),
      hop(cur(cur(p, "Supplier"), "Account manager"), "Location")], "chain-3")
    for p in P if cur(p, "Supplier") and cur(cur(p, "Supplier"), "Account manager")
    and cur(cur(cur(p, "Supplier"), "Account manager"), "Location")])

fam("owner-manager-location", [
    ("R1+R2+L1", f"Where is the manager of {p}'s owner based?",
     [cur(cur(cur(p, "Owner"), "Reports to"), "Location")],
     [hop(p, "Owner"), hop(cur(p, "Owner"), "Reports to"), hop(cur(cur(p, "Owner"), "Reports to"), "Location")], "chain-3")
    for p in P if cur(p, "Owner") and cur(cur(p, "Owner"), "Reports to")
    and cur(cur(cur(p, "Owner"), "Reports to"), "Location")])

# --- inverse / aggregation (answers spread over many notes) ----------------------------------
def inverse(attr, subjects, obj):
    return [s for s in subjects if cur(s, attr) == obj]

fam("owned-by", [
    ("R1 inverse", f"Which projects does {h} own?", inverse("Owner", P, h),
     [hop(p, "Owner") for p in inverse("Owner", P, h)], "list") for h in H if inverse("Owner", P, h)])
fam("supplied-by", [
    ("A1 inverse", f"Which projects does {v} supply?", inverse("Supplier", P, v),
     [hop(p, "Supplier") for p in inverse("Supplier", P, v)], "list") for v in V if inverse("Supplier", P, v)])
fam("approver-of", [
    ("P2 inverse", f"Which policies can {h} approve exceptions to?", inverse("Exception approver", POL, h),
     [hop(q, "Exception approver") for q in inverse("Exception approver", POL, h)], "list")
    for h in H if inverse("Exception approver", POL, h)])
fam("reports", [
    ("R2 inverse", f"Who reports to {h}?", inverse("Reports to", H, h),
     [hop(x, "Reports to") for x in inverse("Reports to", H, h)], "list") for h in H if len(inverse("Reports to", H, h)) >= 2])

# --- joins (inverse through a second hop) ----------------------------------------------------
cities = sorted({cur(h, "Location") for h in H if cur(h, "Location")})
fam("projects-by-owner-city", [
    ("R1+L1 join", f"Which projects are owned by someone based in {c}?",
     [p for p in P if cur(p, "Owner") and cur(cur(p, "Owner"), "Location") == c],
     [x for p in P if cur(p, "Owner") and cur(cur(p, "Owner"), "Location") == c
      for x in (hop(p, "Owner"), hop(cur(p, "Owner"), "Location"))], "join")
    for c in cities if any(cur(p, "Owner") and cur(cur(p, "Owner"), "Location") == c for p in P)])
fam("projects-by-supplier-contact", [
    ("A1+A4 join", f"For which projects is {h} the account manager at the supplier?",
     [p for p in P if cur(p, "Supplier") and cur(cur(p, "Supplier"), "Account manager") == h],
     [x for p in P if cur(p, "Supplier") and cur(cur(p, "Supplier"), "Account manager") == h
      for x in (hop(p, "Supplier"), hop(cur(p, "Supplier"), "Account manager"))], "join")
    for h in H if any(cur(p, "Supplier") and cur(cur(p, "Supplier"), "Account manager") == h for p in P)])
fam("shared-supplier", [
    ("A1 shared", f"Which other projects use the same supplier as {p}?",
     [o for o in P if o != p and cur(o, "Supplier") == cur(p, "Supplier")],
     [hop(x, "Supplier") for x in [p] + [o for o in P if o != p and cur(o, "Supplier") == cur(p, "Supplier")]], "join")
    for p in P if cur(p, "Supplier") and any(o != p and cur(o, "Supplier") == cur(p, "Supplier") for o in P)])

# --- change over time (belief layer + supersession) ------------------------------------------
CHANGING = sorted({a for (e, a) in stated if kind.get(e) == "project" and len(history(e, a)) >= 3})
fam("change-history", [
    ("D1+D2", f"What has {p}'s {a} been, in order, and what is it now?", history(p, a),
     [{"entity": p, "attribute": a, "value": v,
       "notes": sorted({nid for _, val, nid in stated[(p, a)] if val == v})} for v in history(p, a)], "history")
    for p in P for a in CHANGING if len(history(p, a)) >= 3])

# --- CQs the corpus cannot instantiate --------------------------------------------------------
attrs = {a for (_, a) in stated}
for cq, need in [("P3: rule that applies to X -> its exception approver", "a policy->project 'applies to' link"),
                 ("S2: system X runs on -> where it is hosted", "a system entity with its own location"),
                 ("M1/M2: meetings, attendees, who met whom", "meeting entities with attendees"),
                 ("V1: provenance of a claim", "gold for derivation (only note origin is known)")]:
    gaps.append({"cq": cq, "missing": need})

for i, q in enumerate(Q):
    q["id"] = f"g{i:03d}"
with open(out_p, "w") as f:
    for q in Q:
        f.write(json.dumps(q) + "\n")
c = collections.Counter(q["family"] for q in Q)
print(f"{len(Q)} questions in {len(c)} families -> {out_p}")
for k, v in c.items():
    sh = next(q["shape"] for q in Q if q["family"] == k)
    avg = sum(len(q["answers"]) for q in Q if q["family"] == k) / v
    nh = sum(len(q["hops"]) for q in Q if q["family"] == k) / v
    print(f"  {k:28} {v:3}  {sh:8} answers/q {avg:4.1f}  facts/q {nh:4.1f}")
print("corpus gaps (CQs not instantiable on this skeleton):")
for g in gaps:
    print(f"  {g['cq']}  (needs {g['missing']})")
