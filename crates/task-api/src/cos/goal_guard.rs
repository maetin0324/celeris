//! ADR 2026-11-04-project-controller D5・D8（A3）: 人が与えた目的・受け入れ条件と、controller の起票を
//! CoS が独断で変えないための拒否。`/cos/operations` で `instructed_by`（人の発言の引用）が無い要求だけに当てる。
//!
//! - `project.update` で `request`（人の依頼文。正本）を変える → 422 `project_request_human_only`。
//! - `task.update` で draft でない task の `objective`・`acceptance` を変える → 422 `task_goal_human_only`。
//! - controller が `enabled` の案件に root（`parent` なし）を `task.create` で起こす → 422
//!   `project_controller_owns_launch`（続きの root は controller が起票する）。
//!
//! 断った要求は呼び出し側が `audit.reject` で `rejected` の監査を残す。本番 promote・secret・外部公開・
//! 不可逆な操作は registry の EXCLUDED（既存の gate）が断るので、ここでは扱わない。

use serde_json::Value;
use task_core::TaskStore;
use task_core::project_wake::ControllerMode;
use task_core::store::SqliteStore;

use super::operations::{Matched, unprocessable};
use crate::problem::ApiProblem;

pub(crate) const PROJECT_REQUEST_HUMAN_ONLY: &str = "project_request_human_only";
pub(crate) const TASK_GOAL_HUMAN_ONLY: &str = "task_goal_human_only";
pub(crate) const PROJECT_CONTROLLER_OWNS_LAUNCH: &str = "project_controller_owns_launch";

fn present(body: &Value, field: &str) -> bool {
    body.get(field).is_some_and(|v| !v.is_null())
}

/// 断る理由があれば problem を返す。
pub(crate) fn check(store: &SqliteStore, matched: Option<&Matched>, body: &Value) -> Option<ApiProblem> {
    let m = matched?;
    match m.action {
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
                     root; report with project.controller_report or relay the person's message \
                     (instructed_by)",
                )
            })
        }
        _ => None,
    }
}
