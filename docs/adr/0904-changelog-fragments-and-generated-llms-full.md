# ADR-0904: Changelog Fragments, and `llms-full.txt` Generated at Publish Time

**Status**: Accepted
**Date**: 2026-11-23
**Issue**: #904 (amends [ADR-0295](0295-github-pages-documentation-site.md) for `llms-full.txt`;
touches the #852 guard from [ADR-0852](0852-published-response-json-schemas.md))

## Context

Two files were edited by almost every PR: the `## [Unreleased]` block of `CHANGELOG.md` (CONTRIBUTING
required it, and the #852 guard failed a `contract/` PR that did not touch it), and the committed,
generated `docs/llms-full.txt` (drift-checked by `docs-drift.yml`). Concurrent PRs therefore always
overlapped. The Fabrik merge train cannot batch overlapping PRs, so it landed them one at a time with a
full ~40 minute CI run per landing; during the 0.20.0 cycle every member was deferred at least once for
"overlaps #N on CHANGELOG.md", and each landing forced the others through another rebase.

The published outputs of both files are still needed and must not change: the CHANGELOG feeds the
GitHub Release body (cargo-dist `plan`), ships in the release archives and is the documented channel
for behaviour changes that affect raw-socket clients; `llms-full.txt` is served on the docs site
(ADR-0295). Only the *per-PR editing of one shared file* had to go.

## Decision

### 1. Changelog fragments

- A PR records its entry as one new file, `changelog.d/<issue>-<slug>.md`, made of `### <Section>`
  headings (the sections `CHANGELOG.md` already uses, plus `Removed` and `Security`) and entries. The
  PR does not edit `CHANGELOG.md`. The "CHANGELOG rules for raw-socket clients" apply to the fragment.
- `scripts/assemble-changelog.sh` (bash + POSIX awk, no dependencies) does both jobs, so the CI check
  and the release assembly cannot disagree: `--check` validates every fragment and writes nothing;
  with no flag it merges the fragments, plus any entries already under `## [Unreleased]`, into that
  section — one heading per section, fixed order, existing text first, fragments in issue-number
  order (904 before 1000) — and deletes them. It validates everything first and changes nothing on any
  error; with no fragments it exits 0 without touching the file (idempotent). Headings in the existing
  body that are not in the known list are kept after the known ones.
- The fixed order is Upgrading, Upgrade notes, Added, Fixed, Changed, Deprecated, Removed, Security,
  Response schemas, Documentation, Internal, Known issues, Windows notes: it mirrors the released
  sections (0.18.1, 0.19.0, 0.19.1), so the upgrade note still leads the GitHub Release body.
- `changelog.d/` may hold only `README.md` and top-level `<number>-<slug>.md` files; anything else
  (a subdirectory, `.markdown`, `.MD`, a mis-named file) fails `--check`, because the assembler would
  never read it. The assembler also refuses to write if the existing `[Unreleased]` has an unclosed
  code fence (or a `## [` line inside a fence), which would otherwise fold later releases into the new
  section.
- The release manager runs it in the release PR **before** renaming `[Unreleased]` to the version
  (CONTRIBUTING runbook step 4). Nothing else in the release path changes: `release.yml`, cargo-dist's
  `plan`, `assert-plan-output.sh` and the archive staging read the assembled `CHANGELOG.md`.
- `--check` runs in its own CI job, `changelog fragments`, which runs on every PR and should be a
  required check. It is deliberately **not** in `lint`: `lint` is skipped for docs-only PRs
  (ADR-0322), and a fragment-only PR is exactly that, so a check there would never run where it
  matters. It is also not a step in `changes`: a failure there would leave the classifier unrun, and
  every downstream job would take its fail-safe "run everything" branch, starting the full build and
  test suite for a typo in a fragment. `changelog.d/` needs no classifier change — it is not in
  `CODE_PATTERN`, so it is docs-only by default, like `CHANGELOG.md`; `test-patterns.sh` pins it.
  The assembler's self-test runs in `lint` (the scripts match `^scripts/`, i.e. code).
- The #852 guard accepts an added or modified top-level `changelog.d/*.md` fragment (not `README.md`; a
  deleted one is the release PR consuming it) *or* a direct `CHANGELOG.md` edit.
- **Transition.** PRs in flight (#859, #861, #862, #900, #903) still edit `CHANGELOG.md` directly and
  that keeps working: those entries stay where they are and are merged with fragments at assembly.
  Existing `[Unreleased]` text is not rewritten by this change; the first assembly regroups it (the
  block already has duplicate `### Added`/`### Fixed` headings). Removing the direct-edit allowance is
  a later follow-up.

### 2. `llms-full.txt` generated at publish time

- `docs/llms-full.txt` is removed from git and ignored. `scripts/generate-docs-llms-full.sh` is
  unchanged in logic and still writes `docs/llms-full.txt`.
- `scripts/docs-publish-build.sh` runs the generator from the checkout being published, before the
  site build, so each docs version (ADR-0477) serves its own bundle at the same URL as before. A
  generator failure fails the publish. `site/scripts/sync-docs.mjs` now throws if the file is missing
  (it used to skip silently, which would have published a site without the file and no error).
- `docs-drift.yml`'s `check-llms-full` job keeps its id and display name (a required status check may
  be keyed on them) but now only runs the generator, so it still fails on everything the generator
  rejects (a missing H1 or Cargo version), and fails if the file is tracked. The link-check job
  generates the file before building, because `docs/index.md` links to it.

## Consequences

- Two concurrent PRs that each add a changelog entry, or each touch docs, share no changed file, so
  the merge train sees no overlap on either.
- Nothing needs committing for `llms-full.txt`; a contributor no longer reruns the generator, and the
  release runbook's regenerate step is gone. The generator runs locally in one command.
- `docs/llms-full.txt` no longer resolves on github.com (the relative link in `docs/index.md` still
  resolves on the published site). The PR audited every other reference: no Rust code, workflow or
  test reads the committed file; the only consumer is `sync-docs.mjs`, handled above. Historical
  CHANGELOG and release-note mentions are left as written.
- An in-flight PR that regenerated `docs/llms-full.txt` conflicts modify/delete on rebase; resolve by
  dropping the file.
- The generator takes its version from `Cargo.toml`, not `DOCS_VERSION`, so ADR-0477's republish-from-
  a-corrected-branch case can bundle a different version string than the site's label. That was true
  before this change and is left alone.

## Alternatives considered

- **A separate fragment validator.** Rejected: a second parser can disagree with the assembler.
- **Validating in `lint` or `docs-drift.yml`.** `lint` is skipped for docs-only PRs; `docs-drift` is
  scoped to docs drift.
- **Generating `llms-full.txt` inside `sync-docs.mjs`.** Rejected: it would make the Node script shell
  out to bash; explicit generator calls plus a loud failure give the same safety.
- **Keeping the committed file and fixing the merge train.** Out of scope, and the file is a pure
  function of committed inputs.
