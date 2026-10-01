# Feature Specification: Identifier veto and real LLM check for extraction-time entity dedup

**Feature Branch**: `fabrik/issue-650`
**Created**: 2026-10-01
**Status**: Specified
**Input**: User description: "Extraction dedup merges distinct entities differing only by number/identifier (ADR 2018 = ADR 2019): add identifier veto + real LLM dedup check"

## Background

During extraction, Phase B entity resolution (`resolve_phase_b`, `crates/core/src/episode.rs`) resolves each extracted entity in two steps:

1. an exact, case-insensitive name match scoped to group and kind (correct, and unaffected by this issue);
2. otherwise, the best name-embedding candidate with cosine ≥ `DEDUP_THRESHOLD` (0.85), confirmed by `state.dedup.is_duplicate`.

The default confirmer is `PassthroughDedupAdapter`, which always answers "duplicate". The only alternative, `LocalDedupAdapter` (enabled by `LCG_DEDUP_LLM`), posts a bespoke `{candidate, incoming}` body to `LCG_DEDUP_ADAPTER_URL` (default `127.0.0.1:8767`). Nothing in lcg or its sidecars implements that endpoint, and the code marks it "deprecated: remove in Phase B (#59)". In practice, every deployment merges on cosine alone.

bge-base name embeddings treat numbers and identifiers as near-noise. Measured with the real sidecar on 2026-10-01:

| pair | cosine | merges today | correct |
|---|---|---|---|
| `ADR 2018` / `ADR 2019` | 0.946 | yes | distinct |
| `lcg 0.15.0` / `lcg 0.16.2` | 0.971 | yes | distinct |
| `RFC 9110` / `RFC 9111` | 0.935 | yes | distinct |
| `Q3 2025 roadmap` / `Q4 2025 roadmap` | 0.948 | yes | distinct |
| `issue #611` / `issue #612` | 0.866 | yes | distinct |
| `Project Aurora` / `Project Aurora v2` | 0.864 | yes | probably distinct |
| `PostgreSQL` / `Postgres` | 0.934 | yes | same |
| `New York` / `New York City` | 0.948 | yes | same |
| `Alice` / `Alice Smith` | 0.754 | no | arguably same |
| `Kubernetes` / `K8s` | 0.669 | no | same |

Distinct, number-differentiated names score **higher** than genuine aliases, so no threshold can separate them. A production corpus merged `ADR 2018` and `ADR 2019` into one entity, combining their summaries (#647) and edges. Existing bad merges can only be recovered by re-ingestion, so this must be fixed before the reporter re-ingests.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Number- or identifier-differentiated names stay distinct (Priority: P1)

A user ingests documents that mention `ADR 2018` and `ADR 2019` (or versions, RFCs, quarters, issue numbers). Each becomes its own entity with its own summary and edges, regardless of how close their name embeddings are.

**Why this priority**: A wrong merge is unrecoverable without re-ingestion and corrupts summaries and edges. This is the reported defect and works with no LLM configured.

**Independent Test**: With fixed embeddings forcing cosine ≥ 0.85, extract each pair in the table marked "distinct". Each pair yields two entities, via the `Insert` path.

**Acceptance Scenarios**:

1. **Given** an existing entity `ADR 2018` and an extracted `ADR 2019` with name-embedding cosine ≥ 0.85, **When** Phase B resolves it, **Then** it takes the `Insert` path and no merge occurs.
2. **Given** the pairs `lcg 0.15.0`/`lcg 0.16.2`, `RFC 9110`/`RFC 9111`, `Q3 2025 roadmap`/`Q4 2025 roadmap`, `issue #611`/`issue #612`, **When** each is extracted with cosine ≥ 0.85, **Then** none are merged.
3. **Given** `Project Aurora` and an extracted `Project Aurora v2` (one side has a distinguishing token, the other none), **When** resolved with cosine ≥ 0.85, **Then** they are not merged.
4. **Given** `Phase A` and an extracted `Phase B` (standalone single-letter designators), **When** resolved, **Then** they are not merged.
5. **Given** the same pairs on both the hybrid and the brute-force dedup paths, **Then** the outcome is identical.

---

### User Story 2 - Genuine aliases still merge without an LLM (Priority: P1)

A user with no LLM extractor available still gets alias merging for names without identifier differences.

**Why this priority**: The veto must not regress existing behaviour for ordinary aliases.

**Independent Test**: With no LLM dedup check configured, extract `PostgreSQL` against existing `Postgres`, and `New York City` against existing `New York`, with cosine ≥ 0.85.

**Acceptance Scenarios**:

1. **Given** existing `PostgreSQL` and extracted `Postgres` (cosine ≥ 0.85, no distinguishing tokens on either side), **When** no LLM dedup check is configured, **Then** they merge on the embedding path.
2. **Given** existing `New York` and extracted `New York City`, **When** no LLM dedup check is configured, **Then** they merge.

---

### User Story 3 - LLM confirms ambiguous embedding candidates (Priority: P2)

An operator who has configured an extractor (Anthropic API, or the OpenAI-compatible `--extractor-uds` / `--extractor-http` endpoint) gets a constrained yes/no judgement on each embedding candidate that survives the veto, so aliases merge and look-alikes do not.

**Why this priority**: The veto catches only identifier differences. The LLM check covers the rest, and replaces the dead `LocalDedupAdapter` protocol.

**Independent Test**: Use a stubbed extractor that answers "not a duplicate". An embedding candidate above threshold that survives the veto becomes an `Insert`. With a stub that answers "duplicate", it merges.

**Acceptance Scenarios**:

1. **Given** the LLM dedup check is enabled and the stub rejects the candidate, **When** Phase B resolves an above-threshold candidate that passed the veto, **Then** the decision is `Insert`.
2. **Given** the stub confirms, **Then** the decision is `Merge`.
3. **Given** the check is enabled but the extractor call fails or returns an unparseable answer, **Then** the behaviour is deterministic and documented (see FR-012) and extraction does not abort.

---

### User Story 4 - Operators can see how dedup is behaving (Priority: P2)

At startup the log states which dedup mode is active. Extraction results and/or `knowledge_status` report merge counts by path, so a bad-merge rate is visible.

**Why this priority**: Without observability, a silent always-merge default went unnoticed.

**Independent Test**: Start the service in each mode and check the log line. Ingest a mix of exact, vetoed, LLM-confirmed and LLM-rejected resolutions and check the counters.

**Acceptance Scenarios**:

1. **Given** the service starts with no extractor, **Then** the log names the passthrough/no-LLM mode explicitly.
2. **Given** the service starts with an extractor and the LLM check enabled, **Then** the log names the LLM mode.
3. **Given** a run with resolutions through each path, **Then** counters for exact-name, embedding-merge (veto passed, no LLM), LLM-confirmed, LLM-rejected and vetoed are reported and match the observed decisions.

---

### Edge Cases

- One name has a distinguishing token and the other has none (`Project Aurora` / `Project Aurora v2`): vetoed.
- Token order or repetition differs but the sets are equal (`2019 ADR` / `ADR 2019`): sets match, so the veto does not fire.
- Tokens are compared after the same normalization the name index uses (case, punctuation, `#611` vs `611`). Research confirms how `#`-prefixed and hyphenated forms tokenize.
- Mixed alphanumerics (`Q3`, `v2`, `k8s`, `H2O`, `COVID-19`) count as distinguishing tokens because they contain a digit. Research assesses false-negative cost on real corpora (e.g. `K8s` aliases, `Python 3` / `Python`). The lean is to stay conservative, since a missed merge leaves two entities but a wrong merge is unrecoverable.
- Standalone single-letter designators (`Plan B`, `Phase A`) count as distinguishing tokens. Possessives and initials (`Alice B. Smith`, `Brett A.`) must not cause regressions that Research judges unacceptable. This needs explicit treatment.
- When the best embedding candidate is vetoed, whether a lower-ranked candidate may still be considered is a Research/Plan decision. The outcome must be deterministic.
- The edge-endpoint salvage step also reuses `DEDUP_THRESHOLD` against name embeddings. Whether the veto applies there too is for Research to decide.
- Existing exact-name matches and explicit `knowledge_merge_entities` calls are never subject to the veto or the LLM check.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: Before any embedding-based merge in extraction Phase B, the system MUST apply a deterministic identifier-mismatch veto, on both the hybrid and brute-force dedup paths.
- **FR-002**: Names MUST be tokenized the same way the name index normalizes them.
- **FR-003**: The distinguishing tokens of a name are every token containing a digit, plus every standalone single-letter designator.
- **FR-004**: If the two names' distinguishing-token sets differ, including when only one side has any, the candidate MUST NOT be a duplicate and resolution MUST take the `Insert` path.
- **FR-005**: The veto MUST NOT apply to the exact case-insensitive name-match path, to `knowledge_merge_entities`, or to any non-extraction code path.
- **FR-006**: When the veto does not fire and no LLM dedup check is active, an above-threshold candidate MUST merge as today (`PostgreSQL`/`Postgres`, `New York`/`New York City`).
- **FR-007**: The dead `LocalDedupAdapter` HTTP protocol (`LCG_DEDUP_ADAPTER_URL`, `{candidate, incoming}` → `{is_duplicate}`) MUST be replaced by an LLM dedup check that uses the already-configured extractor (Anthropic API or OpenAI-compatible UDS/HTTP endpoint). `LCG_DEDUP_ADAPTER_URL` MUST either be removed or kept as a documented deprecated alias. The "Phase B (#59)" deprecation note MUST be resolved.
- **FR-008**: The LLM check MUST ask a constrained yes/no question containing both entities' names, types and summaries, and MUST be invoked only for embedding candidates that survive the veto.
- **FR-009**: The LLM dedup check MUST be controllable by configuration. Its default (opt-in or opt-out) MUST be chosen and documented, with cost weighed (at most one small call per ambiguous candidate, batched per chunk where feasible).
- **FR-010**: The passthrough adapter MUST remain only as the explicit "no LLM available / disabled" fallback.
- **FR-011**: Startup MUST log which dedup mode is active.
- **FR-012**: If the LLM check errors or returns an unparseable answer, the system MUST apply a documented, deterministic fallback (the conservative choice is `Insert`) and MUST NOT fail the extraction. Cancellation MUST continue to propagate as `Error::Cancelled`.
- **FR-013**: Merge counts by path MUST be exposed in the extraction result and/or `knowledge_status`. The paths are exact-name, embedding merge with veto passed, LLM-confirmed, LLM-rejected, and vetoed.
- **FR-014**: WAL replay MUST be unaffected. Recorded merge mutations replay as written, so existing WALs produce identical graphs. Only new extractions change.
- **FR-015**: If a new `knowledge_*` method or field is added, the MCP tool registry (`crates/service/src/mcp/tools.rs`) and its count assertions, and the IPC parity tests, MUST be updated. Any new configuration is documented in `docs/configuration.md` (and `docs/llms-full.txt`).

### Key Entities

- **Distinguishing-token set**: The set of digit-bearing tokens and standalone single-letter tokens in a normalized entity name.
- **Dedup mode**: One of passthrough (no LLM) or LLM-checked, reported at startup.
- **Merge-path counters**: Per-path tallies of Phase B resolution outcomes.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: All six "distinct" pairs from the table (`ADR 2018`/`2019`, `lcg 0.15.0`/`0.16.2`, `RFC 9110`/`9111`, `Q3`/`Q4 2025 roadmap`, `issue #611`/`#612`, `Project Aurora`/`v2`) are never merged by extraction, tested with fixed embeddings forcing cosine ≥ 0.85, on both dedup paths.
- **SC-002**: `PostgreSQL`/`Postgres` and `New York`/`New York City` still merge on the embedding path when no LLM check is configured.
- **SC-003**: With a stubbed extractor and the LLM check enabled, a rejected embedding candidate becomes an `Insert`, and a confirmed one becomes a `Merge`.
- **SC-004**: Startup logs the active dedup mode, and per-path merge counts are reported and consistent with observed decisions.
- **SC-005**: Existing WALs replay to identical graphs, and the existing exact-name and `knowledge_merge_entities` tests pass unchanged.

## Assumptions

- Extraction-time only. `knowledge_merge_entities` and the exact-name path are unchanged.
- Being conservative on false negatives (missed merges) is acceptable. A missed merge leaves two entities, and a wrong merge is unrecoverable without re-ingestion.
- Research decides the default for the LLM check. The lean is on when an extractor is configured and off otherwise, with an explicit opt-out. The default is logged at startup.
- Research confirms that WAL replay re-applies recorded merge mutations and never re-runs Phase B.
- Patch-sized change, milestone 0.16.3.
- Existing bad merges are not repaired by this change; recovery is by re-ingestion.

## Out of Scope

- Repairing or splitting existing wrong merges in already-ingested graphs.
- Changing `DEDUP_THRESHOLD`, the embedding model, or the exact-name path.
- Changing `knowledge_merge_entities` semantics.
- Better alias recall (`Kubernetes`/`K8s`, `Alice`/`Alice Smith` stay unmerged as today).

## Source References

- `crates/core/src/episode.rs` (`resolve_phase_b`, `DEDUP_THRESHOLD`, edge-endpoint salvage)
- `crates/core/src/dedup_adapter.rs`, `crates/core/src/app_state.rs`
- `docs/configuration.md`, `docs/adr/0029-name-first-entity-resolution.md`, `docs/adr/0051-edge-endpoint-salvage-and-deferred-drop.md`
- `specs/164-cross-episode-entity-resolution/spec.md`
- Issues #59 (Phase B note), #647 (merged summaries)
