---
title: "task-dispatch: 通常 routing から永続ログへ接続"
tasks: [01M59W8DEFMMP2C2SX5VE4KTKG]
unit: dispatch-wire
status: done
updated: 2026-10-19
completed: 2026-10-19
---
# dispatch-wire

## 決めたこと
- `apply_lane_assist` は board の **current call**（`get_call`）から RD の `response_model`・`observed_snapshot`・`call_id` を埋める。snapshot は `CallProvenance.observed_snapshot` のみ（response_model から複製しない）。欠測は None。
- `call_id` は実際の DL 消費行の鍵（`jev_usage_row(..).key.dedup_key()`）。これは `jev_call_id(subject, call_no)` とは別（outcome と board の call_id を含む）。core-record の `jev_call_id` を同一と仮定しない。消費行の組み立てを `jev_usage_row` に切り出し、書込みと RD の両方が同じ関数を使う。`NotAttempted` は call_id None。
- `write_decision_log` の再投影で classifier_status / classifier_call_id / fixed・jev・constrained lane / lane_basis を保持（routing event 無しの再投影で消さない）。
- 実行 model は既存の `exec_model`（WorkerStarted / proxy attempts 由来）をそのまま使い、run_id で結合。再分類 call（call_no）と実行 retry（attempt_no）は別欄。

## 証拠
- `cargo test -p task-dispatch --lib lane_assist`: 15 passed（新試験 `normal_tick_provenance_survives_reopen_export_and_reload`: 通常 tick → routing_decided → 永続 DB 再 open → export → 再読込。3 task（再分類 / 分類なし / snapshot 観測）、重複消費・別 task 混入なし）。
- fake の分類結果による開発検証で、実 API 成功ではない（real_api_verified は false のまま）。
