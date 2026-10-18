# ADR-0690: Prefetch and Cache the Pinned Prebuilt liblbug for Release Builds

**Status**: Accepted
**Date**: 2026-10-18
**Issue**: #690 (triggered by failed Release run 37319403247, #688)

## Context

The `lbug` crate's `build.rs` fetches a prebuilt static `liblbug` (pinned by `LBUG_VERSION` in
`.cargo/config.toml`) from LadybugDB's GitHub releases with **one attempt and no retry**, then
silently falls back to a source build. On the macOS release runner that fallback cannot succeed:
lbug 0.21.1 uses `std::atomic_ref`, which the libc++ of the runner's Xcode 15.4 lacks. One
transient HTTP 500 therefore failed a release with five compile errors that hid the real cause.

The "Restore lbug build artifacts from cache" step in `release.yml` could not help. It restored
`target/release/{build,.fingerprint,deps}/lbug-*`, but `dist build` writes under
`target/<triple>/dist/`, and nothing in `release.yml` ever saved that key. It had never been able
to hit.

## Decision

1. **Prefetch before `dist build`.** `scripts/fetch-lbug-prebuilt.sh <target-triple> <cache-dir>`
   downloads the pinned prebuilt with 5 attempts and 2/4/8/16 s backoff, extracts it, verifies the
   library and both headers are present and non-empty, and exports `LBUG_PREBUILT_CACHE_DIR` via
   `$GITHUB_ENV`. If every attempt fails it exits non-zero naming the URL; there is no source
   fallback.
2. **Cache the directory** with `actions/cache`, keyed on
   `lbug-prebuilt-<runner.os>-<target triple>-<LBUG_VERSION>`. A version bump misses the cache. A
   hit prints "cache hit, no download".
3. **The script is the only place that parses `LBUG_VERSION`** (from `.cargo/config.toml`). The
   workflow reads it once, with the same one-line `sed`, only to build the cache key.
4. **Remove the dead restore step** and its `rustc-ver` / `lbug-hash` helper steps rather than
   repointing them.
5. **A presence check before `dist build`** asserts the exact file `build.rs` will look for.

### The key shape is a coupling to `build.rs`

`build.rs::prebuilt_cache_key()` (lbug 0.21.1) looks for
`$LBUG_PREBUILT_CACHE_DIR/version-<LBUG_VERSION>[-compat]/lib/{liblbug.a|lbug.lib}`. The `-compat`
suffix appears **on Linux only** (`LBUG_LINUX_VARIANT` defaults to `compat`). If the staged
directory name drifts from that, `build.rs` ignores the prefetch and downloads again, silently.
`scripts/test-fetch-lbug-prebuilt.sh` pins the shape and CI runs it on every PR. Re-check the key
function and the asset names in `scripts/download-liblbug.sh` on every `lbug` / `LBUG_VERSION`
bump; the target-to-asset table lives at the top of the fetch script.

### FR-004: no dependency on LadybugDB's `main`

On 0.21.1, `download_lbug.sh` does not actually fetch anything from `refs/heads/main`, because the
crate tarball already ships `scripts/download-liblbug.sh`. That dependency would only return if a
future crate version stopped shipping the script. The fetch script never invokes either of them, so
FR-004 holds regardless.

### Integrity

No sha256 pin is added for the liblbug archives (unlike the extensions, ADR-0593). A pin would need
a manual update on every `LBUG_VERSION` bump for a download that already travels over TLS from
GitHub releases. The bar is: `tar`/`unzip` must succeed and the three files must be non-empty. A
cache entry failing that check is discarded and re-fetched. Cache poisoning beyond truncation is
accepted; a pin is an additive follow-up if it ever matters.

### Env delivery, and the documented fallback

`LBUG_PREBUILT_CACHE_DIR` is delivered through `$GITHUB_ENV` only. `.cargo/config.toml` is not
changed, because that would alter local and CI builds. #550 found that a shell-exported
`PKG_CONFIG_PATH` did not reach `build.rs` under `dist build`; the same mechanism should deliver
this variable, but it is verified rather than assumed. If a release log shows `Build artifacts`
printing `Prebuilt liblbug download` or `building from source`, the variable did not propagate.
The fallback is then to set `LBUG_PREBUILT_CACHE_DIR` in `.cargo/config.toml`'s `[env]` with
`relative = true, force = true`, exactly as `PKG_CONFIG_PATH` is, and to stage the cache directory
at that relative path.

### Rejected: `LBUG_BUILD_FROM_SOURCE` as a belt-and-braces guard

The source-build path is broken (7399 duplicate symbols, `ladybug-rust#18`) and errors less clearly
than a failed fetch step. The presence check already guarantees `build.rs` takes its
`lib_path.exists()` fast path.

## Consequences

- A single upstream 5xx no longer fails a release; an unobtainable prebuilt fails at the fetch step
  with the URL in the log.
- `release.yml` has no PR-time path (it runs on `main` pushes and tags), so the workflow wiring is
  first exercised by the next release. Evidence of success: "cache miss, downloaded" on the first
  run, "cache hit, no download" on the second, and no `Prebuilt liblbug download` warning from
  `Build artifacts`.
- `.github/build-setup.yml` mirrors the steps as documentation only (`allow-dirty = ["ci"]`).
- **Known follow-up, out of scope:** `ci.yml` and `windows.yml` have the same single-attempt
  download exposure and no `LBUG_PREBUILT_CACHE_DIR`. They can adopt the same script.
- Supersedes in part ADR-0021, whose `LBUG_BUILD_FROM_SOURCE` and shared-cache content is stale.
