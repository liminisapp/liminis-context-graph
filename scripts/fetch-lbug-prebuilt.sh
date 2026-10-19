#!/usr/bin/env bash
# fetch-lbug-prebuilt.sh — prefetch the pinned prebuilt static liblbug with retries, so a single
# transient upstream 5xx cannot fail a release build (issue #690, ADR-0690).
#
# Usage:
#   scripts/fetch-lbug-prebuilt.sh <target-triple> <cache-dir>
#
#   <target-triple>  a cargo-dist release target (aarch64-apple-darwin, x86_64-unknown-linux-gnu,
#                    aarch64-unknown-linux-gnu, x86_64-pc-windows-msvc). Unmapped triples die.
#   <cache-dir>      becomes LBUG_PREBUILT_CACHE_DIR. The prebuilt is placed at
#                    <cache-dir>/<key>/lib/{liblbug.a|lbug.lib,lbug.h,lbug.hpp}.
#
# WHY THIS EXISTS
#
# The `lbug` crate's build.rs fetches the prebuilt with one attempt and no retry, then silently
# falls back to a source build, which cannot succeed on the macOS release runner (it needs
# std::atomic_ref, which Xcode 15.4's libc++ lacks). One HTTP 500 therefore failed Release run
# 37319403247 with five unrelated-looking compile errors. build.rs skips its own download when
# <LBUG_PREBUILT_CACHE_DIR>/<key>/lib/<lib> already exists, so populating that path first — with
# retries, and a clear failure naming the URL — closes the hole.
#
# <key> MUST match build.rs's prebuilt_cache_key(): `version-<LBUG_VERSION>`, plus `-compat` on
# Linux only (LBUG_LINUX_VARIANT defaults to `compat`). If the two drift, build.rs ignores the
# prefetch and downloads again, silently — scripts/test-fetch-lbug-prebuilt.sh pins the shape.
#
# The version is read from .cargo/config.toml (the single source of truth). This script never
# invokes the crate's download_lbug.sh / download-liblbug.sh, so it has no dependency on a
# script fetched from LadybugDB's `main` branch.
#
# Bash 3.2-safe (macOS /bin/bash): no arrays, no mapfile.
#
# Test-only environment overrides:
#   LBUG_FETCH_URL_OVERRIDE   space-separated URLs; attempt N uses the Nth (the last repeats)
#   LBUG_FETCH_BACKOFF_BASE   seconds; delay before retry k is base * 2^(k-1) (default 2)
#   LBUG_FETCH_ATTEMPTS       attempts before giving up (default 5)
#   LBUG_FETCH_CONFIG         path to the config.toml to read LBUG_VERSION from

set -euo pipefail

die() { echo "fetch-lbug-prebuilt.sh: $*" >&2; exit 1; }
note() { echo "fetch-lbug-prebuilt.sh: $*" >&2; }

[[ $# -eq 2 ]] || die "usage: $0 <target-triple> <cache-dir>"
triple="$1"
cache_dir="$2"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
config="${LBUG_FETCH_CONFIG:-$here/../.cargo/config.toml}"
[[ -f "$config" ]] || die "config not found: $config"

version="$(sed -n 's/^LBUG_VERSION[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$config" | head -n1)"
[[ -n "$version" ]] || die "could not parse LBUG_VERSION from $config (refusing to fetch 'latest')"
case "$version" in
  *[!0-9A-Za-z._-]*) die "unexpected characters in LBUG_VERSION '$version'" ;;
esac

# Target triple -> upstream asset. A new release target needs a row here.
linux_suffix=""
case "$triple" in
  aarch64-apple-darwin) asset="liblbug-static-osx-arm64.tar.gz"; libname="liblbug.a" ;;
  x86_64-unknown-linux-gnu) asset="liblbug-static-linux-x86_64-compat.tar.gz"; libname="liblbug.a"; linux_suffix="-compat" ;;
  aarch64-unknown-linux-gnu) asset="liblbug-static-linux-aarch64-compat.tar.gz"; libname="liblbug.a"; linux_suffix="-compat" ;;
  x86_64-pc-windows-msvc) asset="liblbug-static-windows-x86_64.zip"; libname="lbug.lib" ;;
  *) die "no liblbug prebuilt mapping for target '$triple' — add it to scripts/fetch-lbug-prebuilt.sh" ;;
esac

key="version-${version}${linux_suffix}"
lib_dir="$cache_dir/$key/lib"
default_url="https://github.com/LadybugDB/ladybug/releases/download/v${version}/${asset}"

# complete <dir> — the library and both headers exist and are non-empty.
complete() {
  [[ -s "$1/$libname" && -s "$1/lbug.h" && -s "$1/lbug.hpp" ]]
}

export_env() {
  local abs
  abs="$(cd "$cache_dir" && pwd)"
  if command -v cygpath >/dev/null 2>&1; then
    abs="$(cygpath -m "$abs")"
  fi
  note "LBUG_PREBUILT_CACHE_DIR=$abs (library: $abs/$key/lib/$libname)"
  if [[ -n "${GITHUB_ENV:-}" ]]; then
    echo "LBUG_PREBUILT_CACHE_DIR=$abs" >>"$GITHUB_ENV"
  fi
}

mkdir -p "$cache_dir"

if complete "$lib_dir"; then
  note "cache hit, no download ($lib_dir)"
  export_env
  exit 0
fi
if [[ -e "$lib_dir" ]]; then
  note "cache entry at $lib_dir is incomplete or empty; discarding and re-fetching"
  rm -rf "${cache_dir:?}/${key:?}"
fi

command -v curl >/dev/null 2>&1 || die "curl not found on PATH"
case "$asset" in
  *.zip) command -v unzip >/dev/null 2>&1 || die "unzip not found on PATH (needed for $asset)" ;;
esac

attempts="${LBUG_FETCH_ATTEMPTS:-5}"
backoff="${LBUG_FETCH_BACKOFF_BASE:-2}"
urls="${LBUG_FETCH_URL_OVERRIDE:-$default_url}"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# nth_url <n> — the nth space-separated URL, or the last one if n is past the end.
nth_url() {
  local n="$1" u last=""
  for u in $urls; do
    last="$u"
    n=$((n - 1))
    [[ $n -le 0 ]] && { echo "$u"; return; }
  done
  echo "$last"
}

last_url=""
attempt=1
while [[ $attempt -le $attempts ]]; do
  url="$(nth_url "$attempt")"
  last_url="$url"
  note "attempt $attempt/$attempts: $url"
  rm -rf "$work/archive" "$work/extract"
  mkdir -p "$work/extract"
  if curl -fsSL --connect-timeout 30 --max-time 600 "$url" -o "$work/archive" \
     && { case "$asset" in
            *.zip) unzip -q -o "$work/archive" -d "$work/extract" ;;
            *) tar -xzf "$work/archive" -C "$work/extract" ;;
          esac; } \
     && complete "$work/extract"; then
    # Stage atomically: never leave a partial <key>/lib behind for build.rs to trust.
    mkdir -p "$cache_dir/$key"
    rm -rf "$lib_dir"
    mv "$work/extract" "$lib_dir"
    note "cache miss, downloaded $url"
    export_env
    exit 0
  fi
  note "attempt $attempt/$attempts failed for $url"
  if [[ $attempt -lt $attempts ]]; then
    delay=$((backoff * (1 << (attempt - 1))))
    note "retrying in ${delay}s"
    sleep "$delay"
  fi
  attempt=$((attempt + 1))
done

die "could not fetch $last_url after $attempts attempts (target $triple, LBUG_VERSION $version); not falling back to a source build, which cannot succeed on the macOS release runner"
