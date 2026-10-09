---
tasks: [01M4F5KS8E1MXZDNJTRAVFJESW]
wu: ops-projects-cron
status: running
---
# ops-projects-cron: tasks の C 3 route・projects 14・cron-jobs 6 の監査付き登録

## cron-jobs 6 route（完了）

- B（caller-owned transaction）: `POST /cron-jobs`・`PATCH /cron-jobs/{id}`・`DELETE /cron-jobs/{id}`・`POST …/pause`・`POST …/resume`。
  task-core に `cron_job_{insert,update,delete,run_update}_tx` を足し（既存の trait 実装も同じ関数を使う）、task-ops に
  検証だけの `prepare_{create,update,pause,resume}` を分けた。API の `crates/task-api/src/cron_jobs.rs` の
  `*_job_op(…, audit: Option<&OperationAudit>)` を handler と `/cos/operations` が共有し、CoS では job の変更・operation 行・
  監査 event・chat card を同じ transaction で書く。名前の重複は 409（rejected で記録）。
- C（外部効果の手順）: `POST /cron-jobs/{id}/run`。手動実行は task 作成を含む複数 commit なので
  `begin_external`（pending + 監査 event）→ `run_now_with` を 1 回 → `finish_external`（applied）。同じ idempotency key の
  再送は記録を返して再実行しない。begin 後に実行が失敗した場合は pending のまま残り、起動時の stale 回収で
  needs_remediation になる（部分的な効果の有無が分からないため）。
- `celerisctl cron create|update|pause|resume|run` は CoS credential のとき `cos_mapped` で同じ operation を送る
  （`commands/cron.rs` の `request_of` を共有）。
- skill の操作表（`config/skills/cos-operator/operations.md`）、`docs/api/v1/gui-api.md`（と `scripts/sync-gui-docs.sh` の写し）、
  `docs/api/cron-jobs.md` を更新。gui 写しにだけあった plan_adopt/tree.adopt の段落は正本へ移した。

検証:
- `cargo nextest run -p task-api --test cos_ops_projects_cron --test cos_ops_registry --test cron_jobs` → 13 passed
  （経由 applied＋監査・直接 422・409/404/422 の rejected 記録・run の pending→applied と再送で再実行しない）
- `cargo nextest run -p task-api --lib cos` → 4 passed（skill 表と ALLOWED の一致を含む）
- `cargo nextest run -p task-ops cron` → 14 passed、`cargo nextest run -p task-core cron` → 19 passed
- `cargo nextest run -p celerisctl cos_mapped cron` → 11 passed
- `cargo clippy -p task-core -p task-ops -p task-api -p celerisctl --all-targets -- -D warnings` → 成功、`git diff --check` → 成功

## projects lifecycle 5 route（完了）

`POST /projects/{id}/{cancel,pause,resume,archive,unarchive}` を B で登録。task-ops の `plan_project_action` が直接経路と
同じ 404/409 規則で書き込みを決め、`apply_project_change_tx` が案件の状態・中止の連鎖（各 task を transaction 内で
読み直して `apply_transition_tx(ProjectCancelled)`）・途中目標の中止を caller-owned transaction で書く。task-core に
`project_set_lifecycle_tx`・`project_set_archived_at_tx`・`milestone_set_lifecycle_tx`・`task_status_tx` を追加。
handler と CoS は `lifecycle::project_action_op` を共有。celerisctl の `projects` は読み取りだけなので包みは不要。

検証: `cargo nextest run -p task-api --test cos_ops_registry --test lifecycle --test cos_operations_domains --test cos_ops_projects_cron`
→ 21 passed、`cargo nextest run -p task-ops lifecycle` → 10 passed、task-api lib cos 4 passed、clippy（3 crate all-targets）成功。

## standing-rules 2 route（完了）

人の決定（standing-rules の作成・削除を CoS にも許す）どおり `POST /standing-rules`（`standing_rule.create`）と
`DELETE /standing-rules/{id}`（`standing_rule.delete`）を B で登録。task-core に `standing_rule_delete_tx`、
API に `create_standing_rule_op`・`delete_standing_rule_op`（handler と共有）。`cos_operations.rs` の「未登録は拒否」
試験は登録済みになった cron・project pause・standing-rules から、除外（console/instruct・secrets）と未知 path に差し替えた。

検証: `cargo nextest run -p task-api` → 631 中 626 passed。落ちた 5 件は browser_e2e/browser_waits の fixture 待ち
（run の TMPDIR 93 文字で Unix socket path 上限）で、短い TMPDIR で `--test browser_e2e --test browser_waits` を流すと
10 passed。clippy（task-core・task-api all-targets）成功。

## 残り

- projects PENDING 7: `POST /projects`、`/projects/{id}/docs/{init,maintenance}`、
  `PUT|DELETE /projects/{id}/docs/page`（KB/git は C）、`POST /reports/{notified,read}`
- tasks の C: `POST /tasks/{id}/changes/{repo}/integrate`、`/pr/merge`。decisions の C: `PUT /knowledge/page`
