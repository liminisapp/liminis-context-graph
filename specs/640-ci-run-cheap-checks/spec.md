# Feature Specification: CI cheap-checks-first pre-gate and run-wide fail-fast

**Feature Branch**: `fabrik/issue-640`
**Created**: 2026-09-30
**Status**: Specified
**Input**: User description: "CI: run cheap checks first (fmt/guards pre-gate) and fail fast — cancel the run on the first job failure"

## Background

`ci.yml` runs its cheapest checks last and has no fail-fast, so a trivial failure costs a full CI run and a failure in one job does not stop the others. Verified on `main`:

1. **The cheapest check runs last.** `cargo fmt --check` is the final step of the `test` job. It runs after the ~10-minute `build-release` compile, the whole `cargo test --release` suite, the R-003 bench, `cargo check --release --benches` and `cargo clippy --release`. Formatting needs no compile and takes seconds, yet a slip is reported only after about 15 minutes.
2. **Other no-compile guards also wait for the release build.** They sit inside `test`, which `needs: build-release`: the `eval script guards` step (`crates/eval/scripts/test-scripts.sh`, which by its own header needs no network, API key or built binary) and the `check no ML runtime deps` step (which uses `cargo tree` and only resolves metadata).
3. **No fail-fast.** `test` and the six real-corpus e2e jobs (`real_corpus_e2e`, `wal_root_migration`, `mcp_real_corpus_e2e`, `mcp_real_corpus_mutation_e2e`, `mcp_real_corpus_admin_data_e2e`, `mcp_real_corpus_admin_lifecycle_e2e`) each depend only on `[changes, build-release]`, so they run in parallel and independently. If `test` fails, all six e2e jobs still run to completion, and vice versa. The only cancellation configured is `concurrency: cancel-in-progress`, which fires on a superseding push, not on a failure.

A PR cannot merge if any required check fails, so every minute spent after the first real failure is wasted runner time and also holds up Fabrik's all-checks CI gate.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Trivial failures are reported in about a minute (Priority: P1)

A contributor pushes a commit with a formatting violation (or a failing no-compile guard). They learn about it from a cheap pre-gate job within about a minute, and no compile or test job is started.

**Why this priority**: The cheap checks are the most common cause of avoidable failures, and today each one costs ~15 minutes of runner time plus contributor wait.

**Independent Test**: Push a PR commit with a deliberate `cargo fmt` violation and observe the run.

**Acceptance Scenarios**:

1. **Given** a PR commit with a `cargo fmt` violation, **When** CI runs, **Then** the `lint` job fails within about a minute and the release build, `test` and all e2e jobs do not run.
2. **Given** a PR commit that violates the ML-runtime-deps guard or the eval script guards, **When** CI runs, **Then** the `lint` job fails and no compile job runs.
3. **Given** a green PR commit, **When** CI runs, **Then** `lint` passes and the build proceeds as before.

---

### User Story 2 - The first real failure stops the rest of the run (Priority: P1)

When `test` (or any post-build job) fails, the sibling jobs still in flight are cancelled promptly instead of running to completion. The original failure remains the visible cause.

**Why this priority**: Saves runner time after the outcome is already decided, and releases Fabrik's all-checks CI gate sooner.

**Independent Test**: Push a PR commit with a failing unit test and observe that the in-flight e2e jobs are cancelled while `test` shows as the failure.

**Acceptance Scenarios**:

1. **Given** a failing unit test in `test` while the e2e jobs are running, **When** `test` fails, **Then** the in-flight e2e jobs are cancelled promptly, `test` is shown as failed, and the others as cancelled.
2. **Given** a failing e2e job while `test` is running, **When** the e2e job fails, **Then** `test` and the other e2e jobs are cancelled and the failing e2e job is the visible failure.
3. **Given** the release build itself fails, **When** it fails, **Then** any remaining jobs in the run are cancelled.
4. **Given** a fully green run, **When** it completes, **Then** nothing is cancelled and wall-clock time is no worse than today apart from the ~1-minute pre-gate.
5. **Given** a failing job, **When** the jobs stay parallel, **Then** e2e jobs are not serialized behind `test`.

---

### User Story 3 - Post-merge failures are still reported (Priority: P1)

A post-merge run on `main` that fails and is then fail-fast-cancelled still opens or updates the `ci-failure` tracking issue (ADR-0298).

**Why this priority**: Fail-fast would otherwise silently defeat the failure notifier if the run's overall conclusion ends up `cancelled`, which the notifier currently ignores.

**Independent Test**: Simulate or inspect a failing `main` run that ended with cancelled sibling jobs and confirm the notifier files or updates a tracking issue naming the real failing job.

**Acceptance Scenarios**:

1. **Given** a run on `main` where one job failed and siblings were cancelled, **When** the run completes, **Then** a tracking issue is opened or updated and identifies the genuinely failed job, not the cancelled siblings.
2. **Given** a run on `main` superseded by a newer push (`cancel-in-progress`) with no real failure, **When** it completes as cancelled, **Then** no tracking issue is filed.

---

### User Story 4 - Docs-only PRs and required-check reporting are unchanged (Priority: P2)

Docs-only PRs still take the fast path (ADR-0322), and the required `test (ubuntu-latest)` check still reports on every PR.

**Why this priority**: Regression guard for merge gating.

**Independent Test**: Open a docs-only PR and confirm the heavy jobs skip and `test (ubuntu-latest)` reports as skipped/passing.

**Acceptance Scenarios**:

1. **Given** a docs-only PR, **When** CI runs, **Then** the `build-release` compile, `test` and e2e jobs are skipped and `test (ubuntu-latest)` still reports a satisfying conclusion.
2. **Given** a classification error, **When** CI runs, **Then** the full suite runs as today.

---

### User Story 5 - The build job name reflects what it does (Priority: P3)

The `build-release` job is renamed (for example to `build (release profile)`) so it is not confused with the cargo-dist Release workflow.

**Why this priority**: Clarity only; no behavioural change.

**Independent Test**: Inspect the run's job list and grep the repo for stale references.

**Acceptance Scenarios**:

1. **Given** the rename, **When** CI runs, **Then** the job appears under its new display name and all references (workflow comments, `needs:` ids if changed, docs, ADRs that cite it as a live name) are consistent.
2. **Given** branch protection, **When** the rename lands, **Then** `test (ubuntu-latest)` remains the sole required check under exactly that name.

---

### Edge Cases

- **Cancelled vs failed misreporting**: a job whose own cancellation step cancels the run may itself be marked cancelled, hiding the real failure. The originating failure must still be recoverable from the run (job conclusions), and the run's overall status must not make a real failure look like a supersede-cancellation.
- **Notifier skips cancelled runs**: `ci-failure-notify.yml` returns early when the run conclusion is `cancelled`; a fail-fast cancel on `main` must not be treated as a supersede.
- **Two jobs fail simultaneously**: both may trigger cancellation; the cancel step must be idempotent and not itself fail the job or mask the failure.
- **Cancel step cannot run or lacks permission** (e.g. fork PRs with a read-only token): the job's original failure must still be reported and the run must not hang or go green.
- **Skipped jobs**: on docs-only PRs, jobs skipped via `if:` must not trigger cancellation and must not break `test (ubuntu-latest)` reporting.
- **`workflow_dispatch` with `e2e_only`**: the existing behaviour of skipping `test` must continue to work with the new `lint` gate.
- **Pre-gate skipped vs failed**: `build-release` must run when `lint` succeeds and must not run when `lint` fails; a skipped `lint` (docs-only) must not block anything that should otherwise report.
- **Concurrency supersede**: pushes that supersede a run continue to cancel it via `cancel-in-progress`; fail-fast must not interfere.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: CI MUST include a `lint` pre-gate job that runs `cargo fmt --all --check`, `crates/eval/scripts/test-scripts.sh` and the ML-runtime-deps guard (`cargo tree`), without compiling the workspace.
- **FR-002**: The release-profile build job MUST NOT start unless `lint` has succeeded (or been legitimately skipped under the docs-only fast path).
- **FR-003**: The `fmt`, `eval script guards` and `check no ML runtime deps` steps MUST be removed from `test` so they run exactly once, in `lint`.
- **FR-004**: On the green path the `lint` job SHOULD complete in roughly 30–60 seconds.
- **FR-005**: The docs-only fast path (ADR-0322) MUST be preserved: the `changes` classification still lets docs-only PRs skip the heavy jobs, and the required `test (ubuntu-latest)` check MUST still report on every PR.
- **FR-006**: The release-profile build job and every job after it (`test` and the six real-corpus e2e jobs) MUST, on failure of that job, cancel the workflow run so that in-flight sibling jobs stop promptly.
- **FR-007**: Jobs after the build MUST remain parallel; e2e jobs MUST NOT be serialized behind `test`.
- **FR-008**: Cancellation MUST NOT misreport: sibling jobs stopped by fail-fast MUST show as cancelled, and the originally failing job MUST remain identifiable as the cause.
- **FR-009**: The `ci-failure` notifier (`ci-failure-notify.yml`, ADR-0298) MUST still open or update a tracking issue for a `main` run that had a genuine job failure and ended with cancelled jobs, and the issue MUST name the genuinely failed job(s). It MUST continue to ignore runs cancelled solely by a superseding push.
- **FR-010**: Fail-fast cancellation MUST be granted only the minimum additional token permission needed on the jobs that perform it; the workflow-level default permission MUST remain read-only.
- **FR-011**: The `build-release` job MUST be renamed to something that reflects it is the run's single release-profile build (for example `build (release profile)`), and all references (workflow files, comments, docs, ADRs where they cite it as a current name) MUST be updated consistently.
- **FR-012**: The required check name `test (ubuntu-latest)` MUST be unchanged, and no other check name change may break branch protection.
- **FR-013**: Whether `cargo clippy` also moves into the pre-gate (as a debug-profile `--all-targets -D warnings` run, which costs a compile) or stays in `test` against the restored release build is to be decided by Research, weighing green-path cost against earlier failure. Either outcome MUST keep clippy running exactly once per run and MUST keep `-D warnings` enforcement.

### Key Entities

- **`lint` job**: New cheap pre-gate; gates the release-profile build.
- **Release-profile build job** (formerly `build-release`): The run's single `cargo build --release` whose outputs `test` and the e2e jobs reuse.
- **Fail-fast cancellation**: A failure-triggered cancel of the whole workflow run, issued from the failing job.
- **`ci-failure` notifier**: `workflow_run` listener that files tracking issues for failing runs on `main`.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A PR commit with a `cargo fmt` violation fails in `lint` within about one minute, and the build, `test` and e2e jobs do not run.
- **SC-002**: A PR commit with a failing unit test in `test` causes the in-flight e2e jobs to be cancelled promptly; the run shows `test` as the failure and the rest as cancelled.
- **SC-003**: A green PR run's wall-clock time is no worse than today apart from the ~1-minute pre-gate.
- **SC-004**: A docs-only PR still takes the fast path and `test (ubuntu-latest)` still reports.
- **SC-005**: The `ci-failure` notifier still opens or updates a tracking issue for a failing post-merge run on `main` that ended with cancellations, naming the real failing job.
- **SC-006**: No step (`fmt`, eval script guards, ML-deps guard) is executed in both `lint` and `test`.

## Assumptions

- `test (ubuntu-latest)` is the only required status check (ADR-0322), so renaming the build job does not affect branch protection, but Research should verify the current branch-protection contexts.
- Cancelling the run from within a failing job can leave the run's overall conclusion as `cancelled`; Research must verify the real behaviour of GitHub Actions here, since it determines how FR-008 and FR-009 are satisfied.
- The `lint` job can run fmt, the eval script guards and the `cargo tree` guard without the heavy lbug/OpenSSL build setup.
- Fork PRs may receive a read-only token; fail-fast is best-effort there and must not turn a failure into a pass.

## Out of Scope

- The cargo-dist `Release` workflow running on PRs (#639).
- `windows.yml`: a separate workflow that CI cannot cancel directly. It could later get its own pre-gate or fail-fast.
- Changing the content, selection or ordering of tests within `test` or the e2e jobs.

## Source References

- `.github/workflows/ci.yml`, `.github/workflows/ci-failure-notify.yml`, `.github/actions/classify-changes/action.yml`
- `crates/eval/scripts/test-scripts.sh`
- ADR-0322 (docs-only fast path), ADR-0298 (CI failure notification), ADR-0341 (build release artifacts once)
- Issues #341, #639
