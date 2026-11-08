//! ADR 2026-11-07-health-review-integration D3.3・D3.5・D3.6: 計画の採用前の risk 判定と、高 risk の計画だけの設計
//! review。base の検査器は一時 git repo に commit した stub の `repo_health.py`（`--print-review-policy` の JSON を
//! 出す）で置き換える。reviewer run は試験の adapter が受け、`RunContext.review.design` の有無で設計 review を見分ける。
//! 外部ネットワークに出ない。

use super::*;

use crate::health_review::policy::{CHECKER_SCRIPT, REVIEW_POLICY_SCHEMA};

pub(super) const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
pub(super) const STORE_PATH: &str = "crates/task-store/src/rows.rs";

/// `source` の方針（高 risk の class `store` = `^crates/task-store/`）を出す stub。
pub(super) fn stub(source: &str) -> String {
    let json = serde_json::json!({
        "schema": REVIEW_POLICY_SCHEMA,
        "source": source,
        "high_risk": [{"class": "store", "paths": ["^crates/task-store/"]}],
        "invariants": {},
        "definition_sha256": DIGEST,
    });
    format!("import sys\nsys.stdout.write({:?})\n", json.to_string())
}

const FAILING_STUB: &str = "import sys\nsys.stderr.write('boom\\n')\nsys.exit(3)\n";

pub(super) fn checker_repo(stub: &str) -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let script = repo.path().join(CHECKER_SCRIPT);
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(&script, stub).unwrap();
    git_out(repo.path(), &["add", "-A"]);
    git_out(repo.path(), &["commit", "-q", "-m", "checker"]);
    repo
}

/// /2 の計画（WorkUnit 1 つ、write path は `path`）。
fn v2_plan(path: &str) -> String {
    let mut wu = wu_spec("a", &[]);
    wu.context.paths = vec![path.to_string()];
    plan_json(vec![wu])
}

/// /3 の計画（leaf 1 つ、write path は `path`）。`new_state` を宣言した brief を付けるかを選ぶ。
pub(super) fn v3_plan_with(path: &str, brief: bool) -> String {
    let mut unit = super::tree::leaf("a", "s1", &[]);
    unit["context"] = serde_json::json!({"paths": [path]});
    let mut plan: serde_json::Value = serde_json::from_str(&super::tree::v3_plan(
        vec![super::tree::stage("s1", false)],
        vec![unit],
    ))
    .unwrap();
    if brief {
        plan["design_brief"] = serde_json::json!({
            "root_cause": "rows are rewritten in place",
            "owner": "task-store",
            "options": [
                {"name": "append a column", "chosen": true, "reason": "keeps history"},
                {"name": "rewrite rows", "chosen": false, "reason": "loses history"}
            ],
            "new_state": ["rows.revision column"],
        });
    }
    plan.to_string()
}

struct Outcome {
    store: Arc<dyn TaskStore>,
    task: Task,
    seen: Vec<task_worker::RunContext>,
    _repo: tempfile::TempDir,
    _root: tempfile::TempDir,
}

impl Outcome {
    fn planner_runs(&self) -> usize {
        self.seen
            .iter()
            .filter(|c| c.execution_planner.is_some())
            .count()
    }
    fn design_reviews(&self) -> Vec<&task_worker::protocol::DesignReviewRequest> {
        self.seen
            .iter()
            .filter_map(|c| c.review.as_ref()?.design.as_deref())
            .collect()
    }
    fn reviewer_runs(&self) -> usize {
        self.seen.iter().filter(|c| c.review.is_some()).count()
    }
    fn events(&self) -> Vec<Event> {
        self.store
            .events_for(self.task.id)
            .unwrap()
            .into_iter()
            .map(|(_, e)| e)
            .collect()
    }
    fn requested(&self) -> Vec<Event> {
        self.events()
            .into_iter()
            .filter(|e| matches!(e, Event::DesignReviewRequested { .. }))
            .collect()
    }
    fn adopted(&self) -> bool {
        self.store
            .execution_plan_active(self.task.id)
            .unwrap()
            .is_some()
    }
}

/// planner が `plans` を順に出す compound task を、`stub` の検査器を commit した repo で回す。
async fn run(stub: &str, plans: Vec<String>, tree: bool) -> Outcome {
    let repo = checker_repo(stub);
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = compound_task(repo.path());
    task.workspace = WorkspaceSpec::Local {
        path: repo.path().to_path_buf(),
        mode: None,
    };
    store.insert(&task).unwrap();
    let adapter = Arc::new(PlannerScriptAdapter::new(
        plans.into_iter().map(Some).collect(),
        HashMap::new(),
    ));
    let mut d = worktree_dispatcher(store.clone(), adapter.clone(), root.path(), None);
    d.config.execution.gate = task_core::GateMode::On;
    if tree {
        d.config.execution.limits = super::tree::tree_limits();
    }
    run_until_idle(&mut d, 400).await;
    let seen = adapter.seen.lock().unwrap().clone();
    Outcome {
        store,
        task,
        seen,
        _repo: repo,
        _root: root,
    }
}

/// D3.6: 低 risk の計画（高 risk の class に合わない path）は今までどおり採用され、reviewer run は 1 本も増えない。
#[tokio::test]
async fn health_review_design_gate_low_risk_plan_is_adopted_without_a_reviewer_run() {
    let out = run(&stub("base"), vec![v2_plan("notes/plan.md")], false).await;
    assert_eq!(out.planner_runs(), 1);
    assert!(out.adopted(), "the low-risk plan is adopted");
    assert!(out.requested().is_empty(), "no design review is requested");
    assert_eq!(out.reviewer_runs(), 0, "no reviewer run is added");
    assert!(
        out.seen
            .iter()
            .any(|c| c.execution_planner.is_none() && c.review.is_none()),
        "the work unit of the adopted plan ran"
    );
}

/// D3.5: 高 risk の path に触れ brief が `new_state` を宣言した計画は採用されず、`DesignReviewRequested` が書かれ、
/// 設計 review の reviewer run が 1 本だけ起きる（文脈は `ReviewRequest.design` だけ）。その run の終わりは task の
/// final review の判定に流れない（`ReviewVerdict` が無く、WU は起きない）。この試験の reviewer は `review.json` を
/// 書かないので判定は不合格（`DesignReviewed { pass: false }`。design-verdict）で、planner がもう一度起きる（2 回目は
/// 計画を出さず、採用は無い）。
#[tokio::test]
async fn health_review_design_gate_high_risk_plan_requests_a_design_review_before_adoption() {
    let out = run(&stub("base"), vec![v3_plan_with(STORE_PATH, true)], true).await;
    assert_eq!(
        out.planner_runs(),
        2,
        "the planner runs once for the plan and once more after the failed design review"
    );
    assert!(
        !out.adopted(),
        "the high-risk plan is not adopted without a passing design review"
    );
    let requested = out.requested();
    assert_eq!(requested.len(), 1);
    let Event::DesignReviewRequested {
        risk,
        policy_sha256,
        plan,
        ..
    } = &requested[0]
    else {
        unreachable!()
    };
    assert_eq!(risk.level, task_core::design_review::RiskLevel::High);
    assert_eq!(risk.reasons, vec!["path_class:store+new_state".to_string()]);
    assert_eq!(policy_sha256.as_deref(), Some(DIGEST));
    assert!(plan.design_brief.is_some());

    let reviews = out.design_reviews();
    assert_eq!(reviews.len(), 1, "exactly one design review run");
    assert_eq!(out.reviewer_runs(), 1, "no other reviewer run");
    let review = out.seen.iter().find_map(|c| c.review.as_ref()).unwrap();
    assert!(review.summary.is_empty() && review.criteria.is_empty());
    assert_eq!(
        reviews[0].risk.level,
        task_core::design_review::RiskLevel::High
    );
    assert_eq!(
        reviews[0]
            .review_policy
            .as_ref()
            .map(|p| p.definition_sha256.as_str()),
        Some(DIGEST)
    );
    assert!(reviews[0].plan_summary.contains(STORE_PATH));

    let events = out.events();
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::WorkerFinished {
                role: Some(RunRole::Reviewer),
                ..
            }
        )),
        "the design review run is recorded as finished"
    );
    assert!(
        !events.iter().any(|e| matches!(
            e,
            Event::ReviewVerdict { .. } | Event::ExecutionPlanned { .. }
        )),
        "the design review run does not reach the final review verdict or adopt the plan"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::DesignReviewed { pass: false, .. })),
        "a reviewer that writes no design verdict fails the design review"
    );
    assert!(
        out.seen
            .iter()
            .all(|c| c.execution_planner.is_some() || c.review.is_some()),
        "no work unit runs while the design review is pending"
    );
}

/// fail closed: base の検査器が方針を出せない（非 0 exit）なら risk は `unknown` で、低 risk の path の計画でも採用
/// されずに設計 review が起きる。
#[tokio::test]
async fn health_review_design_gate_unavailable_policy_fails_closed_to_a_design_review() {
    let out = run(FAILING_STUB, vec![v2_plan("notes/plan.md")], false).await;
    assert!(!out.adopted());
    let requested = out.requested();
    assert_eq!(requested.len(), 1);
    let Event::DesignReviewRequested {
        risk,
        policy_sha256,
        ..
    } = &requested[0]
    else {
        unreachable!()
    };
    assert_eq!(risk.level, task_core::design_review::RiskLevel::Unknown);
    assert!(
        risk.reasons[0].starts_with("review_policy_unavailable:"),
        "{:?}",
        risk.reasons
    );
    assert_eq!(policy_sha256, &None);
    assert_eq!(
        out.design_reviews().len(),
        1,
        "the design review run starts"
    );
    assert!(out.design_reviews()[0].review_policy.is_none());
}

/// D3.3: brief の無い高 risk の planner の計画は `DesignBriefMissing` で planner の再試行に回る（採用も設計 review も
/// しない）。2 回目の計画（低 risk）は採用される。
#[tokio::test]
async fn health_review_design_gate_missing_brief_retries_the_planner() {
    let out = run(
        &stub("base"),
        vec![
            v3_plan_with(STORE_PATH, false),
            v3_plan_with("notes/plan.md", false),
        ],
        true,
    )
    .await;
    assert_eq!(out.planner_runs(), 2, "the planner is retried once");
    assert!(
        out.events().iter().any(|e| matches!(
            e,
            Event::WorkerProgress { msg, .. } if msg.contains("design_brief is required") && msg.contains("store")
        )),
        "the retry carries the DesignBriefMissing reason"
    );
    assert!(out.requested().is_empty());
    assert_eq!(out.reviewer_runs(), 0);
    assert!(out.adopted(), "the second (low-risk) plan is adopted");
}

/// `[review]` 節の無い repo（`source = absent`）は risk `none` で、高 risk に見える path と brief でも従来どおり採用する。
#[tokio::test]
async fn health_review_design_gate_absent_policy_adopts_as_before() {
    let out = run(&stub("absent"), vec![v3_plan_with(STORE_PATH, true)], true).await;
    assert_eq!(out.planner_runs(), 1);
    assert!(out.adopted());
    assert!(out.requested().is_empty());
    assert_eq!(out.reviewer_runs(), 0);
}
