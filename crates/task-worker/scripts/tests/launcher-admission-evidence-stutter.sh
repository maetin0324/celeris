#!/bin/sh
set -eu

repo=$(cd "$(dirname "$0")/../../../.." && pwd)
tmpdir=$(mktemp -d "${TMPDIR:-/tmp}/launcher-stutter.XXXXXX")
trap 'rm -rf "$tmpdir"' EXIT HUP INT TERM
script=$repo/crates/task-worker/scripts/launcher-admission-evidence.sh

if LAUNCHER_EVIDENCE_TEST_CMD='sleep 0.4' sh "$script" --stutter 3 "$tmpdir/success.log"; then
    :
else
    echo 'success dummy command unexpectedly failed' >&2
    exit 1
fi
for run in 1 2 3; do
    grep -Eq "^STUTTER\[stutter-$run\]: stops=[1-9][0-9]*$" "$tmpdir/success.log"
done
test "$(tail -n 1 "$tmpdir/success.log")" = 'EXIT: 0'

if LAUNCHER_EVIDENCE_TEST_CMD='sleep 0.3; exit 3' sh "$script" --stutter 3 "$tmpdir/failure.log"; then
    echo 'failing dummy command unexpectedly succeeded' >&2
    exit 1
fi
last=$(tail -n 1 "$tmpdir/failure.log")
printf '%s\n' "$last" | grep -Eq '^EXIT: [1-9][0-9]*$'
