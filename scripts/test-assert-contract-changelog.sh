#!/usr/bin/env bash
# Tests for scripts/assert-contract-changelog.sh (#852). Builds a throwaway git repository.
#
# Run: scripts/test-assert-contract-changelog.sh
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
guard="$here/assert-contract-changelog.sh"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
failures=0

git -C "$tmp" init -q -b main
git -C "$tmp" config user.email t@example.com
git -C "$tmp" config user.name t
git -C "$tmp" config commit.gpgsign false
mkdir -p "$tmp/contract" "$tmp/docs"
echo '{}' > "$tmp/contract/m.schema.json"
echo 'x' > "$tmp/CHANGELOG.md"
echo 'x' > "$tmp/contract/README.md"
git -C "$tmp" add -A && git -C "$tmp" commit -qm base

# expect <pass|fail> <description> <files to modify...>
expect() {
  local want="$1" why="$2" got f
  shift 2
  git -C "$tmp" checkout -q -B work main
  for f in "$@"; do mkdir -p "$tmp/$(dirname "$f")"; echo "change $RANDOM" >> "$tmp/$f"; done
  git -C "$tmp" add -A && git -C "$tmp" commit -qm change
  if (cd "$tmp" && "$guard" main HEAD >/dev/null 2>&1); then got=pass; else got=fail; fi
  if [ "$got" = "$want" ]; then printf '  ok    %-5s %s\n' "$want" "$why"
  else printf '  FAIL  wanted %s, got %s: %s\n' "$want" "$got" "$why"; failures=$((failures + 1)); fi
}

expect fail 'schema changed, no changelog' contract/m.schema.json
expect fail 'new schema added, no changelog' contract/new.schema.json
expect pass 'schema changed with changelog' contract/m.schema.json CHANGELOG.md
expect pass 'only the contract README changed' contract/README.md
expect pass 'unrelated change' docs/a.md
expect pass 'only the changelog changed' CHANGELOG.md
expect pass 'schema changed with a changelog fragment, no CHANGELOG.md edit' contract/m.schema.json changelog.d/904-x.md
expect pass 'new schema added with a changelog fragment' contract/new.schema.json changelog.d/5-y.md
expect pass 'only a fragment changed' changelog.d/904-x.md
expect fail 'schema changed, only changelog.d/README.md touched' contract/m.schema.json changelog.d/README.md
expect fail 'schema changed, only a non-markdown file in changelog.d/' contract/m.schema.json changelog.d/notes.txt
expect fail 'schema changed, fragment only in a changelog.d/ subdirectory (never assembled)' contract/m.schema.json changelog.d/schemas/870-x.md

# the release PR consumes (deletes) fragments; a deletion is not a changelog entry
git -C "$tmp" checkout -q -B seed main
mkdir -p "$tmp/changelog.d"; echo x > "$tmp/changelog.d/9-old.md"
git -C "$tmp" add -A && git -C "$tmp" commit -qm seed-fragment
git -C "$tmp" checkout -q -B main seed
git -C "$tmp" checkout -q -B work main
git -C "$tmp" rm -q changelog.d/9-old.md; echo "change $RANDOM" >> "$tmp/contract/m.schema.json"
git -C "$tmp" add -A && git -C "$tmp" commit -qm delete-fragment
if (cd "$tmp" && "$guard" main HEAD >/dev/null 2>&1); then
  echo "  FAIL  schema changed with a fragment only deleted should fail"; failures=$((failures + 1))
else echo "  ok    fail  schema changed, fragment only deleted"; fi

# the failure message describes the fragment route
git -C "$tmp" checkout -q -B work main
echo "change $RANDOM" >> "$tmp/contract/m.schema.json"; git -C "$tmp" add -A && git -C "$tmp" commit -qm msg
msg="$(cd "$tmp" && "$guard" main HEAD 2>&1 >/dev/null || true)"
if printf '%s\n' "$msg" | grep -q 'changelog.d/'; then
  echo "  ok    failure message mentions fragments"
else echo "  FAIL  failure message should mention changelog.d/"; failures=$((failures + 1)); fi

if (cd "$tmp" && "$guard" >/dev/null 2>&1); then
  echo "  FAIL  missing base-ref should fail"; failures=$((failures + 1))
else
  echo "  ok    missing base-ref fails"
fi

echo
if [ "$failures" -ne 0 ]; then echo "$failures failure(s)"; exit 1; fi
echo "all assert-contract-changelog checks passed"
