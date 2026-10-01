#!/usr/bin/env bash
# Tests for scripts/assert-no-compiling.sh (#657). Synthetic logs only; no cargo needed.
#
# The guard shipped blind once: cargo's coloured output has an ANSI reset between
# `Compiling` and the space, so `grep -q "Compiling "` never matched and CI printed
# "full cache hit" on a run that recompiled everything. Both directions are asserted, plus
# a negative control proving the old pattern really is blind to the coloured fixture.
#
# Run: scripts/test-assert-no-compiling.sh

# shellcheck disable=SC2016  # fixtures deliberately contain literal backticks/quotes
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
guard="$here/assert-no-compiling.sh"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

failures=0
esc=$'\033'

# expect <pass|fail> <description> <log content (printf format)>
expect() {
  local want="$1" why="$2" content="$3" got
  printf '%b' "$content" >"$tmp/log"
  if "$guard" "$tmp/log" >/dev/null 2>&1; then got=pass; else got=fail; fi
  if [ "$got" = "$want" ]; then
    printf '  ok    %-5s %s\n' "$want" "$why"
  else
    printf '  FAIL  wanted %s, got %s: %s\n' "$want" "$got" "$why"
    failures=$((failures + 1))
  fi
}

coloured="${esc}[1m${esc}[92m   Compiling${esc}[0m proc-macro2 v1.0.106\n"

echo "recompilation must be detected:"
expect fail 'coloured line (reset between Compiling and the space)' "$coloured"
expect fail 'plain line' '   Compiling proc-macro2 v1.0.0\n'
expect fail 'Compiling with no trailing text' '   Compiling\n'
expect fail 'one coloured line among normal output' \
  "   Fresh libc v0.2.1\n${coloured}    Finished release profile in 1s\n"

echo "a clean reuse must pass:"
expect pass 'Finished/Running/test-result lines only' \
  '    Finished `release` profile [optimized] target(s) in 0.4s\n     Running unittests src/lib.rs\ntest result: ok. 5 passed; 0 failed\n'
expect pass 'empty log' ''
expect pass "echoed script text (runner prints the step's own source)" \
  '  if grep -q "Compiling " /tmp/build.log; then\n'
expect pass 'coloured Finished line' "${esc}[1m${esc}[92m    Finished${esc}[0m release profile\n"

echo "the guard must not pass silently:"
if "$guard" "$tmp/does-not-exist" >/dev/null 2>&1; then
  echo "  FAIL  missing log file passed"; failures=$((failures + 1))
else
  echo "  ok    missing log file fails"
fi
if "$guard" >/dev/null 2>&1; then
  echo "  FAIL  no argument passed"; failures=$((failures + 1))
else
  echo "  ok    no argument fails"
fi

echo "negative control (documents the original blind spot):"
printf '%b' "$coloured" >"$tmp/log"
if grep -q "Compiling " "$tmp/log"; then
  echo "  FAIL  the old 'grep -q \"Compiling \"' matched the coloured fixture — fixture is not the real shape"
  failures=$((failures + 1))
else
  echo "  ok    old pattern is blind to the coloured fixture"
fi

if [ "$failures" -ne 0 ]; then
  echo "$failures failure(s)"
  exit 1
fi
echo "all passed"
