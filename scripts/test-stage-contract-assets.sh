#!/usr/bin/env bash
# Tests for scripts/stage-contract-assets.sh (#852). Synthetic inputs plus the real contract/.
# Needs jq.
#
# Run: scripts/test-stage-contract-assets.sh
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/stage-contract-assets.sh"
repo="$(cd "$here/.." && pwd)"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
failures=0

ok()   { printf '  ok    %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; failures=$((failures + 1)); }

schema='{"$schema":"http://json-schema.org/draft-07/schema#","type":"object"}'

echo "a release without its contract must fail:"
mkdir -p "$tmp/empty"
if "$script" v1.2.3 "$tmp/out1" "$tmp/empty" >/dev/null 2>&1; then fail "empty contract dir passed"; else ok "empty contract dir fails"; fi
if "$script" v1.2.3 "$tmp/out2" "$tmp/does-not-exist" >/dev/null 2>&1; then fail "missing contract dir passed"; else ok "missing contract dir fails"; fi
if "$script" "" "$tmp/out3" "$repo/contract" >/dev/null 2>&1; then fail "empty tag passed"; else ok "empty tag fails"; fi

echo "malformed schemas must fail:"
mkdir -p "$tmp/bad-json" "$tmp/bad-draft"
echo 'not json' > "$tmp/bad-json/x.schema.json"
echo '{"type":"object"}' > "$tmp/bad-draft/x.schema.json"
if "$script" v1.2.3 "$tmp/out4" "$tmp/bad-json" >/dev/null 2>&1; then fail "invalid JSON passed"; else ok "invalid JSON fails"; fi
if "$script" v1.2.3 "$tmp/out5" "$tmp/bad-draft" >/dev/null 2>&1; then fail "missing \$schema passed"; else ok "missing draft-07 declaration fails"; fi

echo "healthy input must stage the files and a manifest:"
mkdir -p "$tmp/good"
echo "$schema" > "$tmp/good/b_method.schema.json"
echo "$schema" > "$tmp/good/a_method.schema.json"
echo "ignored" > "$tmp/good/README.md"
if "$script" v1.2.3 "$tmp/out6" "$tmp/good" >/dev/null 2>&1; then ok "synthetic contract stages"; else fail "synthetic contract failed"; fi
[ -f "$tmp/out6/a_method.schema.json" ] && [ -f "$tmp/out6/b_method.schema.json" ] && ok "schemas copied" || fail "schemas not copied"
[ ! -f "$tmp/out6/README.md" ] && ok "non-schema files are not published" || fail "README was published"
m="$tmp/out6/contract-manifest.json"
[ "$(jq -r .lcg_version "$m")" = "1.2.3" ] && ok "manifest carries the version without the v" || fail "wrong lcg_version"
[ "$(jq -r .tag "$m")" = "v1.2.3" ] && ok "manifest carries the tag" || fail "wrong tag"
[ "$(jq -r '.schemas | map(.method) | join(",")' "$m")" = "a_method,b_method" ] && ok "manifest lists schemas sorted" || fail "manifest schema list wrong"
want=$(cd "$tmp/good" && (sha256sum a_method.schema.json 2>/dev/null || shasum -a 256 a_method.schema.json) | cut -d' ' -f1)
[ "$(jq -r '.schemas[0].sha256' "$m")" = "$want" ] && ok "manifest sha256 matches the file" || fail "sha256 mismatch"

echo "the real contract/ must stage, covering every published method:"
if "$script" v0.0.0-test "$tmp/real" "$repo/contract" >/dev/null 2>&1; then ok "real contract stages"; else fail "real contract failed to stage"; fi
for method in health_check knowledge_status knowledge_lint knowledge_search_passages \
  knowledge_find_entities knowledge_find_relationships knowledge_process_chunk; do
  [ -f "$tmp/real/$method.schema.json" ] && ok "$method.schema.json staged" || fail "$method.schema.json missing"
done

echo
if [ "$failures" -ne 0 ]; then echo "$failures failure(s)"; exit 1; fi
echo "all stage-contract-assets checks passed"
