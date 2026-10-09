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

# A short dummy is stopped and resumed while its group may end at any moment.
# Repeating it guards against the race where a kill meets a finished group.
# sleep uses no CPU and no launcher or userns is involved.
for repeat in 1 2 3 4 5 6 7 8 9 10; do
    log=$tmpdir/repeat-$repeat.log
    if ! LAUNCHER_EVIDENCE_TEST_CMD='sleep 0.3' sh "$script" --stutter 3 "$log"; then
        echo "repeat $repeat: dummy command unexpectedly failed" >&2
        exit 1
    fi
    for run in 1 2 3; do
        grep -Eq "^STUTTER\[stutter-$run\]: stops=[1-9][0-9]*$" "$log" || {
            echo "repeat $repeat: stutter-$run applied no stop" >&2
            exit 1
        }
    done
    test "$(tail -n 1 "$log")" = 'EXIT: 0' || {
        echo "repeat $repeat: last line is not EXIT: 0" >&2
        exit 1
    }
done
