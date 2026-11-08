//! ADR 2026-11-07-health-review-integration の付記（二度目の final review の差し戻しへの方針）C1: 設計 review の
//! reviewer run も、`dispatch_one` の worker・planner の run と同じ木の run・費用の上限（`tree_limit_hold_for`）に掛ける。
//! 試験は高 risk の計画の設計 review を待つ木の root（`DesignReviewRequested` を積んだ状態）と、先に積んだ木の run
//! （runs 索引の行。model と token の数から単価で費用を出す）から始め、`tick` を直接回す。時計に依存しない。
//! 外部ネットワークに出ない。

use super::*;

use super::health_review_design_gate::{DIGEST, STORE_PATH, checker_repo, stub, v3_plan_with};
use super::tree_gate::seed_tree_run;

struct Fixture {
    store: Arc<dyn TaskStore>,
    d: Dispatcher,
    task: Task,
    seen: Arc<StdMutex<Vec<task_worker::RunContext>>>,
    _repo: tempfile::TempDir,
    _root: tempfile::TempDir,
}

impl Fixture {
    fn events(&self) -> Vec<Event> {
        self.store
            .events_for(self.task.id)
            .unwrap()
            .into_iter()
            .map(|(_, e)| e)
            .collect()
    }
    /// 設計 review の run の開始（`WorkerStarted { role: reviewer }`）の数。
    fn reviewer_started(&self) -> usize {
        self.events()
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    Event::WorkerStarted {
                        role: Some(RunRole::Reviewer),
                        ..
                    }
                )
            })
            .count()
    }
    /// runs 索引の Reviewer の行の数。
    fn reviewer_rows(&self) -> usize {
        self.store
            .runs_for_task(self.task.id)
            .unwrap()
            .iter()
            .filter(|r| r.role == task_core::RunIndexRole::Reviewer)
            .count()
    }
    /// 木に出た `kind: limit` の決定の key（`DecisionRequested` の数と決定の行が 1 対 1 であることも確かめる）。
    fn limit_decisions(&self) -> Vec<String> {
        let requested = self
            .events()
            .iter()
            .filter(|e| matches!(e, Event::DecisionRequested { .. }))
            .count();
        let rows = self.store.decisions_list(Some(self.task.id)).unwrap();
        assert_eq!(requested, rows.len(), "{rows:?}");
        rows.iter()
            .map(|r| {
                assert_eq!(r.kind, task_core::DecisionKind::Limit, "{r:?}");
                r.key.clone()
            })
            .collect()
    }
    /// 上限で止めた状態の確認: 設計 review の run も他の run も起きず、`key` の決定が木に 1 件だけある。
    fn assert_held(&self, key: &str) {
        assert_eq!(self.reviewer_started(), 0, "no design review run starts");
        assert_eq!(self.reviewer_rows(), 0, "no reviewer row in the runs index");
        assert!(
            self.seen.lock().unwrap().is_empty(),
            "no run reaches the adapter"
        );
        assert_eq!(self.limit_decisions(), vec![key.to_string()]);
    }
}

/// 高 risk の計画（`STORE_PATH` に触れ、brief が `new_state` を宣言する）の設計 review を待つ木の root。`runs` の
/// run（`claude-sonnet-5` の 4 種の token）を先に木に積み、`limits` で木の上限を決める。
fn pending_design_review(
    runs: &[[u64; 4]],
    limits: impl FnOnce(&mut task_core::TreeLimits),
) -> Fixture {
    let repo = checker_repo(&stub("base"));
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = compound_task(repo.path());
    task.workspace = WorkspaceSpec::Local {
        path: repo.path().to_path_buf(),
        mode: None,
    };
    task.tree = Some(task_core::TreeInfo::root(task.id));
    store.create_task(&task, vec![]).unwrap();
    for (i, tokens) in runs.iter().enumerate() {
        seed_tree_run(&store, &task, &format!("w{i}"), "claude-sonnet-5", *tokens);
    }
    let spec: task_core::ExecutionPlanSpec =
        serde_json::from_str(&v3_plan_with(STORE_PATH, true)).unwrap();
    store
        .append_event(
            task.id,
            &Event::DesignReviewRequested {
                plan_sha256: task_core::execution_plan::plan_sha256(&spec),
                plan: Box::new(spec),
                risk: task_core::design_review::RiskAssessment {
                    level: task_core::design_review::RiskLevel::High,
                    reasons: vec!["path_class:store+new_state".to_string()],
                },
                policy_sha256: Some(DIGEST.to_string()),
            },
        )
        .unwrap();
    let adapter = Arc::new(PlannerScriptAdapter::new(vec![], HashMap::new()));
    let seen = adapter.seen.clone();
    let mut d = worktree_dispatcher(store.clone(), adapter, root.path(), None);
    d.config.execution.gate = task_core::GateMode::On;
    d.config.execution.limits = super::tree::tree_limits();
    limits(&mut d.config.execution.limits.tree);
    Fixture {
        store,
        d,
        task,
        seen,
        _repo: repo,
        _root: root,
    }
}

/// 1M input の run 1 本（`claude-sonnet-5` で 2.0 USD）。
const TWO_USD: [u64; 4] = [1_000_000, 0, 0, 0];

/// C1: 木の run 数が `max_tree_runs` に達していれば、設計 review の run（Reviewer の `WorkerStarted`・runs 索引の行）は
/// 起きず、`limit:max_tree_runs` の決定が木に 1 件だけ出る。次の tick でも run は起きず、決定は増えない。
#[tokio::test]
async fn health_review_design_budget_tree_runs_limit_holds_the_design_review_run() {
    let mut f = pending_design_review(&[[1_000, 1_000, 0, 0], [1_000, 1_000, 0, 0]], |t| {
        t.max_tree_runs = 2;
    });
    f.d.tick().unwrap();
    f.assert_held("limit:max_tree_runs");
    f.d.tick().unwrap();
    f.assert_held("limit:max_tree_runs");
}

/// C1: 木の上限に token 数の種類は無い。token の上限は、token の使用量から単価で計算した費用の上限（`max_tree_usd`）
/// で掛かる。木の費用（2.0 + cache read 2M の 0.4 = 2.4 USD）が上限（2 USD）を超えていれば、run 数に余裕があっても
/// 設計 review の run は起きず、`limit:max_tree_usd` の決定が木に 1 件だけ出る（費用不明の run は無い）。
#[tokio::test]
async fn health_review_design_budget_tree_usd_limit_holds_the_design_review_run() {
    let mut f = pending_design_review(&[TWO_USD, [0, 0, 2_000_000, 0]], |t| {
        t.max_tree_runs = 10;
        t.max_tree_usd_micros = Some(2_000_000);
    });
    f.d.tick().unwrap();
    f.assert_held("limit:max_tree_usd");
    let question = f.store.decisions_list(Some(f.task.id)).unwrap()[0]
        .request
        .question
        .clone();
    assert!(!question.contains("費用不明"), "{question}");
    f.d.tick().unwrap();
    f.assert_held("limit:max_tree_usd");
}

/// C1: 上限の内（run 数 1 < 2、費用 2.0 < 3 USD）なら設計 review の run が起きる。開始の tick で Reviewer の
/// `WorkerStarted` と runs 索引の行が書かれ、決定は出ない。reviewer が受け取る文脈は `ReviewRequest.design` で、
/// 方針は base の検査器の出した `definition_sha256` のもの。
#[tokio::test]
async fn health_review_design_budget_within_limits_starts_the_design_review_run() {
    let mut f = pending_design_review(&[TWO_USD], |t| {
        t.max_tree_runs = 2;
        t.max_tree_usd_micros = Some(3_000_000);
    });
    f.d.tick().unwrap();
    assert_eq!(f.reviewer_started(), 1, "the design review run starts");
    assert_eq!(f.reviewer_rows(), 1, "the runs index has the reviewer row");
    assert!(f.limit_decisions().is_empty(), "no limit decision");
    run_until_idle(&mut f.d, 400).await;
    let seen = f.seen.lock().unwrap().clone();
    let design = seen
        .iter()
        .find_map(|c| c.review.as_ref()?.design.as_deref())
        .expect("the reviewer run gets the design review context");
    assert_eq!(design.risk.level, task_core::design_review::RiskLevel::High);
    assert_eq!(
        design
            .review_policy
            .as_ref()
            .map(|p| p.definition_sha256.as_str()),
        Some(DIGEST)
    );
}
