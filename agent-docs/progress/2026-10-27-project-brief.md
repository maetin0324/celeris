---
title: 案件の状態の要約を構造化して永続化する（secretary_summary を置き換える）
tasks: [01M5XXF7QH5BSQJTVW7ETJ3Y6T]
status: done
updated: 2026-10-27
---
# 案件の状態の要約を構造化して永続化する（secretary_summary を置き換える）

完了日: 2026-10-27

判断は [ADR 2026-10-27-project-brief](../adr/2026-10-27-project-brief.md)。

## 変更

- migration 0095（SCHEMA_VERSION 95）: `project_briefs` を作り、`projects.secretary_summary` に値のあった行を `state` に写してから列を消す。
  `Project.secretary_summary` と全ての literal、celerisctl `projects show` の表示、web の参照を消した。
- `crates/task-core/src/project_brief.rs`: `ProjectBrief`・`ProjectBriefInput`（検査）・`WaitingOn`（human / task / external）・`ProjectActivity`。
- `crates/task-core/src/store/project_briefs.rs`: `project_brief_get/list/put/put_tx`、`project_activity`（task の状態別件数と `updated_at` の最大）。
- task-api: `PUT /projects/{id}/status`（管理系、`updated_by = human`）と CoS op `project.status_put`（その案件の案件 CoS スレッドだけ。
  他は 403 `project_brief_forbidden` で rejected）。`GET /projects` の項目に `brief`・`task_counts`・`last_activity_at`・`human_waits`・
  `oldest_human_wait_at`（受信箱と同じ `human_inbox` から数える）、`GET /projects/{id}` に `brief`。
- CoS chat: `CosChatContext.projects`（`CosChatProjectSummary`）。統括 CoS は全案件（アーカイブ除く）、案件 CoS は自分の案件。
  dispatcher `cos_chat/projects.rs` が store から決定的に組み、worker の prompt に「案件の要約」節を出す。
- 文書: `docs/api/v1/api.md`（表・§3.46・§3.48a・CoS op 表）、ADR 2026-10-09 の表、`config/skills/cos-operator/operations.md`、ADR-0033 付記。
- schema 再生成: `docs/api/v1/api-v1.schema.json`・`docs/protocol/worker-protocol.schema.json`・web の生成型。

## 試験（提出時の HEAD で実行。基準は `git merge-base HEAD main`）

- `bash scripts/dev/test-affected.sh` → exit 0（Rust 6505 run / 6505 passed、web Vitest 682 passed・98 files、tsc -b 通過）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- 追加した試験: task-core `project_brief::tests`（検査 4 件）・`store::project_briefs::tests`（置換・案件なし・活動・migration 0095 の写し）、
  task-dispatch `cos_chat::projects::tests`、task-worker `cos_chat_prompt_projects_section_lists_summaries_and_points_to_the_api`、
  task-api `tests/project_brief.rs`（admin PUT と一覧・詳細、件数・最終活動・人待ち、CoS op の案件 CoS 限定）。

## 本番への移行

- migration 0095 が起動時に当たる。手作業は無い。

## 未解決事項

- 案件 CoS が要約をいつ書き直すかは prompt の指示（「状態が変わったら」）だけで、書き忘れの検出は無い。

## 提案

- `GET /projects` は人待ちのために受信箱を毎回組み立てる。一覧が重くなったら受信箱の案件別集計を store に持たせる。
