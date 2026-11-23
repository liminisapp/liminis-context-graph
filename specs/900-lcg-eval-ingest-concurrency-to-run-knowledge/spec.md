# Feature Specification: lcg-eval: --ingest-concurrency to run knowledge_process_chunk calls in parallel

**Feature Branch**: `fabrik/issue-900`
**Created**: 2026-11-23
**Status**: Specified
**Input**: User description: "## Problem

`lcg-eval retrieval` ingests a corpus one chunk at a time. A live extraction of v2 takes about 3 hours, almost all of it spent waiting on one LLM call after another. Every re-extraction experiment pays that cost (E16 and E17 in `docs/retrieval-experiments.md`).

**Cause.**
- `ingest()` in `crates/eval/src/retrieval/lcg.rs` awaits each `knowledge_process_chunk` before sending the next.
- The UDS transport (`exchange`) holds one locked connection for each request/response pair, so there is never more than one request in flight anyway.
- The service doesn't need this. `process_chunk` Phase A (embed and extract) runs without a lock, and concurrent ingests for a group are already supported (`episode.rs`, `ontology_sidecar.rs`).

**Observed (E17).** The LLM endpoint offered 6 concurrent slots. Over about 3,650 calls averaging 2.7 s each, it was about 17% busy, which is roughly one slot.

## Change

1. **`--ingest-concurrency N` on `lcg-eval retrieval`** (and anything else that calls `ingest()`), default 1. Keep up to N `knowledge_process_chunk` calls in flight, using separate connections or a multiplexed transport.
2. **Keep the outputs deterministic where they matter:**
   - Record per-chunk results (`episode_note`, counts) by chunk, not by completion order.
   - Supersessions, `knowledge_build_indices` and the entity-link pass run after all chunks finish, as they do now.
3. **Report:** add the concurrency used to the run metadata in `report.md` and `report.json`.

## Cassettes: decide during Research

Some cassette keys depend on graph state at call time. `consolidate_summary` hashes the existing summary, and the dedup judge hashes the candidate pairs (`crates/core/src/cassette.rs`). With N > 1, the order chunks finish in changes that state, so:
- a cassette recorded at N > 1 may not replay with zero misses at N = 1, or even at N > 1;
- extraction becomes order-dependent, so two live runs at N > 1 can give slightly different graphs.

Research should measure this: record a cassette at N = 4 on a small corpus, replay it at N = 1, and count the misses. Then choose one of these:
- (a) replay stays exact, and nothing more is needed;
- (b) `--record-cassette` with N > 1 is refused, or warns and marks the cassette meta as order-dependent;
- (c) something better that Research finds.

Replay with `--replay-cassette` must stay deterministic. Forcing N = 1 under replay is acceptable.

## Acceptance

- With N = 1, behaviour and outputs are unchanged. The existing tests pass, and a replayed run gives identical `report.json` metrics.
- A test with a stub extractor that delays each call shows N = 4 keeping 4 calls in flight and finishing in about a quarter of the time.
- The chosen cassette rule is implemented and tested.
- A failed chunk still fails the run with the note id and chunk index in the error, as now.
- `docs/retrieval-quality-evaluation.md` documents the flag and its cassette caveat. CHANGELOG `Unreleased` entry. Regenerate `docs/llms-full.txt`.

## Out of scope

- Concurrency inside the service's own `process_chunk`. Each chunk's entity, edge and summary calls are already sequenced on purpose.
- Changing `graph-qa`, which reads an existing workspace and doesn't ingest."

## Background

`lcg-eval retrieval` ingests a corpus one `knowledge_process_chunk` call at a time. A live extraction of the v2 corpus takes about 3 hours, almost all of it waiting on sequential LLM calls, and every re-extraction experiment (E16, E17 in `docs/retrieval-experiments.md`) pays that cost.

The serialisation is entirely on the eval-harness side: the ingest loop awaits each chunk before sending the next, and its Unix-socket transport holds a single locked connection per request/response pair, so at most one request is ever in flight. The service does not require this — the embed-and-extract phase of chunk processing runs without a lock, and concurrent ingests into one group are already supported. In E17 the LLM endpoint offered 6 concurrent slots but, across about 3,650 calls averaging 2.7 s each, was only about 17% busy (roughly one slot's worth).

The change is to let the harness keep up to N chunk calls in flight, while keeping the outputs that matter deterministic. The main open design question — how concurrency interacts with record/replay cassettes, whose keys depend on graph state at call time (the existing-summary hash in `consolidate_summary`, the candidate-pair hash in the dedup judge) — is deliberately delegated to the Research stage, which must measure it before choosing a rule.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Faster live ingest with a concurrency flag (Priority: P1)

An evaluator running `lcg-eval retrieval` against a live LLM endpoint passes `--ingest-concurrency N` and the corpus is ingested with up to N chunk-processing calls in flight at once, cutting wall-clock time roughly by a factor of N (up to the endpoint's capacity).

**Why this priority**: This is the whole point of the issue — removing the ~3 hour per-experiment cost of re-extraction.

**Independent Test**: Run ingest against a stub extractor that delays each call by a fixed time; with N = 4, assert that 4 calls are observed in flight simultaneously and total time is about a quarter of the N = 1 time.

**Acceptance Scenarios**:

1. **Given** a corpus of many chunks and a stub extractor with a fixed per-call delay, **When** ingest runs with `--ingest-concurrency 4`, **Then** the peak number of in-flight `knowledge_process_chunk` calls is 4 and elapsed time is about a quarter of the N = 1 elapsed time.
2. **Given** the flag is omitted, **When** ingest runs, **Then** it behaves as N = 1 and never has more than one call in flight.
3. **Given** any command that calls the shared ingest routine (not only `retrieval`), **When** the flag is supplied, **Then** it is honoured identically.

---

### User Story 2 - N = 1 is unchanged (Priority: P1)

An evaluator who does not use the flag (or passes `1`) sees exactly the behaviour and outputs they get today, including cassette replays reproducing identical metrics.

**Why this priority**: The eval harness is a regression gate; changing default behaviour would invalidate existing baselines and recorded cassettes.

**Independent Test**: Run the existing test suite, and replay an existing cassette at the default and compare `report.json` metrics with those from before the change.

**Acceptance Scenarios**:

1. **Given** an existing recorded cassette, **When** a run replays it with no `--ingest-concurrency`, **Then** `report.json` metrics are identical to the pre-change run.
2. **Given** the existing test suite, **When** it runs after the change, **Then** all tests pass.

---

### User Story 3 - Deterministic per-chunk results and post-ingest passes (Priority: P2)

Regardless of N, per-chunk results are attributed to the chunk they belong to rather than the order calls completed in, and the whole-corpus passes still run only once all chunks have finished.

**Why this priority**: Without it, results and reports would vary run to run for reasons unrelated to extraction quality, undermining comparison between experiments.

**Independent Test**: With a stub whose calls complete in a scrambled order, assert per-chunk results (`episode_note`, counts) in the output are in chunk order and match those from an N = 1 run; assert supersessions, index build and entity-link pass start only after the last chunk completes.

**Acceptance Scenarios**:

1. **Given** chunks completing out of order, **When** ingest finishes, **Then** per-chunk results are keyed and ordered by chunk, not by completion.
2. **Given** ingest at N > 1, **When** the last chunk finishes, **Then** supersessions, `knowledge_build_indices` and the entity-link pass run afterwards, never overlapping any chunk call.

---

### User Story 4 - Safe cassette behaviour under concurrency (Priority: P2)

An evaluator recording or replaying a cassette gets a defined, documented, tested behaviour when N > 1, so they never silently produce a cassette that fails to replay or a replay that is non-deterministic.

**Why this priority**: Cassettes are the free, repeatable path for regression runs; a silently order-dependent cassette would break that guarantee.

**Independent Test**: Research records a cassette at N = 4 on a small corpus and replays it at N = 1 (and N = 4), counting misses; the chosen rule (exact replay, refuse/warn-and-mark on record at N > 1, or another rule Research finds) is then covered by tests.

**Acceptance Scenarios**:

1. **Given** `--replay-cassette`, **When** a run is made with any N, **Then** the result is deterministic (forcing N = 1 under replay is acceptable).
2. **Given** `--record-cassette` with N > 1, **When** the run starts, **Then** the chosen rule is applied (e.g. refuse, or warn and mark the cassette meta as order-dependent) and the behaviour is covered by a test.

---

### User Story 5 - Failures and reporting remain clear (Priority: P3)

When a chunk fails, the run fails with the note id and chunk index in the error as it does today; and the report records which concurrency was used.

**Why this priority**: Diagnosability and reproducibility of experiments; lower risk than the core behaviour.

**Independent Test**: Make a stub fail one chunk at N = 4 and check the error text; inspect `report.md` and `report.json` for the concurrency value.

**Acceptance Scenarios**:

1. **Given** one chunk fails while others are in flight, **When** ingest returns, **Then** the run fails with an error naming the note id and chunk index of the failing chunk.
2. **Given** a completed run, **When** `report.md` and `report.json` are read, **Then** both record the concurrency used.

---

### Edge Cases

- N larger than the number of chunks: behaves as one call per chunk, no error.
- N = 0 or a non-numeric value: rejected with a clear usage error (not silently treated as 1).
- Several chunks failing concurrently: the run still fails, and the reported error identifies at least one failing note id and chunk index deterministically enough to debug.
- A failure while other calls are in flight: no new chunk calls should be started after the failure is known, and in-flight calls are not left running unobserved after the run reports failure.
- Chunks of the same note whose results are order-sensitive in the graph (e.g. per-note ordering, supersessions): the pre-existing post-ingest supersession pass still sees all chunks finished.
- `--replay-cassette` combined with N > 1: must remain deterministic (replay may force N = 1; if it does, the concurrency recorded in the report reflects what was actually used, or is clearly documented).
- The run-metadata concurrency value must not perturb any existing hash/digest of inputs that determines cassette or baseline comparability at N = 1.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: `lcg-eval retrieval` MUST accept `--ingest-concurrency N` (positive integer, default 1), and the shared ingest routine MUST honour it for every command that calls it.
- **FR-002**: With concurrency N, ingest MUST keep up to N `knowledge_process_chunk` calls in flight simultaneously (not more than N), using separate connections or a multiplexed transport.
- **FR-003**: With N = 1 (default or explicit), ingest behaviour and outputs MUST be identical to current behaviour, and a replayed cassette run MUST produce identical `report.json` metrics.
- **FR-004**: Per-chunk results (`episode_note`, counts) MUST be recorded by chunk identity, so that the resulting output order and values do not depend on call completion order.
- **FR-005**: Supersessions, `knowledge_build_indices` and the entity-link pass MUST run only after all chunk calls have finished, as they do today.
- **FR-006**: A failed chunk MUST fail the run with an error containing the note id and chunk index, as today.
- **FR-007**: `report.md` and `report.json` run metadata MUST include the ingest concurrency used.
- **FR-008**: Invalid values (0, negative, non-numeric) for `--ingest-concurrency` MUST be rejected with a clear error.
- **FR-009**: A defined cassette rule for N > 1 MUST be chosen based on measurement during Research (record a cassette at N = 4 on a small corpus, replay at N = 1, count misses) and implemented and tested. Acceptable outcomes are: (a) replay is exact and nothing more is needed; (b) `--record-cassette` with N > 1 is refused, or warns and marks the cassette meta as order-dependent; or (c) a better approach identified by Research.
- **FR-010**: Under `--replay-cassette`, results MUST remain deterministic; forcing N = 1 under replay is permitted.
- **FR-011**: `docs/retrieval-quality-evaluation.md` MUST document the flag and its cassette caveat; a CHANGELOG `Unreleased` entry MUST be added; `docs/llms-full.txt` MUST be regenerated.
- **FR-012**: Tests MUST include a stub extractor that delays each call, demonstrating N = 4 keeps 4 calls in flight and completes in about a quarter of the N = 1 time.

### Key Entities *(if applicable)*

- **Chunk ingest call**: one `knowledge_process_chunk` request for a given note id and chunk index; its result (episode note, counts) is attributed to that chunk.
- **Run metadata**: the settings block in `report.md` / `report.json` that records how a run was configured, now including ingest concurrency.
- **Cassette**: a recorded set of LLM/embedding responses keyed partly by graph-state hashes, with a `.meta.json` sidecar that can carry flags such as order-dependence.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: With a fixed-delay stub, a run at N = 4 shows a peak of exactly 4 calls in flight and wall-clock ingest time within roughly 25–35% of the N = 1 time.
- **SC-002**: At N = 1, the existing test suite passes unchanged and a replayed run's `report.json` metrics are byte-for-byte identical to pre-change metrics.
- **SC-003**: For a stub whose calls finish out of order, per-chunk results at N = 4 are identical in content and order to those at N = 1.
- **SC-004**: A cassette recorded at N = 4 is either replayed with zero misses, or the recording is refused / explicitly marked order-dependent — never silently unreplayable; the measured miss count from Research is documented.
- **SC-005**: A failing chunk at any N yields an error containing its note id and chunk index.
- **SC-006**: Every generated `report.md` and `report.json` states the concurrency used.
- **SC-007**: On a live endpoint with spare slots, a full re-extraction at N close to the slot count takes materially less wall-clock time than the ~3 hour sequential baseline (expected to approach baseline / N, bounded by endpoint capacity).

## Assumptions

- The service already supports concurrent `knowledge_process_chunk` ingests into one group (stated in the issue); no service-side change is needed.
- The default stays 1, so no existing workflow changes unless the flag is passed.
- Live runs at N > 1 may produce slightly different graphs from run to run because extraction becomes order-dependent; this is accepted and documented, not eliminated.
- Forcing N = 1 under replay is an acceptable way to keep replay deterministic if Research finds replay at N > 1 is not exact.
- Whether recording the concurrency in run metadata affects any input digest is to be decided so that N = 1 digests and baselines stay comparable (FR-003).

## Out of Scope *(optional)*

- Concurrency inside the service's own `process_chunk` (each chunk's entity, edge and summary calls are sequenced on purpose).
- Changing `graph-qa`, which reads an existing workspace and does not ingest.
- Making N > 1 live extraction produce order-independent graphs.

## Source References *(optional)*

- Issue #900 (this issue); `docs/retrieval-experiments.md` (E16, E17); `docs/retrieval-quality-evaluation.md` (existing `--record-cassette` / `--replay-cassette` documentation); `docs/testing-and-evaluation.md` (record/replay cassettes).
