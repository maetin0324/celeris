---
status: done
completed: 2026-10-22
---
# fix-watch-flaky

- 原因: 末尾の pid 消滅確認が 1 回だけで、kill 直後の回収前 zombie を見て落ちた。
- 修正: `pid_gone` は zombie（/proc/<pid>/stat の state が Z）を消滅とみなす。末尾は最大 30 秒、20ms sleep のポーリングで消えるまで待つ。本体コードは不変。
- 証拠: 当該試験を 5 回連続 pass、`cargo clippy -p celeris-campaign --tests -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。
