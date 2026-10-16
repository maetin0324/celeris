---
unit: identity
task: 01M51X7MZPYQXVWV2WDG2N70C1
status: done
completed: 2026-10-16
---
# identity — campaign 識別の伝搬

## 実装済み
- migration 0084: `task_campaigns`（root の識別）、`decision_log_runs` に `case_id` `repetition` `manifest_hash` `routing_setting`。
- `task_core::decision_log::CampaignIdentity`、`SqliteStore::task_campaign_{set,set_tx,own,resolve}`、`TaskStore::task_campaign_resolve`。
- `write_decision_log` が task（または祖先）の識別を全 run 行に書く。消費行の `campaign_id` も同値。
- `POST /tasks` の `campaign`（人のみ、CoS は 422、task 作成と同 transaction）。ADR 2026-10-16-campaign-identity-propagation。

## mock 検証済み（temp DB / in-memory）
- `decision_log_wire::campaign_identity_reaches_every_descendant_run_retry_and_review`: root→子→孫、初回・再試行・reviewer の 9 run と消費 9 行に同一識別、再処理で増えない。
- `decision_log_wire::task_without_campaign_keeps_null_identity`: campaign 無しは NULL、別 campaign が混ざらない。
- `handlers::post_tasks_records_campaign_identity_on_the_root_only`: API から root に付く／不正値は 400。

## 実経路検証済み
- なし（本番 SQLite・実 LLM には触れていない）。

## 未実施
- `routing_setting` を dispatcher が試行単位の routing mode として使う配線（現状は記録のみ）。
- export（`decision_log_export.rs`）の record へ case/repetition/manifest_hash/routing_setting を出す変更（export-join unit の担当範囲）。
- `decision_log_interventions` の書き手が repo に存在しない（campaign_id を落とす先の行自体が未生成。metering unit で扱う）。
- CoS chat run など task に属さない run の識別。
- celerisctl / campaign runner から `campaign` を渡す側（外側 runner の接続は別 unit）。
- migration 0084 は他 unit の migration と番号が衝突しうる（統合時に振り直し）。

## 提案
- 統合時に migration 番号の衝突を確認する。

## 証拠コマンド
- `bash scripts/dev/test-parallel.sh` → exit 0、passed 5386 / failed 0 / ignored 13。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- API schema（docs/api/v1/api-v1.schema.json）と web/api/generated を再生成済み。
