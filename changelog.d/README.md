# Changelog fragments

Each PR with a user-visible change records it as **one new file in this directory** instead of
editing `CHANGELOG.md`. Two PRs then never touch the same file, so the merge train can batch them
(#904, [ADR-0904](../docs/adr/0904-changelog-fragments-and-generated-llms-full.md)). This file is
documentation: the tooling ignores it, along with any file that does not end in `.md`.

## Writing a fragment

Name it `<number>-<slug>.md`: the GitHub issue number (the PR number when there is no issue), then
a lowercase slug of letters, digits and hyphens. Example: `904-changelog-fragments.md`. Two
fragments from one issue are fine; give them different slugs.

The content is the text you would have added under `## [Unreleased]`, with the same Keep a
Changelog `###` headings:

```markdown
### Added

- **Short bold summary (#123).** What a user sees now, in the same style as `CHANGELOG.md`.
  - Nested bullets and wrapped lines are copied as they are.

### Fixed

- **Another entry (#123).** ...
```

Rules, all enforced by `scripts/assemble-changelog.sh --check` (run on every PR):

- Use only these `###` sections: `Added`, `Changed`, `Deprecated`, `Removed`, `Fixed`, `Security`,
  `Response schemas`, `Documentation`, `Internal`, `Upgrading`, `Upgrade notes`, `Known issues`,
  `Windows notes`. The assembled release lists them in that order. Names are case-sensitive.
- Every heading needs at least one line of entries under it.
- Nothing but blank lines before the first heading, and no `#` or `##` headings.
- A `###` line inside a code fence is text, not a heading.

The "CHANGELOG rules for raw-socket clients" in `CONTRIBUTING.md` apply to the fragment text: name
the raw-socket clients affected and flag a contract or response-semantics change as **BREAKING**.
Fragment text ends up in the GitHub Release body, so it must not contain credential-looking text
(`docs/release-process.md`).

## Assembling (release manager)

In the release PR, **before** renaming `## [Unreleased]` to the version, run:

```sh
scripts/assemble-changelog.sh
```

It merges every fragment into `## [Unreleased]` together with any entries already there (one
heading per section, existing text first, fragments in file-name order), then deletes the
fragments. It validates everything first and changes nothing if any fragment is invalid. Running
it again with no fragments left is a no-op.

## Transition

PRs may still edit `CHANGELOG.md` directly; those entries stay where they are and are merged with
the fragments at assembly. That allowance is temporary.
