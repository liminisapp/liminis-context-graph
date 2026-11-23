#!/usr/bin/env bash
# Stage the published response JSON Schemas as release assets (#852, ADR-0852).
#
# Usage: scripts/stage-contract-assets.sh <tag> <dest-dir> [contract-dir]
#
# Copies contract/*.schema.json into <dest-dir> (release.yml's `artifacts/`, which
# `gh release create` attaches wholesale) and writes <dest-dir>/contract-manifest.json:
#
#   { "lcg_version": "0.19.1", "tag": "v0.19.1", "draft": "draft-07",
#     "schemas": [ { "method": "...", "file": "....schema.json", "sha256": "..." } ] }
#
# The committed schemas are version-free (so a version bump never churns them and never
# collides with the drift test); the manifest is where a release says which version they
# belong to (FR-009). It is generated here, never committed.
#
# Fails -- loudly, with the reason -- when the tag is empty, the contract directory holds no
# schemas, a schema is not valid JSON or does not declare draft-07, or the destination cannot
# be written. A release must never silently ship without its contract (SC-003).
set -euo pipefail

tag="${1:-}"
dest="${2:-}"
src="${3:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/contract}"

die() { echo "stage-contract-assets: $*" >&2; exit 1; }

[ -n "$tag" ] || die "missing <tag> argument"
[ -n "$dest" ] || die "missing <dest-dir> argument"
[ -d "$src" ] || die "contract directory not found: $src"
command -v jq >/dev/null || die "jq is required"

if command -v sha256sum >/dev/null 2>&1; then
  sha() { sha256sum "$1" | cut -d' ' -f1; }
else
  sha() { shasum -a 256 "$1" | cut -d' ' -f1; }
fi

shopt -s nullglob
files=("$src"/*.schema.json)
[ "${#files[@]}" -gt 0 ] || die "no *.schema.json files in $src -- refusing to publish a release without its contract"

mkdir -p "$dest"
version="${tag#v}"
entries=()
for f in "${files[@]}"; do
  name="$(basename "$f")"
  jq empty "$f" >/dev/null 2>&1 || die "$name is not valid JSON"
  [ "$(jq -r '."$schema" // ""' "$f")" = "http://json-schema.org/draft-07/schema#" ] \
    || die "$name does not declare the draft-07 meta-schema"
  cp "$f" "$dest/$name"
  entries+=("$(jq -n --arg m "${name%.schema.json}" --arg f "$name" --arg s "$(sha "$f")" \
    '{method: $m, file: $f, sha256: $s}')")
done

printf '%s\n' "${entries[@]}" \
  | jq -s --arg v "$version" --arg t "$tag" \
      '{lcg_version: $v, tag: $t, draft: "draft-07", schemas: (. | sort_by(.method))}' \
  > "$dest/contract-manifest.json"

echo "staged ${#files[@]} schema(s) + contract-manifest.json for $tag into $dest"
