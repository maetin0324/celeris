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

## 追加の検証（run 12）
- lease: `expired_lease_during_stage_integration_is_extended_not_reclaimed`（leases.rs の `integrating` 分岐。lease 切れ後に reclaim しても Running のまま lease が延びる）。
- 時間切れの繰り返し: `repeated_integration_check_timeout_blocks_with_a_readable_reason`（replan/repair 上限後に blocked の質問、理由に `integration_check_timeout` と cmd）。実装は phase_integration.rs の `timeout_tag`。
- `| tail` は planner 指示では禁止せず、時間切れ時に log へ注記する決定（ADR 参照）。
