# Feature Specification: Cross-group pointers stay honest and self-repair regardless of group replay order

**Feature Branch**: `fabrik/issue-741`
**Created**: 2026-11-13
**Status**: Specified
**Input**: User description: "Implements community report #739. That report has the full reproduction; it's confirmed on v0.16.4 and v0.18.0.

Problem — Suppose a group holding a cross-group edge (`knowledge_add_cross_group_edge`) is replayed from WAL **before** the group its pointer targets:

- **The edge never appears.** Its hop writes (`MATCH (…{uuid:$dst}) … MERGE`) find no endpoint and do nothing.
- **The pointer still looks healthy.** It keeps the `binding_state: "bound"` and `resolved_uuid` cached in its WAL attributes, so `knowledge_status.cross_group_pointers` reports it as `bound`. Reads don't see the edge.
- **Nothing repairs it.** Two code paths keep it broken, both verified against `main`:
  1. `knowledge_rebuild_from_wal` runs the post-replay `cross_group::rebind_pointers` only when a WAL reset was detected (`if reset_detected`, `crates/core/src/handlers.rs` ~L3606, and the same in the background-job path ~L4084). An ordinary rebuild of the target group returns `cross_group_rebind: null`.
  2. `rebind_pointers` skips a pointer whose stored state is `Bound` with `bound_at_seq >= applied_seq` (`crates/core/src/cross_group.rs` ~L422), without checking that the hop edge exists. Once the target group has replayed, that's always true here, so `knowledge_rebind_pointers` reports `staleness_skipped: 1` and fixes nothing.

So correctness depends on the order in which a consumer rebuilds groups, and nothing reports a problem. Consumers that subscribe to independently published layer groups in arbitrary combinations can't know a safe order. This is the sibling of #392, which handled pointers already recorded as `unbound`.

Proposed fix — Do both of these:

1. **Report it honestly.** When replaying a cross-group edge whose hop write found no endpoint, record the pointer as `unbound` (clear the cached `resolved_uuid` / `bound_at_seq`) rather than keeping the cached `bound`. `knowledge_status` then reports it, and the existing #392 path re-resolves it.
2. **Repair after replay.** After a successful `knowledge_rebuild_from_wal` (full or incremental, any trigger, not only `reset_detected`), run the staleness-gated `rebind_pointers` for pointers whose `source_group_id` is the replayed group, and report its counts in `cross_group_rebind`. Flush any rebind mutations to their owning groups' writers, as the existing reset path does (#383 / #385, ADR-0385).

Research should confirm that (1) is where replay decides the pointer's state. If (2) alone makes the result order-independent and (1) turns out to be invasive, record the trade-off in the PR. The behaviour below is what matters.

Acceptance criteria:
1. Wrong order now works. With #739's reproduction, rebuild `layer` then `up`. After the `up` rebuild: `knowledge_get_entity_neighbors {entity_uuid: <note>, group_ids: [\"layer\"]}` returns the `ADR-1` node and the `ANNOTATES` edge; `cross_group_rebind` in the `up` rebuild result is non-null and counts the repair.
2. No false health in between. After the `layer` rebuild and before the `up` rebuild, `knowledge_status.cross_group_pointers` reports the pointer as `unbound`, not `bound`.
3. The right order is unchanged. Rebuilding `up` then `layer` gives the same final graph as today.
4. Tests. The reproduction becomes an integration test covering both orders, in the style of the existing cross-group and rebind tests. The #392 regression tests still pass.
5. Docs. `docs/ipc-mcp-reference.md` says rebuild order no longer matters, and that `cross_group_rebind` is reported on every rebuild.

Out of scope: changing the staleness gate's semantics for pointers that are genuinely bound and current; WAL format changes; `knowledge_add_cross_group_edge`'s live-write behaviour, where the target already exists."

## Background

A cross-group edge (created with `knowledge_add_cross_group_edge`) lives in one group's WAL (the *source* group) but points at an entity in another group (the *target* group). When the source group is replayed from WAL before the target group has been populated, the edge's endpoint lookup finds nothing and the edge is silently not created. The pointer record, however, keeps the `bound` state and resolved target identifier that were cached when the edge was originally written. The result is a graph that is missing an edge while the status report says everything is bound.

Today nothing recovers from this:

- An ordinary rebuild of the target group does not re-evaluate pointers, because post-replay rebinding runs only when a WAL reset was detected; the rebuild result reports no rebind information.
- The explicit rebind operation trusts the cached `bound` state and skips the pointer as current, so it reports "skipped" and fixes nothing.

This makes correctness depend on the order in which a consumer rebuilds groups, with no signal when the order was wrong. Consumers that subscribe to independently published layer groups in arbitrary combinations cannot know a safe order. This is the sibling of #392, which handled pointers already recorded as `unbound`; this issue closes the case where a pointer is recorded as `bound` but its edge was never materialized. The original reproduction and confirmation on v0.16.4 and v0.18.0 are in #739.

The issue's proposed approach has two parts: (1) record the pointer honestly as unbound when replay could not create its edge, and (2) after every successful rebuild, run the staleness-gated rebind for pointers whose source group was just replayed. The observable behaviour below is the requirement; the approach is a starting point for Research, which should confirm where replay decides pointer state and may adjust the approach if part (1) proves invasive (see Assumptions).

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Rebuilding groups in the "wrong" order still yields the full graph (Priority: P1)

A consumer holds a source group (`layer`) containing a cross-group edge to an entity in a target group (`up`), per #739's reproduction. They rebuild `layer` from WAL first, then `up`. After the second rebuild, the cross-group edge exists and is readable, without the consumer running any extra repair step.

**Why this priority**: This is the data-correctness bug. Consumers cannot control or know a safe rebuild order, and today the edge is permanently lost with no repair path.

**Independent Test**: Using #739's reproduction, rebuild `layer` then `up`; query the neighbors of the note entity in group `layer` and inspect the `up` rebuild result.

**Acceptance Scenarios**:

1. **Given** a source group with a cross-group edge and a target group, both with their WAL intact, **When** the source group is rebuilt and then the target group is rebuilt, **Then** `knowledge_get_entity_neighbors {entity_uuid: <note>, group_ids: ["layer"]}` returns the `ADR-1` node and the `ANNOTATES` edge.
2. **Given** the same setup, **When** the target group's rebuild completes, **Then** its result carries a non-null `cross_group_rebind` whose counts reflect that the pointer was repaired.
3. **Given** the same setup rebuilt through the background-job rebuild path rather than the foreground one, **When** the rebuild completes, **Then** the same repair occurs and the same counts are reported.

---

### User Story 2 - Status never reports a pointer as healthy when its edge is missing (Priority: P1)

Between the two rebuilds in Story 1, an operator inspects `knowledge_status`. The pointer whose edge could not be created must appear as unbound, so the gap is visible and the existing unbound-pointer resolution path (#392) can act on it.

**Why this priority**: Without honest reporting, the failure is silent. This also makes the operator-driven rebind path work even if the automatic repair has not yet run.

**Independent Test**: Rebuild only the source group, then read `knowledge_status.cross_group_pointers`.

**Acceptance Scenarios**:

1. **Given** a source group whose cross-group edge target is not yet present, **When** the source group is rebuilt from WAL, **Then** `knowledge_status.cross_group_pointers` reports that pointer as `unbound`, with no stale resolved identifier presented as current.
2. **Given** a pointer reported as `unbound` by Scenario 1, **When** `knowledge_rebind_pointers` is run after the target group is present, **Then** the pointer is re-resolved and the edge becomes readable (it is not skipped as "stale-gated").

---

### User Story 3 - Rebuilding in the "right" order behaves as before, and rebuild reports rebind on every rebuild (Priority: P2)

A consumer who already rebuilds the target group before the source group sees no change in the resulting graph. Every successful rebuild reports rebind counts in `cross_group_rebind`, so a consumer can tell whether any pointer work happened.

**Why this priority**: Guards against regression and makes the repair observable; lower than the bug fix itself.

**Independent Test**: Rebuild `up` then `layer`, compare the final graph to the pre-change behaviour; inspect `cross_group_rebind` on both rebuild results.

**Acceptance Scenarios**:

1. **Given** the reproduction setup, **When** the target group is rebuilt first and the source group second, **Then** the final graph (nodes, edges, pointer states) is identical to the result produced before this change.
2. **Given** any successful rebuild (full or incremental, triggered by a WAL reset or not), **When** the rebuild result is returned, **Then** `cross_group_rebind` is non-null and reports its counts (zeros when no pointers needed attention).
3. **Given** a pointer that is genuinely bound and current (its edge exists), **When** a rebuild runs, **Then** the pointer is left untouched and counted as skipped by the staleness gate.

---

### User Story 4 - Documentation and regression coverage (Priority: P2)

Integrators reading the IPC/MCP reference learn that rebuild order no longer matters and that `cross_group_rebind` appears on every rebuild. The reproduction is permanently covered by an integration test in both orders.

**Why this priority**: Prevents regression and tells consumers they can drop any ordering workaround.

**Independent Test**: Review the docs diff; run the new integration test and the #392 regression tests.

**Acceptance Scenarios**:

1. **Given** the updated documentation, **When** a reader looks up `knowledge_rebuild_from_wal`, **Then** it states rebuild order across groups does not affect the final graph and that `cross_group_rebind` is reported on every rebuild.
2. **Given** the test suite, **When** it runs, **Then** an integration test exercises both rebuild orders from #739's reproduction and the existing #392 regression tests still pass.

---

### Edge Cases

- **Target group never rebuilt/never exists**: the pointer remains `unbound`, the edge stays absent, and status reports it honestly; no error is raised for the source-group rebuild.
- **Several pointers in one source group**: only pointers whose edge could not be created are demoted; pointers whose targets were present remain bound.
- **Incremental rebuild of the target group**: the repair runs for incremental as well as full rebuilds.
- **Rebuild with a detected WAL reset**: the pre-existing reset-driven rebind still happens, and the result does not double-count or run the repair inconsistently.
- **Repaired edge's owning group differs from the replayed group**: rebind mutations are flushed to the correct owning group's writer, as in the existing reset path (#383 / #385, ADR-0385).
- **Rebuild that fails or is cancelled**: no rebind is run and no pointer state is changed by the repair step.
- **Pointer where the target entity exists but was deleted/replaced**: handled by existing #392 semantics and not made worse.
- **Foreground and background-job rebuild paths** must behave identically.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: When WAL replay of a cross-group edge cannot create the edge because an endpoint is absent, the corresponding pointer MUST end replay in the `unbound` state, with any previously cached resolved identifier and binding sequence cleared, rather than retaining its cached `bound` state.
- **FR-002**: `knowledge_status.cross_group_pointers` MUST report such a pointer as `unbound` after the source group's rebuild and before the target group is available.
- **FR-003**: After every successful `knowledge_rebuild_from_wal` — full or incremental, regardless of what triggered it, and including the background-job path — the system MUST run the staleness-gated pointer rebind for pointers whose source group is the replayed group.
- **FR-004**: The rebuild result's `cross_group_rebind` field MUST be non-null on every successful rebuild and MUST report the rebind counts (including the number of pointers repaired).
- **FR-005**: Any graph mutations produced by the post-rebuild rebind MUST be flushed to the writers of the groups that own them, consistent with the existing reset-driven rebind behaviour.
- **FR-006**: Following the target group's rebuild in the "wrong" order, the cross-group edge MUST be present and readable (neighbors query returns the target node and the edge) without any manual follow-up operation.
- **FR-007**: Rebuilding in the "right" order (target group, then source group) MUST produce the same final graph as before this change.
- **FR-008**: Pointers that are genuinely bound and current MUST continue to be skipped by the staleness gate; the gate's semantics for such pointers MUST NOT change.
- **FR-009**: `knowledge_rebind_pointers`, run on a pointer left `unbound` by FR-001, MUST re-resolve it via the existing #392 path rather than skip it.
- **FR-010**: The reproduction from #739 MUST be added as an integration test covering both rebuild orders, in the style of the existing cross-group and rebind tests; existing #392 regression tests MUST continue to pass.
- **FR-011**: `docs/ipc-mcp-reference.md` MUST state that rebuild order across groups no longer matters and that `cross_group_rebind` is reported on every rebuild.

### Key Entities *(if applicable)*

- **Cross-group pointer**: A record of a cross-group edge, holding a source group, a target group/entity reference, a binding state (`bound`/`unbound`), a cached resolved identifier, and the sequence at which it was bound.
- **Cross-group edge (hop)**: The actual graph edge materialized in the source group's graph, connecting entities across groups; absent when its endpoint was missing at replay time.
- **`cross_group_rebind` result**: The rebuild-result field carrying rebind counts (e.g. rebound, skipped by staleness gate).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: In #739's reproduction, after rebuilding the source group then the target group, the neighbors query returns the expected node and edge in 100% of runs, with no manual rebind call.
- **SC-002**: In the same reproduction, between the two rebuilds, `knowledge_status.cross_group_pointers` reports zero `bound` pointers whose edge is missing.
- **SC-003**: The final graph is identical for both rebuild orders.
- **SC-004**: 100% of successful rebuild results (full, incremental, reset or not, foreground or background) carry a non-null `cross_group_rebind`.
- **SC-005**: The new two-order integration test and all existing #392 regression tests pass in CI.
- **SC-006**: The IPC/MCP reference documents the order-independence guarantee and the always-present `cross_group_rebind`.

## Assumptions

- The behaviour in the acceptance criteria is authoritative; the two-part approach in the issue is a proposal. If demoting the pointer during replay (FR-001) proves invasive, Research/Plan may achieve order-independence through the post-rebuild repair alone, provided FR-002 (honest status between rebuilds) is still satisfied or the trade-off is explicitly recorded in the PR.
- The post-rebuild rebind is scoped to pointers whose source group is the replayed group, which is sufficient because the edge lives in the source group.
- `cross_group_rebind` changing from nullable to always-present on successful rebuilds is an additive, backward-compatible change to the IPC response; consumers already tolerate it being populated after a reset.
- No WAL format change is needed: the unbound state is derived at replay/status time, and the existing #392 path handles re-resolution.
- The reproduction from #739 is available and reducible to a self-contained integration test.

## Out of Scope *(optional)*

- Changing the staleness gate's semantics for pointers that are genuinely bound and current.
- WAL format changes.
- `knowledge_add_cross_group_edge`'s live-write behaviour when the target already exists.

## Source References *(optional)*

- #739 — community report with full reproduction (confirmed on v0.16.4 and v0.18.0)
- #392 — sibling fix for pointers already recorded as `unbound`
- #383 / #385, ADR-0385 — flushing rebind mutations to owning groups' writers
- `docs/ipc-mcp-reference.md` — documentation to update
