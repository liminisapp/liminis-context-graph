# Feature Specification: Docs site auto-publishes on release

**Feature Branch**: `fabrik/issue-626`
**Created**: 2026-09-29
**Status**: Specified
**Input**: User description: "Docs site never auto-publishes: `release: published` trigger doesn't fire for GITHUB_TOKEN-created releases"

## Background

`docs-publish.yml` is designed to run on `release: published` (ADR-0477, `docs/release-process.md` § Docs publishing, CONTRIBUTING runbook step 6). **That trigger has never fired.** Every run of the workflow has been a manual `workflow_dispatch`, so the docs site is only updated when someone remembers to dispatch it by hand.

Evidence: `gh run list -w "Docs publish"` shows six runs total, all `workflow_dispatch` (five on 2026-08-24, one on 2026-09-30 dispatched by hand for v0.16.0). Releases v0.14.0, v0.14.1, v0.14.2, v0.14.3 and v0.15.0 never had docs published; v0.16.0 only by manual dispatch. The live site (http://v3rv.com/liminis-context-graph/) served v0.13.x-era docs through five releases and nobody noticed, because the release workflow is green either way.

Cause: `release.yml`'s `announce` job creates the GitHub Release with `gh release create` using `GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}`. GitHub deliberately does not start new workflow runs from events created with `GITHUB_TOKEN` (exceptions: `workflow_dispatch` and `repository_dispatch`). The `release: published` event exists, but `docs-publish.yml` never sees it.

Options considered in the issue: (1) dispatch `docs-publish.yml` from `release.yml` after the release is created (recommended — `workflow_dispatch` is permitted from `GITHUB_TOKEN` given `actions: write`, and `release.yml` is already hand-maintained via `allow-dirty = ["ci"]`; cargo-dist's `post-announce-jobs` is an alternative route); (2) create the release with a PAT/GitHub App token (adds a secret for no other benefit); (3) make it a runbook step (rejected — the same "someone has to remember" failure). The `release: published` trigger must be kept for releases created by hand.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Normal tag release publishes docs automatically (Priority: P1)

A maintainer pushes a `vX.Y.Z` tag. The release workflow builds artifacts and publishes the GitHub Release. Without any further action, the docs site is rebuilt from that tag and the new version appears at its versioned path and (if it is the latest stable) at the site root.

**Why this priority**: This is the entire defect — five releases shipped with stale docs.

**Independent Test**: Push a pre-release tag (e.g. `vX.Y.Z-rc.1`) or cut the next real release, then confirm a `Docs publish` run exists that was started by the release flow (not by a human) and that it succeeds.

**Acceptance Scenarios**:

1. **Given** a `vX.Y.Z` tag is pushed and the release workflow's artifact builds succeed, **When** the GitHub Release is published by `release.yml`, **Then** a `Docs publish` run for version `X.Y.Z` starts automatically and, on success, the site serves `X.Y.Z` docs at `/liminis-context-graph/vX.Y.Z/` and at the root when it is the latest stable release.
2. **Given** a pre-release tag (`vX.Y.Z-rc.N`), **When** its release is published, **Then** docs are published at the versioned path only and are not promoted to the site root (existing ADR-0477 behaviour is unchanged).
3. **Given** the release workflow fails before the release is created, **When** no release is published, **Then** no docs publish is triggered.

---

### User Story 2 - Hand-created releases still publish docs (Priority: P2)

A maintainer creates a GitHub Release by hand (UI or a non-`GITHUB_TOKEN` credential). The existing `release: published` trigger still fires and publishes docs.

**Why this priority**: Preserves the existing fallback path.

**Independent Test**: Inspect the workflow triggers; the `release: published` trigger and its tag-shape filter remain.

**Acceptance Scenarios**:

1. **Given** a release with a `vX.Y.Z` tag is published by a user, **When** the event fires, **Then** `docs-publish.yml` runs as before.
2. **Given** a non-version release (e.g. `eval-artifacts-2026-07`), **When** published, **Then** docs publishing is skipped (FR-009 of ADR-0477 unchanged).

---

### User Story 3 - Documentation describes the real mechanism (Priority: P2)

A maintainer reading the release docs learns what actually triggers docs publishing, and why the `release: published` event alone is insufficient for releases created by `release.yml`.

**Why this priority**: The docs currently assert a mechanism that never worked, which is how this went unnoticed.

**Independent Test**: Read `docs/release-process.md` § Docs publishing, ADR-0477, and CONTRIBUTING step 6; each names the dispatch-from-`release.yml` mechanism and the `GITHUB_TOKEN` limitation.

**Acceptance Scenarios**:

1. **Given** the updated docs, **When** a maintainer reads the "What happens automatically when you cut a release" section, **Then** it states that `release.yml` dispatches `docs-publish.yml`, and that `release: published` only fires for releases created outside `GITHUB_TOKEN`.

---

### Edge Cases

- A duplicate run if both the dispatch and the `release: published` event fire for the same release (e.g. release created by a PAT in future): must be harmless — the workflow's queuing concurrency group and fresh "latest stable" recomputation already make republishing idempotent.
- The dispatch is issued when the release exists but the tag's docs build fails: the `Docs publish` run goes red; the release itself must not be affected.
- The dispatch step fails (e.g. missing `actions: write`): this should be visible in the release workflow rather than silently swallowed, without invalidating an already-published release.
- Non-version tags or tags that `docs-publish.yml` rejects: the dispatch path must not cause a red run for cases the `release: published` path would skip quietly.
- Release workflow runs on `pull_request` (plan-only): must not dispatch docs publishing.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: When `release.yml` publishes a GitHub Release for a `vX.Y.Z[-pre]` tag, it MUST cause `docs-publish.yml` to run for that version without any manual step, using a mechanism permitted for `GITHUB_TOKEN`-initiated actions.
- **FR-002**: The docs publish MUST run only after the GitHub Release has been created, and MUST NOT run when the release workflow did not publish a release (including `pull_request` plan-only runs).
- **FR-003**: `docs-publish.yml` MUST retain its `release: published` trigger and its version-tag filter so that hand-created releases continue to publish docs.
- **FR-004**: The dispatched run MUST pass the release's version (without leading `v`) so the workflow builds from that release's tag; pre-release versions MUST keep being published to their versioned path only.
- **FR-005**: A failure to dispatch MUST be visible in the release workflow run, and MUST NOT retroactively fail or roll back the published release artifacts.
- **FR-006**: `docs/release-process.md` (§ Docs publishing), ADR-0477, and CONTRIBUTING runbook step 6 MUST describe the mechanism that actually runs and MUST note the `GITHUB_TOKEN` limitation (events created by `GITHUB_TOKEN` do not trigger workflows, except `workflow_dispatch`/`repository_dispatch`) and that the `release: published` trigger is retained for hand-created releases.
- **FR-007**: The post-release verification steps in `docs/release-process.md` MUST remain accurate for the new mechanism (e.g. the check that the latest `Docs publish` run succeeded).

### Key Entities

- **GitHub Release**: created by `release.yml`'s `announce`/`host` flow via `gh release create` with `GITHUB_TOKEN`.
- **Docs publish run**: an execution of `docs-publish.yml`, starting from either a `release` event or a `workflow_dispatch` with `version`.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: For the next release (or a pre-release tag used for verification) cut through the normal tag flow, a `Docs publish` run starts within minutes of the release being published with zero human actions, and the site shows the new version.
- **SC-002**: `gh run list -w "Docs publish"` shows a run for that release whose trigger was the release flow's dispatch, not a human.
- **SC-003**: No document in the repo (`docs/release-process.md`, ADR-0477, CONTRIBUTING) claims that `release: published` fires for releases created by `release.yml`.
- **SC-004**: Hand-created releases and non-version releases behave exactly as before (publishes / quiet skip respectively).

## Assumptions

- The dispatch-from-`release.yml` approach (option 1) is used; PAT/App-token (option 2) and runbook-only (option 3) are rejected. Whether it is implemented as a step/job in `release.yml` or via cargo-dist `post-announce-jobs` is an implementation choice for Plan, provided FR-001–FR-005 hold.
- `release.yml` continues to be hand-maintained under `allow-dirty = ["ci"]`; a hand edit is acceptable.
- Backfilling versioned docs for 0.14.0 through 0.15.0 is **not** part of this issue. The current site is correct for 0.16.0; older versions can be published on request via `gh workflow run docs-publish.yml -f version=<v>` and no code change is needed for that.
- Verification against a real release happens on the next release or a pre-release tag after merge; it cannot be fully exercised in PR CI.

## Out of Scope

- Backfilling docs for v0.14.0–v0.15.0 (see Assumptions).
- Switching the release to a PAT or GitHub App token.
- Changing the docs build, versions switcher, or root-promotion logic in `docs-publish.yml`.
- Alerting when a docs publish fails, beyond the run itself being visible.

## Source References

- `.github/workflows/release.yml` (`host`, `announce` jobs)
- `.github/workflows/docs-publish.yml`
- `docs/release-process.md` § Docs publishing
- `docs/adr/0477-tag-based-versioned-docs-publishing.md`
- `CONTRIBUTING.md` release runbook step 6
