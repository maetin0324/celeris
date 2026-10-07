use super::*;
use task_core::{
    ArtifactRef, Check, Criterion, OrgKind, OrgNode, Profile, SqliteStore, TaskKind, WorkerHint,
    WorkspaceSpec,
};

fn task_with(status: Status) -> Task {
    let now = OffsetDateTime::now_utc();
    Task {
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "t".into(),
        objective: "o".into(),
        acceptance: vec![Criterion {
            text: "c".into(),
            check: Check::Human,
        }],
        inputs: Vec::<ArtifactRef>::new(),
        depends_on: vec![],
        status,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::local("/tmp/ws"),
        repos: Vec::new(),
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
        project_id: None,
        milestone_id: None,
        assignee: None,
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    }
}

/// ADR-0044 D1: 書いた項目だけが変わり、`Event::Edited{fields}` が残る。列（検索用の写し）も揃う。
#[test]
fn an_edit_changes_only_the_written_fields_and_records_an_edited_event() {
    let store = SqliteStore::open_in_memory().expect("store");
    let task = task_with(Status::Ready);
    let id = task.id;
    store.insert(&task).expect("insert");

    let edit = TaskEdit {
        title: Some("新しい題名".into()),
        priority: Some(PriorityInput::Label(crate::add::PriorityLabel::P0)),
        labels: Some(vec!["infra".into(), "infra".into(), "urgent".into()]),
        category: Some(TaskCategory::Bug),
        tier: Some(Tier::Frontier),
        max_turns: Some(30),
        ..TaskEdit::default()
    };
    let result = edit_task(&store, id, edit, &[], OffsetDateTime::now_utc()).expect("edit");
    assert_eq!(
        result.fields,
        vec!["title", "priority", "labels", "category", "tier", "budget"]
    );
    let after = store.get(id).expect("get").expect("task");
    assert_eq!(after.title, "新しい題名");
    assert_eq!(after.priority, 30);
    assert_eq!(
        after.labels,
        vec!["infra".to_string(), "urgent".to_string()],
        "重複は畳む"
    );
    assert_eq!(after.category, TaskCategory::Bug);
    assert_eq!(after.worker_hint.tier, Tier::Frontier);
    assert_eq!(after.budget.max_turns, 30);
    assert_eq!(after.objective, "o", "書かなかった項目は変わらない");
    assert_eq!(after.status, Status::Ready, "状態機械は通らない");

    let events = store.events_for(id).expect("events");
    let edited = events
        .iter()
        .find_map(|(_, e)| match e {
            Event::Edited { fields, by } => Some((fields.clone(), by.clone())),
            _ => None,
        })
        .expect("Edited event");
    assert_eq!(edited.1, "human");
    assert_eq!(edited.0, result.fields);
}

/// ADR-0044 D1: 終端のタスクは 409（`InvalidState`）。`running` / `reviewing` は受け付ける。
#[test]
fn terminal_tasks_refuse_edits_but_running_and_reviewing_accept_them() {
    for status in [Status::Done, Status::Failed, Status::Cancelled] {
        let store = SqliteStore::open_in_memory().expect("store");
        let task = task_with(status);
        let id = task.id;
        store.insert(&task).expect("insert");
        let err = edit_task(
            &store,
            id,
            TaskEdit {
                title: Some("x".into()),
                ..TaskEdit::default()
            },
            &[],
            OffsetDateTime::now_utc(),
        )
        .unwrap_err();
        assert!(
            matches!(err, OpsError::InvalidState { .. }),
            "{status:?}: {err}"
        );
    }
    for status in [Status::Running, Status::Reviewing] {
        let store = SqliteStore::open_in_memory().expect("store");
        let task = task_with(status);
        let id = task.id;
        store.insert(&task).expect("insert");
        let result = edit_task(
            &store,
            id,
            TaskEdit {
                title: Some("x".into()),
                ..TaskEdit::default()
            },
            &[],
            OffsetDateTime::now_utc(),
        )
        .unwrap_or_else(|e| panic!("{status:?}: {e}"));
        assert_eq!(result.fields, vec!["title"]);
        assert_eq!(
            store.get(id).expect("get").expect("task").status,
            status,
            "走っている run は止めない（次の run から効く）"
        );
    }
}

/// 検証: 空の題名・壊れたラベル・自分への依存・知らない担当は拒否し、何も書かない。
#[test]
fn invalid_edits_are_rejected_without_writing() {
    let store = SqliteStore::open_in_memory().expect("store");
    let task = task_with(Status::Ready);
    let id = task.id;
    store.insert(&task).expect("insert");

    for edit in [
        TaskEdit {
            title: Some("  ".into()),
            ..TaskEdit::default()
        },
        TaskEdit {
            labels: Some(vec!["Bad Label".into()]),
            ..TaskEdit::default()
        },
        TaskEdit {
            labels: Some((0..9).map(|i| format!("l{i}")).collect()),
            ..TaskEdit::default()
        },
        TaskEdit {
            depends_on: Some(vec![id]),
            ..TaskEdit::default()
        },
        TaskEdit {
            // 存在しない先行は拒否（循環の検査より前に落ちる）。
            depends_on: Some(vec![TaskId::new()]),
            ..TaskEdit::default()
        },
        TaskEdit {
            assignee: Some(Some("nobody".into())),
            ..TaskEdit::default()
        },
        TaskEdit {
            acceptance: Some(vec![]),
            ..TaskEdit::default()
        },
    ] {
        let err = edit_task(&store, id, edit, &[], OffsetDateTime::now_utc()).unwrap_err();
        assert!(matches!(err, OpsError::Validation(_)), "{err}");
    }
    let after = store.get(id).expect("get").expect("task");
    assert_eq!(after.title, "t");
    assert!(
        store.events_for(id).expect("events").is_empty(),
        "何も積まない"
    );
}

/// Phase 53 の監査: `PATCH depends_on` で**循環**は作れない（作成時は構造上できなかった）。
#[test]
fn depends_on_cannot_create_a_cycle() {
    let store = SqliteStore::open_in_memory().expect("store");
    let a = task_with(Status::Ready);
    let mut b = task_with(Status::Ready);
    b.depends_on = vec![a.id];
    store.insert(&a).expect("insert a");
    store.insert(&b).expect("insert b");

    // a が b に依存すると a → b → a の循環になる。
    let err = edit_task(
        &store,
        a.id,
        TaskEdit {
            depends_on: Some(vec![b.id]),
            ..TaskEdit::default()
        },
        &[],
        OffsetDateTime::now_utc(),
    )
    .unwrap_err();
    assert!(
        matches!(err, OpsError::Validation(ref m) if m.contains("cycle")),
        "{err}"
    );
    assert!(
        store
            .get(a.id)
            .expect("get")
            .expect("task")
            .depends_on
            .is_empty()
    );

    // 循環にならない張り替えは通る。
    let c = task_with(Status::Ready);
    store.insert(&c).expect("insert c");
    let result = edit_task(
        &store,
        a.id,
        TaskEdit {
            depends_on: Some(vec![c.id]),
            ..TaskEdit::default()
        },
        &[],
        OffsetDateTime::now_utc(),
    )
    .expect("edit");
    assert_eq!(result.fields, vec!["depends_on"]);
}

/// Phase 53 の監査: `genre` と食い違う `role` は作成時と同じく拒否する。
#[test]
fn a_role_outside_the_tasks_genre_is_rejected() {
    let store = SqliteStore::open_in_memory().expect("store");
    let mut task = task_with(Status::Ready);
    task.genre = Some("coding".into());
    let id = task.id;
    store.insert(&task).expect("insert");
    let genres = vec![task_core::GenreSpec {
        id: "coding".into(),
        description: "write code".into(),
        roles: vec!["implementer".into()],
        ..task_core::GenreSpec::default()
    }];

    let err = edit_task(
        &store,
        id,
        TaskEdit {
            role: Some(Some("literature-scout".into())),
            ..TaskEdit::default()
        },
        &genres,
        OffsetDateTime::now_utc(),
    )
    .unwrap_err();
    assert!(
        matches!(err, OpsError::Validation(ref m) if m.contains("is not one of genre")),
        "{err}"
    );

    let result = edit_task(
        &store,
        id,
        TaskEdit {
            role: Some(Some("implementer".into())),
            ..TaskEdit::default()
        },
        &genres,
        OffsetDateTime::now_utc(),
    )
    .expect("edit");
    assert_eq!(result.fields, vec!["role"]);
}

/// Phase 53 の監査: 編集の最中にディスパッチャがリースを取っても、`status` / `attempts` /
/// `lease` は**ストアがトランザクションの中で読んだ値**が残る（編集で run を殺さない）。
#[test]
fn an_edit_never_overwrites_the_status_attempts_or_lease() {
    let store = SqliteStore::open_in_memory().expect("store");
    let task = task_with(Status::Ready);
    let id = task.id;
    store.insert(&task).expect("insert");

    // 人が「ready のタスク」を読んで編集フォームを開く（この時点の写し）。
    let stale = store.get(id).expect("get").expect("task");
    assert_eq!(stale.status, Status::Ready);

    // その間にディスパッチャが dispatch してリースを取る。
    store
        .acquire_lease(id, "run-1", std::time::Duration::from_secs(60))
        .expect("lease");
    assert_eq!(
        store.get(id).expect("get").expect("task").status,
        Status::Running
    );

    // 古い写しを持ったまま編集しても、状態機械の 3 つは巻き戻らない。
    let result = edit_task(
        &store,
        id,
        TaskEdit {
            title: Some("編集した".into()),
            ..TaskEdit::default()
        },
        &[],
        OffsetDateTime::now_utc(),
    )
    .expect("edit");
    assert_eq!(result.task.status, Status::Running);
    assert!(result.task.lease.is_some());
    let after = store.get(id).expect("get").expect("task");
    assert_eq!(after.status, Status::Running, "json も running のまま");
    assert_eq!(after.title, "編集した");
    assert_eq!(
        after.lease.as_ref().map(|l| l.worker_run_id.as_str()),
        Some("run-1"),
        "リースを消さない"
    );
    // `ready_tasks` が見る列も running のまま（列と json が食い違わない）。
    assert!(store.ready_tasks(10).expect("ready").is_empty());
}

/// ADR-0074 D2.1（Phase F3 途中確認、区切り 1 (a)）: `PATCH /tasks/{id}` の `pause_after` は
/// `Task.routing.pause_after` に書かれ、出自は常に `PauseSource::Human`（PATCH は管理系 = 人だけ）。
/// `routing` が無い（Phase 114 より前の）タスクでも新しく作られる。
#[test]
fn edit_writes_pause_after_with_human_source_even_without_prior_routing() {
    let store = SqliteStore::open_in_memory().expect("store");
    let task = task_with(Status::Ready);
    assert!(
        task.routing.is_none(),
        "この fixture は routing 無しから始める"
    );
    let id = task.id;
    store.insert(&task).expect("insert");

    let result = edit_task(
        &store,
        id,
        TaskEdit {
            pause_after: Some(task_core::PausePolicy::EachPhase),
            ..TaskEdit::default()
        },
        &[],
        OffsetDateTime::now_utc(),
    )
    .expect("edit");
    assert_eq!(result.fields, vec!["pause_after".to_string()]);
    let routing = result.task.routing.expect("routing created");
    assert_eq!(routing.pause_after, task_core::PausePolicy::EachPhase);
    assert_eq!(routing.pause_after_source, task_core::PauseSource::Human);

    let after = store.get(id).expect("get").expect("task");
    let after_routing = after.routing.expect("routing persisted");
    assert_eq!(after_routing.pause_after, task_core::PausePolicy::EachPhase);

    // 同じ値をもう一度 PATCH しても、何も変わらないので `fields` は空。
    let noop = edit_task(
        &store,
        id,
        TaskEdit {
            pause_after: Some(task_core::PausePolicy::EachPhase),
            ..TaskEdit::default()
        },
        &[],
        OffsetDateTime::now_utc(),
    )
    .expect("edit");
    assert!(noop.fields.is_empty(), "{:?}", noop.fields);
}

/// `expected_status` が現在と違えば 409。何も変えない編集はイベントを積まない。
#[test]
fn expected_status_conflicts_and_a_no_op_edit_records_nothing() {
    let store = SqliteStore::open_in_memory().expect("store");
    let task = task_with(Status::Ready);
    let id = task.id;
    store.insert(&task).expect("insert");
    let err = edit_task(
        &store,
        id,
        TaskEdit {
            title: Some("x".into()),
            expected_status: Some(Status::Running),
            ..TaskEdit::default()
        },
        &[],
        OffsetDateTime::now_utc(),
    )
    .unwrap_err();
    assert!(matches!(err, OpsError::Conflict { .. }), "{err}");

    let result = edit_task(
        &store,
        id,
        TaskEdit {
            title: Some("t".into()),
            ..TaskEdit::default()
        },
        &[],
        OffsetDateTime::now_utc(),
    )
    .expect("no-op edit");
    assert!(result.fields.is_empty());
    assert!(store.events_for(id).expect("events").is_empty());
}

/// ADR-0043 D2 + ADR-0044 D1（Phase 52 + 53 のマージ）: `PATCH` の `repos` は
/// **そのタスクの案件の中**から名前で引く（`POST /tasks` と同じ規則）。知らない名前は 422、
/// 空配列は「リポジトリを使わない」、案件に属さないタスクの `repos` も 422。
#[test]
fn repos_are_resolved_by_name_within_the_tasks_project() {
    use task_core::{Project, ProjectId, ProjectRepo, ProjectStatus, RepoId, RepoKind, RepoRun};

    let store = SqliteStore::open_in_memory().expect("store");
    let now = OffsetDateTime::now_utc();
    let project = Project {
        auto_advance: false,
        slug: None,
        archived_at: None,
        paused_from: None,
        id: ProjectId::new(),
        title: "benchfs".into(),
        request: "複数リポジトリの案件".into(),
        status: ProjectStatus::Active,
        secretary_summary: None,
        workspace: None,
        created_at: now,
        updated_at: now,
    };
    store.project_create(&project).expect("project");
    let code = ProjectRepo {
        id: RepoId::new(),
        project_id: project.id,
        name: "benchfs".into(),
        kind: RepoKind::Git,
        location: WorkspaceSpec::local("/srv/benchfs"),
        default_branch: None,
        sync: None,
        run: RepoRun::Auto,
        is_primary: false,
        created_at: now,
    };
    store.repo_create(&code).expect("code repo");
    let paper = ProjectRepo {
        id: RepoId::new(),
        name: "benchfs-paper".into(),
        location: WorkspaceSpec::local("/srv/benchfs-paper"),
        ..code.clone()
    };
    store.repo_create(&paper).expect("paper repo");

    let mut task = task_with(Status::Ready);
    task.project_id = Some(project.id);
    let id = task.id;
    store.insert(&task).expect("insert");

    // 名前で差し替え → `fields` に `repos`、`Task.repos` が解決済みの参照になる。
    let result = edit_task(
        &store,
        id,
        TaskEdit {
            repos: Some(vec!["benchfs-paper".into(), "benchfs".into()]),
            ..TaskEdit::default()
        },
        &[],
        OffsetDateTime::now_utc(),
    )
    .expect("edit repos");
    assert_eq!(result.fields, vec!["repos".to_string()]);
    assert_eq!(
        result
            .task
            .repos
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>(),
        vec!["benchfs-paper", "benchfs"],
        "書いた順がそのまま（先頭が cwd）"
    );
    assert_eq!(result.task.repos[0].repo_id, paper.id);
    // 列と json の両方に残る。
    assert_eq!(store.get(id).expect("get").expect("task").repos.len(), 2);

    // 案件に無い名前は 422。
    let err = edit_task(
        &store,
        id,
        TaskEdit {
            repos: Some(vec!["unknown".into()]),
            ..TaskEdit::default()
        },
        &[],
        OffsetDateTime::now_utc(),
    )
    .unwrap_err();
    assert!(matches!(err, OpsError::Validation(_)), "{err}");

    // 空配列は「リポジトリを使わない」（継承しない）。
    let cleared = edit_task(
        &store,
        id,
        TaskEdit {
            repos: Some(Vec::new()),
            ..TaskEdit::default()
        },
        &[],
        OffsetDateTime::now_utc(),
    )
    .expect("clear repos");
    assert_eq!(cleared.fields, vec!["repos".to_string()]);
    assert!(cleared.task.repos.is_empty());

    // 案件に属さないタスクの `repos` は 422。
    let orphan = task_with(Status::Ready);
    let orphan_id = orphan.id;
    store.insert(&orphan).expect("insert orphan");
    let err = edit_task(
        &store,
        orphan_id,
        TaskEdit {
            repos: Some(vec!["benchfs".into()]),
            ..TaskEdit::default()
        },
        &[],
        OffsetDateTime::now_utc(),
    )
    .unwrap_err();
    assert!(matches!(err, OpsError::Validation(_)), "{err}");
}

// ---- ADR-0062 Phase 108: `PATCH /tasks/{id}` の `workspace` ----

/// `cos` の下に `web-research`（道具なし）と `cluster-hpc`（`cluster:sirius` を持つ）。
fn cluster_org() -> Vec<OrgNode> {
    let now = OffsetDateTime::now_utc();
    let dept = |id: &str, tools: &[&str]| OrgNode {
        profile: Profile {
            tools: tools.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        },
        id: id.to_string(),
        parent_id: Some("cos".to_string()),
        name: id.to_string(),
        kind: OrgKind::Department,
        genre: None,
        brief: String::new(),
        position: 0,
        created_at: now,
        updated_at: now,
    };
    vec![
        OrgNode {
            profile: Default::default(),
            id: "cos".into(),
            parent_id: None,
            name: "cos".into(),
            kind: OrgKind::Secretary,
            genre: None,
            brief: String::new(),
            position: 0,
            created_at: now,
            updated_at: now,
        },
        dept("web-research", &[]),
        dept("cluster-hpc", &["cluster:sirius"]),
    ]
}

/// (a) `ready` のタスクを `Local` に PATCH → 200、`workspace` が変わる。
#[test]
fn workspace_can_be_switched_to_local_on_a_ready_task() {
    let store = SqliteStore::open_in_memory().expect("store");
    let task = task_with(Status::Ready);
    let id = task.id;
    store.insert(&task).expect("insert");

    let result = edit_task(
        &store,
        id,
        TaskEdit {
            workspace: Some(WorkspaceSpec::local("/tmp/ws2")),
            ..TaskEdit::default()
        },
        &[],
        OffsetDateTime::now_utc(),
    )
    .expect("edit");
    assert_eq!(result.fields, vec!["workspace".to_string()]);
    assert_eq!(result.task.workspace, WorkspaceSpec::local("/tmp/ws2"));
    assert_eq!(
        store.get(id).expect("get").expect("task").workspace,
        WorkspaceSpec::local("/tmp/ws2")
    );
}

/// (b) B1（unroutable）で `blocked` になったタスクを `Local` に PATCH すると `ready` に戻り、
/// 質問の approval が閉じる（既存の「質問に答える」経路に相乗り）。
#[test]
fn a_blocked_unroutable_task_returns_to_ready_when_the_workspace_resolves_it() {
    use task_core::approval::{Approval, ApprovalId, ApprovalStore};

    let store = SqliteStore::open_in_memory().expect("store");
    let mut task = task_with(Status::Ready);
    task.workspace = WorkspaceSpec::Remote {
        cluster: "sirius".into(),
        path: "~".into(),
        mode: None,
    };
    task.assignee = Some("web-research".into());
    let id = task.id;
    store.insert(&task).expect("insert");

    let question = "担当 `web-research` には道具 `cluster:sirius` が無いため…".to_string();
    store
        .apply_transition(
            id,
            Trigger::Unroutable,
            Some(Event::QuestionRaised {
                run_id: format!("cluster-routing-{id}"),
                text: question.clone(),
            }),
        )
        .expect("block");
    assert_eq!(
        store.get(id).expect("get").expect("task").status,
        Status::Blocked
    );
    let approval = Approval {
        id: ApprovalId::new(),
        project_id: None,
        node_id: "web-research".into(),
        task_id: Some(id),
        question: question.clone(),
        decision: None,
        answer: None,
        created_at: OffsetDateTime::now_utc(),
        decided_at: None,
    };
    store.approval_append(&approval).expect("append approval");

    let result = edit_task(
        &store,
        id,
        TaskEdit {
            workspace: Some(WorkspaceSpec::local("/tmp/ws-local")),
            ..TaskEdit::default()
        },
        &[],
        OffsetDateTime::now_utc(),
    )
    .expect("edit");
    assert_eq!(
        result.task.status,
        Status::Ready,
        "経路が通ったので ready に戻る"
    );
    assert_eq!(
        store.get(id).expect("get").expect("task").status,
        Status::Ready
    );
    let decided = store.approval_get(approval.id).expect("get").expect("some");
    assert!(!decided.is_pending(), "B1 の質問は解決済みとして閉じる");

    let events = store.events_for(id).expect("events");
    assert!(events.iter().any(|(_, e)| matches!(
        e,
        Event::Answered { answer, .. } if answer.contains("解決済み")
    )));
}

/// (c) 明示の `Remote` で担当（`web-research`）が `cluster:sirius` を持たなければ 422。
#[test]
fn an_explicit_remote_workspace_is_rejected_when_the_assignee_lacks_the_cluster_tool() {
    let store = SqliteStore::open_in_memory().expect("store");
    for node in cluster_org() {
        store.org_upsert(&node).expect("seed org");
    }
    let mut task = task_with(Status::Ready);
    task.assignee = Some("web-research".into());
    let id = task.id;
    store.insert(&task).expect("insert");

    let err = edit_task(
        &store,
        id,
        TaskEdit {
            workspace: Some(WorkspaceSpec::Remote {
                cluster: "sirius".into(),
                path: "~".into(),
                mode: None,
            }),
            ..TaskEdit::default()
        },
        &[],
        OffsetDateTime::now_utc(),
    )
    .unwrap_err();
    let OpsError::Validation(msg) = err else {
        panic!("{err}");
    };
    assert!(msg.contains("cluster:sirius"), "{msg}");
    assert!(msg.contains("cluster-hpc"), "候補ノードを挙げる: {msg}");
    assert_eq!(
        store.get(id).expect("get").expect("task").workspace,
        WorkspaceSpec::local("/tmp/ws"),
        "検証に落ちたので何も書かない"
    );
}

/// (d) `running` への `workspace` 編集は 409（`draft`/`ready`/`blocked`/`failed` だけ許す）。
#[test]
fn workspace_edits_are_refused_on_running_tasks() {
    let store = SqliteStore::open_in_memory().expect("store");
    let task = task_with(Status::Running);
    let id = task.id;
    store.insert(&task).expect("insert");

    let err = edit_task(
        &store,
        id,
        TaskEdit {
            workspace: Some(WorkspaceSpec::local("/tmp/ws2")),
            ..TaskEdit::default()
        },
        &[],
        OffsetDateTime::now_utc(),
    )
    .unwrap_err();
    assert!(matches!(err, OpsError::InvalidState { .. }), "{err}");
    assert_eq!(
        store.get(id).expect("get").expect("task").workspace,
        WorkspaceSpec::local("/tmp/ws")
    );
}

/// (e) `assignee` を `cluster-hpc`（`cluster:sirius` を持つ）に変えつつ `Remote` のまま → 200。
#[test]
fn changing_the_assignee_to_a_node_with_the_cluster_tool_is_accepted() {
    let store = SqliteStore::open_in_memory().expect("store");
    for node in cluster_org() {
        store.org_upsert(&node).expect("seed org");
    }
    let mut task = task_with(Status::Ready);
    task.workspace = WorkspaceSpec::Remote {
        cluster: "sirius".into(),
        path: "~".into(),
        mode: None,
    };
    let id = task.id;
    store.insert(&task).expect("insert");

    let result = edit_task(
        &store,
        id,
        TaskEdit {
            assignee: Some(Some("cluster-hpc".into())),
            ..TaskEdit::default()
        },
        &[],
        OffsetDateTime::now_utc(),
    )
    .expect("edit");
    assert_eq!(result.fields, vec!["assignee".to_string()]);
    assert_eq!(result.task.assignee.as_deref(), Some("cluster-hpc"));
}

/// ADR-0098 D7（Phase R7-10）: 案件を持たない・まだ run していない task に案件を付けると、案件の primary の
/// リポジトリも付く。既に案件を持つ task・run したことのある task・子 task・知らない案件は拒否する。
#[test]
fn project_id_can_be_attached_once_to_a_project_less_task_that_never_ran() {
    use task_core::{Project, ProjectId, ProjectRepo, ProjectStatus, RepoId, RepoKind, RepoRun};
    let store = SqliteStore::open_in_memory().expect("store");
    let now = OffsetDateTime::now_utc();
    let mk_project = |title: &str| {
        let project = Project {
            auto_advance: false,
            slug: None,
            archived_at: None,
            paused_from: None,
            id: ProjectId::new(),
            title: title.into(),
            request: "r".into(),
            status: ProjectStatus::Active,
            secretary_summary: None,
            workspace: None,
            created_at: now,
            updated_at: now,
        };
        store.project_create(&project).expect("project");
        project.id
    };
    let project = mk_project("p");
    let other = mk_project("q");
    let primary = ProjectRepo {
        id: RepoId::new(),
        project_id: project,
        name: "agent-platform".into(),
        kind: RepoKind::Git,
        location: WorkspaceSpec::local("/srv/agent-platform"),
        default_branch: None,
        sync: None,
        run: RepoRun::Auto,
        is_primary: true,
        created_at: now,
    };
    store.repo_create(&primary).expect("repo");
    let attach = |id: TaskId, project_id: ProjectId| {
        edit_task(
            &store,
            id,
            TaskEdit {
                project_id: Some(project_id),
                ..TaskEdit::default()
            },
            &[],
            OffsetDateTime::now_utc(),
        )
    };

    // 付けられる: draft、案件無し、未実行。primary が repos に入り、Edited に両方の欄が残る。
    let task = task_with(Status::Draft);
    store.insert(&task).expect("insert");
    let result = attach(task.id, project).expect("attach");
    assert_eq!(
        result.fields,
        vec!["project_id".to_string(), "repos".to_string()]
    );
    let stored = store.get(task.id).expect("get").expect("task");
    assert_eq!(stored.project_id, Some(project));
    assert_eq!(
        stored.repos.iter().map(|r| r.repo_id).collect::<Vec<_>>(),
        vec![primary.id]
    );
    // 同じ値は何もしない。別の案件への変更は 422。
    assert!(attach(task.id, project).expect("same").fields.is_empty());
    assert!(matches!(
        attach(task.id, other),
        Err(OpsError::Validation(m)) if m.contains("cannot be changed")
    ));

    // 一度でも run した task（attempts > 0）は 409 相当。
    let mut ran = task_with(Status::Ready);
    ran.attempts = 1;
    store.insert(&ran).expect("insert");
    assert!(matches!(
        attach(ran.id, project),
        Err(OpsError::InvalidState { .. })
    ));
    // blocked（何かが起きた後）も拒否。
    let blocked = task_with(Status::Blocked);
    store.insert(&blocked).expect("insert");
    assert!(matches!(
        attach(blocked.id, project),
        Err(OpsError::InvalidState { .. })
    ));
    // 子 task は親に従う。
    let mut child = task_with(Status::Draft);
    child.parent_id = Some(task.id);
    store.insert(&child).expect("insert");
    assert!(matches!(
        attach(child.id, project),
        Err(OpsError::Validation(m)) if m.contains("parent")
    ));
    // 知らない案件。
    let fresh = task_with(Status::Ready);
    store.insert(&fresh).expect("insert");
    assert!(matches!(
        attach(fresh.id, ProjectId::new()),
        Err(OpsError::ProjectNotFound(_))
    ));
    // 同じ PATCH の repos は案件の中で解決する（primary を足さない）。
    let explicit = edit_task(
        &store,
        fresh.id,
        TaskEdit {
            project_id: Some(project),
            repos: Some(vec![]),
            ..TaskEdit::default()
        },
        &[],
        OffsetDateTime::now_utc(),
    )
    .expect("attach with explicit repos");
    assert_eq!(explicit.fields, vec!["project_id".to_string()]);
    assert!(explicit.task.repos.is_empty());
}
