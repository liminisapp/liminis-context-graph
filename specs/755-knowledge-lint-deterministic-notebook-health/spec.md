# Feature Specification: knowledge_lint — deterministic notebook health report

**Feature Branch**: `fabrik/issue-755`
**Created**: 2026-11-15
**Status**: Specified
**Input**: User description: "## Problem

Every "LLM wiki" system has a lint or health pass: Karpathy's LLM Wiki gist (contradictions, stale claims, orphans, missing links), claude-second-brain's `/lint`, agent-second-brain's health score, COG's memory-hygiene. All of them do it by having an LLM **read files**. That is slow, nondeterministic and expensive, and it scales with notebook size.

Liminis Notes is that same wiki pattern over LCG, and LCG already holds the structure these passes try to rediscover. Most notebook-health questions are **graph queries**, not LLM reading.

## Desired capability

A `knowledge_lint` read (and MCP tool, `read` scope) that returns a structured, deterministic health report for a group. Each finding cites the episodes, documents, entities or facts involved.

- **Live contradictions:** two currently-believed facts that conflict, with both sides cited.
- **Stale derived pages:** documents of origin `derived` (#752) whose supporting primary evidence has since been superseded or corrected (ADR-0705/0707).
- **Weakly supported facts:** facts supported only by derived content, or by a single primary episode.
- **Orphan and near-duplicate entities:** entities with no live support, and candidate merges above a threshold.
- **Unlinked mentions:** entities mentioned in a document that has no link to the entity's page, and frequently-mentioned entities with no page at all.

Other requirements:
- The report is paged (consistent with ADR-0667 and #691).
- Each finding carries a stable ID, so the app can track and dismiss findings over time.
- A summary block holds counts per category.
- Fix actions are out of scope: lint reports, and the app or agent decides.

## Acceptance (request level)

- On `demo-notebook`, `knowledge_lint` returns each category, with cited findings that a human can verify by opening the cited notes.
- It runs without any LLM call, and stays sub-linear or near-linear in group size.
- Stale-page detection reacts to a correction or supersession of a page's primary evidence.

## Dependencies

- **#752 (episode origin)** for the derived/primary categories. The other categories can ship first.
- #691 (relationship paging), for walking facts efficiently.
- #732 (mental models): lint is the health view over them."

## Background

"LLM wiki" systems (Karpathy's LLM Wiki gist, claude-second-brain `/lint`, agent-second-brain's health score, COG memory-hygiene) all include a health pass, and all implement it by having an LLM read the notes. That is slow, nondeterministic and costly, and the cost grows with notebook size. Liminis Notes is the same pattern layered over LCG. LCG already records the structure those passes try to rediscover:

- **Belief time** (ADR-0705): corrections end belief in an episode without deleting it, so "what used to support this fact" is recoverable.
- **Document supersession** (ADR-0707): `doc_id` plus an explicit supersession record.
- **Episode origin** (ADR-0752, #752, already merged): `primary` / `external` / `derived` / `answer`, with per-origin `support` (distinct source documents) and `corroboration` on relationship and entity reads.
- **Keyset-cursor paging** (ADR-0667) on the read path.

Most notebook-health questions are therefore graph queries. This feature turns them into one read-only, LLM-free, deterministic report that the app or an agent can poll, page through, and track over time. It reports only. Deciding what to fix, and fixing it, stays with the caller.

Facts about the current system that shape the scope:

- Ingest never invalidates one fact because of another, and `valid_at`/`invalid_at` are never written. LCG therefore holds **no existing contradiction signal**; any "live contradiction" must be derived at read time from explicit, deterministic criteria.
- LCG has no first-class notion of a note "page" for an entity, or of links between documents. Documents are addressable only through `doc_id`, and episodes carry free-form `attributes`. The unlinked-mentions category depends on data LCG may not yet hold (see Assumptions).

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Get a paged, cited health report for a group (Priority: P1)

A notebook app (or agent) calls `knowledge_lint` for a group and receives a summary of finding counts per category plus a page of findings. Each finding names its category and the episodes, documents, entities or facts involved, so a human can open those notes and verify it.

**Why this priority**: The envelope — summary, paging, stable IDs, citations, determinism — is what every category plugs into. A report with even one category is a usable slice.

**Independent Test**: Call `knowledge_lint` on a seeded group over IPC and MCP (`read` scope). Confirm the summary counts equal the total findings per category, paging with the returned cursor yields every finding exactly once, and two calls on an unchanged graph return identical findings and IDs.

**Acceptance Scenarios**:

1. **Given** a group with findings in several categories, **When** the caller requests a report, **Then** the response contains a `summary` with a count per category and a page of findings, each with `category`, a stable `id`, and citations.
2. **Given** more findings than the page limit, **When** the caller follows `next_cursor`, **Then** every finding is returned exactly once and the final page has `next_cursor: null`.
3. **Given** an unchanged graph, **When** the report is requested twice, **Then** the findings, their order and their IDs are byte-identical.
4. **Given** the caller restricts the request to a subset of categories, **When** the report runs, **Then** only those categories appear in findings and summary, and unselected categories cost no work.
5. **Given** a cursor from a different group or category selection, **When** it is replayed, **Then** it is rejected rather than silently paging a different result set (ADR-0667 behaviour).

---

### User Story 2 - Find stale derived pages (Priority: P1)

After a human corrects or supersedes a primary note, the app wants to know which agent-written (`derived`) pages restated facts from the old version and now rest on evidence that is no longer believed.

**Why this priority**: This is the headline value for the wiki pattern (derived pages silently rotting) and the acceptance criterion that must "react to a correction or supersession".

**Independent Test**: Ingest a primary document and a derived page restating its facts. Report: no stale finding. Correct the primary chunk (ADR-0705) or supersede the primary document (ADR-0707). Report: the derived page is flagged and cites both the superseded primary evidence and the stale facts.

**Acceptance Scenarios**:

1. **Given** a derived document whose facts were supported by primary episodes that are all now superseded or corrected, **When** the report runs, **Then** the derived document is a stale-page finding citing the derived document and the no-longer-believed primary episodes/documents.
2. **Given** the same derived document before the primary evidence changed, **When** the report runs, **Then** no stale-page finding exists for it.
3. **Given** a derived fact that still has at least one live primary or external supporting document, **When** the report runs, **Then** that fact does not make its page stale.
4. **Given** a derived page whose facts never had primary support, **When** the report runs, **Then** it is not a stale-page finding (it is a weak-support case instead).

---

### User Story 3 - Find weakly supported facts (Priority: P2)

A user wants to see which currently-believed facts rest only on derived or answer content, or on a single primary episode, so they know what to verify or corroborate.

**Why this priority**: High value and cheap, built directly on ADR-0752's `support` aggregation.

**Independent Test**: Seed facts with (a) primary support from two distinct documents, (b) one primary document, (c) derived content only. Only (b) and (c) are reported, each with its origin breakdown.

**Acceptance Scenarios**:

1. **Given** a believed fact whose support is entirely `derived`/`answer`, **When** the report runs, **Then** it is a weak-support finding with reason `derived_only`.
2. **Given** a believed fact supported by exactly one corroborating (primary or external) document, **When** the report runs, **Then** it is a weak-support finding with reason `single_source`.
3. **Given** a fact with two or more distinct corroborating documents, **When** the report runs, **Then** it is not reported.
4. **Given** a fact no longer believed (its episodes were superseded), **When** the report runs, **Then** it is not reported.

---

### User Story 4 - Find live contradictions (Priority: P2)

A user wants to see pairs of currently-believed facts that conflict, with both sides cited, so they can decide which is right.

**Why this priority**: Frequently requested in wiki lint, but the hardest to define deterministically without an LLM, so it is bounded to explicit criteria (see FR-009 and Assumptions).

**Independent Test**: Seed two believed facts from the same source entity with the same single-valued relation and different targets. Report: one contradiction finding citing both facts and their episodes/documents. Supersede one: the finding disappears.

**Acceptance Scenarios**:

1. **Given** two believed facts with the same source entity and a relation type designated single-valued, but different target entities, **When** the report runs, **Then** one contradiction finding cites both facts, both sides' supporting episodes/documents, and their origin breakdowns.
2. **Given** one of the two facts has been superseded, **When** the report runs, **Then** no contradiction is reported.
3. **Given** a relation type not designated single-valued, **When** the report runs, **Then** differing targets are not reported.
4. **Given** the same pair of facts, **When** the report runs repeatedly, **Then** the finding has the same ID and the same side ordering every time.

---

### User Story 5 - Find orphan and near-duplicate entities (Priority: P3)

A user wants to find entities with no live support and candidate merges of entities that look like the same thing, to keep the graph tidy.

**Why this priority**: Hygiene value; builds on existing entity data and the existing merge capability's vocabulary, but lower urgency.

**Independent Test**: Seed an entity whose every mentioning episode was superseded, and two entities with near-identical names. The orphan and the candidate pair appear; a clearly distinct pair does not.

**Acceptance Scenarios**:

1. **Given** an entity with no live mention and no live relationship, **When** the report runs, **Then** it is an orphan finding citing the entity and the (no-longer-believed) episodes that once mentioned it.
2. **Given** two entities whose similarity is at or above the threshold, **When** the report runs, **Then** one near-duplicate finding cites both entities with the similarity score and what it was based on.
3. **Given** a caller-supplied threshold, **When** it is raised, **Then** the set of candidates shrinks monotonically.
4. **Given** the report runs, **Then** no entity is merged, edited or deleted.

---

### User Story 6 - Find unlinked mentions (Priority: P3)

A user wants to know which documents mention an entity without linking to its page, and which frequently-mentioned entities have no page at all.

**Why this priority**: Matches the wiki "missing links" check, but depends on page/link data LCG may not hold yet; it must degrade honestly rather than guess.

**Independent Test**: With page and link data present for a group, seed a document that mentions an entity but does not link to its page, and a frequently-mentioned entity with no page. Both are reported. In a group with no page/link data, the category reports itself unavailable rather than returning false findings or an empty "all clear".

**Acceptance Scenarios**:

1. **Given** a document that mentions an entity that has a page, and the document has no link to that page, **When** the report runs, **Then** an unlinked-mention finding cites the document, the entity, and the page.
2. **Given** an entity mentioned in at least the configured number of distinct documents with no designated page, **When** the report runs, **Then** a missing-page finding cites the entity and its mentioning documents.
3. **Given** a group in which LCG holds no page or link data, **When** the report runs, **Then** this category is reported as `unavailable` with a reason and zero findings, and is not counted as "healthy".

---

### Edge Cases

- Empty group, or a group ID that does not exist: a well-formed empty report (all counts zero), not an error, consistent with other reads. An unknown group is not distinguishable from an empty one.
- Omitted `group_ids`: follows ADR-0667/0615 (reads broad). Each finding states its group.
- Legacy episodes with no `origin` read as `primary`; episodes with no `doc_id` count as their own document for support (ADR-0752). Lint applies the same rules and does not invent documents.
- Facts with no producing episode (asserted, cross-group, legacy) count as `primary` per ADR-0752 and are never "stale".
- A derived document partly stale: only the stale facts are cited, and the finding states how many of the document's facts are affected.
- Finding IDs must survive unrelated graph churn: adding unrelated episodes, rebuilding from the WAL, or restoring a checkpoint must not change the ID of an unchanged finding. A finding whose subject changes materially (e.g. a contradiction side is replaced) is a new finding with a new ID.
- A finding that disappears and later reappears carries the same ID (so dismissal tracking can notice a resurfaced problem).
- The graph changes between pages: cursors are keyset-based (ADR-0667), so paging never duplicates a finding but may reflect newer state; the response says so by omission, not by locking.
- Very large groups: the report must still return a first page within the performance bound (SC-004) and not materialise every finding before responding where avoidable.
- A group in degraded mode (ADR-0009) or still rebuilding: the report fails the same way other reads do, with no partial or misleading result.
- Origin is the caller's claim and is not authenticated (ADR-0752); lint inherits that and states it in the reference docs.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: LCG MUST provide a read method `knowledge_lint` and a matching MCP tool in the `read` scope. It MUST NOT mutate the graph, the WAL, or any index, and MUST NOT require write permission.
- **FR-002**: `knowledge_lint` MUST make no LLM call and no embedding-model call. Every finding MUST be computable from data already stored in the graph.
- **FR-003**: Given the same graph state and the same request, the report MUST be deterministic: identical findings, order, IDs and counts.
- **FR-004**: The request MUST accept a group scope (same semantics as other reads: omitted ⇒ all groups), an optional category selection, a page limit, and an opaque cursor. The response MUST contain `findings`, a `summary` block, and `next_cursor`.
- **FR-005**: The `summary` MUST give the total finding count for each selected category, computed over the whole scope rather than the returned page, and MUST be the same on every page of one report. A category that cannot be evaluated MUST be reported as `unavailable` with a reason, distinct from a count of zero.
- **FR-006**: Findings MUST be paged with the keyset-cursor mechanism of ADR-0667. The cursor MUST be bound to the query shape (groups, categories, thresholds), so replaying it against a different query is rejected. Paging MUST cover every finding exactly once on an unchanged graph.
- **FR-007**: Every finding MUST carry a stable `id`, a `category`, a `group_id`, and citations. The `id` MUST be a deterministic function of the category and the identities of the subject items (e.g. the cited fact, document or entity UUIDs/IDs), not of the time of the run, page position, or row order. It MUST be unchanged by unrelated graph changes, WAL rebuild, or checkpoint restore.
- **FR-008**: Citations MUST identify the involved episodes (UUIDs), documents (`doc_id`), entities (UUID and name) and facts (UUID, source, target, relation name), and MUST be sufficient for a human to locate the cited notes. Each finding MUST also carry a short machine-readable `reason` code and the numbers behind the decision (e.g. support counts per origin, similarity score).
- **FR-009**: **Live contradictions.** The report MUST list pairs of currently-believed facts (belief per ADR-0705, evaluated now) that conflict under explicit deterministic criteria: same group, same source entity, same relation type designated single-valued, different target entity. Each finding MUST cite both facts, their supporting episodes/documents, and their per-origin support. Facts not currently believed MUST NOT take part. Relation types not designated single-valued MUST NOT be reported. Where the group has no designation available, the category MUST report `unavailable` with that reason. How the designation reaches LCG is a Plan decision.
- **FR-010**: **Stale derived pages.** The report MUST list each document whose episodes are all of origin `derived` and which carries at least one fact that previously had corroborating (primary or external) support and now has none, because those supporting episodes were superseded, corrected, or their document superseded (ADR-0705/0707). The finding MUST cite the derived document, the affected facts, and the formerly supporting episodes/documents with the cause (`corrected`, `document_superseded`, or `removed`). A document with no such fact MUST NOT be reported.
- **FR-011**: Stale-page detection MUST reflect a correction or supersession as soon as it is recorded, with no re-ingest of the derived page required.
- **FR-012**: **Weakly supported facts.** The report MUST list each currently-believed fact whose corroboration (primary + external distinct documents, per ADR-0752 `support`) is zero (`derived_only`) or exactly one (`single_source`). The finding MUST include the full per-origin support breakdown. Same-fact siblings MUST be aggregated by the same key ADR-0752 uses.
- **FR-013**: **Orphan entities.** The report MUST list entities with no live mention and no live relationship, citing the entity and any superseded episodes that once mentioned it. Entities created by direct assertion with no mentions MUST NOT be reported as orphans merely for lacking mentions, if they have a live relationship; an asserted entity with nothing at all MAY be reported.
- **FR-014**: **Near-duplicate entities.** The report MUST list candidate merge pairs whose similarity is at or above a threshold, using only stored data (names and/or stored embeddings). The threshold MUST have a documented default and be overridable per request, and the response MUST echo the effective threshold. Each finding MUST cite both entities, the score, and the signal behind it. Entities in different groups MUST NOT be paired. Pairs MUST be emitted once, in a canonical order. Lint MUST NOT merge anything.
- **FR-015**: **Unlinked mentions and missing pages.** When page and link data exists in the group, the report MUST list (a) documents that mention an entity with a page but have no link to that page, and (b) entities mentioned in at least a configurable minimum number of distinct documents (documented default) that have no page. When it does not exist, FR-005's `unavailable` applies. The data convention that makes a document an entity's page, and a link between documents, is a Plan decision; it MUST NOT require an LLM or text-matching pass at lint time.
- **FR-016**: Lint MUST respect origin: derived and answer content never counts as corroboration in any category, consistent with ADR-0752.
- **FR-017**: Lint MUST NOT apply, queue or suggest fix actions. Findings describe what was found and why; remediation (merge, re-ingest, delete) remains the caller's decision. Reporting a `near_duplicate` is not a merge instruction.
- **FR-018**: Invalid input (unknown category, non-positive or over-large limit, threshold out of range, malformed cursor) MUST be rejected with the same error class as other read validation, before any work is done.
- **FR-019**: The method MUST be documented in the IPC & MCP reference (request/response schema, categories, reason codes, ID stability guarantee, threshold defaults) and in the README, with a note that origin is a caller claim. The MCP registry and its tests (tool counts and per-scope buckets) MUST be updated with the new tool.
- **FR-020**: Behaviour for requests that do not call `knowledge_lint` MUST be unchanged, and graphs/WALs written by earlier versions MUST be lintable without migration.

### Key Entities *(if applicable)*

- **Finding**: A single reported problem: `id` (stable), `category`, `group_id`, `reason`, supporting numbers, and citations. Never persisted by LCG; the app owns tracking and dismissal state.
- **Category**: One of `contradiction`, `stale_derived_page`, `weak_support`, `orphan_entity`, `near_duplicate_entity`, `unlinked_mention` (names provisional; final names are a Plan decision).
- **Summary**: Per-category total count plus availability status for the selected scope.
- **Fact**: A relationship edge, aggregated with its same-fact siblings by the ADR-0752 key for support purposes.
- **Document**: Addressed by `doc_id`; an episode without one counts as its own document.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: On `demo-notebook`, one request returns every category (or marks it `unavailable` with a reason), and for a sample of at least 10 findings across categories a reviewer can confirm each one by opening only the cited notes.
- **SC-002**: A full report runs with zero LLM and zero embedding-model calls (verified by running with those backends disabled or absent).
- **SC-003**: Repeated requests on an unchanged graph return byte-identical findings and IDs, and the IDs of unaffected findings are unchanged after a WAL rebuild and after unrelated ingests.
- **SC-004**: Report time grows near-linearly with group size: on groups of 1k, 10k and 50k episodes (the sizes already used by the benchmark suite), time for a full pass at 10× the size is no more than about 12× the time at 1×, and the first page of a single-category request returns without a full-group pass where the category allows.
- **SC-005**: After correcting or superseding a primary document, the next report lists every derived page that rested solely on it and no derived page that retains live primary support, with no re-ingest of the derived pages.
- **SC-006**: Paging a report to exhaustion returns each finding exactly once, and the summed page sizes equal the summary count for each category.
- **SC-007**: A lint run leaves the graph, the WAL, and the index state byte-for-byte unchanged.
- **SC-008**: A no-origin, pre-#755 client sees no change in any other read's response.

## Assumptions

- **#752 is merged** (ADR-0752 exists), so the derived/primary categories need no wait. #691 is treated as an efficiency aid for walking facts, not a functional prerequisite; this spec does not depend on #732 (mental models), which can consume lint later.
- **Contradiction is deliberately narrow.** With no LLM and no stored contradiction signal, the only deterministic definition that is not noisy is the single-valued-relation rule (FR-009). Broader semantic contradictions ("X is in Berlin" vs "X moved to Paris" under different relation types) are out of reach and out of scope. Research should check whether the ontology can carry the single-valued designation, or whether it must come from the request.
- **Stale means "lost its primary support", not "mentions something wrong".** A derived page is stale only when facts it restates previously had corroborating support that has since been superseded or corrected (FR-010). LCG does not record which primary note a derived page was written from; the relation is inferred through shared facts using ADR-0752's same-fact key.
- **Pages and links are not yet modelled in LCG.** The unlinked-mentions category ships to the extent the data is available and reports `unavailable` otherwise. The convention (for example episode attributes that designate an entity's page and a document's outgoing links) is left to Plan, and may be added by the app side.
- **Near-duplicate similarity** may use normalized-name similarity and stored entity embeddings; no new embedding is computed. Default threshold and minimum-mentions default are chosen in Plan and documented.
- **Single-source weak support** counts distinct documents, not distinct episodes, consistent with ADR-0752.
- **Group scope** follows ADR-0667: omitted ⇒ all groups, findings are per-group, and no finding ever pairs or spans groups (cross-group edges are out of scope for contradictions and duplicates).
- **Stable IDs are content-derived**, so dismissal and tracking live in the app and need no LCG storage.
- **Counts are exact**, computed over the whole scope on every call; no caching is required.

## Out of Scope *(optional)*

- Any fix action: merging, re-ingesting, superseding, deleting, or re-stamping origin.
- Persisting findings, dismissals, or lint history inside LCG.
- LLM-judged contradictions, stale claims, or quality scoring; semantic checks that need reading text.
- Computing a single "health score".
- Defining or maintaining notebook pages and links beyond what is needed to read them; building a page/link authoring feature.
- Mental-model (#732) health views, which can be built on lint later.
- Scheduling or push notification of lint results.

## Source References *(optional)*

- Karpathy, "LLM Wiki" gist; claude-second-brain `/lint`; agent-second-brain health score; COG memory-hygiene (prior art cited in the issue; all LLM-read-based).
- ADR-0667 (read-path paging cursor), ADR-0705 (belief time), ADR-0707 (document supersession), ADR-0752 (episode origin and corroboration), ADR-0009 (degraded mode).
- README sections "Corrections end belief, not history", "Derived pages never corroborate themselves", "Superseded is not the same as corrected".
