#!/bin/sh
# ADR-0115 / ADR-0116 D-L: launcher の実 session で ptrace 拒否と本番 admission の許可/拒否表を
# daemon UID の host shell（run sandbox の外）で走らせ、出力全体を <out-file> に残す。
# 前提: root が protocol v4 の launcher を設置・socket/service を再起動済みで、
# celeris-browser の subuid/subgid が有効。run sandbox 内の UID では SO_PEERCRED の許可に通らない。
#
#   sh crates/task-worker/scripts/launcher-admission-evidence.sh <out-file>
#
# <out-file> には `CELERIS_LAUNCHER_TESTS=require cargo test -p task-worker --test
# browser_launcher_ptrace -- --nocapture` の stdout/stderr をそのまま書き、最後に
# `EXIT: <code>` を 1 行足す。終了コードは cargo のもの。host の前提（v4 launcher・
# celeris-browser の SCM_CREDENTIALS・subuid）が欠ければ require なので失敗する。
set -u

if [ "$#" -ne 1 ] || [ -z "$1" ]; then
    echo "usage: sh $0 <out-file>" >&2
    exit 2
fi
out=$1

repo=$(cd "$(dirname "$0")/../../.." && pwd) || exit 2
out_dir=$(dirname "$out")
mkdir -p "$out_dir" || exit 2

echo "repo=$repo rev=$(git -C "$repo" rev-parse --short=12 HEAD 2>/dev/null) uid=$(id -u) out=$out" >&2

cd "$repo" || exit 2
CELERIS_LAUNCHER_TESTS=require cargo test -p task-worker --test browser_launcher_ptrace \
    -- --nocapture >"$out" 2>&1
code=$?
echo "EXIT: $code" >>"$out"

grep -E '^(ADMISSION|SKIP:|PTRACE_ATTACH|strace -p|test result:|EXIT:)' "$out" >&2
exit "$code"
