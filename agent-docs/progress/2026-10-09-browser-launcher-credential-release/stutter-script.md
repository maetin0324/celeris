---
title: Launcher admission stutter 台本の dash 修正
tasks: [01M4G7XB8QT6MSFR29C0QMSGK5]
status: complete
updated: 2026-10-09
---

# Launcher admission stutter 台本の dash 修正

`launcher-admission-evidence.sh --stutter 3` は各試験を `setsid` で起動し、`ps -o pgid= -p "$test_pid"` で実際の process group を取得して、その group が生きている間 STOP/CONT を繰り返す。dash で `kill -STOP -PGID` と `kill -CONT -PGID` を使い、各回の STOP 数を `STUTTER[stutter-N]: stops=<n>` に記録する。停止間隔は `sleep 0.002`、再開間隔は `sleep 0.001`。

## dash kill 形式の実測

一時 `setsid sleep` を dash から起動し `ps -o pgid= -p "$pid"` で PGID を確認した。`kill -s STOP -- -PGID` は rc=0 だったが、dash では同形式の CONT が rc=2 となった。`kill -STOP -PGID` と `kill -CONT -PGID` はともに rc=0 だったため、台本では後者を採用した。`$!` が PGID と一致する前提にはせず、常に `ps` の値を使う。

## 試験 hook

`LAUNCHER_EVIDENCE_TEST_CMD` はこの台本の試験専用 hook で、値は `sh -c` に渡す。本番の証跡取得手順では設定しない。未設定時は従来どおり `CELERIS_LAUNCHER_TESTS=require cargo test -p task-worker --test browser_launcher_ptrace -- --nocapture` を実行する。

## 検証

- `dash -c '... setsid sleep ... kill -STOP -"$pgid"; kill -CONT -"$pgid" ...'` — PGID を ps で確認、STOP/CONT とも rc=0。
- `sh -n crates/task-worker/scripts/launcher-admission-evidence.sh` — exit 0。
- `sh crates/task-worker/scripts/tests/launcher-admission-evidence-stutter.sh` — dummy success は 3 回すべて stops>0 と `EXIT: 0`、dummy exit 3 は非 0 の `EXIT` を出して台本も非 0。
