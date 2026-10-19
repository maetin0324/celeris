---
title: "celeris: 通常分類から board への応答版伝搬"
tasks: [01M59W8DEFMMP2C2SX5VE4KTKG]
unit: daemon-board
status: done
updated: 2026-10-19
completed: 2026-10-19
---
# daemon-board

## 決めたこと
- `ClassificationStage::classify_subject` は `board.put_with_provenance` を使い、成功時のみ `CallProvenance{response_model, observed_snapshot}` を載せる（応答の `model` と独立 `model_snapshot` を別欄のまま）。fallback・credential 待ち・config は全欄 None。fixed は分類器を呼ばず board にも置かない（従来通り）。
- `ClassificationEntry.model_snapshot`（旧欄）は応答 model のまま（task-core 不変）。設定の `model_id` は版に使わない。
- real_api_verified は変更なし（false）。fake 応答は実 API 成功ではない。

## 証拠
- `cargo test -p celeris --lib classify`: 15 passed（新試験 `board_call_carries_response_versions_and_fallback_has_none`: 設定 model≠応答 model、snapshot 欠測、fallback/credential、再分類で call_id 更新）。
- 全検査の結果は下記。

## 未解決
- record/export への反映は dispatch-wire。
