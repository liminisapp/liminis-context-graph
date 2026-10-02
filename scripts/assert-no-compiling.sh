#!/usr/bin/env bash
# Fail if a captured cargo log contains a `Compiling` status line (#657).
#
# Used by CI jobs that are supposed to consume a prebuilt tree (`test` and the six e2e
# jobs): any `Compiling` line there means something rebuilt instead of reusing
# build-release's output.
#
# Colour-safe on purpose. `dtolnay/rust-toolchain` exports CARGO_TERM_COLOR=always, and
# with colour on cargo writes the status word and the rest of the line as separate styled
# runs:  ESC[1mESC[92m   CompilingESC[0m proc-macro2 v1.0.106
# A colour reset sits *between* `Compiling` and the following space, so the older
# `grep -q "Compiling "` could never match real output — it reported "full cache hit" on a
# run that recompiled all ~233 crates. ANSI sequences are therefore stripped first, and the
# match is anchored to the start of the line so that echoed script text (the runner prints
# the step's own `grep -q "Compiling "` source into the log) cannot false-match.
#
# Usage: scripts/assert-no-compiling.sh <logfile>
# Exit:  0 no `Compiling` lines; 1 found some (printed, ANSI-stripped); 2 bad usage/log.

set -uo pipefail

log="${1:-}"
if [ -z "$log" ] || [ ! -f "$log" ] || [ ! -r "$log" ]; then
  echo "usage: $0 <logfile> (log missing, unreadable or not a regular file: '${log}')" >&2
  exit 2
fi

esc=$'\033'
# Strip CSI sequences (colour/style); then drop a trailing CR in case of progress redraws.
# A failed normalisation must not read as "no Compiling lines" — fail closed.
if ! stripped=$(sed -E "s/${esc}\[[0-9;]*[A-Za-z]//g; s/\r\$//" "$log"); then
  echo "ERROR: could not normalise '$log'; refusing to treat it as a clean log" >&2
  exit 2
fi

if hits=$(printf '%s\n' "$stripped" | grep -E '^[[:space:]]*Compiling([[:space:]]|$)'); then
  echo "ERROR: cargo printed 'Compiling' lines in a job that must only reuse a prebuilt tree:"
  printf '%s\n' "$hits" | head -n 50
  count=$(printf '%s\n' "$hits" | wc -l | tr -d ' ')
  echo "($count Compiling line(s) in total)"
  exit 1
fi
echo "OK: no 'Compiling' lines in $log"
