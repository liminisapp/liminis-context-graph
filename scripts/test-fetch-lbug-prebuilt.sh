#!/usr/bin/env bash
# Tests for scripts/fetch-lbug-prebuilt.sh (#690). Offline: file:// URLs and synthetic archives.
#
# Covers the retry contract (SC-001), the fail-fast-naming-the-URL contract (SC-004), the cache
# hit path (SC-002), and the key shape that must match lbug's build.rs prebuilt_cache_key().
#
# Run: scripts/test-fetch-lbug-prebuilt.sh

set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/fetch-lbug-prebuilt.sh"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

failures=0
ok() { printf '  ok    %s\n' "$1"; }
bad() { printf '  FAIL  %s\n' "$1"; failures=$((failures + 1)); }

# Synthetic archive with the flat layout the upstream tarballs use.
mkdir "$tmp/src"
printf 'lib' >"$tmp/src/liblbug.a"
printf 'lib' >"$tmp/src/lbug.lib"
printf 'h' >"$tmp/src/lbug.h"
printf 'hpp' >"$tmp/src/lbug.hpp"
tar -czf "$tmp/good.tar.gz" -C "$tmp/src" liblbug.a lbug.h lbug.hpp
mkdir "$tmp/empty-src"
: >"$tmp/empty-src/liblbug.a"
printf 'h' >"$tmp/empty-src/lbug.h"
printf 'hpp' >"$tmp/empty-src/lbug.hpp"
tar -czf "$tmp/empty-lib.tar.gz" -C "$tmp/empty-src" liblbug.a lbug.h lbug.hpp
printf 'not a tarball' >"$tmp/garbage.tar.gz"

cfg="$tmp/config.toml"
printf '# LBUG_VERSION = "9.9.9" in a comment must be ignored\n[env]\nLBUG_VERSION = "1.2.3"\n' >"$cfg"

export LBUG_FETCH_CONFIG="$cfg"
export LBUG_FETCH_BACKOFF_BASE=0
unset GITHUB_ENV LBUG_FETCH_URL_OVERRIDE LBUG_FETCH_ATTEMPTS

# run <triple> <cache> — leaves stdout+stderr in $tmp/out, status in $rc.
run() {
  "$script" "$1" "$2" >"$tmp/out" 2>&1
  rc=$?
}

good="file://$tmp/good.tar.gz"
missing="file://$tmp/does-not-exist.tar.gz"

echo "fetch + key shape:"
rm -rf "$tmp/c1"
LBUG_FETCH_URL_OVERRIDE="$good" run aarch64-apple-darwin "$tmp/c1"
if [ $rc -eq 0 ] && [ -s "$tmp/c1/version-1.2.3/lib/liblbug.a" ]; then
  ok 'macOS: key is version-<v> with no suffix, version parsed past the comment'
else bad "macOS fetch (rc=$rc): $(cat "$tmp/out")"; fi

rm -rf "$tmp/c2"
LBUG_FETCH_URL_OVERRIDE="$good" run x86_64-unknown-linux-gnu "$tmp/c2"
if [ $rc -eq 0 ] && [ -s "$tmp/c2/version-1.2.3-compat/lib/liblbug.a" ] && [ ! -e "$tmp/c2/version-1.2.3" ]; then
  ok 'linux x86_64: key carries the -compat suffix'
else bad "linux x86_64 fetch (rc=$rc): $(cat "$tmp/out")"; fi

rm -rf "$tmp/c2b"
LBUG_FETCH_URL_OVERRIDE="$good" run aarch64-unknown-linux-gnu "$tmp/c2b"
if [ $rc -eq 0 ] && [ -s "$tmp/c2b/version-1.2.3-compat/lib/liblbug.a" ]; then
  ok 'linux aarch64: key carries the -compat suffix'
else bad "linux aarch64 fetch (rc=$rc): $(cat "$tmp/out")"; fi

if command -v zip >/dev/null 2>&1; then
  (cd "$tmp/src" && zip -q "$tmp/good.zip" lbug.lib lbug.h lbug.hpp)
  rm -rf "$tmp/c2c"
  LBUG_FETCH_URL_OVERRIDE="file://$tmp/good.zip" run x86_64-pc-windows-msvc "$tmp/c2c"
  if [ $rc -eq 0 ] && [ -s "$tmp/c2c/version-1.2.3/lib/lbug.lib" ]; then
    ok 'windows: zip extracted, lbug.lib expected, no suffix'
  else bad "windows fetch (rc=$rc): $(cat "$tmp/out")"; fi
else
  echo "  skip  windows zip case (no zip binary)"
fi

echo "retry (SC-001):"
rm -rf "$tmp/c3"
LBUG_FETCH_URL_OVERRIDE="$missing $good" run aarch64-apple-darwin "$tmp/c3"
if [ $rc -eq 0 ] && [ -s "$tmp/c3/version-1.2.3/lib/lbug.h" ] && grep -q 'attempt 2/5' "$tmp/out"; then
  ok 'bad URL on attempt 1, attempt 2 succeeds'
else bad "retry (rc=$rc): $(cat "$tmp/out")"; fi

rm -rf "$tmp/c3b"
LBUG_FETCH_URL_OVERRIDE="file://$tmp/garbage.tar.gz $good" run aarch64-apple-darwin "$tmp/c3b"
if [ $rc -eq 0 ] && grep -q 'attempt 2/5' "$tmp/out"; then
  ok 'corrupt archive on attempt 1 is a failed attempt, not a success'
else bad "corrupt archive (rc=$rc): $(cat "$tmp/out")"; fi

rm -rf "$tmp/c3c"
LBUG_FETCH_URL_OVERRIDE="file://$tmp/empty-lib.tar.gz $good" run aarch64-apple-darwin "$tmp/c3c"
if [ $rc -eq 0 ] && [ -s "$tmp/c3c/version-1.2.3/lib/liblbug.a" ]; then
  ok 'zero-byte library on attempt 1 is a failed attempt'
else bad "empty library (rc=$rc): $(cat "$tmp/out")"; fi

echo "fail fast (SC-004):"
rm -rf "$tmp/c4"
LBUG_FETCH_URL_OVERRIDE="$missing" LBUG_FETCH_ATTEMPTS=3 run aarch64-apple-darwin "$tmp/c4"
if [ $rc -ne 0 ] && grep -q "could not fetch $missing after 3 attempts" "$tmp/out" \
   && [ ! -e "$tmp/c4/version-1.2.3/lib" ]; then
  ok 'all attempts fail: non-zero, names the URL, leaves no partial lib dir'
else bad "all-fail (rc=$rc): $(cat "$tmp/out")"; fi

echo "cache (SC-002):"
LBUG_FETCH_URL_OVERRIDE="$missing" LBUG_FETCH_ATTEMPTS=1 run aarch64-apple-darwin "$tmp/c1"
if [ $rc -eq 0 ] && grep -q 'cache hit, no download' "$tmp/out" && ! grep -q 'attempt 1' "$tmp/out"; then
  ok 'complete cache entry short-circuits with no download'
else bad "cache hit (rc=$rc): $(cat "$tmp/out")"; fi

rm -rf "$tmp/c5"
mkdir -p "$tmp/c5/version-1.2.3/lib"
: >"$tmp/c5/version-1.2.3/lib/liblbug.a"
LBUG_FETCH_URL_OVERRIDE="$good" run aarch64-apple-darwin "$tmp/c5"
if [ $rc -eq 0 ] && [ -s "$tmp/c5/version-1.2.3/lib/liblbug.a" ] && grep -q 'incomplete' "$tmp/out"; then
  ok 'truncated cache entry is discarded and re-fetched'
else bad "truncated cache (rc=$rc): $(cat "$tmp/out")"; fi

echo "GITHUB_ENV export:"
rm -rf "$tmp/c6"; : >"$tmp/ghenv"
GITHUB_ENV="$tmp/ghenv" LBUG_FETCH_URL_OVERRIDE="$good" run aarch64-apple-darwin "$tmp/c6"
if [ $rc -eq 0 ] && grep -q '^LBUG_PREBUILT_CACHE_DIR=/' "$tmp/ghenv"; then
  ok 'writes an absolute LBUG_PREBUILT_CACHE_DIR'
else bad "GITHUB_ENV (rc=$rc): $(cat "$tmp/ghenv")"; fi

echo "loud failures:"
run riscv64-unknown-linux-gnu "$tmp/c7"
if [ $rc -ne 0 ] && grep -q "no liblbug prebuilt mapping for target 'riscv64" "$tmp/out"; then
  ok 'unmapped triple dies'
else bad "unmapped triple (rc=$rc): $(cat "$tmp/out")"; fi

printf '[env]\nOTHER = "x"\n' >"$tmp/noversion.toml"
LBUG_FETCH_CONFIG="$tmp/noversion.toml" run aarch64-apple-darwin "$tmp/c8"
if [ $rc -ne 0 ] && grep -q 'could not parse LBUG_VERSION' "$tmp/out"; then
  ok 'missing LBUG_VERSION dies instead of fetching latest'
else bad "missing version (rc=$rc): $(cat "$tmp/out")"; fi

echo "the real config parses:"
LBUG_FETCH_CONFIG="$here/../.cargo/config.toml" LBUG_FETCH_URL_OVERRIDE="$missing" LBUG_FETCH_ATTEMPTS=1 \
  run aarch64-apple-darwin "$tmp/c9"
real="$(sed -n 's/^LBUG_VERSION *= *"\(.*\)"/\1/p' "$here/../.cargo/config.toml")"
if [ -n "$real" ] && grep -q "after 1 attempts (target aarch64-apple-darwin, LBUG_VERSION $real)" "$tmp/out"; then
  ok "repo .cargo/config.toml yields LBUG_VERSION $real"
else bad "real config (rc=$rc): $(cat "$tmp/out")"; fi

if [ $failures -ne 0 ]; then
  echo "$failures failure(s)"
  exit 1
fi
echo "all passed"
