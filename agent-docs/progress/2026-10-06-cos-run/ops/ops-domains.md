# /cos/operations の残りの領域（approval・execution・project・knowledge）を共有操作関数へ

---
tasks: [01M47997XM2GYH4E5J2QNAJZ7R]
status: done
completed: 2026-10-06
---

## 許可表 `ALLOWED`（`crates/task-api/src/cos/operations.rs`）の最終一覧

| 領域 | method | path | action | 共有操作関数（handler は `audit = None`） | target_kind |
|---|---|---|---|---|---|
| task | POST | `/api/v1/tasks` | `task.create` | `handlers::tasks::create_task_op` | task |
| comment | POST | `/api/v1/tasks/{id}/comments` | `comment.create` | `handlers::task_actions::create_comment_op` | task |
| decision | POST | `/api/v1/decisions/{id}/answer` | `decision.answer` | `decisions::answer_op` | decision |
| approval | POST | `/api/v1/approvals/{id}/decide` | `approval.decide` | `approvals::decide_op` | approval |
| execution | POST | `/api/v1/tasks/{id}/execution/phase-gate` | `execution.phase_gate` | `execution::phase_gate_op` | task |
| project | PATCH | `/api/v1/projects/{id}` | `project.update` | `handlers::projects::set_project_text_op`（CoS 入口 `cos_patch_project`） | project |
| knowledge | POST | `/api/v1/knowledge/inbox/{id}/reject` | `knowledge.reject` | `knowledge::reject_op` | knowledge |

上記以外（URL・`..`・`/api/v1/cos` 再帰・未登録・method 不一致）は従来どおり 422 `cos_operation_not_allowed` で、rejected 行と理由つき監査 event を残す。

## この葉で足したこと
- approval: `task_ops::approval::decide` を `plan_decide`（検証・答える相手・standing の規則）と `finish_decide`（`blocked` task の再開）に分けた。CoS は決定の行と standing rule を `cos_operation_apply` の transaction 内で書く（task-core `SqliteStore::approval_decide_tx`・`standing_rule_append_tx`）。
- execution: `task_ops::phase_gate::plan_phase_gate`（`PhaseGatePlan::{Withdraw, Resume}`）。CoS の continue/replan は遷移と人のメモの `Answered` を `apply_transition_tx`（pub に）で同じ transaction に書く。withdraw は中止の連鎖が 1 transaction に収まらないので CoS は 422（人が取り下げる）。
- project: CoS が変えられるのは `title`/`request` だけ（`CosProjectPatchBody`、deny_unknown_fields。status・workspace・slug は 422）。検証は handler と共有の `project_text_input`、書き込みは `project_set_text_tx`。
- knowledge: KB は git（SQLite の外）なので、候補の取り下げを apply の closure 内で行う。取り下げに失敗すれば cos_operations の applied 行・監査 event・card は残らない。
- 既存 handler（approvals decide・phase-gate・PATCH projects の名前/説明・knowledge reject）は同じ関数を `None` で呼ぶ（挙動は不変。既存試験 approvals 25・execution 14・knowledge・project_plan 8 は通る）。

## 証拠
- `cargo test -p task-api --test cos_operations_domains` → 7 passed（`cos_chat_ops_domain_{task_create,comment_create,decision_answer,approval_decide,execution_phase_gate,project_update,knowledge_reject}`。各領域で CoS credential の直接呼び出しが 422 `cos_audit_context_required`・領域の書き込みなし、operation 経由が applied で監査 envelope event 1 件と chat card event が残る）
- `cargo test -p task-api --test cos_operations --test cos_auth --test approvals --test execution --test knowledge --test project_plan` → 全 pass
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0、`cargo fmt --all --check` → exit 0
- `bash scripts/dev/test-parallel.sh` → exit 0、4060 passed / 0 failed / 13 ignored

## 未解決事項
- 各領域は代表操作 1 つだけ。approval の standing rule 削除、execution の plan-gate・decompose・tree/adopt・execution-plan、project の status/workspace/slug・pause/cancel、knowledge の accept・page 書き込みは許可表に無い（人の管理操作のまま）。
- 監査ありの後始末は commit 後の別書き込み: approval の `blocked` task 再開（`gate::answer`）。人の経路と同じ処理。
- knowledge reject は git commit の後に SQLite の commit が失敗すると、監査の無い取り下げが残りうる（KB が DB の外のため）。
- `expected_revision` は記録のみ（ops-api と同じ）。

## 提案
- 許可表を増やすときは、1 transaction に収まらない操作（中止の連鎖など）を CoS に開けるか ADR で決める。
