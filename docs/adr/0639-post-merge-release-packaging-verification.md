# ADR-0639: Verify Release Packaging Post-Merge on `main`, Not on Every PR Push

**Status:** Accepted
**Date:** 2026-09-30
**Issue:** #639

## Context

`release.yml` ran a full four-target release build (`aarch64-apple-darwin`,
`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`) on **every PR
commit**: it triggered on `pull_request`, and `pr-run-mode = "upload"` in
`[workspace.metadata.dist]` made `build-local-artifacts` run for PRs. Nothing is published from a
PR (`host`, `announce` and `dispatch-docs` skip), so this only re-checked packaging on every push —
at the cost of a ~20-minute `x86_64-pc-windows-msvc` build each time. Only `test (ubuntu-latest)`
is a required check, but Fabrik's Validate CI gate waits on every check, so the Windows build paced
every Fabrik PR.

The checks only the release path runs protect **what ships**: the pinned extension-hash check
([ADR-0593](0593-pin-lbug-extension-bytes.md)), the OpenSSL linkage guard
([ADR-0550](0550-openssl-dynamic-linkage-via-rpath.md)) and the Windows static-OpenSSL link
([ADR-0581](0581-windows-static-openssl.md)). Nothing ships from a PR commit, so one run per merge
to `main` is enough.

## Decision

1. `release.yml` no longer triggers on `pull_request` (`pr-run-mode = "skip"`).
2. `release.yml` triggers on `push` to `main` as well as on version tags. A `main` push builds all
   four targets and runs every release-only step, but **publishes nothing**.
3. Publishing is keyed on `github.ref_type == 'tag'` (plan outputs `tag`, `tag-flag`,
   `publishing`, and the `plan` step's `host` vs `plan` choice). The cargo-dist-generated logic
   equates "not a pull request" with "publishing", which would plan a branch push as
   `--tag=main`. `host`, `announce` and `dispatch-docs` already gate on `publishing == 'true'`
   (`announce` via `host.result`), so they exclude a branch push with no further edit.
4. `build-local-artifacts.if` builds on `publishing == 'true' || github.event_name == 'push'`.
   Left as generated under `skip`, it would build nothing on a `main` push and the run would go
   green having verified nothing.
5. `ci-failure-notify.yml` watches `Release` and `Windows`, so a failed run on `main` files a
   `ci-failure` issue (`workflow:release` / `workflow:windows`). The listener's existing
   `head_branch == 'main' && event != 'pull_request'` guard is unchanged. A failed tag-triggered
   run has the tag as its head branch and is intentionally not reported — the maintainer who
   pushed the tag sees it.
6. The release runbook treats an open `workflow:release` `ci-failure` issue as a blocker before
   tagging (or an explicitly justified exception recorded in the release PR).

### Hand-edit boundary

Checked against cargo-dist 0.32.0 (`dist generate --mode=ci` with `allow-dirty` removed):

- **Generator-honoured:** `pr-run-mode = "skip"` removes `pull_request:` from `on:`.
- **Hand edits, preserved only by `allow-dirty = ["ci"]`:** the `push: branches: [main]` trigger,
  the tag-based publishing predicate, the `build-local-artifacts.if`, the `dispatch-docs` job
  ([#626](https://github.com/verveguy/liminis-context-graph/issues/626)) and the OpenSSL /
  extension-pin / linkage steps. Removing `allow-dirty` and running `dist generate` would
  silently undo them. `dist plan` with `skip` still returns the full four-target matrix.

The `release.yml` header records the same boundary.

## Consequences

- PR checks no longer wait on release builds; the Fabrik Validate gate is paced by the remaining
  checks only.
- **A PR that breaks packaging (a dependency bump, a build-script change) is caught after merge,
  not before**, and is fixed forward before the next tag. This trade-off was accepted by the issue
  author. Linux keeps per-PR coverage of the pin and OpenSSL checks via `ci.yml`, and Windows
  keeps path-filtered per-PR coverage via `windows.yml`; macOS and aarch64 Linux are now covered
  only post-merge.
- Every merge to `main`, including docs-only ones, runs the four-target build (no path filter).
- **`Windows` failure coverage on `main` is best-effort under bursts.** `windows.yml` has
  `concurrency: cancel-in-progress: true` keyed on the ref, so two quick merges share the group
  `windows-refs/heads/main` and the second cancels the first mid-build. The listener ignores
  cancelled runs, so a merge whose Windows build was cancelled and whose successor then passes is
  never reported, even if it would have failed. `Release` has no `concurrency:` block and does not
  have this gap, and it covers the Windows target's packaging on every merge. Changing
  `windows.yml` is out of scope here (FR-011); making `cancel-in-progress` false on `main` is a
  possible follow-up.
- The new behaviour cannot be exercised locally; the first `Release` run on `main` after merge is
  the real test (four builds, pin and linkage steps executed, `host`/`announce`/`dispatch-docs`
  skipped).

## Alternatives Considered

- **Keep `pr-run-mode = "upload"` and only drop the trigger.** Avoids the `if:` edit but
  contradicts the goal and leaves a misleading setting.
- **A `concurrency:` group cancelling superseded `main` runs.** Would leave a merge commit
  unverified, and the listener ignores cancelled runs, so the gap would be silent. Rapid merges
  instead queue as independent runs.
- **Path-filtering the `main` push trigger, nightly builds, or making `Release` a required
  check.** Out of scope for this change.
