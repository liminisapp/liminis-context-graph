#!/usr/bin/env bash
# Guard for release.yml's `verify-plan` job (#725, ADR-0725).
#
# Reads the dist plan manifest -- from $PLAN_VAL if set, otherwise from stdin -- and exits
# non-zero unless it is non-empty, valid JSON with a non-empty
# `.ci.github.artifacts_matrix.include` array.
#
# Why it exists: the GitHub runner silently DROPS a job output that looks like a credential
# ("Skip output 'val' since it may contain secret"). `plan`'s `val` is the whole dist
# manifest, including the CHANGELOG-derived release notes, so credential-looking changelog
# text (`Authorization: Bearer <key>`) empties it for every downstream job. The build jobs
# then skip silently on `fromJson('')`, and a tolerant `host` published an empty release.
# The drop happens when outputs are handed to downstream jobs, after the producing job's
# steps finish, so this must run in a job that `needs: plan`.
#
# It NEVER echoes the value: it may contain the text that was masked.
#
# Reason codes: empty | not-json | no-matrix | empty-matrix.
set -uo pipefail

if [ "${PLAN_VAL+set}" = set ]; then
  val="$PLAN_VAL"
else
  val="$(cat)"
fi

reason=""
if [ -z "${val//[[:space:]]/}" ]; then
  reason=empty
elif ! printf '%s' "$val" | jq empty >/dev/null 2>&1; then
  reason=not-json
elif ! printf '%s' "$val" | jq -e '.ci.github.artifacts_matrix.include | type == "array"' >/dev/null 2>&1; then
  reason=no-matrix
elif ! printf '%s' "$val" | jq -e '.ci.github.artifacts_matrix.include | length > 0' >/dev/null 2>&1; then
  reason=empty-matrix
fi

if [ -z "$reason" ]; then
  echo "plan output OK: build matrix present and non-empty"
  exit 0
fi

msg="The release plan output reached downstream jobs empty or without a build matrix (reason: ${reason}). \
A likely cause is secret masking: the runner drops a job output that looks like a credential \
(e.g. 'Authorization: Bearer <key>' in the CHANGELOG section). Reword the offending text, re-cut the tag, \
and re-run. No release was created."
echo "::error title=Release plan output lost (${reason})::${msg}"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    echo "### Release plan output lost (\`${reason}\`)"
    echo
    echo "$msg"
    echo
    echo "See docs/release-process.md (\"Plan output lost\") and ADR-0725."
  } >>"$GITHUB_STEP_SUMMARY"
fi
exit 1
