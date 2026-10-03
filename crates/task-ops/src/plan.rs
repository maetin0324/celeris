//! `celerisctl plan` の判断 — DESIGN.md §5.9 / ADR-0007 D6 / ADR-0010 D4（P-19, ADR-0013 D7）。
//!
//! 大目標を表す文字列 1 つから根の `Plan` タスクを組み立て、`TaskStore::create_task` で
//! `insert` + `Event::Created` を単一トランザクションとして書き込む（ADR-0010 D2）。
//! 子タスクの生成はプランナー（ワーカー）の出力を Reviewer が検証・展開する経路で行うため、
//! ここでは `acceptance = []`（暗黙のプラン検証条件のみ。ADR-0007 D4）で `Draft` の
//! 1 タスクを作るだけに留める。根の Plan のみを作る（`parent_id` は常に `None`）。
//! `workspace` を省略した場合は `WorkspaceSpec::Local{ path: "<task_id>" }`（相対パス、P-19）。

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::{
    Budget, Status, Task, TaskId, TaskKind, TaskStore, Tier, WorkerHint, WorkspaceSpec,
};
use time::OffsetDateTime;

use crate::error::OpsError;

const TITLE_MAX_CHARS: usize = 80;

/// `celerisctl plan` から組み立てる新規 Plan タスクの指定。API の `POST /plans` の本文でもある（`docs/api/v1/gui-api.md` §3.14）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewPlanSpec {
    /// 大目標。1行目の先頭80文字が `title` になる。
    pub goal: String,
    #[serde(default)]
    pub workspace: Option<PathBuf>,
    #[serde(default = "default_plan_tier")]
    pub tier: Tier,
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "default_plan_max_turns")]
    pub max_turns: u32,
    #[serde(default = "default_plan_max_wall_secs")]
    pub max_wall_secs: u64,
    #[serde(default = "default_plan_max_retries")]
    pub max_retries: u32,
}

fn default_plan_tier() -> Tier {
    Tier::Frontier
}
fn default_plan_max_turns() -> u32 {
    30
}
fn default_plan_max_wall_secs() -> u64 {
    900
}
fn default_plan_max_retries() -> u32 {
    1
}

/// 文字列の1行目を、char境界を保ったまま先頭 `max_chars` 文字に切り詰める。
fn truncate_title(goal: &str, max_chars: usize) -> String {
    let first_line = goal.lines().next().unwrap_or("");
    first_line.chars().take(max_chars).collect()
}

/// `spec` から `Plan` kind の `Task` を組み立て、`store.create_task` で原子的に挿入する。
pub fn create_plan(
    store: &dyn TaskStore,
    spec: NewPlanSpec,
    now: OffsetDateTime,
) -> Result<Task, OpsError> {
    if spec.goal.trim().is_empty() {
        return Err(OpsError::Validation("goal must not be blank".to_string()));
    }

    let title = truncate_title(&spec.goal, TITLE_MAX_CHARS);

    let id = TaskId::new();
    let workspace = match spec.workspace {
        Some(path) => WorkspaceSpec::Local { path, mode: None },
        None => WorkspaceSpec::Local {
            path: PathBuf::from(id.to_string()),
            mode: None,
        },
    };

    let budget = Budget {
        max_turns: spec.max_turns,
        max_wall_secs: spec.max_wall_secs,
        max_retries: spec.max_retries,
    };

    let task = Task {
        tree: None,
        paused_at: None,
        routing: None,
        repos: Vec::new(),
        id,
        parent_id: None,
        kind: TaskKind::Plan,
        title,
        objective: spec.goal,
        acceptance: vec![],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Draft,
        priority: spec.priority,
        worker_hint: WorkerHint {
            tier: spec.tier,
            adapter: None,
        },
        workspace,
        budget,
        attempts: 0,
        lease: None,
        created_at: now,
        updated_at: now,
        role: None,
        genre: None,
        aggregate: false,
        // ADR-0046 D2 / D4（Phase 59）: `celerisctl plan` の根は既定（能力タグ無し・production）。
        skills: Vec::new(),
        mode: task_core::TaskMode::default(),
        project_id: None,
        milestone_id: None,
        assignee: None,
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    };

    store.create_task(&task, vec![])?;
    Ok(task)
}

#[cfg(test)]
mod tests;
