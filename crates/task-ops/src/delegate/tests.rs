use super::*;
use std::path::PathBuf;
use task_core::{
    Budget, Check, Criterion, Event, RunRole, SqliteStore, Task, TaskKind, Tier, WorkerHint,
    WorkspaceSpec,
};

fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

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

fn profile_node(
    id: &str,
    parent: Option<&str>,
    kind: task_core::OrgKind,
    allowed: &[&str],
) -> task_core::OrgNode {
    let t = now();
    task_core::OrgNode {
        id: id.into(),
        parent_id: parent.map(str::to_string),
        name: id.into(),
        kind,
        genre: None,
        brief: String::new(),
        profile: task_core::Profile {
            harnesses: task_core::HarnessPrefs {
                allowed: allowed.iter().map(|s| s.to_string()).collect(),
                default: allowed.first().map(|s| s.to_string()),
            },
            ..task_core::Profile::default()
        },
        position: 0,
        created_at: t,
        updated_at: t,
    }
}

fn child_spec(key: &str, genre: &str, deps: &[&str]) -> task_core::ExecutionChildSpec {
    task_core::ExecutionChildSpec {
        requirements: Default::default(),
        key: key.into(),
        title: format!("child {key}"),
        objective: format!("do {key}"),
        acceptance: vec![Criterion {
            text: "c".into(),
            check: Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
        }],
        genre: Some(genre.into()),
        skills: vec!["survey".into()],
        features: None,
        depends_on: deps.iter().map(|d| d.to_string()).collect(),
    }
}

#[test]
fn browser_allowed_domains_execution_plan_children_cannot_exceed_parent() {
    let store = SqliteStore::open_in_memory().unwrap();
    let mut parent = make_task(None, Status::Running);
    parent.skills = vec!["browser-enabled".into()];
    parent.requirements.browser = Some(task_core::BrowserRequirements {
        allowed_domains: vec!["https://*.example.com:8443".into()],
    });
    store.insert(&parent).unwrap();
    let genres = [GenreSpec {
        id: "coding".into(),
        ..GenreSpec::default()
    }];
    let mut child = child_spec("browser", "coding", &[]);
    child.skills = parent.skills.clone();
    for (requirements, accepted) in [
        (None, false),
        (Some(vec![]), false),
        (Some(vec!["http://example.com".into()]), false),
        (Some(vec!["https://other.com:8443".into()]), false),
        (Some(vec!["https://billing.example.com:8443".into()]), true),
    ] {
        child.requirements.browser =
            requirements.map(|allowed_domains| task_core::BrowserRequirements { allowed_domains });
        let result = plan_children(
            &store,
            &parent,
            &[child.clone()],
            &[],
            &genres,
            &DelegationLimits::default(),
            now(),
        );
        assert_eq!(result.is_ok(), accepted, "{result:?}");
    }
}

/// ADR-0074 D3.7（Phase F4b (f)）: planner の `children` は既存の委譲の検証（`plan_delegation`）を
/// 通って `child-<key>` の印つきの子になる。担当になるはずのノードが親と別の部なら秘書への質問
/// （認可されるまで子は作らない）、認可（`once`）の後は通る、`denied` なら拒否。木の深さの上限など
/// 委譲の検証に落ちれば拒否。
#[test]
fn plan_children_runs_the_delegation_checks_and_asks_before_crossing_departments() {
    use task_core::OrgKind;
    use task_core::approval::{Approval, ApprovalId, ApprovalStore, Decision};
    let store = SqliteStore::open_in_memory().unwrap();
    for n in [
        profile_node("cos", None, OrgKind::Secretary, &["conversation"]),
        profile_node("engineering", Some("cos"), OrgKind::Department, &["coding"]),
        profile_node(
            "research",
            Some("cos"),
            OrgKind::Department,
            &["literature"],
        ),
    ] {
        store.org_upsert(&n).unwrap();
    }
    let genres: Vec<GenreSpec> = ["coding", "literature"]
        .iter()
        .map(|g| GenreSpec {
            id: g.to_string(),
            ..GenreSpec::default()
        })
        .collect();
    let mut parent = make_task(None, Status::Running);
    parent.assignee = Some("engineering".into());
    parent.genre = Some("coding".into());
    store.insert(&parent).unwrap();
    let limits = DelegationLimits::default();

    // 同じ部（coding）の子はそのまま通る。依存は同じ children の中の index になる。
    let same = [
        child_spec("impl", "coding", &[]),
        child_spec("tests", "coding", &["impl"]),
    ];
    match plan_children(&store, &parent, &same, &[], &genres, &limits, now()).unwrap() {
        ChildrenPlan::Ready(tasks) => {
            assert_eq!(tasks.len(), 2);
            assert!(tasks[0].labels.contains(&"child-impl".to_string()));
            assert_eq!(tasks[1].depends_on, vec![tasks[0].id]);
            assert!(tasks.iter().all(|t| t.parent_id == Some(parent.id)));
            assert!(tasks.iter().all(|t| t.assignee.is_none()));
            assert_eq!(tasks[0].skills, vec!["survey".to_string()]);
        }
        other => panic!("{other:?}"),
    }

    // 別の部（literature → research）の子は秘書への質問。
    let cross = [child_spec("lit", "literature", &[])];
    let question =
        match plan_children(&store, &parent, &cross, &[], &genres, &limits, now()).unwrap() {
            ChildrenPlan::NeedsAuthorization(qs) => {
                assert_eq!(qs.len(), 1);
                assert!(
                    qs[0].starts_with("cross-department: engineering -> research"),
                    "{qs:?}"
                );
                qs[0].clone()
            }
            other => panic!("{other:?}"),
        };
    // 人が「今回だけ」認めれば通る。
    let mut approval = Approval {
        id: ApprovalId::new(),
        project_id: None,
        node_id: "engineering".into(),
        task_id: Some(parent.id),
        question: question.clone(),
        decision: Some(Decision::Once),
        answer: None,
        created_at: now(),
        decided_at: Some(now()),
    };
    store.approval_append(&approval).unwrap();
    assert!(matches!(
        plan_children(&store, &parent, &cross, &[], &genres, &limits, now()).unwrap(),
        ChildrenPlan::Ready(t) if t.len() == 1
    ));
    // 後から「認めない」と答え直せば拒否。
    approval.id = ApprovalId::new();
    approval.decision = Some(Decision::Denied);
    approval.decided_at = Some(now() + time::Duration::seconds(5));
    store.approval_append(&approval).unwrap();
    let err =
        plan_children(&store, &parent, &cross, &[], &genres, &limits, now()).expect_err("denied");
    assert!(err.contains("denied"), "{err}");

    // 委譲の検証（木の深さの上限）に落ちれば拒否。
    let shallow = DelegationLimits {
        max_tree_depth: 1,
        ..DelegationLimits::default()
    };
    let err =
        plan_children(&store, &parent, &same, &[], &genres, &shallow, now()).expect_err("too deep");
    assert!(err.contains("tree depth"), "{err}");
}

fn make_task(parent_id: Option<TaskId>, status: Status) -> Task {
    let t = now();
    Task {
        harness_id: None,
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id,
        kind: TaskKind::Execute,
        title: "t".into(),
        objective: "o".into(),
        acceptance: vec![],
        inputs: vec![],
        depends_on: vec![],
        status,
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
        created_at: t,
        updated_at: t,
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

fn insert(store: &SqliteStore, task: &Task) {
    store.insert(task).expect("insert");
}

#[test]
fn accepts_proposals_maps_index_deps_and_applies_role_defaults() {
    let store = SqliteStore::open_in_memory().expect("open");
    let parent = make_task(None, Status::Running);
    insert(&store, &parent);

    let roles = vec![RoleSpec {
        id: "implementer".into(),
        tier: Some(Tier::Cheap),
        harness: Some("codex".into()),
        max_turns: None,
        max_wall_secs: None,
        instructions: None,
    }];
    let mut a = dt("a", vec![]);
    a.role = Some("implementer".into());
    let b = dt("b", vec![DelegateDep::Index(0)]);
    let proposals = vec![a, b];

    let out = plan_delegation(
        &store,
        &parent,
        &proposals,
        0,
        &roles,
        &[],
        &DelegationLimits::default(),
        now(),
    )
    .expect("plan_delegation");
    assert_eq!(out.accepted.len(), 2);
    assert!(out.rejected.is_empty());
    assert_eq!(out.accepted[0].worker_hint.tier, Tier::Cheap);
    assert_eq!(out.accepted[1].depends_on, vec![out.accepted[0].id]);
}

/// ADR-0027 D1: `plan_delegation` は分野を解決し（役割の分野が一意なら継ぐ）、`genre` を指定した
/// タスクの `role` がその分野に無ければ拒否する。unrelated_genre はここまでの検証を通っても
/// role/genre の不整合で落ちる。
#[test]
fn plan_delegation_resolves_genre_and_rejects_role_genre_mismatch() {
    let store = SqliteStore::open_in_memory().expect("open");
    let mut parent = make_task(None, Status::Running);
    parent.genre = Some("coding".into());
    insert(&store, &parent);

    let genres = vec![task_core::GenreSpec {
        id: "literature".into(),
        description: "survey".into(),
        default_role: Some("literature-reader".into()),
        roles: vec!["literature-scout".into(), "literature-reader".into()],
        ..task_core::GenreSpec::default()
    }];

    // 1. role だけ指定: 分野が一意に決まるので "literature" を継ぐ。
    let mut by_role = dt("by-role", vec![]);
    by_role.role = Some("literature-scout".into());
    // 2. genre と role が矛盾: 拒否される。
    let mut mismatched = dt("mismatched", vec![]);
    mismatched.genre = Some("literature".into());
    mismatched.role = Some("implementer".into());
    // 3. 何も指定しない: 親の分野 "coding" を継ぐ。
    let inherits = dt("inherits", vec![]);

    let proposals = vec![by_role, mismatched, inherits];
    let out = plan_delegation(
        &store,
        &parent,
        &proposals,
        0,
        &[],
        &genres,
        &DelegationLimits::default(),
        now(),
    )
    .expect("plan_delegation");
    assert_eq!(out.accepted.len(), 2);
    assert_eq!(out.accepted[0].title, "by-role");
    assert_eq!(out.accepted[0].genre.as_deref(), Some("literature"));
    assert_eq!(out.accepted[1].title, "inherits");
    assert_eq!(out.accepted[1].genre.as_deref(), Some("coding"));
    assert_eq!(out.rejected.len(), 1);
    assert!(
        out.rejected[0].contains("mismatched"),
        "{}",
        out.rejected[0]
    );
    assert!(
        out.rejected[0].contains("is not one of genre"),
        "{}",
        out.rejected[0]
    );
}

#[test]
fn per_run_limit_rejects_the_remainder() {
    let store = SqliteStore::open_in_memory().expect("open");
    let parent = make_task(None, Status::Running);
    insert(&store, &parent);

    let limits = DelegationLimits {
        max_delegate_per_run: 2,
        ..DelegationLimits::default()
    };
    let proposals = vec![dt("a", vec![]), dt("b", vec![])];
    let out = plan_delegation(&store, &parent, &proposals, 1, &[], &[], &limits, now())
        .expect("plan_delegation");
    assert_eq!(out.accepted.len(), 1);
    assert_eq!(out.rejected.len(), 1);
    assert!(
        out.rejected[0].contains("per-run delegation limit"),
        "{}",
        out.rejected[0]
    );
}

#[test]
fn tree_depth_limit_rejects_everything() {
    let store = SqliteStore::open_in_memory().expect("open");
    let root = make_task(None, Status::Running);
    insert(&store, &root);
    let mid = make_task(Some(root.id), Status::Running);
    insert(&store, &mid);
    let parent = make_task(Some(mid.id), Status::Running);
    insert(&store, &parent);
    // tree_depth(parent) == 3, +1 == 4 > max_tree_depth (3) -> reject all
    let limits = DelegationLimits {
        max_tree_depth: 3,
        ..DelegationLimits::default()
    };
    let proposals = vec![dt("a", vec![]), dt("b", vec![])];
    let out = plan_delegation(&store, &parent, &proposals, 0, &[], &[], &limits, now())
        .expect("plan_delegation");
    assert!(out.accepted.is_empty());
    assert_eq!(out.rejected.len(), 2);
    assert!(
        out.rejected[0].contains("tree depth would become 4"),
        "{}",
        out.rejected[0]
    );
}

#[test]
fn tree_runs_limit_rejects_everything_and_ignores_reviewer_runs() {
    let store = SqliteStore::open_in_memory().expect("open");
    let root = make_task(None, Status::Running);
    insert(&store, &root);
    let child = make_task(Some(root.id), Status::Running);
    insert(&store, &child);

    // 2 ワーカー run（root と child）+ 1 reviewer run（数えない）。
    store
        .append_event(
            root.id,
            &Event::WorkerStarted {
                run_id: "r1".into(),
                adapter: "a".into(),
                model: "m".into(),
                provider: None,
                account: None,
                role: None,
                task_role: None,
            },
        )
        .expect("append");
    store
        .append_event(
            child.id,
            &Event::WorkerStarted {
                run_id: "r2".into(),
                adapter: "a".into(),
                model: "m".into(),
                provider: None,
                account: None,
                role: None,
                task_role: None,
            },
        )
        .expect("append");
    store
        .append_event(
            child.id,
            &Event::WorkerStarted {
                run_id: "rev1".into(),
                adapter: "a".into(),
                model: "m".into(),
                provider: None,
                account: None,
                role: Some(RunRole::Reviewer),
                task_role: None,
            },
        )
        .expect("append");

    assert_eq!(tree_worker_runs(&store, root.id).expect("count"), 2);

    let limits = DelegationLimits {
        max_tree_runs: 2,
        ..DelegationLimits::default()
    };
    let proposals = vec![dt("a", vec![])];
    let out = plan_delegation(&store, &child, &proposals, 0, &[], &[], &limits, now())
        .expect("plan_delegation");
    assert!(out.accepted.is_empty());
    assert_eq!(out.rejected.len(), 1);
    assert!(
        out.rejected[0].contains("tree already has 2 worker runs"),
        "{}",
        out.rejected[0]
    );
}

#[test]
fn rejects_self_and_ancestor_and_missing_dependencies_but_accepts_others() {
    let store = SqliteStore::open_in_memory().expect("open");
    let grandparent = make_task(None, Status::Running);
    insert(&store, &grandparent);
    let parent = make_task(Some(grandparent.id), Status::Running);
    insert(&store, &parent);
    let missing_id = TaskId::new();

    let proposals = vec![
        dt("self-ref", vec![DelegateDep::Id(parent.id.to_string())]),
        dt(
            "ancestor-ref",
            vec![DelegateDep::Id(grandparent.id.to_string())],
        ),
        dt("missing-ref", vec![DelegateDep::Id(missing_id.to_string())]),
        dt("ok", vec![]),
    ];
    let out = plan_delegation(
        &store,
        &parent,
        &proposals,
        0,
        &[],
        &[],
        &DelegationLimits::default(),
        now(),
    )
    .expect("plan_delegation");
    assert_eq!(out.accepted.len(), 1);
    assert_eq!(out.accepted[0].title, "ok");
    assert_eq!(out.rejected.len(), 3);
    assert!(
        out.rejected[0].contains("is the delegating task itself"),
        "{}",
        out.rejected[0]
    );
    assert!(
        out.rejected[1].contains("is an ancestor of the delegating task"),
        "{}",
        out.rejected[1]
    );
    assert!(
        out.rejected[2].contains("does not exist"),
        "{}",
        out.rejected[2]
    );
}

#[test]
fn rejects_dependency_with_failed_status() {
    let store = SqliteStore::open_in_memory().expect("open");
    let parent = make_task(None, Status::Running);
    insert(&store, &parent);
    let failed_dep = make_task(None, Status::Failed);
    insert(&store, &failed_dep);

    let proposals = vec![dt("a", vec![DelegateDep::Id(failed_dep.id.to_string())])];
    let out = plan_delegation(
        &store,
        &parent,
        &proposals,
        0,
        &[],
        &[],
        &DelegationLimits::default(),
        now(),
    )
    .expect("plan_delegation");
    assert!(out.accepted.is_empty());
    assert_eq!(out.rejected.len(), 1);
    assert!(
        out.rejected[0].contains("has status Failed and cannot be depended on"),
        "{}",
        out.rejected[0]
    );
}

/// ADR-0039 D2: 委譲した子は **明示 > 案件の workspace > 親** の順で作業場所を決める。
/// 案件の作業場所は DB（`projects.workspace`）から引く（ストアの読み取りだけ）。
#[test]
fn delegated_children_inherit_the_projects_workspace() {
    let store = SqliteStore::open_in_memory().expect("open");
    let now_ts = now();
    let project = task_core::Project {
        auto_advance: false,
        auto_integrate: false,
        slug: None,
        archived_at: None,
        paused_from: None,
        id: task_core::ProjectId::new(),
        title: "Pluvio".into(),
        request: "PoC".into(),
        status: task_core::ProjectStatus::Active,
        workspace: Some(WorkspaceSpec::Remote {
            cluster: "pegasus".into(),
            path: PathBuf::from("/work/NBB/rmaeda/workspace/rust/benchfs"),
            mode: None,
        }),
        created_at: now_ts,
        updated_at: now_ts,
    };
    store.project_create(&project).expect("project");

    let mut parent = make_task(None, Status::Running);
    parent.project_id = Some(project.id);
    insert(&store, &parent);

    // 1. 何も書かなければ案件の作業場所（親の `/tmp/ws` ではない）。
    let out = plan_delegation(
        &store,
        &parent,
        &[dt("a", vec![])],
        0,
        &[],
        &[],
        &DelegationLimits::default(),
        now(),
    )
    .expect("plan_delegation");
    assert_eq!(
        out.accepted[0].workspace,
        project.workspace.clone().expect("some")
    );

    // 2. 子が明示すれば案件より強い。
    let mut explicit = dt("b", vec![]);
    let elsewhere = WorkspaceSpec::Local {
        path: PathBuf::from("/home/rmaeda/workspace/rust/pluvio-poc"),
        mode: None,
    };
    explicit.workspace = Some(elsewhere.clone());
    let out = plan_delegation(
        &store,
        &parent,
        &[explicit],
        0,
        &[],
        &[],
        &DelegationLimits::default(),
        now(),
    )
    .expect("plan_delegation");
    assert_eq!(out.accepted[0].workspace, elsewhere);

    // 3. 案件に属さない親では従来どおり親を継ぐ。
    let orphan = make_task(None, Status::Running);
    insert(&store, &orphan);
    let out = plan_delegation(
        &store,
        &orphan,
        &[dt("c", vec![])],
        0,
        &[],
        &[],
        &DelegationLimits::default(),
        now(),
    )
    .expect("plan_delegation");
    assert_eq!(out.accepted[0].workspace, orphan.workspace);
}

#[test]
fn pending_children_does_not_count_terminal_children() {
    let store = SqliteStore::open_in_memory().expect("open");
    let parent = make_task(None, Status::Running);
    insert(&store, &parent);
    let done_child = make_task(Some(parent.id), Status::Done);
    insert(&store, &done_child);
    let running_child = make_task(Some(parent.id), Status::Running);
    insert(&store, &running_child);

    assert_eq!(pending_children(&store, parent.id).expect("pending"), 1);
}

/// ADR 2026-10-18: 既存の子（done・running・ready）と同じ範囲の提案は拒否し、failed の子と同じ範囲は再起票できる。
#[test]
fn rejects_reproposing_the_scope_of_an_existing_child() {
    let store = SqliteStore::open_in_memory().expect("open");
    let parent = make_task(None, Status::Running);
    insert(&store, &parent);
    for (title, status) in [
        ("31a endpoint", Status::Done),
        ("31b tests", Status::Running),
        ("31c docs", Status::Ready),
        ("31d retry", Status::Failed),
    ] {
        let mut c = make_task(Some(parent.id), status);
        c.title = title.into();
        c.objective = format!("implement {title} fully");
        insert(&store, &c);
    }
    let proposals = vec![
        dt("31a endpoint", vec![]),
        {
            let mut t = dt("other name", vec![]);
            t.objective = "implement 31b tests fully".into();
            t
        },
        dt("31c docs", vec![]),
        dt("31d retry", vec![]),
        dt("31e new scope", vec![]),
    ];
    let out = plan_delegation(
        &store,
        &parent,
        &proposals,
        0,
        &[],
        &[],
        &DelegationLimits::default(),
        now(),
    )
    .expect("plan_delegation");
    assert_eq!(out.rejected.len(), 3, "{:?}", out.rejected);
    assert!(
        out.rejected
            .iter()
            .all(|r| r.contains("duplicates existing child"))
    );
    let titles: Vec<&str> = out.accepted.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles, vec!["31d retry", "31e new scope"]);
}

/// ADR 2026-10-18-parent-waits-for-children 付記: 「待て」の読み取りは決定的な語の照合で、否定を含めば読まない。
#[test]
fn wait_instruction_matches_wait_phrases_and_rejects_negations() {
    use child_wait::is_wait_instruction;
    assert!(is_wait_instruction("01M5WC8PPW の再実行を待ってください"));
    assert!(is_wait_instruction(
        "Please WAIT FOR THE RERUN of the child"
    ));
    assert!(is_wait_instruction("子の完了を待つ"));
    assert!(!is_wait_instruction("子の完了を待たずに進めて"));
    assert!(!is_wait_instruction("don't wait for the rerun, proceed"));
    assert!(!is_wait_instruction("進めてください"));
}

/// `failed` の子を `Transitioned{to: failed}` 付きで作る。
fn failed_child(store: &SqliteStore, parent: TaskId) -> Task {
    let child = make_task(Some(parent), Status::Ready);
    insert(store, &child);
    store
        .apply_transition(child.id, task_core::Trigger::TokenBudgetExhausted, None)
        .expect("fail child");
    child
}

fn comment(
    store: &SqliteStore,
    task: TaskId,
    kind: task_core::comment::CommentAuthorKind,
    run_id: Option<&str>,
    body: &str,
    at: OffsetDateTime,
) {
    let c = task_core::comment::TaskComment::new(
        task,
        kind,
        None,
        body.into(),
        run_id.map(str::to_owned),
        at,
    );
    store.comment_add(&c, None).expect("comment");
}

fn awaited(store: &SqliteStore, parent: TaskId, escalated: &[TaskId]) -> Vec<TaskId> {
    let mut lookup = |id: TaskId| Ok(escalated.contains(&id));
    child_wait::awaited_children(store, parent, false, &mut lookup).expect("awaited")
}

/// 印の無い `failed` の子は待たない。未解決の escalated 項目、人・CoS の「待て」の comment があれば待つ。
/// worker の run の comment は指示に数えない。
#[test]
fn awaited_children_holds_a_failed_child_only_while_told_to_wait() {
    use task_core::comment::CommentAuthorKind;
    let store = SqliteStore::open_in_memory().expect("open");
    let parent = make_task(None, Status::Ready);
    insert(&store, &parent);
    let child = failed_child(&store, parent.id);

    assert!(
        awaited(&store, parent.id, &[]).is_empty(),
        "印が無ければ終端"
    );
    assert_eq!(awaited(&store, parent.id, &[child.id]), vec![child.id]);

    comment(
        &store,
        child.id,
        CommentAuthorKind::Node,
        Some("run-1"),
        "再実行を待ちます",
        now(),
    );
    assert!(
        awaited(&store, parent.id, &[]).is_empty(),
        "worker の comment は数えない"
    );

    comment(
        &store,
        child.id,
        CommentAuthorKind::Node,
        None,
        "CoS: 修正の昇格後に再実行するので、それまで再実行を待つこと",
        now(),
    );
    assert_eq!(awaited(&store, parent.id, &[]), vec![child.id]);
}

/// 子が reopen されて再び失敗したら、前の失敗への「待て」は効かない（親を起こす）。
#[test]
fn a_wait_instruction_before_the_latest_failure_does_not_hold() {
    use task_core::comment::CommentAuthorKind;
    let store = SqliteStore::open_in_memory().expect("open");
    let parent = make_task(None, Status::Ready);
    insert(&store, &parent);
    let child = failed_child(&store, parent.id);
    comment(
        &store,
        parent.id,
        CommentAuthorKind::Human,
        None,
        "子の再実行を待って",
        now() - time::Duration::hours(1),
    );
    assert!(awaited(&store, parent.id, &[]).is_empty());

    // 失敗の後の人の回答（親への answer）は効く。
    store
        .apply_transition(parent.id, task_core::Trigger::Dispatch, None)
        .expect("dispatch");
    store
        .apply_transition(parent.id, task_core::Trigger::WorkerQuestion, None)
        .expect("question");
    crate::gate::answer(&store, parent.id, "子の再実行を待ってください".into(), None)
        .expect("answer");
    assert_eq!(awaited(&store, parent.id, &[]), vec![child.id]);
    // 再実行中（ready）は、親の最後の run より後の指示があるので待つ。
    store
        .apply_transition(child.id, task_core::Trigger::Reopen, None)
        .expect("reopen");
    assert_eq!(awaited(&store, parent.id, &[]), vec![child.id]);
    // 再び失敗したら外れる。
    store
        .apply_transition(child.id, task_core::Trigger::TokenBudgetExhausted, None)
        .expect("fail again");
    assert!(awaited(&store, parent.id, &[]).is_empty());
}
