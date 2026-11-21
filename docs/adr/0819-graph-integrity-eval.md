# ADR-0819: `lcg-eval graph-integrity` — Deterministic Graph Scoring Over a Copied DB

**Status**: Accepted
**Date**: 2026-11-21
**Issue**: #819

## Context

Every retrieval eval scores passage ranking, which never reads an edge's type, direction or
endpoints. A graph with a third of its typed links pointing the wrong way (#816) scored well. The
retrieval-v2 fixture already carries the ground truth (`skeleton.json`: entities with kinds, facts as
`(entity, attribute, value)`), but nothing in LCG compared the extracted graph with it.

## Decision

1. **A separate subcommand, `lcg-eval graph-integrity`, with a pure scoring core.** `score()` takes
   plain `EntityRow` / `RelatesToEdge` slices, the skeleton, the mapping and optional ontology
   signatures, and returns a `Report`. Maps are ordered, so the same input gives identical JSON. This
   is the snapshot-then-pure-analysis shape of `knowledge_lint` (ADR-0755) and lets each metric be
   unit-tested on a hand-built graph with no DB.
2. **Read a copy of the DB in-process; do not start the service and do not open the original.**
   Opening a lbug DB can replay and checkpoint its WAL, and the service migrates, binds a socket,
   heals indexes and checkpoints on exit, and also requires an embedder (which would break "no
   provider"). The command copies the DB file and its `<file>.*` sidecars to a temp dir and opens the
   copy with `Db::open`, so "never writes to the workspace" is structural rather than a promise about
   a code path. Cost: a disk copy. `crates/eval/build.rs` adds `-rdynamic` (as core and service do) so
   lbug's vector/FTS extensions can resolve host symbols when opening in the `lcg-eval` binary.
3. **The skeleton → graph mapping is data**, in `graph-mapping.json` next to the skeleton, not in code
   and not inside `skeleton.json` (which `retrieval-gen --check` regenerates). Direction is declared
   per accepted relation type, relative to the fact's subject, never inferred. Accepting free-form
   names there covers graphs built without an ontology.
4. **Metric rules are defined once** (`graph_integrity/rules.rs`) and documented: normalised exact
   name match (no substring/alias/fuzzy), `labels`-based typing, word-bounded exact value text,
   split entities pooling their nodes' edges, "misattached" as an independent flag from "stated".
5. **Skeleton denominators, not E13's.** The skeleton yields 60 true links and 440 literal value
   facts, where the E13 table uses 72 and 428 (same sum). The difference is the 12 `Location` links,
   which E13 scored as links because the value resolves to a graph node; here a fact is a link only
   when its value is a skeleton entity, so `Location` is a value fact. Excluding it, the link rows
   reproduce E13 (56/71 → 44/59, 71/71 → 59/59). The mapping must name the ontology's relation types
   for every link attribute, and a test enforces it for v2.
6. **Signature marks are read from `attributes.signature_mismatch`**, the key #816 writes (ADR-0816),
   held as a named constant. Graphs built before #816 carry none, so when no edge carries it the
   report says marks are absent, never "clean". Fit / reversed / neither uses the same signature
   resolution as #816's ingest check: declared type lists resolved through the ontology
   (`effective_source_types` / `effective_target_types`), symmetric relations unconstrained.
7. **Exit codes distinguish failure classes**: 3 for an unusable workspace (so an all-zero report is
   never produced), 2 for usage or incompatible `--compare`, 1 for other errors.

## Consequences

- Graph quality becomes comparable across ingests (`--compare`), which is what fixes for
  misattached and missing value facts need in order to be verified.
- The command is not run in CI: it needs a kept workspace.
- A mismatch between the mapping's accepted names and a graph's vocabulary shows up as "not typed",
  so the mapping is the tuning point, not the code.
