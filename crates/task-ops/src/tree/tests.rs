use super::*;
use task_core::{Budget, Check, Criterion, SqliteStore, TaskKind, Tier, WorkerHint, WorkspaceSpec};

fn parent(repos: &[&str]) -> Task {
    let now = OffsetDateTime::now_utc();
    Task {
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: vec!["rust".into()],
        repos: repos
            .iter()
            .map(|n| task_core::RepoRef {
                repo_id: task_core::RepoId::new(),
                name: (*n).to_string(),
            })
            .collect(),
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "browser capability".into(),
        objective: "o".into(),
        acceptance: vec![],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Ready,
        priority: 2,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::Local {
            path: "/tmp/ws".into(),
            mode: None,
        },
        budget: Budget {
            max_turns: 30,
            max_wall_secs: 900,
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

fn v3(units: serde_json::Value) -> ExecutionPlanSpec {
    serde_json::from_value(serde_json::json!({
        "schema": task_core::EXECUTION_PLAN_SCHEMA_V3,
        "rationale": "r",
        "stages": [{"key": "phase-2", "kind": "implement", "title": "Phase 2"}],
        "units": units,
    }))
    .unwrap()
}

fn task_unit(repos: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "key": "p2-b", "stage": "phase-2", "kind": "task", "title": "broker",
        "objective": "build the broker",
        "acceptance": [serde_json::to_value(Criterion {
            text: "ok".into(),
            check: Check::Command { cmd: "true".into(), expect_exit: 0 },
        }).unwrap()],
        "repos": repos,
        "skills": ["go"],
    })
}

/// R1a からの持ち越し: kind task の unit の `repos` は親の repos の部分集合。
#[test]
fn task_unit_repos_must_be_a_subset_of_the_parent_repos() {
    let store = SqliteStore::open_in_memory().unwrap();
    let p = parent(&["agent-platform", "docs"]);
    assert!(
        check_task_unit_repos(&store, &p, &v3(serde_json::json!([task_unit(&["docs"])]))).is_ok()
    );
    assert!(check_task_unit_repos(&store, &p, &v3(serde_json::json!([task_unit(&[])]))).is_ok());
    let err = check_task_unit_repos(
        &store,
        &p,
        &v3(serde_json::json!([task_unit(&["docs", "benchfs"])])),
    )
    .unwrap_err();
    assert!(err.contains("\"benchfs\""), "{err}");
    assert!(err.contains("agent-platform, docs"), "{err}");
    // 親が repos を持たず案件も無ければ、repos を書いた unit はすべて外れる。
    assert!(
        check_task_unit_repos(
            &store,
            &parent(&[]),
            &v3(serde_json::json!([task_unit(&["docs"])]))
        )
        .is_err()
    );
}

/// D4 (4): 子は unit の repos（親の部分集合）・skills、親の workspace・budget・案件を継ぎ、ready・
/// `child-<key>`・`tree`（深さ 2、root = 親）を持ち、objective の末尾に木の位置が固定の書式で入る。
#[test]
fn build_child_task_inherits_from_the_parent_and_the_unit() {
    let store = SqliteStore::open_in_memory().unwrap();
    let p = parent(&["agent-platform", "docs"]);
    let spec = v3(serde_json::json!([task_unit(&["docs"])]));
    let (child, downgrades) = build_child_task(
        &store,
        &p,
        "plan-1",
        &spec.units[0],
        &[],
        &[],
        &[],
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    assert!(downgrades.is_empty());
    assert_eq!(child.parent_id, Some(p.id));
    assert_eq!(child.status, Status::Ready);
    assert_eq!(child.kind, TaskKind::Execute);
    assert_eq!(child.labels, vec!["child-p2-b".to_string()]);
    assert_eq!(child.skills, vec!["go".to_string()]);
    assert_eq!(
        child
            .repos
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>(),
        vec!["docs"]
    );
    assert_eq!(child.repos[0].repo_id, p.repos[1].repo_id);
    assert_eq!(child.workspace, p.workspace);
    // R5b-fix3: max(親 30 turns / 900 s, leaf 1 run の既定 30 / 1,800)。
    assert_eq!(
        child.budget,
        Budget {
            max_turns: 30,
            max_wall_secs: 1800,
            max_retries: 2,
        }
    );
    assert_eq!(child.assignee, None);
    let tree = child.tree.as_ref().unwrap();
    assert_eq!(tree.root_id, p.id);
    assert_eq!(tree.depth, 2);
    assert_eq!(
        tree.parent_unit,
        Some(ParentUnit {
            task_id: p.id,
            plan_id: "plan-1".into(),
            unit_key: "p2-b".into(),
            stage: "phase-2".into(),
            attempt: 1,
        })
    );
    assert_eq!(
        child.objective,
        format!(
            "build the broker\n\n{TREE_PATH_HEADING}\n- 深さ 1: 「browser capability」（段階 phase-2）\n- 深さ 2: この task「broker」（unit p2-b）"
        )
    );
    // unit が repos を書かなければ親と同じ。
    let spec = v3(serde_json::json!([task_unit(&[])]));
    let (child, _) = build_child_task(
        &store,
        &p,
        "plan-1",
        &spec.units[0],
        &[],
        &[],
        &[],
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    assert_eq!(child.repos, p.repos);
}

#[test]
fn browser_allowed_domains_plan_child_requires_explicit_subset() {
    let store = SqliteStore::open_in_memory().unwrap();
    let mut p = parent(&[]);
    p.skills = vec!["browser-enabled".into()];
    p.requirements.browser = Some(task_core::BrowserRequirements {
        allowed_domains: vec!["https://*.example.com:8443".into()],
    });
    for (requested, accepted) in [
        (serde_json::Value::Null, false),
        (serde_json::json!({"browser":{"allowed_domains":[]}}), false),
        (
            serde_json::json!({"browser":{"allowed_domains":["http://example.com"]}}),
            false,
        ),
        (
            serde_json::json!({"browser":{"allowed_domains":["https://billing.example.com:8443"]}}),
            true,
        ),
        (
            serde_json::json!({"browser":{"allowed_domains":["https://billing.example.com"]}}),
            false,
        ),
        (
            serde_json::json!({"browser":{"allowed_domains":["https://other.com:8443"]}}),
            false,
        ),
        (
            serde_json::json!({"browser":{"allowed_domains":["https://example.com:8443"]}}),
            false,
        ),
    ] {
        let mut unit = task_unit(&[]);
        unit["skills"] = serde_json::json!(["browser-enabled"]);
        if !requested.is_null() {
            unit["requirements"] = requested;
        }
        let plan = v3(serde_json::json!([unit]));
        let result = build_child_task(
            &store,
            &p,
            "plan-browser",
            &plan.units[0],
            &[],
            &[],
            &[],
            OffsetDateTime::now_utc(),
        );
        assert_eq!(result.is_ok(), accepted, "{result:?}");
    }
}

fn build(store: &SqliteStore, p: &Task, plan_id: &str) -> Task {
    let spec = v3(serde_json::json!([task_unit(&[])]));
    build_child_task(
        store,
        p,
        plan_id,
        &spec.units[0],
        &[],
        &[],
        &[],
        OffsetDateTime::now_utc(),
    )
    .unwrap()
    .0
}

/// R5b-fix3 (D2): 親の `Local` の path が親自身の id なら、子は自分の id（親のディレクトリを共有しない）。
/// mode は親のまま。`Remote` と、リポジトリを指す `Local` の絶対パスはそのまま継ぐ。
#[test]
fn child_gets_its_own_local_dir_when_the_parent_uses_its_id() {
    let store = SqliteStore::open_in_memory().unwrap();
    let mut p = parent(&[]);
    p.workspace = WorkspaceSpec::Local {
        path: p.id.to_string().into(),
        mode: Some(task_core::WorkspaceMode::Worktree),
    };
    let child = build(&store, &p, "plan-1");
    assert_eq!(
        child.workspace,
        WorkspaceSpec::Local {
            path: child.id.to_string().into(),
            mode: Some(task_core::WorkspaceMode::Worktree),
        }
    );
    assert_ne!(child.workspace, p.workspace);

    let remote = WorkspaceSpec::Remote {
        cluster: "sirius".into(),
        path: "/work/NBB/rmaeda/workspace/benchfs".into(),
        mode: None,
    };
    assert_eq!(child_own_workspace(&p, child.id, &remote), remote);
    let repo_path = WorkspaceSpec::local("/home/u/repo");
    assert_eq!(child_own_workspace(&p, child.id, &repo_path), repo_path);
}

/// R5b-fix3 (D3a): 既定の予算（10 turns / 600 s）の親でも、子は leaf 1 run の既定（30 / 1,800）を下回らない。
/// 親の方が大きければ親のまま。
#[test]
fn child_budget_is_never_below_one_leaf_run() {
    let store = SqliteStore::open_in_memory().unwrap();
    let mut p = parent(&[]);
    p.budget = Budget {
        max_turns: 10,
        max_wall_secs: 600,
        max_retries: 2,
    };
    let child = build(&store, &p, "plan-1");
    assert_eq!(
        child.budget,
        Budget {
            max_turns: task_core::tree::TREE_CHILD_MIN_MAX_TURNS,
            max_wall_secs: task_core::tree::TREE_CHILD_MIN_MAX_WALL_SECS,
            max_retries: 2,
        }
    );
    p.budget = Budget {
        max_turns: 60,
        max_wall_secs: 3600,
        max_retries: 1,
    };
    assert_eq!(build(&store, &p, "plan-1").budget, p.budget);
}

/// ADR-0079「R6-2」（R5b-fix3 (D3c) を改める）: 子の `execution_hint` は計画の書き手によらず明示。`gate` を
/// 省けば `{compound, explicit: true}`、`gate: atomic` なら `{atomic, explicit: true}`、`gate: compound` は
/// planner の計画でも `{compound, explicit: true}`（計画の版を引かない。見つからない計画の仮の子も同じ）。
#[test]
fn child_execution_hint_is_explicit_and_follows_the_unit_gate() {
    use task_core::{ExecutionHintSpec, ExecutionMode};
    let store = SqliteStore::open_in_memory().unwrap();
    let p = parent(&[]);
    let hint = |t: &Task| t.routing.as_ref().and_then(|r| r.execution_hint);
    let explicit = |mode| {
        Some(ExecutionHintSpec {
            mode,
            explicit: true,
        })
    };
    // 既定（gate なし）: planner の計画・見つからない計画でも明示の compound。
    assert_eq!(
        hint(&build(&store, &p, "plan-unknown")),
        explicit(ExecutionMode::Compound)
    );
    assert_eq!(
        hint(&build(&store, &p, "")),
        explicit(ExecutionMode::Compound)
    );
    let with_gate = |gate: &str| {
        let mut unit = task_unit(&[]);
        unit["gate"] = serde_json::json!(gate);
        let spec = v3(serde_json::json!([unit]));
        build_child_task(
            &store,
            &p,
            "plan-planner",
            &spec.units[0],
            &[],
            &[],
            &[],
            OffsetDateTime::now_utc(),
        )
        .unwrap()
        .0
    };
    assert_eq!(hint(&with_gate("atomic")), explicit(ExecutionMode::Atomic));
    assert_eq!(
        hint(&with_gate("compound")),
        explicit(ExecutionMode::Compound)
    );
}

#[test]
fn unit_mirror_follows_the_child_status() {
    assert_eq!(
        unit_mirror(Status::Done),
        Some((WorkUnitStatus::Done, "child_done"))
    );
    assert_eq!(
        unit_mirror(Status::Failed),
        Some((WorkUnitStatus::Failed, "child_failed"))
    );
    assert_eq!(
        unit_mirror(Status::Cancelled),
        Some((WorkUnitStatus::Failed, "child_cancelled"))
    );
    for s in [
        Status::Draft,
        Status::Ready,
        Status::Running,
        Status::Blocked,
        Status::Reviewing,
    ] {
        assert_eq!(unit_mirror(s), None, "{s:?} keeps the unit running");
    }
}
