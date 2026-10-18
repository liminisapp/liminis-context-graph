# Feature Specification: Release builds survive a failed prebuilt-liblbug download

**Feature Branch**: `fabrik/issue-690`
**Created**: 2026-10-18
**Status**: Specified
**Input**: User description: "Release: prefetch and cache the pinned prebuilt liblbug so one failed download can't fail a release. The `lbug` crate's build script fetches a prebuilt static `liblbug` (pinned by `LBUG_VERSION` in `.cargo/config.toml`) with one attempt and no retry, then silently falls back to a source build that cannot succeed on the macOS release runner. A single transient HTTP 500 failed Release run 37319403247 (#688). Prefetch the pinned prebuilt with retries before `dist build`, cache it, stop depending on a script fetched from LadybugDB's `main` branch, and fix or remove the misleading lbug cache step in `release.yml`."

## Background

The `lbug` crate's build script fetches a prebuilt static `liblbug`, pinned by `LBUG_VERSION = "0.21.1"` in `.cargo/config.toml`, from LadybugDB's GitHub releases. It makes **one attempt with no retry**. If that attempt fails, it silently falls back to compiling lbug's C++ from source.

On the macOS release runner that fallback **cannot succeed**. lbug 0.21.1 uses `std::atomic_ref` (`src/c_api/connection.cpp:110`), which the libc++ shipped with the runner's Xcode 15.4 lacks.

So one transient HTTP error fails the whole release build. That happened in #688: Release run 37319403247 on `main` at `b16a742b` failed. The fetch of `liblbug-static-osx-arm64.tar.gz` returned `curl: (56) ... error: 500`, the build fell back to source and hit 5 compile errors. The same commit's previous Release run had passed.

`download_lbug.sh` also fetches LadybugDB's `download-liblbug.sh` from `refs/heads/main` on `raw.githubusercontent.com` at build time. That is a second network dependency, and it isn't pinned to the version being built.

The existing "Restore lbug build artifacts from cache" step in `release.yml` caches `target/release/{build,.fingerprint,deps}/lbug-*`. The release build compiles under `target/<triple>/dist/...`, so that cache doesn't cover the release build at all, and the prebuilt is downloaded fresh on every release build.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - A release build doesn't fail on one transient download error (Priority: P1)

As the release manager, I want the release build to get the pinned prebuilt `liblbug` reliably, so that a single upstream 5xx doesn't fail a tag's release.

**Why this priority**: This is the whole point of the issue. A transient upstream error currently costs a failed release and a manual re-run, and the failure message (source-build compile errors) hides the real cause.

**Independent Test**: Run the release workflow (or the prefetch step in isolation) with the first download attempt forced to fail, for example by a bad URL on attempt 1, and confirm the build still produces artifacts. Then force every attempt to fail and confirm the step fails fast with a message naming the URL.

**Acceptance Scenarios**:

1. **Given** the upstream release asset returns a transient 5xx once, **When** the release workflow runs, **Then** the fetch is retried and the build uses the prebuilt library.
2. **Given** the prebuilt for the current `LBUG_VERSION` was fetched by an earlier run, **When** the release workflow runs, **Then** it is restored from the Actions cache and no download is needed.
3. **Given** the prebuilt really can't be obtained, **When** the release workflow runs, **Then** the build fails fast with a clear message naming the URL. It must not silently fall back to a source build that is known to fail on this runner.

---

### Edge Cases

- The cache holds a prebuilt for an older `LBUG_VERSION`: the cache key includes the version, so a version bump misses the cache and fetches the new one.
- A cache entry is present but corrupt or truncated: the build must not silently use it; a failed integrity or presence check should fall through to a fresh fetch (or fail fast with a clear message).
- Matrix targets map to different upstream asset names (macOS arm64, Linux x86_64 and arm64, Windows): each must resolve to its own asset and cache entry; a missing mapping for a target fails loudly rather than defaulting.
- `LBUG_VERSION` is absent or unparseable in `.cargo/config.toml`: the step fails with a clear message rather than fetching `latest`.
- The extension-hash pin check and the macOS OpenSSL linkage guard run after the fetch and must not be bypassed or reordered in a way that changes their results.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: Before `dist build`, a workflow step fetches the pinned prebuilt `liblbug` for the matrix target, with retries and backoff, into a directory exported as `LBUG_PREBUILT_CACHE_DIR`. lbug's build script already honours that directory and skips its own download when the library is present.
- **FR-002**: The fetched prebuilt is cached with `actions/cache`, keyed on at least `LBUG_VERSION` and the target platform.
- **FR-003**: The version fetched comes from `LBUG_VERSION` in `.cargo/config.toml`, the single source of truth, and isn't duplicated in the workflow.
- **FR-004**: The fetch doesn't depend on a script fetched from LadybugDB's `main` branch at build time.
- **FR-005**: Fix or remove the "Restore lbug build artifacts from cache" step, so it either covers the release build's real paths or no longer appears to.
- **FR-006**: Apply the same treatment to every `release.yml` build target that downloads the prebuilt, including the Linux targets and Windows, unless a target is shown not to need it.
- **FR-007**: If the prebuilt cannot be obtained after all retries, the step fails the job with a message that names the URL that was attempted; the release build must not proceed to a source-build fallback.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A release build where the first prebuilt download fails still produces artifacts. Demonstrate this by simulating a failed first attempt, for example with a bad URL on attempt 1.
- **SC-002**: A second release run for the same `LBUG_VERSION` restores the prebuilt from cache, and its log shows no download.
- **SC-003**: The extension-hash pin check and the macOS OpenSSL linkage guard still pass, and the published archives are unchanged in layout.
- **SC-004**: When every download attempt fails, the job fails at the fetch step (not later in a compile step) and its log names the URL.

## Assumptions

- The `LBUG_PREBUILT_CACHE_DIR` mechanism in the pinned `lbug` crate behaves as the issue describes (the build script skips its own download when the library is already present); the Research stage verifies this against the crate source.
- The release workflow is regenerated by cargo-dist from `.github/build-setup.yml` plus hand-added steps; the new step's placement must respect the existing ordering constraints (as the OpenSSL staging step already does), which Plan decides.
- The prebuilt URL for a given version and platform follows LadybugDB's published release-asset naming, and the target-triple-to-asset-name mapping lives alongside the existing target mappings used for extension staging.
- CI (`ci.yml`) is not in scope for this issue unless it is shown to share the same single-attempt download exposure; the issue is about the release workflow.
- Retry count and backoff values are an implementation choice; a small number of attempts (for example three to five) with increasing delay is sufficient for transient 5xx errors.

## Out of Scope

- Upgrading the macOS runner's Xcode so the from-source fallback compiles. That is optional extra safety, not the fix: a source build adds a full lbug C++ compile to every affected release. If done, note it, but don't rely on it.
- Changing which lbug version is pinned, or the lbug crate's own build script.
- Changing OpenSSL linkage or the extension-staging behaviour.
- Restructuring `ci.yml`'s lbug cache.
