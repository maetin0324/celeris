---
tasks: [01M4651ZZP8FJKGG6W2WPFNKBH]
unit: sidecar-model
status: done
completed: 2026-10-05
---

# Phase 5 sidecar-model: /estimate v1 の純粋型と検証

## 変更

- `task-core::model_router::estimator::sidecar` に version 1 の descriptor、request/response、型付き検証 error を追加。ID、版、候補の完全一致、重複、有限な [0,1]、依存申告、JSON サイズを検査する。
- 検証済み応答だけから `QualityEstimator` を実装する同期 snapshot を構築する。HTTP、最終 lane/source/account の選択は持たない。
- `ShadowKind::Estimator` を追加し、生成量・費用・出力を持たない比較記録として検証する。event と API の schema を再生成した。
- 後続の Python wrapper と共有する正常 request/descriptor/response、異常 response 6 種の JSON fixture を追加した。

## 証拠

- `cargo test -p task-core routing_sidecar_protocol_validates_identity_range_and_size -- --exact --nocapture` → 統合試験 1 件 passed。
- `UPDATE_SCHEMA=1 cargo test -p task-core event_row_schema_matches_committed` → 1 件 passed。
- `UPDATE_SCHEMA=1 cargo test -p task-api committed_schema_matches_generated` → 1 件 passed。
- `cargo clippy -p task-core -- -D warnings` → exit 0。

## 未解決事項

- HTTP timeout、privacy gate、実 sidecar 接続は後続の proxy-client / proxy-shadow-est unit が担当する。
- `ShadowRecord` の `detail` は既存の任意テキスト欄であり、比較値の機械可読な保存形式は proxy-shadow-est unit で定める。
