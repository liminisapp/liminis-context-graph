#!/usr/bin/env bash
# Tests for scripts/assemble-changelog.sh (#904). Runs on throwaway fixtures; never touches the
# real CHANGELOG.md or changelog.d/.
#
# Run: scripts/test-assemble-changelog.sh
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/assemble-changelog.sh"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
failures=0

pass() { printf '  ok    %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; failures=$((failures + 1)); }

# fresh <name>: a clean fixture dir with CHANGELOG.md + changelog.d/; sets $d, $cl, $fd
fresh() {
  d="$tmp/$1"; cl="$d/CHANGELOG.md"; fd="$d/changelog.d"
  rm -rf "$d"; mkdir -p "$fd"
}
run() { CHANGELOG_FILE="$cl" FRAGMENT_DIR="$fd" "$script" "$@"; }

# A changelog shaped like the real [Unreleased]: duplicate headings, non-standard sections.
write_changelog() {
  cat > "$cl" <<'EOF'
# Changelog

Intro text.

## [Unreleased]

### Fixed

- existing fix one
  continued line

### Response schemas

- existing schema entry
  - nested bullet

### Added

- existing added one

### Fixed

- existing fix two (second Fixed block)

### Upgrading

- existing upgrading note

## [1.0.0] - 2026-01-01

### Added

- released thing

[Unreleased]: https://example.invalid/compare/v1.0.0...HEAD
EOF
}

echo "assemble-changelog self-test"

# --- 1. grouping, order, mixed sources --------------------------------------------------------
fresh mixed; write_changelog
cat > "$fd/20-b.md" <<'EOF'
### Changed

- frag changed (20)

### Added

- frag added (20)
EOF
cat > "$fd/10-a.md" <<'EOF'
### Added

- frag added (10)
  - nested under (10)

### Known issues

- frag known issue

### Fixed

- frag fixed (10)
EOF
before_released="$(sed -n '/^## \[1.0.0\]/,$p' "$cl")"
if run >/dev/null 2>"$tmp/err"; then pass "assemble succeeds"; else fail "assemble failed: $(cat "$tmp/err")"; fi
heads="$(sed -n '/^## \[Unreleased\]/,/^## \[1.0.0\]/p' "$cl" | grep '^### ' | tr '\n' '|')"
want='### Added|### Changed|### Fixed|### Response schemas|### Upgrading|### Known issues|'
[ "$heads" = "$want" ] && pass "one heading per section, canonical order" || fail "headings: $heads"
added="$(awk '/^## \[1.0.0\]/{exit} /^### Added/{f=1;next} /^### /{f=0} f' "$cl")"
expected_added='
- existing added one

- frag added (10)
  - nested under (10)

- frag added (20)'
[ "$(printf '%s' "$added" | sed '/^$/d')" = "$(printf '%s' "$expected_added" | sed '/^$/d')" ] \
  && pass "existing text first, then fragments in file-name order" || fail "Added block: $added"
fixed_n="$(grep -c 'existing fix' "$cl")"
[ "$fixed_n" = 2 ] && grep -q 'second Fixed block' "$cl" && pass "duplicate existing blocks merged without loss" || fail "fixed entries lost"
grep -q '^  continued line' "$cl" && grep -q '^  - nested bullet' "$cl" && pass "multi-line and nested entries preserved" || fail "nesting lost"
[ "$(sed -n '/^## \[1.0.0\]/,$p' "$cl")" = "$before_released" ] && pass "released sections and link refs untouched" || fail "released section changed"
[ ! -e "$fd/10-a.md" ] && [ ! -e "$fd/20-b.md" ] && pass "fragments deleted" || fail "fragments remain"
[ "$(grep -c '^## \[Unreleased\]' "$cl")" = 1 ] && pass "single [Unreleased] heading" || fail "Unreleased count"
# blank-line shape: exactly one blank between sections, none doubled
if awk 'prev=="" && $0=="" && NR>1 {bad=1} {prev=$0} END{exit bad}' "$cl"; then pass "no doubled blank lines"; else fail "doubled blank lines"; fi

# --- 2. idempotence ----------------------------------------------------------------------------
cp "$cl" "$tmp/after1"
if run >/dev/null 2>&1 && cmp -s "$tmp/after1" "$cl"; then pass "second run is a no-op (exit 0, no diff)"; else fail "second run changed the file"; fi

# --- 3. end to end: assemble, then rename, then extract the version section like release does --
extract_version_section() { awk -v v="$2" '$0 ~ "^## \\[" v "\\]" {f=1; print; next} /^## \[/ {f=0} f' "$1"; }
sed -i.bak 's/^## \[Unreleased\]/## [1.1.0] - 2026-02-02/' "$cl" && rm -f "$cl.bak"
sec="$(extract_version_section "$cl" 1.1.0)"
if printf '%s\n' "$sec" | grep -q 'frag added (10)' && printf '%s\n' "$sec" | grep -q 'existing upgrading note' \
  && printf '%s\n' "$sec" | grep -q 'frag known issue' && ! printf '%s\n' "$sec" | grep -q 'released thing'; then
  pass "renamed section carries fragment and existing entries, and only those"
else fail "version section extraction: $sec"; fi

# --- 4. transition: fragment-only entries in an empty [Unreleased] ------------------------------
fresh empty
printf '# Changelog\n\n## [Unreleased]\n\n## [1.0.0] - 2026-01-01\n\n### Added\n\n- old\n' > "$cl"
printf '### Added\n\n- only fragment\n' > "$fd/5-only.md"
run >/dev/null 2>&1 \
  && [ "$(sed -n '/^## \[Unreleased\]/,/^## \[1.0.0\]/p' "$cl" | grep -c 'only fragment')" = 1 ] \
  && grep -q '^- old' "$cl" && pass "fragment into an empty [Unreleased]" || fail "empty Unreleased: $(cat "$cl")"

# --- 5. [Unreleased] is the last section --------------------------------------------------------
fresh last
printf '# Changelog\n\n## [Unreleased]\n\n### Added\n\n- keep\n' > "$cl"
printf '### Fixed\n\n- new\n' > "$fd/6-x.md"
run >/dev/null 2>&1 && grep -q '^### Fixed' "$cl" && grep -q '^- keep' "$cl" && [ -z "$(tail -c1 "$cl")" ] \
  && pass "[Unreleased] as the last section" || fail "last-section case"

# --- 6. no fragments: no-op, even for a nonstandard changelog ------------------------------------
fresh none; write_changelog; cp "$cl" "$tmp/orig"
if run >/dev/null 2>&1 && cmp -s "$tmp/orig" "$cl"; then pass "no fragments: file untouched (existing duplicates not reordered)"; else fail "no-op violated"; fi
rm -rf "$fd"
if run >/dev/null 2>&1; then pass "missing changelog.d/ is not an error"; else fail "missing dir errored"; fi
fresh none2; write_changelog
if run --check >/dev/null 2>&1; then pass "--check with no fragments passes"; else fail "--check empty"; fi

# --- 7. non-fragment files ignored --------------------------------------------------------------
fresh ignore; write_changelog
echo 'format note: ### Bogus' > "$fd/README.md"; : > "$fd/.gitkeep"; echo x > "$fd/notes.txt"
printf '### Added\n\n- real\n' > "$fd/7-real.md"
run >/dev/null 2>&1 && [ -e "$fd/README.md" ] && [ -e "$fd/.gitkeep" ] && [ -e "$fd/notes.txt" ] && [ ! -e "$fd/7-real.md" ] \
  && ! grep -q Bogus "$cl" && pass "README.md and non-.md files ignored and kept" || fail "ignore rules"

# --- 8. --check never writes ---------------------------------------------------------------------
fresh check; write_changelog; cp "$cl" "$tmp/orig"
printf '### Added\n\n- x\n' > "$fd/8-x.md"
run --check >/dev/null 2>&1 && cmp -s "$tmp/orig" "$cl" && [ -e "$fd/8-x.md" ] \
  && pass "--check validates without writing or deleting" || fail "--check wrote"

# --- 9. invalid fragments: each fails, names the file, changes nothing ---------------------------
bad() { # <label> <file-name> <content> <message regex>
  local label="$1" name="$2" content="$3" rx="$4"
  fresh bad; write_changelog; cp "$cl" "$tmp/orig"
  printf '### Added\n\n- good one\n' > "$fd/1-good.md"
  printf '%b' "$content" > "$fd/$name"
  for m in "" --check; do
    if run $m >"$tmp/out" 2>"$tmp/err"; then fail "$label ${m:-assemble}: expected failure"; continue; fi
    if grep -q "$name" "$tmp/err" && grep -Eq "$rx" "$tmp/err"; then
      if cmp -s "$tmp/orig" "$cl" && [ -e "$fd/1-good.md" ] && [ -e "$fd/$name" ]; then
        pass "$label ${m:-assemble}: fails naming the file, tree untouched"
      else fail "$label ${m:-assemble}: tree modified"; fi
    else fail "$label ${m:-assemble}: message wrong: $(cat "$tmp/err")"; fi
  done
}
bad "unknown section"        2-x.md '### Misc\n\n- a\n'                  'unknown section "Misc"'
bad "no section heading"     3-x.md '- just a bullet\n'                  'before the first|no ### section'
bad "text before heading"    4-x.md 'preface\n\n### Added\n\n- a\n'      'before the first'
bad "empty body"             5-x.md '### Added\n\n### Fixed\n\n- a\n'    'section "Added" has no entries'
bad "empty file"             6-x.md ''                                   'no ### section'
bad "only whitespace"        7-x.md '\n\n  \n'                           'no ### section'
bad "empty heading name"     8-x.md '###\n\n- a\n'                       'empty section heading'
bad "level-2 heading"        9-x.md '### Added\n\n- a\n\n## [9.9.9]\n'   'level-1/level-2'
bad "unterminated fence"     10-x.md '### Added\n\n```\n- a\n'           'unterminated'
bad "bad file name"          Notes.md '### Added\n\n- a\n'               'file name must be'
bad "name without number"    x-y.md '### Added\n\n- a\n'                 'file name must be'
bad "lowercase section case" 11-x.md '### added\n\n- a\n'                'unknown section "added"'

# --- 10. heading-like text in fences / code spans is not a heading ------------------------------
fresh fence; write_changelog
printf '### Added\n\n- uses `### Bogus` inline\n\n```md\n### Also Bogus\n## [9.9.9]\n```\n' > "$fd/12-f.md"
if run >/dev/null 2>"$tmp/err" && grep -q '^### Also Bogus' "$cl" && [ "$(grep -c '^## \[Unreleased\]' "$cl")" = 1 ] \
   && ! sed -n '/^## \[Unreleased\]/,/^## \[1.0.0\]/p' "$cl" | grep -q '^### Bogus'; then
  pass "fenced and inline heading-like text kept as entry text"
else fail "fence handling: $(cat "$tmp/err")"; fi

# --- 11. CRLF fragments --------------------------------------------------------------------------
fresh crlf; write_changelog
printf '### Added\r\n\r\n- crlf entry\r\n' > "$fd/13-c.md"
if run >/dev/null 2>&1 && grep -q '^- crlf entry$' "$cl" && ! grep -q "$(printf '\r')" "$cl"; then pass "CRLF fragment normalised"; else fail "CRLF"; fi

# --- 12. fragment without trailing newline -------------------------------------------------------
fresh nonl; write_changelog
printf '### Added\n\n- no trailing newline' > "$fd/14-n.md"
run >/dev/null 2>&1 && grep -q '^- no trailing newline$' "$cl" && pass "missing trailing newline handled" || fail "no trailing newline"

# --- 13. no [Unreleased] heading -----------------------------------------------------------------
fresh nounrel
printf '# Changelog\n\n## [1.0.0] - 2026-01-01\n' > "$cl"; cp "$cl" "$tmp/orig"
printf '### Added\n\n- x\n' > "$fd/15-x.md"
if ! run >/dev/null 2>"$tmp/err" && grep -q 'Unreleased' "$tmp/err" && cmp -s "$tmp/orig" "$cl" && [ -e "$fd/15-x.md" ]; then
  pass "missing [Unreleased] fails, nothing changed"; else fail "missing Unreleased"; fi

# --- 14. usage -----------------------------------------------------------------------------------
if "$script" --bogus >/dev/null 2>&1; then fail "unknown flag should fail"; else pass "unknown flag fails"; fi

# --- 15. the repository's own fragments are valid -----------------------------------------------
if "$script" --check >/dev/null 2>"$tmp/err"; then pass "repository changelog.d/ fragments are valid"; else fail "repo fragments invalid: $(cat "$tmp/err")"; fi

echo
if [ "$failures" -ne 0 ]; then echo "$failures failure(s)"; exit 1; fi
echo "all assemble-changelog checks passed"
