---
title: task 一覧 API: ?with=integrations で付加、要求ごと 1 回の map 化（WU api-with）
updated: 2026-10-21
tasks: [01M5EDRV4B2K5NYPMF9EZ0YN9A]
status: done
completed: 2026-10-21
---
# WU api-with

ADR: `agent-docs/adr/2026-10-21-task-list-opt-in-integrations.md` D1・D2。

## 実施
- `GET /tasks` は既定で integrations / promotion を付けない。`?with=integrations`（未知の値は 400）のときだけ付ける。旧い常時付加ループは削除。
- `IntegrationAttacher`（tasks.rs）: `task_integration_list` を要求ごと 1 回読んで task_id→Vec の map にし、repo の解決（`repo_get` + `resolve_default_branch`）は repo ごと 1 回、promotion の最新 release は `NewestRelease::read` で要求ごと 1 回。単体 GET も同じ attacher（従来どおり常に付く）。
- `task_ops::view::attach_integrations` は store を読まず、呼び出し側が渡した states を使う。`stranded::promotion_view_for` / `NewestRelease` を追加（`promotion_view` は薄い wrapper）。`IntegrationContext.releases_dir` は `release` に置換。
- 試験用の注入点: `task_ops::changes::git_spawns_in(dir)`（git -C dir の起動回数）。
- 付加される repo は「状態の行が名指す repo」だけ（案件の repo 全件の `repo_list` は不要なので廃止。view は元から状態の行が名指す repo だけ）。
- `docs/api/v1/api.md`・`api-v1.schema.json` を再生成/更新（`UPDATE_SCHEMA=1 cargo test -p task-api`）。gui/ web/ の生成型・画面は次 stage。

## 証拠
- `cargo test -p task-api --test task_list_with`: 3 passed（既定は git 起動 0（N=1,20）、with=integrations は git 起動数が N=1 と N=20 で等しく >0、reader 接続の増分も等しい、未知 with は 400）。
- `bash scripts/dev/test-affected.sh`: exit 0（5007 passed, 16 skipped）。`cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- celeris/tests/integration_api.rs の一覧は `?with=integrations` に追従。

## 未解決事項
- `UPDATE_SCHEMA=1 cargo test -p task-api` 全体は sandbox で `browser_live_cross`（userns）が失敗する（本件と無関係）。schema は先に生成済み。

## 本番への移行
config・DB の変更なし。一覧の既定応答から integrations / promotion が消えるので web 追従（web-with）と同時に release する。
