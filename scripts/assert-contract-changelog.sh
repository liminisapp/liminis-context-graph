#!/usr/bin/env bash
# PR guard (#852, ADR-0852, FR-011/SC-005): a change to a published response schema must be
# recorded in the changelog: a changelog.d/ fragment (#904) or, during the transition, a direct
# CHANGELOG.md edit.
#
# Usage: scripts/assert-contract-changelog.sh <base-ref> [head-ref]   (head defaults to HEAD)
#
# Compares <base-ref>...<head-ref> (the PR's own changes). If anything under contract/ changed
# (README.md excluded -- it is prose about the directory, not a schema) and the PR neither adds or
# edits a changelog.d/*.md fragment (changelog.d/README.md excluded -- it is the format note) nor
# edits CHANGELOG.md, exits non-zero with instructions. Run from inside the repository.
set -euo pipefail

base="${1:-}"
head="${2:-HEAD}"
[ -n "$base" ] || { echo "usage: $0 <base-ref> [head-ref]" >&2; exit 2; }

changed="$(git diff --name-only "$base...$head")"

if ! printf '%s\n' "$changed" | grep -E '^contract/' | grep -Ev '^contract/README\.md$' >/dev/null; then
  echo "no published schema changed; nothing to record"
  exit 0
fi

# A deleted fragment is the release PR consuming it, not an entry: only added/modified count.
fragments="$(git diff --name-only --diff-filter=AM "$base...$head" | grep -E '^changelog\.d/.+\.md$' | grep -Ev '^changelog\.d/README\.md$' || true)"
if [ -n "$fragments" ]; then
  echo "published schema changed and a changelog fragment was recorded"
  exit 0
fi

if printf '%s\n' "$changed" | grep -Fxq 'CHANGELOG.md'; then
  echo "published schema changed and CHANGELOG.md was updated"
  exit 0
fi

cat >&2 <<MSG
ERROR: this change modifies a published response schema under contract/ but records no changelog entry.

Schema changes are part of the release notes. Add a fragment file changelog.d/<issue>-<slug>.md
(see changelog.d/README.md) with a "### Response schemas" section naming the method and the field.
(Until the transition ends, editing CHANGELOG.md directly under [Unreleased] also satisfies this
check, but a fragment avoids overlapping other PRs.) Mark a removed or renamed field, a widened type
(for example `string` to `string|null`), a required field made optional, or an added union
alternative as **BREAKING**. Adding an optional field is not breaking but still needs an entry.
See CONTRIBUTING.md, "Published response schemas".
MSG
exit 1
