# Feature Specification: lcg-eval passes the staged ontology to the spawned service and verifies it

**Feature Branch**: `fabrik/issue-881`
**Created**: 2026-11-22
**Status**: Specified
**Input**: User description: "lcg-eval: --ontology never reaches the spawned service (LIMINIS_WORKSPACE_ROOT unset); verify the loaded ontology. `lcg-eval retrieval --ontology <FILE>` (and `graph-qa`, which starts its service the same way) runs the LCG service **without the ontology**. Extraction is silently free-form, and nothing in the eval output says so. Cause: `spawn_service` (`crates/eval/src/retrieval/lcg.rs`) writes the ontology to `<workspace>/.lcg/ontology.yaml` but never sets `LIMINIS_WORKSPACE_ROOT` for the child process. The service finds its ontology only through that variable (`crates/core/src/app_state.rs`, then `load_ontology` in `crates/core/src/ontology.rs`). With the variable unset, the service logs `ontology: none — free-form extraction` to its own stderr, which lcg-eval doesn't keep. The `ontology: loaded 12 entity type(s), 27 relation type(s), mode=strict` line in lcg-eval's output comes from lcg-eval validating its own copy of the file, not from the service, so the run looks correct. The bug came in with ade1d26d (\"feat(eval): retrieval --ontology, --ontology-mode and --keep-workspace\", PR #842 / #821). It isn't in any release. The service is unaffected. Observed: a full v2 re-extraction with `--ontology ontology-v3.1.yaml` (strict) and `--record-cassette` produced 0 of 1006 recorded extract requests carrying the ontology's entity or relation types (the earlier E13 run, which set the variable by hand, had all 1006); 1683 of 1835 edges have relation types the ontology doesn't declare (E13 graph: 74 of 2620); the workspace's WAL ontology record has hash `none`; and the cassette's `.meta.json` records the strict v3.1 ontology's hash, so the cassette is mislabelled. Change: (1) `spawn_service` always sets `LIMINIS_WORKSPACE_ROOT` to the eval workspace, overriding any inherited value. (2) After startup, when `--ontology` is given, call `knowledge_get_ontology` for the eval group; if the service's hash doesn't match the staged file's hash or the service reports no ontology, exit non-zero with an error naming both hashes; do the same in `graph-qa` when its workspace has an ontology. (3) Cassette metadata records the hash the service reports, not the staged file's. (4) Add the service-reported ontology hash and mode to the run metadata in `report.md` and `report.json`."

## Background

`lcg-eval retrieval --ontology <FILE>` stages the ontology file into the eval workspace and then launches the LCG service as a child process. The service discovers its ontology solely through the `LIMINIS_WORKSPACE_ROOT` environment variable, which the eval harness never sets for the child. The service therefore starts with no ontology and extracts free-form, while lcg-eval's own output (which validates its own copy of the file) reports the ontology as loaded. The run looks correct but isn't.

The defect was introduced by the `--ontology` / `--ontology-mode` / `--keep-workspace` work (PR #842 / #821) and has not shipped in any release; the service itself is unaffected.

Observed impact on a full v2 re-extraction with a strict v3.1 ontology and cassette recording:

- 0 of 1006 recorded extract requests carry the ontology's entity or relation types (the earlier E13 run, with the variable set by hand, carried them in all 1006).
- 1683 of 1835 edges use relation types the ontology doesn't declare (E13: 74 of 2620).
- The workspace's WAL ontology record has hash `none`.
- The cassette's `.meta.json` records the strict v3.1 ontology's hash, so the cassette is mislabelled: replayed as an ontology cassette it would misreport its contents.

Beyond fixing the missing variable, the issue is that nothing in the harness checks what the service actually loaded, so this class of silent failure can recur. The fix therefore also adds a post-startup verification and makes the reports and cassette metadata state what the service reports rather than what the harness intended.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - `--ontology` actually reaches the service (Priority: P1)

An evaluator runs `lcg-eval retrieval --ontology <FILE>` (any mode). The service the harness spawns loads the staged ontology, so extraction prompts contain the ontology's entity and relation types and the resulting graph is ontology-constrained.

**Why this priority**: This is the defect itself. Without it every `--ontology` eval run is silently free-form and its results are invalid.

**Independent Test**: Start the service through the harness's spawn path with a staged ontology and query `knowledge_get_ontology` for the eval group; the returned hash equals the staged file's hash. This test must fail on current `main`.

**Acceptance Scenarios**:

1. **Given** a staged ontology in the eval workspace, **When** the service is started through the harness's spawn path, **Then** `knowledge_get_ontology` for the eval group returns the staged file's hash.
2. **Given** the operator's shell already has `LIMINIS_WORKSPACE_ROOT` pointing at an unrelated workspace, **When** the harness spawns the service, **Then** the service uses the eval workspace's ontology, not the unrelated workspace's.
3. **Given** `--ontology` and `--record-cassette`, **When** a chunk is ingested, **Then** the recorded extract request contains the ontology's entity-type and relation-type sections.
4. **Given** no `--ontology` flag, **When** the harness runs, **Then** behaviour is unchanged from before (free-form extraction, no ontology verification failure).

---

### User Story 2 - A run without the ontology fails loudly (Priority: P1)

If, for any reason, the service comes up with no ontology or a different one than the harness staged, the eval run stops with a clear error rather than proceeding and producing a plausible-looking but invalid result.

**Why this priority**: The original bug went unnoticed because nothing checked the service's state. A guard converts any future recurrence (or an inherited-environment surprise) into an immediate, diagnosable failure instead of hours of wasted extraction.

**Independent Test**: Simulate a service that reports a different hash (or no ontology) than the staged file; `lcg-eval retrieval --ontology <FILE>` exits non-zero with a message naming both hashes.

**Acceptance Scenarios**:

1. **Given** `--ontology <FILE>` and a service reporting a hash that differs from the staged file's hash, **When** startup verification runs, **Then** the command exits non-zero with an error naming both the expected (staged) and actual (service-reported) hashes.
2. **Given** `--ontology <FILE>` and a service reporting no ontology, **When** startup verification runs, **Then** the command exits non-zero with an error that states the service has no ontology and names the expected hash.
3. **Given** a matching hash, **When** verification runs, **Then** the run proceeds normally.
4. **Given** the verification fails, **Then** no extraction or ingest work is performed beyond what was needed to start and query the service.

---

### User Story 3 - `graph-qa` honours and verifies its workspace ontology (Priority: P2)

`lcg-eval graph-qa`, which starts its service the same way, starts that service with the workspace's ontology when the workspace has one, and verifies it with the same check.

**Why this priority**: `graph-qa` shares the defect but is a secondary entry point; the same fix and guard should apply for consistency.

**Independent Test**: Run `graph-qa` against a workspace containing an ontology and confirm the service reports that ontology's hash; with a mismatch, the command exits non-zero as in Story 2.

**Acceptance Scenarios**:

1. **Given** a `graph-qa` workspace that has an ontology, **When** the service starts, **Then** `knowledge_get_ontology` returns that ontology's hash and the run proceeds.
2. **Given** a `graph-qa` workspace with an ontology but a service that reports a different hash or none, **When** verification runs, **Then** the command exits non-zero with an error naming both hashes.
3. **Given** a `graph-qa` workspace with no ontology, **When** it runs, **Then** behaviour is unchanged and no verification failure occurs.

---

### User Story 4 - Reports and cassettes state what the service actually loaded (Priority: P2)

Run metadata in `report.md` and `report.json` shows the ontology hash and mode as reported by the service, and the cassette's `.meta.json` records the service-reported hash. A free-form run can no longer be mistaken for an ontology run, and a cassette's label matches its contents.

**Why this priority**: Fixes the mislabelled-cassette symptom and makes any future discrepancy visible to a reader of the report, independent of the startup guard.

**Independent Test**: Run an eval with `--ontology` and inspect `report.md`, `report.json` and the cassette `.meta.json`; each carries the service-reported hash (and, for reports, mode). Run without `--ontology` and confirm the report shows the run as having no ontology.

**Acceptance Scenarios**:

1. **Given** a run with `--ontology`, **When** the report is written, **Then** `report.md` and `report.json` both include the service-reported ontology hash and mode.
2. **Given** a run without an ontology, **When** the report is written, **Then** the ontology field in both reports indicates none (free-form) rather than being omitted ambiguously.
3. **Given** `--record-cassette` with `--ontology`, **When** the cassette is finalised, **Then** `.meta.json` records the hash obtained from the service, not a hash computed from the staged file.

---

### Edge Cases

- The operator's environment already defines `LIMINIS_WORKSPACE_ROOT` (to the same or a different workspace): the harness value always wins.
- The service reports an ontology with the right hash but a different mode than requested: the mode is recorded as the service reports it in the run metadata. (Whether a mode mismatch should also abort is not required by this issue; see Assumptions.)
- The service is slow to become ready: verification runs only after the existing readiness check passes, and a failure to reach the service is reported as a startup failure, not as an ontology mismatch.
- `knowledge_get_ontology` returns an error or malformed response: the run exits non-zero with a message that identifies the verification step as the failure point.
- Runs without `--ontology` (including `graph-qa` on a workspace with no ontology): no verification is attempted and nothing about the run changes.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The harness's service-spawn path MUST always set `LIMINIS_WORKSPACE_ROOT` for the child process to the eval workspace, overriding any value inherited from the operator's environment.
- **FR-002**: When `--ontology` is given to `lcg-eval retrieval`, after the service is ready the harness MUST query the service's loaded ontology for the eval group via `knowledge_get_ontology`.
- **FR-003**: If the service-reported ontology hash differs from the staged file's hash, or the service reports no ontology, `lcg-eval retrieval` MUST exit non-zero with an error message naming both the expected (staged) hash and the actual (service-reported) hash or its absence.
- **FR-004**: `lcg-eval graph-qa` MUST start its service with the workspace's ontology when the workspace has one, and MUST apply the same verification and failure behaviour as FR-002/FR-003.
- **FR-005**: The cassette's `.meta.json` MUST record the ontology hash reported by the service, not a hash computed from the staged file.
- **FR-006**: `report.md` and `report.json` MUST include the ontology hash and mode as reported by the service in the run metadata; for runs without an ontology they MUST explicitly indicate none.
- **FR-007**: With `--ontology`, recorded cassette extract requests MUST contain the ontology's entity-type and relation-type sections.
- **FR-008**: Runs without `--ontology` (and `graph-qa` runs on a workspace without an ontology) MUST behave as they did before this change.
- **FR-009**: The change MUST include an integration test that starts the service via the harness spawn path with a staged ontology and asserts `knowledge_get_ontology` returns the staged hash, and that fails on the pre-fix code.
- **FR-010**: The change MUST include a test of the guard: a service that comes up without the staged ontology (e.g. a simulated hash mismatch) causes `lcg-eval retrieval` to exit non-zero with a clear message.
- **FR-011**: The change MUST add a CHANGELOG `Unreleased` entry, and `docs/llms-full.txt` MUST be regenerated if any documentation changes.

### Key Entities *(if applicable)*

- **Staged ontology**: The ontology file the harness copies into the eval workspace's `.lcg` directory; the harness's *intended* ontology, identified by its content hash.
- **Service-reported ontology**: The hash and mode the running service returns from `knowledge_get_ontology` for the eval group; the *actual* ontology in effect and the authoritative value for reports and cassette metadata.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: In a run with `--ontology` and cassette recording, 100% of recorded extract requests contain the ontology's entity-type and relation-type sections (previously 0%).
- **SC-002**: For every `--ontology` run that completes, the service-reported ontology hash equals the staged file's hash.
- **SC-003**: Any `--ontology` run in which the service lacks the staged ontology exits non-zero before extraction work begins, with an error naming both hashes (zero silently free-form runs).
- **SC-004**: Every report and cassette produced states the ontology hash/mode the service reported; no report or cassette from an ontology-less service carries an ontology hash.
- **SC-005**: Runs without `--ontology` produce the same results and exit status as before the change.
- **SC-006**: The new integration test fails on current `main` and passes after the change.

## Assumptions

- The mismatch guard compares ontology **hash** only (as specified); a mode difference is recorded in the report but does not by itself abort the run.
- `knowledge_get_ontology` already exists and returns the hash (and mode) of the ontology loaded for a group; no change to the service or its own ontology discovery is needed.
- "The eval group" is the group the harness already uses for ingestion and queries in the run.
- For `graph-qa`, "workspace has an ontology" means an ontology file is present in the workspace's `.lcg` directory at start time.
- Reports for runs without an ontology show an explicit "none" value rather than omitting the field; this is a low-impact presentational choice that can be adjusted.

## Out of Scope *(optional)*

- Re-running the affected evals; the maintainer re-runs the joint re-extraction after this lands.
- Any change to the service's own ontology discovery (`LIMINIS_WORKSPACE_ROOT` handling, `load_ontology`).
- Capturing or persisting the child service's stderr in lcg-eval.

## Source References *(optional)*

- Introducing change: ade1d26d, PR #842 / #821.
- Spawn path: `crates/eval/src/retrieval/lcg.rs` (`spawn_service`); service discovery: `crates/core/src/app_state.rs`, `crates/core/src/ontology.rs`.
- Existing `knowledge_get_ontology` documentation: `docs/ontology.md`, `docs/ipc-mcp-reference.md`.
