---
layout: default
title: Graph-Integrity Evaluation
---

# Graph-integrity evaluation: score the extracted graph against a fixture skeleton

`lcg-eval graph-integrity` is a deterministic report on the **graph** an ingest produced. It reads
the nodes and edges of an ingested workspace and scores them against a fixture's `skeleton.json`,
the ground truth the fixture was generated from. No LLM, no judge, no network and no extraction
provider is involved, and the workspace is never written.

## Why it exists

The retrieval evaluation ([retrieval-quality-evaluation.md](retrieval-quality-evaluation.md))
scores passage ranking (nDCG over notes). That path never reads an edge's type, direction or
endpoints, so a badly wrong graph can score well. It happened: about a third of typed edges on
known-true links pointed the wrong way (#816) and no test or eval had flagged it. This command makes
typing, links, direction and value-fact attachment into numbers, so a fix can be shown to move them.

It measures. It does not repair anything.

## Running it

```bash
cargo build --release -p lcg-eval
target/release/lcg-eval graph-integrity --workspace <kept-workspace> --out /tmp/gi-ontology
# before/after: deltas against an earlier report
target/release/lcg-eval graph-integrity --workspace <other-workspace> --out /tmp/gi-repaired \
  --compare /tmp/gi-ontology/report.json
```

The input is a workspace directory that a service has ingested into, for example one kept from a
retrieval run (the retrieval runner removes its temporary workspace by default, so keep one by other
means). The command accepts any workspace whose DB a service could open.

| Flag | Default | Meaning |
|---|---|---|
| `--workspace <DIR>` | required | The ingested workspace. |
| `--group <ID>` | `retrieval-eval` | Only this group is read. |
| `--skeleton <FILE>` | `crates/eval/fixtures/retrieval-v2/skeleton.json` | Ground truth. |
| `--mapping <FILE>` | `graph-mapping.json` next to the skeleton | Skeleton → graph expectations. |
| `--ontology <FILE>` | the workspace's own | Ontology for the signature section. |
| `--db-path <PATH>` | `.lcg/db/liminis.db`, then `.graphiti/db` | DB location relative to the workspace. |
| `--out <DIR>` | none | Also write `report.md` and `report.json` (the layout `lcg-eval retrieval --out` uses); with `--compare` also `compare.md`. The markdown is always printed to stdout. |
| `--compare <FILE>` | none | An earlier `report.json`; print per-metric deltas. |

**Workspace ontology default.** The group's own `.lcg/ontology/<group>.yaml` wins, then the
workspace-wide `.lcg/ontology.yaml` (the order the service resolves it); with neither, the graph is
treated as free-form and the fit section is not applicable. The report states which one was used.

**Exit codes.**

| Code | Meaning |
|---|---|
| 0 | Success. |
| 1 | Runtime error: unreadable skeleton or mapping, an invalid `--compare` file (the message names it), an unloadable `--ontology`, an unwritable `--out`. |
| 2 | Usage error, or `--compare` against a report produced with a different skeleton or mapping. |
| 3 | The workspace is unusable: missing, not an ingested workspace, its DB cannot be opened or queried (degraded or corrupt; nothing is repaired), or it holds no nodes in the group. An all-zero report is never produced. |

### How it reads without writing

Opening a lbug database can replay and checkpoint its WAL, and starting the service migrates,
binds a socket and heals indexes. So the original is never opened: the DB file and its sidecars
(`liminis.db.*`) are copied into a temporary directory and the **copy** is opened in-process
(`lcg_core::db::Db`, then the same read queries `knowledge_get_nodes_by_group` and
`knowledge_get_edges_by_group` use). The workspace itself is only read, including the ontology
files, so a run (successful or failed) leaves it byte-identical. No service process or embedder is
started. See [ADR-0819](adr/0819-graph-integrity-eval.md).

The vector/FTS extensions that opening needs are loaded the way the service loads them (a staged
bundle, or lbug's own install into your home directory). They are never written into the workspace.

## The mapping file

The mapping is data, kept next to the fixture (`crates/eval/fixtures/retrieval-v2/graph-mapping.json`),
so a second corpus or a different relation vocabulary needs no code change.

```json
{
  "mapping_version": 1,
  "kinds": { "project": { "node_types": ["Project"] }, "vendor": { "node_types": ["Vendor", "Organization"] } },
  "links": {
    "Owner": { "relation_types": [
      { "name": "OWNED_BY", "direction": "subject_to_object" },
      { "name": "OWNS",     "direction": "object_to_subject" } ] }
  },
  "junk": { "prefixes": ["recap-", "digest-"] }
}
```

- `kinds`: skeleton entity `kind` → the node type labels accepted for it.
- `links`: skeleton fact attribute → the accepted relation types. `direction` is stated **relative to
  the skeleton fact's subject** (the fact's `entity`): `subject_to_object` means the edge source is
  the subject and the target is the value entity; `object_to_subject` is the reverse. Direction is
  declared per accepted relation type and is never inferred. Accepting a free-form name
  (`MANAGES`) is how the same format covers graphs built without an ontology.
- `junk.prefixes`: first slug segments that mark a note ID (see [Junk](#junk)).
- A kind in the skeleton with no `kinds` entry, or a link-shaped attribute with no `links` entry, is
  listed in the report as **unmapped** and **excluded** from the relevant denominators; it is not
  counted as a failure.

The report records the SHA-256 of the skeleton and the mapping. `--compare` refuses two reports
whose hashes differ.

## Matching rules

Each rule is defined once, in `crates/eval/src/graph_integrity/rules.rs`.

- **Normalisation.** Lowercase; `_`, `-` and any whitespace read as one space; ends trimmed. So
  `Cedar_Core`, `CEDAR-CORE` and `cedar  core` are the same name, and `OWNED_BY` equals `owned by`.
- **Name match.** Equality of normalised names. Never a substring, never an alias, never fuzzy:
  `Cedar Core` does not match `Cedar Supply` or `Cedar`.
- **Node of an entity.** Every node whose normalised name equals the entity's. Several nodes means
  the entity is *split*; edges on any of them count as incident to the entity.
- **Type match.** Some entry of the node's `labels` (the type plus its ancestors) equals,
  case-insensitively, one of the mapping's `node_types` for the kind. The implicit `Entity` label
  never counts. `labels` is used rather than `kind`, which is an identity field that is `Entity`
  unless the ontology marks the type identity-bearing.
- **Edge type.** The stored `relation_type` when non-empty, else the edge `name`, compared through
  normalisation against the mapping's `name`s.
- **Value text.** The normalised value must occur in the normalised edge `fact` at alphanumeric word
  boundaries (so `6` does not match inside `96`). This is an exact-string rule: a fact restated
  as `4200` for `$4,200` is "not stated" although the value is present. **"Not stated" therefore
  over-counts the true misses (it is an upper bound on them) and "stated" under-counts, so "stated"
  is a lower bound.** The word-boundary containment can push the other way only in rare cases (a short
  value such as `9` found in an unrelated mention on an incident edge), so neither figure is exact.

## Metrics

All counts are reported as numerator/denominator where a denominator is meaningful, identically in
`report.md` and `report.json`.

### Typing

- **Found**: skeleton entities with at least one matching node, of all skeleton entities.
- **Split**: entities matched by two or more nodes (a count).
- **Typed**: entities, among those whose kind is mapped, with at least one matching node carrying
  an accepted type. An absent entity is not typed. The denominator is the number of mapped entities.

### Links, per attribute and in total

A *true link* is a skeleton fact whose value is itself a skeleton entity and whose attribute has a
`links` entry, de-duplicated by (subject, attribute, value entity). Per link:

- **Present**: any edge, of any type and in either direction, between a node of the subject and a
  node of the value entity.
- **Expected relation type**: among those edges, at least one whose type is accepted for the
  attribute.
- **Direction correct**: among links with the expected type, at least one expected-type edge whose
  orientation matches that relation type's declared `direction`. Judged only among edges of the
  expected type; a link with both a forward and a reversed edge counts as correct. The denominator
  is the number of links with the expected type, so `56/71` reads "56 of the 71 typed links point the
  right way".

### Value facts

A *value fact* is a skeleton fact whose attribute is not a mapped link and whose value is not a
skeleton entity, de-duplicated by (entity, attribute, value). The denominator is all of them.

- **Stated**: some edge incident to a node of the entity has the value in its `fact` text.
- **Misattached**: some such edge that states the value has, as its other endpoint, a node that is a
  *different* skeleton entity, and its `fact` text does not name that entity. For example
  "Marlin Core has a monthly budget of $4,200" stored as `Marlin Core HAS_MEASURE Tomas Kowalski`.
  An edge whose text does name the other entity (a person set the budget) is a legitimate relation,
  not misattached. The two flags are independent: that Marlin Core edge is both stated and
  misattached; each fact is counted once per flag.

### Junk

Nodes **named like a note ID**. A node is junk when it is not a skeleton entity and either its
normalised name equals a normalised note ID from the skeleton (`project-cedar-core-1`), or its raw
name is lowercase slug-shaped (`[a-z0-9]+(-[a-z0-9]+)+`), starts with one of `junk.prefixes` and has
an all-digit segment (`recap-01-aiko-osei`, `digest-047`). A node that merely looks unusual is not
junk. This is measured here and fixed elsewhere.

### Signature

- **Marks**: the number of edges carrying a stored `signature_mismatch` mark (#816), per edge type,
  read from the edge `attributes` JSON key `signature_mismatch` (null or `false` do not count; the
  key name is the constant `SIGNATURE_MARK_KEY`). When **no** edge carries the key, the report says
  marks are *absent* (a graph or build without #816) so that zero is not read as "clean".
- **Fit / reversed / neither**, per edge type, against the ontology's declared signature
  (`source_type` → `target_type`, each a name or a list since #816): *fit* when the source node's
  labels include any of the source types and the target's include any of the target types (labels
  already include ancestors); *reversed* when they satisfy it swapped and do not fit; *neither*
  otherwise. The declared types are resolved as #816's ingest check resolves them (names the
  ontology does not declare are ignored). An empty side matches anything; a relation with no
  endpoint types, and a `symmetric` relation, is counted as *unconstrained*. Edge types in the graph
  that the ontology does not declare are listed separately. With no ontology, or an ontology with no
  signatures, the section is reported as not applicable.

## Comparing two graphs

`--compare <report.json>` prints, for every metric, the earlier value, this run's value, the signed
delta (`this − earlier`) and whether it is an improvement (higher is better for found, typed, links
and stated; lower is better for split, misattached, junk, marks, reversed and neither). `run_meta`
(workspace path, version, timestamp) is ignored. For ratio metrics the verdict is judged by the
rate (`num/den`), not the numerator, because the denominator can differ between graphs (for example
direction-correct is `35/35` on a free-form graph and `56/71` under an ontology: the delta is `+21`
but the verdict is "worse rate"); such rows are marked "(denominators differ)". A different skeleton
or mapping hash exits 2, but `report.md` and `report.json` are still written to `--out` first; a file
that is not a valid report exits 1 naming the file.

## Determinism

The same workspace, skeleton, mapping and ontology give identical JSON. The only run-dependent data
(workspace path, tool version, timestamp) is in a separate `run_meta` block.

## Reconciliation with the E13 baseline

The numbers in the E13 table of #819 were produced by a one-off script outside LCG. That script and
the three graph dumps are not in this repository, so the reproduction could not be verified here. The
known difference: the script's denominators are 72 true links and 428 value facts, while this
command, applying the rules above to `skeleton.json`, finds **60** distinct true links (five
attributes × 12 pairs) and **440** distinct literal value facts. The two sum to the same 500, so the
script appears to class 12 facts as links that this command counts as value facts. If those are
identified they are a mapping change (one more link attribute), not a code change. Other residuals
to expect, all from the string rules above: the junk count depends on the note-ID rule (here an
exact ID or a prefixed slug with a digit segment), and free-form typing depends on the `node_types`
list in the mapping, which is the tuning point for a free-form graph's labels.

## Not covered

Repairing misattached or missing value facts, fixing note-ID junk nodes, running this in CI on every
PR (it needs a kept workspace), and corpora without a skeleton.
