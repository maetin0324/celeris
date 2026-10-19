---
task: 01M5AC7DNXVSZE0B214Z2GZ75N
tasks: [01M5AC7DNXVSZE0B214Z2GZ75N]
title: 子孫・retry・continuation・repairで固定値を保持（dispatch-lineage）
work_unit: dispatch-lineage
status: done
updated: 2026-10-19
---
# dispatch-lineage

## 変更（task-dispatch のみ）
- `dispatcher/variant_pin.rs`: `Dispatcher::variant_start_check(&Task)` を追加。`task_campaign_resolve`（祖先連鎖全体の整合解決。矛盾・循環は Err）の `variant_org` を取り、共有 org が pin と一致するか（`check_org`）、assignee が pin 範囲か（`check_assignee`。assignee なしは失敗）を再検査する。variant なしは `Ok(())`。
- `dispatch_run.rs` `dispatch_one`: credential gate の後・lease 取得と `WorkerStarted` の前に `variant_start_check` を呼ぶ。失敗は `block_variant_task`（blocked＋理由 event。run・lease を作らない）。`dispatch_one` は retry・continuation・repair WU・並列 WU の 2 本目以降も通る単一の起動点なので、割当前（`assign_if_needed`）と run 起動直前の 2 回 drift を検査する。子・孫は祖先の pin を連鎖解決で継ぐ。
- RunContext への記録は worker-pin 以降（本葉では追加しない）。review run の起動点（`spawn_review`）は対象外（coding の構成 pin は worker run のもの）。

## 試験（`tests/variant_pin.rs` に 5 件追加、計 13 件）
正: root/子/孫・retry の既存 assignee は org 一致で通る。variant なしは no-op。
負: 割当後の org drift を全世代で検出、pin 外 assignee・assignee なしを拒否、祖先と矛盾する子の識別は保存時に拒否され root の pin が解決される。

## 証拠
- `cargo test -p task-dispatch` → 962 passed、0 failed（`variant_pin` 13 passed）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/test-parallel.sh` は統合葉に委ねた（未実行）

## 未解決事項
- `dispatch_one` を通した end-to-end の drift 起動拒否試験は normal-e2e に委ねた（本葉は `variant_start_check` の単体と assign 経路）。
