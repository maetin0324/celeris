---
tasks: [01M4F5KS8E1MXZDNJTRAVFJESW]
wu: ops-projects-cron
status: done
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

## reports 2 route（完了）

`POST /reports/read`（`report.read`、B）: task-core に `report_mark_read_tx` を足し、報告と対応する通知
（`notice_mark_ids_read_tx`）を監査と同じ transaction で既読にする。handler と `mark_read_op` を共有。
`POST /reports/notified`（`report.notified`）: 効果は API process のメモリの通知時刻（DB 列なし）。`DispatchEnv` に
`api: ApiState` を足し、`audit.apply` の closure の中で進める（同じ key の再送は closure を走らせないので進めない）。

検証: `cargo nextest run -p task-api --test reports --test inbox_notifications --test cos_ops_registry --test cos_operations`
→ 23 passed、`cos_ops_projects_cron` 6 passed、clippy 成功。

## POST /projects（完了）

`project.create`（B）: task-core の `project_create_impl` を `project_create_tx`（slug 決定と primary repo 行を含む）に分け、
案件の作成を監査と同じ transaction で書く。秘書の最初の返事（`greet_the_secretary`）は直接経路と同じく commit 後の
best-effort。同じ key の再送は新しい id を書かないので、返事も起きない。handler と `create_project_op` を共有。

検証: 短い TMPDIR で `cargo nextest run -p task-api` → 633 passed、clippy 成功。

## projects docs 4 route（完了、C）

`POST /projects/{id}/docs/init`（`docs.init`）、`PUT|DELETE /projects/{id}/docs/page`（`docs.page_put`・`docs.page_delete`）、
`POST /projects/{id}/docs/maintenance`（`docs.maintenance`。記録の action は `docs.maintenance_<op>`）を外部効果の手順で登録。
- task-core に `cos_operation_fail_external`（pending → rejected。`finish_external` と共通の settle）を追加。外部の系が変更前に
  拒否したと分かる失敗だけに使う。
- API に `OperationAudit::external`（begin → effect を 1 回 → finish / fail。再送は effect を走らせない）と `fail_external`。
- docs は `docs_init_op`・`put_page_op`・`delete_page_op`・`maintenance_op` を handler と共有。etag 不一致・main 編集中・
  page 無し・path の誤りは `rejected`。maintenance は apply 後の失敗だけ `docs_maintenance_partial` にして pending のまま残す。
- CoS の DELETE は path/etag/message を body で送る（operation の path に query を付けられないため）。

検証: `cos_ops_projects_cron_docs` 2 passed（local の一時 git repo。commit 回数で再送の非再実行と rejected の無変更を確認）、
`--test docs --test cos_ops_registry --test cos_ops_projects_cron --test cos_operations` 28 passed、task-api lib cos 4 passed、clippy 成功。

## PUT /knowledge/page（完了、C）

`knowledge.page_put`: KB は git なので外部効果の手順。`put_page_op` を handler と共有し、KB 未用意・path の誤り・`_inbox/` は
効果の前に拒否（rejected 行）、etag 不一致と page 無しは git の変更前拒否として `rejected`、その他の失敗は pending。
CoS の編集も人の依頼の中継として人の印（`mark_human_authored`）で保存し、誰が中継したかは監査の記録に残す。

検証: `cos_ops_projects_cron_docs`（KB 試験を含む 3 件）・knowledge・cos_ops_registry・task-api lib → 102 passed、clippy 成功。

## tasks の取り込み 2 route（完了、C）

`POST /tasks/{id}/changes/{repo}/integrate`（`task.integrate`）と `…/pr/merge`（`task.pr_merge`）を外部効果の手順で登録。
`changes.rs` を「効果の前の検査」（discard の confirm、木の子、repo、PR の origin/gh、開いた PR）と「効果」（git・`gh`・取り込み
記録）に分け、`integrate_op`・`pr_merge_op` を handler と共有。検査の失敗は rejected 行、`default_branch_busy` は git の変更前
拒否として `rejected`、merge 失敗・衝突・`gh pr merge` 失敗は取り込み記録（failed/conflict）として applied、それ以外は pending。
CoS の記録の target は task（task の event 列に監査が積まれる）。celerisctl には取り込みの subcommand が無いので包みは不要。

検証: `cos_ops_projects_cron_changes` 2 passed（偽の `gh` の呼び出し記録と local bare origin。merge の再送で main が動かない、
`gh pr merge` は再送しても 1 回、閉じた PR は 409）、`--test changes` 合格、短い TMPDIR で `cargo nextest run -p task-api`
→ 638 passed、clippy（task-core・task-api all-targets）成功。

## 残り

- projects PENDING: 無し
- tasks・decisions・projects の PENDING は空。admin の PENDING に cron-jobs は無い（受け入れ条件 0）。残りは全体検査

## 全体検査（2026-10-09、HEAD 03e04e4d の上）

- `cargo clippy --workspace -- -D warnings` → exit 0
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` → exit 0（passed 4936、failed 0、ignored 14、doctest exit 0）
- run の既定 TMPDIR（93 文字）では exit 100: 63 件が `path must be shorter than SUN_LEN`（browser launcher・credentiald 等の
  Unix socket）。短い TMPDIR（`/tmp/cx.XXXX`）でも script 内の `<TMPDIR>/celeris-test-parallel.XXXXXX/tmp` が長く、
  `browser_e2e` 4 件が fixture 待ちで時間切れ（単独では 4 passed）。どちらもこの WU の変更と無関係の環境要因。

## 提案

- test-parallel.sh の内側 TMPDIR は Unix socket を張る試験の SUN_LEN 上限に当たりやすい。内側 dir 名を短くする
  （例: `ctp.XXXX/t`）か、socket だけ短い専用 dir に作る試験基盤の整理を別 task で検討する。
