# Feature Specification: Bump lbug 0.21.1 → 0.21.2

**Feature Branch**: `fabrik/issue-699`
**Created**: 2026-10-18
**Status**: Specified
**Input**: User description: "Bump lbug 0.21.1 → 0.21.2 (supersedes Dependabot #693)"

## Background

LadybugDB released **v0.21.2** on 2026-10-01. It has 9 commits on top of v0.21.1, all bug fixes, including:

- **`NodeGroup::scan` ran past a sub-node-group morsel's end.** `row_idx_t` is unsigned, so the subtraction wrapped and the clamp scanned to the end of the group. This is a silent wrong-result bug, and the main reason to take this release.
- **A buffer-manager race.** `VMRegion` grew its per-granule residency table while other threads read it without a lock. It now uses `ConcurrentVector` and an atomic `numFrameGroups`.
- **A planner fix** for the by-value set in correlated `OPTIONAL` + `UNWIND`.
- **Smaller fixes:** null-safe cast error messages, cached table-function plans, and exhaustive enum switches.

Dependabot #693 bumps only `Cargo.toml`/`Cargo.lock`. It cannot merge as it is. Its `test` job fails `extension_version_was_verified_against_current_lbug_pin` and the e2e jobs were cancelled after it. That is the designed tripwire: an lbug bump must also move `LBUG_VERSION` and `LBUG_CRATE_VERSION_VERIFIED_AGAINST` (CLAUDE.md, ADR-0559, ADR-0593). #676, the 0.21.0 → 0.21.1 bump, is the template.

Already checked (2026-10-18):

- **Storage version stays 47.** The only change to `storage_version_info.h` is a new map entry, `{"0.21.2", STORAGE_VERSION_47}`.
- **The crate's Rust code is unchanged.** The published `lbug` 0.21.1 and 0.21.2 crates have identical `src/` and an identical `scripts/download_lbug.sh`. `build.rs` only adds two `rerun-if-changed` lines for packaging scripts used in source builds (`collect-single-file-header.py`, `headers.txt`).
- **Prebuilt bundles exist for 0.21.2:** `liblbug-static-{osx-arm64,linux-x86_64-compat,linux-x86_64-perf,windows-x86_64}` and the rest, on the upstream v0.21.2 release.
- **There are no new extensions.** `extension.ladybugdb.com/v0.21.2/...` and `/v0.21.1/...` return 404, and `/v0.21.0/...` returns 200. `LBUG_EXTENSION_VERSION` is expected to stay `0.21.0`, as it did for 0.21.1, but this must be confirmed.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - lcg runs on lbug 0.21.2 (Priority: P1)

As an lcg user, I want lcg built on lbug 0.21.2, so that I get its scan-correctness and concurrency fixes.

**Why this priority**: The release fixes a silent wrong-result scan bug and a buffer-manager race. Dependabot's partial bump is also blocked by the pin tripwire until the pins move together.

**Independent Test**: Apply the bump and run CI on the PR. Every job must pass, and an existing 0.21.1-built database must open correctly under the new build.

**Acceptance Scenarios**:

1. **Given** the bump is applied, **When** CI runs, **Then** every job passes with nothing cancelled. That includes the full Rust suite, all six real-corpus e2e jobs, Windows with real index creation and queries, and `storage_v42_migration`.
2. **Given** a database created by lcg 0.17.0 (lbug 0.21.1), **When** it is opened by the bumped build, **Then** it opens with no storage migration. The FTS indexes are rebuilt exactly once (the #649 marker compares the exact lbug version), and later opens do not rebuild.

---

### Edge Cases

- **The extension directory changes in 0.21.2.** This contradicts the expectation that it stays `0.21.0`. The extensions must then be re-pinned per ADR-0593, and the Windows extension CLI matrix must be re-run before Windows is claimed to work (the lbug 0.20.x Windows extension crash precedent).
- **Only some of the three pins are moved.** The `extension_version_was_verified_against_current_lbug_pin` tripwire must fail, which is how Dependabot #693 fails today.
- **The `.cargo/config.toml` `LBUG_VERSION` pin does not match the crate pin.** The build would fetch a mismatched native bundle (the v0.9.0 release failure). The two pins must move together.
- **Historical mentions of 0.21.1** in ADRs, release notes and older changelog entries must stay as they are. Only statements of the *current* pin change.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: Move all three pins together:
  - `lbug = "=0.21.2"` in the workspace `Cargo.toml`, with `Cargo.lock` synced;
  - `LBUG_VERSION = "0.21.2"` in `.cargo/config.toml`;
  - `LBUG_CRATE_VERSION_VERIFIED_AGAINST = "0.21.2"` in `crates/core/src/lbug_extension_home.rs`.
- **FR-002**: Confirm the extension directory 0.21.2 actually uses, as #676 did:
  - read the `LBUG_EXTENSION_VERSION` CMake define;
  - run `INSTALL vector` with the upstream v0.21.2 CLI and an empty `HOME`;
  - check the CDN paths.

  If it is still `0.21.0`, leave `LBUG_EXTENSION_VERSION` and `LBUG_EXTENSION_HASHES` unchanged, and add a dated note to `LBUG_EXTENSION_HASHES` that the 0.21.0 bytes were re-verified against 0.21.2. If it changed, re-pin the extensions per ADR-0593, and re-run the Windows extension CLI matrix before claiming Windows works.
- **FR-003**: Update docs that state the *current* pin: README, CLAUDE.md (the `LBUG_VERSION = "0.21.1"` sentence), ADR-0559, and the `storage_v42_migration` comments. Leave historical mentions alone.
- **FR-004**: Add a `CHANGELOG.md` `## [Unreleased]` entry noting the bump and the one-time FTS rebuild on first start.
- **FR-005**: Close Dependabot #693 when this lands, noting that this issue's PR supersedes it.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: CI is all green on the PR, with nothing cancelled.
- **SC-002**: An upgrade test, a test fixture, or a manual check recorded in the PR opens a 0.21.1-built database and shows one FTS rebuild, then no rebuild on the next open, with data unchanged.
- **SC-003**: The release-artifact build and `scripts/assert-openssl-linkage.sh` pass on all four targets. That includes the post-merge `Release` run on `main`.

## Assumptions

- No schema or storage change is involved, so this ships as a patch release (0.17.1).
- `LBUG_EXTENSION_VERSION` stays `0.21.0`, because no 0.21.1 or 0.21.2 extension directories exist on the CDN. FR-002 verifies this, and its alternative branch covers the case where it does not hold.
- The one-time FTS rebuild on first start is accepted behaviour. The #649 marker records the exact lbug version, as it did for the 0.21.0 → 0.21.1 bump.

## Out of Scope *(optional)*

- Upstream's later fixes for LadybugDB/ladybug#1092 (PR #1108, stale pre-0.21 string hash indexes detected loudly) and PR #1201 (legacy sub-index probe for stale non-ASCII STRING primary keys). They are on upstream `main` after v0.21.2 and arrive with the next lbug release.
- Any schema, storage-format, or IPC protocol change.
- Rewriting historical ADR, release-note, or changelog mentions of 0.21.1.

## Source References *(optional)*

- Dependabot PR #693 (partial bump that fails the tripwire)
- #676 (0.21.0 → 0.21.1 bump, the template for this change)
- ADR-0559, ADR-0593 (pin tripwire and extension re-pinning), #649 (FTS rebuild marker)
- LadybugDB v0.21.2 release notes
