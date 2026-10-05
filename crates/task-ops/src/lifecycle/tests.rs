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
        requirements: Default::default(),
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
