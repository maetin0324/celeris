//! `POST /tasks/{id}/retry`（Phase 31。実機の事故、2026-09-18）: 失敗した仕事をやり直す。
//!
//! 「進行不能になった案件の調査タスクを人が一手でやり直す」手段が API にも GUI にも無かった事故から。
//! `failed` または `cancelled` のタスクを**複製して新しいタスクを作る**（`Failed`/`Cancelled` を非終端に
//! 戻す状態機械の遷移は足さない。DESIGN の状態機械を壊さないため）。`depends_on` は元と同じにし、
//! 元のタスクに依存していた未終端（またはその依存の失敗で `cancelled` になった）タスクの `depends_on` を
//! 新しい id に張り替える（後者は `draft` に戻す）。書き込みは `TaskStore::retry_task` に任せ、ここでは
//! 検証と `Task` の組み立てだけを行う。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::{Status, Task, TaskId, TaskStore, WorkspaceSpec};
use time::OffsetDateTime;

use crate::error::OpsError;

/// `POST /tasks/{id}/retry` の応答（201）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RetryResult {
    /// 新しく作られたタスク。
    pub task_id: TaskId,
    /// `depends_on` を新しいタスクへ張り替えた（元は `original` に依存していた）タスクの id。
    #[serde(default)]
    pub rewired: Vec<TaskId>,
}

/// `id` のタスクが `failed`/`cancelled` でなければ `OpsError::InvalidState`（API は 409）。
/// それ以外の検証は無い（複製元は既に一度検証を通っている）。
///
/// ADR-0062 Phase 108 追記: `workspace` を与えると複製先の作業場所をそれに差し替える（省略時は従来どおり
/// 元のタスクの `workspace` を複製する）。検証は `PATCH /tasks/{id}` の `workspace` と同じ規則
/// （`Remote.cluster` は API 層〈`validated_workspace`〉が見る。ここでは `Local.path` が空でないこと、
/// 明示の `Remote` で元の担当が `cluster:<id>` を持たなければ 422、を見る）。
pub fn retry_task(
    store: &dyn TaskStore,
    id: TaskId,
    accept: bool,
    workspace: Option<WorkspaceSpec>,
    now: OffsetDateTime,
) -> Result<RetryResult, OpsError> {
    retry_task_with_execution(store, id, accept, workspace, None, "human", now)
}

/// ADR-0072「Phase F6 実装時の決定」: [`retry_task`] に、複製先の実行の形（`execution`）の人の明示を
/// 足したもの。`Some(mode)` なら、複製を作った直後に [`crate::regate::set_execution_mode`] で
/// `execution_hint = {mode, explicit: true}` を書き、`Event::ExecutionHintSet{source}` を複製先に残す
/// （`source` は `"human"` / `"mcp:<client_id>"`）。gate の対象外のタスクに `execution` を書くと
/// 複製の**前に** `OpsError::Validation`（API は 422）。
///
/// どちらの場合も、複製先の `routing.execution`（元の gate の判定）は**引き継がない**: 複製先の最初の
/// dispatch で gate が今の設定（`[execution] gate`）で判定し直し、複製先自身の `ExecutionGated` を残す
/// （本番 2026-09-28: shadow の判定 `shadow: true / source: hint` が複製に写り、gate=on の下で古い判定の
/// まま planner に進み、GUI にも古い判定が出た）。`execution_hint`（人の明示・CoS のヒント）は引き継ぐ。
#[allow(clippy::too_many_arguments)]
pub fn retry_task_with_execution(
    store: &dyn TaskStore,
    id: TaskId,
    accept: bool,
    workspace: Option<WorkspaceSpec>,
    execution: Option<task_core::ExecutionMode>,
    source: &str,
    now: OffsetDateTime,
) -> Result<RetryResult, OpsError> {
    let original = store.get(id)?.ok_or(OpsError::NotFound(id))?;
    if !matches!(original.status, Status::Failed | Status::Cancelled) {
        return Err(OpsError::InvalidState {
            id,
            context: format!("status={:?}", original.status),
            action: "retried".to_string(),
        });
    }
    if execution.is_some() && task_core::execution_gate::out_of_scope_rule(&original).is_some() {
        return Err(OpsError::Validation(
            "execution: this task is out of scope of the Complexity Gate (it always runs atomic)"
                .to_string(),
        ));
    }

    let workspace = match workspace {
        Some(ws) => {
            if let WorkspaceSpec::Local { path, .. } = &ws
                && path.as_os_str().is_empty()
            {
                return Err(OpsError::Validation(
                    "workspace.path must not be empty".to_string(),
                ));
            }
            if let WorkspaceSpec::Remote { cluster, .. } = &ws
                && let Some(assignee) = original.assignee.as_deref()
            {
                let org = store.org_list()?;
                crate::matching::assignee_has_cluster_tool(&org, assignee, cluster)
                    .map_err(OpsError::Validation)?;
            }
            ws
        }
        None => original.workspace.clone(),
    };

    // ADR-0069: やり直しは元のタスクの routing の出自を引き継ぐ。ただし gate の判定
    // （`routing.execution`）は引き継がない（上の doc comment。複製先で判定し直す）。
    let routing = original.routing.clone().map(|mut r| {
        r.execution = None;
        r
    });
    let new_task = Task {
        expected_write_paths: original.expected_write_paths.clone(),
        tree: None,
        paused_at: None,
        routing,
        // ADR-0043 D2: やり直しは元のタスクと同じリポジトリで作業する。
        repos: original.repos.clone(),
        id: TaskId::new(),
        parent_id: original.parent_id,
        kind: original.kind,
        title: original.title.clone(),
        objective: original.objective.clone(),
        acceptance: original.acceptance.clone(),
        inputs: original.inputs.clone(),
        depends_on: original.depends_on.clone(),
        status: if accept { Status::Ready } else { Status::Draft },
        priority: original.priority,
        worker_hint: original.worker_hint.clone(),
        workspace,
        budget: original.budget,
        attempts: 0,
        lease: None,
        created_at: now,
        updated_at: now,
        role: original.role.clone(),
        genre: original.genre.clone(),
        aggregate: original.aggregate,
        project_id: original.project_id,
        milestone_id: original.milestone_id,
        assignee: original.assignee.clone(),
        conversation: None,
        // ADR-0044 D3（Phase 53）: やり直したタスクは元のラベル・種類を引き継ぐ（人が付けた分類なので）。
        labels: original.labels.clone(),
        category: original.category,
        // ADR-0046 D2 / D4（Phase 59）: やり直しは元の能力タグと進め方をそのまま引き継ぐ。
        skills: original.skills.clone(),
        mode: original.mode,
    };
    let new_id = new_task.id;
    let rewired = store.retry_task(id, &new_task)?;
    if let Some(mode) = execution {
        crate::regate::set_execution_mode(store, new_id, mode, source, None, now)?;
    }
    Ok(RetryResult {
        task_id: new_id,
        rewired,
    })
}

#[cfg(test)]
mod tests;
