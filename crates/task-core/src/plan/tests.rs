use super::*;
use crate::model::{Check, WorkspaceSpec};
use std::path::PathBuf;

fn new_task(title: &str, deps: Vec<usize>) -> NewTask {
    NewTask {
        harness: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        title: title.into(),
        objective: format!("do {title}"),
        acceptance: vec![Criterion {
            text: "c".into(),
            check: Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
        }],
        depends_on: deps,
        kind: NewTaskKind::Execute,
        tier: None,
        role: None,
        genre: None,
        assignee: None,
        workspace: None,
        category: None,
        labels: Vec::new(),
        partial_ok: None,
    }
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

fn parent() -> Task {
    let now = OffsetDateTime::now_utc();
    Task {
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Plan,
        title: "plan".into(),
        objective: "goal".into(),
        acceptance: vec![],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Reviewing,
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
            max_turns: 30,
            max_wall_secs: 900,
            max_retries: 1,
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

#[test]
fn valid_plan_passes_and_materializes_children_with_inherited_fields() {
    let plan = PlanOutput {
        tasks: vec![
            new_task("a", vec![]),
            new_task("b", vec![0]),
            new_task("c", vec![0, 1]),
        ],
    };
    validate(&plan, 1, &PlanLimits::default(), &[], &[]).unwrap();
    let p = parent();
    let children = materialize(
        &p,
        &plan,
        &[],
        &[],
        &[],
        WorkspaceContext::default(),
        OffsetDateTime::now_utc(),
    );
    assert_eq!(children.len(), 3);
    for c in &children {
        assert_eq!(c.parent_id, Some(p.id));
        assert_eq!(c.status, Status::Draft);
        assert_eq!(c.priority, 3);
        assert_eq!(c.workspace, p.workspace);
        assert_eq!(c.budget, p.budget);
        assert_eq!(c.worker_hint.adapter.as_deref(), Some("fake"));
        // ADR-0028 D3: tier に既定が無ければ（役割・分野・タスクいずれも無指定）親の tier を継ぐ
        // （委譲と同じ規則。以前は独立した既定 `Standard` だった）。
        assert_eq!(c.worker_hint.tier, Tier::Frontier);
        assert_eq!(c.kind, TaskKind::Execute);
    }
    assert_eq!(children[1].depends_on, vec![children[0].id]);
    assert_eq!(children[2].depends_on, vec![children[0].id, children[1].id]);
}

/// ADR-0033 D2（監査 D-3。GUI 監査対応 Phase 29 で確認）: 分解した子は親の `project_id` /
/// `milestone_id` を必ず継ぐ（案件の仕事の木から子が消えないように）。偽プランナーの出力からでも同じ。
#[test]
fn materialize_carries_the_parents_project_and_milestone_id() {
    let mut p = parent();
    p.project_id = Some(crate::org::ProjectId::new());
    p.milestone_id = Some(crate::org::MilestoneId::new());
    let plan = PlanOutput {
        tasks: vec![new_task("a", vec![]), new_task("b", vec![])],
    };
    let children = materialize(
        &p,
        &plan,
        &[],
        &[],
        &[],
        WorkspaceContext::default(),
        OffsetDateTime::now_utc(),
    );
    for c in &children {
        assert_eq!(
            c.project_id, p.project_id,
            "child must stay in the parent's project"
        );
        assert_eq!(c.milestone_id, p.milestone_id);
    }

    // 案件が無い Plan（従来どおり）では子にも付かない。
    let none = parent();
    let children = materialize(
        &none,
        &plan,
        &[],
        &[],
        &[],
        WorkspaceContext::default(),
        OffsetDateTime::now_utc(),
    );
    assert!(
        children
            .iter()
            .all(|c| c.project_id.is_none() && c.milestone_id.is_none())
    );
}

#[test]
fn rejects_count_empty_fields_and_missing_acceptance() {
    let limits = PlanLimits {
        min_tasks: 2,
        max_tasks: 3,
    };
    let one = PlanOutput {
        tasks: vec![new_task("a", vec![])],
    };
    assert!(matches!(
        validate(&one, 1, &limits, &[], &[]),
        Err(PlanError::TaskCount {
            actual: 1,
            min: 2,
            max: 3
        })
    ));
    let mut empty_title = PlanOutput {
        tasks: vec![new_task("a", vec![]), new_task("b", vec![])],
    };
    empty_title.tasks[1].title = "  ".into();
    assert!(matches!(
        validate(&empty_title, 1, &limits, &[], &[]),
        Err(PlanError::EmptyField {
            index: 1,
            field: "title"
        })
    ));
    let mut no_acc = PlanOutput {
        tasks: vec![new_task("a", vec![]), new_task("b", vec![])],
    };
    no_acc.tasks[0].acceptance.clear();
    assert!(matches!(
        validate(&no_acc, 1, &limits, &[], &[]),
        Err(PlanError::NoAcceptance { index: 0 })
    ));
}

#[test]
fn rejects_bad_dependencies_and_cycles() {
    let oor = PlanOutput {
        tasks: vec![new_task("a", vec![7])],
    };
    let err = validate(&oor, 1, &PlanLimits::default(), &[], &[]).unwrap_err();
    assert!(matches!(
        err,
        PlanError::DependencyOutOfRange {
            index: 0,
            target: 7,
            ..
        }
    ));
    assert!(err.to_string().contains("out of range"));

    let self_dep = PlanOutput {
        tasks: vec![new_task("a", vec![0])],
    };
    assert!(matches!(
        validate(&self_dep, 1, &PlanLimits::default(), &[], &[]),
        Err(PlanError::SelfDependency { index: 0 })
    ));

    let cycle = PlanOutput {
        tasks: vec![
            new_task("a", vec![2]),
            new_task("b", vec![0]),
            new_task("c", vec![1]),
        ],
    };
    assert!(matches!(
        validate(&cycle, 1, &PlanLimits::default(), &[], &[]),
        Err(PlanError::Cycle { .. })
    ));

    let diamond = PlanOutput {
        tasks: vec![
            new_task("a", vec![]),
            new_task("b", vec![0]),
            new_task("c", vec![0]),
            new_task("d", vec![1, 2]),
        ],
    };
    validate(&diamond, 1, &PlanLimits::default(), &[], &[]).unwrap();
}

#[test]
fn nested_plan_respects_depth_limit() {
    let mut nested = PlanOutput {
        tasks: vec![new_task("sub", vec![])],
    };
    nested.tasks[0].kind = NewTaskKind::Plan;
    validate(&nested, 1, &PlanLimits::default(), &[], &[]).unwrap();
    validate(&nested, 2, &PlanLimits::default(), &[], &[]).unwrap();
    assert!(matches!(
        validate(&nested, 3, &PlanLimits::default(), &[], &[]),
        Err(PlanError::DepthExceeded {
            index: 0,
            depth: 4,
            max: 3
        })
    ));
    let p = parent();
    let children = materialize(
        &p,
        &nested,
        &[],
        &[],
        &[],
        WorkspaceContext::default(),
        OffsetDateTime::now_utc(),
    );
    assert_eq!(children[0].kind, TaskKind::Plan);
}

#[test]
fn parse_rejects_unknown_fields_and_reports_serde_errors() {
    let ok = r#"{"tasks":[{"title":"t","objective":"o","acceptance":[{"text":"c","check":{"type":"reviewer"}}]}]}"#;
    let plan = parse_and_validate(ok, 1, &PlanLimits::default(), &[], &[]).unwrap();
    assert_eq!(plan.tasks[0].acceptance[0].check, Check::Reviewer);
    assert_eq!(plan.tasks[0].kind, NewTaskKind::Execute);
    let unknown = r#"{"tasks":[{"title":"t","objective":"o","acceptance":[{"text":"c","check":{"type":"human"}}],"bogus":1}]}"#;
    let err = parse_and_validate(unknown, 1, &PlanLimits::default(), &[], &[]).unwrap_err();
    assert!(err.contains("bogus"), "{err}");
    assert!(parse_and_validate("not json", 1, &PlanLimits::default(), &[], &[]).is_err());
}

/// ADR-0028 D3: 子の分野は 明示 > `role` の分野（一意なら） > 親の分野の順で決まる（委譲と同じ規則）。
#[test]
fn materialize_resolves_genre_by_precedence() {
    let mut p = parent();
    p.genre = Some("coding".into());
    let genres = vec![
        genre("coding", Some("implementer"), &["lead", "implementer"]),
        genre(
            "literature",
            Some("literature-reader"),
            &["literature-scout", "literature-reader"],
        ),
    ];

    // 1. 明示した genre が最優先（`materialize` は検証済みの plan だけを渡す前提で、ここでは
    // role/genre の整合そのものは見ない）。
    let mut explicit = new_task("explicit", vec![]);
    explicit.role = Some("implementer".into());
    explicit.genre = Some("literature".into());
    let plan = PlanOutput {
        tasks: vec![explicit],
    };
    let children = materialize(
        &p,
        &plan,
        &[],
        &[],
        &genres,
        WorkspaceContext::default(),
        OffsetDateTime::now_utc(),
    );
    assert_eq!(children[0].genre.as_deref(), Some("literature"));

    // 2. genre 未指定・role が一意に決まる分野に属する。
    let mut by_role = new_task("by-role", vec![]);
    by_role.role = Some("literature-scout".into());
    let plan = PlanOutput {
        tasks: vec![by_role],
    };
    let children = materialize(
        &p,
        &plan,
        &[],
        &[],
        &genres,
        WorkspaceContext::default(),
        OffsetDateTime::now_utc(),
    );
    assert_eq!(children[0].genre.as_deref(), Some("literature"));

    // 3. genre も role も無ければ親の分野を継ぐ。
    let plan = PlanOutput {
        tasks: vec![new_task("neither", vec![])],
    };
    let children = materialize(
        &p,
        &plan,
        &[],
        &[],
        &genres,
        WorkspaceContext::default(),
        OffsetDateTime::now_utc(),
    );
    assert_eq!(children[0].genre.as_deref(), Some("coding"));
}

/// ADR-0028 D3: `tier` / `adapter` / `budget` は タスクの値 > 役割の既定 > 分野の既定
/// （`default_role` の役割）> 親の値、の順（委譲と同じ規則）。
#[test]
fn materialize_applies_role_and_genre_defaults_like_delegation() {
    let p = parent(); // tier = Frontier, adapter = Some("fake")
    let roles = vec![RoleSpec {
        id: "implementer".into(),
        tier: None,
        adapter: Some("codex".into()),
        max_turns: None,
        max_wall_secs: None,
        instructions: None,
    }];
    let genres = vec![genre("coding", Some("lead"), &["lead", "implementer"])];
    let genre_default_role = vec![RoleSpec {
        id: "lead".into(),
        tier: Some(Tier::Cheap),
        adapter: None,
        max_turns: Some(20),
        max_wall_secs: None,
        instructions: None,
    }];
    let mut all_roles = roles;
    all_roles.extend(genre_default_role);
    let mut t = new_task("impl", vec![]);
    t.role = Some("implementer".into());
    t.genre = Some("coding".into());
    let plan = PlanOutput { tasks: vec![t] };
    let children = materialize(
        &p,
        &plan,
        &[],
        &all_roles,
        &genres,
        WorkspaceContext::default(),
        OffsetDateTime::now_utc(),
    );
    // adapter: role(implementer) の既定が優先。
    assert_eq!(children[0].worker_hint.adapter.as_deref(), Some("codex"));
    // tier: role に既定が無いので分野の既定役割（lead）から。
    assert_eq!(children[0].worker_hint.tier, Tier::Cheap);
    assert_eq!(children[0].budget.max_turns, 20);
    assert_eq!(children[0].budget.max_retries, p.budget.max_retries);
}

/// ADR-0033 D4（Phase 24）: 計画の `assignee` は子タスクに残り、`role` が無いときだけ
/// そのノードの分野が既定（tier / adapter / 予算）の解決に効く。
#[test]
fn materialize_drops_the_plan_supplied_assignee_and_records_it() {
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
        node("research-writing", None),
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
    let p = parent();

    let mut only_assignee = new_task("調べる", vec![]);
    only_assignee.assignee = Some("research-survey".into());
    let mut with_role = new_task("書く", vec![]);
    with_role.assignee = Some("research-writing".into());
    with_role.role = Some("writer".into());
    let mut unknown = new_task("誰？", vec![]);
    unknown.assignee = Some("nobody".into());
    let plan = PlanOutput {
        tasks: vec![only_assignee, with_role, unknown],
    };
    let children = materialize(
        &p,
        &plan,
        &org,
        &roles,
        &genres,
        WorkspaceContext::default(),
        now,
    );

    // ADR-0069 D1（Phase 114）: 計画（LLM）が書いた担当は捨て、`routing.dropped_assignee` に残す
    // （以前は組織にある id なら子に記録し、その分野を既定に使っていた）。担当の分野も使わない。
    let dropped = |i: usize| {
        children[i]
            .routing
            .as_ref()
            .and_then(|r| r.dropped_assignee.clone())
    };
    assert_eq!(children[0].assignee, None);
    assert_eq!(dropped(0).as_deref(), Some("research-survey"));
    assert_eq!(
        children[0].genre, p.genre,
        "the dropped assignee's genre is not used"
    );

    assert_eq!(children[1].assignee, None);
    assert_eq!(dropped(1).as_deref(), Some("research-writing"));
    assert_eq!(
        children[1].worker_hint.adapter.as_deref(),
        Some("claude-code")
    );
    assert_eq!(children[1].worker_hint.tier, Tier::Frontier);
    assert_eq!(
        children[1].routing.as_ref().map(|r| r.tier_source),
        Some(crate::model::TierSource::Default)
    );

    assert_eq!(children[2].assignee, None);
    assert_eq!(dropped(2).as_deref(), Some("nobody"));
    assert_eq!(children[2].genre, p.genre);
}

/// Phase 38（ADR-0028 追記）テスト用: ハーネス系（`paperqa`）の `literature` と、ハーネスでない
/// `coding`、それぞれの担当ノードを持つ組織・役割・分野。
fn harness_setup() -> (Vec<crate::org::OrgNode>, Vec<RoleSpec>, Vec<GenreSpec>) {
    use crate::org::{OrgKind, OrgNode};
    let now = OffsetDateTime::now_utc();
    let node = |id: &str, genre_id: &str| OrgNode {
        profile: Default::default(),
        id: id.into(),
        parent_id: Some("research".into()),
        name: id.into(),
        kind: OrgKind::Section,
        genre: Some(genre_id.to_string()),
        brief: String::new(),
        position: 0,
        created_at: now,
        updated_at: now,
    };
    let org = vec![
        node("research-literature", "literature"),
        node("coding-poc", "coding"),
    ];
    let roles = vec![
        RoleSpec {
            id: "literature-reader".into(),
            adapter: Some("paperqa".into()),
            ..RoleSpec::default()
        },
        RoleSpec {
            id: "implementer".into(),
            adapter: Some("claude-code".into()),
            ..RoleSpec::default()
        },
    ];
    let genres = vec![
        GenreSpec {
            output_artifacts: vec![
                "answer.md: 引用付きの答え".into(),
                "papers.json: 検索した論文の一覧（コーパス）".into(),
                "sources.json".into(),
            ],
            ..genre(
                "literature",
                Some("literature-reader"),
                &["literature-reader"],
            )
        },
        GenreSpec {
            output_artifacts: vec!["diff".into()],
            ..genre("coding", Some("implementer"), &["implementer"])
        },
    ];
    (org, roles, genres)
}

/// Phase 38（ADR-0028 追記。実機のレビュー不合格から）: ハーネス系の担当に「`candidates.json` に
/// まとめよ」と要求した `artifact_exists` は落ち、`objective` に本当の成果物の名前が注記される。
/// 一致している条件（`answer.md`）はそのまま残る。
#[test]
fn fix_harness_artifacts_drops_unknown_artifact_checks_and_notes_the_real_ones() {
    let (org, roles, genres) = harness_setup();
    let mut t = new_task("候補テーマの抽出", vec![]);
    t.genre = Some("literature".into()); // ADR-0069: 担当ではなく harness で指定する
    t.objective = "候補テーマを 3〜5 件、引用付きで candidates.json にまとめよ".into();
    t.acceptance = vec![
        Criterion {
            text: "candidates.json がある".into(),
            check: Check::ArtifactExists {
                name: "candidates.json".into(),
            },
        },
        Criterion {
            text: "answer.md がある".into(),
            check: Check::ArtifactExists {
                name: "answer.md".into(),
            },
        },
    ];
    let mut plan = PlanOutput { tasks: vec![t] };
    let warnings = fix_harness_artifacts(&mut plan, &parent(), &org, &roles, &genres);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("candidates.json"), "{}", warnings[0]);
    assert!(
        warnings[0].contains("answer.md / papers.json / sources.json"),
        "{}",
        warnings[0]
    );
    let fixed = &plan.tasks[0];
    assert_eq!(
        fixed.acceptance,
        vec![Criterion {
            text: "answer.md がある".into(),
            check: Check::ArtifactExists {
                name: "answer.md".into()
            }
        }],
        "名前が一致する条件だけが残る"
    );
    assert!(
            fixed.objective.ends_with(
                "（注: この担当の成果物は answer.md / papers.json / sources.json に固定。要求した内容は answer.md の中で述べる）"
            ),
            "{}",
            fixed.objective
        );
    // 直した plan はそのまま検証を通り、子にできる（壊さず直す）。
    validate(&plan, 1, &PlanLimits::default(), &genres, &[]).unwrap();
}

/// Phase 38: 落とすと受け入れ条件が 0 件になるときは、同じ文をレビュアー条件にして残す
/// （内容はレビュアーが `answer.md` の中で判定する）。
#[test]
fn fix_harness_artifacts_keeps_the_criterion_as_a_reviewer_check_when_nothing_else_remains() {
    let (org, roles, genres) = harness_setup();
    let mut t = new_task("候補テーマの抽出", vec![]);
    t.genre = Some("literature".into()); // ADR-0069: 担当ではなく harness で指定する
    t.acceptance = vec![Criterion {
        text: "候補テーマ 3 件が引用付きで書かれている".into(),
        check: Check::ArtifactExists {
            name: "candidates.json".into(),
        },
    }];
    let mut plan = PlanOutput { tasks: vec![t] };
    let warnings = fix_harness_artifacts(&mut plan, &parent(), &org, &roles, &genres);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(
        plan.tasks[0].acceptance,
        vec![Criterion {
            text: "候補テーマ 3 件が引用付きで書かれている".into(),
            check: Check::Reviewer
        }]
    );
    validate(&plan, 1, &PlanLimits::default(), &genres, &[]).unwrap();
}

/// Phase 38: ハーネスでない分野（coding = claude-code）と、名前が一致しているハーネスのタスクには
/// 触らない（`objective` も `acceptance` も 1 バイトも変わらない）。
#[test]
fn fix_harness_artifacts_leaves_other_genres_and_matching_names_alone() {
    let (org, roles, genres) = harness_setup();
    let mut coding = new_task("実装", vec![]);
    coding.genre = Some("coding".into()); // ADR-0069: 担当ではなく harness で指定する
    coding.acceptance = vec![Criterion {
        text: "design.md がある".into(),
        check: Check::ArtifactExists {
            name: "design.md".into(),
        },
    }];
    let mut literature = new_task("調べる", vec![]);
    literature.genre = Some("literature".into()); // ADR-0069: 担当ではなく harness で指定する
    literature.acceptance = vec![Criterion {
        text: "answer.md がある".into(),
        check: Check::ArtifactExists {
            name: "answer.md".into(),
        },
    }];
    let mut plan = PlanOutput {
        tasks: vec![coding, literature],
    };
    let before = plan.clone();
    let warnings = fix_harness_artifacts(&mut plan, &parent(), &org, &roles, &genres);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(plan, before);
}

/// ADR-0063 D3（Phase 109）: 調査系（literature/web-research）の子で、受け入れ条件に
/// 「未確認」等の一文も `partial_ok: true` も無ければ**警告**（拒否はしない。plan は変更されない）。
#[test]
fn warn_missing_partial_ok_flags_research_children_without_the_escape_hatch() {
    let (org, roles, genres) = harness_setup();
    let mut t = new_task("CHFS の関連研究", vec![]);
    t.genre = Some("literature".into()); // ADR-0069: 担当ではなく harness で指定する
    t.acceptance = vec![Criterion {
        text: "CHFS/FinchFS/GekkoFS/UnifyFS の関連研究をまとめている".into(),
        check: Check::Reviewer,
    }];
    let plan = PlanOutput { tasks: vec![t] };
    let before = plan.clone();
    let warnings = warn_missing_partial_ok(&plan, &parent(), &org, &roles, &genres);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("literature"), "{}", warnings[0]);
    assert!(warnings[0].contains("未確認"), "{}", warnings[0]);
    // 拒否はしない: plan はそのまま。
    assert_eq!(plan, before);
}

/// 逃げ道（受け入れ条件の一文、または `partial_ok: true`）があれば警告しない。coding のような
/// 調査系でない分野は対象外。
#[test]
fn warn_missing_partial_ok_is_quiet_when_the_escape_hatch_is_present_or_the_genre_is_not_research()
{
    let (org, roles, genres) = harness_setup();

    let mut with_marker = new_task("CHFS の関連研究", vec![]);
    with_marker.genre = Some("literature".into()); // ADR-0069: 担当ではなく harness で指定する
    with_marker.acceptance = vec![Criterion {
        text: "一次情報で確認できなかった項目は「未確認」と明記されていれば不合格にしない".into(),
        check: Check::Reviewer,
    }];

    let mut with_flag = new_task("Web 調査", vec![]);
    with_flag.genre = Some("web-research".into());
    with_flag.partial_ok = Some(true);
    with_flag.acceptance = vec![Criterion {
        text: "出典付きで書く".into(),
        check: Check::Reviewer,
    }];

    let mut coding = new_task("実装", vec![]);
    coding.genre = Some("coding".into()); // ADR-0069: 担当ではなく harness で指定する
    coding.acceptance = vec![Criterion {
        text: "design.md がある".into(),
        check: Check::ArtifactExists {
            name: "design.md".into(),
        },
    }];

    let plan = PlanOutput {
        tasks: vec![with_marker, with_flag, coding],
    };
    let warnings = warn_missing_partial_ok(&plan, &parent(), &org, &roles, &genres);
    assert!(warnings.is_empty(), "{warnings:?}");
}

/// ADR-0028 D3: 知らない `genre`、または `genre` + `role` の不整合は Plan の失敗になる。
#[test]
fn validate_rejects_unknown_genre_and_role_not_in_genre() {
    let genres = vec![genre(
        "coding",
        Some("implementer"),
        &["lead", "implementer"],
    )];

    let mut unknown = new_task("a", vec![]);
    unknown.genre = Some("literature".into());
    let plan = PlanOutput {
        tasks: vec![unknown],
    };
    assert_eq!(
        validate(&plan, 1, &PlanLimits::default(), &genres, &[]),
        Err(PlanError::UnknownGenre {
            index: 0,
            genre: "literature".into()
        })
    );

    let mut mismatched = new_task("b", vec![]);
    mismatched.genre = Some("coding".into());
    mismatched.role = Some("literature-scout".into());
    let plan = PlanOutput {
        tasks: vec![mismatched],
    };
    assert_eq!(
        validate(&plan, 1, &PlanLimits::default(), &genres, &[]),
        Err(PlanError::RoleNotInGenre {
            index: 0,
            role: "literature-scout".into(),
            genre: "coding".into(),
        })
    );

    // 分野を使わない設定（`genres` が空）でも `genre` を指定すれば同じくエラー。
    let mut no_config = new_task("c", vec![]);
    no_config.genre = Some("coding".into());
    let plan = PlanOutput {
        tasks: vec![no_config],
    };
    assert_eq!(
        validate(&plan, 1, &PlanLimits::default(), &[], &[]),
        Err(PlanError::UnknownGenre {
            index: 0,
            genre: "coding".into()
        })
    );

    // parse_and_validate 経由でも同じ（Plan run の暗黙条件から見えるエラー文言）。
    let json = r#"{"tasks":[{"title":"t","objective":"o","acceptance":[{"text":"c","check":{"type":"human"}}],"genre":"literature"}]}"#;
    let err = parse_and_validate(json, 1, &PlanLimits::default(), &genres, &[]).unwrap_err();
    assert!(err.contains("literature"), "{err}");
}

/// ADR-0039 D2: 分解した子の作業場所は **明示 > 案件 > 親** の 3 段で決まる。
#[test]
fn child_workspace_is_explicit_then_project_then_parent() {
    let p = parent();
    let project = WorkspaceSpec::Local {
        path: PathBuf::from("/home/rmaeda/workspace/rust/pluvio-poc"),
        mode: None,
    };
    let explicit = WorkspaceSpec::Local {
        path: PathBuf::from("/home/rmaeda/workspace/rust/other"),
        mode: None,
    };

    // 案件も明示も無ければ従来どおり親を継ぐ。
    let plan = PlanOutput {
        tasks: vec![new_task("a", vec![])],
    };
    let children = materialize(
        &p,
        &plan,
        &[],
        &[],
        &[],
        WorkspaceContext::default(),
        OffsetDateTime::now_utc(),
    );
    assert_eq!(children[0].workspace, p.workspace);

    // 案件の作業場所は親より強い。
    let ws = WorkspaceContext {
        repos: &[],
        project: Some(&project),
        home: None,
    };
    let children = materialize(&p, &plan, &[], &[], &[], ws, OffsetDateTime::now_utc());
    assert_eq!(children[0].workspace, project);

    // タスクが明示すれば案件より強い。
    let mut explicit_task = new_task("b", vec![]);
    explicit_task.workspace = Some(explicit.clone());
    let plan = PlanOutput {
        tasks: vec![new_task("a", vec![]), explicit_task],
    };
    let children = materialize(&p, &plan, &[], &[], &[], ws, OffsetDateTime::now_utc());
    assert_eq!(children[0].workspace, project);
    assert_eq!(children[1].workspace, explicit);
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

/// ADR-0062 B2（Phase 107）→ ADR-0069 D1（Phase 114）: 計画が書いた担当は捨てるので、案件から継いだ
/// Remote workspace は**担当未定のまま Remote に残る**（ADR-0062 B2 の「担当未定なら判定しない」側。
/// matching が `cluster:<id>` を持つノードだけを候補にするので後で矛盾しない）。以前は計画の
/// `assignee` が道具を持たなければここで Local に落としていた。
#[test]
fn plan_child_inherited_remote_workspace_stays_remote_because_the_plan_assignee_is_dropped() {
    let p = parent();
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
    let mut without_tool = new_task("survey", vec![]);
    without_tool.assignee = Some("web-research".into());
    let plan = PlanOutput {
        tasks: vec![without_tool],
    };
    let mut reasons: Vec<(TaskId, String)> = Vec::new();
    let children = materialize_logging(
        &p,
        &plan,
        &org,
        &[],
        &[],
        ws,
        OffsetDateTime::now_utc(),
        &mut |id, reason| reasons.push((id, reason.to_string())),
    );
    assert_eq!(children[0].workspace, project);
    assert!(reasons.is_empty(), "{reasons:?}");
    assert_eq!(children[0].assignee, None);
    assert_eq!(
        children[0]
            .routing
            .as_ref()
            .and_then(|r| r.dropped_assignee.as_deref()),
        Some("web-research")
    );
}

/// ADR-0039 D2: 案件が Remote なら子も Remote（従来の ADR-0018 経路に乗る）。D5: `~` は展開する。
#[test]
fn a_remote_project_makes_remote_children_and_tilde_is_expanded() {
    let p = parent();
    let remote = WorkspaceSpec::Remote {
        cluster: "pegasus".into(),
        path: PathBuf::from("/work/NBB/rmaeda/workspace/rust/benchfs"),
        mode: None,
    };
    let plan = PlanOutput {
        tasks: vec![new_task("a", vec![])],
    };
    let home = PathBuf::from("/home/rmaeda");
    let ws = WorkspaceContext {
        repos: &[],
        project: Some(&remote),
        home: Some(&home),
    };
    let children = materialize(&p, &plan, &[], &[], &[], ws, OffsetDateTime::now_utc());
    assert_eq!(
        children[0].workspace, remote,
        "Remote の path はクラスタ側なので触らない"
    );

    let tilde = WorkspaceSpec::Local {
        path: PathBuf::from("~/workspace/rust/pluvio-poc"),
        mode: None,
    };
    let ws = WorkspaceContext {
        repos: &[],
        project: Some(&tilde),
        home: Some(&home),
    };
    let children = materialize(&p, &plan, &[], &[], &[], ws, OffsetDateTime::now_utc());
    assert_eq!(
        children[0].workspace,
        WorkspaceSpec::Local {
            path: PathBuf::from("/home/rmaeda/workspace/rust/pluvio-poc"),
            mode: None
        }
    );
}

fn project_repo(name: &str, primary: bool) -> crate::repos::ProjectRepo {
    crate::repos::ProjectRepo {
        id: crate::repos::RepoId::new(),
        project_id: crate::org::ProjectId::new(),
        name: name.into(),
        kind: crate::repos::RepoKind::Git,
        location: WorkspaceSpec::local(format!("/srv/{name}")),
        default_branch: None,
        sync: None,
        run: crate::repos::RepoRun::Auto,
        is_primary: primary,
        created_at: OffsetDateTime::now_utc(),
    }
}

/// ADR-0043 D2: 計画の `repos` は案件に登録されている名前でなければならない（知らない名前は差し戻し）。
#[test]
fn a_plan_can_only_name_repositories_the_project_has() {
    let known = vec!["benchfs".to_string(), "benchfs-paper".to_string()];
    let mut ok = new_task("a", vec![]);
    ok.repos = vec!["benchfs".into(), "benchfs-paper".into()];
    let plan = PlanOutput { tasks: vec![ok] };
    assert_eq!(
        validate(&plan, 1, &PlanLimits::default(), &[], &known),
        Ok(())
    );

    let mut bad = new_task("b", vec![]);
    bad.repos = vec!["benchfs".into(), "nope".into()];
    let plan = PlanOutput { tasks: vec![bad] };
    assert_eq!(
        validate(&plan, 1, &PlanLimits::default(), &[], &known),
        Err(PlanError::UnknownRepo {
            index: 0,
            position: 1,
            repo: "nope".into(),
            known: "benchfs, benchfs-paper".into(),
        })
    );

    // 案件にリポジトリが無ければ `repos` を書いた計画は通らない。
    let mut any = new_task("c", vec![]);
    any.repos = vec!["benchfs".into()];
    let plan = PlanOutput { tasks: vec![any] };
    assert!(matches!(
        validate(&plan, 1, &PlanLimits::default(), &[], &[]),
        Err(PlanError::UnknownRepo { known, .. }) if known == "none"
    ));

    // `parse_and_validate` 経由でも同じ（Plan run の暗黙条件から見えるエラー文言）。
    let json = r#"{"tasks":[{"title":"t","objective":"o","acceptance":[{"text":"c","check":{"type":"reviewer"}}],"repos":["nope"]}]}"#;
    let err = parse_and_validate(json, 1, &PlanLimits::default(), &[], &known).unwrap_err();
    assert!(err.contains("nope"), "{err}");
    // 書かなければ従来どおり通る（既存の計画はそのまま）。
    let json = r#"{"tasks":[{"title":"t","objective":"o","acceptance":[{"text":"c","check":{"type":"reviewer"}}]}]}"#;
    assert!(parse_and_validate(json, 1, &PlanLimits::default(), &[], &[]).is_ok());
}

/// ADR-0043 D2: 子のリポジトリは **明示（名前）> 親 > 案件の primary**。
#[test]
fn child_repos_are_explicit_then_parent_then_the_project_primary() {
    let repos = vec![
        project_repo("benchfs", true),
        project_repo("benchfs-paper", false),
    ];
    let primary = crate::repos::RepoRef::of(&repos[0]);
    let paper = crate::repos::RepoRef::of(&repos[1]);
    let ws = WorkspaceContext {
        repos: &repos,
        project: None,
        home: None,
    };

    // 何も書かず、親も持たなければ案件の primary。
    let p = parent();
    assert!(p.repos.is_empty());
    let plan = PlanOutput {
        tasks: vec![new_task("a", vec![])],
    };
    let children = materialize(&p, &plan, &[], &[], &[], ws, OffsetDateTime::now_utc());
    assert_eq!(children[0].repos, vec![primary.clone()]);

    // 親が持っていれば親を継ぐ（primary ではない）。
    let mut inheriting = parent();
    inheriting.repos = vec![paper.clone()];
    let children = materialize(
        &inheriting,
        &plan,
        &[],
        &[],
        &[],
        ws,
        OffsetDateTime::now_utc(),
    );
    assert_eq!(children[0].repos, vec![paper.clone()]);

    // 明示が一番強い（複数可。並びはそのまま = `repos[0]` が cwd）。
    let mut explicit = new_task("b", vec![]);
    explicit.repos = vec!["benchfs-paper".into(), "benchfs".into()];
    let plan = PlanOutput {
        tasks: vec![new_task("a", vec![]), explicit],
    };
    let children = materialize(
        &inheriting,
        &plan,
        &[],
        &[],
        &[],
        ws,
        OffsetDateTime::now_utc(),
    );
    assert_eq!(
        children[0].repos,
        vec![paper.clone()],
        "書かない子は親を継ぐ"
    );
    assert_eq!(children[1].repos, vec![paper, primary]);

    // 案件にリポジトリが無ければ空のまま（従来の 1 つの `workspace` だけで動く）。
    let children = materialize(
        &p,
        &plan,
        &[],
        &[],
        &[],
        WorkspaceContext::default(),
        OffsetDateTime::now_utc(),
    );
    assert!(children[0].repos.is_empty());
}

/// ADR-0007 D2 / ADR-0003 D6: 生成スキーマとコミット済みファイルの一致。`UPDATE_SCHEMA=1` で再生成。
#[test]
fn committed_schema_matches_generated() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/protocol/plan-output.schema.json"
    );
    let generated = serde_json::to_string_pretty(&schema_value()).unwrap() + "\n";
    if std::env::var_os("UPDATE_SCHEMA").is_some() {
        std::fs::write(path, &generated).unwrap();
    }
    let committed = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {path}: {e} (run with UPDATE_SCHEMA=1 to generate)"));
    assert_eq!(
        committed, generated,
        "schema drift: run `UPDATE_SCHEMA=1 cargo test -p task-core`"
    );
}
