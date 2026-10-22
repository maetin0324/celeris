---
tasks: [01M5H617AKZ119RE9EGH1JMPX5]
status: done
completed: 2026-10-22
---
# Jev 分類を WU の起動に間に合わせる

ADR: `agent-docs/adr/2026-10-22-jev-classification-before-launch.md`

## 実装
- `classify_tick.rs`: pending を含む未完 WU の先読み分類（統合・kind task を除く）、1 run 8 件上限、`DISPATCH_WAIT_MAX`
- `tick_loop.rs`: 分類段の有無で `Dispatcher::set_classification_wait`
- `task-dispatch` `lane_assist.rs` / `dispatch_run.rs`: 未分類 subject の起動を上限付きで見送る（`defer_for_classification`）
- `route_select/assist.rs`: `jev_specificity` / `jev_confidence` / `jev_confidence_missing`
- `classify_stage.rs`: 分類ログに specificity・confidence

## 試験
task-dispatch `lane_assist`（待ち→分類後に起動・上限は注入時計・待たない条件・WU 単位）、classify_tick（先読み・上限）、
assist_tests（欠測の区別）、classify_stage（ログに値が出て入力本文が出ない）。

## 証拠
下の検査コマンドの結果は run の報告本文を参照。

## 提案
- 本番で not_attempted 率が下がったか確認（release 後）。
