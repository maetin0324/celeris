//! ADR 2026-10-20-mlsys-main2-fixes D1-3/D1-4/D5: task 木の token 予算（`Budget.max_tokens`）。
//! 途中の累積 usage での停止・予算到達後の continuation/retry/reviewer の抑止・失敗した reviewer run の usage。
//! LLM は使わない（stub adapter）。停止は時刻ではなく `UsageProgress` の出来事で決まる（sleep に依らない）。
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn tokens(input: u64) -> task_core::Usage {
    task_core::Usage {
        input_tokens: Some(input),
        output_tokens: Some(0),
        cache_read_tokens: None,
        cache_creation_tokens: None,
        cost_usd: None,
        duplicate_reads: None,
        session_resumed: None,
        context_tokens: None,
    }
}

const PASS_REVIEW: &str = r#"{"verdicts":[{"criterion":0,"pass":true,"reason":"ok"}]}"#;

/// worker run では `UsageProgress` を `step` ずつ（累積値で）`steps` 回出し、最後まで止められなければ `done`。
/// 2 行目の後に runs 索引の `usage_json` が途中値になっていることを、adapter 自身が store で確かめる。
struct ProgressAdapter {
    store: Arc<dyn TaskStore>,
    step: u64,
    steps: usize,
    emitted: Arc<AtomicUsize>,
    midway_index_usage: Arc<StdMutex<Option<task_core::Usage>>>,
}

#[async_trait]
impl WorkerAdapter for ProgressAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        _limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        std::fs::create_dir_all(&req.artifacts_dir).unwrap();
        if req.task.kind == TaskKind::Review {
            std::fs::write(req.artifacts_dir.join("review.json"), PASS_REVIEW).unwrap();
            return Ok(RunOutcome {
                terminal: Terminal::Done {
                    summary: "review".into(),
                    evidence: vec![],
                    usage: Some(tokens(1)),
                },
                exit_code: Some(0),
            });
        }
        for i in 1..=self.steps {
            sink.usage_progress(&tokens(self.step * i as u64));
            self.emitted.fetch_add(1, Ordering::SeqCst);
            if i == 2 {
                let row = self.store.run_index_get(run_id).unwrap().unwrap();
                *self.midway_index_usage.lock().unwrap() = row.usage;
            }
            // dispatcher の監視（select）に順番を渡す。止める判定は usage の出来事だけで決まる。
            tokio::task::yield_now().await;
        }
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: "finished".into(),
                evidence: vec![],
                usage: Some(tokens(self.step * self.steps as u64)),
            },
            exit_code: Some(0),
        })
    }
}

fn budget_task(dir: &std::path::Path, max_tokens: u64, check: Check) -> Task {
    let mut task = new_task(dir, check, 2);
    task.budget.max_tokens = Some(max_tokens);
    task
}

fn worker_finished(store: &Arc<dyn TaskStore>, task_id: TaskId) -> Vec<Event> {
    store
        .events_for(task_id)
        .unwrap()
        .into_iter()
        .map(|(_, e)| e)
        .filter(|e| matches!(e, Event::WorkerFinished { .. }))
        .collect()
}

fn assert_runs_replay_clean(store: &Arc<dyn TaskStore>, task_id: TaskId) {
    let event_rows = store.event_rows_for(task_id, None, usize::MAX).unwrap();
    let (_, replayed) = task_ops::replay::rebuild_work_units_and_runs(task_id, &event_rows);
    let stored = store.runs_for_task(task_id).unwrap();
    for row in &stored {
        let replayed_row = replayed
            .iter()
            .find(|r| r.run_id == row.run_id)
            .unwrap_or_else(|| panic!("run {} not replayed", row.run_id));
        assert_eq!(replayed_row.usage, row.usage, "usage of {}", row.run_id);
        assert_eq!(replayed_row.status, row.status, "status of {}", row.run_id);
    }
}

/// D1-3: `max_tokens = 250k`、stub worker が 100k ずつ累積値を出す。3 行目（300k）の後に run が止まり
/// `BudgetExhausted{Tokens}`、task は failed（continuation も reviewer も起きない）。超過は 1 行ぶん以下。
/// run の途中で runs 索引の usage が途中値（200k）になり、終端の usage は最後の累積値。
#[tokio::test]
async fn token_budget_stops_run_midway() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let max = 250_000;
    let step = 100_000;
    let task = budget_task(dir.path(), max, Check::Reviewer);
    let task_id = task.id;
    store.insert(&task).unwrap();
    let emitted = Arc::new(AtomicUsize::new(0));
    let midway = Arc::new(StdMutex::new(None));
    let adapter = Arc::new(ProgressAdapter {
        store: store.clone(),
        step,
        steps: 10,
        emitted: emitted.clone(),
        midway_index_usage: midway.clone(),
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    assert_eq!(
        emitted.load(Ordering::SeqCst),
        3,
        "stopped right after 300k"
    );
    assert_eq!(
        midway.lock().unwrap().map(|u| u.budget_tokens()),
        Some(200_000),
        "runs index carries the in-flight cumulative usage"
    );
    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Failed, "{stored:?}");
    assert_eq!(stored.attempts, 0);

    let finished = worker_finished(&store, task_id);
    assert_eq!(
        finished.len(),
        1,
        "no continuation, no reviewer: {finished:?}"
    );
    let Event::WorkerFinished {
        outcome,
        usage,
        end,
        role,
        ..
    } = &finished[0]
    else {
        unreachable!()
    };
    assert_eq!(*role, None);
    assert_eq!(
        *end,
        Some(task_core::RunEnd::BudgetExhausted {
            kind: task_core::BudgetKind::Tokens
        })
    );
    assert_eq!(outcome, "token_budget_exhausted: 300000/250000");
    let used = usage.expect("stopped run keeps its usage").budget_tokens();
    assert_eq!(used, 300_000);
    assert!(
        used - max <= step,
        "overshoot {} > one response",
        used - max
    );

    let runs = store.runs_for_task(task_id).unwrap();
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(runs[0].usage.map(|u| u.budget_tokens()), Some(300_000));
    assert_eq!(
        runs[0].status,
        task_core::RunIndexStatus::from_run_end(task_core::RunEnd::BudgetExhausted {
            kind: task_core::BudgetKind::Tokens
        })
    );
    assert_runs_replay_clean(&store, task_id);
}

/// 呼ばれるたびに次の終端を返す（worker run）。reviewer run は合格の review.json を書いて usage 1 で done。
struct ScriptedAdapter {
    script: StdMutex<std::collections::VecDeque<Terminal>>,
    worker_runs: Arc<AtomicUsize>,
    reviewer_runs: Arc<AtomicUsize>,
}

impl ScriptedAdapter {
    fn new(script: Vec<Terminal>) -> Self {
        Self {
            script: StdMutex::new(script.into()),
            worker_runs: Arc::new(AtomicUsize::new(0)),
            reviewer_runs: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait]
impl WorkerAdapter for ScriptedAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        std::fs::create_dir_all(&req.artifacts_dir).unwrap();
        if req.task.kind == TaskKind::Review {
            self.reviewer_runs.fetch_add(1, Ordering::SeqCst);
            std::fs::write(req.artifacts_dir.join("review.json"), PASS_REVIEW).unwrap();
            return Ok(RunOutcome {
                terminal: Terminal::Done {
                    summary: "review".into(),
                    evidence: vec![],
                    usage: Some(tokens(1)),
                },
                exit_code: Some(0),
            });
        }
        self.worker_runs.fetch_add(1, Ordering::SeqCst);
        let terminal = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .expect("an unexpected extra worker run");
        Ok(RunOutcome {
            terminal,
            exit_code: Some(0),
        })
    }
}

fn turns_exhausted(input: u64) -> Terminal {
    Terminal::BudgetExhausted {
        kind: task_core::BudgetKind::Turns,
        message: "error_max_turns".into(),
        usage: Some(tokens(input)),
    }
}

fn done(input: u64) -> Terminal {
    Terminal::Done {
        summary: "done".into(),
        evidence: vec![],
        usage: Some(tokens(input)),
    }
}

/// D1-4: turns 切れで終わった run の usage が上限以上なら continuation も reviewer も起きず failed。
/// done で終わっても上限以上なら reviewer は起きない。上限未満なら continuation が 1 本起き
/// （残り予算の内側の再開）、reviewer まで進む。
#[tokio::test]
async fn token_budget_exhausted_no_continuation_or_review() {
    // (a) turns 切れ（300k）≥ 250k: continuation しない。
    // (b) done（300k）≥ 250k: reviewer を起こさない。
    for (first, label) in [(turns_exhausted(300_000), "turns"), (done(300_000), "done")] {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
        let task = budget_task(dir.path(), 250_000, Check::Reviewer);
        let task_id = task.id;
        store.insert(&task).unwrap();
        let adapter = Arc::new(ScriptedAdapter::new(vec![first]));
        let (workers, reviewers) = (adapter.worker_runs.clone(), adapter.reviewer_runs.clone());
        let mut d = dispatcher(store.clone(), adapter, 1);
        let report = run_until_idle(&mut d, 400).await;
        assert!(report.idle, "{label}: {report:?}");
        let stored = store.get(task_id).unwrap().unwrap();
        assert_eq!(stored.status, Status::Failed, "{label}: {stored:?}");
        assert_eq!(workers.load(Ordering::SeqCst), 1, "{label}");
        assert_eq!(reviewers.load(Ordering::SeqCst), 0, "{label}");
        let finished = worker_finished(&store, task_id);
        assert_eq!(finished.len(), 1, "{label}: {finished:?}");
        assert!(
            matches!(&finished[0], Event::WorkerFinished { outcome, .. }
                if outcome == "token_budget_exhausted: 300000/250000"),
            "{label}: {finished:?}"
        );
        assert!(
            store
                .runs_for_task(task_id)
                .unwrap()
                .iter()
                .all(|r| r.role != task_core::RunIndexRole::Reviewer),
            "{label}"
        );
        assert_runs_replay_clean(&store, task_id);
    }

    // (c) 上限未満（300k < 1M）: continuation が 1 本起き、done（100k）の後に reviewer で done。
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = budget_task(dir.path(), 1_000_000, Check::Reviewer);
    let task_id = task.id;
    store.insert(&task).unwrap();
    let adapter = Arc::new(ScriptedAdapter::new(vec![
        turns_exhausted(300_000),
        done(100_000),
    ]));
    let (workers, reviewers) = (adapter.worker_runs.clone(), adapter.reviewer_runs.clone());
    let mut d = dispatcher(store.clone(), adapter, 1);
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");
    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{stored:?}");
    assert_eq!(workers.load(Ordering::SeqCst), 2, "one continuation");
    assert_eq!(reviewers.load(Ordering::SeqCst), 1);
    assert!(
        worker_finished(&store, task_id).iter().any(|e| matches!(e,
            Event::WorkerFinished { outcome, .. } if outcome.starts_with("continue: "))),
        "the continuation runs inside the remaining budget"
    );
    let total: u64 = store
        .runs_for_task(task_id)
        .unwrap()
        .iter()
        .filter_map(|r| r.usage.map(|u| u.budget_tokens()))
        .sum();
    assert_eq!(total, 400_001);
}

/// 失敗する reviewer run（`Error{retryable:false}` に usage 付き）。worker run は done（usage なし）。
struct FailingReviewerAdapter;

#[async_trait]
impl WorkerAdapter for FailingReviewerAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        std::fs::create_dir_all(&req.artifacts_dir).unwrap();
        let terminal = if req.task.kind == TaskKind::Review {
            Terminal::Error {
                message: "reviewer crashed".into(),
                retryable: false,
                usage: Some(tokens(4_321)),
            }
        } else {
            done(10)
        };
        Ok(RunOutcome {
            terminal,
            exit_code: Some(1),
        })
    }
}

/// D5: 失敗した reviewer run（`review.rs` の失敗経路）でも、観測できた usage が `worker_finished.usage` と
/// runs 索引に入る。replay（events から作り直した runs）とも一致する。
#[tokio::test]
async fn failed_reviewer_run_records_usage() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(dir.path(), Check::Reviewer, 0);
    let task_id = task.id;
    store.insert(&task).unwrap();
    let mut d = dispatcher(store.clone(), Arc::new(FailingReviewerAdapter), 1);
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(task_id).unwrap().unwrap().status, Status::Failed);
    let reviewer_usage = worker_finished(&store, task_id)
        .into_iter()
        .find_map(|e| match e {
            Event::WorkerFinished {
                role: Some(task_core::RunRole::Reviewer),
                usage,
                outcome,
                ..
            } => {
                assert!(outcome.starts_with("error(retryable=false)"), "{outcome}");
                Some(usage)
            }
            _ => None,
        })
        .expect("a reviewer WorkerFinished");
    assert_eq!(reviewer_usage.map(|u| u.budget_tokens()), Some(4_321));
    let row = store
        .runs_for_task(task_id)
        .unwrap()
        .into_iter()
        .find(|r| r.role == task_core::RunIndexRole::Reviewer)
        .expect("a reviewer run in the runs index");
    assert_eq!(row.usage.map(|u| u.budget_tokens()), Some(4_321));
    assert_eq!(row.status, task_core::RunIndexStatus::Failed);
    assert_runs_replay_clean(&store, task_id);
}
