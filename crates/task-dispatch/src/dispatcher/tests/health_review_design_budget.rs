//! ADR 2026-11-07-health-review-integration の付記（二度目の final review の差し戻しへの方針）C1: 設計 review の
//! reviewer run も、`dispatch_one` の worker・planner の run と同じ木の run・費用の上限（`tree_limit_hold_for`）に掛ける。
//! 試験は高 risk の計画の設計 review を待つ木の root（`DesignReviewRequested` を積んだ状態）と、先に積んだ木の run
//! （runs 索引の行。model と token の数から単価で費用を出す）から始め、`tick` を直接回す。時計に依存しない。
//! 外部ネットワークに出ない。
//!
//! 付記 C2〜C4: 設計 review の run にも task 木の token 予算の watch（`token_budget::run_watch`）を付け、run と sink が
//! 同じ watch を持つ。止める処理は final review と共通の `review::run_within_token_budget`。偽の adapter の
//! `UsageProgress` で駆動し、run の終わりは spawn した run の handle で待つ（sleep で待たない）。

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
    let adapter = Arc::new(PlannerScriptAdapter::new(vec![], HashMap::new()));
    let seen = adapter.seen.clone();
    pending_design_review_with(runs, limits, None, adapter, seen)
}

/// `pending_design_review` に task の token 予算（`Budget.max_tokens`）と adapter を足したもの。
fn pending_design_review_with(
    runs: &[[u64; 4]],
    limits: impl FnOnce(&mut task_core::TreeLimits),
    max_tokens: Option<u64>,
    adapter: Arc<dyn WorkerAdapter>,
    seen: Arc<StdMutex<Vec<task_worker::RunContext>>>,
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
    task.budget.max_tokens = max_tokens;
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

/// input だけの累積 usage（`Usage::budget_tokens` は input+output+cache）。
fn input_tokens(n: u64) -> task_core::Usage {
    task_core::Usage {
        input_tokens: Some(n),
        output_tokens: Some(0),
        ..Default::default()
    }
}

/// 設計 review の run で `UsageProgress`（累積 `usage` token）を 1 回出す偽の adapter。`hang` なら、その後は止められる
/// まで終わらない（watch の出来事だけで止まる）。そうでなければ `done` で終わる。届いた文脈を `seen` に残す。
struct UsageAdapter {
    usage: u64,
    hang: bool,
    seen: Arc<StdMutex<Vec<task_worker::RunContext>>>,
}

#[async_trait]
impl WorkerAdapter for UsageAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        self.seen.lock().unwrap().push(req.context.clone());
        sink.usage_progress(&input_tokens(self.usage));
        if self.hang {
            std::future::pending::<()>().await;
        }
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: "design review".into(),
                evidence: vec![],
                usage: Some(input_tokens(self.usage)),
            },
            exit_code: Some(0),
        })
    }
}

/// 高 risk の計画の設計 review を待つ木の root（task の token 予算 `max_tokens`）。adapter は `UsageAdapter`。
fn budgeted(runs: &[[u64; 4]], max_tokens: Option<u64>, usage: u64, hang: bool) -> Fixture {
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(UsageAdapter {
        usage,
        hang,
        seen: seen.clone(),
    });
    pending_design_review_with(runs, |_| {}, max_tokens, adapter, seen)
}

impl Fixture {
    /// 走っている run の handle の終わりを待つ（出来事待ち。30 秒は失敗したときだけ効く保険）。終わりの記録は次の tick。
    async fn join_running(&mut self) {
        for entry in self.d.running.values_mut() {
            tokio::time::timeout(Duration::from_secs(30), &mut entry.handle)
                .await
                .expect("the design review run ends")
                .unwrap();
        }
    }
    /// 設計 review の run の終わり（`WorkerFinished { role: reviewer }`）の `(end, outcome, usage)`。
    fn reviewer_finished(
        &self,
    ) -> Vec<(Option<task_core::RunEnd>, String, Option<task_core::Usage>)> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                Event::WorkerFinished {
                    role: Some(RunRole::Reviewer),
                    end,
                    outcome,
                    usage,
                    ..
                } => Some((end, outcome, usage)),
                _ => None,
            })
            .collect()
    }
}

const TOKENS_END: Option<task_core::RunEnd> = Some(task_core::RunEnd::BudgetExhausted {
    kind: task_core::BudgetKind::Tokens,
});

/// C2: `run_watch` は task 木（予算を持つ task とその子孫）の全 run の消費を前置きにし、`exclude_run` の行は数えない。
/// token 予算の無い task には watch を作らない。
#[tokio::test]
async fn health_review_design_budget_run_watch_reads_the_tree_budget() {
    let f = budgeted(
        &[[1_000, 200, 300, 0], [500, 0, 0, 0]],
        Some(5_000),
        0,
        false,
    );
    let watch = crate::dispatcher::token_budget::run_watch(f.store.as_ref(), &f.task, None)
        .unwrap()
        .expect("a task with a token budget gets a watch");
    assert!(!watch.exceeded(), "2,000 used < 5,000");
    watch.observe(&input_tokens(2_999));
    assert!(!watch.exceeded(), "2,000 + 2,999 < 5,000");
    watch.observe(&input_tokens(3_000));
    assert!(watch.exceeded(), "2,000 + 3,000 reaches 5,000");
    let without_w1 =
        crate::dispatcher::token_budget::run_watch(f.store.as_ref(), &f.task, Some("w1"))
            .unwrap()
            .unwrap();
    without_w1.observe(&input_tokens(3_000));
    assert!(
        !without_w1.exceeded(),
        "w1 (500) is not counted: 1,500 + 3,000 < 5,000"
    );
    let free = budgeted(&[[1_000, 0, 0, 0]], None, 0, false);
    assert!(
        crate::dispatcher::token_budget::run_watch(free.store.as_ref(), &free.task, None)
            .unwrap()
            .is_none(),
        "no token budget, no watch"
    );
}

/// C2: `pick_reviewer` は渡された watch を `ReviewerRun` と sink の両方に付ける（同じ `Arc`。sink の `UsageProgress`
/// が run の watch を鳴らす）。`Ok(None)`（予算の無い task）では付けない。final review の読み取りの error は今のとおり
/// warn を出して watch 無しで走る。
#[tokio::test]
async fn health_review_design_budget_reviewer_run_and_sink_share_the_watch() {
    let mut f = budgeted(&[], Some(1_000), 0, false);
    let watch = Arc::new(crate::dispatcher::token_budget::TokenBudgetWatch::new(
        0, 1_000,
    ));
    let pick =
        f.d.pick_reviewer(&f.task, "subject", Ok(Some(watch.clone())))
            .expect("a reviewer route");
    let run_watch = pick
        .run
        .token_budget
        .as_ref()
        .expect("the run has the watch");
    assert!(
        Arc::ptr_eq(run_watch, &watch),
        "the run holds the given watch"
    );
    pick.run.sink.usage_progress(&input_tokens(1_000));
    assert!(watch.exceeded(), "the sink observes into the same watch");
    let none = f.d.pick_reviewer(&f.task, "subject", Ok(None)).unwrap();
    assert!(none.run.token_budget.is_none(), "no token budget, no watch");
    let failed =
        f.d.pick_reviewer(&f.task, "subject", Err(StoreError::Invalid("x".into())))
            .unwrap();
    assert!(failed.run.token_budget.is_none(), "read error: no watch");
}

/// C2・C3: 上限の内で起きた設計 review の run は token 予算の watch の下で走る。偽の adapter が `UsageProgress` で予算を
/// 超えると run は止まり、`BudgetExhausted{Tokens}`（usage は最後の累積値）で終わる。C4: その run は `done` で終わらない
/// run と同じく判定にならず、起こし直しの run は予算が尽きているので adapter を起こさない。
#[tokio::test]
async fn health_review_design_budget_usage_over_budget_stops_the_design_review_run() {
    let mut f = budgeted(&[[1_000, 0, 0, 0]], Some(5_000), 4_000, true);
    f.d.tick().unwrap();
    assert_eq!(f.reviewer_started(), 1, "the design review run starts");
    f.join_running().await;
    f.d.tick().unwrap();
    let finished = f.reviewer_finished();
    let (end, outcome, usage) = finished.first().expect("the stopped run is recorded");
    assert_eq!(*end, TOKENS_END, "{outcome}");
    assert!(outcome.contains("5000/5000"), "{outcome}");
    assert_eq!(usage.and_then(|u| u.input_tokens), Some(4_000));
    assert!(
        !f.events()
            .iter()
            .any(|e| matches!(e, Event::DesignReviewed { .. })),
        "a stopped run gives no verdict"
    );
    assert_eq!(
        f.seen.lock().unwrap().len(),
        1,
        "only the first run reaches the adapter"
    );
}

/// 予算の無い task の設計 review の run は watch 無しで走る: 同じ `UsageProgress` を出しても止まらず `done` で終わる。
#[tokio::test]
async fn health_review_design_budget_without_budget_runs_to_done() {
    let mut f = budgeted(&[[1_000, 0, 0, 0]], None, 4_000, false);
    f.d.tick().unwrap();
    f.join_running().await;
    f.d.tick().unwrap();
    let finished = f.reviewer_finished();
    let (end, outcome, _) = finished.first().expect("the run is recorded");
    assert_eq!(*end, Some(task_core::RunEnd::Completed), "{outcome}");
}

/// C3・C4: 予算が既に尽きていれば、設計 review の run は adapter を起こさず `BudgetExhausted{Tokens}` で終わる。
/// 起こし直しも同じで、`max_reviewer_retries`（1）を超えると `DesignReviewed { pass: false }` が 1 件残る。
#[tokio::test]
async fn health_review_design_budget_exhausted_budget_never_starts_the_adapter() {
    let mut f = budgeted(&[[6_000, 0, 0, 0]], Some(5_000), 1, false);
    f.d.config.max_reviewer_retries = 1;
    for _ in 0..8 {
        f.d.tick().unwrap();
        if f.events()
            .iter()
            .any(|e| matches!(e, Event::DesignReviewed { .. }))
        {
            break;
        }
        f.join_running().await;
    }
    assert!(
        f.seen.lock().unwrap().is_empty(),
        "no run reaches the adapter"
    );
    let finished = f.reviewer_finished();
    assert_eq!(finished.len(), 2, "{finished:?}");
    for (end, outcome, _) in &finished {
        assert_eq!(*end, TOKENS_END, "{outcome}");
        assert!(outcome.contains("6000/5000"), "{outcome}");
    }
    let verdicts: Vec<bool> = f
        .events()
        .into_iter()
        .filter_map(|e| match e {
            Event::DesignReviewed { pass, .. } => Some(pass),
            _ => None,
        })
        .collect();
    assert_eq!(verdicts, vec![false]);
}
