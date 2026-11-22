# Feature Specification: Research — retention policy for superseded episodes

**Feature Branch**: `fabrik/issue-828`
**Created**: 2026-11-21
**Status**: Specified
**Input**: User description: "## Summary

Research whether lcg needs a retention policy for **superseded episodes**, and if so, what shape it should take.

When a document is re-ingested, lcg supersedes its stale episodes (`superseded_by`, see `crates/core/src/document/ingest.rs`) rather than deleting them. That history is deliberate: the belief and correction reads (`belief.rs`, `correction_reads.rs`) and passage rerank (ADR-0796) use it. But nothing bounds it, so a frequently revised note keeps every past revision's episodes, along with the entities and edges they support, indefinitely.

This carries forward the concern from shadoworg/liminis-framework#64, which was filed against the retired Python engine and closed as obsolete. That issue's specific problem — unbounded episode growth per `chunk_id` from add-without-replace — does not exist in lcg, because re-ingest supersedes. This issue is about the deliberate history that supersession keeps.

## Questions for research

1. **Is it a real cost?** Measure it on a realistic workspace (the demo notebook, or a synthetic history of N revisions per note). How do superseded episodes, and the entities and edges supported only by superseded episodes, grow relative to live ones? What is the effect on database size, WAL size, rebuild time and query latency?
2. **What depends on superseded history?** List every read and feature that uses superseded episodes, and say how far back each one needs: the last revision only, a time window, or all of history.
3. **If pruning is warranted, what is safe to remove?** An entity or edge supported only by pruned episodes must not survive as an orphan, and one still supported by a live episode must not be lost. Check this against the graph-is-rebuildable-from-files invariant: does pruning change what a rebuild from the markdown sources produces? (It should not, since a rebuild only sees current files.)
4. **Policy shape:** keep last N revisions, an age limit, or prune at compaction or checkpoint time. Opt-in or on by default, and configured per group or globally.

## Deliverable

A findings write-up, posted as a comment or as a draft ADR, that answers 1–2 with numbers and recommends either "no policy needed" or a concrete policy for 3–4. **No implementation in this issue**; a follow-up would be filed if a policy is recommended.

## Out of scope

- Group purge (`group_purge`, issue #361), which already exists.
- Changes to the correction or belief read APIs."

## Background

lcg keeps history on purpose: re-ingesting a document marks its stale episodes `superseded_by` rather than deleting them, and several features (belief reads, correction reads, passage rerank per ADR-0796) read that history. Nothing bounds it, so a frequently revised note accumulates every past revision's episodes, plus the entities and edges that only those episodes support.

The concern originated in shadoworg/liminis-framework#64, filed against the retired Python engine and closed as obsolete. Its specific problem, unbounded per-`chunk_id` growth from add-without-replace, does not exist in lcg. What remains is the cost of the *deliberate* history. It is currently unknown whether that cost is large enough to matter, or whether a retention policy would be safe.

This is a **research issue**: the output is a findings write-up and a recommendation, not code. If a policy is recommended, a separate implementation issue is filed afterwards.

Related existing work, for orientation: existing episode-removal methods delete only `Episodic` nodes and deliberately leave entities in place (ADR-0038, issue #219); `group_purge` (ADR-0361, issue #361) removes an entire group completely; DB→WAL dump compaction (ADR-0028) is an existing compaction point that a policy might hook into.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Quantify the cost of retained superseded history (Priority: P1)

A maintainer wants to know whether superseded episodes are a real cost, using measurements rather than intuition.

**Why this priority**: This decides whether any policy is needed at all. If the cost is negligible, the right answer is "no policy" and everything else is moot.

**Independent Test**: A reader of the write-up can reproduce the headline numbers from the described workload and method, and can see for each metric how superseded-only data compares with live data.

**Acceptance Scenarios**:

1. **Given** a realistic workspace (the demo notebook) and a synthetic workload of N revisions per note across several values of N, **When** the measurements are taken, **Then** the write-up reports counts of live episodes, superseded episodes, and entities and edges supported *only* by superseded episodes, for each N.
2. **Given** the same workloads, **When** the measurements are taken, **Then** the write-up reports database size, WAL size, rebuild time and representative query latency, and states how each grows with N relative to a live-only baseline.
3. **Given** the measurements, **When** the write-up draws a conclusion, **Then** it states plainly whether the growth is material, with the threshold or reasoning that makes it so.

---

### User Story 2 - Map what depends on superseded history (Priority: P1)

A maintainer wants to know which reads and features use superseded episodes and how much history each needs, so that any pruning can be checked against real consumers.

**Why this priority**: The safe retention depth is bounded below by the deepest consumer. Without this inventory no policy can be judged safe.

**Independent Test**: Every consumer in the inventory can be traced to a named read or feature, with a stated depth requirement (last revision only, a time window, or all of history) and a reason.

**Acceptance Scenarios**:

1. **Given** the codebase and the ADRs, **When** the inventory is compiled, **Then** it lists every read or feature that returns, filters, ranks or otherwise depends on superseded episodes, including belief reads, correction reads and passage rerank.
2. **Given** each listed consumer, **When** its depth need is assessed, **Then** the write-up classifies it as last-revision-only, time-window (with the window), or all-of-history, with justification.
3. **Given** the inventory, **When** it is reviewed, **Then** it also states which consumers would change behaviour if history were truncated to a given depth.

---

### User Story 3 - Determine what is safe to prune (Priority: P2)

If the cost is material, a maintainer wants a clear rule for which episodes, entities and edges can be removed without orphaning data or losing live data.

**Why this priority**: Only needed if User Story 1 shows a real cost and User Story 2 shows some history is expendable. It is skipped (with that stated) if the recommendation is "no policy needed".

**Independent Test**: The write-up gives a rule that can be applied to a small worked example graph, with a stated expected result for each node and edge.

**Acceptance Scenarios**:

1. **Given** an entity or edge supported only by episodes selected for pruning, **When** the safety rule is applied, **Then** the write-up specifies that it is removed rather than left orphaned.
2. **Given** an entity or edge also supported by at least one live (or retained) episode, **When** the safety rule is applied, **Then** the write-up specifies that it is kept.
3. **Given** the graph-is-rebuildable-from-files invariant, **When** pruning is compared with a rebuild from the markdown sources, **Then** the write-up states whether pruning changes what a rebuild produces, with the reasoning.
4. **Given** the existing episode-removal semantics (ADR-0038) and group purge (ADR-0361), **When** the rule is proposed, **Then** the write-up explains how it differs from or reuses them.

---

### User Story 4 - Recommend a policy shape or "no policy" (Priority: P2)

A maintainer wants a concrete recommendation they can turn into an implementation issue, or an explicit decision that none is needed.

**Why this priority**: This is the actionable end of the research, but it depends on Stories 1–3.

**Independent Test**: The recommendation names one option on each axis below, with reasons, or states "no policy needed" with the evidence that justifies it.

**Acceptance Scenarios**:

1. **Given** the findings, **When** the recommendation is written, **Then** it chooses between keep-last-N-revisions, an age limit, and pruning at compaction or checkpoint time, and says why the others were not chosen.
2. **Given** the findings, **When** the recommendation is written, **Then** it states whether the policy should be opt-in or on by default, and whether it is configured per group or globally.
3. **Given** a recommended policy, **When** the write-up concludes, **Then** it lists the follow-up issue(s) to file. **Given** "no policy needed", **Then** it states what measurement result would change that conclusion.

---

### Edge Cases

- A note whose revisions oscillate between two contents (A, B, A): the write-up should say whether entities supported by an older revision are still supported by a live one, and whether that affects the retention rule.
- An entity or edge shared across many notes or groups, supported by a mix of live and superseded episodes.
- A note deleted from disk: its episodes are not "superseded by" a later revision, so the write-up should state whether they fall inside or outside this question.
- Episodes with no attribution (see ADR-0292, which never supersedes unattributed episodes): these are never superseded and are not subject to retention.
- A workload where each revision changes little, so chunks are largely identical across revisions, versus one where revisions rewrite the whole note.
- Measurements that differ markedly between a small demo workspace and a large production-scale WAL (ADR-0026 cites tens of thousands of WAL files).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The deliverable MUST report, for the demo notebook and for synthetic histories of N revisions per note (at least three values of N), the counts of live episodes, superseded episodes, and entities and edges supported only by superseded episodes.
- **FR-002**: The deliverable MUST report database size, WAL size, rebuild time and representative query latency for those workloads, stating how each scales with N relative to a live-only baseline.
- **FR-003**: The deliverable MUST describe the measurement method (workload generation, hardware, commands) well enough for another person to reproduce the numbers.
- **FR-004**: The deliverable MUST inventory every read and feature that depends on superseded episodes, including belief reads, correction reads and passage rerank (ADR-0796).
- **FR-005**: The deliverable MUST classify each inventoried consumer's required history depth as last revision only, a time window, or all of history, with justification.
- **FR-006**: The deliverable MUST state a clear conclusion on whether the retained history is a material cost, and the threshold or reasoning behind that conclusion.
- **FR-007**: If recommending pruning, the deliverable MUST define a safety rule under which an entity or edge supported only by pruned episodes is removed and one supported by any retained episode is kept.
- **FR-008**: If recommending pruning, the deliverable MUST state whether pruning changes the result of a rebuild from the markdown sources, and whether the graph-is-rebuildable-from-files invariant still holds.
- **FR-009**: If recommending pruning, the deliverable MUST recommend a policy shape (keep-last-N, age limit, or at compaction/checkpoint time), opt-in vs. default-on, and per-group vs. global configuration, with reasons.
- **FR-010**: The deliverable MUST be a findings write-up posted as an issue comment or a draft ADR, and its recommendation MUST be one of "no policy needed" or a concrete policy covering FR-007 to FR-009. If a draft ADR is used, it MUST be named `docs/adr/0828-<slug>.md` per the repository's ADR-numbering rule.
- **FR-011**: If a policy is recommended, the deliverable MUST name the follow-up implementation issue(s) to be filed; if "no policy needed", it MUST state what evidence would reopen the question.
- **FR-012**: This issue MUST NOT include implementation: no production code, schema or API changes, and no change to the belief or correction read APIs.
- **FR-013**: Any scratch measurement scripts or harness used MUST NOT be left as shipped product code, and the measured workspace MUST NOT be mutated in the main checkout.

### Key Entities *(if applicable)*

- **Episode (live / superseded)**: A unit of ingested content. A superseded episode has been replaced by a later revision of the same source and is marked as such, but is retained.
- **Entity and edge**: Graph facts extracted from episodes; each is supported by one or more episodes. Support by only superseded episodes is the "orphan-on-prune" risk.
- **Revision history**: The ordered chain of an individual note's episodes across re-ingests.
- **Retention policy**: A proposed rule bounding how much superseded history is kept (the object of the recommendation).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: The write-up contains a table of episode, entity and edge counts (live vs. superseded-only) for at least three revision-depth values, plus the demo notebook.
- **SC-002**: The write-up contains measured database size, WAL size, rebuild time and query latency for each workload, each with a stated growth relative to baseline.
- **SC-003**: Every read or feature depending on superseded episodes is listed with a depth classification; a reviewer finds no consumer missing when checking the superseded-episode code paths.
- **SC-004**: The write-up ends in an unambiguous recommendation (either "no policy needed" or a concrete policy across the safety, shape, default and scope questions) that a maintainer can turn directly into a follow-up issue without further research.
- **SC-005**: No production code, schema or API change is merged as part of this issue.

## Assumptions

- A synthetic history generator can be built from scratch scripts that re-ingest edited markdown through the normal ingest path; the demo notebook is available as the realistic baseline.
- "Material cost" is judged by the researcher against stated reasoning (for example, growth relative to live data and impact on rebuild time or latency); the maintainer reviews that judgment on the write-up, so no numeric threshold is fixed in advance.
- A draft ADR is preferred over a comment if a concrete policy is recommended; a comment is sufficient for "no policy needed".
- Rebuild from markdown sources sees only current files, so pruning of superseded history is expected not to change a rebuild's output; this is verified, not assumed, in the write-up.
- Measurements are taken in the debug or release profile as practical, within the repository's long-running command constraints, and the profile is reported.

## Out of Scope *(optional)*

- Group purge (`group_purge`, issue #361), which already exists.
- Changes to the correction or belief read APIs.
- Any implementation of a retention policy (to be a separate follow-up issue if recommended).
- The retired Python engine's add-without-replace growth (liminis-framework#64), which does not occur in lcg.

## Source References *(optional)*

- shadoworg/liminis-framework#64 (closed as obsolete; origin of the concern)
- `docs/adr/0796-passage-rerank-and-query-expansion.md`
- `docs/adr/0361-group-scoped-purge.md`
- `docs/adr/0028-db-wal-dump-compaction.md`
- `docs/adr/0292-chunk-ingest-never-supersedes-unattributed-episodes.md`
