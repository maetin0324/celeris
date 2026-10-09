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
              process group repeatedly stopped and resumed by this script.
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
        # Test-only hook for deterministic script checks; never use in the
        # production evidence procedure. The value is passed to sh -c.
        test_command=${LAUNCHER_EVIDENCE_TEST_CMD:-'CELERIS_LAUNCHER_TESTS=require cargo test -p task-worker --test browser_launcher_ptrace -- --nocapture'}
        n=1
        while [ "$n" -le 3 ]; do
            label="stutter-$n"
            echo "RUN[$label] required launcher admission + SIGSTOP stutter" >>"$out"
            setsid sh -c "$test_command" >>"$out" 2>&1 &
            test_pid=$!
            pgid=
            tries=0
            while [ "$tries" -lt 200 ]; do
                pgid=$(ps -o pgid= -p "$test_pid" 2>/dev/null | tr -d ' ')
                [ -n "$pgid" ] && break
                kill -0 "$test_pid" 2>/dev/null || break
                tries=$((tries + 1))
                sleep 0.005
            done
            stops=0
            if [ -n "$pgid" ]; then
                while kill -0 "$test_pid" 2>/dev/null; do
                    if kill -STOP -"$pgid" 2>/dev/null; then
                        stops=$((stops + 1))
                    else
                        code=1
                        break
                    fi
                    sleep 0.002
                    if ! kill -CONT -"$pgid" 2>/dev/null; then code=1; break; fi
                    sleep 0.001
                done
                # Do not leave the group stopped if it raced with completion.
                kill -CONT -"$pgid" 2>/dev/null || true
            else
                code=1
            fi
            wait "$test_pid"
            test_code=$?
            echo "STUTTER[$label]: stops=$stops" >>"$out"
            [ "$stops" -gt 0 ] || code=1
            [ "$test_code" -eq 0 ] || code=$test_code
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
echo "EXIT: $code" >>"$out"
exit "$code"
