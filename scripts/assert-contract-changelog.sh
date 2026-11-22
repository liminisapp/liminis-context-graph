#!/usr/bin/env bash
# PR guard (#852, ADR-0852, FR-011/SC-005): a change to a published response schema must be
# recorded in CHANGELOG.md.
#
# Usage: scripts/assert-contract-changelog.sh <base-ref> [head-ref]   (head defaults to HEAD)
#
# Compares <base-ref>...<head-ref> (the PR's own changes). If anything under contract/ changed
# (README.md excluded -- it is prose about the directory, not a schema) and CHANGELOG.md did
# not, exits non-zero with instructions. Run from inside the repository.
set -euo pipefail

base="${1:-}"
head="${2:-HEAD}"
[ -n "$base" ] || { echo "usage: $0 <base-ref> [head-ref]" >&2; exit 2; }

changed="$(git diff --name-only "$base...$head")"

if ! printf '%s\n' "$changed" | grep -E '^contract/' | grep -Ev '^contract/README\.md$' >/dev/null; then
  echo "no published schema changed; nothing to record"
  exit 0
fi

if printf '%s\n' "$changed" | grep -Fxq 'CHANGELOG.md'; then
  echo "published schema changed and CHANGELOG.md was updated"
  exit 0
fi

cat >&2 <<MSG
ERROR: this change modifies a published response schema under contract/ but not CHANGELOG.md.

Schema changes are part of the release notes. Add an entry under [Unreleased] (heading
"### Response schemas" if present) naming the method and the field. Mark a removed or renamed
field, a narrowed type, or an optional field made required as **BREAKING**. Adding an optional
field is not breaking but still needs an entry. See CONTRIBUTING.md, "Published response schemas".
MSG
exit 1
