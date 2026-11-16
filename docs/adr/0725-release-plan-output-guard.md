# ADR-0725: Fail a Tag Release Run When the Plan Output Is Lost (`verify-plan`)

**Status**: Accepted
**Date**: 2026-11-15
**Issue**: #725 (triggered by the first `v0.18.0` Release run, which published a release holding only `dist-manifest.json` from a green run; the changelog was reworded in #723 and the tag re-cut)

## Context

`release.yml`'s `plan` job exposes the whole dist manifest as its `val` output. On a tag push that
manifest embeds the CHANGELOG section as the release notes. The first `v0.18.0` section contained
`` `Authorization: Bearer <key>` ``; the runner masked it as a credential and dropped `val`
("Skip output 'val' since it may contain secret") with no error.

Everything downstream read an empty value:

- `build-local-artifacts.if` evaluates `fromJson(needs.plan.outputs.val)...include != null`; with
  an empty `val` that is false, so the build was **skipped**, and `build-global-artifacts` skipped
  after it.
- `host.if` accepted `skipped` for both build jobs (the cargo-dist generated condition), so it
  created the release from nothing, and `announce` and `dispatch-docs` followed.

The workflow could not tell "nothing to build" from "the plan result was lost", and reported
success for both.

## Decision

1. **A tag-only guard job, `verify-plan`** (`needs: plan`, `if: needs.plan.outputs.publishing ==
   'true'`). It receives `needs.plan.outputs.val` via `env:` and runs
   `scripts/assert-plan-output.sh`, which fails if the value is empty, not JSON, has no
   `.ci.github.artifacts_matrix.include` array, or has an empty one. The failure message and step
   summary name secret masking as the likely cause and never echo the value.
2. **Why a separate job.** Masking is applied when job outputs are handed to downstream jobs,
   after the producing job's steps finish. A step at the end of `plan` cannot see the drop; only a
   downstream job observes what the builds observed.
3. **`host` requires success, not "not failed".** `host.needs` gains `verify-plan`, and `host.if`
   requires `plan`, `verify-plan`, `build-local-artifacts` and `build-global-artifacts` to all be
   `success`. A skipped build for any other reason therefore also blocks publishing. Tightening
   `host.if` alone would leave a green run with a skipped `host`; the guard job is what turns the
   run red. `announce` and `dispatch-docs` gate on `host` succeeding and inherit the protection.
4. **`verify-plan` is deliberately not in any build job's `needs`.** On a push to `main` it is
   skipped (`publishing` is `'false'`), and a skipped dependency skips its dependants, so the
   builds would skip and `main` would go green having verified nothing, the failure class the
   header warns about. `main` behaviour is unchanged and the guard cannot fail it. A failed `plan`
   still skips the guard, so that failure stays the run's failure.
5. **Offline test.** `scripts/test-assert-plan-output.sh` covers the guard's logic and is run by
   `ci.yml`'s `lint` job. The end-to-end procedure (a throwaway pre-release tag with
   credential-looking text in its CHANGELOG section) is in `docs/release-process.md`.

## Consequences

- A tag run whose plan lists no artifacts now fails by design; a release with nothing in it is
  never the intent.
- `verify-plan` and the changed `host.needs` / `host.if` are hand-maintained and are listed in the
  `release.yml` header and in ADR-0639's hand-edit boundary (extending that list is the only
  change to ADR-0639).
- The guard observes the *effect*, so any cause of a lost plan output is caught, not just
  changelog text. It is blind only if `publishing` itself were dropped; that is the literal string
  `true`, which cannot match a secret pattern.
- Like the rest of `release.yml`, this cannot be run end-to-end before merge (ADR-0639); the first
  tag run after merge is the live exercise.
