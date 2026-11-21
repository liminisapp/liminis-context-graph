# Feature Specification: `knowledge_walk` and `knowledge_get_ontology` — caller-planned typed relation walks

**Feature Branch**: `fabrik/issue-820`
**Created**: 2026-11-21
**Status**: Specified
**Input**: User description: "## Problem

LCG can't answer a question by following typed relations. `knowledge_graph_search` uses the graph only to gather candidates, then flattens them into ranked passages. `knowledge_get_entity_neighbors` goes one hop with no ranking.

#815's E13 probe (`docs/retrieval-experiments.md`) shows what a typed walk is worth on the v2 graph questions (129 questions: 2- and 3-hop chains, lists, joins, history; Haiku 4.5 planner and reader):

| | ALL | test half |
|---|---|---|
| `qmd` | 0.33 | |
| `lcg-passages-belief-rerank` (shipped default + rerank) | 0.37 | |
| typed walk, planned by an LLM from the question and the ontology's relation menu (v5) | **0.91** | **0.94** |

That's +0.54 [+0.44, +0.64] over the best LCG passage search. The walk exists only as probe Python, so no LCG client can use it.

## Change

Two read-only IPC methods, both also exposed as MCP tools. The **planner is the caller** (the agent using LCG): the tool gives it the menu and runs the walk it plans. LCG makes no LLM call.

### `knowledge_get_ontology`

Returns the effective ontology for a group, so a caller can plan a walk:
- mode;
- entity types with descriptions and parents;
- relation types with descriptions, `source_type`/`target_type` lists, `inverse_aliases` and `symmetric` (#816).

It omits `extract: false` types unless `include_asserted: true`.

### `knowledge_walk`

**Request:**
- `group_ids`
- `start`: an entity name or UUID, resolved as `knowledge_resolve_entity` does. An ambiguous name walks from every match.
- `hops`: a list of `{relation_types: [..], phrase, direction: \"out\" | \"in\" | \"any\"}`. `phrase` is a short description of the hop used for ranking, e.g. \"supplier of the project\".
- `answer_type` (optional): an entity type name, or `\"value\"`.
- `query` (optional): the question text, used for the completion hop.
- `mode`: `\"single\" | \"list\" | \"history\"` (default `\"single\"`).
- `direction_mode`: `\"soft\" | \"hard\"` (default `\"soft\"`).

**The algorithm is E13's v5, kept generic.** No relation names or paths are hard-coded.
- **Each hop:** the candidates are edges incident to the frontier whose far end hasn't been visited. Keep those whose `relation_type` is in `relation_types`; if none match, fall back to all.
- **Direction:**
  - `out` means the frontier entity is the edge's source as its type reads; `in` means it's the target.
  - `soft` adds a small fixed bonus (0.05 cosine) to edges that match.
  - `hard` keeps only matching edges, falling back if none.
- **Last hop:** when `answer_type` is an entity type and `mode` isn't history, keep candidates whose far end has that type, falling back if none.
- **Ranking:** cosine between the hop phrase's embedding (query-side instruction) and the edge's fact embedding, plus the bonus. Keep the top B: 3 for single, 10 for list and history. Those far ends become the next frontier.
- **Completion hop:** when the hops end on no entity of `answer_type`, take one more hop to entities of that type, ranked by `query`.
- **Edges:** history mode includes invalidated edges; the other modes only live ones.

**Response:** for each hop, the chosen edges in rank order: `{uuid, relation_type, fact, source: {uuid, name, type}, target: {uuid, name, type}, valid_at, invalid_at, score, direction_matched, signature_mismatch}`. Also:
- the resolved start entities;
- whether a type filter or direction filter fell back, at which hop;
- whether a completion hop ran;
- `facts_text`: the deduplicated facts in the typed reader format E13 used, `- <fact> (<source> [<Type>] -> <target> [<Type>]) [from <date> until <date>]`.

E13's biggest single gain came from showing the reader each endpoint's type.

**MCP tool descriptions** tell the agent how to plan:
1. Call `knowledge_get_ontology` once.
2. Pick the start entity, the relation types per hop, the direction as the type name reads (\"X REPORTS_TO Y: from X to Y is `out`\"), and the answer type.
3. Fall back to `knowledge_search_passages` for questions about how something changed over time, or questions with no clear start entity. E13's planner routed history to passages, and that scored 1.00.

## Acceptance

- **Unit tests on a hand-built graph:**
  - a 2-hop chain is found;
  - `soft` ranks a direction-matching edge above an otherwise equal reversed one;
  - `hard` drops the reversed edge but falls back when nothing matches;
  - the answer-type filter and its fallback work;
  - the completion hop reaches an answer-typed entity;
  - visited nodes aren't revisited;
  - history mode includes invalidated edges.
- **Parity with the probe:** given E13's recorded v5 plans (in #815's scratch), `knowledge_walk` on the kept v2 ontology workspace returns the same edges for at least 95% of questions, and the reader score is within 0.02 of 0.91. The maintainer runs this.
- `knowledge_get_ontology` round-trips every field of an ontology using #816's schema additions.
- IPC/MCP reference, `docs/llms-full.txt` and a CHANGELOG entry. An ADR records why the caller plans and LCG makes no LLM call.
- Read-only: no WAL writes.

## Out of scope

- An LCG-side planner or answer reader (an LLM in LCG).
- Folding the walk into `knowledge_search_passages` or `knowledge_graph_search`.
- The eval runner for this (separate issue).
- Graph quality fixes: #816 (direction), #819 (integrity report), and value-fact misattachment."

## Background

LCG cannot answer a question by following typed relations. `knowledge_graph_search` uses the graph only to gather candidates and then flattens them into ranked passages; `knowledge_get_entity_neighbors` goes a single hop with no ranking. Questions that are naturally chains ("who supplies the project that X's team owns?"), lists, or joins therefore get poor answers from the passage-search tools.

The E13 probe in #815 (`docs/retrieval-experiments.md`) measured what a typed walk is worth on the v2 graph questions (129 questions: 2- and 3-hop chains, lists, joins, history; Haiku 4.5 as planner and reader). Scores (fraction correct): `qmd` 0.33, the best LCG passage search (`lcg-passages-belief-rerank`) 0.37, and an LLM-planned typed walk (v5) 0.91 overall and 0.94 on the test half. That is +0.54 [+0.44, +0.64] over the best LCG passage search. The walk exists only as probe Python, so no LCG client can use it.

The design decision is that **the planner is the caller** — the agent using LCG. LCG supplies the menu (the effective ontology) and executes the walk the caller plans. LCG itself makes no LLM call. E13's biggest single gain came from showing the reader each endpoint's entity type, so the response includes a ready-made typed fact listing.

Related work: #816 (ontology edge direction: `inverse_aliases`, `symmetric`, signature-mismatch marking), #815 (E13 probe and recorded plans), #819 (integrity report). At the time of writing, #816 is in flight on its own branch and its schema additions are not yet on `main`.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Read the ontology as a planning menu (Priority: P1)

An agent using LCG wants to plan a walk. It calls `knowledge_get_ontology` for a group and receives the effective ontology: mode, entity types (descriptions, parents) and relation types (descriptions, `source_type`/`target_type` lists, `inverse_aliases`, `symmetric`).

**Why this priority**: The walk cannot be planned without the menu, and it is independently useful for any client wanting to inspect the schema.

**Independent Test**: Load a workspace with an ontology that uses every field, call `knowledge_get_ontology`, and compare the output to the source ontology field by field.

**Acceptance Scenarios**:

1. **Given** a group with an ontology declaring entity types, relation types with signatures, inverse aliases and a symmetric relation, **When** the caller invokes `knowledge_get_ontology`, **Then** every declared field appears in the response.
2. **Given** an ontology containing types marked `extract: false`, **When** the caller omits `include_asserted`, **Then** those types are omitted; **When** it passes `include_asserted: true`, **Then** they are included.
3. **Given** a group with no ontology, **When** the caller invokes the method, **Then** it receives a well-formed response that indicates there is no ontology (free-form mode) rather than an error.

---

### User Story 2 - Walk typed relations to an answer (Priority: P1)

A caller that has planned a walk (start entity, per-hop relation types, phrases and directions, optional answer type) calls `knowledge_walk` and receives, per hop, the chosen edges ranked, plus a `facts_text` block it can hand to a reader.

**Why this priority**: This is the core value: +0.54 over the best passage search on graph questions.

**Independent Test**: On a hand-built graph, issue a two-hop walk and verify the expected edges are returned per hop in rank order, with typed endpoints and a correctly formatted `facts_text`.

**Acceptance Scenarios**:

1. **Given** a graph where A —R1→ B —R2→ C, **When** the caller walks from A with hops `[R1, R2]`, **Then** hop 1 contains the A→B edge, hop 2 contains the B→C edge, and `facts_text` lists both facts with typed endpoints.
2. **Given** an ambiguous start name matching several entities, **When** the walk runs, **Then** it starts from every match and the response reports all resolved start entities.
3. **Given** a start that resolves to nothing, **When** the walk runs, **Then** the caller receives a clear not-found result, not a silent empty walk.
4. **Given** a walk request in `list` mode, **When** executed, **Then** up to 10 edges per hop are kept, versus 3 in `single` mode.

---

### User Story 3 - Direction handling (Priority: P1)

The caller states a direction per hop ("as the type name reads": X REPORTS_TO Y, from X to Y is `out`). `soft` mode nudges ranking toward matching edges; `hard` mode filters to them.

**Why this priority**: Graph direction is unreliable in extracted graphs; the walk must tolerate it (soft) or enforce it (hard) as the caller chooses.

**Independent Test**: Build two otherwise equal edges, one matching the requested direction and one reversed; verify soft ranks the match first and hard drops the reversed edge, falling back when none match.

**Acceptance Scenarios**:

1. **Given** two edges with equal fact similarity, one matching `out` and one reversed, **When** `direction_mode` is `soft`, **Then** the matching edge ranks above the reversed one and each edge's `direction_matched` is reported.
2. **Given** the same edges, **When** `direction_mode` is `hard`, **Then** the reversed edge is not returned.
3. **Given** no edge at a hop matches the requested direction, **When** `direction_mode` is `hard`, **Then** all candidates are retained and the response records that a direction fallback occurred at that hop.
4. **Given** `direction: "any"`, **When** the walk runs, **Then** no direction bonus or filtering applies.

---

### User Story 4 - Answer type, completion hop, and history (Priority: P2)

The caller supplies `answer_type` (an entity type, or `"value"`) and optionally `query`. On the last hop the walk prefers far ends of that type; if the planned hops end without reaching an entity of that type, one extra completion hop runs. In `history` mode invalidated edges are included.

**Why this priority**: These refine accuracy but a basic walk (Stories 2–3) is still usable without them.

**Independent Test**: On a hand-built graph, verify the type filter, its fallback, the completion hop, visited-node exclusion, and invalidated-edge inclusion each behave as specified.

**Acceptance Scenarios**:

1. **Given** the last hop has candidates of several types and `answer_type` is an entity type (mode not `history`), **When** the walk runs, **Then** only answer-typed far ends are kept; if none exist, all are kept and the response records the type fallback at that hop.
2. **Given** the planned hops end on no entity of `answer_type`, **When** `query` is provided, **Then** one completion hop runs to entities of that type, ranked by `query`, and the response records that it ran.
3. **Given** a node already visited at an earlier hop, **When** later hops are computed, **Then** it is not revisited.
4. **Given** an invalidated edge incident to the frontier, **When** the mode is `history`, **Then** it is eligible and its `invalid_at` is reported; **When** the mode is `single` or `list`, **Then** it is excluded.

---

### User Story 5 - Discoverable and safely usable from MCP (Priority: P2)

An agent connected via MCP-over-stdio sees both tools with descriptions that teach it how to plan a walk and when to use `knowledge_search_passages` instead.

**Why this priority**: The planner is the caller, so the tool descriptions are the planning prompt.

**Independent Test**: List MCP tools and read the two descriptions; confirm they contain the three-step planning guidance and the fallback guidance.

**Acceptance Scenarios**:

1. **Given** an MCP client, **When** it lists tools, **Then** `knowledge_get_ontology` and `knowledge_walk` appear in the read scope with schemas matching the IPC request shapes.
2. **Given** a read-scoped MCP client, **When** it calls either tool, **Then** the call succeeds and writes nothing to the WAL.

---

### Edge Cases

- Empty `hops` list: reject with a validation error, unless `answer_type` and `query` are supplied (in which case only the completion hop could run); the behaviour must be explicit and documented.
- A hop with empty `relation_types`: treated as "no relation filter" (all candidates), not as an error and not recorded as a fallback.
- `relation_types` naming types absent from the ontology or graph: not an error; the fallback to all candidates applies and is reported.
- Frontier empty at some hop (dead end): the walk stops early and the response says at which hop.
- `answer_type: "value"`: no entity-type filter is applied (values are not entity types); the completion hop applies only to entity-type answer types.
- `answer_type` naming an unknown entity type: reported as a validation error or as a no-match fallback, consistently and documented.
- Cycles and self-loops: visited nodes are never revisited; self-loop edges do not produce a frontier.
- Edges lacking a fact embedding: handled without failure (ranked last or excluded, documented) and never crash the walk.
- An edge whose endpoint has no type: reported with an empty/absent type and rendered without a `[Type]` marker in `facts_text`.
- `signature_mismatch`: reflects whether the edge's endpoint types violate the ontology's declared `source_type`/`target_type` for its relation; false when no signature is declared.
- Multiple start entities and duplicate facts reached via different paths: `facts_text` deduplicates.
- Unknown or invalid `mode`/`direction_mode`/`direction` values: rejected with a clear validation error.
- Group with no ontology: `knowledge_walk` still works (relation types are free-form strings); `signature_mismatch` is false.
- Degraded mode (corrupt WAL / DB not open): both methods return the same degraded-mode error as other read methods.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The service MUST expose an IPC method `knowledge_get_ontology` that returns, for a group, the effective ontology: mode, entity types (name, description, parents) and relation types (name, description, `source_type` list, `target_type` list, `inverse_aliases`, `symmetric`).
- **FR-002**: `knowledge_get_ontology` MUST omit types marked `extract: false` by default and MUST include them when `include_asserted: true` is passed.
- **FR-003**: `knowledge_get_ontology` MUST round-trip every field of an ontology that uses the schema additions from #816, with no declared field silently dropped.
- **FR-004**: The service MUST expose an IPC method `knowledge_walk` accepting `group_ids`, `start`, `hops` (each `{relation_types, phrase, direction}`), optional `answer_type`, optional `query`, `mode` (`single` | `list` | `history`, default `single`) and `direction_mode` (`soft` | `hard`, default `soft`).
- **FR-005**: `knowledge_walk` MUST resolve `start` (name or UUID) the same way `knowledge_resolve_entity` does, and MUST walk from every match when the name is ambiguous.
- **FR-006**: At each hop, candidates MUST be the edges incident to the current frontier whose far end has not been visited; those whose `relation_type` is in the hop's `relation_types` are kept, and if none match, all candidates are kept and the fallback is reported.
- **FR-007**: `out` MUST mean the frontier entity is the edge's source as its relation type reads; `in` MUST mean it is the target; `any` MUST apply no direction preference.
- **FR-008**: In `soft` direction mode, edges matching the requested direction MUST receive a fixed bonus of 0.05 cosine; in `hard` mode only matching edges are kept, falling back to all candidates (and reporting it) if none match.
- **FR-009**: On the last hop, when `answer_type` is an entity type and `mode` is not `history`, candidates whose far end has that type MUST be preferred, falling back to all candidates (and reporting it) if none have it.
- **FR-010**: Candidates MUST be ranked by cosine similarity between the embedding of the hop `phrase` (embedded with the query-side instruction) and the edge's fact embedding, plus any direction bonus; the top B are kept (B = 3 in `single` mode, 10 in `list` and `history` modes) and their far ends become the next frontier.
- **FR-011**: When the planned hops end on no entity of `answer_type` (entity-type answer types only), the walk MUST run one completion hop to entities of that type, ranked by similarity to `query`, and MUST report that it ran.
- **FR-012**: `history` mode MUST include invalidated edges; `single` and `list` modes MUST include only live edges.
- **FR-013**: The `knowledge_walk` response MUST contain, per hop, the chosen edges in rank order, each with `uuid`, `relation_type`, `fact`, `source` and `target` (`uuid`, `name`, `type`), `valid_at`, `invalid_at`, `score`, `direction_matched` and `signature_mismatch`.
- **FR-014**: The response MUST also contain the resolved start entities, which hop (if any) had a relation-type, direction, or answer-type filter fall back, whether a completion hop ran, and `facts_text`.
- **FR-015**: `facts_text` MUST list the deduplicated facts, one per line, in the format `- <fact> (<source> [<Type>] -> <target> [<Type>]) [from <date> until <date>]`.
- **FR-016**: The walk algorithm MUST be generic: no relation names, entity names, or paths are hard-coded.
- **FR-017**: Both methods MUST be read-only: they MUST NOT write to the WAL or mutate the graph, and LCG MUST NOT make any LLM call in either (embedding the hop phrase and query is allowed).
- **FR-018**: Both methods MUST be exposed as MCP tools in the read scope, with the tool registry and its count assertions updated.
- **FR-019**: The `knowledge_walk` MCP tool description MUST instruct the agent to (1) call `knowledge_get_ontology` once; (2) pick the start entity, relation types per hop, direction as the type name reads (with the "X REPORTS_TO Y: from X to Y is `out`" example) and answer type; (3) fall back to `knowledge_search_passages` for questions about how something changed over time or with no clear start entity.
- **FR-020**: Invalid input (unknown `mode`, `direction_mode`, or `direction`; missing `start`; malformed `hops`) MUST yield a clear validation error, and an unresolvable `start` MUST yield an explicit not-found result.
- **FR-021**: Documentation MUST be delivered: the IPC/MCP reference, `docs/llms-full.txt`, a CHANGELOG entry, and an ADR (named per the repo's issue-number convention, `0820-…`) recording why the caller plans and LCG makes no LLM call.
- **FR-022**: Unit tests MUST cover, on a hand-built graph: a 2-hop chain; soft direction ranking; hard filtering and its fallback; answer-type filtering and fallback; the completion hop; no revisiting of visited nodes; and history mode including invalidated edges.

### Key Entities

- **Ontology (effective)**: The mode plus the entity-type and relation-type definitions that apply to a group after group/workspace resolution.
- **Hop**: One planned step: allowed relation types, a ranking phrase, and a direction.
- **Frontier**: The set of entities reached after the previous hop (initially the resolved start entities).
- **Walk edge**: A graph edge chosen at a hop, carrying typed endpoints, validity interval, score, direction-match and signature-mismatch flags.
- **Fallback record**: Which filter (relation type, direction, answer type) fell back, and at which hop.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Given E13's recorded v5 plans on the kept v2 ontology workspace, `knowledge_walk` returns the same edges as the probe for at least 95% of the 129 questions. (Run by the maintainer.)
- **SC-002**: Reader accuracy using `knowledge_walk` output on those questions is within 0.02 of the probe's 0.91. (Run by the maintainer.)
- **SC-003**: `knowledge_get_ontology` output for an ontology using every #816 field contains every declared field, verified by a round-trip test.
- **SC-004**: All seven required hand-built-graph behaviours (FR-022) pass in the unit-test suite.
- **SC-005**: Calling either method leaves the WAL and graph unchanged (verified by test).
- **SC-006**: An MCP client can discover both tools in the read scope, and their descriptions contain the planning guidance of FR-019.
- **SC-007**: The documented artifacts (IPC/MCP reference, `docs/llms-full.txt`, CHANGELOG entry, ADR) are all present.

## Assumptions

- #816's ontology schema additions (`inverse_aliases`, `symmetric`, signature-mismatch marking) land before or with this work; the ontology round-trip and `signature_mismatch` depend on them. Until then, those fields are absent in the repository on `main`.
- "Effective ontology" means the same group-then-workspace-then-none resolution used for extraction (see `docs/ontology.md`).
- Scores are cosine similarities in the embedder's space; the 0.05 bonus is a fixed constant, not a request parameter.
- Hop phrase embeddings use the same query-side instruction convention as passage search; the walk reuses the existing embedder.
- Ranking uses the edge's stored fact embedding; no new index is required.
- The completion hop ranks entities of `answer_type` by similarity of `query` to the entity (name/summary) embedding; if `query` is absent, the completion hop does not run.
- Beam widths (3 and 10) are fixed constants, not request parameters.
- When `group_ids` is omitted, the same default group scoping as other read methods applies.
- Parity with the probe is verified manually by the maintainer; no automated gate for it ships in this issue.

## Out of Scope *(optional)*

- An LCG-side planner or answer reader (any LLM inside LCG).
- Folding the walk into `knowledge_search_passages` or `knowledge_graph_search`.
- The eval runner for this method (separate issue).
- Graph quality fixes: #816 (direction), #819 (integrity report), and value-fact misattachment.

## Source References *(optional)*

- #815 — E13 probe results, `docs/retrieval-experiments.md`; recorded v5 plans in #815's scratch.
- #816 — ontology edge direction schema additions.
- `docs/ontology.md`, `docs/ipc-mcp-reference.md` — existing ontology and IPC/MCP conventions.
