---
tasks: [01M4MEEKX485VD5SWKX0P30D4Y]
status: complete
title: hint-drop
updated: 2026-10-11
---
# hint-drop

## 状況
`WorkerHint.adapter` と、その task 作成・編集・表示経路を削除し、選択済み harness は `Task.harness_id` に保存する。`RoleSpec` / config role / API の既定値も `harness` 名に変更し、委譲・計画・会話タスクの作成時に task の harness 欄へ写す。移行処理は旧 task JSON の値を登録 harness と照合し、解決不能値に `harness_migration_unresolved` を記録する。

旧 JSON migration 試験は旧 task JSON を DB に直接作る形へ変更。Rust source に `worker_hint.adapter` の連結表記は残していない。Schema は `UPDATE_SCHEMA=1 cargo test -p task-core -p task-api schema_matches` で更新・確認した。

## 検査
- `cargo check --workspace --all-targets`: 成功。
- `cargo test -p task-core`: 907 passed。
- `TMPDIR=/tmp bash -c '( cargo test -p task-core && cargo test -p task-api ) && ( test -s agent-docs/progress/2026-10-11-llm-source-harness-runtime/hint-drop.md && cargo clippy --workspace --all-targets -- -D warnings )'`: 成功。task-core 907 tests、task-api 全試験、workspace all-targets clippy を確認。
- 前回不合格だった `! sed -n '/pub struct WorkerHint/,/^}/p' crates/task-core/src/model.rs | grep -q adapter && ! grep -rnE 'worker_hint\.adapter' crates --include='*.rs'`: 成功。
- `sh "$CELERIS_WU_SCOPE_PATHS"`: 成功。変更ファイル一覧は crates、docs/api/v1、agent-docs/progress、tests/e2e のみ。

深い run の既定 `$TMPDIR` では credentiald の Unix socket path が長すぎて readiness に失敗したため、試験 fixture のみ `/tmp` に置く `TMPDIR=/tmp` で実行した。cargo build target は渡された `CARGO_TARGET_DIR` のまま。

## 未解決
- なし。
