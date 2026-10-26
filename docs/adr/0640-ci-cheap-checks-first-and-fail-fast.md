# ADR-0640: CI Cheap-Checks-First Pre-Gate and Run-Wide Fail-Fast

**Status**: Accepted
**Date**: 2026-09-30
**Issue**: #640

## Context

`ci.yml` ran its cheapest check last and had no fail-fast:

- `cargo fmt --check` was the final step of `test`, after the ~14-minute release build, the
  ~12-minute `cargo test --release`, the R-003 bench and clippy. A formatting slip was reported
  only after about 15 minutes. The eval script guards and the ML-runtime-deps `cargo tree` guard
  also sat behind the build although neither compiles anything.
- `test` and the six real-corpus e2e jobs depend only on `[changes, build-release]`, so they ran
  independently. A failure in one did not stop the others.

A PR cannot merge while any required check fails, so every minute after the first real failure was
wasted runner time and also held up Fabrik's all-checks CI gate.

## Decision

1. **A `lint` pre-gate job** (`lint (fmt + guards)`) runs `cargo fmt --all --check`,
   `crates/eval/scripts/test-scripts.sh` and the `cargo tree` ML-deps guard. It compiles nothing
   (`cargo tree` resolves metadata only, so lbug's `build.rs` never runs) and needs no lbug cache,
   OpenSSL staging or extension bundle. The three steps were **removed** from `test`; each runs
   exactly once.
2. **`build-release` is gated on `lint`** (`needs: [changes, lint]`, `needs.lint.result ==
   'success'`). `lint` carries the same fail-safe `if:` as the build (ADR-0322), so a docs-only run
   skips both from one condition. Downstream jobs are unchanged: a failed `lint` skips the build,
   and `needs.build-release.result == 'success'` cascades the skip.
3. **Display name only is renamed**: `build release artifacts` → `build (release profile)`. The job
   id `build-release` stays, so the `needs:` lists and `needs.build-release.result` expressions
   (and the ADRs that cite the id) remain valid. `test (ubuntu-latest)` is unchanged.
4. **Fail-fast on `test` and the six e2e jobs.** Each ends with a step that runs
   `gh run cancel "$GITHUB_RUN_ID"` under `if: failure()` with `continue-on-error: true`.
   - `failure()` is false when the job itself is cancelled, so cancellations do not cascade.
   - It is best-effort: fork PRs get a read-only token (403) and a second simultaneous failure hits
     an already-cancelling run (409). Neither changes the job's original failed conclusion.
   - The step is inlined in each job because the e2e jobs do no checkout, so a local composite
     action would not resolve.
   - These seven jobs get a job-level `permissions: {contents: read, actions: write}` (a job-level
     block replaces the workflow-level one, so both keys are required). The workflow default stays
     `contents: read`.
   - The jobs stay parallel; no `needs:` edge is added between `test` and the e2e jobs. In practice
     the e2e jobs finish first, so the saving is mostly an e2e failure cancelling `test` (~12 min).
5. **No cancel step in `build-release`.** `changes` and `lint` are already done, so it has no
   in-flight sibling to stop, and a cancel could only turn the run's `failure` into `cancelled`.
   This departs from the issue's literal FR-006 text deliberately.
6. **clippy stays in `test`** (`--release -- -D warnings`, ~14 s against the restored build). A
   debug `--all-targets` run in `lint` would be a cold full-workspace compile that either lengthens
   the critical path or adds cost for no earlier signal. It still runs exactly once.
7. **The failure notifier treats a `cancelled` run with a real failure as actionable**
   (`ci-failure-notify.yml`, amends ADR-0298). A `gate` step computes `actionable`: `success` →
   true (still resolves an open issue); `skipped` → false; `cancelled` → true only if the jobs API
   shows a job that failed/timed out or holds a step with conclusion `failure`; anything else →
   true. A run cancelled purely by a superseding push has neither and stays a no-op. The
   failing-job filter excludes `cancelled` jobs unless they hold a failed step, so the issue and
   its `--log-failed` excerpt name the real cause, not a cancelled sibling.
8. **The `cargo tree` guard is hardened**: `cargo tree` output is captured on its own with stderr
   kept, so a registry failure fails the step. Previously `2>/dev/null` plus a trailing `|| true`
   made an empty result read as "OK".

## Consequences

- A formatting or guard failure is reported by `lint` in about a minute and no compile job starts.
  A green run costs roughly one extra minute of wall-clock (runner start, checkout, toolchain,
  cold `cargo tree`).
- **Skipped-required-check trade-off.** `test (ubuntu-latest)` is the only required check. When
  `lint` fails, `test` is *skipped*, and GitHub counts a skipped required check as passing. A
  `build-release` failure already behaved this way before this change. What prevents the merge is
  the red overall run and Fabrik's all-checks gate, not branch protection. **Recommended manual
  follow-up (a repo setting, not a file):** add `lint (fmt + guards)` to the required status
  checks. Note that on docs-only PRs it is skipped, which also counts as passing.
- **Cancel semantics were not verified empirically.** It is not established from documentation
  what conclusion the job that issues the cancel, or the run, ends with, or what `workflow_run`
  reports. The design tolerates either: the notifier keys off step-level `failure`, which survives
  both a `failure` and a `cancelled` job conclusion. To verify, run `workflow_dispatch` on a
  scratch branch with a deliberately failing test, then inspect
  `gh api repos/<owner>/<repo>/actions/runs/<id>/jobs` (job and step `conclusion`s) and the run's
  `conclusion`.
- Fail-fast applies on `main` too, so a failing post-merge run may end `cancelled`; decision 7 is
  what keeps `ci-failure` issues filed.
- `windows.yml` is a separate workflow that CI cannot cancel; it is out of scope.

## References

- #640 — this change; #341 / ADR-0341 (single build job), #322 / ADR-0322 (docs-only fast path),
  #298 / ADR-0298 (failure notifier), #430 / ADR-0430 (`pipefail` shell default).
- `.github/workflows/ci.yml`, `.github/workflows/ci-failure-notify.yml`
