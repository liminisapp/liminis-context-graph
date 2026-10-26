# Feature Specification: LLM-verified extraction dedup via the configured extractor

**Feature Branch**: `fabrik/issue-652`
**Created**: 2026-10-01
**Status**: Specified
**Input**: User description: "Make LCG_DEDUP_LLM actually work: LLM-verified extraction dedup via the configured extractor (replace dead LCG_DEDUP_ADAPTER_URL protocol)"

## Background

During extraction, Phase B entity resolution (`crates/core/src/episode.rs`) resolves each extracted entity in two steps: an exact case-insensitive name match, and otherwise the best name-embedding candidate with cosine ≥ 0.85, confirmed through `state.dedup.is_duplicate`. Two confirmers exist today (`crates/core/src/dedup_adapter.rs`, selected in `app_state.rs`):

- **`PassthroughDedupAdapter`** (the default) always answers "duplicate", so every candidate above the threshold merges.
- **`LocalDedupAdapter`** (enabled by `LCG_DEDUP_LLM`) POSTs a bespoke `{candidate, incoming}` body to `LCG_DEDUP_ADAPTER_URL` (default `http://127.0.0.1:8767`) and reads `is_duplicate`. Nothing in lcg, its sidecars (`native/local-inference`) or its docs implements that endpoint, and the code is marked "deprecated: remove in Phase B (see #59)". Setting `LCG_DEDUP_LLM` without a hand-built server makes every embedding-path dedup check fail.

So no deployment can get an LLM-verified dedup today. An operator who wants one (Orac, which is hitting wrong merges such as `ADR 2018` = `ADR 2019`) has no working option. #650 adds a deterministic identifier veto that catches numeric and identifier differences, but semantic false merges, such as two different people with similar names, still need a real judgement.

This issue replaces the dead protocol with an LLM dedup check that goes through the extractor lcg already uses (the Anthropic API, or the OpenAI-compatible `--extractor-uds` / `--extractor-http` endpoint, including the CoreML sidecar's `/v1/chat/completions`). It needs no new server, protocol or deployment piece. A wrong merge cannot be undone without re-ingestion, while a missed merge is recoverable, so every failure mode of the check resolves to "not a duplicate".

**Relationship to other issues.** #650 owns the deterministic identifier veto and the merge-path counters, and it deliberately leaves `LocalDedupAdapter` and `LCG_DEDUP_ADAPTER_URL` untouched. This issue builds on #650: the LLM check sees only candidates the veto did not reject, and it extends #650's counters with LLM-confirmed and LLM-rejected paths. #651 (summary consolidation on merge) is independent. All three target milestone 0.16.3.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - LLM rejects look-alike candidates (Priority: P1)

An operator has an extractor configured and enables the LLM dedup check. When Phase B finds an embedding-similar candidate that the veto did not reject, the extractor is asked whether both entities are the same real-world thing. If it says no, the entity is inserted separately instead of merged.

**Why this priority**: This is the core value. It is the only way to prevent semantic false merges (for example, two different people with similar names) that no deterministic rule can catch.

**Independent Test**: Use a stubbed extractor that answers "not a duplicate". An above-threshold embedding candidate that passed the veto resolves to `Insert`.

**Acceptance Scenarios**:

1. **Given** an extractor is configured and `LCG_DEDUP_LLM` is enabled, **When** the stub answers "not a duplicate" for an above-threshold candidate, **Then** the decision is `Insert`.
2. **Given** the same setup, **When** the stub answers "duplicate", **Then** the decision is `Merge`.
3. **Given** the extractor is the Anthropic provider, **Then** the check works, tested with a stubbed Anthropic transport. **Given** it is the OpenAI-compatible provider (UDS or HTTP), **Then** the check works, tested with a stubbed OpenAI-compatible transport.

---

### User Story 2 - Failures never cause a wrong merge or abort extraction (Priority: P1)

If the extractor times out, errors, or returns an answer that is not a clear verdict, the candidate is treated as not a duplicate, a warning is logged, and extraction continues.

**Why this priority**: The check is advisory. A flaky extractor must not abort ingestion, and an unverifiable merge must not be performed.

**Independent Test**: Stub the extractor to return malformed text, an HTTP error, and a timeout. Each resolves to `Insert` with a logged warning.

**Acceptance Scenarios**:

1. **Given** the check is enabled, **When** the extractor response is malformed or ambiguous, **Then** the decision is `Insert` and a warning is logged.
2. **Given** the check is enabled, **When** the extractor call fails or times out, **Then** the decision is `Insert`, a warning is logged, and the chunk's extraction still completes.
3. **Given** the operation is cancelled while a check is in flight, **Then** cancellation propagates as `Error::Cancelled` as for any other Phase B call and is not swallowed into `Insert`.

---

### User Story 3 - Cheap pairs never reach the LLM (Priority: P1)

Only pairs that genuinely need a judgement cost an LLM call. Exact-name matches, pairs the veto rejected, and pairs with identical normalized names make no call.

**Why this priority**: Controls cost and latency, and keeps the deterministic layers authoritative.

**Independent Test**: With a counting stub extractor, resolve an exact-name match, a veto-rejected pair (`ADR 2018` / `ADR 2019`), and an identical-normalized-name pair. The stub is never called. An ambiguous above-threshold pair calls it once.

**Acceptance Scenarios**:

1. **Given** an exact case-insensitive name match, **Then** no LLM call is made.
2. **Given** a candidate the #650 veto rejects, **Then** no LLM call is made and the decision is `Insert`.
3. **Given** a candidate whose normalized name equals the incoming name, **Then** no LLM call is made.
4. **Given** several candidates in one chunk, **Then** they are judged in as few calls as practical (batched), and the verdict for each candidate is attributed correctly.

---

### User Story 4 - Operators see which dedup mode is active (Priority: P2)

At startup the log states whether dedup is passthrough, veto-only, or LLM-verified, and `knowledge_status` reports the same mode. The merge-path counters introduced by #650 gain LLM-confirmed and LLM-rejected entries.

**Why this priority**: The old silent always-merge default went unnoticed. An operator must be able to tell what protects their graph.

**Independent Test**: Start the service in each configuration and check the log line and the `knowledge_status` field. Ingest a mix of resolutions and check the counters.

**Acceptance Scenarios**:

1. **Given** no extractor is configured, **Then** startup logs the veto-only (or passthrough) mode and `knowledge_status` reports it.
2. **Given** an extractor is configured and the LLM check is active, **Then** startup logs LLM-verified mode and `knowledge_status` reports it.
3. **Given** `LCG_DEDUP_LLM` is enabled but no extractor is available, **Then** the service starts (it does not fail), logs a warning that the check cannot run, and reports the mode it actually uses.
4. **Given** a run with LLM-confirmed and LLM-rejected resolutions, **Then** the counters reflect them.

---

### User Story 5 - The dead adapter protocol is retired (Priority: P2)

`LCG_DEDUP_ADAPTER_URL` and the bespoke `{candidate, incoming}` protocol no longer drive any behaviour. An operator who still has the variable set is told so.

**Why this priority**: The old path is non-functional and misleading, and the "Phase B (#59)" deprecation note is stale.

**Independent Test**: Start the service with `LCG_DEDUP_ADAPTER_URL` set and check that a deprecation warning is logged and no HTTP request is made to that URL.

**Acceptance Scenarios**:

1. **Given** `LCG_DEDUP_ADAPTER_URL` is set, **Then** startup logs a warning that it is deprecated and ignored.
2. **Given** `LCG_DEDUP_LLM` is enabled, **Then** the check uses the configured extractor and never contacts `LCG_DEDUP_ADAPTER_URL`.
3. **Given** the change ships, **Then** the CHANGELOG records the removal or deprecation, and `docs/configuration.md` no longer describes the old protocol.

---

### User Story 6 - WAL replay is unaffected (Priority: P1)

Existing WALs replay to identical graphs. Replay re-applies recorded merge or insert mutations and never calls the LLM.

**Why this priority**: Replay determinism is a hard invariant of the system.

**Independent Test**: Replay existing WAL fixtures with the check enabled and a stub extractor that fails on any call. Graphs are identical to before and the stub is never called.

**Acceptance Scenarios**:

1. **Given** a WAL containing recorded merge and insert mutations, **When** it is replayed with the LLM check enabled, **Then** the resulting graph is identical and no LLM call is made.

---

### Edge Cases

- The extractor answers with extra prose around the verdict, a markdown-fenced answer, or an empty body: only a clear verdict counts; anything else is "not a duplicate".
- A batched call returns fewer verdicts than candidates, duplicates, or out-of-order identifiers: unattributable candidates default to `Insert`.
- Entities with empty or missing summaries or types: the check still runs on names, using whatever fields exist.
- The same candidate pair appears repeatedly (within one chunk or across chunks): it is not judged twice when a verdict is already known for the process lifetime (optional cache).
- Replay mode (`LCG_REPLAY_LLM` cassette): no live network call may happen. The check must not break cassette-replay runs (see Assumptions).
- Extractor is slow: the check has a bounded timeout and does not hold the Phase C write lock.
- Very large chunks with many candidates: batching must stay within the extractor's request limits.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: When the LLM dedup check is active, every embedding-path candidate that survives the #650 veto MUST be judged by the configured extractor. A "duplicate" verdict yields `Merge`; a "not a duplicate" verdict yields `Insert`.
- **FR-002**: The check MUST reuse the extraction backend already configured for the service: the Anthropic API or the OpenAI-compatible UDS/HTTP endpoint. It MUST NOT require a new server, protocol or deployment component.
- **FR-003**: The judgement MUST be a constrained question containing both entities' names, types/labels and summaries, requiring a strict structured answer (strict JSON or yes/no).
- **FR-004**: A malformed, ambiguous, empty or unattributable answer, an extractor error, and a timeout MUST all be treated as "not a duplicate" (`Insert`), MUST log a warning, and MUST NOT abort extraction. Cancellation MUST still propagate as `Error::Cancelled`.
- **FR-005**: Exact-name matches, candidates rejected by the #650 veto, and pairs with identical normalized names MUST NOT trigger an LLM call.
- **FR-006**: Candidates within a chunk SHOULD be batched into a single extractor call where practical. Verdicts MUST be attributed to the correct candidate. Verdicts MAY be cached by name-pair hash for the process lifetime.
- **FR-007**: `LCG_DEDUP_LLM` MUST be the switch for this extractor-backed check. Its default (on whenever an extractor is configured, or opt-in) MUST be chosen during research, documented, and logged at startup.
- **FR-008**: Startup MUST log the active dedup mode (passthrough, veto-only, or LLM-verified), and `knowledge_status` MUST report it. If the check is enabled but no extractor is available, the service MUST start, warn, and report the mode actually in effect.
- **FR-009**: #650's merge-path counters MUST be extended with LLM-confirmed and LLM-rejected paths, and they MUST be consistent with observed decisions.
- **FR-010**: `LCG_DEDUP_ADAPTER_URL` and the bespoke `{candidate, incoming}` protocol MUST NOT drive any behaviour. The variable MUST either be removed with a CHANGELOG note, or be deprecated with a warning logged when it is set. The "deprecated: remove in Phase B (see #59)" note MUST be resolved.
- **FR-011**: The LLM call MUST run in the lock-free Phase B decision loop and MUST NOT run under the Phase C write lock.
- **FR-012**: WAL replay MUST NOT call the LLM. Recorded merge and insert mutations replay as written, and existing WALs MUST replay to identical graphs.
- **FR-013**: There MUST be no schema change.
- **FR-014**: If a new `knowledge_*` method or response field is added, the MCP tool registry (`crates/service/src/mcp/tools.rs`) and its count assertions, and the IPC parity tests, MUST be updated. New or changed configuration MUST be documented in `docs/configuration.md` (and `docs/llms-full.txt`), with an ADR numbered `docs/adr/0652-<slug>.md`.

### Key Entities

- **Dedup mode**: One of passthrough, veto-only, or LLM-verified. It is logged at startup and reported by `knowledge_status`.
- **Dedup verdict**: The result of judging a candidate pair: duplicate, not duplicate, or unknown (failure). Unknown is treated as not duplicate.
- **Merge-path counters**: #650's per-path tallies of Phase B outcomes, extended with LLM-confirmed and LLM-rejected.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: With a stubbed extractor, an above-threshold candidate that passes the veto becomes an `Insert` when the stub rejects it and a `Merge` when the stub confirms it, on both the Anthropic and OpenAI-compatible paths.
- **SC-002**: Malformed, failed and timed-out LLM responses each produce an `Insert` plus a logged warning in 100% of tested cases, and extraction never aborts because of them.
- **SC-003**: Zero LLM calls are made for exact-name matches, veto-rejected pairs and identical-normalized-name pairs, verified with a counting stub.
- **SC-004**: Startup logs the active dedup mode in each configuration and `knowledge_status` reports the same mode.
- **SC-005**: Setting `LCG_DEDUP_ADAPTER_URL` produces a deprecation warning (or the variable is removed with a CHANGELOG entry), and no request is ever sent to that URL.
- **SC-006**: Existing WAL fixtures replay to identical graphs with zero LLM calls, and existing dedup tests pass unchanged.

## Assumptions

- #650 lands first, or is rebased under this work. This issue depends on its veto and counter structure and does not reimplement them.
- Research decides the default (lean: on when an extractor is configured, opt-out otherwise) by weighing the per-chunk cost of the extra calls. The chosen default is logged at startup.
- A missed merge is acceptable and recoverable. A wrong merge is not, so all failures resolve to `Insert`.
- Replay-mode runs (`LCG_REPLAY_LLM`) make no live dedup calls. Research decides whether dedup verdicts are recorded to and served from the cassette, or the check is skipped in that mode, and the choice is documented.
- Research confirms that WAL replay never re-runs Phase B.
- Patch-sized change, milestone 0.16.3.

## Out of Scope

- The deterministic identifier veto and the base merge-path counters (#650).
- Summary consolidation on merge and summary re-embedding (#651).
- Repairing or splitting existing wrong merges in already-ingested graphs. Recovery is re-ingestion.
- Changing `DEDUP_THRESHOLD`, the embedding model, the exact-name path, or `knowledge_merge_entities`.
- Improving alias recall (`Kubernetes` / `K8s` stay unmerged as today).
- A separate dedicated dedup model or endpoint distinct from the extractor.

## Source References

- `crates/core/src/episode.rs` (Phase B, ~159–195 and `state.dedup.is_duplicate` call site)
- `crates/core/src/dedup_adapter.rs`, `crates/core/src/app_state.rs` (~149–200)
- `crates/core/src/extractor.rs` (`Extractor` trait)
- `docs/configuration.md` (`LCG_DEDUP_LLM`, `LCG_DEDUP_ADAPTER_URL`)
- Issues #59 (Phase B note), #650 (veto and counters), #651 (summary consolidation)
