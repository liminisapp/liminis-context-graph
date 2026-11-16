#!/usr/bin/env bash
# Tests for scripts/assert-plan-output.sh (#725). Synthetic inputs only; needs jq.
#
# Run: scripts/test-assert-plan-output.sh
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
guard="$here/assert-plan-output.sh"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

failures=0

# expect <pass|fail> <reason|-> <description> <PLAN_VAL content>
expect() {
  local want="$1" reason="$2" why="$3" content="$4" got out
  if out=$(PLAN_VAL="$content" GITHUB_STEP_SUMMARY="$tmp/summary" "$guard" 2>&1); then got=pass; else got=fail; fi
  if [ "$got" != "$want" ]; then
    printf '  FAIL  wanted %s, got %s: %s\n' "$want" "$got" "$why"
    failures=$((failures + 1))
    return
  fi
  if [ "$want" = fail ]; then
    case "$out" in
      *"reason: $reason"*) ;;
      *) printf '  FAIL  %s: expected reason %s in output\n' "$why" "$reason"; failures=$((failures + 1)); return ;;
    esac
    case "$out" in
      *masking*) ;;
      *) printf '  FAIL  %s: message does not name secret masking\n' "$why"; failures=$((failures + 1)); return ;;
    esac
  fi
  printf '  ok    %-5s %s\n' "$want" "$why"
}

echo "lost or malformed plan output must fail:"
expect fail empty 'empty string' ''
expect fail empty 'whitespace only' $'  \n\t'
expect fail no-matrix 'JSON null' 'null'
expect fail no-matrix 'empty object' '{}'
expect fail no-matrix 'no ci key' '{"announcement_title":"x"}'
expect fail no-matrix 'ci without matrix' '{"ci":{"github":{}}}'
expect fail no-matrix 'include is not an array' '{"ci":{"github":{"artifacts_matrix":{"include":null}}}}'
expect fail empty-matrix 'include is an empty array' '{"ci":{"github":{"artifacts_matrix":{"include":[]}}}}'
expect fail not-json 'not JSON' 'this is not json'

echo "healthy plan output must pass:"
expect pass - 'single entry' '{"ci":{"github":{"artifacts_matrix":{"include":[{"runner":"ubuntu-22.04"}]}}}}'
expect pass - 'multiple entries' '{"ci":{"github":{"artifacts_matrix":{"include":[{"runner":"a"},{"runner":"b"},{"runner":"c"}]}}}}'

echo "stdin input is honoured when PLAN_VAL is unset:"
if printf '%s' '{"ci":{"github":{"artifacts_matrix":{"include":[{"r":1}]}}}}' | env -u PLAN_VAL "$guard" >/dev/null 2>&1; then
  echo "  ok    stdin valid passes"
else
  echo "  FAIL  stdin valid should pass"; failures=$((failures + 1))
fi
if printf '' | env -u PLAN_VAL "$guard" >/dev/null 2>&1; then
  echo "  FAIL  stdin empty should fail"; failures=$((failures + 1))
else
  echo "  ok    stdin empty fails"
fi

echo "the input is never echoed:"
secret='Authorization: Bearer sekrit-token-123'
out=$(PLAN_VAL="$secret" GITHUB_STEP_SUMMARY="$tmp/summary2" "$guard" 2>&1)
if printf '%s' "$out" | grep -q 'sekrit-token-123' || grep -q 'sekrit-token-123' "$tmp/summary2"; then
  echo "  FAIL  value leaked into output or summary"; failures=$((failures + 1))
else
  echo "  ok    value not echoed"
fi
if grep -q 'masking' "$tmp/summary2"; then
  echo "  ok    step summary names secret masking"
else
  echo "  FAIL  step summary missing masking note"; failures=$((failures + 1))
fi

if [ "$failures" -ne 0 ]; then
  echo "$failures failure(s)"
  exit 1
fi
echo "all assert-plan-output tests passed"
