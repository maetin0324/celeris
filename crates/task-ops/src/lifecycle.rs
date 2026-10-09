//! ADR-0044 D6（Phase 55）: 案件の **中止・一時停止・アーカイブ**と、ADR-0079 D13（Phase R5a）の task の
//! **subtree の一時停止**（`pause_task` / `resume_task`）。
//!
//! ADR-0079 D13: 階層は task の subtree と案件の 2 つ（途中目標の中止・一時停止〈`POST /milestones/{id}/…`〉は
//! 410 にして外した。既存の paused / cancelled の途中目標の行による dispatch の抑止だけは `ready_tasks` に残る）。
//! タスクの中止は従来どおり `crate::gate::cancel`（木の子へは `parent_cancelled` で連鎖）。連鎖は**決定的・同期**:
//!
//! | 操作 | 何が起きる |
//! |---|---|
//! | `cancel` | 属する**非終端タスクを全部** `cancelled` にする（`Trigger::ProjectCancelled` / `MilestoneCancelled`。理由が `Event::Transitioned.reason` に残る）。案件の中止は非終端の途中目標も `cancelled` にする。最後に自分が `cancelled` |
//! | `pause` | 自分が `paused`。元の状態は `paused_from` に残す。**属するタスクは dispatch されない**（`ready` のまま。`TaskStore::ready_tasks` が見る）。走っている run は終わるまで走る |
//! | `resume` | `paused_from` へ戻す（無ければ案件は `active`、途中目標は `in_progress`）。`paused_from` は消す |
//! | `archive` | **終端（`done` / `cancelled`）の案件だけ**。`archived_at` を入れる。一覧から既定で隠れる |
//! | `unarchive` | `archived_at` を消す |
//!
//! 走っている run は「タスクが `running` でなくなった」ことでディスパッチャが気付き、
//! ADR-0044 §5 Phase 53 追記の統一された止め方（プロセスグループへ SIGTERM →
//! `kill_grace_secs` → SIGKILL）で止まる。worktree とブランチはその後の掃除
//! （ADR-0043 D2 / Phase 52 の `cleanup_cancelled_worktrees`）が消す。
//!
//! 通知は作らない（ADR-0044 D8）。案件・途中目標の変更そのものは `updated_at` にだけ残し、
//! **タスク側にはイベントが残る**（`Event::Transitioned{reason: "project_cancelled" | "milestone_cancelled"}`）。
//! 案件用のイベント表は作らない（D8 の「タイムラインに残るだけ」に対して、案件のタイムラインは
//! 既存の `messages` と報告で足りる）。
//!
//! LLM も I/O も使わない（DESIGN 原則 1）。

use task_core::{
    MilestoneId, MilestoneStatus, Project, ProjectId, ProjectStatus, Status, TaskStore, Trigger,
};
use time::OffsetDateTime;

use crate::error::OpsError;
use crate::view::{TaskRef, task_ref};

/// 案件・途中目標の状態を変えたときの結果（API がそのまま返す）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, schemars::JsonSchema)]
pub struct ProjectLifecycle {
    pub project: Project,
    /// この操作の連鎖で `cancelled` になったタスク（`cancel` 以外では空）。
    #[serde(default)]
    pub cancelled_tasks: Vec<TaskRef>,
    /// この操作の連鎖で `cancelled` になった途中目標の id（`cancel` 以外では空）。
    #[serde(default)]
    pub cancelled_milestones: Vec<MilestoneId>,
}

/// 中止の連鎖で 1 回に見るタスクの上限。案件のタスクは実機で数十〜数百件なので、
/// `GET /projects/{id}` の `PROJECT_TASKS_LIMIT`（500）より広く取っておく。
const CASCADE_TASK_LIMIT: usize = 5_000;

/// `filter` に合うタスクを（アーカイブの有無に関わらず）集める。索引の効く列で絞るので
/// 全件を読まない。
fn tasks_matching(
    store: &dyn TaskStore,
    filter: task_core::ListFilter,
) -> Result<Vec<task_core::Task>, OpsError> {
    Ok(store
        .list_page(
            &filter,
            task_core::ListOrder::CreatedDesc,
            None,
            CASCADE_TASK_LIMIT,
        )?
        .items)
}

/// その案件のタスク（`tasks.project_id` の索引で引く）。
fn project_tasks(store: &dyn TaskStore, id: ProjectId) -> Result<Vec<task_core::Task>, OpsError> {
    tasks_matching(
        store,
        task_core::ListFilter {
            project_id: Some(id),
            ..task_core::ListFilter::default()
        },
    )
}

/// 非終端のタスクに `trigger` を当てて `cancelled` にする。既に終端のものは触らない。
/// 1 件ごとに `apply_transition`（＝ストアの同一トランザクション）で、子・後続への伝播も
/// 従来の `cancel` と同じように起きる。伝播で先に `cancelled` になったタスクは、この後の
/// 反復で「終端なので触らない」になるだけで二重には数えない。
fn cancel_tasks(
    store: &dyn TaskStore,
    tasks: Vec<task_core::Task>,
    trigger: Trigger,
) -> Result<Vec<TaskRef>, OpsError> {
    let mut out = Vec::new();
    for task in tasks {
        // 直前の伝播で終端になっているかもしれないので、その都度いまの姿を見る。
        let Some(current) = store.get(task.id)? else {
            continue;
        };
        if current.status.is_terminal() {
            continue;
        }
        store.apply_transition(current.id, trigger.clone(), None)?;
        if let Some(after) = store.get(current.id)?
            && after.status == Status::Cancelled
        {
            out.push(task_ref(&after));
        }
    }
    Ok(out)
}

fn project_or_404(store: &dyn TaskStore, id: ProjectId) -> Result<Project, OpsError> {
    store.project_get(id)?.ok_or(OpsError::ProjectNotFound(id))
}

fn project_conflict(project: &Project, action: &str) -> OpsError {
    OpsError::InvalidLifecycle {
        subject: format!("project {}", project.id),
        context: format!("status={}", project.status.as_str()),
        action: action.to_string(),
    }
}

/// 中止できる途中目標か（＝終端でない。達成済み・再設計済み・中止済みは触らない）。
/// 案件ごとの連鎖にも、`POST /milestones/{id}/cancel` にも同じ判定を使う。
fn milestone_is_cancellable(status: MilestoneStatus) -> bool {
    !status.is_terminal()
}

// ---- 案件 ----

/// `POST /projects/{id}/cancel`。既に `cancelled` なら 409。
pub fn cancel_project(store: &dyn TaskStore, id: ProjectId) -> Result<ProjectLifecycle, OpsError> {
    let project = project_or_404(store, id)?;
    if project.status == ProjectStatus::Cancelled {
        return Err(project_conflict(&project, "cancelled"));
    }
    let cancelled_tasks =
        cancel_tasks(store, project_tasks(store, id)?, Trigger::ProjectCancelled)?;
    let mut cancelled_milestones = Vec::new();
    for milestone in store.milestone_list(id)? {
        if milestone_is_cancellable(milestone.status) {
            store.milestone_set_lifecycle(milestone.id, MilestoneStatus::Cancelled, Some(None))?;
            cancelled_milestones.push(milestone.id);
        }
    }
    store.project_set_lifecycle(id, ProjectStatus::Cancelled, Some(None))?;
    Ok(ProjectLifecycle {
        project: project_or_404(store, id)?,
        cancelled_tasks,
        cancelled_milestones,
    })
}

/// `POST /projects/{id}/pause`。終端（`done` / `cancelled`）と既に `paused` は 409。
pub fn pause_project(store: &dyn TaskStore, id: ProjectId) -> Result<ProjectLifecycle, OpsError> {
    let project = project_or_404(store, id)?;
    if project.status == ProjectStatus::Paused || project.status.is_terminal() {
        return Err(project_conflict(&project, "paused"));
    }
    store.project_set_lifecycle(id, ProjectStatus::Paused, Some(Some(project.status)))?;
    Ok(ProjectLifecycle {
        project: project_or_404(store, id)?,
        cancelled_tasks: Vec::new(),
        cancelled_milestones: Vec::new(),
    })
}

/// `POST /projects/{id}/resume`。`paused` でなければ 409。戻り先は `paused_from`（無ければ `active`）。
pub fn resume_project(store: &dyn TaskStore, id: ProjectId) -> Result<ProjectLifecycle, OpsError> {
    let project = project_or_404(store, id)?;
    if project.status != ProjectStatus::Paused {
        return Err(project_conflict(&project, "resumed"));
    }
    let back = project.paused_from.unwrap_or(ProjectStatus::Active);
    store.project_set_lifecycle(id, back, Some(None))?;
    Ok(ProjectLifecycle {
        project: project_or_404(store, id)?,
        cancelled_tasks: Vec::new(),
        cancelled_milestones: Vec::new(),
    })
}

/// `POST /projects/{id}/archive`。**終端（`done` / `cancelled`）の案件だけ**（他は 409）。
/// 既にアーカイブ済みなら何もせずそのまま返す（GUI の二度押しを 409 にしない）。
pub fn archive_project(
    store: &dyn TaskStore,
    id: ProjectId,
    now: OffsetDateTime,
) -> Result<ProjectLifecycle, OpsError> {
    let project = project_or_404(store, id)?;
    if !project.status.is_terminal() {
        return Err(project_conflict(&project, "archived"));
    }
    if project.archived_at.is_none() {
        store.project_set_archived_at(id, Some(now))?;
    }
    Ok(ProjectLifecycle {
        project: project_or_404(store, id)?,
        cancelled_tasks: Vec::new(),
        cancelled_milestones: Vec::new(),
    })
}

/// `POST /projects/{id}/unarchive`。アーカイブされていなければそのまま返す。
pub fn unarchive_project(
    store: &dyn TaskStore,
    id: ProjectId,
) -> Result<ProjectLifecycle, OpsError> {
    let project = project_or_404(store, id)?;
    if project.archived_at.is_some() {
        store.project_set_archived_at(id, None)?;
    }
    Ok(ProjectLifecycle {
        project: project_or_404(store, id)?,
        cancelled_tasks: Vec::new(),
        cancelled_milestones: Vec::new(),
    })
}

// ---- task の subtree（ADR-0079 D13、Phase R5a）----

/// `POST /tasks/{id}/pause|resume` の結果（API がそのまま返す）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, schemars::JsonSchema)]
pub struct TaskPauseResult {
    /// 一時停止・再開した task（更新後）。
    pub task: TaskRef,
    /// 一時停止の時刻（RFC 3339）。`resume` の後は `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused_at: Option<String>,
    /// この操作で dispatch が止まる・戻る子孫（非終端のもの。`parent_id` と木の親の鎖。自分は含まない）。
    #[serde(default)]
    pub subtree: Vec<TaskRef>,
}

/// subtree を辿る上限（壊れた行で無限に辿らない。木は `max_depth = 3`、委譲は 5 段なので十分広い）。
const SUBTREE_LIMIT: usize = 5_000;

/// `root` の子孫（`parent_id` の鎖と、採用で `parent_id` を書き換えない木の子〈`tree.parent_unit`〉）。
/// 自分は含まない。幅優先で、同じ task は 1 回だけ。
fn descendants(
    store: &dyn TaskStore,
    root: &task_core::Task,
) -> Result<Vec<task_core::Task>, OpsError> {
    let tree_members = match root.tree.as_ref() {
        Some(tree) => store.tree_tasks(tree.root_id)?,
        None => Vec::new(),
    };
    let mut seen = std::collections::HashSet::from([root.id]);
    let mut queue = std::collections::VecDeque::from([root.id]);
    let mut out = Vec::new();
    while let Some(id) = queue.pop_front() {
        let mut next = store.children(id)?;
        next.extend(
            tree_members
                .iter()
                .filter(|t| {
                    t.tree
                        .as_ref()
                        .and_then(|tree| tree.parent_unit.as_ref())
                        .is_some_and(|u| u.task_id == id)
                })
                .cloned(),
        );
        for child in next {
            if out.len() >= SUBTREE_LIMIT || !seen.insert(child.id) {
                continue;
            }
            queue.push_back(child.id);
            out.push(child);
        }
    }
    Ok(out)
}

fn task_conflict(task: &task_core::Task, action: &str) -> OpsError {
    OpsError::InvalidLifecycle {
        subject: format!("task {}", task.id),
        context: format!(
            "status={} paused={}",
            crate::view::status_key(task.status),
            task.paused_at.is_some()
        ),
        action: action.to_string(),
    }
}

fn set_paused(
    store: &dyn TaskStore,
    task: &task_core::Task,
    paused_at: Option<OffsetDateTime>,
    now: OffsetDateTime,
) -> Result<TaskPauseResult, OpsError> {
    let (next, event) = paused_edit(task, paused_at, now, "human");
    // `status` / `attempts` / `lease` はストアがトランザクションの中で読み直した値で上書きする（編集と同じ。
    // 走っている run のリースを壊さない）。
    let updated = store.update_task(&next, event)?;
    pause_result(store, &updated)
}

fn paused_edit(
    task: &task_core::Task,
    paused_at: Option<OffsetDateTime>,
    now: OffsetDateTime,
    by: &str,
) -> (task_core::Task, task_core::Event) {
    let mut next = task.clone();
    next.paused_at = paused_at;
    next.updated_at = now;
    let event = task_core::Event::Edited {
        fields: vec!["paused_at".to_string()],
        by: by.to_string(),
    };
    (next, event)
}

/// 一時停止・再開の結果（書いた後の task から組み立てる。読むだけ）。
pub fn pause_result(
    store: &dyn TaskStore,
    updated: &task_core::Task,
) -> Result<TaskPauseResult, OpsError> {
    let subtree = descendants(store, updated)?
        .iter()
        .filter(|t| !t.status.is_terminal())
        .map(task_ref)
        .collect();
    Ok(TaskPauseResult {
        task: task_ref(updated),
        paused_at: updated.paused_at.map(crate::view::to_rfc3339),
        subtree,
    })
}

/// [`pause_task`]（`paused = true`）・[`resume_task`]（`false`）の書き込み計画（読むだけ）。返った task と
/// event を `update_task`（CoS の監査経路では `SqliteStore::edit_task_tx`）で書く。`by` は `Edited.by`。
pub fn plan_set_paused(
    store: &dyn TaskStore,
    id: task_core::TaskId,
    paused: bool,
    by: &str,
    now: OffsetDateTime,
) -> Result<(task_core::Task, task_core::Event), OpsError> {
    let task = store.get(id)?.ok_or(OpsError::NotFound(id))?;
    if paused {
        if task.status.is_terminal()
            || task.paused_at.is_some()
            || task_core::is_conversation(&task)
        {
            return Err(task_conflict(&task, "paused"));
        }
        Ok(paused_edit(&task, Some(now), now, by))
    } else {
        if task.paused_at.is_none() {
            return Err(task_conflict(&task, "resumed"));
        }
        Ok(paused_edit(&task, None, now, by))
    }
}

/// `POST /tasks/{id}/pause`（ADR-0079 D13、Phase R5a）: task の **subtree を一時停止**する。
///
/// - 自分に `paused_at` を入れ、`Event::Edited{fields: ["paused_at"], by: "human"}` を残す（状態機械は触らない。
///   replay の状態・attempts は変わらない）。子孫には書かない: `TaskStore::ready_tasks` が祖先を辿って見るので、
///   一時停止の後に作られた子（計画の unit から daemon が作る子 task、委譲の子）も止まる。
/// - **走っている run は終わるまで走る**（案件の一時停止〈ADR-0044 D6〉と同じ意味）。その後 task が `ready` に戻っても
///   dispatch されず、`running` の task の並列 WU の 2 本目以降も起きない（`TaskStore::halted_by_pause`）。
///   `reviewing` の最終レビューと人の操作（回答・承認・中止）は止めない。
/// - 終端の task・既に一時停止中・対話（止めると人が秘書と話せなくなる）は 409。
pub fn pause_task(
    store: &dyn TaskStore,
    id: task_core::TaskId,
    now: OffsetDateTime,
) -> Result<TaskPauseResult, OpsError> {
    let task = store.get(id)?.ok_or(OpsError::NotFound(id))?;
    if task.status.is_terminal() || task.paused_at.is_some() || task_core::is_conversation(&task) {
        return Err(task_conflict(&task, "paused"));
    }
    set_paused(store, &task, Some(now), now)
}

/// `POST /tasks/{id}/resume`（ADR-0079 D13、Phase R5a）: `pause_task` を戻す（`paused_at` を消す）。
/// 一時停止中でなければ 409（終端でも、一時停止中なら消せる）。祖先がまだ一時停止中なら子孫は止まったまま。
pub fn resume_task(
    store: &dyn TaskStore,
    id: task_core::TaskId,
    now: OffsetDateTime,
) -> Result<TaskPauseResult, OpsError> {
    let task = store.get(id)?.ok_or(OpsError::NotFound(id))?;
    if task.paused_at.is_none() {
        return Err(task_conflict(&task, "resumed"));
    }
    set_paused(store, &task, None, now)
}

#[cfg(test)]
mod tests;
