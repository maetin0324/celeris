//! ADR 2026-11-07-health-review-integration D2.3・I3・付記 (b): worker と planner の run の起動時に base（target）側の
//! 検査器で review 方針を読み、`RunContext.workspace.review_policy` に `Present` のときだけ入れる（試験名の prefix は
//! `health_review_worker_profile_`）。本物の `--print-review-policy` の代わりに一時 git repo に stub の `repo_health.py` を
//! commit して使う。外部ネットワークに出ない。

use super::*;

use crate::health_review::policy::REVIEW_POLICY_SCHEMA;
use task_core::required_checks::health::CHECKER_SCRIPT;

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

/// 記録した run の文脈のうち、planner の run（`execution_planner` あり）と worker の run の review 方針。
fn seen_policies(
    adapter: &PlannerScriptAdapter,
) -> (
    Vec<Option<task_core::design_review::ReviewPolicy>>,
    Vec<Option<task_core::design_review::ReviewPolicy>>,
) {
    let seen = adapter.seen.lock().unwrap();
    let policies = |planner: bool| -> Vec<_> {
        seen.iter()
            .filter(|c| c.execution_planner.is_some() == planner)
            .map(|c| c.review_policy().cloned())
            .collect()
    };
    (policies(true), policies(false))
}

/// 付記 B2・B3: planner の run と WU の run の `RunContext.workspace.review_policy` に、task の target（main。base）の
/// 検査器の方針が入る。source repo の作業ツリーで検査器を書き換えても（未 commit）、base の版が使われる（I3）。
/// planner の文脈（`execution_planner`）には方針の欄が無い（旧い置き場所を消した）。
#[tokio::test]
async fn health_review_worker_profile_reaches_planner_and_work_unit_runs() {
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
        vec![Some(plan_json(vec![
            wu_spec("a", &[]),
            wu_spec("b", &["a"]),
        ]))],
        HashMap::new(),
    ));
    let mut d = worktree_dispatcher(store.clone(), adapter.clone(), root.path(), None);
    d.config.execution.gate = task_core::GateMode::On;
    run_until_idle(&mut d, 400).await;

    let (planner, workers) = seen_policies(&adapter);
    assert!(!planner.is_empty(), "a planner run was dispatched");
    assert_eq!(workers.len(), 2, "both work units ran");
    for policy in planner.iter().chain(&workers) {
        assert_eq!(policy.as_ref(), Some(&expected("base-class")));
    }
    let planner_json = serde_json::to_value(task_worker::protocol::ExecutionPlannerContext {
        max_work_units: 1,
        ..Default::default()
    })
    .unwrap();
    assert!(
        planner_json.get("review_policy").is_none(),
        "{planner_json}"
    );
}

/// 付記 B3: 計画の無い atomic task の worker run にも base 側の方針が入る。
#[tokio::test]
async fn health_review_worker_profile_reaches_the_atomic_run() {
    let repo = checker_repo(&stub_printing("builtin", "generic"));
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = git_task(
        repo.path(),
        None,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
    );
    store.insert(&task).unwrap();
    let adapter = Arc::new(PlannerScriptAdapter::new(Vec::new(), HashMap::new()));
    let mut d = worktree_dispatcher(store.clone(), adapter.clone(), root.path(), None);
    run_until_idle(&mut d, 200).await;

    let (planner, workers) = seen_policies(&adapter);
    assert!(planner.is_empty(), "an atomic task has no planner run");
    assert_eq!(workers, vec![Some(expected("generic"))]);
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
}

/// 付記 B5: 方針を読めない repo の run も止めずに走らせ、`review_policy` は `None`（repo 固有の節は出ない）。
/// tick は target だけを決める（`review_policy_target`）: `source = absent` と非 0 exit の検査器は target が決まり、
/// 読み込みで `Absent`・`Unavailable` になる。git の repo の無い作業場所は `Ok(None)`、remote は `Err`（D1: `Absent` に
/// しない）で、remote の run の文脈には方針が入らない。
#[tokio::test]
async fn health_review_worker_profile_unreadable_policy_stays_out_of_the_run() {
    let absent = checker_repo(&stub_printing("absent", "unused"));
    let failing = checker_repo("import sys\nsys.stderr.write('boom\\n')\nsys.exit(3)\n");
    let plain = tempfile::tempdir().unwrap();
    for (dir, has_target) in [
        (absent.path(), true),
        (failing.path(), true),
        (plain.path(), false),
    ] {
        let root = tempfile::tempdir().unwrap();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
        let task = git_task(
            dir,
            None,
            Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
        );
        store.insert(&task).unwrap();
        let adapter = Arc::new(PlannerScriptAdapter::new(Vec::new(), HashMap::new()));
        let mut d = worktree_dispatcher(store.clone(), adapter.clone(), root.path(), None);
        let target = d.review_policy_target(&task);
        assert!(
            matches!(&target, Ok(t) if t.is_some() == has_target),
            "{dir:?}: {target:?}"
        );
        run_until_idle(&mut d, 200).await;
        let (_, workers) = seen_policies(&adapter);
        assert_eq!(workers, vec![None], "{dir:?}");
        let status = store.get(task.id).unwrap().unwrap().status;
        assert_eq!(status, Status::Done, "{dir:?}");
    }

    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter = Arc::new(PlannerScriptAdapter::new(Vec::new(), HashMap::new()));
    let d = worktree_dispatcher(store, adapter, root.path(), None);
    let mut remote = compound_task(plain.path());
    remote.workspace = WorkspaceSpec::Remote {
        cluster: "pegasus".into(),
        path: "/work/x".into(),
        mode: None,
    };
    let extras = d.run_extras(&remote, None, None, "claude-code").unwrap();
    assert_eq!(
        extras.workspace.policy_target,
        Err("remote workspace".to_string())
    );
    let context = extras.workspace.into_context(remote.id).await;
    assert_eq!(context.and_then(|w| w.review_policy), None);
}
