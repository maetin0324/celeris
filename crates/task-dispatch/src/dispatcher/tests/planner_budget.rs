//! ADR-0074「R7-11 実装時の明確化」: planner run は（最初の計画も replan も）`[execution.planner] max_turns /
//! max_wall_secs` を、WU（leaf）の run は ADR-0072 D18 の予算を、ワーカーに渡す `RunRequest.task.budget`
//! （claude の `--max-turns` の元）で受け取る。本番（root task 01M3PAX6…、/3、task の予算 10 turns / 600 s）では
//! `run_worker` が DB から task を読み直すので、どの planner run も task の 10 turns で走って `error_max_turns` で
//! 落ちていた。すべて偽のアダプタと一時ディレクトリだけで、外部ネットワークに出ない。

use std::collections::VecDeque;

use super::tree::{assert_replay_is_clean, leaf, stage, tree_dispatcher, v3_plan};
use super::*;

/// 1 本の run が受け取った要求の要点。
#[derive(Debug, Clone)]
struct SeenRun {
    planner: bool,
    replan: bool,
    budget: Budget,
    wall_clock: Duration,
}

/// planner run には計画の列を順に書き（`flag` の 2 番目の値の本目の planner run では `flag` も作る）、worker の
/// run には `result.json` を書く。すべての run の予算を記録する。
struct BudgetAdapter {
    plans: StdMutex<VecDeque<String>>,
    seen: StdMutex<Vec<SeenRun>>,
    flag: Option<(std::path::PathBuf, usize)>,
}

impl BudgetAdapter {
    fn new(plans: Vec<String>, flag: Option<(std::path::PathBuf, usize)>) -> Self {
        BudgetAdapter {
            plans: StdMutex::new(plans.into_iter().collect()),
            seen: StdMutex::new(Vec::new()),
            flag,
        }
    }

    fn seen(&self) -> Vec<SeenRun> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait]
impl WorkerAdapter for BudgetAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let planner = req.context.execution_planner.as_ref();
        let planner_index = {
            let mut seen = self.seen.lock().unwrap();
            seen.push(SeenRun {
                planner: planner.is_some(),
                replan: planner.is_some_and(|p| p.replan),
                budget: req.task.budget,
                wall_clock: limits.wall_clock,
            });
            seen.iter().filter(|s| s.planner).count()
        };
        std::fs::create_dir_all(&req.artifacts_dir).ok();
        if planner.is_some() {
            if let Some(json) = self.plans.lock().unwrap().pop_front() {
                std::fs::write(req.artifacts_dir.join("execution-plan.json"), json)
                    .expect("write execution-plan.json");
            }
            if let Some((flag, at)) = &self.flag
                && planner_index == *at
            {
                std::fs::write(flag, "fixed").expect("write flag");
            }
        } else {
            std::fs::write(
                req.artifacts_dir.join("result.json"),
                serde_json::json!({"summary": "ok", "evidence": []}).to_string(),
            )
            .expect("write result.json");
        }
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: "ok".into(),
                evidence: vec![],
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

/// 本番の root の形: 人の明示の compound、task の予算は 10 turns / 600 s（`[execution.planner]` の既定 24 / 900 より
/// 小さい）。
fn production_root(dir: &std::path::Path, check: Check) -> Task {
    let mut root = new_task(dir, check, 2);
    root.title = "browser capability".into();
    root.routing = Some(task_core::TaskRouting {
        execution_hint: Some(task_core::ExecutionHintSpec {
            mode: task_core::ExecutionMode::Compound,
            explicit: true,
        }),
        ..Default::default()
    });
    root.budget.max_turns = 10;
    root.budget.max_wall_secs = 600;
    root
}

/// planner run はすべて `[execution.planner]` の予算、leaf の run はすべて D18 の既定（`max(task, 30 / 1800)`）。
fn assert_budgets(d: &Dispatcher, seen: &[SeenRun]) {
    let planner = &d.config.execution.planner;
    assert_eq!((planner.max_turns, planner.max_wall_secs), (24, 900));
    for (i, s) in seen.iter().enumerate() {
        let (turns, wall) = if s.planner { (24, 900) } else { (30, 1800) };
        assert_eq!(
            (s.budget.max_turns, s.budget.max_wall_secs),
            (turns, wall),
            "run {i} ({s:?}): RunRequest.task.budget must carry the dispatcher's budget"
        );
        assert_eq!(s.wall_clock, Duration::from_secs(wall), "run {i} ({s:?})");
    }
}

fn transition_reasons(store: &Arc<dyn TaskStore>, id: TaskId) -> Vec<String> {
    store
        .events_for(id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::Transitioned { reason, .. } => Some(reason),
            _ => None,
        })
        .collect()
}

/// 本番の再現 1: /3 の計画を持つ root の最終レビューが不合格（`review_fail`）→ replan の planner run。最初の計画の
/// planner run も replan の planner run も 24 turns / 900 s で走り、leaf の run は 30 turns / 1800 s で走る
/// （修正前はすべて task の 10 turns / 600 s が `RunRequest` に載っていた）。
#[tokio::test]
async fn review_fail_replan_planner_gets_the_planner_budget() {
    let dir = tempfile::tempdir().unwrap();
    let flag = dir.path().join("fixed.flag");
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = production_root(
        dir.path(),
        Check::Command {
            cmd: format!("test -f {}", flag.display()),
            expect_exit: 0,
        },
    );
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let v1 = v3_plan(vec![stage("s1", false)], vec![leaf("a", "s1", &[])]);
    let v2 = v3_plan(vec![stage("s1", false)], vec![leaf("b", "s1", &[])]);
    // 2 本目の planner run（review_fail の後の replan）が flag を作る = 2 回目の審査が通る。
    let adapter = Arc::new(BudgetAdapter::new(vec![v1, v2], Some((flag.clone(), 2))));
    let mut d = tree_dispatcher(&store, adapter.clone());
    d.config.retry_backoff_base = Duration::ZERO;
    d.config.retry_backoff_max = Duration::ZERO;
    let report = run_until_idle(&mut d, 1500).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(root_id).unwrap().unwrap();
    let reasons = transition_reasons(&store, root_id);
    assert_eq!(stored.status, Status::Done, "{stored:?} {reasons:?}");
    assert!(reasons.iter().any(|r| r == "review_fail"), "{reasons:?}");
    let seen = adapter.seen();
    let planners: Vec<&SeenRun> = seen.iter().filter(|s| s.planner).collect();
    assert_eq!(planners.len(), 2, "{seen:?}");
    assert!(!planners[0].replan && planners[1].replan, "{seen:?}");
    assert!(seen.iter().any(|s| !s.planner), "{seen:?}");
    assert_budgets(&d, &seen);
    // DB の task の予算は変えない（ワーカーに渡す写しだけ）。
    assert_eq!(stored.budget.max_turns, 10);
    assert_eq!(stored.budget.max_wall_secs, 600);
    assert_replay_is_clean(&store);
}

/// 本番の再現 2: leaf の失敗（検査が落ち続ける）→ R2b の replan の planner が 2 回不正 → `plan_invalid` の決定 →
/// 人が `replan` と答える → planner がもう一度走り、版 2 が採用されて done。最初の計画・child-failure の replan・
/// 不正な計画の後の再試行・`plan_invalid` の `replan` の回答の後の planner の 4 本とも 24 turns / 900 s。
#[tokio::test]
async fn plan_invalid_replan_answer_planner_gets_the_planner_budget() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = production_root(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
    );
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let mut failing = leaf("a", "s1", &[]);
    failing["checks"] = serde_json::json!([{"cmd": "false", "expect_exit": 0}]);
    let v1 = v3_plan(vec![stage("s1", false)], vec![failing]);
    let mut bad = leaf("d", "s1", &[]);
    bad.as_object_mut().unwrap().remove("checks");
    let bad_plan = v3_plan(vec![stage("s1", false)], vec![bad]);
    let v2 = v3_plan(vec![stage("s1", false)], vec![leaf("d", "s1", &[])]);
    let adapter = Arc::new(BudgetAdapter::new(
        vec![v1, bad_plan.clone(), bad_plan, v2],
        None,
    ));
    let mut d = tree_dispatcher(&store, adapter.clone());
    // `plan_invalid` で止まるまで回す（決定を待つ root は `ready` のまま残るので idle にはならない）。
    for _ in 0..1500 {
        d.tick().unwrap();
        let asked = store
            .decisions_list(Some(root_id))
            .unwrap()
            .iter()
            .any(|r| r.kind == task_core::DecisionKind::PlanInvalid);
        if asked && store.list(Some(Status::Running)).unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let invalid = store
        .decisions_list(Some(root_id))
        .unwrap()
        .into_iter()
        .rfind(|r| r.key == task_core::tree::PLAN_INVALID_DECISION_KEY)
        .expect("plan_invalid decision");
    assert_eq!(invalid.status, task_core::DecisionStatus::Open);
    assert_eq!(adapter.seen().iter().filter(|s| s.planner).count(), 3);
    let outcome = task_ops::decision::answer(
        store.as_ref(),
        &invalid.id,
        Some("replan"),
        Some("write the docs leaf with a check"),
        "human",
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    assert_eq!(outcome.effect, task_core::DecisionEffect::Replan);
    let report = run_until_idle(&mut d, 1500).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Done);
    assert_eq!(
        store
            .execution_plan_active(root_id)
            .unwrap()
            .unwrap()
            .version,
        2
    );

    let seen = adapter.seen();
    let planners: Vec<&SeenRun> = seen.iter().filter(|s| s.planner).collect();
    assert_eq!(planners.len(), 4, "{seen:?}");
    assert!(!planners[0].replan, "{seen:?}");
    assert!(planners[1..].iter().all(|p| p.replan), "{seen:?}");
    assert_budgets(&d, &seen);
    assert_replay_is_clean(&store);
}
