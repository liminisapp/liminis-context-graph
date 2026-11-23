### Added

- **Changelog fragments: one file per PR instead of editing `CHANGELOG.md` (#904,
  ADR-0904).** Every PR used to add its entry to the same `## [Unreleased]` block, so concurrent PRs
  always overlapped and the merge train landed them one at a time. A PR now adds
  `changelog.d/<issue>-<slug>.md` using the usual `###` section headings, and the release PR runs
  `scripts/assemble-changelog.sh` before renaming `[Unreleased]`, which merges the fragments (and any
  entries already under `[Unreleased]`) into one grouped section and deletes them.
  - `scripts/assemble-changelog.sh --check` validates every fragment (known section, entries under
    every heading, file name) and runs on every PR, including docs-only ones.
  - The contract guard (#852) accepts a `changelog.d/` fragment as the changelog entry for a
    `contract/` change. Editing `CHANGELOG.md` directly still works during the transition.
  - The published `CHANGELOG.md`, the GitHub Release body and the release archives are unchanged.
  - The "CHANGELOG rules for raw-socket clients" in CONTRIBUTING apply to fragment text.

### Changed

- **`docs/llms-full.txt` is no longer committed (#904, ADR-0904).** `scripts/generate-docs-llms-full.sh`
  runs when the docs site is built and published, so the site still serves `llms-full.txt` at the same
  URL. The `docs-drift` check now runs the generator on each PR (it still rejects the same constructs)
  instead of diffing a committed copy, and a docs build fails if the file is missing rather than
  publishing without it. The raw `docs/llms-full.txt` link on github.com no longer resolves.
