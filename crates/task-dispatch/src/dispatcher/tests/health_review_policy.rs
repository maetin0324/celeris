//! ADR 2026-11-07-health-review-integration D2.3・I3: planner run の起動時に base（target）側の検査器で review 方針を
//! 読み、`ExecutionPlannerContext.review_policy` に `Present` のときだけ入れる。本物の `--print-review-policy` の
//! 代わりに一時 git repo に stub の `repo_health.py` を commit して使う。外部ネットワークに出ない。

use super::*;

use crate::health_review::policy::{CHECKER_SCRIPT, REVIEW_POLICY_SCHEMA};

const DIGEST: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

fn stub_printing(source: &str, class: &str) -> String {
    let json = serde_json::json!({
        "schema": REVIEW_POLICY_SCHEMA,
        "source": source,
        "owner_map": "docs/architecture-map.md",
        "high_risk": [{"class": class, "paths": ["^crates/task-store/"]}],
        "invariants": {"events-append-only": "CLAUDE.md"},
        "definition_sha256": DIGEST,
    });
    format!("import sys\nsys.stdout.write({:?})\n", json.to_string())
}

fn expected(class: &str) -> task_core::design_review::ReviewPolicy {
    task_core::design_review::ReviewPolicy {
        owner_map: Some("docs/architecture-map.md".into()),
        high_risk: vec![task_core::design_review::PathClass {
            class: class.into(),
            paths: vec!["^crates/task-store/".into()],
        }],
        invariants: [("events-append-only".to_string(), "CLAUDE.md".to_string())]
            .into_iter()
            .collect(),
        definition_sha256: DIGEST.into(),
    }
}

/// main に `stub` を commit した repo。
fn checker_repo(stub: &str) -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let script = repo.path().join(CHECKER_SCRIPT);
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(&script, stub).unwrap();
    git_out(repo.path(), &["add", "-A"]);
    git_out(repo.path(), &["commit", "-q", "-m", "checker"]);
    repo
}

fn compound_git_task(repo: &std::path::Path) -> Task {
    let mut task = compound_task(repo);
    task.workspace = WorkspaceSpec::Local {
        path: repo.to_path_buf(),
        mode: None,
    };
    task
}

/// planner run の `RunContext.execution_planner.review_policy` に main（base）の検査器の方針が入る。source repo の
/// 作業ツリーで検査器を書き換えても（未 commit）、base の版が使われる。
#[tokio::test]
async fn health_review_policy_reaches_the_planner_run_context() {
    let repo = checker_repo(&stub_printing("base", "base-class"));
    std::fs::write(
        repo.path().join(CHECKER_SCRIPT),
        stub_printing("base", "worktree-class"),
    )
    .unwrap();
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = compound_git_task(repo.path());
    store.insert(&task).unwrap();
    let adapter = Arc::new(PlannerScriptAdapter::new(
        vec![Some(plan_json(vec![wu_spec("a", &[])]))],
        HashMap::new(),
    ));
    let mut d = worktree_dispatcher(store.clone(), adapter.clone(), root.path(), None);
    d.config.execution.gate = task_core::GateMode::On;
    run_until_idle(&mut d, 400).await;

    let planner: Vec<_> = adapter
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter_map(|c| c.execution_planner.clone())
        .collect();
    assert!(!planner.is_empty(), "a planner run was dispatched");
    assert_eq!(planner[0].review_policy, Some(expected("base-class")));
    let work_units: Vec<_> = adapter
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter(|c| c.execution_planner.is_none())
        .cloned()
        .collect();
    assert!(!work_units.is_empty(), "the plan was adopted and run");
}

/// /2 の planner（木が無効）と /3 の planner（木が有効、`tree` が入る）の両方の文脈に同じ方針が入る。
#[tokio::test]
async fn health_review_policy_is_in_both_v2_and_tree_planner_contexts() {
    let repo = checker_repo(&stub_printing("builtin", "generic"));
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = compound_git_task(repo.path());
    store.insert(&task).unwrap();
    let adapter = Arc::new(PlannerScriptAdapter::new(Vec::new(), HashMap::new()));
    let mut d = worktree_dispatcher(store.clone(), adapter, root.path(), None);

    let flat = d
        .execution_planner_context(&task, task.budget, false)
        .unwrap();
    assert!(flat.tree.is_none());
    assert_eq!(flat.review_policy, Some(expected("generic")));

    d.config.execution.limits.tree.enabled = true;
    let tree = d
        .execution_planner_context(&task, task.budget, false)
        .unwrap();
    assert!(tree.tree.is_some(), "the /3 planner gets the tree context");
    assert_eq!(tree.review_policy, Some(expected("generic")));
}

/// `Absent`・`Unavailable` は planner の文脈に入れない（`None`）。状態は `planner_review_policy` で区別できる:
/// `source = absent` → `Absent`、非 0 exit → `Unavailable`、git の repo の無い作業場所 → `Absent`、remote → `Unavailable`。
#[tokio::test]
async fn health_review_policy_absent_and_unavailable_stay_out_of_the_planner_context() {
    use task_core::design_review::ReviewPolicyState;
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter = Arc::new(PlannerScriptAdapter::new(Vec::new(), HashMap::new()));
    let d = worktree_dispatcher(store.clone(), adapter, root.path(), None);

    let absent = checker_repo(&stub_printing("absent", "unused"));
    let failing = checker_repo("import sys\nsys.stderr.write('boom\\n')\nsys.exit(3)\n");
    let plain = tempfile::tempdir().unwrap();
    for (dir, want_absent) in [
        (absent.path(), true),
        (failing.path(), false),
        (plain.path(), true),
    ] {
        let task = compound_git_task(dir);
        store.insert(&task).unwrap();
        let state = d.planner_review_policy(&task);
        assert_eq!(
            matches!(state, ReviewPolicyState::Absent),
            want_absent,
            "{dir:?}: {state:?}"
        );
        if !want_absent {
            assert!(
                matches!(&state, ReviewPolicyState::Unavailable(why) if why.contains("boom")),
                "{state:?}"
            );
        }
        let ctx = d
            .execution_planner_context(&task, task.budget, false)
            .unwrap();
        assert_eq!(ctx.review_policy, None, "{dir:?}");
    }

    let mut remote = compound_task(plain.path());
    remote.workspace = WorkspaceSpec::Remote {
        cluster: "pegasus".into(),
        path: "/work/x".into(),
        mode: None,
    };
    assert_eq!(
        d.planner_review_policy(&remote),
        ReviewPolicyState::Unavailable("remote workspace".into())
    );
}
