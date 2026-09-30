use super::*;
use task_core::{Event, SqliteStore};

#[test]
fn browser_task_routes_to_opencode_and_can_switch_to_claude() {
    let store = SqliteStore::open_in_memory().unwrap();
    for (explicit, expected) in [(None, "acp"), (Some("claude-code"), "claude-code")] {
        let mut spec = base_spec();
        spec.skills = vec!["browser-enabled".into()];
        spec.adapter = explicit.map(str::to_owned);
        let task = create_task(&store, spec, now()).unwrap();
        assert_eq!(task.worker_hint.adapter.as_deref(), Some(expected));
    }
    let mut spec = base_spec();
    spec.skills = vec!["browser-enabled".into()];
    spec.adapter = Some("codex".into());
    assert!(create_task(&store, spec, now()).is_err());
    let mut spec = base_spec();
    spec.skills = vec!["browser-enabled".into()];
    spec.cluster = Some("pegasus".into());
    assert!(create_task(&store, spec, now()).is_err());
}

#[test]
fn explicit_assignee_cannot_gain_browser_access_from_task_skill() {
    let store = org_store();
    let mut spec = base_spec();
    spec.skills = vec!["browser-enabled".into()];
    spec.assignee = Some("research-survey".into());
    let error = create_task(&store, spec, now()).unwrap_err();
    assert!(
        error.to_string().contains("no browser capability grant"),
        "{error}"
    );
}

fn base_spec() -> NewTaskSpec {
    NewTaskSpec {
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        title: "do something".to_string(),
        objective: "make it work".to_string(),
        // ADR-0067 D2: `human` チェックには成果物か知識ベースの参照が要る。
        acceptance: vec![
            CriterionSpec::Human {
                text: "it works".to_string(),
            },
            CriterionSpec::ArtifactExists {
                name: "result.md".to_string(),
            },
        ],
        kind: TaskKind::Execute,
        tier: None,
        priority: Some(PriorityInput::Number(0)),
        parent: None,
        depends_on: vec![],
        max_turns: None,
        max_wall_secs: None,
        max_retries: 2,
        role: None,
        genre: None,
        aggregate: false,
        project_id: None,
        milestone_id: None,
        assignee: None,
        workspace: Some(PathBuf::from("/tmp/workspace")),
        cluster: None,
        workspace_mode: None,
        adapter: None,
        labels: Vec::new(),
        category: None,
        status: None,
        features: None,
        execution: None,
        pause_after: None,
        stages_hint: Vec::new(),
        provenance: SpecProvenance::default(),
    }
}

fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

#[test]
fn create_task_inserts_task_and_created_event() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let spec = base_spec();

    let task = create_task(&store, spec, now()).expect("create_task");

    assert_eq!(task.title, "do something");
    assert_eq!(task.objective, "make it work");
    assert_eq!(task.kind, TaskKind::Execute);
    assert_eq!(task.status, Status::Draft);
    assert_eq!(task.worker_hint.tier, Tier::Standard);
    assert_eq!(task.worker_hint.adapter, None);
    assert_eq!(task.budget.max_turns, 10);
    assert_eq!(task.budget.max_wall_secs, 600);
    assert_eq!(task.budget.max_retries, 2);
    assert_eq!(task.attempts, 0);
    assert!(task.lease.is_none());
    assert_eq!(
        task.workspace,
        WorkspaceSpec::Local {
            path: PathBuf::from("/tmp/workspace"),
            mode: None
        }
    );

    let fetched = store.get(task.id).expect("get").expect("some");
    assert_eq!(fetched, task);

    let events = store.events_for(task.id).expect("events_for");
    assert_eq!(events.len(), 1);
    match &events[0].1 {
        Event::Created { task: created, .. } => assert_eq!(created.id, task.id),
        other => panic!("expected Created event, got {other:?}"),
    }
}

/// ADR-0074 D2.1（Phase F3 途中確認、区切り 1 (a)）: 人（`SpecOrigin::Human`。既定）が書いた
/// `pause_after` は `Task.routing.pause_after` に写り、出自は `PauseSource::Human`。
#[test]
fn create_task_carries_pause_after_from_a_human_spec() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let mut spec = base_spec();
    spec.pause_after = Some(task_core::PausePolicy::EachPhase);

    let task = create_task(&store, spec, now()).expect("create_task");
    let routing = task.routing.expect("routing recorded");
    assert_eq!(routing.pause_after, task_core::PausePolicy::EachPhase);
    assert_eq!(routing.pause_after_source, task_core::PauseSource::Human);
}

/// (e): 省略時は `none`（既定）で、出自は `Human`（従来のタスクと 1 バイトも変わらない）。
#[test]
fn create_task_defaults_pause_after_to_none_with_human_source() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let spec = base_spec();

    let task = create_task(&store, spec, now()).expect("create_task");
    let routing = task.routing.expect("routing recorded");
    assert_eq!(routing.pause_after, task_core::PausePolicy::None);
    assert_eq!(routing.pause_after_source, task_core::PauseSource::Human);
}

/// CoS（`SpecOrigin::Agent`）が書いた `pause_after` は出自 `PauseSource::Agent` で記録される
/// （ADR-0069 D1 が `assignee`/`tier` を捨てるのとは違い、`pause_after` は止まる方向で安全側
/// なので CoS の値もそのまま採用する。ADR-0074 D2.1）。
#[test]
fn create_task_carries_pause_after_from_an_agent_spec_with_agent_source() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let mut spec = base_spec();
    spec.pause_after = Some(task_core::PausePolicy::After {
        phases: vec!["design".to_string()],
    });
    spec.provenance = SpecProvenance {
        origin: SpecOrigin::Agent,
        ..Default::default()
    };

    let task = create_task(&store, spec, now()).expect("create_task");
    let routing = task.routing.expect("routing recorded");
    assert_eq!(
        routing.pause_after,
        task_core::PausePolicy::After {
            phases: vec!["design".to_string()]
        }
    );
    assert_eq!(routing.pause_after_source, task_core::PauseSource::Agent);
}

/// ADR-0016 D1 / M3: タスクの値 > 役割の既定 > 全体の既定。設定に無い役割は名前だけ保存する。
#[test]
fn create_task_with_roles_fills_omitted_values_from_the_role_then_global_defaults() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let roles = vec![RoleSpec {
        id: "lead".to_string(),
        tier: Some(Tier::Frontier),
        adapter: Some("claude-code".to_string()),
        max_turns: Some(40),
        max_wall_secs: None,
        instructions: Some("you lead".to_string()),
    }];
    let mut spec = base_spec();
    spec.role = Some("lead".to_string());
    spec.aggregate = true;
    spec.max_turns = Some(7);
    let task = create_task_with_roles(&store, spec, &roles, &[], now()).expect("create");
    assert_eq!(task.role.as_deref(), Some("lead"));
    assert!(task.aggregate);
    assert_eq!(task.worker_hint.tier, Tier::Frontier, "role default");
    assert_eq!(
        task.worker_hint.adapter.as_deref(),
        Some("claude-code"),
        "role default"
    );
    assert_eq!(task.budget.max_turns, 7, "task value wins");
    assert_eq!(task.budget.max_wall_secs, 600, "global default");
    let json = serde_json::to_value(&task).unwrap();
    assert_eq!(json["role"], "lead");
    assert_eq!(json["aggregate"], true);

    let mut spec = base_spec();
    spec.role = Some("nobody".to_string());
    let task =
        create_task_with_roles(&store, spec, &roles, &[], now()).expect("unknown role is allowed");
    assert_eq!(task.role.as_deref(), Some("nobody"));
    assert_eq!(task.worker_hint.tier, Tier::Standard);
    assert_eq!(task.budget.max_turns, 10);
    let json = serde_json::to_value(&task).unwrap();
    assert!(json.get("aggregate").is_none(), "false is omitted: {json}");

    // 役割なしの JSON（旧クライアント）はそのまま読める。
    let spec: NewTaskSpec = serde_json::from_str(
        r#"{"title":"t","objective":"o","acceptance":[{"type":"human","text":"x"}],"tier":"cheap","max_turns":3}"#,
    )
    .unwrap();
    assert_eq!(spec.tier, Some(Tier::Cheap));
    assert_eq!(spec.max_turns, Some(3));
    assert_eq!(spec.max_wall_secs, None);
    assert!(spec.role.is_none());
}

// ---- ADR-0033 D2（Phase 23）: assignee から先に解決する ----

fn org_node(
    id: &str,
    parent: Option<&str>,
    kind: task_core::OrgKind,
    genre: Option<&str>,
) -> task_core::OrgNode {
    let now = OffsetDateTime::now_utc();
    task_core::OrgNode {
        profile: Default::default(),
        id: id.into(),
        parent_id: parent.map(str::to_string),
        name: id.into(),
        kind,
        genre: genre.map(str::to_string),
        brief: String::new(),
        position: 0,
        created_at: now,
        updated_at: now,
    }
}

fn org_store() -> SqliteStore {
    let store = SqliteStore::open_in_memory().expect("open store");
    store
        .org_upsert(&org_node(
            "secretary",
            None,
            task_core::OrgKind::Secretary,
            None,
        ))
        .expect("secretary");
    store
        .org_upsert(&org_node(
            "research",
            Some("secretary"),
            task_core::OrgKind::Department,
            None,
        ))
        .expect("department");
    store
        .org_upsert(&org_node(
            "research-survey",
            Some("research"),
            task_core::OrgKind::Section,
            Some("literature"),
        ))
        .expect("section");
    store
}

fn literature_setup() -> (Vec<RoleSpec>, Vec<GenreSpec>) {
    let roles = vec![
        RoleSpec {
            id: "literature-reader".into(),
            tier: Some(Tier::Cheap),
            adapter: Some("paperqa".into()),
            max_turns: Some(5),
            max_wall_secs: Some(1200),
            instructions: None,
        },
        RoleSpec {
            id: "lead".into(),
            tier: Some(Tier::Frontier),
            adapter: Some("claude-code".into()),
            ..RoleSpec::default()
        },
    ];
    let genres = vec![genre(
        "literature",
        Some("literature-reader"),
        &["literature-reader"],
    )];
    (roles, genres)
}

/// `assignee` があれば、そのノードの分野 →`default_role`→ 役割の既定で `WorkerHint` が埋まる。
#[test]
fn assignee_fills_the_worker_hint_from_the_org_nodes_genre() {
    let store = org_store();
    let (roles, genres) = literature_setup();
    let mut spec = base_spec();
    spec.assignee = Some("research-survey".into());
    let task = create_task_with_roles(&store, spec, &roles, &genres, now()).expect("create");
    assert_eq!(task.assignee.as_deref(), Some("research-survey"));
    assert_eq!(
        task.genre.as_deref(),
        Some("literature"),
        "the node's genre is adopted"
    );
    assert_eq!(task.worker_hint.tier, Tier::Cheap);
    assert_eq!(task.worker_hint.adapter.as_deref(), Some("paperqa"));
    assert_eq!(task.budget.max_turns, 5);
    assert_eq!(task.budget.max_wall_secs, 1200);
    assert_eq!(task.role, None, "assignee does not invent a role name");
}

/// ADR-0069 D1（Phase 114）: 人の経路（API / CLI。`SpecOrigin::Human` が既定）の `assignee` / `tier` は
/// 人の明示として従い、`routing` に出自が残る。`provenance` は API の JSON からは偽装できない。
#[test]
fn human_spec_records_explicit_provenance_and_cannot_be_spoofed() {
    let store = org_store();
    let (roles, genres) = literature_setup();
    let mut spec = base_spec();
    spec.assignee = Some("research-survey".into());
    spec.tier = Some(Tier::Frontier);
    let task = create_task_with_roles(&store, spec, &roles, &genres, now()).expect("create");
    assert_eq!(task.assignee.as_deref(), Some("research-survey"));
    let routing = task.routing.expect("routing");
    assert_eq!(routing.tier_source, task_core::TierSource::Human);
    assert!(routing.assignee_explicit);

    // tier を書かなければ policy が決める（Default）。System はいつも System。
    let task = create_task_with_roles(&store, base_spec(), &roles, &genres, now()).unwrap();
    assert_eq!(
        task.routing.map(|r| r.tier_source),
        Some(task_core::TierSource::Default)
    );
    let mut spec = base_spec();
    spec.provenance = SpecProvenance::system();
    let task = create_task_with_roles(&store, spec, &roles, &genres, now()).unwrap();
    assert_eq!(
        task.routing.map(|r| r.tier_source),
        Some(task_core::TierSource::System)
    );

    // `provenance` は `serde(skip)`: JSON に書いても未知フィールドとして拒否される。
    let err = serde_json::from_str::<NewTaskSpec>(
        r#"{"title":"t","objective":"o","acceptance":[],"provenance":{"origin":"system"}}"#,
    );
    assert!(err.is_err());
}

/// タスク自身の値は `assignee` より強い。`role` を明示したら、その役割・分野が優先される。
#[test]
fn explicit_values_still_win_over_the_assignee() {
    let store = org_store();
    let (roles, genres) = literature_setup();
    let mut spec = base_spec();
    spec.assignee = Some("research-survey".into());
    spec.tier = Some(Tier::Frontier);
    spec.max_turns = Some(33);
    let task = create_task_with_roles(&store, spec, &roles, &genres, now()).expect("create");
    assert_eq!(task.worker_hint.tier, Tier::Frontier, "the task value wins");
    assert_eq!(task.budget.max_turns, 33);
    assert_eq!(
        task.worker_hint.adapter.as_deref(),
        Some("paperqa"),
        "still filled from the assignee"
    );

    // 監査 D-2: `role` を明示すると、`role` の tier/adapter が `assignee` 由来の既定より勝つ
    // （解決順は task > role > assignee > genre.default_role）。`role` に無いフィールド（ここでは
    // `max_turns`）は次の階層（assignee の既定）まで降りて埋まる。`assignee` は常に記録される。
    let mut spec = base_spec();
    spec.assignee = Some("research-survey".into());
    spec.role = Some("lead".into());
    let task = create_task_with_roles(&store, spec, &roles, &genres, now())
        .expect("role wins over assignee");
    assert_eq!(task.role.as_deref(), Some("lead"));
    assert_eq!(
        task.assignee.as_deref(),
        Some("research-survey"),
        "assignee is still recorded"
    );
    assert_eq!(
        task.worker_hint.tier,
        Tier::Frontier,
        "the role's tier wins over the assignee's default"
    );
    assert_eq!(
        task.worker_hint.adapter.as_deref(),
        Some("claude-code"),
        "the role's adapter wins over the assignee's default"
    );
    assert_eq!(
        task.budget.max_turns, 5,
        "the role has no max_turns of its own, so the assignee's default fills it"
    );

    let mut spec = base_spec();
    spec.assignee = Some("research-survey".into());
    spec.role = Some("lead".into());
    spec.genre = Some("literature".into());
    let err = create_task_with_roles(&store, spec, &roles, &genres, now()).unwrap_err();
    assert!(err.to_string().contains("is not one of genre"), "{err}");
}

/// 分野を持たないノード（部）や `assignee` 無しでは、従来の解決順がそのまま残る。
#[test]
fn without_an_assignee_nothing_changes() {
    let store = org_store();
    let (roles, genres) = literature_setup();
    let task = create_task_with_roles(&store, base_spec(), &roles, &genres, now()).expect("create");
    assert_eq!(task.worker_hint.tier, Tier::Standard, "global default");
    assert_eq!(task.worker_hint.adapter, None);
    assert_eq!(task.budget.max_turns, DEFAULT_MAX_TURNS);
    assert_eq!(task.assignee, None);

    let mut spec = base_spec();
    spec.assignee = Some("research".into());
    let task = create_task_with_roles(&store, spec, &roles, &genres, now()).expect("create");
    assert_eq!(task.assignee.as_deref(), Some("research"));
    assert_eq!(task.genre, None, "a department has no genre");
    assert_eq!(task.worker_hint.tier, Tier::Standard);
}

/// 知らない `assignee`・存在しない案件・案件違いの途中目標は 422 相当の検証エラー。
#[test]
fn unknown_assignee_project_or_milestone_is_rejected() {
    let store = org_store();
    let (roles, genres) = literature_setup();
    let mut spec = base_spec();
    spec.assignee = Some("nobody".into());
    let err = create_task_with_roles(&store, spec, &roles, &genres, now()).unwrap_err();
    assert!(err.to_string().contains("is not an org node"), "{err}");

    let mut spec = base_spec();
    spec.project_id = Some(task_core::ProjectId::new());
    let err = create_task_with_roles(&store, spec, &roles, &genres, now()).unwrap_err();
    assert!(err.to_string().contains("does not exist"), "{err}");

    let now_ts = OffsetDateTime::now_utc();
    let project = task_core::Project {
        auto_advance: false,
        slug: None,
        archived_at: None,
        paused_from: None,
        id: task_core::ProjectId::new(),
        title: "t".into(),
        request: "r".into(),
        status: task_core::ProjectStatus::Active,
        secretary_summary: None,
        workspace: None,
        created_at: now_ts,
        updated_at: now_ts,
    };
    store.project_create(&project).unwrap();
    let other = task_core::Project {
        id: task_core::ProjectId::new(),
        ..project.clone()
    };
    store.project_create(&other).unwrap();
    let milestone = store
        .milestone_create(other.id, "m", "", task_core::MilestoneStatus::Approved)
        .unwrap();

    let mut spec = base_spec();
    spec.project_id = Some(project.id);
    spec.milestone_id = Some(milestone.id);
    let err = create_task_with_roles(&store, spec, &roles, &genres, now()).unwrap_err();
    assert!(
        err.to_string().contains("does not belong to project"),
        "{err}"
    );

    let mut spec = base_spec();
    spec.milestone_id = Some(milestone.id);
    let err = create_task_with_roles(&store, spec, &roles, &genres, now()).unwrap_err();
    assert!(
        err.to_string().contains("milestone_id requires project_id"),
        "{err}"
    );

    // 正しい組み合わせは通り、列にも載る。
    let mut spec = base_spec();
    spec.project_id = Some(other.id);
    spec.milestone_id = Some(milestone.id);
    let task = create_task_with_roles(&store, spec, &roles, &genres, now()).expect("create");
    assert_eq!(task.project_id, Some(other.id));
    assert_eq!(task.milestone_id, Some(milestone.id));
}

fn a_project(store: &dyn task_core::TaskStore) -> task_core::Project {
    let now_ts = OffsetDateTime::now_utc();
    let project = task_core::Project {
        auto_advance: false,
        slug: None,
        archived_at: None,
        paused_from: None,
        id: task_core::ProjectId::new(),
        title: "t".into(),
        request: "r".into(),
        status: task_core::ProjectStatus::Active,
        secretary_summary: None,
        workspace: None,
        created_at: now_ts,
        updated_at: now_ts,
    };
    store.project_create(&project).unwrap();
    project
}

fn a_project_with_workspace(
    store: &dyn task_core::TaskStore,
    workspace: WorkspaceSpec,
) -> task_core::Project {
    let now_ts = OffsetDateTime::now_utc();
    let project = task_core::Project {
        auto_advance: false,
        slug: None,
        archived_at: None,
        paused_from: None,
        id: task_core::ProjectId::new(),
        title: "benchfs".into(),
        request: "r".into(),
        status: task_core::ProjectStatus::Active,
        secretary_summary: None,
        workspace: Some(workspace),
        created_at: now_ts,
        updated_at: now_ts,
    };
    store.project_create(&project).unwrap();
    project
}

/// R5b-fix3 (D1): 案件の primary がリモート（sirius）なら、`workspace` / `cluster` を省いた root task は
/// 案件の workspace を継ぐ（手元の空のディレクトリで走らない）。`workspace_mode` は上書きとして効く。
#[test]
fn project_task_inherits_a_remote_project_workspace() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let remote = WorkspaceSpec::Remote {
        cluster: "sirius".into(),
        path: "/work/NBB/rmaeda/workspace/rust/benchfs".into(),
        mode: None,
    };
    let project = a_project_with_workspace(&store, remote.clone());
    let mut spec = base_spec();
    spec.project_id = Some(project.id);
    spec.workspace = None;
    let task = create_task(&store, spec, now()).expect("create");
    assert_eq!(task.workspace, remote);
    assert_eq!(task.repos.len(), 1, "the primary repo is inherited");

    let mut spec = base_spec();
    spec.project_id = Some(project.id);
    spec.workspace = None;
    spec.workspace_mode = Some(task_core::WorkspaceMode::Worktree);
    let task = create_task(&store, spec, now()).expect("create");
    assert_eq!(
        task.workspace,
        WorkspaceSpec::Remote {
            cluster: "sirius".into(),
            path: "/work/NBB/rmaeda/workspace/rust/benchfs".into(),
            mode: Some(task_core::WorkspaceMode::Worktree),
        }
    );
}

/// R5b-fix3 (D1): 明示の `Local` の作業場所とリモートの primary の組み合わせは 422（Validation）。
#[test]
fn explicit_local_workspace_with_a_remote_repo_is_rejected() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let project = a_project_with_workspace(
        &store,
        WorkspaceSpec::Remote {
            cluster: "sirius".into(),
            path: "/work/NBB/rmaeda/workspace/rust/benchfs".into(),
            mode: None,
        },
    );
    let mut spec = base_spec();
    spec.project_id = Some(project.id);
    spec.workspace = Some("/tmp/somewhere".into());
    let err = create_task(&store, spec, now()).unwrap_err();
    assert!(matches!(err, OpsError::Validation(_)), "{err:?}");
    assert!(err.to_string().contains("remote repo"), "{err}");
}

/// R5b-fix3 (D1): ローカルの案件は従来どおり `Local{<id>}`（リポジトリは `<id>/repos/<name>` に並ぶ）。
#[test]
fn project_task_with_a_local_primary_keeps_its_own_dir() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let project = a_project_with_workspace(&store, WorkspaceSpec::local("/home/u/repo"));
    let mut spec = base_spec();
    spec.project_id = Some(project.id);
    spec.workspace = None;
    let task = create_task(&store, spec, now()).expect("create");
    assert_eq!(task.workspace, WorkspaceSpec::local(task.id.to_string()));
    assert_eq!(task.repos.len(), 1);
}

/// ADR-0079 D13（Phase R5a）: 案件直下に `milestone_id` 無しで作った root task にも、その子にも途中目標の行は
/// できない（ADR-0074 D3.8 の自動作成の廃止）。既存の途中目標の行は状態が変わらない。
#[test]
fn no_milestone_rows_for_new_root_tasks() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let project = a_project(&store);
    let frozen = store
        .milestone_create(
            project.id,
            "以前の途中目標",
            "",
            task_core::MilestoneStatus::InProgress,
        )
        .expect("create milestone");

    let mut spec = base_spec();
    spec.title = "隣接領域の調査".into();
    spec.project_id = Some(project.id);
    let root = create_task(&store, spec, now()).expect("create_task");
    assert!(task_core::is_root_task(&root));
    assert_eq!(root.milestone_id, None, "root task に途中目標を結ばない");

    let mut child_spec = base_spec();
    child_spec.project_id = Some(project.id);
    child_spec.parent = Some(root.id);
    let child = create_task(&store, child_spec, now()).expect("create child");
    assert!(!task_core::is_root_task(&child));
    assert_eq!(child.milestone_id, None);

    let milestones = store.milestone_list(project.id).expect("list");
    assert_eq!(milestones.len(), 1, "新しい途中目標の行はできない");
    assert_eq!(milestones[0].id, frozen.id);
    assert_eq!(
        milestones[0].status,
        task_core::MilestoneStatus::InProgress,
        "既存の行は凍結（状態が変わらない）"
    );
}

/// 人が既存の途中目標を明示したとき（旧い使い方）は結ぶだけで、新しい行は作らない。
#[test]
fn an_explicit_milestone_id_is_kept_without_creating_rows() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let project = a_project(&store);
    let milestone = store
        .milestone_create(
            project.id,
            "手で作った途中目標",
            "",
            task_core::MilestoneStatus::Approved,
        )
        .expect("create milestone");

    let mut spec = base_spec();
    spec.project_id = Some(project.id);
    spec.milestone_id = Some(milestone.id);
    let task = create_task(&store, spec, now()).expect("create_task");

    assert_eq!(task.milestone_id, Some(milestone.id));
    let milestones = store.milestone_list(project.id).expect("list");
    assert_eq!(milestones.len(), 1, "新しい途中目標を作らない");
}

/// ADR-0079 D12（Phase R5a）: `NewTaskSpec.stages_hint` は `Task.routing.stages_hint` にそのまま写る。
#[test]
fn create_task_carries_stages_hint_into_routing() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let mut spec = base_spec();
    spec.stages_hint = vec![
        task_core::StageHint {
            title: "Phase 1".into(),
            scope: "MVP".into(),
        },
        task_core::StageHint {
            title: "Phase 2".into(),
            scope: String::new(),
        },
    ];
    let task = create_task(&store, spec, now()).expect("create_task");
    let stored = store.get(task.id).expect("get").expect("some");
    let routing = stored.routing.expect("routing");
    assert_eq!(routing.stages_hint.len(), 2);
    assert_eq!(routing.stages_hint[0].title, "Phase 1");
    assert_eq!(routing.stages_hint[0].scope, "MVP");
    assert_eq!(routing.stages_hint[1].title, "Phase 2");
}

fn genre(id: &str, default_role: Option<&str>, roles: &[&str]) -> GenreSpec {
    GenreSpec {
        id: id.into(),
        description: format!("{id} description"),
        default_role: default_role.map(str::to_string),
        roles: roles.iter().map(|r| r.to_string()).collect(),
        ..GenreSpec::default()
    }
}

/// ADR-0027 D1: タスクの値 > 役割の既定 > 分野の既定（`default_role` の役割）> 全体の既定。
/// 役割が無くても分野だけで既定が効く（「genre だけ」のケース）。
#[test]
fn create_task_with_roles_applies_genre_default_role_when_task_has_no_role() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let roles = vec![RoleSpec {
        id: "literature-reader".to_string(),
        tier: Some(Tier::Standard),
        adapter: Some("acp".to_string()),
        max_turns: Some(5),
        max_wall_secs: Some(1200),
        instructions: None,
    }];
    let genres = vec![genre(
        "literature",
        Some("literature-reader"),
        &["literature-reader"],
    )];
    let mut spec = base_spec();
    spec.genre = Some("literature".to_string());
    let task = create_task_with_roles(&store, spec, &roles, &genres, now()).expect("create");
    assert_eq!(task.role, None, "genre alone must not set the task's role");
    assert_eq!(task.genre.as_deref(), Some("literature"));
    assert_eq!(task.worker_hint.tier, Tier::Standard);
    assert_eq!(task.worker_hint.adapter.as_deref(), Some("acp"));
    assert_eq!(task.budget.max_turns, 5);
    assert_eq!(task.budget.max_wall_secs, 1200);
}

/// ADR-0027 D1: `genre` 未指定で `role` がちょうど 1 つの分野に属するなら、その分野を継ぐ。
#[test]
fn create_task_with_roles_infers_genre_from_a_role_that_belongs_to_exactly_one_genre() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let roles = vec![RoleSpec {
        id: "literature-scout".to_string(),
        ..RoleSpec::default()
    }];
    let genres = vec![genre(
        "literature",
        Some("literature-reader"),
        &["literature-scout"],
    )];
    let mut spec = base_spec();
    spec.role = Some("literature-scout".to_string());
    let task = create_task_with_roles(&store, spec, &roles, &genres, now()).expect("create");
    assert_eq!(task.genre.as_deref(), Some("literature"));
}

/// ADR-0027 D1: `genres` が設定されているとき、知らない `genre` はエラー、`genre` + `role` の
/// 不整合（`role` がその分野の `roles` に無い）もエラー（何も挿入しない）。`genres` が空の設定
/// （`--config` 無しの `celerisctl add`）では検証しない。
#[test]
fn create_task_with_roles_rejects_unknown_genre_and_role_genre_mismatch_only_when_genres_configured()
 {
    let store = SqliteStore::open_in_memory().expect("open store");
    let genres = vec![genre(
        "coding",
        Some("implementer"),
        &["lead", "implementer"],
    )];

    let mut spec = base_spec();
    spec.genre = Some("literature".to_string());
    let err = create_task_with_roles(&store, spec, &[], &genres, now()).unwrap_err();
    assert!(matches!(err, OpsError::Validation(_)), "{err:?}");
    assert!(store.list(None).expect("list").is_empty());

    let mut spec = base_spec();
    spec.genre = Some("coding".to_string());
    spec.role = Some("literature-scout".to_string());
    let err = create_task_with_roles(&store, spec, &[], &genres, now()).unwrap_err();
    assert!(matches!(err, OpsError::Validation(_)), "{err:?}");
    assert!(store.list(None).expect("list").is_empty());

    // `genres` が空: 分野を使わない設定では検証しない（自由記述のまま保存する）。
    let mut spec = base_spec();
    spec.genre = Some("literature".to_string());
    let task = create_task_with_roles(&store, spec, &[], &[], now()).expect("no genres configured");
    assert_eq!(task.genre.as_deref(), Some("literature"));
}

#[test]
fn create_task_with_approval_kind_starts_ready() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let mut spec = base_spec();
    spec.kind = TaskKind::Approval;

    let task = create_task(&store, spec, now()).expect("create_task");
    assert_eq!(task.status, Status::Ready);
}

#[test]
fn create_task_preserves_acceptance_order_as_given() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let mut spec = base_spec();
    spec.acceptance = vec![
        CriterionSpec::Human {
            text: "human check".to_string(),
        },
        CriterionSpec::Command {
            cmd: "cargo test".to_string(),
            expect_exit: 0,
        },
        CriterionSpec::ArtifactExists {
            name: "bench.json".to_string(),
        },
        CriterionSpec::Reviewer {
            text: "looks good".to_string(),
        },
    ];

    let task = create_task(&store, spec, now()).expect("create_task");
    assert_eq!(task.acceptance.len(), 4);
    assert_eq!(task.acceptance[0].check, Check::Human);
    assert!(matches!(task.acceptance[1].check, Check::Command { .. }));
    assert!(matches!(
        task.acceptance[2].check,
        Check::ArtifactExists { .. }
    ));
    assert_eq!(task.acceptance[3].check, Check::Reviewer);
}

#[test]
fn create_task_check_cmd_produces_command_criterion_with_expected_text() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let mut spec = base_spec();
    spec.acceptance = vec![CriterionSpec::Command {
        cmd: "cargo test".to_string(),
        expect_exit: 0,
    }];

    let task = create_task(&store, spec, now()).expect("create_task");
    assert_eq!(task.acceptance.len(), 1);
    assert_eq!(task.acceptance[0].text, "`cargo test` exits 0");
    assert_eq!(
        task.acceptance[0].check,
        Check::Command {
            cmd: "cargo test".to_string(),
            expect_exit: 0
        }
    );
}

#[test]
fn create_task_check_artifact_produces_artifact_exists_criterion_with_expected_text() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let mut spec = base_spec();
    spec.acceptance = vec![CriterionSpec::ArtifactExists {
        name: "bench.json".to_string(),
    }];

    let task = create_task(&store, spec, now()).expect("create_task");
    assert_eq!(task.acceptance.len(), 1);
    assert_eq!(task.acceptance[0].text, "artifact bench.json exists");
    assert_eq!(
        task.acceptance[0].check,
        Check::ArtifactExists {
            name: "bench.json".to_string()
        }
    );
}

#[test]
fn create_task_check_reviewer_produces_reviewer_criterion_with_text_verbatim() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let mut spec = base_spec();
    spec.acceptance = vec![CriterionSpec::Reviewer {
        text: "the diff is minimal and well-tested".to_string(),
    }];

    let task = create_task(&store, spec, now()).expect("create_task");
    assert_eq!(task.acceptance.len(), 1);
    assert_eq!(
        task.acceptance[0].text,
        "the diff is minimal and well-tested"
    );
    assert_eq!(task.acceptance[0].check, Check::Reviewer);
}

#[test]
fn create_task_without_workspace_defaults_to_relative_task_id_path() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let mut spec = base_spec();
    spec.workspace = None;

    let task = create_task(&store, spec, now()).expect("create_task");
    assert_eq!(
        task.workspace,
        WorkspaceSpec::Local {
            path: PathBuf::from(task.id.to_string()),
            mode: None
        }
    );
}

#[test]
fn create_task_without_any_acceptance_criterion_errors_and_inserts_nothing() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let mut spec = base_spec();
    spec.acceptance = vec![];

    let result = create_task(&store, spec, now());
    assert!(matches!(result, Err(OpsError::Validation(_))));
    assert!(store.list(None).expect("list tasks").is_empty());
}

#[test]
fn create_task_with_missing_dependency_errors_and_inserts_nothing() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let mut spec = base_spec();
    spec.depends_on = vec![TaskId::new()];

    let result = create_task(&store, spec, now());
    assert!(matches!(result, Err(OpsError::Validation(_))));
    assert!(store.list(None).expect("list tasks").is_empty());
}

/// ADR-0018: `cluster` を指定すると `WorkspaceSpec::Remote` になり、`workspace` はクラスタ側のパスになる。
/// ADR-0059 D1: `workspace_mode` を省略すると `mode: None`（従来どおりクラスタの `sync` に従う）。
#[test]
fn create_task_with_cluster_makes_a_remote_workspace() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let mut spec = base_spec();
    spec.cluster = Some("pegasus".to_string());
    spec.workspace = Some(PathBuf::from("/work/NBB/rmaeda/workspace/rust/benchfs"));
    let task = create_task(&store, spec, now()).expect("create");
    assert_eq!(
        task.workspace,
        task_core::WorkspaceSpec::Remote {
            cluster: "pegasus".to_string(),
            path: PathBuf::from("/work/NBB/rmaeda/workspace/rust/benchfs"),
            mode: None,
        }
    );

    // workspace を省略するとタスク ID のディレクトリ（クラスタ側の相対パス）になる。
    let mut spec = base_spec();
    spec.cluster = Some("pegasus".to_string());
    spec.workspace = None;
    let task = create_task(&store, spec, now()).expect("create");
    assert_eq!(
        task.workspace,
        task_core::WorkspaceSpec::Remote {
            cluster: "pegasus".to_string(),
            path: PathBuf::from(task.id.to_string()),
            mode: None,
        }
    );

    // ADR-0059 D1: `workspace_mode = "shared"` を明示すると `mode: Some(Shared)` になる。
    let mut spec = base_spec();
    spec.cluster = Some("pegasus".to_string());
    spec.workspace = Some(PathBuf::from("~"));
    spec.workspace_mode = Some(task_core::WorkspaceMode::Shared);
    let task = create_task(&store, spec, now()).expect("create");
    assert_eq!(
        task.workspace,
        task_core::WorkspaceSpec::Remote {
            cluster: "pegasus".to_string(),
            path: PathBuf::from("~"),
            mode: Some(task_core::WorkspaceMode::Shared),
        }
    );
}

/// ADR-0014 D3（P-G16）: 空白だけの title / objective、存在しない親は検証エラーで、何も挿入しない。
#[test]
fn create_task_rejects_blank_title_or_objective_and_missing_parent() {
    type Mutate = fn(&mut NewTaskSpec);
    let store = SqliteStore::open_in_memory().expect("open store");
    let cases: Vec<(Mutate, &str)> = vec![
        (|s| s.title = "  ".into(), "title must not be blank"),
        (|s| s.objective = "\n".into(), "objective must not be blank"),
        (|s| s.parent = Some(TaskId::new()), "does not exist"),
    ];
    for (mutate, expected) in cases {
        let mut spec = base_spec();
        mutate(&mut spec);
        match create_task(&store, spec, now()) {
            Err(OpsError::Validation(msg)) => assert!(msg.contains(expected), "{msg}"),
            other => {
                panic!("expected a validation error containing {expected:?}, got {other:?}")
            }
        }
    }
    assert!(store.list(None).expect("list tasks").is_empty());

    let parent = create_task(&store, base_spec(), now()).expect("parent");
    let mut child = base_spec();
    child.parent = Some(parent.id);
    assert_eq!(
        create_task(&store, child, now()).expect("child").parent_id,
        Some(parent.id)
    );
}

#[test]
fn create_task_with_failed_dependency_errors_and_inserts_nothing() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let mut dep_spec = base_spec();
    dep_spec.workspace = Some(PathBuf::from("/tmp/dep"));
    let dep = create_task(&store, dep_spec, now()).expect("create dep");

    // Drive the dependency to `failed` via a valid path:
    // draft -> accept -> ready -> acquire_lease -> running -> worker_error(false) -> failed.
    store
        .apply_transition(dep.id, task_core::Trigger::Accept, None)
        .expect("accept dep");
    let acquired = store
        .acquire_lease(dep.id, "run-dep", std::time::Duration::from_secs(60))
        .expect("acquire lease");
    assert!(acquired);
    store
        .apply_transition(
            dep.id,
            task_core::Trigger::WorkerError { retryable: false },
            None,
        )
        .expect("fail dep");
    assert_eq!(
        store.get(dep.id).expect("get").expect("some").status,
        Status::Failed
    );

    let mut spec = base_spec();
    spec.depends_on = vec![dep.id];

    let result = create_task(&store, spec, now());
    assert!(result.is_err());
    // Only the dependency task should exist; the new task must not be inserted.
    assert_eq!(store.list(None).expect("list tasks").len(), 1);
}
