---
task: build-tmp-hygiene
wu: repair-design-1
status: done
completed: 2026-10-07
---
# repair-design-1（design 段の統合後検査の修復）

- 失敗: `cargo test -p e2e --test account_pool_scenarios` が 3 本とも `<target>/debug/celeris not found` で落ちた
- 原因: この task の scratch target（`CARGO_TARGET_DIR=/local/celeris/data/scratch/targets/task-01M4B4J92KBR73EQA5S7FWB21G/target`）が
  新しく、`celeris` / `celerisctl` の binary が未 build だった。e2e crate は両 binary に cargo 依存を持たず
  `current_exe()` の隣で探すため、`cargo test -p e2e` だけでは build されない。source の問題ではない
  （main の修正 0fa576a8 は取り込み済み）
- 直し: `cargo build -p celeris -p celerisctl`（exit 0、1m10s）を同じ target に流した。code は変えていない
- 再実行: `cargo test -p e2e --test account_pool_scenarios` → 3 passed / 0 failed（exit 0）
- 範囲外差分の検査 2 本: exit 0
- 提案: 統合の検査で e2e を単独で流すときは先に `cargo build -p celeris -p celerisctl` を置く（main-sync の葉でも同じ注記）
