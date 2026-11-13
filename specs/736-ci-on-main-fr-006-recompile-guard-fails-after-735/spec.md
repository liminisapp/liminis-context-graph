# Feature Specification: Fix FR-006 recompile guard failing on `main` after #735

**Feature Branch**: `fabrik/issue-736`
**Created**: 2026-11-13
**Status**: Specified
**Input**: User description: the original issue body, reproduced verbatim below.

> ## Problem
>
> Since #735 merged (`297e2d03`, 2026-11-13), the required `test (ubuntu-latest)` job fails **on `main` only**. Two attempts of [run 43439700482](https://github.com/liminisapp/liminis-context-graph/actions/runs/43439700482) failed identically.
>
> - `cargo test --release` passes. Every test is green.
> - The next step, **"Assert no workspace code recompiled after restoring build-release's output (FR-006)"**, fails because three workspace crates recompiled:
>   ```
>   Compiling lcg-core v0.18.0 (.../crates/core)
>   Compiling lcg-service v0.18.0 (.../crates/service)
>   Compiling lcg-eval v0.18.0 (.../crates/eval)
>   (3 Compiling line(s) in total)
>   ```
> - The fail-fast step then cancels the run, which is why the CI-failure notifier reported it as "cancelled".
>
> **The same guard passed** on #735's own PR run (merge ref `32ee4f10`, [run 43435957267](https://github.com/liminisapp/liminis-context-graph/actions/runs/43435957267)) and on `main` at `104f21c8` (2026-11-07). #735 changed only `[workspace.metadata.dist.github-custom-runners]` in the root `Cargo.toml`.
>
> **Ruled out:**
> - *A future-dated commit.* `297e2d03`'s commit time is 14:07:04Z; `build` ran 14:08–14:20 and `test` 14:20–14:31.
> - *Build scripts reading CI environment variables.* `crates/core/build.rs` and `crates/service/build.rs` only read `CARGO_CFG_TARGET_FAMILY`. No workspace crate uses `env!`/`option_env!` beyond `CARGO_*`.
> - *A dependency or toolchain change.* Only the three path crates recompile, not registry dependencies, so it isn't a rustc change.
>
> **Relevant mechanics** (`ci.yml`, see #341 / #657 / ADR-0341):
> - both jobs stamp tracked sources with the commit time (`git ls-files -z | xargs -0 touch -d "@$(git log -1 --format=%ct)"`);
> - the test job restores `target/release` from the `build` job's artifact, then stamps every restored file with the job's current time.
>
> ## Acceptance criteria
>
> 1. Find the actual cause, using cargo's own fingerprint evidence rather than inference: for example, run the guard's build with `CARGO_LOG=cargo::core::compiler::fingerprint=info` and read the `Dirty …` lines. Record the cause in the PR description.
> 2. Fix it so the guard passes on `main` pushes as well as on PRs, without weakening the FR-006 guard. Whitelisting the three crates is not acceptable.
> 3. Show it passing: a green `test (ubuntu-latest)` on the PR, and an explanation of why the `main` push will now behave the same. If the cause is push-specific, show it on a `main`-like ref, for example by re-running with `workflow_dispatch` on the branch if the workflow supports it.
> 4. Close this issue with the fix.
>
> ## Out of scope
>
> Other CI restructuring. The Release and Windows workflows, which are green on `main`.

## Background

CI's `test (ubuntu-latest)` job restores the release build output produced by the `build` job and then runs `cargo test --release`. A guard step (FR-006, from #341 / #657 / ADR-0341) asserts that restoring that output leaves nothing in the workspace needing recompilation. Its purpose is to keep the test job from silently paying for a second release build of workspace code, which is the cost the build/test split exists to avoid.

Since #735 merged to `main` (`297e2d03`, 2026-11-13), that guard fails on `main` pushes. Three workspace crates (`lcg-core`, `lcg-service`, `lcg-eval`) are recompiled even though tests pass and no registry dependency recompiles. The same guard passed on #735's own PR run and on earlier `main` commits, and #735 only touched release-runner metadata in the root manifest. Because `test (ubuntu-latest)` is a required check and its failure cancels the run, `main` is currently red and the notifier reports it as "cancelled".

The failure is a regression in how CI decides "nothing needs rebuilding" (cargo fingerprint freshness). The cause is not yet established. Several hypotheses are already ruled out (future-dated commit, build scripts reading CI env vars, dependency or toolchain change). The most valuable first step is to obtain cargo's own fingerprint evidence for why those crates are considered dirty, rather than to infer.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Establish the actual cause from cargo's own evidence (Priority: P1)

A maintainer investigating the red `main` needs to know precisely why cargo considers the three workspace crates stale after the restore, in terms cargo itself reports, so that the fix addresses the real cause and not a guess.

**Why this priority**: Without a evidenced cause, any fix risks masking the problem, weakening the guard, or breaking again on the next push. Everything else depends on it.

**Independent Test**: Run the guard's build under cargo's fingerprint logging on the failing configuration and read the reported reason each of the three crates is marked dirty. The PR description states that reason and quotes or links the evidence.

**Acceptance Scenarios**:

1. **Given** the failing `main` configuration, **When** the guard's build is run with cargo fingerprint logging enabled, **Then** the log shows a `Dirty` reason for each recompiled workspace crate.
2. **Given** that evidence, **When** the PR is opened, **Then** the PR description records the cause, the evidence it came from, and why it appears on `main` pushes but did not on #735's PR run or on `main` at `104f21c8`.

---

### User Story 2 - The FR-006 guard passes on `main` pushes without being weakened (Priority: P1)

A maintainer merging to `main` needs the required `test (ubuntu-latest)` job to be green on push events as it is on PR events, while the guard keeps detecting genuine unwanted recompilation.

**Why this priority**: `main` is red on a required check, which blocks trust in CI and the release flow.

**Independent Test**: Run the CI test job on a `main`-like ref and confirm the guard step passes; separately confirm the guard still fails when workspace code is deliberately made stale.

**Acceptance Scenarios**:

1. **Given** the fix, **When** the `test (ubuntu-latest)` job runs on the PR, **Then** it is green including the FR-006 step.
2. **Given** the fix, **When** the job runs in a context equivalent to a `main` push, **Then** the FR-006 step passes with zero `Compiling` lines for workspace crates.
3. **Given** the fix, **When** a workspace source file genuinely changes between the `build` and `test` jobs, **Then** the guard still fails.

---

### User Story 3 - Show that the `main` push will behave like the verified run (Priority: P2)

A reviewer needs confidence that a green PR run implies a green `main` push, given that the original failure appeared only on `main`.

**Why this priority**: The defect was invisible on PR CI. Without demonstrating the push context, a green PR proves little.

**Independent Test**: The PR description gives the reasoning for why the cause differs between push and PR contexts and why the fix covers both. Where the cause is push-specific, a `main`-like run (for example a manual dispatch on the branch, if the workflow supports it) is shown passing.

**Acceptance Scenarios**:

1. **Given** the cause is push-specific, **When** the PR is reviewed, **Then** it includes a passing run on a `main`-like ref, or a stated reason why none is possible and an equivalent argument.
2. **Given** the cause is not push-specific, **When** the PR is reviewed, **Then** it explains why the PR run previously passed and why the fix is nonetheless sufficient for pushes.

---

### Edge Cases

- The failure may be intermittent or ordering-dependent (the same guard passed before). A single green run is not enough evidence; the explanation must account for why the earlier runs passed.
- The cause may differ between `push` and `pull_request` events (merge ref versus branch head, different commit timestamps, cache-hit versus cache-miss for restored state).
- Fixing the symptom by loosening the guard (allow-listing the three crates, ignoring `Compiling` lines for path crates, or removing the step) would hide real regressions and is not acceptable.
- A fix that makes the guard pass on Linux must not alter behavior of the Release or Windows workflows.
- Re-running the failed job on the same commit may not exercise a fix; verification must use the fixed workflow.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The cause of the three-crate recompile on `main` after #735 MUST be identified using cargo's own fingerprint diagnostics (for example the `Dirty …` reasons from fingerprint logging), not by inference alone.
- **FR-002**: The identified cause, the evidence supporting it, and the explanation of why earlier runs (#735's PR run and `main` at `104f21c8`) passed MUST be recorded in the PR description.
- **FR-003**: After the fix, the "Assert no workspace code recompiled after restoring build-release's output (FR-006)" step MUST pass on `main` pushes as well as on pull requests.
- **FR-004**: The fix MUST NOT weaken the FR-006 guard: it MUST still fail when workspace code is recompiled for a genuine reason, and MUST NOT exclude `lcg-core`, `lcg-service`, `lcg-eval` or any other workspace crate from the check.
- **FR-005**: The `test (ubuntu-latest)` job MUST be green on the PR that carries the fix, including the FR-006 step.
- **FR-006**: The PR MUST include evidence or reasoning that the `main` push will behave like the verified PR run; if the cause is push-specific, a run on a `main`-like ref MUST be shown passing where the workflow allows it.
- **FR-007**: The fix MUST be confined to restoring correct behavior of the existing build/test freshness mechanism and MUST NOT restructure CI beyond what the cause requires.
- **FR-008**: The Release and Windows workflows MUST remain unchanged in behavior.
- **FR-009**: Merging the PR MUST close issue #736.

### Key Entities

- **FR-006 recompile guard**: the CI step that fails the test job if any workspace crate prints a `Compiling` line after the release output is restored.
- **Build-release artifact**: the `target/release` output produced by the `build` job and restored by the `test` job.
- **Fingerprint evidence**: cargo's per-unit reasons for considering a crate fresh or dirty, which is the authoritative source for the cause.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: The `test (ubuntu-latest)` job on the fix PR completes green with zero workspace-crate `Compiling` lines in the guard step.
- **SC-002**: The next push to `main` after merge completes the `test (ubuntu-latest)` job green, and consecutive `main` runs do not regress.
- **SC-003**: The PR description names a single evidenced cause backed by cargo fingerprint output, plus the reason it was absent from the earlier passing runs.
- **SC-004**: A deliberate change to a workspace source between the `build` and `test` jobs still causes the guard to fail (guard sensitivity preserved), demonstrated or argued from the unchanged guard logic.
- **SC-005**: The diff does not list `lcg-core`, `lcg-service` or `lcg-eval` as exceptions anywhere in the guard.

## Assumptions

- The failure is deterministic on `main` push contexts (two identical failures were observed), and a cause is discoverable from cargo's fingerprint logging.
- The cause lies in CI's timestamp/restore handling or its interaction with the commit, merge or push context, not in a toolchain or dependency change (registry crates do not recompile).
- #735's manifest change is a trigger or exposing condition for the failure rather than a functional change to compilation; the investigation should not assume it is irrelevant either.
- Workflow-dispatch on the branch may or may not be supported by the CI workflow; if it is not, an alternative `main`-like verification or a reasoned equivalence is acceptable, and adding a dispatch trigger is permitted only if the cause requires it.
- Local reproduction may not be possible; CI runs are the authoritative environment.

## Out of Scope *(optional)*

- Other CI restructuring beyond what the root cause requires.
- The Release and Windows workflows, which are green on `main`.
- Changing the FR-006 guard's strictness or scope.

## Source References *(optional)*

- Failing run: https://github.com/liminisapp/liminis-context-graph/actions/runs/43439700482 (two attempts, identical failure)
- Passing PR run: https://github.com/liminisapp/liminis-context-graph/actions/runs/43435957267
- #735 (merge `297e2d03`), `104f21c8` (last known-good `main`)
- #341, #657, ADR-0341 (the build/test split and the FR-006 guard)
