---
title: 段の統合の検査の孤児・lease 切れの修正
tasks: [01M5D93NQDW3GX6W71W7MD8BY7]
status: done
updated: 2026-10-20
completed: 2026-10-20
---
# 検査の孤児を残さない・統合中の lease 延長

完了日: 2026-10-20。ADR は `agent-docs/adr/2026-10-20-integration-check-process-group-and-lease.md`。

## 証拠
- `cargo test -p task-worker exec_timeout_kills`: exit 0、`exec_timeout_kills_stopped_descendants_and_notes_the_log` 1 passed（SIGSTOP で止まる子孫が時間切れで残らず、log に停止子孫の注記が残る）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/test-affected.sh`: exit 0。

## 未解決・提案
- planner の指示文で check に `| tail` を使わないよう書く（未実装）。
- 時間切れ理由に「cargo が lock 待ち / 子が停止」を出す、検査開始前に target dir の stale 孤児を掃除する。
- sccache の rustc 停止そのものへの手当ては未着手。
