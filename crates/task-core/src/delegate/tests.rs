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
        harness_id: Some(crate::llm_route::HarnessId::from("fake")),
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
        },
        workspace: WorkspaceSpec::Local {
            path: PathBuf::from("/tmp/ws"),
            mode: None,
        },
        budget: Budget {
            max_turns: 10,
            max_wall_secs: 600,
            max_retries: 2,
            max_tokens: None,
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
        harness: Some("codex".into()),
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
    assert_eq!(out[0].harness_id.as_ref().map(AsRef::as_ref), Some("codex"));
    assert_eq!(out[0].budget.max_turns, 3);
    assert_eq!(out[0].budget.max_wall_secs, 600);
    assert_eq!(out[0].priority, 3);
    assert!(!out[0].aggregate);
    // 分野の設定が無くても、role/genre 共に未指定な子は親の分野をそのまま継ぐ。
    assert_eq!(out[0].genre.as_deref(), Some("coding"));
    // b は a（採用）だけに依存し、c（不採用）への依存は落ちる。役割無しは親の tier / adapter。
    assert_eq!(out[1].depends_on, vec![out[0].id]);
    assert_eq!(out[1].worker_hint.tier, Tier::Frontier);
    assert_eq!(out[1].harness_id.as_ref().map(AsRef::as_ref), Some("fake"));
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
        harness: Some("acp".into()),
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
    assert_eq!(out[0].harness_id.as_ref().map(AsRef::as_ref), Some("acp"));
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
            harness: Some("codex".into()),
            max_turns: None,
            max_wall_secs: None,
            instructions: None,
        },
        RoleSpec {
            id: "lead".into(),
            tier: Some(Tier::Cheap),
            harness: None,
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
    assert_eq!(out[0].harness_id.as_ref().map(AsRef::as_ref), Some("codex"));
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
            harness: Some("paperqa".into()),
            max_turns: Some(5),
            ..RoleSpec::default()
        },
        RoleSpec {
            id: "writer".into(),
            tier: Some(Tier::Standard),
            harness: Some("claude-code".into()),
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
    assert_eq!(
        out[0].harness_id.as_ref().map(AsRef::as_ref),
        Some("claude-code")
    );
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

fn org_node(id: &str, parent: Option<&str>, allowed: &[&str], tools: &[&str]) -> OrgNode {
    let now = OffsetDateTime::now_utc();
    OrgNode {
        id: id.into(),
        parent_id: parent.map(str::to_string),
        name: id.into(),
        kind: if parent.is_none() {
            crate::org::OrgKind::Secretary
        } else {
            crate::org::OrgKind::Department
        },
        genre: None,
        brief: String::new(),
        profile: crate::profile::Profile {
            tools: tools.iter().map(|s| s.to_string()).collect(),
            harnesses: crate::profile::HarnessPrefs {
                allowed: allowed.iter().map(|s| s.to_string()).collect(),
                default: None,
            },
            ..Default::default()
        },
        position: 0,
        created_at: now,
        updated_at: now,
    }
}

fn sirius_project() -> WorkspaceSpec {
    WorkspaceSpec::Remote {
        cluster: "sirius".into(),
        path: PathBuf::from("/work/NBB/rmaeda/workspace/rust/benchfs"),
        mode: None,
    }
}

/// 実機の組織の形: web-research は cluster 道具を持たず、cluster:sirius を持つのは coding の課だけ。
fn org_without_a_sirius_web_researcher() -> Vec<OrgNode> {
    vec![
        org_node("cos", None, &["conversation", "plan"], &[]),
        org_node("web-research", Some("cos"), &["web-research"], &["tavily"]),
        org_node("cluster-hpc", Some("cos"), &["coding"], &["cluster:sirius"]),
    ]
}

fn delegate_one(child: DelegateTask, org: &[OrgNode]) -> (Task, Vec<(TaskId, String)>) {
    let p = parent();
    let project = sirius_project();
    let ws = WorkspaceContext {
        repos: &[],
        project: Some(&project),
        home: None,
    };
    let mut reasons: Vec<(TaskId, String)> = Vec::new();
    let mut out = materialize_delegated_logging(
        &p,
        &[child],
        &[0],
        org,
        &[],
        &[],
        ws,
        OffsetDateTime::now_utc(),
        &mut |id, reason| reasons.push((id, reason.to_string())),
    );
    (out.remove(0), reasons)
}

/// ADR 2026-10-27-inherited-remote-unassigned-child (a): 案件の Remote(sirius) を継いだ担当未定の
/// web-research の子は、web-research を受けて cluster:sirius も持つノードが無いので Local(<id>) に落ちる。
/// 理由は on_downgrade と `routing.workspace_downgrade`（`Event::Created` に載る）に残る。
#[test]
fn inherited_remote_goes_local_when_no_cluster_node_takes_the_harness() {
    let org = org_without_a_sirius_web_researcher();
    let mut survey = dt("survey", vec![]);
    survey.genre = Some("web-research".into());
    // 委譲が書いた担当は捨てる（ADR-0069 D1）ので、ここは担当未定の経路。
    survey.assignee = Some("cluster-hpc".into());
    let (child, reasons) = delegate_one(survey, &org);
    assert!(child.assignee.is_none());
    assert_eq!(
        child.workspace,
        WorkspaceSpec::Local {
            path: PathBuf::from(child.id.to_string()),
            mode: None,
        }
    );
    assert_eq!(reasons.len(), 1, "{reasons:?}");
    assert_eq!(reasons[0].0, child.id);
    assert!(
        reasons[0].1.contains("web-research") && reasons[0].1.contains("cluster:sirius"),
        "{reasons:?}"
    );
    assert_eq!(
        child
            .routing
            .as_ref()
            .and_then(|r| r.workspace_downgrade.as_deref()),
        Some(reasons[0].1.as_str())
    );
}

/// (b): web-research を受けて cluster:sirius も持つノードがあれば Remote のまま（matching がそこへ流す）。
#[test]
fn inherited_remote_stays_when_a_cluster_node_takes_the_harness() {
    let mut org = org_without_a_sirius_web_researcher();
    org.push(org_node(
        "sirius-research",
        Some("cos"),
        &["web-research"],
        &["cluster:sirius"],
    ));
    let mut survey = dt("survey", vec![]);
    survey.genre = Some("web-research".into());
    let (child, reasons) = delegate_one(survey, &org);
    assert_eq!(child.workspace, sirius_project());
    assert!(reasons.is_empty(), "{reasons:?}");
    assert!(
        child
            .routing
            .as_ref()
            .is_none_or(|r| r.workspace_downgrade.is_none())
    );
    // 親の分野（coding）を継ぐ子も、coding を受けて cluster:sirius を持つ課があるので Remote のまま。
    let (coding, reasons) = delegate_one(dt("build", vec![]), &org);
    assert_eq!(coding.genre.as_deref(), Some("coding"));
    assert_eq!(coding.workspace, sirius_project());
    assert!(reasons.is_empty(), "{reasons:?}");
}

/// (c): 明示した Remote は、受けられるノードが無くても落とさない（明示は明示のまま。matching が人に聞く）。
#[test]
fn explicit_remote_is_never_downgraded() {
    let org = org_without_a_sirius_web_researcher();
    let mut survey = dt("survey", vec![]);
    survey.genre = Some("web-research".into());
    survey.workspace = Some(sirius_project());
    let (child, reasons) = delegate_one(survey, &org);
    assert_eq!(child.workspace, sirius_project());
    assert!(reasons.is_empty(), "{reasons:?}");
}

/// matching が走らない子（組み込みハーネス）と組織が空の構成は、判定せず Remote のまま。
#[test]
fn inherited_remote_stays_when_matching_does_not_run() {
    let org = org_without_a_sirius_web_researcher();
    let mut review = dt("review", vec![]);
    review.genre = Some(crate::harness::BUILTIN_REVIEWER.into());
    let (child, reasons) = delegate_one(review, &org);
    assert_eq!(child.workspace, sirius_project());
    assert!(reasons.is_empty(), "{reasons:?}");
    let mut survey = dt("survey", vec![]);
    survey.genre = Some("web-research".into());
    let (child, reasons) = delegate_one(survey, &[]);
    assert_eq!(child.workspace, sirius_project());
    assert!(reasons.is_empty(), "{reasons:?}");
}

/// ADR-0062 B2 の担当が決まっている経路（POST /tasks の明示の担当など）: 担当が cluster:<id> を
/// 持たなければ落とし、持てば Remote のまま。
#[test]
fn downgrade_rule_still_applies_when_an_assignee_is_known() {
    let org = org_without_a_sirius_web_researcher();
    let mut t = parent();
    t.workspace = sirius_project();
    t.genre = Some("web-research".into());
    t.assignee = Some("web-research".into());
    let reason = downgrade_inherited_remote(&mut t, false, &org);
    assert!(matches!(t.workspace, WorkspaceSpec::Local { .. }));
    assert!(reason.is_some_and(|r| r.contains("assignee web-research has no cluster:sirius")));
    let mut t = parent();
    t.workspace = sirius_project();
    t.genre = Some("web-research".into());
    t.assignee = Some("cluster-hpc".into());
    assert!(downgrade_inherited_remote(&mut t, false, &org).is_none());
    assert_eq!(t.workspace, sirius_project());
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
