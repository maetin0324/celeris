//! `celerisctl plan-lint`（ADR-0067 D5。Phase 111）。
//!
//! DB 上の `draft` / `ready` のタスクの受け入れ条件を、計画時の検証（ADR-0067 D2:
//! `task_core::validate_human_checks_have_deliverable`）と同じ規則で点検し、違反を一覧する。
//! **読み取り専用**（直しはしない。`ready_tasks`/`list` と同じ `TaskStore` の読み取りだけで、
//! LLM 呼び出しも書き込みも無い）。

use std::process::ExitCode;

use task_core::{Status, TaskStore, validate_human_checks_have_deliverable};

use crate::error::CliError;
use crate::outln;

/// 点検対象のタスク 1 件の違反。
pub struct Violation {
    pub task_id: task_core::TaskId,
    pub title: String,
    pub status: Status,
    pub reason: String,
}

/// DB 上の `draft`/`ready` タスクを D2 の規則で点検する（純粋な読み取り。判断は
/// `validate_human_checks_have_deliverable` に委ねる）。
pub fn lint(store: &dyn TaskStore) -> Result<Vec<Violation>, CliError> {
    let mut violations = Vec::new();
    for status in [Status::Draft, Status::Ready] {
        for task in store.list(Some(status))? {
            if let Err(reason) = validate_human_checks_have_deliverable(&task.acceptance) {
                violations.push(Violation {
                    task_id: task.id,
                    title: task.title,
                    status: task.status,
                    reason,
                });
            }
        }
    }
    violations.sort_by_key(|v| v.task_id);
    Ok(violations)
}

pub fn run(store: &dyn TaskStore) -> Result<ExitCode, CliError> {
    let violations = lint(store)?;
    if violations.is_empty() {
        outln!(
            "違反はありません（draft/ready のタスクの human チェックには全て artifacts か知識ベースの参照が付いています）。"
        );
        return Ok(ExitCode::SUCCESS);
    }
    for v in &violations {
        outln!("{} [{:?}] {:?}: {}", v.task_id, v.status, v.title, v.reason);
    }
    outln!("{} 件の違反。", violations.len());
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
#[path = "plan_lint_tests.rs"]
mod tests;
