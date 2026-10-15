---
title: "log-wire: dispatcher の実値を決定ログ欄へ書く配線"
tasks: [01M4Z716SMR90GX0WDEHA9B02Z]
status: done
updated: 2026-10-15
completed: 2026-10-15
---
# log-wire（unit）

log-core の `decision_log_*` へ、dispatcher が実際の判断・実行・結果を書く。routing の選択（`route_select`）は変えていない。

## 変更

- `crates/task-dispatch/src/dispatcher/decision_log_wire.rs`（新規）: events（`routing_decided`・`worker_started`・`routing_request_decided`・`worker_finished`・WU 検査/review の結果）から run ごとに決定的に投影し、`run_id` 主キーの置換で書く。LLM は呼ばない。
  - 提案: lane（clamp 前後）、候補、除外と理由、提案先頭（根拠表 score 最上位。無ければ選択）、policy/route/evidence 版、propensity、parent_run_id（同じ役割・WU で直前の run = retry）、attempt_no。
  - 実行: `worker_started` の source/model/account/adapter、route decision の harness。proxy が実際に試した列があれば最後の候補を実行とし、`fallback_reason` は proxy の理由、無ければ shadow/explore/model_pin 等の名前。提案先頭とは別欄。
  - 成果: run 終了ラベル（wall-clock の予算切れ = `timeout`、cancel、harness_error …）と元の値、WU 検証（判断のある run だけ。他は None）、wall_ms。終わっていない run は書かない。campaign 合否は書かず、既存値を保持する。
  - 消費: run ごとに 1 行（`run:<id>:worker_finished`）。cost は定価換算（推定）で、実支出 `cash_cost_usd` は欠測のまま。
  - 他の writer が入れた campaign_id / arm / routing_mode / classifier 等は、こちらが持たないとき既存値を残す。
- 呼び出し点: `record_routing_outcomes`（完了・review・WU 検査・統合）、shutdown 中断、lease 回収、`close_aborted_run`（cancel）。どれも冪等。
- `TaskStore` に `decision_log_record_proposal/execution/outcome/usage` と `decision_log_run_get` を追加（`SqliteStore` へ転送）。task-api の型は変えていない（schema 再生成なし）。

## 証拠

- `cargo test -p task-dispatch --lib decision_log` 5 passed: fallback で提案と実行が別欄・提案先頭の実行 index に帰属しない / retry と再処理で行数・合計が増えない / cancel・timeout が別ラベル / 他 writer の campaign 欄を保持 / dispatch の実行時に新欄が埋まる（`tests/decision_log.rs`）。
- `cargo test -p task-dispatch`: 878 passed（変更前の既存試験すべて通過 = routing 選択結果は不変）。
- workspace 試験・clippy は下記。

## 未解決事項・提案

- campaign_id / arm / routing_mode / Jev classifier の書き手は後続 unit（campaign / Jev adapter）。cos・cluster 待機・介入の消費は export-agg / 後続。
- reviewer run の判断 record は無いので、reviewer 行は実行と消費のみ。

## 最終検査

- `bash scripts/dev/test-parallel.sh`: passed 5160 / failed 0 / ignored 13、`test-parallel: ok`。
- `cargo clippy --workspace -- -D warnings`: exit 0（`type_complexity` を `Verdict` 構造体で解消後）。
