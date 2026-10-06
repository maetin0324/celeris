use super::*;
use crate::model::{Budget, Check, WorkspaceSpec};
use std::path::PathBuf;

fn dt(title: &str, deps: Vec<DelegateDep>) -> DelegateTask {
    DelegateTask {
        requirements: Default::default(),
        title: title.into(),
        objective: format!("do {title}"),
        acceptance: vec![Criterion {
            text: "c".into(),
            check: Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
        }],
        role: None,
        genre: None,
        depends_on: deps,
        tier: None,
        assignee: None,
        workspace: None,
    }
}

fn parent() -> Task {
    let now = OffsetDateTime::now_utc();
    Task {
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "p".into(),
        objective: "o".into(),
        acceptance: vec![],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Running,
        priority: 3,
        worker_hint: WorkerHint {
            tier: Tier::Frontier,
            adapter: Some("fake".into()),
        },
        workspace: WorkspaceSpec::Local {
            path: PathBuf::from("/tmp/ws"),
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
        role: Some("lead".into()),
        genre: Some("coding".into()),
        aggregate: true,
        project_id: None,
        milestone_id: None,
        assignee: None,
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    }
}

#[test]
fn depends_on_accepts_index_or_id_and_rejects_other_shapes() {
    let json = r#"{"title":"a","objective":"o","acceptance":[{"text":"c","check":{"type":"human"}}],
            "depends_on":[0,"01J9ZX5T3K8Q7W6V5R4P3N2M1H"]}"#;
    let t: DelegateTask = serde_json::from_str(json).unwrap();
    assert_eq!(t.depends_on[0], DelegateDep::Index(0));
    assert_eq!(
        t.depends_on[1],
        DelegateDep::Id("01J9ZX5T3K8Q7W6V5R4P3N2M1H".into())
    );
    assert!(
        serde_json::from_str::<DelegateTask>(
            r#"{"title":"a","objective":"o","acceptance":[],"bogus":1}"#
        )
        .is_err()
    );
    assert!(
        serde_json::from_str::<DelegateTask>(
            r#"{"title":"a","objective":"o","acceptance":[],"depends_on":[true]}"#
        )
        .is_err()
    );
}

#[test]
fn validate_each_reports_per_item_and_marks_cycles() {
    let tasks = vec![
        dt("ok", vec![]),
        dt("", vec![]),
        dt("self", vec![DelegateDep::Index(2)]),
        dt("range", vec![DelegateDep::Index(9)]),
        dt("cyc-a", vec![DelegateDep::Index(5)]),
        dt("cyc-b", vec![DelegateDep::Index(4)]),
        dt("bad-id", vec![DelegateDep::Id("nope".into())]),
        dt("dep-ok", vec![DelegateDep::Index(0)]),
    ];
    let r = validate_each(&tasks, &[]);
    assert!(r[0].is_ok());
    assert_eq!(
        r[1],
        Err(DelegateError::EmptyField {
            index: 1,
            field: "title"
        })
    );
    assert_eq!(r[2], Err(DelegateError::SelfDependency { index: 2 }));
    assert!(matches!(
        r[3],
        Err(DelegateError::DependencyOutOfRange {
            index: 3,
            target: 9,
            len: 8,
            ..
        })
    ));
    assert!(
        matches!(r[4], Err(DelegateError::Cycle { .. })),
        "{:?}",
        r[4]
    );
    assert!(
        matches!(r[5], Err(DelegateError::Cycle { .. })),
        "{:?}",
        r[5]
    );
    assert!(matches!(
        r[6],
        Err(DelegateError::InvalidId { index: 6, .. })
    ));
    assert!(r[7].is_ok());
    let mut no_acc = dt("x", vec![]);
    no_acc.acceptance.clear();
    assert_eq!(
        validate_each(&[no_acc], &[])[0],
        Err(DelegateError::NoAcceptance { index: 0 })
    );
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

/// ADR-0027 D1: `genre` が知らない id なら `UnknownGenre`。`genre` + `role` で `role` がその分野の
/// `roles` に無ければ `RoleNotInGenre`。分野を使わない設定（`genres` が空）でも同じ扱い。
#[test]
fn validate_each_rejects_unknown_genre_and_role_not_in_genre() {
    let genres = vec![genre(
        "coding",
        Some("implementer"),
        &["lead", "implementer"],
    )];
    let mut unknown = dt("a", vec![]);
    unknown.genre = Some("literature".into());
    let mut mismatched = dt("b", vec![]);
    mismatched.genre = Some("coding".into());
    mismatched.role = Some("literature-scout".into());
    let mut ok = dt("c", vec![]);
    ok.genre = Some("coding".into());
    ok.role = Some("implementer".into());
    let r = validate_each(&[unknown, mismatched, ok], &genres);
    assert_eq!(
        r[0],
        Err(DelegateError::UnknownGenre {
            index: 0,
            genre: "literature".into()
        })
    );
    assert_eq!(
        r[1],
        Err(DelegateError::RoleNotInGenre {
            index: 1,
            role: "literature-scout".into(),
            genre: "coding".into(),
        })
    );
    assert!(r[2].is_ok());

    // 分野を使わない設定（`[[genres]]` が無い）でも `genre` を指定すれば同じくエラー。
    let mut no_config = dt("d", vec![]);
    no_config.genre = Some("coding".into());
    assert_eq!(
        validate_each(&[no_config], &[])[0],
        Err(DelegateError::UnknownGenre {
            index: 0,
            genre: "coding".into()
        })
    );
}

#[test]
fn materialize_applies_role_defaults_and_maps_index_dependencies_of_accepted_only() {
    let p = parent();
    let roles = vec![RoleSpec {
        id: "implementer".into(),
        tier: Some(Tier::Cheap),
        adapter: Some("codex".into()),
        max_turns: Some(3),
        max_wall_secs: None,
        instructions: Some("implement".into()),
    }];
    let mut a = dt("a", vec![]);
    a.role = Some("implementer".into());
    let b = dt("b", vec![DelegateDep::Index(0), DelegateDep::Index(2)]);
    let mut c = dt("c", vec![]);
    c.tier = Some(Tier::Standard);
    c.role = Some("implementer".into());
    let tasks = vec![a, b, c];
    let out = materialize_delegated(
        &p,
        &tasks,
        &[0, 1],
        &[],
        &roles,
        &[],
        WorkspaceContext::default(),
        OffsetDateTime::now_utc(),
    );
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].parent_id, Some(p.id));
    assert_eq!(out[0].status, Status::Draft);
    assert_eq!(out[0].role.as_deref(), Some("implementer"));
    assert_eq!(out[0].worker_hint.tier, Tier::Cheap);
    assert_eq!(out[0].worker_hint.adapter.as_deref(), Some("codex"));
    assert_eq!(out[0].budget.max_turns, 3);
    assert_eq!(out[0].budget.max_wall_secs, 600);
    assert_eq!(out[0].priority, 3);
    assert!(!out[0].aggregate);
    // 分野の設定が無くても、role/genre 共に未指定な子は親の分野をそのまま継ぐ。
    assert_eq!(out[0].genre.as_deref(), Some("coding"));
    // b は a（採用）だけに依存し、c（不採用）への依存は落ちる。役割無しは親の tier / adapter。
    assert_eq!(out[1].depends_on, vec![out[0].id]);
    assert_eq!(out[1].worker_hint.tier, Tier::Frontier);
    assert_eq!(out[1].worker_hint.adapter.as_deref(), Some("fake"));
    // タスクの tier は役割の既定より優先。
    let out = materialize_delegated(
        &p,
        &tasks,
        &[2],
        &[],
        &roles,
        &[],
        WorkspaceContext::default(),
        OffsetDateTime::now_utc(),
    );
    assert_eq!(out[0].worker_hint.tier, Tier::Standard);
}

/// ADR-0027 D1: 子の分野は `genre` の明示 > `role` の分野（一意なら） > 親の分野の順。
#[test]
fn materialize_resolves_genre_by_precedence() {
    let p = parent(); // genre = Some("coding")
    let genres = vec![
        genre("coding", Some("implementer"), &["lead", "implementer"]),
        genre(
            "literature",
            Some("literature-reader"),
            &["literature-scout", "literature-reader"],
        ),
    ];

    // 1. 明示した genre が最優先（role の分野や親の分野より勝つ）。
    let mut explicit = dt("explicit", vec![]);
    explicit.role = Some("implementer".into()); // coding の役割
    explicit.genre = Some("literature".into());
    let out = materialize_delegated(
        &p,
        &[explicit],
        &[0],
        &[],
        &[],
        &genres,
        WorkspaceContext::default(),
        OffsetDateTime::now_utc(),
    );
    assert_eq!(out[0].genre.as_deref(), Some("literature"));

    // 2. genre 未指定・role がちょうど 1 つの分野に属する: その分野を継ぐ。
    let mut by_role = dt("by-role", vec![]);
    by_role.role = Some("literature-scout".into());
    let out = materialize_delegated(
        &p,
        &[by_role],
        &[0],
        &[],
        &[],
        &genres,
        WorkspaceContext::default(),
        OffsetDateTime::now_utc(),
    );
    assert_eq!(out[0].genre.as_deref(), Some("literature"));

    // 3. role が複数の分野に属する（一意に決まらない）: 親の分野を継ぐ。
    let ambiguous_genres = vec![
        genre("coding", None, &["shared"]),
        genre("literature", None, &["shared"]),
    ];
    let mut ambiguous = dt("ambiguous", vec![]);
    ambiguous.role = Some("shared".into());
    let out = materialize_delegated(
        &p,
        &[ambiguous],
        &[0],
        &[],
        &[],
        &ambiguous_genres,
        WorkspaceContext::default(),
        OffsetDateTime::now_utc(),
    );
    assert_eq!(
        out[0].genre.as_deref(),
        Some("coding"),
        "falls back to the parent's genre"
    );

    // 4. genre も role も無い: 親の分野を継ぐ。
    let none_of_either = dt("neither", vec![]);
    let out = materialize_delegated(
        &p,
        &[none_of_either],
        &[0],
        &[],
        &[],
        &genres,
        WorkspaceContext::default(),
        OffsetDateTime::now_utc(),
    );
    assert_eq!(out[0].genre.as_deref(), Some("coding"));
}

/// ADR-0027 D1: role 無しで genre だけ指定した子は、その分野の `default_role` の既定
/// （tier / adapter / budget）を借りる（「genre だけ」のケース）。
#[test]
fn materialize_applies_genre_default_role_when_task_has_no_role() {
    let p = parent();
    let roles = vec![RoleSpec {
        id: "literature-reader".into(),
        tier: Some(Tier::Standard),
        adapter: Some("acp".into()),
        max_turns: Some(5),
        max_wall_secs: Some(1200),
        instructions: None,
    }];
    let genres = vec![genre(
        "literature",
        Some("literature-reader"),
        &["literature-reader"],
    )];
    let mut t = dt("investigate", vec![]);
    t.genre = Some("literature".into());
    let out = materialize_delegated(
        &p,
        &[t],
        &[0],
        &[],
        &roles,
        &genres,
        WorkspaceContext::default(),
        OffsetDateTime::now_utc(),
    );
    assert_eq!(
        out[0].role, None,
        "genre alone must not set the task's role"
    );
    assert_eq!(out[0].genre.as_deref(), Some("literature"));
    assert_eq!(out[0].worker_hint.tier, Tier::Standard);
    assert_eq!(out[0].worker_hint.adapter.as_deref(), Some("acp"));
    assert_eq!(out[0].budget.max_turns, 5);
    assert_eq!(out[0].budget.max_wall_secs, 1200);
}

/// ADR-0027 D1 の決め方の順（tier/adapter/budget）: タスクの値 > 役割の既定 > 分野の既定 > 親の値。
/// 役割自身に既定が無いフィールドだけ、分野の既定（`default_role` の役割）が埋める。
#[test]
fn materialize_role_default_wins_over_genre_default_which_wins_over_parent() {
    let p = parent(); // tier = Frontier, adapter = fake
    let roles = vec![
        RoleSpec {
            id: "implementer".into(),
            tier: None, // 役割自身は tier を決めない -> 分野の既定に委ねる
            adapter: Some("codex".into()),
            max_turns: None,
            max_wall_secs: None,
            instructions: None,
        },
        RoleSpec {
            id: "lead".into(),
            tier: Some(Tier::Cheap),
            adapter: None,
            max_turns: Some(20),
            max_wall_secs: None,
            instructions: None,
        },
    ];
    let genres = vec![genre("coding", Some("lead"), &["lead", "implementer"])];
    let mut t = dt("impl", vec![]);
    t.role = Some("implementer".into());
    t.genre = Some("coding".into());
    let out = materialize_delegated(
        &p,
        &[t],
        &[0],
        &[],
        &roles,
        &genres,
        WorkspaceContext::default(),
        OffsetDateTime::now_utc(),
    );
    // adapter は role（implementer）の既定が優先（parent の "fake" にも分野の既定にも負けない）。
    assert_eq!(out[0].worker_hint.adapter.as_deref(), Some("codex"));
    // tier は role（implementer）に既定が無いので、分野の既定役割（lead）の tier を借りる
    // （親の tier = Frontier ではなく Cheap になる）。
    assert_eq!(out[0].worker_hint.tier, Tier::Cheap);
    // max_turns も role に無いので分野の既定役割から。
    assert_eq!(out[0].budget.max_turns, 20);
}

/// ADR-0033 D4（Phase 24）→ ADR-0069 D1（Phase 114）: 委譲（LLM）が書いた `assignee` は**捨て**、
/// `routing.dropped_assignee` に残す。担当の分野も既定の解決に使わない（担当は matching が決める）。
#[test]
fn materialize_drops_the_delegated_assignee_and_records_it() {
    use crate::org::{OrgKind, OrgNode};
    let now = OffsetDateTime::now_utc();
    let node = |id: &str, genre_id: Option<&str>| OrgNode {
        profile: Default::default(),
        id: id.into(),
        parent_id: Some("research".into()),
        name: id.into(),
        kind: OrgKind::Section,
        genre: genre_id.map(str::to_string),
        brief: String::new(),
        position: 0,
        created_at: now,
        updated_at: now,
    };
    let org = vec![
        node("research-survey", Some("literature")),
        node("research-data", None),
    ];
    let roles = vec![
        RoleSpec {
            id: "literature-reader".into(),
            tier: Some(Tier::Cheap),
            adapter: Some("paperqa".into()),
            max_turns: Some(5),
            ..RoleSpec::default()
        },
        RoleSpec {
            id: "writer".into(),
            tier: Some(Tier::Standard),
            adapter: Some("claude-code".into()),
            ..RoleSpec::default()
        },
    ];
    let genres = vec![genre(
        "literature",
        Some("literature-reader"),
        &["literature-reader"],
    )];
    let p = parent();

    // 1. assignee だけ: そのノードの分野 → default_role の既定が効く。
    let mut t = dt("survey", vec![]);
    t.assignee = Some("research-survey".into());
    let out = materialize_delegated(
        &p,
        &[t],
        &[0],
        &org,
        &roles,
        &genres,
        WorkspaceContext::default(),
        now,
    );
    assert_eq!(out[0].assignee, None);
    assert_eq!(
        out[0]
            .routing
            .as_ref()
            .and_then(|r| r.dropped_assignee.as_deref()),
        Some("research-survey")
    );
    assert_eq!(
        out[0].genre, p.genre,
        "the dropped assignee's genre is not used"
    );
    assert_eq!(
        out[0].routing.as_ref().map(|r| r.tier_source),
        Some(crate::model::TierSource::Default)
    );

    // 2. assignee + role: tier / adapter / 予算は role が勝ち、assignee は担当としてだけ残る。
    let mut t = dt("survey", vec![]);
    t.assignee = Some("research-survey".into());
    t.role = Some("writer".into());
    let out = materialize_delegated(
        &p,
        &[t],
        &[0],
        &org,
        &roles,
        &genres,
        WorkspaceContext::default(),
        now,
    );
    assert_eq!(out[0].assignee, None);
    assert_eq!(out[0].worker_hint.adapter.as_deref(), Some("claude-code"));
    assert_eq!(out[0].worker_hint.tier, Tier::Standard);

    // 3. 分野を持たないノード・組織に無い id は既定を変えない（知らない id は担当にもしない）。
    let mut t = dt("tidy", vec![]);
    t.assignee = Some("research-data".into());
    let out = materialize_delegated(
        &p,
        &[t],
        &[0],
        &org,
        &roles,
        &genres,
        WorkspaceContext::default(),
        now,
    );
    assert_eq!(out[0].assignee, None);
    assert_eq!(out[0].genre, p.genre);
    let mut t = dt("ghost", vec![]);
    t.assignee = Some("nobody".into());
    let out = materialize_delegated(
        &p,
        &[t],
        &[0],
        &org,
        &roles,
        &genres,
        WorkspaceContext::default(),
        now,
    );
    assert_eq!(out[0].assignee, None);
}

/// ADR-0039 D2: 委譲した子の作業場所も **明示 > 案件 > 親** の 3 段。案件が Remote なら子も Remote。
#[test]
fn delegated_child_workspace_is_explicit_then_project_then_parent() {
    let p = parent();
    let now = OffsetDateTime::now_utc();
    let project = WorkspaceSpec::Local {
        path: PathBuf::from("/home/rmaeda/workspace/rust/pluvio-poc"),
        mode: None,
    };
    let explicit = WorkspaceSpec::Remote {
        cluster: "pegasus".into(),
        path: PathBuf::from("/work/NBB/rmaeda/workspace/rust/benchfs"),
        mode: None,
    };

    // 1. 案件も明示も無い → 親を継ぐ（従来）。
    let out = materialize_delegated(
        &p,
        &[dt("a", vec![])],
        &[0],
        &[],
        &[],
        &[],
        WorkspaceContext::default(),
        now,
    );
    assert_eq!(out[0].workspace, p.workspace);

    // 2. 案件の作業場所 > 親。
    let ws = WorkspaceContext {
        repos: &[],
        project: Some(&project),
        home: None,
    };
    let out = materialize_delegated(&p, &[dt("a", vec![])], &[0], &[], &[], &[], ws, now);
    assert_eq!(out[0].workspace, project);

    // 3. 明示 > 案件（別のクラスタで検証させたいとき）。
    let mut t = dt("b", vec![]);
    t.workspace = Some(explicit.clone());
    let out = materialize_delegated(&p, &[t], &[0], &[], &[], &[], ws, now);
    assert_eq!(out[0].workspace, explicit);

    // 4. 案件が Remote なら子も Remote（ADR-0018 の写し + `.taskd/remote-exec` 経路に乗る）。
    let ws = WorkspaceContext {
        repos: &[],
        project: Some(&explicit),
        home: None,
    };
    let out = materialize_delegated(&p, &[dt("c", vec![])], &[0], &[], &[], &[], ws, now);
    assert_eq!(out[0].workspace, explicit);
}

fn org_node_with_tools(id: &str, tools: &[&str]) -> OrgNode {
    let now = OffsetDateTime::now_utc();
    OrgNode {
        id: id.into(),
        parent_id: Some("cos".into()),
        name: id.into(),
        kind: crate::org::OrgKind::Department,
        genre: None,
        brief: String::new(),
        profile: crate::profile::Profile {
            tools: tools.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        },
        position: 0,
        created_at: now,
        updated_at: now,
    }
}

/// ADR-0062 B2（Phase 107）→ ADR-0069 D1（Phase 114）: 委譲が書いた担当は捨てるので、案件から継いだ
/// Remote workspace は担当未定のまま Remote に残る（matching が `cluster:<id>` を持つノードだけを
/// 候補にする）。明示した子（`t.workspace = Some(..)`）も従来どおり落とさない。
#[test]
fn inherited_remote_workspace_stays_remote_because_the_delegated_assignee_is_dropped() {
    let p = parent();
    let now = OffsetDateTime::now_utc();
    let project = WorkspaceSpec::Remote {
        cluster: "sirius".into(),
        path: PathBuf::from("/work/NBB/rmaeda/workspace/rust/benchfs"),
        mode: None,
    };
    let ws = WorkspaceContext {
        repos: &[],
        project: Some(&project),
        home: None,
    };
    let org = vec![
        org_node_with_tools("web-research", &["tavily", "exa"]),
        org_node_with_tools("cluster-hpc", &["cluster:sirius"]),
    ];

    // 以前は Local(<task_id>) に落ちた。今は担当を捨てるので Remote のまま、reason も出ない。
    let mut without_tool = dt("survey", vec![]);
    without_tool.assignee = Some("web-research".into());
    let mut reasons: Vec<(TaskId, String)> = Vec::new();
    let out = materialize_delegated_logging(
        &p,
        &[without_tool],
        &[0],
        &org,
        &[],
        &[],
        ws,
        now,
        &mut |id, reason| reasons.push((id, reason.to_string())),
    );
    assert_eq!(out[0].workspace, project);
    assert!(reasons.is_empty(), "{reasons:?}");

    // 担当が cluster:sirius を持つ → Remote のまま、reason は出ない。
    let mut with_tool = dt("compute", vec![]);
    with_tool.assignee = Some("cluster-hpc".into());
    let mut reasons2: Vec<(TaskId, String)> = Vec::new();
    let out2 = materialize_delegated_logging(
        &p,
        &[with_tool],
        &[0],
        &org,
        &[],
        &[],
        ws,
        now,
        &mut |id, reason| reasons2.push((id, reason.to_string())),
    );
    assert_eq!(out2[0].workspace, project);
    assert!(reasons2.is_empty());

    // 明示した子は落とさない（担当が道具を持たなくても、明示は明示のまま。検証は別経路で拒否する）。
    let mut explicit_child = dt("explicit", vec![]);
    explicit_child.assignee = Some("web-research".into());
    explicit_child.workspace = Some(project.clone());
    let out3 = materialize_delegated(&p, &[explicit_child], &[0], &org, &[], &[], ws, now);
    assert_eq!(out3[0].workspace, project);
}

/// ADR-0062 B2 の降格そのもの（担当が決まっている経路のための関数。ADR-0069 以降、計画・委譲は
/// 担当を渡さないのでここには `None` が来るが、規則自体は保つ）。
#[test]
fn downgrade_rule_still_applies_when_an_assignee_is_known() {
    let project = WorkspaceSpec::Remote {
        cluster: "sirius".into(),
        path: PathBuf::from("/work/x"),
        mode: None,
    };
    let org = vec![
        org_node_with_tools("web-research", &["tavily"]),
        org_node_with_tools("cluster-hpc", &["cluster:sirius"]),
    ];
    let id = TaskId::new();
    let (ws, reason) = downgrade_inherited_remote_if_needed(
        project.clone(),
        false,
        Some("web-research"),
        &org,
        id,
    );
    assert!(matches!(ws, WorkspaceSpec::Local { .. }));
    assert!(reason.is_some_and(|r| r.contains("cluster:sirius")));
    let (ws, reason) =
        downgrade_inherited_remote_if_needed(project.clone(), false, Some("cluster-hpc"), &org, id);
    assert_eq!(ws, project);
    assert!(reason.is_none());
    let (ws, _) = downgrade_inherited_remote_if_needed(project.clone(), false, None, &org, id);
    assert_eq!(ws, project);
}

/// ADR-0039 D5: `~` は `$HOME` で展開する。`Remote` の `~` はクラスタ側の home なので触らない。
#[test]
fn tilde_is_expanded_for_local_workspaces_only() {
    let home = PathBuf::from("/home/rmaeda");
    let local = WorkspaceSpec::Local {
        path: PathBuf::from("~/workspace/rust/pluvio-poc"),
        mode: None,
    };
    assert_eq!(
        local.with_home_expanded(Some(&home)),
        WorkspaceSpec::Local {
            path: PathBuf::from("/home/rmaeda/workspace/rust/pluvio-poc"),
            mode: None
        }
    );
    assert_eq!(
        local.with_home_expanded(None),
        local,
        "$HOME が無ければそのまま"
    );
    let remote = WorkspaceSpec::Remote {
        cluster: "pegasus".into(),
        path: PathBuf::from("~/workspace/rust/benchfs"),
        mode: None,
    };
    assert_eq!(remote.with_home_expanded(Some(&home)), remote);
    let absolute = WorkspaceSpec::Local {
        path: PathBuf::from("/tmp/ws"),
        mode: None,
    };
    assert_eq!(absolute.with_home_expanded(Some(&home)), absolute);
    // `~user` は展開しない（その home を知らない）。
    let other = WorkspaceSpec::Local {
        path: PathBuf::from("~someone/ws"),
        mode: None,
    };
    assert_eq!(other.with_home_expanded(Some(&home)), other);
    assert_eq!(
        crate::model::expand_home(std::path::Path::new("~"), Some(&home)),
        home
    );
}
