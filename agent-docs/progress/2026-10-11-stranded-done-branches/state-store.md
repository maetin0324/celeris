---
tasks: [01M4MAZNNYT4JVRYKCVZ1TF1VF]
---
# state-store

- task×repo の現在状態を保存する migration 0067 と `TaskRepoIntegrationState` / `TaskRepoIntegrationStatus` を追加した。
- TaskStore に upsert/get/state filter list を追加。状態遷移時は同一 transaction 内で `integration_detected` / `integration_merged` / `integration_discarded` / `integration_held` を追記し、同じ状態の再 upsert は event を増やさない。
- 既存 `task_integrations` の PR 操作履歴は migration 0067 で `task_change_integrations` に改名して既存 API の backing table とし、同名の `task_integrations` を task×repo の現在状態表にした。履歴行は保持する。
- 実行: `cargo check -p task-core` — 成功。
- 実行: `UPDATE_SCHEMA=1 cargo test -p task-core task_integration_upsert_emits_only_on_state_change_and_lists_by_state --lib` — 成功（1 passed）。
- 未解決: API/CLI・daemon sweep・promotion は後続 unit の担当。
- 再実行: `UPDATE_SCHEMA=1 cargo test -p task-core` — 成功（901 unit + 1 integration + 3 doc tests）。migration 0067 の legacy-table rename と migration downgrade fixtures、event schema 一致を含む。
- 実行: `cargo clippy --workspace -- -D warnings` — task-api の `crates/task-api/src/query.rs::event_type_name` が新 event 5 種を網羅せず失敗。後続 `api-cli` unit の担当 file のため、この WorkUnit では変更しない。
- 実行: `cargo clippy -p task-core -- -D warnings` — 成功。
- 実行: `cargo fmt --all -- --check`、`git diff --check`、`sh "$CELERIS_WU_SCOPE_PATHS"` — 全て成功。
