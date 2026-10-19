---
title: "task-core: 分類段・lane 各段・call ID の記録型と永続化・export 契約"
tasks: [01M59W8DEFMMP2C2SX5VE4KTKG]
unit: core-record
status: done
updated: 2026-10-19
completed: 2026-10-19
---
# core-record（task-core 内のみ）

## 決めたこと
- 既存 `ClassifierRecord.model` = **応答が名乗った model 名**（`ClassificationEntry.model_snapshot` の値。設定名ではない）。`snapshot` = 応答と**独立に観測**した版。観測経路が無ければ None で、model から複製しない。
- `LaneAssistRecord`（routing_decided の `lane_assist`）に nullable 5 欄: `fixed_lane`・`jev_lane`（Jev 単独、confidence 門を通った提案のみ）・`response_model`・`observed_snapshot`・`call_id`。制約後 lane は既存 `final_lane`。省略は `skip_serializing_if`（旧 event と同形）。
- DL run 行（migration 0087、全 NULL 可）: `classifier_status`・`classifier_call_id`・`fixed_lane`・`jev_lane`・`constrained_lane`・`lane_basis`。`decided_lane`（既存方策の lane）とは別欄。
- 純粋投影 `DecisionLogProposal::apply_lane_assist`（`from_routing_record` が呼ぶ）。`NotAttempted` は classifier(kind) を作らず status だけ残す。confidence は RD に無いので None（作らない）。
- call ID = `DecisionLogUsage::jev_call_id(subject, call_no)` = 消費行の `UsageKey::dedup_key`（`decision:jev:<subject>:<call_no>`）。`attempt_no`（run の試行番号）とは別。再分類・credential 再試行は call_no が進み、キャッシュ再利用で同じ分類を複数 run が使えば同じ call_id を共有する（消費行は call 単位 1 行で二重計上しない）。
- export は trial / decision record に上の欄を常に出し、欠測は null。schema 名 `celeris.decision-log/1` は不変（追加のみ）。

## 証拠
- `cargo test -p task-core --lib`: 1017 passed（schema 再生成後）。新試験: `assist_tests`（3 件）、`decision_log_tests` の `lane_stages_status_version_and_call_id_survive_store_reload_and_export` / `missing_stage_values_stay_null_and_not_attempted_makes_no_classifier`。
- `bash scripts/dev/test-parallel.sh`: exit 0、5831 passed / 14 skipped。`cargo clippy --workspace --all-targets -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- schema 再生成: `UPDATE_SCHEMA=1` で task-core / task-api、web `pnpm gen:types`（gui は退役済みで対象なし、docs/protocol は差分なし）。
- SCHEMA_VERSION 86→87 の pin 試験と旧 schema 巻き戻し試験（cluster_job / cron の DROP COLUMN）を更新。

## 未解決・後続（他 WU の担当）
- 実際の応答 model・call_id・snapshot を `ClassificationEntry`/`LaneAssistRecord` へ入れる配線は jev-response / daemon-board / dispatch-wire。`ClassificationEntry` は本 WU で変更していない（`observed_snapshot`/`call_id` は dispatch 側が `record` に設定する想定）。
- `decision_log_wire::write_decision_log` の「他 writer の欄を残す」処理（`classifier` を残す箇所）に新 6 欄の保持を足す必要がある（dispatch-wire）。
- 実 API の成功は未検証。合成/fake の値は実 API 成功ではない。

## 提案
- ClassificationEntry に `observed_snapshot`・`call_id` を足す場合は jev-response と衝突しないよう integrate 段で行う。
