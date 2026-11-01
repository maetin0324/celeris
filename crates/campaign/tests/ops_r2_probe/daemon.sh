#!/bin/sh
# ops-r2 probe の fixture（ADR 2026-10-30-ops-r2-isolation-principal、probe unit）。
# 測定 daemon の代わりに ops-r2-sandbox.sh の中で動き、daemon の子として 2 つの経路を起こす。
#   - CoS が自分で済ませる経路: CoS chat run（daemon の直接の子、cwd は thread の dir）
#   - task 経由の経路: worker run（daemon → worker → harness の孫、cwd は task の作業場所）
# どちらも probe-read を走らせ、結果を <state>/probe/{cos,worker}.json、exit を *.exit、pid namespace を *.ns に書く。
# LLM・network・本物の daemon は使わない。
# usage: daemon.sh <state> <probe binary> [probe-read の --deny-readable-file/--allow ...]
set -eu
[ $# -ge 2 ] || { echo 'usage: daemon.sh <state> <probe> [args...]' >&2; exit 2; }
STATE=$1 PROBE=$2
shift 2
OUT=$STATE/probe
COS=$STATE/cos/threads/probe-thread
WORKER=$STATE/ws/probe-task/repos/work
mkdir -p "$OUT" "$COS" "$WORKER"
set +e
(cd "$COS" && readlink /proc/self/ns/pid > "$OUT/cos.ns" && exec "$PROBE" probe-read --principal B-cos "$@") > "$OUT/cos.json"
echo $? > "$OUT/cos.exit"
(cd "$WORKER" && exec sh -c 'readlink /proc/self/ns/pid > "$1"; shift; exec "$@"' worker "$OUT/worker.ns" \
    "$PROBE" probe-read --principal B-worker "$@") > "$OUT/worker.json"
echo $? > "$OUT/worker.exit"
exit 0
