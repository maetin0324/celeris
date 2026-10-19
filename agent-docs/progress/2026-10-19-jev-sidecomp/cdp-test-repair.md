---
task: cdp-test-repair
status: done
---
# cdp-test-repair

- 観測: 単独は通過。`taskset -c 0` で 4 並走 ×60 回のうち 16/240 が `barrier: SinkFailed`（約 5.5 s）。
- 原因: `LiveTap::demux` の NUL 走査が chunk ごとに先頭から（巨大 frame で二乗）。詳細は `agent-docs/adr/2026-10-19-jev-cdp-test-repair.md`。
- 修復: 走査済み位置を保持（`crates/task-worker/src/browser_launcher/live.rs`）。回帰試験を `live_tests.rs` に追加（応答の順序を channel で強制）。
- 修復後: 同じ 1 core 取り合い 240 回で失敗 0。
- 注意: 追加した回帰試験は旧コードで必ず落ちるわけではない（性能起因のため）。順序と漏れの保証を固定する。
- 注意: TMPDIR が長いと別の live_tests が SUN_LEN で落ちる（この run の環境固有。短い TMPDIR で通過）。
- 全体: test-parallel.sh 5840 passed / 0 failed（exit 0）、clippy -D warnings 成功、fmt --check 成功。
