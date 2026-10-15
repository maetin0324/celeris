---
unit: classify-stage
status: done
completed: 2026-10-15
---
# classify-stage

分類段 `crates/celeris/src/daemon/classify_stage.rs`（`daemon.rs` に mod 行、celeris に `jev-classifier` 依存）。

## 変更
- `RequestClassifier` trait（本番 `JevClassifier`、試験は fake）。`ClassificationStage::classify_for_mode(mode, subject, input)`: `Fixed` は呼ばず board にも置かず `not_attempted`。
- 失敗は `fell_back_to_fixed{reason}`（breaker_open / timeout / … / config）。`KeyMissing`・`Auth` は reason `key_missing`/`auth` と `CredentialGate`（secret id・用途のみ）を board 項目に載せ、既存の dispatcher `apply_lane_assist` → `enter_credential_wait` へ渡す（新しい待ち機構なし）。
- `real_api_verified` は `LaneAssistRecord` 側で常に false（fallback と別欄）。実 API は呼ばない。分類は lane を下げず cheap にしない。成功率は載せない。
- dispatcher/store には Jev 参照なし（wire-mode の走査試験が引き続き保証）。

## 受け入れ条件と証拠
0. `fallback_records_fell_back_while_real_api_verified_stays_false`（Timeout/BreakerOpen/KeyMissing/Auth → fixed lane、`real_api_verified=false`）、`failures_become_fell_back_to_fixed_with_a_reason_name`。
1. `fixed_mode_never_calls_the_classifier_and_leaves_the_board_empty`。
2. `classification_never_lowers_to_cheap_and_carries_no_success_rate`。
3. `missing_key_hands_the_gate_to_the_existing_credential_wait`（stage 側）＋ task-dispatch の `missing_key_enters_the_existing_credential_wait_for_that_unit_only`（待ち本体）。
- `cargo test -p celeris --lib classify_stage`: 6 passed。全体の test-parallel・clippy は完了報告。

## 未解決事項
- `JevClassifier` の構築（`JevConfig` 欄）、`ApiKeyProvider` の secrets_dir 実装、ready な WU を分類して board を埋める tick 連携は未配線（`#[allow(dead_code)]`）。live smoke（有料・人の承認）後。
- Jev 実 API 統合は未検証。

## 提案
- 起動 unit で WU の ready 化時に分類して board を埋め、鍵登録後は board を差し替えて再開する。
