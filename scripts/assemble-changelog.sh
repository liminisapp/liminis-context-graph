#!/usr/bin/env bash
# Changelog fragments (#904, ADR-0904): validate and assemble changelog.d/ into CHANGELOG.md.
#
# Every PR records its user-visible change as ONE new file, changelog.d/<issue>-<slug>.md,
# instead of editing the shared `## [Unreleased]` block of CHANGELOG.md. Concurrent PRs then
# touch disjoint files, so the merge train does not see them as overlapping. The release PR
# runs this script once, before renaming `## [Unreleased]` to the version.
#
# Usage:
#   scripts/assemble-changelog.sh --check    validate every fragment; write nothing
#   scripts/assemble-changelog.sh            merge fragments into [Unreleased]; delete them
#
# Environment (used by the self-test, rarely needed otherwise):
#   CHANGELOG_FILE   default <repo>/CHANGELOG.md
#   FRAGMENT_DIR     default <repo>/changelog.d
#
# Fragment format (see changelog.d/README.md):
#   * file name  <digits>-<slug>.md  (slug: lowercase letters, digits, hyphens)
#   * one or more `### <Section>` headings, each followed by at least one non-blank line
#   * only blank lines before the first heading; no `#` / `##` headings
#   * entry text is copied verbatim (multi-line and nested bullets are fine); heading-like
#     lines inside ``` / ~~~ fences are text, not headings
#   * changelog.d/README.md and any non-.md file are ignored
#
# Assembly: every fragment is validated first (any error => exit 1, nothing modified). With no
# fragments the script exits 0 without touching CHANGELOG.md, so a second run is a no-op. With
# fragments, the current [Unreleased] body and the fragments (sorted by file name) are regrouped
# to one heading per section in the fixed order below -- existing text first within a section --
# and any heading in the existing body that is not a known section is kept after the known ones.
#
# Portable: bash 3.2 + POSIX awk (macOS and Linux). No external dependencies.
set -euo pipefail
export LC_ALL=C

# Known sections, in assembly order. Keep in sync with changelog.d/README.md and CONTRIBUTING.md.
KNOWN="Added|Changed|Deprecated|Removed|Fixed|Security|Response schemas|Documentation|Internal|Upgrading|Upgrade notes|Known issues|Windows notes"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CHANGELOG_FILE="${CHANGELOG_FILE:-$root/CHANGELOG.md}"
FRAGMENT_DIR="${FRAGMENT_DIR:-$root/changelog.d}"

mode=assemble
case "${1:-}" in
  "") ;;
  --check) mode=check ;;
  -h|--help) sed -n '2,31p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
  *) echo "usage: $0 [--check]" >&2; exit 2 ;;
esac

# --- per-fragment validation (awk) -------------------------------------------------------------
VALIDATE_AWK=$(cat <<'AWK'
function fail(msg) { printf "ERROR: %s: line %d: %s\n", file, FNR, msg > "/dev/stderr"; bad = 1 }
function close_section() {
  if (cur != "" && !body) {
    printf "ERROR: %s: section \"%s\" has no entries\n", file, cur > "/dev/stderr"; bad = 1
  }
}
BEGIN {
  nk = split(known, K, "|")
  for (i = 1; i <= nk; i++) ok[K[i]] = 1
}
{
  sub(/\r$/, "")
  line = $0
  isblank = (line ~ /^[ \t]*$/)

  if (line ~ /^ *(```|~~~)/) {
    marker = substr(line, match(line, /[`~]/), 1)
    if (!infence) { infence = 1; fencechar = marker }
    else if (marker == fencechar) infence = 0
    if (cur == "" && !isblank) { fail("text before the first ### section heading") }
    else if (cur != "") body = 1
    next
  }
  if (infence) {
    if (cur == "") fail("text before the first ### section heading")
    else if (!isblank) body = 1
    next
  }
  if (line ~ /^#[ \t]/ || line ~ /^##[ \t]/ || line == "#" || line == "##") {
    fail("level-1/level-2 headings are not allowed in a fragment (use ### <Section>)")
    next
  }
  if (line ~ /^###[ \t]/ || line == "###") {
    close_section()
    name = line; sub(/^###[ \t]*/, "", name); sub(/[ \t]+$/, "", name)
    if (name == "") { fail("empty section heading"); cur = "\001"; body = 1; next }
    if (!(name in ok)) {
      fail("unknown section \"" name "\" (known: " known ")")
    }
    cur = name; body = 0; headings++
    next
  }
  if (isblank) next
  if (cur == "") { fail("text before the first ### section heading"); next }
  body = 1
}
END {
  if (infence) { printf "ERROR: %s: unterminated code fence\n", file > "/dev/stderr"; bad = 1 }
  if (headings == 0 && !bad) { printf "ERROR: %s: no ### section heading found\n", file > "/dev/stderr"; bad = 1 }
  if (headings > 0) close_section()
  exit bad ? 1 : 0
}
AWK
)

# --- assembly (awk): CHANGELOG.md first, then the fragments, in order ---------------------------
ASSEMBLE_AWK=$(cat <<'AWK'
function addline(l) {
  if (l ~ /^[ \t]*$/) { pend++; return }
  if (piece == "" && !havepiece) { piece = l; havepiece = 1 }
  else { for (j = 0; j < pend; j++) piece = piece "\n"; piece = piece "\n" l }
  pend = 0
}
function endpiece() {
  if (havepiece) {
    if (!(cur in reg)) {
      reg[cur] = 1
      if (cur != "" && !(cur in isknown)) extra[++ne] = cur
    }
    if (cur in txt) txt[cur] = txt[cur] "\n\n" piece
    else txt[cur] = piece
  }
  piece = ""; havepiece = 0; pend = 0
}
function emit(   i) {
  if ("" in txt) { print ""; print txt[""] }
  for (i = 1; i <= nk; i++) if (K[i] in txt) { print ""; print "### " K[i]; print ""; print txt[K[i]] }
  for (i = 1; i <= ne; i++) { print ""; print "### " extra[i]; print ""; print txt[extra[i]] }
  if (np > 0) { print ""; for (i = 1; i <= np; i++) print postl[i] }
}
BEGIN {
  nk = split(known, K, "|")
  for (i = 1; i <= nk; i++) isknown[K[i]] = 1
  phase = "pre"
}
FNR == 1 {
  if (fileno >= 1) endpiece()
  cur = ""; infence = 0
  fileno++
  frag = (fileno > 1)
}
{
  sub(/\r$/, "")
  line = $0
  if (!frag && phase == "pre") {
    print line
    if (line ~ /^## \[Unreleased\]/) { phase = "body"; cur = ""; infence = 0 }
    next
  }
  if (!frag && phase == "post") { postl[++np] = line; next }
  # body of the existing [Unreleased] section, or a fragment
  if (line ~ /^ *(```|~~~)/) {
    marker = substr(line, match(line, /[`~]/), 1)
    if (!infence) { infence = 1; fencechar = marker }
    else if (marker == fencechar) infence = 0
    addline(line); next
  }
  if (!infence) {
    if (!frag && line ~ /^## /) { endpiece(); phase = "post"; postl[++np] = line; next }
    if (line ~ /^###[ \t]/ || line == "###") {
      endpiece()
      cur = line; sub(/^###[ \t]*/, "", cur); sub(/[ \t]+$/, "", cur)
      next
    }
  }
  addline(line)
}
END {
  if (phase == "pre") { print "ERROR: no \"## [Unreleased]\" heading found in the changelog" > "/dev/stderr"; exit 3 }
  endpiece()
  emit()
}
AWK
)

# --- collect fragments -------------------------------------------------------------------------
frags=()
if [ -d "$FRAGMENT_DIR" ]; then
  for f in "$FRAGMENT_DIR"/*.md; do
    [ -f "$f" ] || continue
    base="$(basename "$f")"
    [ "$base" = "README.md" ] && continue
    frags+=("$f")
  done
fi

errors=0
if [ "${#frags[@]}" -gt 0 ]; then
  for f in "${frags[@]}"; do
    base="$(basename "$f")"
    if ! printf '%s\n' "$base" | grep -Eq '^[0-9]+-[a-z0-9]+(-[a-z0-9]+)*\.md$'; then
      echo "ERROR: $base: file name must be <issue-or-PR-number>-<slug>.md (slug: lowercase letters, digits, hyphens)" >&2
      errors=$((errors + 1))
    fi
    if ! awk -v known="$KNOWN" -v file="$base" "$VALIDATE_AWK" "$f"; then
      errors=$((errors + 1))
    fi
  done
fi

if [ "$errors" -gt 0 ]; then
  echo "changelog fragments invalid ($errors file(s) with problems); nothing was changed. See changelog.d/README.md." >&2
  exit 1
fi

if [ "${#frags[@]}" -eq 0 ]; then
  echo "no changelog fragments in ${FRAGMENT_DIR#"$root"/}; nothing to do"
  exit 0
fi

if [ "$mode" = check ]; then
  echo "${#frags[@]} changelog fragment(s) valid"
  exit 0
fi

# --- assemble ----------------------------------------------------------------------------------
[ -f "$CHANGELOG_FILE" ] || { echo "ERROR: $CHANGELOG_FILE not found" >&2; exit 2; }
out="$(mktemp "$(dirname "$CHANGELOG_FILE")/.assemble-changelog.XXXXXX")"
trap 'rm -f "$out"' EXIT
if ! awk -v known="$KNOWN" "$ASSEMBLE_AWK" "$CHANGELOG_FILE" "${frags[@]}" > "$out"; then
  echo "ERROR: assembly failed; CHANGELOG.md and the fragments are untouched" >&2
  exit 1
fi
chmod 644 "$out"
mv "$out" "$CHANGELOG_FILE"
rm -f "${frags[@]}"
echo "assembled ${#frags[@]} fragment(s) into ${CHANGELOG_FILE#"$root"/} under [Unreleased]; fragments removed"
