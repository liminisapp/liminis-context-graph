# Feature Specification: Fail a tag release run when its build jobs are skipped instead of publishing an empty release

**Feature Branch**: `fabrik/issue-725`
**Created**: 2026-11-15
**Status**: Specified
**Input**: User description: "Release: a tag run whose build jobs are skipped publishes an empty release and reports success. The first `v0.18.0` Release run ([40778784687](https://github.com/liminisapp/liminis-context-graph/actions/runs/40778784687)) finished green, yet the release it published had only `dist-manifest.json` attached: no archives, installers or checksums. Trigger: the CHANGELOG section contained `` `Authorization: Bearer <key>` ``. What the runner did: it masked that text as a credential and dropped the plan job's `val` output (*"Skip output 'val' since it may contain secret"*). Effect on the builds: `build-local-artifacts` evaluated `fromJson(needs.plan.outputs.val)...include != null` against an empty value and was skipped, and `build-global-artifacts` was skipped after it. Why it still published: `host` treats `skipped` as acceptable for both build jobs, so it created the release anyway. `announce` and `dispatch-docs` then ran. We fixed it by rewording the changelog (#723) and re-cutting the tag. Any future changelog text that trips masking, or any other way the plan output comes back empty, would fail the same silent way. Acceptance criteria: On a tag push (`publishing == 'true'`), the run **fails** if the plan output is empty or has no `artifacts_matrix`, rather than skipping the builds. (Option: a step at the end of `plan` that re-reads its own output and fails if the value would be dropped. Masking happens after the step, so check for a non-empty matrix in a cheap downstream job instead. Option: tighten `host`'s `if:` so that, when publishing, it requires `build-local-artifacts` and `build-global-artifacts` to be `success`, not `skipped`.) Branch pushes to `main` keep their current behaviour (plan only). A test or note in the workflow header explains the masking trap. Keep the generated-vs-hand-edited notes in the header accurate (#639). Out of scope: Changing the release-notes content pipeline."

## Background

The Release workflow plans a release in a `plan` job, then builds artifacts in later jobs whose conditions are derived from the plan's output. Whenever that output reaches the build jobs empty, the builds are skipped. A skipped build is treated as acceptable by the job that creates the GitHub Release, so the release is published with no archives, installers or checksums, and the announcement and docs-publish jobs run afterwards. The run is green end to end.

This happened on the first `v0.18.0` run. The CHANGELOG section included a string that looks like a credential (`Authorization: Bearer <key>`). The runner's secret masking treated it as one and dropped the plan job's output. Nothing failed, so the only symptom was a release with just `dist-manifest.json` attached. It was caught by inspection and fixed by rewording the changelog (#723) and re-cutting the tag.

The underlying defect is not the changelog wording. The workflow cannot tell "nothing to build" from "the plan result was lost", and it reports success for both. Any future text that trips masking, or any other cause of an empty plan output, would fail the same silent way on a tag push, which is the one situation where a published but empty release does real damage: users get a release that looks real but has nothing in it. The branch-push-to-`main` path only plans and verifies, publishes nothing, and is not affected.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - A tag run with a lost plan output fails loudly (Priority: P1)

A maintainer pushes a release tag. If the plan output reaches the downstream jobs empty or without a build matrix, for any reason including secret masking, the run ends red and no release is published or announced.

**Why this priority**: This is the whole point of the issue. An empty release that reports success is the worst outcome, because it goes unnoticed and is already public.

**Independent Test**: Simulate a tag run in which the plan output is empty or has no `artifacts_matrix` (for example by feeding the guard an empty value, or temporarily putting masked-looking text in the changelog on a test tag). Confirm the run fails at or before the point where the release would be created, and that no release, announcement or docs dispatch happens.

**Acceptance Scenarios**:

1. **Given** a tag push whose plan output is empty, **When** the workflow runs, **Then** the run fails, no GitHub Release is created or modified, and the announcement and docs-dispatch jobs do not run.
2. **Given** a tag push whose plan output is present but contains no `artifacts_matrix`, **When** the workflow runs, **Then** the run fails for the same reasons.
3. **Given** a tag push whose build jobs were skipped for any reason, **When** the workflow reaches the publish step, **Then** it does not publish.
4. **Given** the failure in scenarios 1 to 3, **When** a maintainer reads the failed run, **Then** a failing job or step message states that the plan output was empty or missing its build matrix and points at secret masking as a likely cause.

---

### User Story 2 - Healthy tag runs and branch pushes are unchanged (Priority: P1)

A normal tag push with a populated plan output still builds, hosts, announces and dispatches docs exactly as before. A push to `main` still only plans, builds and verifies, and publishes nothing.

**Why this priority**: The guard must not introduce false failures. A guard that breaks working releases or the per-merge verification on `main` is worse than the bug.

**Independent Test**: Run the workflow on a tag with a populated plan and confirm a release with all artifacts is published as today. Run it on a push to `main` and confirm that it behaves as before: no publish, no new failure, and no guard failing on the legitimate absence of publishing.

**Acceptance Scenarios**:

1. **Given** a tag push with a normal plan output, **When** the workflow runs, **Then** all existing jobs run and succeed and the release has its archives, installers and checksums.
2. **Given** a push to `main`, **When** the workflow runs, **Then** it plans and builds with its current behaviour, publishes nothing, and the guard does not fail it.

---

### User Story 3 - The masking trap is documented where maintainers will look (Priority: P2)

A maintainer editing the workflow or diagnosing a bad release can read in the workflow header why an empty plan output can happen (secret masking dropping a job output) and what now protects against it. The existing generated-versus-hand-maintained notes in the header stay accurate.

**Why this priority**: The failure mode is non-obvious. The runner's only signal was a log line saying the output "may contain secret". Without a note, the next person rediscovers it from scratch. It is secondary to the guard itself.

**Independent Test**: Read the header: it names the masking behaviour, states that the publish path now refuses to proceed without built artifacts, and lists any newly hand-added jobs or conditions in the hand-maintained list. Then check that every claim in the hand-edit boundary list still matches the workflow.

**Acceptance Scenarios**:

1. **Given** the updated workflow, **When** a maintainer reads its header, **Then** they find an explanation of the masking trap and which part of the workflow guards against it.
2. **Given** the guard adds or changes a hand-maintained condition or job, **When** the header's hand-edit boundary list is read, **Then** that item is listed there, and no listed item is stale.

---

### Edge Cases

- **Masking happens after a job's steps finish.** A check inside the `plan` job that reads its own output cannot see the drop, because the output is only dropped when it is handed to downstream jobs. The guard must therefore observe the value as downstream jobs receive it.
- **Tag push where the plan legitimately has no builds.** The plan for a real release always includes artifacts. A tag run that plans none is treated as a failure, because publishing a release with nothing in it is never the intent.
- **Plan job itself fails.** This already stops publishing and stays unchanged. The guard must not mask or replace that failure.
- **Only one of the two build jobs is skipped.** The publish step must not proceed, because a release missing either the local or the global artifacts is incomplete.
- **Branch pushes to `main` skip publishing by design.** The guard must key on the tag-based publishing condition, so the absence of a release on `main` is never reported as an error.
- **Re-running a failed run after fixing the cause.** A re-run on the corrected tag or changelog must succeed with no leftover state from the failed attempt.
- **Other output-dropping causes.** Any cause of an empty plan output, not just changelog text, must be caught by the same guard.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: On a tag push (`publishing == 'true'`), the run MUST fail if the plan output, as received by downstream jobs, is empty.
- **FR-002**: On a tag push, the run MUST fail if the plan output has no `artifacts_matrix`, or one with no entries.
- **FR-003**: On a tag push, the release-creating step MUST NOT proceed unless both the local-artifact and global-artifact build jobs completed successfully. A skipped result from either MUST block it.
- **FR-004**: When FR-001, FR-002 or FR-003 triggers, the announcement and docs-dispatch jobs MUST NOT run and no release may be created or modified.
- **FR-005**: The failure MUST produce a message, visible in the run, that says the plan output was empty or missing its build matrix and names secret masking of a job output as a known cause.
- **FR-006**: Behaviour on a push to `main` MUST be unchanged: it plans, builds and verifies as today, publishes nothing, and is never failed by the new guard.
- **FR-007**: Behaviour of a healthy tag run (populated plan, successful builds) MUST be unchanged, including the set of artifacts published.
- **FR-008**: The workflow header MUST explain the masking trap (a job output that looks like a credential is silently dropped, which skips the dependent builds) and name the mechanism that now guards against it.
- **FR-009**: The workflow header's generated-versus-hand-maintained notes MUST remain accurate: any job, step or condition added or changed by this work MUST be listed as hand-maintained, and no existing note may become stale.
- **FR-010**: The guard MUST be verifiable without cutting a real release, either through a test or through a documented, repeatable procedure that reproduces an empty plan output and shows the run failing.

### Key Entities

- **Plan output**: The structured value produced by the planning job that downstream jobs read to decide what to build. Its loss is the root cause here.
- **Build jobs (local and global)**: The jobs that produce archives, installers and checksums, gated on the plan output.
- **Host / publish job**: The job that creates the GitHub Release from the built artifacts. It currently accepts a skipped build as success.
- **Publishing condition**: The tag-based predicate that separates a release run from a `main` verification run.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A tag run whose plan output is empty or lacks a build matrix ends with a failed overall status, and 0 releases, announcements or docs dispatches result from it.
- **SC-002**: A published release from a tag run always contains its archives, installers and checksums. A release with only `dist-manifest.json` can no longer come from a green run.
- **SC-003**: Healthy tag runs and `main` pushes show no change in outcome or set of published artifacts compared with before the change.
- **SC-004**: A maintainer reading the failed run can identify "plan output lost, probably masked" as the cause from the run page alone, without reading the workflow source.
- **SC-005**: Every statement in the workflow header about what is generated versus hand-maintained is true of the final workflow.

## Assumptions

- The release workflow stays hand-maintained under the existing `allow-dirty` arrangement from #639, so adding a guard does not conflict with regeneration. This work adds to the hand-maintained list rather than changing that arrangement.
- A tag run that plans zero build artifacts is never a valid release, so failing it is correct.
- Which of the two options in the issue (a downstream matrix check, tightening the publish condition, or both) to use is a design decision for later stages. The requirements above are satisfied by either, and the strongest reading, covering both the lost plan output and the skipped builds, is preferred.
- The verification in FR-010 may be a documented manual procedure rather than an automated test, since workflow behaviour is hard to cover with the repo's Rust test suite. A header note alone does not meet it.
- No change to tag formats, artifact set, or release notes content is wanted.

## Out of Scope *(optional)*

- Changing the release-notes content pipeline, including how CHANGELOG sections are extracted or whether credential-like text is sanitized.
- Preventing secret masking from firing at all, since the goal is to detect its effect rather than avoid it.
- Changing branch-push (`main`) behaviour beyond confirming it is unaffected.
- Changes to the OpenSSL, extension-pin or linkage steps, or to the docs-dispatch job's own logic.

## Source References *(optional)*

- Failing run: https://github.com/liminisapp/liminis-context-graph/actions/runs/40778784687
- #723 (changelog rewording that worked around the incident) and #639 (hand-edit boundary and header notes)
- `.github/workflows/release.yml` (header, `plan`, `build-local-artifacts`, `host` conditions)
