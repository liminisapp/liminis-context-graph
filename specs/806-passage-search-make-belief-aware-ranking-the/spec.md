# Feature Specification: Passage search: make belief-aware ranking the default

**Feature Branch**: `fabrik/issue-806`
**Created**: 2026-11-18
**Status**: Specified
**Input**: User description: "## Problem

#789 (PR #798) added belief-aware passage ranking as an opt-in request parameter (`belief_aware: true`). It is the largest retrieval gain measured so far.

On the v2 corpus (`meridian-notebook-2` / `meridian-questions-3`, 244 questions; k = 10; BGE base v1.5; Haiku 4.5 extraction replayed from the committed cassette; Sonnet 4.6 judge), nDCG@10:

| config | overall | direct | paraphrase | temporal | answer | multi-hop | p50 |
|---|---|---|---|---|---|---|---|
| `lcg-passages` (vector, today's default) | 0.568 | 0.756 | 0.150 | 0.628 | 0.789 | 0.662 | 24 ms |
| `lcg-passages-belief` | **0.649** | 0.815 | 0.161 | **0.802** | 0.944 | **0.746** | 25 ms |
| `lcg-passages-hybrid-belief` | **0.652** | 0.846 | 0.111 | **0.802** | **0.971** | 0.728 | 27 ms |

- **Vector + belief vs vector:** +0.081 [+0.063, +0.100] overall.
- **Temporal:** +0.174 [+0.114, +0.238].
- **Latency:** essentially unchanged.

Every client that doesn't pass the parameter misses the gain. That includes the Liminis Notes agent today.

## Proposal

1. **Make belief-aware ranking the default for `knowledge_search_passages`**, in both `vector` and `hybrid` modes.
   - An explicit `belief_aware: false` still turns it off.
   - A workspace switch, `LCG_PASSAGE_BELIEF_AWARE=off`, restores the old default for anyone who needs it.
2. **Keep `vector` as the default mode.** Hybrid + belief is indistinguishable from vector + belief overall, and is worse on paraphrase (0.111 vs 0.161).
3. **Update the MCP tool description, the IPC reference, `docs/configuration.md` and the CHANGELOG.** The CHANGELOG entry is a behaviour change: results are re-ordered by default.
4. **Eval harness:** `lcg-passages` keeps meaning \"the shipped default\", which is now belief-aware. Add `lcg-passages-raw` (`belief_aware: false`) so the old baseline stays measurable.

## Acceptance

- Requests without `belief_aware` get belief-aware ranking. `belief_aware: false` and `LCG_PASSAGE_BELIEF_AWARE=off` each restore the previous ordering byte-for-byte. Test both.
- Existing tests that assert the old default order are updated, and the PR description lists each one, with a line on why the new order is right.
- No other read changes.

## Context

Depends on #789 / PR #798 being merged. The maintainer will re-run v1 and v2 on the PR head; workers don't need the embedder sidecar."

## Background

#789 (PR #798, now merged) added belief-aware passage ranking to `knowledge_search_passages` behind a `belief_aware` request parameter. Belief-aware ranking places a derived page below the primary episode it cites, and a superseded episode below its successor. On the v2 retrieval corpus (244 questions, nDCG@10) it is the largest retrieval gain measured so far:

| config | overall | direct | paraphrase | temporal | answer | multi-hop | p50 |
|---|---|---|---|---|---|---|---|
| `lcg-passages` (vector, today's default) | 0.568 | 0.756 | 0.150 | 0.628 | 0.789 | 0.662 | 24 ms |
| `lcg-passages-belief` | **0.649** | 0.815 | 0.161 | **0.802** | 0.944 | **0.746** | 25 ms |
| `lcg-passages-hybrid-belief` | **0.652** | 0.846 | 0.111 | **0.802** | **0.971** | 0.728 | 27 ms |

Vector + belief beats vector by +0.081 nDCG@10 overall [+0.063, +0.100] and by +0.174 on temporal questions [+0.114, +0.238], with essentially unchanged latency. Hybrid + belief is indistinguishable from vector + belief overall and worse on paraphrase (0.111 vs 0.161), so `vector` stays the default mode.

Today the gain is opt-in in vector mode, so every client that does not pass the parameter misses it. That includes the Liminis Notes agent. (As shipped by #798, hybrid mode already defaults to belief-aware on; vector mode defaults to off. The practical effect of this issue is therefore to change the vector-mode default and to add a workspace-level switch and a way to measure the raw baseline.)

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Clients get belief-aware ranking without opting in (Priority: P1)

A client such as the Liminis Notes agent calls `knowledge_search_passages` with no `mode` and no `belief_aware`. It receives results ranked with the belief-aware rules, so derived pages sit below the primary episodes they cite and superseded episodes sit below their successors.

**Why this priority**: This is the whole point of the issue. It delivers the measured +0.081 nDCG@10 to every existing client with no client change.

**Independent Test**: On a corpus with a derived page that cites a primary episode and a superseded episode with a successor, call the method with neither parameter and verify that the result order matches an explicit `belief_aware: true` call, and differs from an explicit `belief_aware: false` call.

**Acceptance Scenarios**:

1. **Given** no `mode` and no `belief_aware` in the request and no workspace switch set, **When** passages are searched, **Then** the mode is `vector` and belief-aware ranking is applied.
2. **Given** `mode: "hybrid"` and no `belief_aware`, **When** passages are searched, **Then** belief-aware ranking is applied.
3. **Given** the same request with an explicit `belief_aware: true`, **When** passages are searched, **Then** the result order is identical to scenarios 1 and 2 respectively.

---

### User Story 2 - A request can opt out per call (Priority: P1)

A caller that needs the pre-change ordering (for a diff, a regression, or a baseline) sends `belief_aware: false`. It gets exactly the ordering the service produced before this change.

**Why this priority**: The behaviour change re-orders results by default. A per-request escape hatch is what makes that safe to ship.

**Independent Test**: Compare the result of `belief_aware: false` in vector mode with a snapshot of the pre-change default vector output on the same corpus; they must be identical.

**Acceptance Scenarios**:

1. **Given** `belief_aware: false` and the default mode, **When** passages are searched, **Then** the results (order, ids and scores) are byte-for-byte what the previous default vector search returned.
2. **Given** `mode: "hybrid"` and `belief_aware: false`, **When** passages are searched, **Then** the results are byte-for-byte what the same request returned before this change.
3. **Given** the workspace switch is set to `off` and the request sets `belief_aware: true`, **When** passages are searched, **Then** belief-aware ranking is applied (the explicit request value wins over the switch).

---

### User Story 3 - An operator can restore the old default workspace-wide (Priority: P2)

An operator who cannot change client code, or who needs the old ordering for a while, sets `LCG_PASSAGE_BELIEF_AWARE=off` for the service. Requests that omit `belief_aware` then behave as they did before this change.

**Why this priority**: A safety valve for deployments with downstream expectations about ordering. Less urgent than the default itself because per-request opt-out already exists.

**Independent Test**: Start the service with `LCG_PASSAGE_BELIEF_AWARE=off`, send a vector-mode request with no `belief_aware`, and compare to the pre-change default output on the same corpus.

**Acceptance Scenarios**:

1. **Given** `LCG_PASSAGE_BELIEF_AWARE=off` and a vector-mode request without `belief_aware`, **When** passages are searched, **Then** the results are byte-for-byte the pre-change default vector results.
2. **Given** `LCG_PASSAGE_BELIEF_AWARE` is unset, **When** passages are searched without `belief_aware`, **Then** belief-aware ranking is applied.
3. **Given** `LCG_PASSAGE_BELIEF_AWARE` holds a value the service does not recognise, **When** the service starts or the first request is served, **Then** the service behaves as if it were unset and tells the operator the value was ignored, consistent with how an unrecognised `LCG_PASSAGE_SEARCH_MODE` is handled.

---

### User Story 4 - The eval harness measures both the shipped default and the raw baseline (Priority: P2)

A maintainer running the retrieval eval sees `lcg-passages` reporting the shipped default (now belief-aware vector), and can also run `lcg-passages-raw` to measure the old non-belief baseline.

**Why this priority**: Without it, the +0.081 gain can no longer be reproduced or tracked for regressions, because the old baseline would disappear.

**Independent Test**: Run the eval harness's configuration listing and a dry run. `lcg-passages` must send the same request a default client would send, and `lcg-passages-raw` must send `belief_aware: false`.

**Acceptance Scenarios**:

1. **Given** the eval harness, **When** the `lcg-passages` configuration runs, **Then** it exercises the service's default behaviour (belief-aware, vector mode).
2. **Given** the eval harness, **When** the `lcg-passages-raw` configuration runs, **Then** it sends `belief_aware: false` and reproduces the pre-change `lcg-passages` ranking.
3. **Given** the other existing `lcg-passages-*` configurations, **When** they run, **Then** their meaning is unchanged.

---

### User Story 5 - Documentation and tests state the new default accurately (Priority: P2)

A client author or reviewer reading the MCP tool description, the IPC reference, `docs/configuration.md` and the CHANGELOG learns that results are belief-aware by default, how to turn it off per request, and how to turn it off per workspace. A reviewer of the PR sees every existing test whose expected order changed, with a reason.

**Why this priority**: The change silently re-orders results for every caller, so it must be discoverable and auditable.

**Independent Test**: Read each document and confirm none still describes belief-aware ranking as off by default in vector mode; check the PR description for the list of changed tests.

**Acceptance Scenarios**:

1. **Given** the updated docs, **When** a reader looks up `belief_aware`, **Then** the default is stated as on in both modes, and `LCG_PASSAGE_BELIEF_AWARE` is documented.
2. **Given** the CHANGELOG, **When** a reader checks the Unreleased section, **Then** an entry flags the change as a behaviour change under which results are re-ordered by default, and says how to restore the previous ordering.
3. **Given** the PR description, **When** a reviewer reads it, **Then** every existing test whose expected default ordering changed is listed with a line on why the new order is correct.

---

### Edge Cases

- `belief_aware` is present but not a boolean: the existing validation error is unchanged.
- `belief_aware: null`: treated the same as omitted (consistent with the current handling of null).
- Explicit request value and environment switch disagree: the request wins.
- A corpus with no derived pages and no superseded episodes: belief-aware and raw rankings coincide, so results are unchanged by the default flip.
- `LCG_PASSAGE_SEARCH_MODE=hybrid` set in the workspace: the default mode is hybrid and belief-aware is on by default, as today.
- The embedder sidecar is unavailable: behaviour is unchanged; this change adds no new dependency.
- Case and spelling of the switch value (`OFF`, `0`, `false`): recognised values are enumerated in the docs, and anything else is treated as unrecognised.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: A `knowledge_search_passages` request that omits `belief_aware` (or sends null) MUST be ranked belief-aware in `vector` mode when no workspace switch is set.
- **FR-002**: A request that omits `belief_aware` MUST be ranked belief-aware in `hybrid` mode when no workspace switch is set.
- **FR-003**: A request with `belief_aware: false` MUST return results byte-for-byte identical to those the same request returned before this change, in both modes.
- **FR-004**: A request with `belief_aware: true` MUST behave as it does today, regardless of the workspace switch.
- **FR-005**: The service MUST support a workspace switch `LCG_PASSAGE_BELIEF_AWARE`; the value `off` makes requests that omit `belief_aware` rank without the belief-aware rules. In vector mode this MUST be byte-for-byte the pre-change default output.
- **FR-006**: An explicit `belief_aware` in the request MUST take precedence over `LCG_PASSAGE_BELIEF_AWARE`.
- **FR-007**: An unrecognised `LCG_PASSAGE_BELIEF_AWARE` value MUST be ignored (default behaviour applies) and reported to the operator, not cause a failure.
- **FR-008**: The default mode MUST remain `vector` (still overridable by `LCG_PASSAGE_SEARCH_MODE` or the request `mode`).
- **FR-009**: No read method other than `knowledge_search_passages` may change behaviour, request shape or response shape. The response shape of `knowledge_search_passages` is unchanged.
- **FR-010**: The MCP tool description for `knowledge_search_passages`, the IPC/MCP reference (and its generated `llms-full.txt` copy), and `docs/configuration.md` MUST state the new default, the per-request opt-out and the `LCG_PASSAGE_BELIEF_AWARE` switch.
- **FR-011**: The CHANGELOG Unreleased section MUST record this as a behaviour change (results re-ordered by default) with the two ways to restore the previous ordering.
- **FR-012**: The eval harness MUST keep `lcg-passages` meaning "the shipped default" (now belief-aware vector) and MUST add a `lcg-passages-raw` configuration that sends `belief_aware: false`; the other `lcg-passages-*` configurations keep their meaning.
- **FR-013**: Automated tests MUST cover: default-on in vector mode, default-on in hybrid mode, `belief_aware: false` restoring the previous ordering, `LCG_PASSAGE_BELIEF_AWARE=off` restoring the previous ordering, request-over-switch precedence, and the unrecognised-switch-value case.
- **FR-014**: Existing tests that asserted the old default order MUST be updated, and the PR description MUST list each one with a line on why the new order is right.

### Key Entities *(if applicable)*

- **Belief-aware ranking**: A re-ranking step over passage candidates that places a derived page below the primary episode it cites, and a superseded episode below its successor. Does not remove results.
- **`LCG_PASSAGE_BELIEF_AWARE`**: Workspace-level default for belief-aware ranking, applied only when the request does not say.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: On the v2 corpus (244 questions, k = 10), the `lcg-passages` configuration scores nDCG@10 of about 0.649 (the former `lcg-passages-belief` figure), versus 0.568 before, and temporal nDCG@10 of about 0.802; p50 latency stays within a few milliseconds of the previous 24 ms.
- **SC-002**: `lcg-passages-raw` reproduces the former `lcg-passages` figures (0.568 overall) on the same corpus.
- **SC-003**: With `belief_aware: false`, or with `LCG_PASSAGE_BELIEF_AWARE=off` in vector mode, the output on the test corpus is byte-for-byte the pre-change output.
- **SC-004**: A client sending no `belief_aware` and no `mode` gets results identical to an explicit `mode: "vector", belief_aware: true` request.
- **SC-005**: No other read method's output changes (existing parity and response-shape tests for them pass untouched).
- **SC-006**: Every existing test whose expected order changed is listed in the PR description with a justification.

## Assumptions

- #789 / PR #798 is merged, so `mode`, `belief_aware` and `LCG_PASSAGE_SEARCH_MODE` already exist on `main`.
- Because hybrid mode already defaults to belief-aware on, no hybrid-mode ordering changes by default; the user-visible default change is in vector mode. Hybrid is still covered by tests so the default cannot regress.
- `LCG_PASSAGE_BELIEF_AWARE=off` applies to both modes. In vector mode it restores the old default exactly. In hybrid mode it yields weighted-RRF without belief rules (what `belief_aware: false` gives today), which differs from the pre-change hybrid default (belief on). This is accepted because the issue frames the switch as "turn the default off".
- The switch is read the same way `LCG_PASSAGE_SEARCH_MODE` is read, including its handling of unrecognised values, and `off` is the documented value that disables the default; the full list of accepted spellings (e.g. `on`) is settled in planning.
- The maintainer re-runs the v1 and v2 evals on the PR head; workers do not need the embedder sidecar, so SC-001 and SC-002 are verified by the maintainer, and worker-side verification relies on deterministic tests.
- Client-side changes (Liminis Notes agent, Python-side protocol) are not needed: the request and response shapes do not change.

## Out of Scope *(optional)*

- Changing the default mode away from `vector` or making `hybrid` the default.
- Changing the belief-aware rules themselves, fusion weights, or the paraphrase weakness of hybrid.
- Any change to `knowledge_find_entities`, `knowledge_find_relationships` or other read methods.
- New MCP tools or changes to the MCP registry count.
- Re-running or re-baselining the eval corpus; that is the maintainer's task on the PR head.

## Source References *(optional)*

- #789 / PR #798 — hybrid passage search and belief-aware re-ranking (ADR-0789)
- v2 retrieval corpus: `meridian-notebook-2` / `meridian-questions-3` (#788)
