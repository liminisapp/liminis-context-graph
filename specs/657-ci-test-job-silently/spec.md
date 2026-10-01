# Feature Specification: CI `test` job reuses the `build-release` build, and the recompile guard actually works

**Feature Branch**: `fabrik/issue-657`
**Created**: 2026-10-01
**Status**: Specified
**Input**: User description: "CI: test job silently recompiles the entire workspace (~9 min/run) — hand-off from build-release broken, and the FR-006 guard is blind to ANSI-colored cargo output"

## Background

The CI design (#341, ADR-0341, FR-001/FR-006) is that `build (release profile)` (job id `build-release`) compiles the workspace once and hands `target/release` to `test (ubuntu-latest)` as a same-run artifact; `test` then *runs* the tests without recompiling. That hand-off is silently broken, and the guard that should have caught it cannot fire.

Evidence from CI run 36911444513 on `main` (2026-10-01, a push):

| job | step | time |
|---|---|---|
| build (release profile) | `cargo build --release` | 127 s |
| build (release profile) | `cargo test --release --no-run` (all test binaries) | 490 s |
| build (release profile) | upload full release build | 160 s |
| test (ubuntu-latest) | download full release build | 63 s |
| test (ubuntu-latest) | `cargo test --release` (restored build) | **661 s** |

In the `test` step cargo printed **233 `Compiling` lines**, starting from `proc-macro2` and `libc`, then `Finished release profile ... in 8m 46s`. The tests themselves take about 2 minutes (slowest binary, `ipc_parity`, 15.5 s). About **9 minutes of every CI run's critical path is a duplicate full compile**.

**Why the guard did not fire.** `ci.yml` greps the captured log with `grep -q "Compiling " /tmp/build.log`. With coloured cargo output, cargo writes `ESC[1mESC[92m   CompilingESC[0m proc-macro2` — a colour-reset sequence sits *between* `Compiling` and the space — so the pattern never matches real output. In that run the only 3 matches were the guard's own script text echoed by the runner, and the step printed `OK: full cache hit confirmed, no recompilation`. The same pattern is used by all six e2e jobs. Those run prebuilt binaries, so their guards are vacuous rather than wrong, but they share the defect.

A guard that cannot fail is worse than no guard: it reported a green "full cache hit" on a run that did the opposite. The guard must therefore be repaired *first*, so the reuse fix is measurable and provably red-before / green-after.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - The recompile guard detects real recompilation (Priority: P1)

A maintainer relies on the "no Compiling lines after restoring build-release's output" check to know the hand-off works. The check must match cargo's `Compiling` output regardless of colour settings, so it fails when `test` recompiles.

**Why this priority**: Without a working guard, neither the bug nor its fix is observable. Everything else depends on this.

**Independent Test**: Run the corrected guard against a captured log containing coloured cargo output (including `ESC[0m` between `Compiling` and the space) and against an uncoloured one; both must fail. Run it against a log with no `Compiling` lines; it must pass. Then run it in CI on current `main` behaviour (before the reuse fix) and observe it fail.

**Acceptance Scenarios**:

1. **Given** a log containing `ESC[1mESC[92m   CompilingESC[0m proc-macro2`, **When** the guard runs, **Then** it exits non-zero and prints the offending lines.
2. **Given** a log containing plain `   Compiling proc-macro2 v1.0.0`, **When** the guard runs, **Then** it exits non-zero.
3. **Given** a log containing only `Finished` and test-result lines, **When** the guard runs, **Then** it exits zero.
4. **Given** the guard deployed on a branch that still has the broken hand-off, **When** CI runs, **Then** the `test` job fails at the guard step (red run kept as proof in the PR).

---

### User Story 2 - `test` reuses the restored `target/release` without recompiling (Priority: P1)

A contributor pushing to a PR expects the `test` job to spend its time running tests, not rebuilding ~233 crates that `build-release` just built.

**Why this priority**: This is the ~9-minute critical-path saving that motivates the issue.

**Independent Test**: In a CI run, the `cargo test --release` step of `test` prints zero `Compiling` lines for workspace or dependency crates, and its duration is roughly test execution time.

**Acceptance Scenarios**:

1. **Given** `build-release` has uploaded its release build for a commit, **When** `test` downloads it and runs `cargo test --release`, **Then** the output contains no `Compiling` lines and the corrected guard passes.
2. **Given** the fix, **When** `test`'s `cargo test --release` step completes, **Then** its duration is about 2–3 minutes rather than about 11.
3. **Given** the fix, **When** the same tests, clippy, R-003 bench gate and e2e jobs run, **Then** each runs with the same coverage as before (nothing dropped to gain speed).

---

### User Story 3 - The six e2e guards use the same colour-safe check (Priority: P2)

The six e2e jobs in `ci.yml` carry the same blind `grep "Compiling "`. They should use the same repaired check so the pattern is consistent and a future accidental `cargo` invocation in those jobs is caught.

**Why this priority**: Currently vacuous, not producing wrong results, but a latent trap and an inconsistency.

**Independent Test**: Each e2e guard step, fed a coloured `Compiling` log, fails; fed a clean log, passes.

**Acceptance Scenarios**:

1. **Given** any of the six e2e jobs' captured log contains a coloured `Compiling` line, **When** its guard runs, **Then** the job fails.
2. **Given** a normal e2e run that only executes prebuilt binaries, **When** the guard runs, **Then** it passes.

---

### User Story 4 - The mechanism and the colour pitfall are documented (Priority: P2)

A future maintainer debugging a recompile regression should find the actual reason the hand-off failed and the ANSI pitfall written down, in ADR-0341 and in the `ci.yml` comments, rather than the current text that describes the intended behaviour as if it worked.

**Why this priority**: The existing comments assert the mtime normalisation makes the restore a "pure cache hit"; that was never verified by a working guard.

**Independent Test**: Read ADR-0341 and the comments at the guard and mtime-normalisation steps; they state the real root cause and mention that colour codes defeat a naive `grep "Compiling "`.

**Acceptance Scenarios**:

1. **Given** the PR is merged, **When** a reader opens ADR-0341, **Then** it describes the actual cause of the missed cache reuse, the fix, and the ANSI colour pitfall (including that cargo can emit a reset between `Compiling` and the space).

---

### Edge Cases

- Cargo output with colour disabled (`CARGO_TERM_COLOR=never`, or non-TTY without `always`): the guard must still work.
- `Compiling` appearing legitimately in a non-cargo context (for example, echoed script text or a test's own stdout): the guard should match cargo's status-line shape (leading whitespace then `Compiling`, after ANSI stripping) rather than any occurrence, so it neither misses real output nor false-fires on echoed text.
- A genuine cold-cache or cache-bust run where `build-release` itself compiles: the guard applies only to jobs that are supposed to be consuming a prebuilt tree, so this is unaffected.
- Docs-only runs where `build-release`/`test` are skipped: unchanged.
- `workflow_dispatch` with `e2e_only`: `test` is skipped; unchanged.
- Cargo may legitimately print `Fresh`/`Finished`/`Running` lines; these must not trip the guard.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The recompile guard in the `test` job MUST detect `Compiling` status lines in cargo output independent of ANSI colour codes (including a colour-reset sequence between `Compiling` and the following whitespace), either by stripping ANSI sequences before matching or by running the guarded cargo invocation with colour disabled.
- **FR-002**: The guard MUST NOT depend on a trailing space after `Compiling`.
- **FR-003**: The guard MUST NOT false-match on the guard's own echoed script text.
- **FR-004**: The same colour-safe check MUST be applied to all six e2e jobs' guards in `ci.yml`.
- **FR-005**: The corrected guard MUST fail on current `main` behaviour (before the reuse fix); a red CI run, or an equivalent demonstration, MUST be included in the PR as proof.
- **FR-006**: The root cause of `test` recompiling the restored `target/release` MUST be identified with evidence (e.g. cargo's fingerprint logging, `CARGO_LOG=cargo::core::compiler::fingerprint=info`, showing why units are dirty), and the cause stated in the PR/ADR. Candidate causes to be ruled in or out: environment or `RUSTFLAGS` differences between the jobs; differing toolchain resolution; differing `CARGO_HOME`/registry paths embedded in fingerprints; mtime normalisation not matching what `build-release` saw or not covering build-script inputs; artifact upload/download not preserving file attributes (e.g. mtimes) the fingerprints depend on.
- **FR-007**: After the fix, `test`'s `cargo test --release` step MUST print zero `Compiling` lines for workspace or dependency crates.
- **FR-008**: The fix MUST NOT change what CI verifies: the same tests, clippy, R-003 bench gate, OpenSSL linkage assertion and e2e coverage MUST still run.
- **FR-009**: The fix MUST NOT regress the #341 properties: `build-release` remains the only full-workspace compile and only writer of the `lbug-cache-*` key; e2e jobs still transfer only their six binaries.
- **FR-010**: ADR-0341 and the relevant `ci.yml` comments MUST be updated to describe the actual mechanism of cross-job reuse and the ANSI-colour pitfall. Any documentation claiming the restore is a "pure cache hit" by virtue of the mtime normalisation alone MUST be corrected if the investigation shows otherwise.
- **FR-011**: Any diagnostic step added to investigate (e.g. fingerprint logging) MUST either be removed before merge or be inexpensive and clearly non-blocking.

### Key Entities

- **`release-build` artifact**: The same-run artifact containing `target/release/`, produced by `build-release` and consumed by `test`.
- **Recompile guard**: The log-grep step asserting no `Compiling` lines appear in a job that is supposed to consume prebuilt output (one in `test`, six in the e2e jobs).
- **Cargo fingerprint**: Per-unit freshness record under `target/release/.fingerprint` that determines whether a unit is rebuilt.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: The corrected guard fails on a CI run reproducing today's recompiling behaviour, and passes once reuse is fixed; evidence of the failing run is linked from the PR.
- **SC-002**: In a CI run after the fix, `test`'s `cargo test --release` log contains zero `Compiling` lines for workspace or dependency crates.
- **SC-003**: `test`'s `cargo test --release` step duration falls from about 11 minutes to about 2–3 minutes (roughly test execution time), cutting about 9 minutes from the CI critical path.
- **SC-004**: All six e2e guard steps use the colour-safe check; a synthetic coloured-`Compiling` log makes each fail.
- **SC-005**: The set of CI checks that run (test suite, clippy, R-003 gate, e2e jobs, linkage assertions) is unchanged versus before the PR.
- **SC-006**: ADR-0341 and the `ci.yml` comments state the actual mechanism and the colour pitfall.

## Assumptions

- Run 36911444513 is representative: the recompile happens on every code-touching run, not intermittently.
- `ci.yml` in this checkout does not itself set `CARGO_TERM_COLOR`; the issue reports coloured output in CI, so colour is being forced from some other source (runner/org environment, or a setting not visible in the checked-in workflow). The Research stage should confirm where it comes from, but the guard MUST be correct regardless of its source.
- Cargo's `Compiling` status line is the correct signal for "this job built something"; `Fresh` lines (only emitted under verbose mode) are not in scope.
- The fix may be located in workflow YAML, scripts, or cache/artifact handling; the issue does not prescribe which. Whether the root cause is a single fingerprint input or several is unknown until diagnosed.
- A temporary red demonstration run (guard deployed before the fix) is acceptable on the feature branch's CI history.

## Out of Scope

- Reducing `build-release`'s own compile time or the 160 s artifact upload / 63 s download (follow-up if desired).
- Replacing the artifact hand-off with `actions/cache` or another mechanism, unless the diagnosis shows the current mechanism cannot preserve fingerprints (then a change of mechanism is in scope as the fix).
- Changing what tests, clippy checks, bench gates or e2e suites run.
- `windows.yml` (it sets `CARGO_TERM_COLOR: never` and has no such guard) and `release.yml`.
- Altering the #341 design goals (single compile, single lbug-cache writer, small e2e artifact).

## Source References

- `.github/workflows/ci.yml` — `build-release` and `test` jobs, mtime-normalisation steps, FR-006 guard step, six e2e guard steps.
- `docs/adr/0341-build-release-artifacts-once.md`
- `specs/341-ci-build-release-artifacts/spec.md` (FR-001, FR-006)
- `docs/adr/0640-ci-cheap-checks-first-and-fail-fast.md`
- CI run 36911444513 (evidence)
