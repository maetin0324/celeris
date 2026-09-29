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
    let mut next = task.clone();
    next.paused_at = paused_at;
    next.updated_at = now;
    // `status` / `attempts` / `lease` はストアがトランザクションの中で読み直した値で上書きする（編集と同じ。
    // 走っている run のリースを壊さない）。
    let updated = store.update_task(
        &next,
        task_core::Event::Edited {
            fields: vec!["paused_at".to_string()],
            by: "human".to_string(),
        },
    )?;
    let subtree = descendants(store, &updated)?
        .iter()
        .filter(|t| !t.status.is_terminal())
        .map(task_ref)
        .collect();
    Ok(TaskPauseResult {
        task: task_ref(&updated),
        paused_at: updated.paused_at.map(crate::view::to_rfc3339),
        subtree,
    })
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
mod tests {
    use super::*;
    use task_core::{
        ArtifactRef, Budget, Check, Criterion, ListFilter, ListOrder, SqliteStore, Task, TaskKind,
        Tier, WorkerHint, WorkspaceSpec,
    };

    fn store() -> SqliteStore {
        SqliteStore::open_in_memory().unwrap_or_else(|e| panic!("open: {e}"))
    }

    fn a_project(store: &dyn TaskStore, status: ProjectStatus) -> Project {
        let now = OffsetDateTime::now_utc();
        let project = Project {
            auto_advance: false,
            slug: None,
            id: ProjectId::new(),
            title: "案件".to_string(),
            request: "やって".to_string(),
            status,
            secretary_summary: None,
            workspace: None,
            archived_at: None,
            paused_from: None,
            created_at: now,
            updated_at: now,
        };
        store
            .project_create(&project)
            .unwrap_or_else(|e| panic!("create: {e}"));
        project
    }

    fn a_task(
        store: &dyn TaskStore,
        project: Option<ProjectId>,
        milestone: Option<MilestoneId>,
        status: Status,
    ) -> Task {
        let now = OffsetDateTime::now_utc();
        let task = Task {
            tree: None,
            paused_at: None,
            routing: None,
            mode: Default::default(),
            skills: Vec::new(),
            repos: Vec::new(),
            id: task_core::TaskId::new(),
            parent_id: None,
            kind: TaskKind::Execute,
            title: "仕事".to_string(),
            objective: "やる".to_string(),
            acceptance: vec![Criterion {
                text: "通る".to_string(),
                check: Check::Command {
                    cmd: "true".to_string(),
                    expect_exit: 0,
                },
            }],
            inputs: Vec::<ArtifactRef>::new(),
            depends_on: vec![],
            status,
            priority: 0,
            worker_hint: WorkerHint {
                tier: Tier::Standard,
                adapter: None,
            },
            workspace: WorkspaceSpec::Local {
                path: "/tmp/workspace".into(),
                mode: None,
            },
            budget: Budget {
                max_turns: 10,
                max_wall_secs: 600,
                max_retries: 2,
            },
            attempts: 0,
            lease: None,
            created_at: now,
            updated_at: now,
            role: None,
            genre: None,
            aggregate: false,
            project_id: project,
            milestone_id: milestone,
            assignee: None,
            conversation: None,
            labels: Vec::new(),
            category: Default::default(),
        };
        store
            .insert(&task)
            .unwrap_or_else(|e| panic!("insert: {e}"));
        task
    }

    #[test]
    fn cancelling_a_project_cancels_its_open_tasks_and_milestones_and_keeps_terminal_ones() {
        let store = store();
        let project = a_project(&store, ProjectStatus::Active);
        let running = a_task(&store, Some(project.id), None, Status::Running);
        let ready = a_task(&store, Some(project.id), None, Status::Ready);
        let done = a_task(&store, Some(project.id), None, Status::Done);
        let other = a_task(&store, None, None, Status::Ready);
        let reached = store
            .milestone_create(project.id, "済", "", MilestoneStatus::Reached)
            .unwrap_or_else(|e| panic!("{e}"));
        let open = store
            .milestone_create(project.id, "途中", "", MilestoneStatus::InProgress)
            .unwrap_or_else(|e| panic!("{e}"));

        let result = cancel_project(&store, project.id).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(result.project.status, ProjectStatus::Cancelled);
        assert_eq!(result.cancelled_tasks.len(), 2);
        assert_eq!(result.cancelled_milestones, vec![open.id]);

        let status_of = |id| {
            store
                .get(id)
                .unwrap_or_else(|e| panic!("{e}"))
                .map(|t| t.status)
        };
        assert_eq!(status_of(running.id), Some(Status::Cancelled));
        assert_eq!(status_of(ready.id), Some(Status::Cancelled));
        assert_eq!(status_of(done.id), Some(Status::Done));
        // 他の案件のタスクは触らない。
        assert_eq!(status_of(other.id), Some(Status::Ready));
        let milestones = store
            .milestone_list(project.id)
            .unwrap_or_else(|e| panic!("{e}"));
        let reached_now = milestones
            .iter()
            .find(|m| m.id == reached.id)
            .map(|m| m.status);
        assert_eq!(reached_now, Some(MilestoneStatus::Reached));
    }

    #[test]
    fn the_cascade_reason_says_the_project_cancelled_the_task() {
        let store = store();
        let project = a_project(&store, ProjectStatus::Active);
        let by_project = a_task(&store, Some(project.id), None, Status::Ready);

        cancel_project(&store, project.id).unwrap_or_else(|e| panic!("{e}"));

        let reasons = store
            .events_for(by_project.id)
            .unwrap_or_else(|e| panic!("{e}"))
            .into_iter()
            .filter_map(|(_, e)| match e {
                task_core::Event::Transitioned { reason, .. } => Some(reason),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(reasons.contains(&"project_cancelled".to_string()));
    }

    #[test]
    fn pause_remembers_the_previous_status_and_resume_restores_it() {
        let store = store();
        let project = a_project(&store, ProjectStatus::Proposed);
        let paused = pause_project(&store, project.id).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(paused.project.status, ProjectStatus::Paused);
        assert_eq!(paused.project.paused_from, Some(ProjectStatus::Proposed));

        // 二重の一時停止は 409。
        assert!(matches!(
            pause_project(&store, project.id),
            Err(OpsError::InvalidLifecycle { .. })
        ));

        let resumed = resume_project(&store, project.id).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(resumed.project.status, ProjectStatus::Proposed);
        assert_eq!(resumed.project.paused_from, None);
        assert!(matches!(
            resume_project(&store, project.id),
            Err(OpsError::InvalidLifecycle { .. })
        ));
    }

    #[test]
    fn only_a_terminal_project_can_be_archived_and_archived_tasks_are_hidden_by_default() {
        let store = store();
        let project = a_project(&store, ProjectStatus::Active);
        let task = a_task(&store, Some(project.id), None, Status::Done);
        let now = OffsetDateTime::now_utc();

        assert!(matches!(
            archive_project(&store, project.id, now),
            Err(OpsError::InvalidLifecycle { .. })
        ));

        store
            .project_set_lifecycle(project.id, ProjectStatus::Done, Some(None))
            .unwrap_or_else(|e| panic!("{e}"));
        let archived = archive_project(&store, project.id, now).unwrap_or_else(|e| panic!("{e}"));
        assert!(archived.project.archived_at.is_some());
        // 二度目は 409 にしない（そのまま返す）。
        assert!(archive_project(&store, project.id, now).is_ok());

        let hidden = ListFilter {
            hide_archived: true,
            ..ListFilter::default()
        };
        let page = store
            .list_page(&hidden, ListOrder::CreatedDesc, None, 100)
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(!page.items.iter().any(|t| t.id == task.id));

        let shown = ListFilter::default();
        let page = store
            .list_page(&shown, ListOrder::CreatedDesc, None, 100)
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(page.items.iter().any(|t| t.id == task.id));

        unarchive_project(&store, project.id).unwrap_or_else(|e| panic!("{e}"));
        let page = store
            .list_page(&hidden, ListOrder::CreatedDesc, None, 100)
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(page.items.iter().any(|t| t.id == task.id));
    }

    #[test]
    fn a_paused_project_stops_dispatch_and_resume_brings_it_back() {
        let store = store();
        let project = a_project(&store, ProjectStatus::Active);
        let task = a_task(&store, Some(project.id), None, Status::Ready);

        let ready = |s: &SqliteStore| {
            s.ready_tasks(100)
                .unwrap_or_else(|e| panic!("{e}"))
                .into_iter()
                .map(|t| t.id)
                .collect::<Vec<_>>()
        };
        assert!(ready(&store).contains(&task.id));

        pause_project(&store, project.id).unwrap_or_else(|e| panic!("{e}"));
        assert!(!ready(&store).contains(&task.id));
        // 状態機械は触らない: タスクは `ready` のまま。
        assert_eq!(
            store
                .get(task.id)
                .unwrap_or_else(|e| panic!("{e}"))
                .map(|t| t.status),
            Some(Status::Ready)
        );

        resume_project(&store, project.id).unwrap_or_else(|e| panic!("{e}"));
        assert!(ready(&store).contains(&task.id));
    }

    #[test]
    fn a_paused_milestone_stops_dispatch_of_its_tasks_only() {
        let store = store();
        let project = a_project(&store, ProjectStatus::Active);
        let milestone = store
            .milestone_create(project.id, "途中", "", MilestoneStatus::InProgress)
            .unwrap_or_else(|e| panic!("{e}"));
        let inside = a_task(&store, Some(project.id), Some(milestone.id), Status::Ready);
        let outside = a_task(&store, Some(project.id), None, Status::Ready);

        // ADR-0079 D13（Phase R5a）: 新しく paused にする入口は無いが、既存の paused の行の抑止は残る。
        store
            .milestone_set_lifecycle(
                milestone.id,
                MilestoneStatus::Paused,
                Some(Some(MilestoneStatus::InProgress)),
            )
            .unwrap_or_else(|e| panic!("{e}"));
        let ready: Vec<_> = store
            .ready_tasks(100)
            .unwrap_or_else(|e| panic!("{e}"))
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert!(!ready.contains(&inside.id));
        assert!(ready.contains(&outside.id));
    }

    #[test]
    fn unknown_ids_are_not_found() {
        let store = store();
        assert!(matches!(
            cancel_project(&store, ProjectId::new()),
            Err(OpsError::ProjectNotFound(_))
        ));
        assert!(matches!(
            pause_task(&store, task_core::TaskId::new(), OffsetDateTime::now_utc()),
            Err(OpsError::NotFound(_))
        ));
    }

    fn ready_ids(s: &SqliteStore) -> Vec<task_core::TaskId> {
        s.ready_tasks(100)
            .unwrap_or_else(|e| panic!("{e}"))
            .into_iter()
            .map(|t| t.id)
            .collect()
    }

    fn rewrite(store: &dyn TaskStore, task: &Task, field: &str) -> Task {
        store
            .update_task(
                task,
                task_core::Event::Edited {
                    fields: vec![field.into()],
                    by: "test".into(),
                },
            )
            .unwrap_or_else(|e| panic!("{e}"))
    }

    fn child_of(store: &dyn TaskStore, parent: &Task, status: Status) -> Task {
        let mut child = a_task(store, parent.project_id, None, status);
        child.parent_id = Some(parent.id);
        rewrite(store, &child, "parent_id")
    }

    /// ADR-0079 D13（Phase R5a）: root の pause で子孫（子・孫・採用で `parent_id` の無い木の子）が dispatch されず、
    /// resume で戻る。状態機械は触らない（`ready` のまま、replay の差分 0）。兄弟の root は止まらない。
    #[test]
    fn subtree_pause_stops_descendants() {
        let store = store();
        let project = a_project(&store, ProjectStatus::Active);
        let root = a_task(&store, Some(project.id), None, Status::Running);
        let child = child_of(&store, &root, Status::Ready);
        let grandchild = child_of(&store, &child, Status::Ready);
        // 木の子（採用: `parent_id` は無く、`tree.parent_unit` だけが root を指す）。
        let mut adopted = a_task(&store, Some(project.id), None, Status::Ready);
        adopted.tree = Some(task_core::TreeInfo {
            root_id: root.id,
            depth: 2,
            parent_unit: Some(task_core::ParentUnit {
                task_id: root.id,
                plan_id: "p".into(),
                unit_key: "u".into(),
                stage: "s".into(),
                attempt: 1,
            }),
            base_commit: None,
        });
        let adopted = rewrite(&store, &adopted, "tree");
        let sibling = a_task(&store, Some(project.id), None, Status::Ready);
        let before = ready_ids(&store);
        for t in [&child, &grandchild, &adopted, &sibling] {
            assert!(before.contains(&t.id));
        }

        let now = OffsetDateTime::now_utc();
        let paused = pause_task(&store, root.id, now).unwrap_or_else(|e| panic!("{e}"));
        assert!(paused.paused_at.is_some());
        let subtree: Vec<_> = paused.subtree.iter().map(|t| t.id).collect();
        assert!(subtree.contains(&child.id), "{subtree:?}");
        assert!(subtree.contains(&grandchild.id), "{subtree:?}");
        let ready = ready_ids(&store);
        assert!(!ready.contains(&child.id), "child must not dispatch");
        assert!(
            !ready.contains(&grandchild.id),
            "grandchild must not dispatch"
        );
        assert!(!ready.contains(&adopted.id), "tree child via parent_unit");
        assert!(
            ready.contains(&sibling.id),
            "an independent root keeps running"
        );
        // 走っている root は running のまま（run は終わるまで走る）。並列 WU の 2 本目以降の判定は止まっている。
        let root_now = store
            .get(root.id)
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|| panic!("root"));
        assert_eq!(root_now.status, Status::Running);
        assert!(
            store
                .halted_by_pause(&root_now)
                .unwrap_or_else(|e| panic!("{e}"))
        );
        assert!(
            !store
                .halted_by_pause(&sibling)
                .unwrap_or_else(|e| panic!("{e}"))
        );

        // 二重の一時停止は 409。子は自分では一時停止していないので resume は 409（祖先が止めている）。
        assert!(matches!(
            pause_task(&store, root.id, now),
            Err(OpsError::InvalidLifecycle { .. })
        ));
        assert!(matches!(
            resume_task(&store, child.id, now),
            Err(OpsError::InvalidLifecycle { .. })
        ));

        let resumed = resume_task(&store, root.id, now).unwrap_or_else(|e| panic!("{e}"));
        assert!(resumed.paused_at.is_none());
        let ready = ready_ids(&store);
        for t in [&child, &grandchild, &adopted, &sibling] {
            assert!(ready.contains(&t.id));
        }
        assert!(matches!(
            resume_task(&store, root.id, now),
            Err(OpsError::InvalidLifecycle { .. })
        ));

        // events が正本: pause / resume は状態・attempts を変えない（replay の差分は 0。`a_task` は `insert` で
        // 作るので `Created` の無い行の差分は試験の組み立ての都合として除く）。
        let report = crate::replay::replay(&store).unwrap_or_else(|e| panic!("{e}"));
        let real: Vec<_> = report
            .mismatches
            .iter()
            .filter(|m| m.replayed != "<no Created event>")
            .collect();
        assert!(real.is_empty(), "{real:?}");
        let edited = store
            .events_for(root.id)
            .unwrap_or_else(|e| panic!("{e}"))
            .into_iter()
            .filter(|(_, e)| {
                matches!(e, task_core::Event::Edited { fields, by } if fields == &["paused_at".to_string()] && by == "human")
            })
            .count();
        assert_eq!(
            edited, 2,
            "pause と resume が Edited{{paused_at}} を 1 件ずつ残す"
        );
    }

    /// 案件の pause は root task の子孫にも効く（子が `project_id` を持たなくても祖先の案件で止まる）。
    #[test]
    fn project_pause_applies_to_root_task_subtrees() {
        let store = store();
        let project = a_project(&store, ProjectStatus::Active);
        let root = a_task(&store, Some(project.id), None, Status::Running);
        let mut orphan_child = a_task(&store, None, None, Status::Ready);
        orphan_child.parent_id = Some(root.id);
        let orphan_child = rewrite(&store, &orphan_child, "parent_id");
        assert!(ready_ids(&store).contains(&orphan_child.id));
        pause_project(&store, project.id).unwrap_or_else(|e| panic!("{e}"));
        assert!(!ready_ids(&store).contains(&orphan_child.id));
        resume_project(&store, project.id).unwrap_or_else(|e| panic!("{e}"));
        assert!(ready_ids(&store).contains(&orphan_child.id));
    }

    /// 終端・対話の task は一時停止できない（409）。
    #[test]
    fn terminal_and_conversation_tasks_cannot_be_paused() {
        let store = store();
        let now = OffsetDateTime::now_utc();
        let done = a_task(&store, None, None, Status::Done);
        assert!(matches!(
            pause_task(&store, done.id, now),
            Err(OpsError::InvalidLifecycle { .. })
        ));
        let mut conv = a_task(&store, None, None, Status::Ready);
        conv.conversation = Some(task_core::MessageId::new());
        let conv = rewrite(&store, &conv, "conversation");
        assert!(matches!(
            pause_task(&store, conv.id, now),
            Err(OpsError::InvalidLifecycle { .. })
        ));
    }
}
