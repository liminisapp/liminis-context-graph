# Feature Specification: lcg-eval graph-integrity — score the extracted graph against the fixture skeleton

**Feature Branch**: `fabrik/issue-819`
**Created**: 2026-11-21
**Status**: Specified
**Input**: User description: "lcg-eval graph-integrity: score the extracted graph against the fixture skeleton (typing, links, direction, value-fact attachment). Nothing in LCG checks the extracted graph against ground truth. Every retrieval eval so far has scored passage ranking (nDCG over notes), which never reads an edge's type, direction or endpoints. So the graph could be badly wrong without any number moving. It was. #816 found 34% of typed edges on known-true links pointing the wrong way, which no test or eval had flagged. It only surfaced when #815's E13 walked the graph. The retrieval-v2 fixture already has the ground truth needed: `skeleton.json` lists every entity with its kind and every fact as `(entity, attribute, value)`. A baseline computed from it, outside LCG (`docs/retrieval-experiments.md`, E13), across three graphs of the v2 corpus: skeleton entities typed correctly 35/48 (free-form), 47/48 (ontology v3.1), 47/48 (v3.1 + #816 repair); true links present (any edge between the pair) 72/72 on all three; true links with the expected relation type 35/72, 71/72, 71/72; typed true links with correct direction 35/35, 56/71, 71/71; value facts stated on an edge of the right entity 259/428, 290/428, 290/428; value facts pinned to an unrelated skeleton entity 2, 29, 29; junk nodes named after note IDs 2, 13, 13. Two of these failures have no issue yet: misattached value facts ('Marlin Core has a monthly budget of $4,200' is stored as `Marlin Core HAS_MEASURE Tomas Kowalski`; when the object of a fact is a value, the extractor pins it to whatever entity is nearby) and a third of value facts not being stated on their entity at all (138/428 under the ontology; the exact string match makes this a lower bound). Change: add `lcg-eval graph-integrity`, a deterministic graph-quality report over an ingested workspace, scored against a fixture skeleton. No LLM, no judge. Input: a workspace directory (e.g. one kept by a retrieval run), a group (default `retrieval-eval`), a skeleton (default: the retrieval-v2 fixture), and optionally the ontology the graph was built with (default: the workspace's own). It reads nodes and edges through the service the same way the retrieval runner does, and never writes. The skeleton mapping (entity kind to expected type, link attribute to relation type and direction) lives in the fixture or a small mapping file next to it, not in code. The same mapping, extended, then covers other corpora. Metrics: Typing (skeleton entities found, split across several nodes, typed as expected); Links, per attribute (present by any edge, present with the expected relation type, direction correct); Value facts (stated on an edge incident to the right entity; misattached — the edge's other end is a different skeleton entity that the fact text doesn't name); Junk (nodes named like note IDs); Signature (counts of `signature_mismatch` edges by type, read from the stored marks (#816), plus the per-type fit / reversed / neither counts against the ontology's signatures when one is given). Output: a markdown report plus JSON, in the same output layout as `lcg-eval retrieval`. A `--compare <other.json>` mode prints the deltas between two graphs. Acceptance: on the kept v2 workspaces the report reproduces the baseline table above, to within the documented string-matching rules; a unit test on a hand-built graph covers each metric (a reversed link, a misattached value fact, a split entity and a junk node are each counted exactly once); the command runs without network, without an extraction provider, and without writing to the workspace; `docs/` documents the command and every metric's definition, and the CHANGELOG has an entry. Out of scope: fixing the misattachment or the missing value facts; note-ID junk nodes (measured here, fixed separately); running it in CI on every PR; corpora without a skeleton."

## Background

Every retrieval eval to date scores passage ranking (nDCG over notes). That path never reads an edge's type, direction or endpoints, so a badly wrong graph can score well. This happened: #816 found 34% of typed edges on known-true links pointing the wrong way, and no test or eval had flagged it. It surfaced only when #815's E13 walked the graph by hand.

The retrieval-v2 fixture's `skeleton.json` already holds the ground truth: 48 entities (12 each of project, vendor, person, policy) with kinds, and per-note facts as `(entity, attribute, value, statement)`. A one-off script outside LCG (E13 in `docs/retrieval-experiments.md`) computed this baseline over three graphs of the v2 corpus:

| | free-form | ontology v3.1 | v3.1 + #816 repair |
|---|---|---|---|
| skeleton entities typed correctly | 35/48 | 47/48 | 47/48 |
| true links present (any edge between the pair) | 72/72 | 72/72 | 72/72 |
| true links with the expected relation type | 35/72 | 71/72 | 71/72 |
| typed true links with correct direction | 35/35 | 56/71 | **71/71** |
| value facts stated on an edge of the right entity | 259/428 | 290/428 | 290/428 |
| value facts pinned to an unrelated skeleton entity | 2 | 29 | 29 |
| junk nodes named after note IDs | 2 | 13 | 13 |

Two failures revealed by the baseline have no issue yet: (a) **misattached value facts**: "Marlin Core has a monthly budget of $4,200" is stored as `Marlin Core HAS_MEASURE Tomas Kowalski`, because when the object of a fact is a value the extractor pins it to a nearby entity; (b) **missing value facts**: about a third (138/428 under the ontology) are not stated on their entity at all. The exact-string-match check makes (b) a lower bound. This issue builds the measurement that lets those be fixed and verified; it does not fix them.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Score a kept workspace against the skeleton (Priority: P1)

A maintainer who has an ingested workspace (e.g. one kept from a retrieval run) runs `lcg-eval graph-integrity` against it and gets a deterministic report on typing, links, value facts, junk nodes and signature marks, computed against the retrieval-v2 skeleton. No LLM, judge, network or extraction provider is involved, and the workspace is not modified.

**Why this priority**: This is the whole point: it turns "the graph is wrong in ways no number shows" into numbers. Everything else builds on it.

**Independent Test**: Run the command on a kept v2 workspace; compare the report's numbers to the E13 baseline row for that graph; verify the workspace contents are byte-identical before and after.

**Acceptance Scenarios**:

1. **Given** a kept workspace ingested from the v2 corpus, **When** the maintainer runs `lcg-eval graph-integrity` with its path and defaults (group `retrieval-eval`, retrieval-v2 skeleton), **Then** a markdown report and a JSON file are written to the output location in the same layout `lcg-eval retrieval` uses, containing every metric defined below.
2. **Given** the three E13 graphs (free-form, ontology v3.1, v3.1 + #816 repair), **When** the command is run on each, **Then** each metric matches the corresponding baseline-table cell, to within the documented string-matching rules.
3. **Given** a machine with no network and no extraction credentials, **When** the command runs, **Then** it completes normally.
4. **Given** a workspace, **When** the command finishes (or fails), **Then** no file in the workspace has been created, modified or deleted by it.
5. **Given** a workspace path that does not exist, is not an ingested workspace, or contains no nodes in the requested group, **When** the command runs, **Then** it exits non-zero with a message saying which of these it is, rather than reporting an all-zero graph.

---

### User Story 2 - Compare two graphs (Priority: P1)

A maintainer who has changed something (applied a repair, switched ontology, changed extraction) runs `lcg-eval graph-integrity` on the new graph with `--compare <other.json>` pointing at an earlier report and sees per-metric deltas.

**Why this priority**: The motivating use is before/after measurement (#816 repair, ontology v3.1 vs. free-form, and future fixes for misattachment). Without compare, the maintainer diffs two JSON files by hand.

**Independent Test**: Produce JSON reports for two graphs; run compare; check every delta equals the difference of the two reports' values.

**Acceptance Scenarios**:

1. **Given** two reports produced by this command, **When** compare is run, **Then** the output lists each metric with both values and the signed delta, and marks which direction is an improvement.
2. **Given** a compare file produced against a different skeleton or mapping, **When** compare is run, **Then** the command refuses (or clearly warns) rather than printing meaningless deltas.
3. **Given** a compare file that is not a valid report, **When** compare is run, **Then** it exits non-zero with an error naming the file.

---

### User Story 3 - Skeleton mapping is data, not code (Priority: P2)

A maintainer adds a second corpus (or adjusts how an attribute maps to a relation type) by editing a mapping file that sits next to the fixture, with no code change.

**Why this priority**: The issue requires the mapping to be extensible to other corpora, but only v2 must work now.

**Independent Test**: Change one attribute's expected relation type or direction in the mapping file; the report's relation-type and direction results for that attribute change accordingly without rebuilding.

**Acceptance Scenarios**:

1. **Given** a mapping file declaring entity kind → expected node type and link attribute → expected relation type and direction, **When** the command is given that file (or finds it next to the skeleton), **Then** the report uses exactly those expectations.
2. **Given** a skeleton entity kind or link attribute with no mapping entry, **When** the command runs, **Then** it is reported as "unmapped" and excluded from the relation-type/direction/typing denominators (not silently counted as failures) — see Assumptions.

---

### User Story 4 - Signature and ontology fit (Priority: P2)

A maintainer who built the graph under an ontology wants to know how many edges violate relation signatures: how many `signature_mismatch` marks (#816) are stored per edge type, and, for each edge type, how many edges fit the ontology's endpoint signature, are reversed, or fit neither.

**Why this priority**: It explains *why* direction is wrong, but the skeleton-based metrics work without it.

**Independent Test**: On a hand-built graph with known signatures, one reversed edge, one fitting edge and one neither-fitting edge, the per-type counts are exactly 1/1/1.

**Acceptance Scenarios**:

1. **Given** a graph with stored `signature_mismatch` marks, **When** the command runs, **Then** the report lists the count of marked edges per edge type.
2. **Given** an ontology (explicitly passed, or the workspace's own by default), **When** the command runs, **Then** per edge type it reports fit / reversed / neither counts against that type's signature.
3. **Given** no ontology is available (free-form graph), **When** the command runs, **Then** the fit/reversed/neither section is reported as not applicable, and the rest of the report is unaffected.
4. **Given** a graph with no stored marks, **When** the command runs, **Then** the mark counts are zero and the report says marks are absent rather than implying a clean graph (see Assumptions).

---

### User Story 5 - Verified on a hand-built graph (Priority: P1)

A developer runs the unit tests and sees every metric exercised on a small hand-built graph where each defect is present exactly once.

**Why this priority**: The acceptance criteria require it, and it is the only check that runs without a kept v2 workspace.

**Independent Test**: The test builds a graph (no service, no LLM) containing exactly one reversed link, one misattached value fact, one split entity and one junk node, plus correct counterparts, and asserts each is counted exactly once.

**Acceptance Scenarios**:

1. **Given** the hand-built graph, **When** it is scored, **Then** reversed links = 1, misattached value facts = 1, split entities = 1, junk nodes = 1, and no other defect counters are non-zero.
2. **Given** the same graph, **When** scored twice, **Then** the JSON output is identical (deterministic).

---

### Edge Cases

- **Entity absent**: a skeleton entity with no matching node counts as "not found"; its links and value facts count as absent, not as errors.
- **Split entity**: several nodes match one skeleton entity (name match). It is counted once as split; its edges on any of the nodes count towards link and value-fact presence for that entity (see Assumptions).
- **Name matching**: matching is case-insensitive and whitespace-normalised; exact rules are documented. Aliases or fuzzy matches are not performed.
- **Link with multiple edges** between the same pair (different types, or both directions): present is true if any edge exists; relation-type correct if any edge carries the expected type; direction is judged only among edges of the expected type.
- **Edge of the expected type between the pair but reversed *and* forward**: direction is counted correct (a forward edge exists).
- **Value fact whose value string appears in several edges**: counted as stated once if at least one edge incident to the right entity contains it.
- **Value text matching** is exact-string after the documented normalisation; the report states this makes "not stated" a lower bound of true misses.
- **Misattachment**: the edge's other end is a skeleton entity different from the fact's entity, and the fact text does not name that entity. A value that is legitimately a relation to a named entity is not misattached.
- **Junk nodes**: "named like a note ID" is defined by the fixture's note-id pattern (e.g. `project-cedar-core-1`, `recap-01-aiko-osei`); non-skeleton nodes that merely look unusual are not junk.
- **Entity typed with several labels**: judged against the expected type according to the documented rule.
- **Multiple groups in the workspace**: only the requested group is read.
- **Workspace in use or degraded**: if the service cannot open the DB, the command fails with a clear error and does not attempt recovery or repair.
- **Empty ontology / ontology with no signatures**: the fit section is reported as not applicable.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The CLI MUST provide a `lcg-eval graph-integrity` subcommand that scores an ingested workspace's graph against a fixture skeleton, without altering the existing `lcg-eval` and `lcg-eval retrieval` command behaviour.
- **FR-002**: Inputs MUST be: a workspace directory (required), a group (default `retrieval-eval`), a skeleton (default: the retrieval-v2 fixture), and an optional ontology (default: the workspace's own ontology).
- **FR-003**: The command MUST read nodes and edges through the service, the same way the retrieval runner reads graphs, and MUST NOT write to the workspace (no files created, modified or deleted; no mutating service calls).
- **FR-004**: The command MUST run with no network access, no extraction provider configured and no LLM/judge call.
- **FR-005**: The mapping from skeleton entity kind to expected node type, and from link attribute to expected relation type and direction, MUST be loaded from data stored in or next to the fixture, not hard-coded. A v2 mapping MUST be supplied.
- **FR-006**: The report MUST include **typing** metrics: number of skeleton entities found, number split across several nodes, and number typed as expected.
- **FR-007**: The report MUST include **link** metrics per attribute and in total: true links present by any edge between the pair; present with the expected relation type; and, among those with the expected type, direction correct.
- **FR-008**: The report MUST include **value-fact** metrics: facts stated on an edge incident to the right entity; and facts misattached (the edge's other end is a different skeleton entity that the fact text does not name). Both are reported as counts over the total of value facts in the skeleton.
- **FR-009**: The report MUST include a **junk** metric: number of nodes named like note IDs.
- **FR-010**: The report MUST include **signature** metrics: the count of `signature_mismatch` edges by edge type, read from stored marks; and, when an ontology is available, per edge type the count of edges that fit the signature, are reversed against it, or fit neither.
- **FR-011**: Every metric MUST be reported as numerator and denominator (where meaningful) in both markdown and JSON, with the same values in each.
- **FR-012**: Output MUST be a markdown report plus a JSON file, written in the same output layout as `lcg-eval retrieval`.
- **FR-013**: `--compare <other.json>` MUST print per-metric values from both graphs and the signed delta, and MUST refuse to compare reports produced against different skeletons or mappings.
- **FR-014**: Results MUST be deterministic: the same workspace, skeleton, mapping and ontology yield identical JSON (excluding any explicit run-time metadata fields, which MUST be segregated and documented).
- **FR-015**: The command MUST fail with a distinct non-zero exit and clear message when the workspace is missing, not an ingested workspace, unreadable, or has no nodes in the requested group.
- **FR-016**: Name matching, value-text matching, type-label matching, split-entity handling and the "named like a note ID" rule MUST each be defined in one place and documented, so results are reproducible by others.
- **FR-017**: Skeleton kinds or attributes without a mapping entry MUST be listed as unmapped in the report and excluded from the relevant denominators.
- **FR-018**: A unit test on a hand-built graph MUST cover each metric, with a reversed link, a misattached value fact, a split entity and a junk node each counted exactly once.
- **FR-019**: `docs/` MUST document the command, its flags, the mapping file format, and the definition of every metric including the string-matching rules; the CHANGELOG MUST have an entry.

### Key Entities *(if applicable)*

- **Skeleton**: ground truth for a corpus: entities (name, kind) and facts `(entity, attribute, value, statement)`. For v2: 48 entities, 428 value facts, 72 true links.
- **Mapping**: data declaring entity kind → expected node type and link attribute → (relation type, direction), kept next to the fixture.
- **Integrity report**: the set of metrics (typing, links per attribute, value facts, junk, signature) with numerators and denominators; serialised as markdown and JSON; comparable across two graphs.
- **Signature mark**: the stored `signature_mismatch` marker (#816) on an edge.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: On each of the three kept v2 workspaces, every metric in the baseline table is reproduced to within the documented string-matching rules (any residual differences from the reference script are listed and explained in the docs).
- **SC-002**: The unit test on the hand-built graph passes, with each of the four named defects counted exactly once.
- **SC-003**: A workspace is byte-identical (file listing and contents) before and after a run, including a run that fails.
- **SC-004**: The command completes with network disabled and no provider credentials set.
- **SC-005**: A `--compare` between the ontology v3.1 report and the v3.1 + #816 repair report shows direction-correct improving from 56/71 to 71/71 and all other metrics unchanged.
- **SC-006**: A maintainer can find, in `docs/`, the definition of every metric in the report and the format of the mapping file without reading code.

## Assumptions

- **#816 marks may not exist in every graph.** `signature_mismatch` marks come from #816; on a graph or build without them the mark counts are zero and the report states that marks were absent, so zero is not read as "clean".
- **"Kept workspace" availability.** The retrieval runner currently ingests into a temporary workspace that is removed afterwards; the E13 graphs were kept by other means (#815 scratch). This issue consumes a kept workspace; making the retrieval runner keep workspaces is not required here, though the command accepts any workspace a service can open.
- **Open without writing.** Opening the workspace's DB through the service is acceptable provided the command makes no graph or file mutations; if the service must be started against the workspace, it is started in a way that does not create or modify files there (e.g. working on a read-only copy is acceptable if opening cannot be made non-mutating).
- **Matching rules** follow the E13 reference script's rules (case-insensitive, whitespace-normalised exact string match for names and values) because SC-001 needs reproduction; they are documented, and any intentional deviation is listed.
- **Split entities**: edges on any node matching a skeleton entity count as incident to that entity.
- **Unmapped kinds/attributes** are excluded from denominators rather than counted as failures.
- **Typing** compares a node's type label (as stored by the service) to the mapped expected type; for a free-form graph with no ontology types, the mapping's expected type is compared as stored, so free-form typing of 35/48 is expected to be reproduced from labels.
- **Default skeleton and mapping** are the retrieval-v2 files under `crates/eval/fixtures/retrieval-v2/`.
- **Report layout** reuses the retrieval command's output directory conventions; exact file names follow that command.

## Out of Scope *(optional)*

- Fixing misattachment of value facts, or the missing value facts (separate extraction issue, to follow).
- Fixing note-ID junk nodes (measured only).
- Running graph-integrity in CI on every PR (needs a kept workspace; may be wired into the eval workflow later).
- Corpora without a skeleton.
- Changing the retrieval runner to keep workspaces.
- Any repair or rewrite of the graph.

## Source References *(optional)*

- `docs/retrieval-experiments.md`, E13 (baseline table and reference script; three graph dumps in #815's scratch)
- #816 (direction repair, `signature_mismatch` marks), #815 (E13 graph walk)
- `crates/eval/fixtures/retrieval-v2/skeleton.json` (ground truth)
- `docs/retrieval-quality-evaluation.md` (retrieval runner and output layout)
- `docs/ontology.md` (relation signatures)
