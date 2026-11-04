//! ADR 2026-11-04-project-controller D5・D8（A3）: 人が与えた目的・受け入れ条件と、controller の起票を
//! CoS が独断で変えないための拒否。`/cos/operations` で `instructed_by`（人の発言の引用）が無い要求だけに当てる。
//!
//! - `project.update` で `request`（人の依頼文。正本）を変える → 422 `project_request_human_only`。
//! - `task.update` で draft でない task の `objective`・`acceptance` を変える → 422 `task_goal_human_only`。
//! - controller が `enabled` の案件に root（`parent` なし）を `task.create` で起こす → 422
//!   `project_controller_owns_launch`（続きの root は controller が起票する）。
//!
//! - controller が `off` でない案件（`dry_run`・`shadow`・`enabled`）の案件 CoS（kind `project` の thread の run）が
//!   本番 release を promote する → 422 `production_requires_human`（自律運営の中で本番を変えない）。状態行が無い・
//!   `off` の案件と統括 CoS の promote は従来の gate のまま（I13）。
//!
//! - 同じ範囲の案件 CoS が外部公開（`task.pr_merge`：forge 上の PR を merge する）を行う → 422
//!   `external_publication_requires_human`、不可逆な削除・終端（`repo.delete`・`account.delete`・`org.delete`・
//!   `skill.delete`・`project.cancel`）を行う → 422 `irreversible_requires_human`。
//!
//! 断った要求は呼び出し側が `audit.reject` で `rejected` の監査を残す。secret の書き込み（`PUT /secrets/…`）は
//! registry の EXCLUDED が、routing の段の変更は human_only が断る（試験 `project_cos_refuses_production`）。

use serde_json::Value;
use task_core::TaskStore;
use task_core::project_wake::ControllerMode;
use task_core::store::SqliteStore;

use super::operations::{Matched, unprocessable};
use crate::problem::ApiProblem;

pub(crate) const PROJECT_REQUEST_HUMAN_ONLY: &str = "project_request_human_only";
pub(crate) const TASK_GOAL_HUMAN_ONLY: &str = "task_goal_human_only";
pub(crate) const PROJECT_CONTROLLER_OWNS_LAUNCH: &str = "project_controller_owns_launch";
pub(crate) const PRODUCTION_REQUIRES_HUMAN: &str = "production_requires_human";
pub(crate) const EXTERNAL_PUBLICATION_REQUIRES_HUMAN: &str = "external_publication_requires_human";
pub(crate) const IRREVERSIBLE_REQUIRES_HUMAN: &str = "irreversible_requires_human";

/// 案件 CoS が自律運営の中で独断で行えない操作と、断る code・文。
fn autonomy_refusal(action: &str) -> Option<(&'static str, &'static str)> {
    match action {
        "release.promote" => Some((
            PRODUCTION_REQUIRES_HUMAN,
            "a project CoS does not promote a production release on its own judgment; \
             relay the person's message (instructed_by)",
        )),
        "task.pr_merge" => Some((
            EXTERNAL_PUBLICATION_REQUIRES_HUMAN,
            "merging a pull request publishes the change outside Celeris; a project CoS does \
             this only when relaying the person's message (instructed_by)",
        )),
        "repo.delete" | "account.delete" | "org.delete" | "skill.delete" | "project.cancel" => {
            Some((
                IRREVERSIBLE_REQUIRES_HUMAN,
                "this operation cannot be undone; a project CoS does this only when relaying \
                 the person's message (instructed_by)",
            ))
        }
        _ => None,
    }
}

/// controller が `off` でない案件の案件 thread の run か（I13: 状態行が無い・`off` の案件と統括 CoS は従来の gate）。
fn in_autonomous_project(store: &SqliteStore, thread_id: &str) -> bool {
    let Ok(Some(thread)) = store.chat_thread_get(thread_id) else {
        return false;
    };
    if thread.kind != task_core::chat::ChatThreadKind::Project {
        return false;
    }
    let Some(project) = thread.project_id.as_deref() else {
        return false;
    };
    matches!(store.project_controller_state(project), Ok(Some(state)) if state.mode != ControllerMode::Off)
}

fn present(body: &Value, field: &str) -> bool {
    body.get(field).is_some_and(|v| !v.is_null())
}

/// 断る理由があれば problem を返す。
pub(crate) fn check(
    store: &SqliteStore,
    thread_id: &str,
    matched: Option<&Matched>,
    body: &Value,
) -> Option<ApiProblem> {
    let m = matched?;
    match m.action {
        action if autonomy_refusal(action).is_some() => {
            let (code, detail) = autonomy_refusal(action)?;
            in_autonomous_project(store, thread_id).then(|| unprocessable(code, detail))
        }
        "project.update" if present(body, "request") => Some(unprocessable(
            PROJECT_REQUEST_HUMAN_ONLY,
            "the project request is the person's own text; CoS may change it only when relaying \
             the person's message (instructed_by)",
        )),
        "task.update" if present(body, "objective") || present(body, "acceptance") => {
            let id: task_core::TaskId = m.id.as_deref()?.parse().ok()?;
            let task = store.get(id).ok()??;
            (task.status != task_core::Status::Draft).then(|| {
                unprocessable(
                    TASK_GOAL_HUMAN_ONLY,
                    "the objective and acceptance of a task that is not a draft are the giver's; \
                     CoS may change them only when relaying the person's message (instructed_by)",
                )
            })
        }
        "task.create" if !present(body, "parent") => {
            let project = body.get("project_id")?.as_str()?;
            let state = store.project_controller_state(project).ok()??;
            (state.mode == ControllerMode::Enabled).then(|| {
                unprocessable(
                    PROJECT_CONTROLLER_OWNS_LAUNCH,
                    "the project controller is enabled for this project and launches its next \
                     root; record the reasoning in the brief with project.status_put or relay the \
                     person's message (instructed_by)",
                )
            })
        }
        _ => None,
    }
}
