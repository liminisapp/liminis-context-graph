# Feature Specification: CI: stop running full release builds on every PR commit; verify packaging post-merge on main

**Feature Branch**: `fabrik/issue-639`
**Created**: 2026-09-30
**Status**: Specified
**Input**: User description: "CI: stop running full release builds on every PR commit; verify packaging post-merge on main instead (issue #639)"

## Background

`release.yml` runs a full four-target release build (`aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`) on **every PR commit**: it triggers on `pull_request` as well as on version tags, and `[workspace.metadata.dist]` sets `pr-run-mode = "upload"`, which makes the platform-build job run for PRs.

Nothing is published from a PR (`host`, `announce` and `dispatch-docs` skip), so this only re-checks packaging on every push. The cost is real. PR #638 had 5 pushes, each running 4 builds. Linux and macOS take 2–6 minutes, but `x86_64-pc-windows-msvc` takes 20–21 minutes every time. It is not a required check (only `test (ubuntu-latest)` is), but Fabrik's Validate CI gate (`fabrik:awaiting-ci`) waits on all checks, so the Windows release build paces every Fabrik PR.

The checks that only the release path runs are:

- the pinned extension-hash check per target (#593 / ADR-0593);
- the OpenSSL linkage guard on the shipped macOS and Windows binaries (ADR-0550);
- the Windows static-OpenSSL link (ADR-0581).

They protect **what ships**. Nothing ships from a PR commit, so running them once per merge to `main` is enough. The trade-off is accepted by the issue author: a PR that breaks packaging (a dependency bump or build-script change) is caught after merge rather than before, and is fixed forward before the next tag.

Two existing facts shape the work:

- `release.yml` is cargo-dist-generated but hand-maintained (`allow-dirty = ["ci"]`, plus the hand-added `dispatch-docs` job from #626). `pr-run-mode = "upload"` was deliberately chosen to exercise the release path before tags (see the comment in `Cargo.toml`); this issue replaces per-PR coverage with per-merge coverage.
- As generated, the workflow treats "not a pull request" as "publishing" and derives the tag from the ref name. A plain branch push to `main` would therefore be mistaken for a tag release unless that logic is changed.

`ci-failure-notify.yml` already turns a failed post-merge run of selected workflows into a `ci-failure` tracking issue, but it does not listen to `Release` (or `Windows`), so a broken packaging build on `main` would be invisible until the next tag.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - PRs no longer pay for release builds (Priority: P1)

A contributor (or a Fabrik agent) pushes commits to a PR branch. No `Release` workflow run is started, so the PR's checks finish without waiting on four-target release builds — in particular the ~20-minute Windows build.

**Why this priority**: This is the whole motivation — removing the per-push cost and the Fabrik Validate CI-gate delay.

**Independent Test**: Push a commit to a PR branch touching code-relevant paths; confirm the Actions tab shows no `Release` run for that commit and the PR's check list contains no `Release` checks.

**Acceptance Scenarios**:

1. **Given** an open PR, **When** a commit is pushed to its branch, **Then** no `Release` workflow run is created.
2. **Given** an open PR, **When** the PR is opened, reopened or synchronized, **Then** no `Release` workflow run is created and the PR's required check `test (ubuntu-latest)` is unaffected.

---

### User Story 2 - Packaging is verified once per merge to main, without publishing (Priority: P1)

When a PR merges (any push to `main`), a `Release` run builds all four targets and runs the full set of release-only checks — staging, pinned extension-hash verification, OpenSSL linkage guards — but publishes nothing.

**Why this priority**: This preserves the protection that per-PR builds gave (ADR-0398's and the macOS linkage history show that unverified release paths have shipped broken tags), at one run per merge rather than one per push.

**Independent Test**: Merge a change to `main`; confirm one `Release` run builds four targets, the pin and linkage steps execute, and no GitHub Release, docs publish or announcement is produced.

**Acceptance Scenarios**:

1. **Given** a commit lands on `main`, **When** the push event fires, **Then** a `Release` run starts and builds `aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` and `x86_64-pc-windows-msvc`.
2. **Given** that run, **When** each target build executes, **Then** OpenSSL staging, the extension-pin verification, and the OpenSSL linkage assertions run exactly as they do for a tag build.
3. **Given** that run, **When** it completes, **Then** `host`, `announce` and `dispatch-docs` are skipped: no GitHub Release is created, no announcement is made, and `docs-publish.yml` is not dispatched.

---

### User Story 3 - Tag releases behave exactly as before (Priority: P1)

A maintainer pushes a version tag. The workflow builds, checks and publishes the release, then dispatches the docs publish (#626), as today.

**Why this priority**: Shipping is the workflow's primary purpose; the change must not regress it.

**Independent Test**: Push a version tag (or inspect the workflow's computed plan for a tag ref) and confirm the tag, publishing flag, `host`, `announce` and `dispatch-docs` behave as before.

**Acceptance Scenarios**:

1. **Given** a pushed tag matching the version pattern, **When** the workflow runs, **Then** the planned tag equals the pushed tag, artifacts are built and checked, the GitHub Release is published, and `dispatch-docs` dispatches `docs-publish.yml` for that version.
2. **Given** a tag that does not match the version-tag scheme, **When** `dispatch-docs` runs, **Then** it still skips quietly as it does today.

---

### User Story 4 - A broken packaging build on main is visible (Priority: P2)

If the post-merge `Release` run on `main` fails, a `ci-failure` tracking issue is created (or an existing open one updated), so release-runbook step 0 (`gh issue list --label ci-failure --state open`) catches it before the next tag.

**Why this priority**: Without this, moving the check post-merge would trade a pre-merge signal for no signal at all — the exact failure mode #298 documented.

**Independent Test**: Cause (or simulate) a failed `Release` run on `main` and confirm the notify workflow files or updates a `ci-failure` + `workflow:release` issue; confirm a later green run resolves it per the listener's existing behavior.

**Acceptance Scenarios**:

1. **Given** a failed `Release` run whose head branch is `main`, **When** it completes, **Then** `ci-failure-notify.yml` creates or updates a tracking issue labelled `ci-failure` and `workflow:release`.
2. **Given** a cancelled or skipped `Release` run, **When** it completes, **Then** no issue is created or updated.
3. **Given** a `Release` run for a tag or for a pull request, **When** it completes, **Then** the listener's existing head-branch/event guard keeps it from filing a "main is broken" issue.

---

### User Story 5 - Maintainers and contributors are told where packaging is verified (Priority: P2)

`docs/release-process.md` and CONTRIBUTING's release runbook state that release packaging is verified post-merge on `main`, not per PR, and describe what to do about a red post-merge `Release` run before tagging. The header of `release.yml` documents which parts of this behavior are generator-honoured versus hand edits.

**Why this priority**: The behavior change is otherwise surprising (PRs silently lose a check), and the hand-edit boundary must be discoverable by whoever next runs `dist generate`.

**Independent Test**: Read the two docs and the `release.yml` header; each states the post-merge model, and the header names which settings are generator-honoured and which are hand-carried.

**Acceptance Scenarios**:

1. **Given** the updated docs, **When** a maintainer reads the release runbook, **Then** it says packaging is checked on `main` after merge and instructs them to check for an open `Release` `ci-failure` issue (and fix forward) before tagging.
2. **Given** the updated `release.yml`, **When** a maintainer opens it, **Then** its header states whether `dist generate` honours `pr-run-mode = "skip"` and the `push: branches: [main]` trigger, and which edits are hand-maintained.

---

### Edge Cases

- **Branch push mistaken for a release**: a push to `main` must not be planned as a tag release (no `--tag=main`, no `host --steps=create`, `publishing` not true) and must not reach `host`, `announce` or `dispatch-docs`.
- **Tag pushed to a commit that is also on `main`**: the tag run and the `main` push run are separate events; each behaves per its own ref (tag publishes, branch push does not).
- **Docs-only or non-code merges to `main`**: the `push` trigger has no path filter in the issue's proposal, so these merges also run the full four-target build (the `test` job, by contrast, skips for docs-only PRs). Accepted as-is; see Out of Scope.
- **Rapid successive merges**: each merge produces its own run; the notify listener's existing per-workflow serialization and reconcile-from-GitHub behavior handles fail→succeed→fail bursts.
- **Fork PRs / PR from a branch named `main`**: no `Release` run exists for PRs, so there is nothing for the listener to misattribute; its existing `event != 'pull_request'` guard remains.
- **Tag-run failures**: a failed tag-triggered `Release` run has a tag as its head branch, so the notify listener's `head_branch == 'main'` guard will not file an issue for it; tag failures remain visible to the maintainer who pushed the tag, as today.
- **Future `dist generate`**: regenerating `release.yml` must not silently reintroduce the `pull_request` trigger or drop the `push: branches: [main]` trigger and build-only handling (or, if it cannot be guaranteed, the file header documents the hand edits — see FR-009).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: `release.yml` MUST NOT trigger on `pull_request`; a push to a PR branch MUST NOT create a `Release` run.
- **FR-002**: `[workspace.metadata.dist]` MUST set `pr-run-mode = "skip"`, and the stale comment explaining why PR builds were enabled MUST be replaced with one explaining the post-merge model.
- **FR-003**: `release.yml` MUST trigger on `push` to `main` in addition to the existing version-tag push trigger.
- **FR-004**: A `Release` run triggered by a push to `main` MUST build all four targets (`aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`) and run every release-only step that a tag build runs: OpenSSL staging, extension staging and pinned-hash verification (#593), and the OpenSSL linkage assertions (ADR-0550, ADR-0581).
- **FR-005**: A `Release` run triggered by a push to `main` MUST NOT publish anything: `host`, `announce` and `dispatch-docs` MUST be skipped, no GitHub Release may be created, and `docs-publish.yml` MUST NOT be dispatched. The `if:` conditions of those jobs, and the plan step's tag/publishing derivation, MUST be verified to exclude a branch push — not merely assumed to, given that the generated logic equates "not a pull request" with "publishing".
- **FR-006**: A `Release` run triggered by a version tag MUST behave as it does today: plan with that tag, build, check, publish, and dispatch the docs publish (#626).
- **FR-007**: `ci-failure-notify.yml` MUST list `Release` among the workflows it watches, so a failed `Release` run on `main` creates or updates a `ci-failure` tracking issue (with its `workflow:release` label) under the listener's existing dedup, cancel/skip and resolve-on-success behavior.
- **FR-008**: `ci-failure-notify.yml` SHOULD also list `Windows` (`windows.yml`, which already runs on push to `main`), and its header comment listing the covered workflows MUST be updated to match whatever set is chosen.
- **FR-009**: The header of `release.yml` MUST document, based on actually checking `dist generate`/`dist plan` behavior with the pinned cargo-dist version (0.32.0), which of the `pr-run-mode = "skip"` handling and the `push: branches: [main]` trigger are produced by the generator and which are hand edits preserved only by `allow-dirty = ["ci"]`.
- **FR-010**: `docs/release-process.md` and CONTRIBUTING's release runbook MUST state that release packaging is verified post-merge on `main`, not per PR, and MUST direct maintainers to treat an open `Release` `ci-failure` issue as a blocker (or explicitly justified exception) before tagging. The runbook's step 0 list of non-gating workflows and the step describing the release workflow's behavior MUST be updated accordingly, and the PR-time-coverage wording in any affected ADR/doc that claims release builds run on PRs MUST be corrected.
- **FR-011**: No other workflow's triggers, required checks (`test (ubuntu-latest)`), or branch-protection settings change as a result of this work.

### Key Entities

- **Release workflow (`release.yml`)**: the cargo-dist-derived, hand-maintained workflow that plans, builds four targets, and (tag-only) publishes.
- **dist configuration (`[workspace.metadata.dist]`)**: source of `pr-run-mode` and `allow-dirty`, which determine what `dist generate` emits and preserves.
- **CI Failure Notify (`ci-failure-notify.yml`)**: `workflow_run` listener that converts failed post-merge runs on `main` into labelled `ci-failure` issues.
- **Release runbook**: the maintainer checklist in CONTRIBUTING and `docs/release-process.md`, whose step 0 consults open `ci-failure` issues.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A push to a PR branch starts zero `Release` workflow runs (previously four platform builds per push, including a ~20-minute Windows build).
- **SC-002**: A merge to `main` starts exactly one `Release` run that builds all four targets and executes the pin and linkage checks, and produces no GitHub Release, no docs dispatch and no announcement.
- **SC-003**: A pushed version tag still builds, checks and publishes a release and dispatches the docs publish, with results indistinguishable from the pre-change workflow.
- **SC-004**: A failed `Release` run on `main` results in an open `ci-failure` issue labelled `workflow:release` (created or updated).
- **SC-005**: The Fabrik Validate CI gate on a PR no longer waits on any Release build; the PR's remaining checks are the only ones gating.
- **SC-006**: The release runbook and `docs/release-process.md` describe post-merge packaging verification, and `release.yml`'s header records which behavior is generator-honoured versus hand-edited.

## Assumptions

- The `push: branches: [main]` trigger applies to every merge, without path filters; docs-only merges also run the four-target build. (Filtering, or a scheduled/dispatch-only alternative, is out of scope.)
- Some hand editing of `release.yml` is unavoidable (at minimum, making a branch push build-only rather than publishing); `allow-dirty = ["ci"]` already permits this. Whether `dist generate` additionally honours `pr-run-mode = "skip"` and a custom `push` trigger is to be verified during implementation and documented (FR-009) rather than assumed.
- Adding `Windows` to the failure listener is included because it already runs on push to `main` and has the same visibility gap; it is a low-cost addition and can be dropped without affecting the other requirements.
- The notify listener's existing `head_branch == 'main' && event != 'pull_request'` guard is sufficient for a `Release` push-to-main run; tag-run failures are intentionally left to the maintainer who pushed the tag.
- Fixing forward a broken `main` packaging build before the next tag is the accepted remediation; no automatic revert or merge-blocking is introduced.

## Out of Scope

- Path-filtering the `main` push trigger, nightly/scheduled packaging builds, or making the `Release` build a required check.
- Changing `windows.yml`'s own triggers or the required `test (ubuntu-latest)` check.
- Speeding up the Windows build itself (caching, prebuilt OpenSSL, etc.).
- Notifying on failed tag-triggered `Release` runs.
- Any change to what a tag release publishes, including the docs publish mechanism from #626.
- Reworking branch protection.

## Source References

- `.github/workflows/release.yml` (plan job outputs, `build-local-artifacts`, `host`, `announce`, `dispatch-docs` conditions)
- `Cargo.toml` — `[workspace.metadata.dist]` (`pr-run-mode`, `allow-dirty`)
- `.github/workflows/ci-failure-notify.yml`, `.github/workflows/windows.yml`
- `docs/release-process.md`, `CONTRIBUTING.md` (Release runbook, step 0)
- ADR-0298 (CI failure notification), ADR-0550 (OpenSSL dynamic linkage), ADR-0581 (Windows static OpenSSL), ADR-0593 (pin lbug extension bytes)
- Issues #298, #593, #626; PR #638
