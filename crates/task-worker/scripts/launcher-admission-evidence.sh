#!/bin/sh
# Run host-only launcher admission evidence. This script never changes host state.
set -u

usage() {
    cat <<EOF
Usage: sh $0 <out-file>
       sh $0 --stutter 3 <out-file>
       sh $0 --credential <out-file>
       sh $0 --help

--stutter 3   Run the required launcher admission test three times with its
              SIGSTOP stutter scenario enabled by the test harness.
--credential Run the launcher_credential_ unit and integration tests.
EOF
}

if [ "$#" -eq 1 ] && [ "$1" = --help ]; then usage; exit 0; fi
mode=admission
if [ "$#" -gt 0 ]; then
    case "$1" in
        --stutter) [ "$#" -eq 3 ] && [ "$2" = 3 ] || { usage >&2; exit 2; }; mode=stutter; shift 2 ;;
        --credential) [ "$#" -eq 2 ] || { usage >&2; exit 2; }; mode=credential; shift ;;
    esac
fi
[ "$#" -eq 1 ] && [ -n "$1" ] || { usage >&2; exit 2; }
out=$1
repo=$(cd "$(dirname "$0")/../../.." && pwd) || exit 2
mkdir -p "$(dirname "$out")" || exit 2
echo "repo=$repo rev=$(git -C "$repo" rev-parse --short=12 HEAD 2>/dev/null) uid=$(id -u) mode=$mode out=$out" >&2
cd "$repo" || exit 2

run_logged() {
    label=$1
    shift
    echo "RUN[$label] $*" >>"$out"
    "$@" >>"$out" 2>&1
    code=$?
    echo "EXIT[$label]: $code" >>"$out"
    grep -E '^(ADMISSION|SKIP:|PTRACE_ATTACH|strace -p|test result:|EXIT\[)' "$out" | tail -12 >&2 || true
    return "$code"
}

: >"$out"
case "$mode" in
    admission) CELERIS_LAUNCHER_TESTS=require cargo test -p task-worker --test browser_launcher_ptrace -- --nocapture >>"$out" 2>&1; code=$? ;;
    stutter)
        code=0
        n=1
        while [ "$n" -le 3 ]; do
            label="stutter-$n"
            echo "RUN[$label] required launcher admission + SIGSTOP stutter" >>"$out"
            setsid env CELERIS_LAUNCHER_TESTS=require cargo test -p task-worker --test browser_launcher_ptrace -- --nocapture >>"$out" 2>&1 &
            test_pid=$!
            # Pause the test harness twice while it owns a live real launcher session.
            # The test itself still performs cleanup after each resume.
            stutter=1
            while [ "$stutter" -le 2 ]; do
                alive=0
                tries=0
                while [ "$tries" -lt 200 ]; do
                    kill -0 "$test_pid" 2>/dev/null || break
                    kill -STOP -- "-$test_pid" 2>/dev/null && { alive=1; break; }
                    tries=$((tries + 1))
                    sleep 0.05
                done
                if [ "$alive" -eq 1 ]; then
                    sleep 0.1
                    kill -CONT -- "-$test_pid" 2>/dev/null || true
                    sleep 0.1
                fi
                stutter=$((stutter + 1))
            done
            wait "$test_pid"
            code=$?
            echo "EXIT[$label]: $code" >>"$out"
            grep -E '^(ADMISSION|SKIP:|PTRACE_ATTACH|strace -p|test result:|EXIT\[)' "$out" | tail -12 >&2 || true
            [ "$code" -eq 0 ] || break
            n=$((n + 1))
        done
        ;;
    credential)
        run_logged credential-unit cargo test -p task-worker launcher_credential_ -- --nocapture || code=$?
        if [ "${code:-0}" -eq 0 ]; then run_logged credential-broker cargo test -p celeris-credentiald --test prod_admission launcher_credential_ -- --nocapture || code=$?; fi
        code=${code:-0}
        ;;
esac
if [ "$mode" = admission ]; then echo "EXIT: $code" >>"$out"; fi
exit "$code"
