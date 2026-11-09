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

/// planner run 1 本の振る舞い（ADR 2026-11-09-planner-turn-limit-and-comment-interrupt の試験で run の終わり方を
/// 差し替える。本物の LLM は呼ばない）。
enum PlannerStep {
    /// 計画を書いて `Done` で終わる。
    Plan(String),
    /// 計画を（あれば）書いて turn の上限（claude の `error_max_turns`）で終わる。
    TurnsOut(Option<String>),
    /// 人が comment で割り込む（API と同じ `post_human_comment_as`）。run は abort されるまで返らない。
    CommentInterrupt(Arc<dyn TaskStore>),
}

/// planner run には振る舞いの列を順に当て（`flag` の 2 番目の値の本目の planner run では `flag` も作る）、worker の
/// run には `result.json` を書く。すべての run の予算を記録する。
struct BudgetAdapter {
    steps: StdMutex<VecDeque<PlannerStep>>,
    seen: StdMutex<Vec<SeenRun>>,
    flag: Option<(std::path::PathBuf, usize)>,
}

impl BudgetAdapter {
    fn new(plans: Vec<String>, flag: Option<(std::path::PathBuf, usize)>) -> Self {
        BudgetAdapter {
            steps: StdMutex::new(plans.into_iter().map(PlannerStep::Plan).collect()),
            seen: StdMutex::new(Vec::new()),
            flag,
        }
    }

    fn steps(steps: Vec<PlannerStep>) -> Self {
        BudgetAdapter {
            steps: StdMutex::new(steps.into_iter().collect()),
            seen: StdMutex::new(Vec::new()),
            flag: None,
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
        let done = Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        };
        if planner.is_some() {
            let step = self.steps.lock().unwrap().pop_front();
            let (plan, terminal) = match step {
                Some(PlannerStep::Plan(json)) => (Some(json), done),
                Some(PlannerStep::TurnsOut(plan)) => (
                    plan,
                    Terminal::BudgetExhausted {
                        kind: task_core::BudgetKind::Turns,
                        message: "claude result: error_max_turns".into(),
                        usage: None,
                    },
                ),
                Some(PlannerStep::CommentInterrupt(store)) => {
                    task_ops::comment::post_human_comment_as(
                        store.as_ref(),
                        req.task.id,
                        Some("human:web".to_string()),
                        "計画の前にこの点も見てください".into(),
                        OffsetDateTime::now_utc(),
                    )
                    .expect("post the interrupting comment");
                    std::future::pending::<()>().await;
                    (None, done)
                }
                None => (None, done),
            };
            if let Some(json) = plan {
                std::fs::write(req.artifacts_dir.join("execution-plan.json"), json)
                    .expect("write execution-plan.json");
            }
            if let Some((flag, at)) = &self.flag
                && planner_index == *at
            {
                std::fs::write(flag, "fixed").expect("write flag");
            }
            return Ok(RunOutcome {
                terminal,
                exit_code: Some(0),
            });
        } else {
            std::fs::write(
                req.artifacts_dir.join("result.json"),
                serde_json::json!({"summary": "ok", "evidence": []}).to_string(),
            )
            .expect("write result.json");
        }
        Ok(RunOutcome {
            terminal: done,
            exit_code: Some(0),
        })
    }
}

/// 本番の root の形: 人の明示の compound、task の予算は 10 turns / 600 s（`[execution.planner]` の既定 40 / 1,200 より
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
    // ADR 2026-11-09-planner-turn-limit-and-comment-interrupt D3: 既定は 40 / 1,200（ADR-0074 D5.3 の 24 / 900 から戻した）。
    assert_eq!((planner.max_turns, planner.max_wall_secs), (40, 1200));
    for (i, s) in seen.iter().enumerate() {
        let (turns, wall) = if s.planner { (40, 1200) } else { (30, 1800) };
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
/// planner run も replan の planner run も 40 turns / 1,200 s で走り、leaf の run は 30 turns / 1800 s で走る
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
/// 不正な計画の後の再試行・`plan_invalid` の `replan` の回答の後の planner の 4 本とも 40 turns / 1,200 s。
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

fn planner_finishes(
    store: &Arc<dyn TaskStore>,
    id: TaskId,
) -> Vec<(String, Option<task_core::RunEnd>)> {
    store
        .events_for(id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::WorkerFinished {
                outcome,
                role: Some(RunRole::Planner),
                end,
                ..
            } => Some((outcome, end)),
            _ => None,
        })
        .collect()
}

fn plan_invalid_decisions(store: &Arc<dyn TaskStore>, id: TaskId) -> Vec<task_core::DecisionRow> {
    store
        .decisions_list(Some(id))
        .unwrap()
        .into_iter()
        .filter(|r| r.kind == task_core::DecisionKind::PlanInvalid)
        .collect()
}

/// `dir` の下の拒否した計画の file（`execution-plan.rejected.<run>.attempt-<n>.json`）の名前。
fn rejected_plan_files(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                stack.push(path);
            } else if name.starts_with("execution-plan.rejected.") {
                out.push(name);
            }
        }
    }
    out
}

/// ADR 2026-11-09-planner-turn-limit-and-comment-interrupt D1（本番 01M6YJF7HJ の 2 回目の run）: planner run が turn の
/// 上限（`error_max_turns`）で終わっても、完全な `execution-plan.json` が残っていれば通常の検証にかけて採用する。
/// 守る不変条件: 採否は run の終わり方でなく計画 file の検証で決まり、run の記録（`end`）は上限のまま残る。既存の試験の
/// planner はすべて `Done` で終わるので、上限で終わった run の計画を捨てる（修正前の）挙動を検出できなかった。
#[tokio::test]
async fn planner_budget_turns_out_with_a_complete_plan_is_validated_and_adopted() {
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
    let plan = v3_plan(vec![stage("s1", false)], vec![leaf("a", "s1", &[])]);
    let adapter = Arc::new(BudgetAdapter::steps(vec![PlannerStep::TurnsOut(Some(
        plan,
    ))]));
    let mut d = tree_dispatcher(&store, adapter.clone());
    let report = run_until_idle(&mut d, 1500).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(root_id).unwrap().unwrap();
    assert_eq!(
        stored.status,
        Status::Done,
        "{stored:?} {:?}",
        transition_reasons(&store, root_id)
    );
    assert_eq!(
        store
            .execution_plan_active(root_id)
            .unwrap()
            .unwrap()
            .version,
        1
    );
    let seen = adapter.seen();
    assert_eq!(seen.iter().filter(|s| s.planner).count(), 1, "{seen:?}");
    assert_budgets(&d, &seen);
    let finishes = planner_finishes(&store, root_id);
    assert_eq!(finishes.len(), 1, "{finishes:?}");
    // outcome は採用の文（/3 の計画は unit を `work_units` に持たないので件数は既存の書式どおり）。
    assert!(
        finishes[0].0.starts_with("done: ") && finishes[0].0.ends_with("work unit(s) planned"),
        "{finishes:?}"
    );
    assert_eq!(
        finishes[0].1,
        Some(task_core::RunEnd::BudgetExhausted {
            kind: task_core::BudgetKind::Turns
        }),
        "the run record keeps the turn limit: {finishes:?}"
    );
    assert!(plan_invalid_decisions(&store, root_id).is_empty());
    assert!(rejected_plan_files(dir.path()).is_empty());
    assert_replay_is_clean(&store);
}

/// D1 の裏側: turn の上限で終わった run の計画 file が不正なら（検証は緩めない）拒否して file を移し、file が無ければ
/// 理由に run の終わり方を添えて不正な試行に数える。2 回で /3 の `plan_invalid`（ADR-0079 D9）。
/// 守る不変条件: 上限で終わった run は検証を素通りしない・file の無い run は今どおり試行に数える。
#[tokio::test]
async fn planner_budget_turns_out_without_a_valid_plan_is_an_invalid_attempt() {
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
    let mut bad = leaf("a", "s1", &[]);
    bad.as_object_mut().unwrap().remove("checks");
    let bad_plan = v3_plan(vec![stage("s1", false)], vec![bad]);
    let adapter = Arc::new(BudgetAdapter::steps(vec![
        PlannerStep::TurnsOut(Some(bad_plan)),
        PlannerStep::TurnsOut(None),
    ]));
    let mut d = tree_dispatcher(&store, adapter.clone());
    for _ in 0..1500 {
        d.tick().unwrap();
        if !plan_invalid_decisions(&store, root_id).is_empty()
            && store.list(Some(Status::Running)).unwrap().is_empty()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let decisions = plan_invalid_decisions(&store, root_id);
    assert_eq!(decisions.len(), 1, "{decisions:?}");
    assert_eq!(decisions[0].status, task_core::DecisionStatus::Open);
    let seen = adapter.seen();
    assert_eq!(seen.iter().filter(|s| s.planner).count(), 2, "{seen:?}");
    assert_budgets(&d, &seen);
    let finishes = planner_finishes(&store, root_id);
    assert_eq!(finishes.len(), 2, "{finishes:?}");
    assert!(
        finishes[0]
            .0
            .starts_with("error(retryable=true): invalid execution plan:")
            && !finishes[0].0.contains("が見つからない"),
        "the bad plan was validated and rejected: {finishes:?}"
    );
    assert!(
        finishes[1]
            .0
            .contains("execution-plan.json が見つからない（budget_exhausted(Turns)"),
        "{finishes:?}"
    );
    for (_, end) in &finishes {
        assert_eq!(
            *end,
            Some(task_core::RunEnd::BudgetExhausted {
                kind: task_core::BudgetKind::Turns
            })
        );
    }
    let rejected = rejected_plan_files(dir.path());
    assert_eq!(rejected.len(), 1, "{rejected:?}");
    assert!(rejected[0].ends_with(".attempt-1.json"), "{rejected:?}");
    assert_replay_is_clean(&store);
}

/// ADR 2026-11-09-planner-turn-limit-and-comment-interrupt D2（本番 01M6ZDF9Q4）: 人の comment の割り込みで止まった
/// planner run は、`plan_invalid` に倒す 2 回の試行に数えない。割り込み → turn の上限（file 無し）→ 妥当な計画 の順で、
/// 修正前は 2 本目で「2 回とも不正」として `plan_invalid` を出していた。今は 2 本目が 1 回目の試行で、3 本目で採用される。
/// 守る不変条件: 試行の数は計画の失敗だけを数える。既存の試験は割り込みと planner の試行の数を組み合わせていなかった。
#[tokio::test]
async fn planner_budget_comment_interrupt_is_not_a_planner_attempt() {
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
    let plan = v3_plan(vec![stage("s1", false)], vec![leaf("a", "s1", &[])]);
    let adapter = Arc::new(BudgetAdapter::steps(vec![
        PlannerStep::CommentInterrupt(store.clone()),
        PlannerStep::TurnsOut(None),
        PlannerStep::Plan(plan),
    ]));
    let mut d = tree_dispatcher(&store, adapter.clone());
    d.config.retry_backoff_base = Duration::ZERO;
    d.config.retry_backoff_max = Duration::ZERO;
    let report = run_until_idle(&mut d, 1500).await;
    assert!(report.idle, "{report:?}");

    let decisions = plan_invalid_decisions(&store, root_id);
    assert!(decisions.is_empty(), "{decisions:?}");
    let stored = store.get(root_id).unwrap().unwrap();
    assert_eq!(
        stored.status,
        Status::Done,
        "{stored:?} {:?}",
        transition_reasons(&store, root_id)
    );
    assert_eq!(
        store
            .execution_plan_active(root_id)
            .unwrap()
            .unwrap()
            .version,
        1
    );
    let seen = adapter.seen();
    assert_eq!(seen.iter().filter(|s| s.planner).count(), 3, "{seen:?}");
    assert_budgets(&d, &seen);
    let interrupted = store
        .events_for(root_id)
        .unwrap()
        .into_iter()
        .filter(|(_, e)| {
            matches!(e, Event::WorkerFinished { outcome, .. }
                if outcome == task_ops::comment::INTERRUPTED_OUTCOME)
        })
        .count();
    assert_eq!(interrupted, 1);
    // 不正な試行は turn の上限の 1 本だけ（拒否の進捗が 1 件、試行番号 1）。
    let retries = store
        .events_for(root_id)
        .unwrap()
        .into_iter()
        .filter(|(_, e)| {
            matches!(e, Event::WorkerProgress { msg, .. }
                if msg.starts_with(super::super::planner_flow::PLANNER_RETRY_PREFIX))
        })
        .count();
    assert_eq!(retries, 1);
    assert_replay_is_clean(&store);
}
