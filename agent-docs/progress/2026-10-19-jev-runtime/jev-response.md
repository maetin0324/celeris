---
title: "jev-classifier: 応答由来 model/snapshot の保持"
tasks: [01M59W8DEFMMP2C2SX5VE4KTKG]
unit: jev-response
status: done
updated: 2026-10-19
completed: 2026-10-19
---
# jev-response

## 変更
- `Classification` に `response_model`（応答 body の `model`）を追加。`model_snapshot` は応答の独立欄 `model_snapshot` がある時だけ Some（従来は `model` を入れていた）。`model_id` は設定の要求 model のまま。欠測は None（設定値・0 で補完しない）。
- `classify_stage.rs` の 1 行: board の `ClassificationEntry.model_snapshot`（core-record 契約では応答 model 名）へ `response_model` を渡す（従来挙動の維持）。daemon の伝搬整理は daemon-board が担当。
- fake HTTP 試験 4 件（設定/応答 model 差、snapshot 有/無、model 欠測、fallback 後・cache roundtrip）と parser 単体試験。いずれも fake server による開発検証で、実 API 成功ではない。

## 証拠
- `cargo test -p jev-classifier`: 18 passed。
- `bash scripts/dev/test-parallel.sh`: exit 0、failed 0。`cargo clippy --workspace --all-targets -- -D warnings` ok、`cargo fmt --all -- --check` ok。

## 提案
- 実 API が独立 snapshot 欄を返すかは live 確認（live-runbook）まで未確認。形を推測して欄を足していない。
