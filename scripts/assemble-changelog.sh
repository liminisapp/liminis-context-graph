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
#   * changelog.d/README.md is the only other thing allowed in the directory (it is ignored);
#     any other entry -- a subdirectory, .markdown, .MD, .md.txt, a mis-named file -- is an
#     error, so nothing can sit in changelog.d/ that the assembler would silently never read
#
# Assembly: every fragment is validated first (any error => exit 1, nothing modified). With no
# fragments the script exits 0 without touching CHANGELOG.md, so a second run is a no-op. With
# fragments, the current [Unreleased] body and the fragments (sorted by their numeric prefix, so
# 904-x.md comes before 1000-x.md; ties by file name) are regrouped to one heading per section in
# the fixed order below -- existing text first within a section -- and any heading in the existing
# body that is not a known section is kept after the known ones. The existing body is checked for
# balanced code fences first: a line starting `## [` inside a fence (an unclosed fence swallowing
# the next release looks exactly like this), or a fence still open at the end of the file, is an error (exit 1, nothing written) rather than folding later releases into
# [Unreleased].
#
# Portable: bash 3.2 + POSIX awk (macOS and Linux). No external dependencies.
set -euo pipefail
export LC_ALL=C

# Known sections, in assembly order. The order mirrors the released sections (0.18.1, 0.19.0,
# 0.19.1): the upgrade note leads the GitHub Release body, then Added, Fixed, Changed, and the
# known issues come last. Keep in sync with changelog.d/README.md and CONTRIBUTING.md.
KNOWN="Upgrading|Upgrade notes|Added|Fixed|Changed|Deprecated|Removed|Security|Response schemas|Documentation|Internal|Known issues|Windows notes"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CHANGELOG_FILE="${CHANGELOG_FILE:-$root/CHANGELOG.md}"
FRAGMENT_DIR="${FRAGMENT_DIR:-$root/changelog.d}"

mode=assemble
case "${1:-}" in
  "") ;;
  --check) mode=check ;;
  -h|--help) sed -n '2,37p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
  *) echo "usage: $0 [--check]" >&2; exit 2 ;;
esac

# --- shared awk: CommonMark-style fence tracking ---------------------------------------------------
# fenceline(line) -> 1 if the line opens or closes a code fence (updating infence/fchar/flen).
# A fence opens with 3+ backticks or tildes (a backtick opener may not have a backtick in its info
# string, so ```inline``` is not a fence) and closes only with the SAME character, at least as many
# of them, and nothing else on the line.
FENCE_AWK=$(cat <<'AWK'
function fenceline(line,    t, c, n, rest) {
  t = line; sub(/^ */, "", t)
  c = substr(t, 1, 1)
  if (c != "`" && c != "~") return 0
  n = 0
  while (substr(t, n + 1, 1) == c) n++
  if (n < 3) return 0
  rest = substr(t, n + 1)
  if (infence) {
    if (c == fchar && n >= flen && rest ~ /^[ \t]*$/) { infence = 0; return 1 }
    return 0
  }
  if (c == "`" && index(rest, "`") > 0) return 0
  infence = 1; fchar = c; flen = n
  return 1
}
AWK
)

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

  if (fenceline(line)) {
    if (cur == "") fail("text before the first ### section heading")
    else body = 1
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
  if (fileno == 1 && infence) {
    print "ERROR: CHANGELOG.md: [Unreleased] ends inside an unclosed code fence (fix the fence; nothing was written)" > "/dev/stderr"
    failed = 1; exit 1
  }
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
  if (fenceline(line)) { addline(line); next }
  if (infence && !frag && line ~ /^## \[/) {
    printf "ERROR: CHANGELOG.md: line %d: \"%s\" inside a code fence in [Unreleased]: a release heading in a fence is either an unclosed fence swallowing later releases or text that must not start a line with "## [" (nothing was written)\n", FNR, line > "/dev/stderr"
    failed = 1; exit 1
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
  if (failed) exit 1
  if (phase == "pre") { print "ERROR: no \"## [Unreleased]\" heading found in the changelog" > "/dev/stderr"; exit 3 }
  endpiece()
  emit()
}
AWK
)

# --- collect fragments -------------------------------------------------------------------------
# Everything in changelog.d/ must be README.md or a <number>-<slug>.md file. Anything else (a
# subdirectory, .markdown, .MD, .md.txt, a dotfile, a mis-named file) is an error: the assembler
# reads only changelog.d/<number>-<slug>.md, so a stray file would pass validation and then never
# be assembled or deleted. Fragments are ordered by numeric prefix (904 before 1000), then name.
NAME_RE='^[0-9]+-[a-z0-9]+(-[a-z0-9]+)*\.md$'
errors=0
keyed=""
if [ -d "$FRAGMENT_DIR" ]; then
  shopt -s dotglob nullglob
  for f in "$FRAGMENT_DIR"/*; do
    base="$(basename "$f")"
    case "$base" in .|..) continue ;; esac
    if [ -f "$f" ] && [ "$base" = "README.md" ]; then continue; fi
    if [ -f "$f" ] && printf '%s\n' "$base" | grep -Eq "$NAME_RE"; then
      num="${base%%-*}"
      keyed="${keyed}${num}"$'\t'"${f}"$'\n'
    else
      echo "ERROR: changelog.d/$base: not README.md or a <issue-or-PR-number>-<slug>.md file (slug: lowercase letters, digits, hyphens; no subdirectories); the assembler would never read it" >&2
      errors=$((errors + 1))
    fi
  done
  shopt -u dotglob nullglob
fi
frags=()
if [ -n "$keyed" ]; then
  while IFS=$'\t' read -r _ path; do
    frags+=("$path")
  done < <(printf '%s' "$keyed" | sort -t "$(printf '\t')" -k1,1n -k2,2)
fi

if [ "${#frags[@]}" -gt 0 ]; then
  for f in "${frags[@]}"; do
    base="$(basename "$f")"
    if ! awk -v known="$KNOWN" -v file="$base" "$FENCE_AWK"$'\n'"$VALIDATE_AWK" "$f"; then
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
if ! awk -v known="$KNOWN" "$FENCE_AWK"$'\n'"$ASSEMBLE_AWK" "$CHANGELOG_FILE" "${frags[@]}" > "$out"; then
  echo "ERROR: assembly failed; CHANGELOG.md and the fragments are untouched" >&2
  exit 1
fi
chmod 644 "$out"
mv "$out" "$CHANGELOG_FILE"
rm -f "${frags[@]}"
echo "assembled ${#frags[@]} fragment(s) into ${CHANGELOG_FILE#"$root"/} under [Unreleased]; fragments removed"
