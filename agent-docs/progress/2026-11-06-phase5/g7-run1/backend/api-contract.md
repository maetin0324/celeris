---
title: chat/controller の公開契約と schema・文書（api-contract、Phase 4）
tasks: [01M6RJTQQ629D0NP953708JMS4]
status: done
updated: 2026-11-06
completed: 2026-11-06
---
# api-contract（Phase 4）

完了日: 2026-11-06。正本 ADR: [2026-11-06-cos-run-launch-accounting-and-drain](../../../../adr/2026-11-06-cos-run-launch-accounting-and-drain.md) D5。新しい設計判断はなく、ADR は足していない。
永続 API の使い方と未接続箇所は [durable-contract.md](durable-contract.md)。

## 変更（commit は件名で列挙）
- `phase 4: chat/controller の公開契約（not_launched_runs）と HTTP 回帰試験・API 文書`

- `ProjectBudgetToday.not_launched_runs`（`task-core/src/project_wake.rs`）を足し、`task-api/src/handlers/project_controller.rs` が `BudgetUsage.not_launched_runs` を写す。
- chat run view の `launched_at`・`prelaunch_reason`・`cost_state` は task-core の `ChatRun` に既にあり、list・detail・run event・thread の `active_run` は同じ型で出る。task-api 側の追加実装は無く、HTTP の回帰試験で固定した。
- `docs/api/v1/api.md` に起動境界・旧行 fail closed・attempt 欠測と既知 usage の保存・store 通知と後続接続の責務の節と、`budget_today.not_launched_runs` を書いた。
- schema・web 生成型: `UPDATE_SCHEMA=1 cargo test -p task-api --lib committed_schema_matches_generated && node web/scripts/gen-types.mjs` を実行し差分なし。`ChatRun` の 3 欄は生成済みで、`ProjectControllerView` は OpenAPI の schema 対象外。

## 修正前の回帰
- コマンド: `cargo test -p task-api --test project_controller budget_today_counts`（`not_launched_runs` の公開前）
- exit: 101。assertion: `budget_today` に `not_launched_runs` が無く `left: Null  right: 0`（`project_controller.rs` の `before["not_launched_runs"]`）。
- chat 側 `cargo test -p task-api --test chat_api chat_api_launch_boundary` の最初の実行は、`launched_at`/`prelaunch_reason` が null で返る前提の assertion で失敗した（`launched_at must be null, got {…"cost_state":"not_launched"…}`）。実装は欄を省略する型で、試験の前提を「省略 = null」に直した。view の不具合を直したものではない。

## 修正後
- 同じ 2 コマンド: それぞれ 1 passed。chat の試験は起動前終了（`not_launched`・`route_unavailable`）、起動後 usage 無し・旧行・attempt 一部欠測（`unknown`）、既知費用（`known`）、RFC 3339、list/detail の一致、run event、thread の `active_run` を検証する。controller の試験は除外数 2、起動後 usage 無しが `usd_unknown_runs` 1 で除外数は不変を検証する。
- 提出時の HEAD で実行（`$CELERIS_WU_BASE` 以後、最後の同期は無し）:
  - `bash scripts/dev/test-affected.sh`: exit 0（nextest と doctest 通過）。
  - `cargo clippy --workspace -- -D warnings`: exit 0。
  - `cargo fmt --all -- --check`: exit 0。
  - 範囲: `git diff --name-only $CELERIS_WU_BASE` は task-api 3 ファイル、`project_wake.rs`、`docs/api/v1/api.md` のみ（進捗ファイルを除く）。

## 未解決・提案
- `ChatRun.launched_at`・`prelaunch_reason` は None のとき欄ごと省略され、null を返さない（`skip_serializing_if`）。依頼の「RFC3339/null」は「省略 = null」と読んで文書に書いた。常に null を出すなら `task-core/src/chat/mod.rs` の 2 欄の `skip_serializing_if` を外し、schema と web 型の再生成が要る。この WU の編集範囲外なので変えていない。
- web の型検査・unit と生成型の鮮度は次の web-contract WU で確認する。
