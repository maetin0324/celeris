---
tasks: [01M4MEEKX485VD5SWKX0P30D4Y]
status: done
title: store 段の統合失敗を直す（schema drift と旧版 DB 試験）
updated: 2026-10-11
---
# store 段の統合失敗を直す（schema drift と旧版 DB 試験）

## 原因

migration 0068（`tasks.harness_id`、`runs.selected_*`、`account_limits` の作り直し）が入った後、次の 6 件が落ちていた。

- schema drift 3 件: `task-api` の `committed_schema_matches_generated`、`task-core` の `store::tests::event_row_schema_matches_committed`、`task-worker` の `protocol::tests::committed_schema_matches_generated`。API・event・protocol の schema が 0068 の選択欄・`AdapterChoice` 等を含む版に追いついていなかった。
- 旧版 DB の降格試験 2 件: `cluster_job::tests::migration_0034_adds_cluster_job_waits_to_a_schema_33_db` と `cron::store_tests::cron_job_migration_applies_to_an_existing_schema_37_db`。降格 SQL が 0068 の列を落とさないまま版数だけ戻したため、再適用で `duplicate column name: harness_id` になった。
- 版数期待 1 件: `delivery::tests::migration_0042_fills_gaps_in_main_schema_41_database` の期待版数列に 68 が無かった。

## 直したこと

- schema の再生成（`UPDATE_SCHEMA=1`）:
  - `cargo test -p task-api committed_schema_matches_generated`
  - `cargo test -p task-core event_row_schema_matches_committed`
  - `cargo test -p task-worker protocol::tests::committed_schema_matches_generated`
  - 変更は `docs/api/v1/api-v1.schema.json`・`docs/api/v1/event.schema.json`・`docs/protocol/worker-protocol.schema.json` の追記のみ（計 142 行追加、削除 0）。
- 生成型の追従:
  - `gui/app/celeris/types.ts`: `corepack pnpm@11.27.0 -C gui install --offline --frozen-lockfile` の後 `corepack pnpm@11.27.0 -C gui gen:types`（17 行追加）。
  - `web/api/generated/types.ts` と `schema.json`: `node web/scripts/gen-types.mjs`。
- 降格 SQL に 0068 の逆操作を足した（migration 本体は変えていない）:
  - `tasks DROP COLUMN harness_id`
  - `runs DROP COLUMN` の `selected_harness_id`・`selected_source_id`・`selected_model_id`・`selected_account_id`・`selection_metadata_json`
  - 対象は `cluster_job/tests.rs` と `cron/store_tests.rs`。両方とも既に `DROP TABLE account_limits` を持つので、0066 の形へ戻った後に 0066・0068 が再適用される。
- `delivery.rs` の期待版数列に 68 を足した（67 は他 task の予約で欠番のまま）。
- 他の降格試験の確認: `feed/tests.rs`（版 41・47 だけ削除）、`store/client_open_tests.rs`、`celerisctl/tests/no_migrate.rs` は 0068 を再適用させる形ではないため変更不要。いずれも通ることを確認した。

## 実行コマンドと結果

- `UPDATE_SCHEMA=1 cargo test -p task-api committed_schema_matches_generated` → `1 passed`
- `UPDATE_SCHEMA=1 cargo test -p task-core event_row_schema_matches_committed` → exit 0
- `UPDATE_SCHEMA=1 cargo test -p task-worker protocol::tests::committed_schema_matches_generated` → `1 passed`
- 再実行（UPDATE なし）:
  - `cargo test -p task-core -- event_row_schema_matches_committed cron::store_tests::cron_job_migration_applies cluster_job::tests::migration_0034 delivery::tests::migration_0042 feed::tests client_open` → `21 passed; 0 failed`
  - `cargo test -p celerisctl --test no_migrate` → 0 failed
  - `cargo test -p task-api committed_schema_matches_generated` → `1 passed`
  - `cargo test -p task-worker protocol::tests::committed_schema_matches_generated` → `1 passed`
- `cargo test --workspace --no-run` → exit 0
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0（`Finished`）
- `corepack pnpm@11.27.0 -C gui gen:types` と `node web/scripts/gen-types.mjs` の後も、生成物以外に差分は出ていない。

## test-parallel の結果と未解決事項

`bash scripts/dev/test-parallel.sh`（TMPDIR を short path にして実行）:

- 5138 tests run: 5106 passed, 32 failed, 13 skipped。doctest exit 0。
- 落ちた 32 件はすべて credentiald（injection_ipc・broker・browser_doctor）、task-api（browser_e2e・browser_waits）、task-worker の browser launcher 系。
- 原因は負荷下の fixture readiness timeout（`timed out waiting for fixture readiness`）。
- 同じ 32 件は単独で通る: `TMPDIR=… cargo nextest run -p celeris-credentiald -p task-api` → 730 passed, 2 skipped。`… -p task-worker -p celeris` → 1667 passed, 10 skipped。
- 既定の TMPDIR（93 文字）では 108 件が `path must be shorter than SUN_LEN` で落ちた。これは run の TMPDIR が長すぎるための環境要因。
- この WU の変更は browser・credentiald の経路に触れていない。落ちた試験の原因を直すのは別件。
- 未解決: 負荷下の fixture readiness 待ちの決定化（browser 系、時間依存）は本 WU の範囲外。親工程で扱うか、人が決める。
- 注意: run の TMPDIR が長く、Unix socket の path 上限 108 バイトを超える。Celeris 側で socket 用の短い dir を渡すか、試験が tempdir の長さに依存しないようにする提案。
