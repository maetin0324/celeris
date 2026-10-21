//! 2026-10-20-continuation-limit-by-progress: 進捗がある continuation は回数では人に聞かず、
//! 固い上限（`continuation_hard_limit`）でだけ聞く。task（WU を持たない）と WU の両方の経路。
//! LLM・外部ネットワーク・sleep は使わない（run ごとに `completed` が増える偽アダプタ）。

use std::sync::atomic::AtomicUsize;

use super::*;

/// 旧い既定（3 回）より大きい、試験用の固い上限。
const HARD_LIMIT: u32 = 6;

/// run のたびに `completed` が 1 件増える checkpoint を yield する（毎回進捗あり）。
struct GrowingYieldAdapter {
    calls: AtomicUsize,
}

#[async_trait]
impl WorkerAdapter for GrowingYieldAdapter {
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
        std::fs::create_dir_all(&req.artifacts_dir).ok();
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(RunOutcome {
            terminal: Terminal::Yielded {
                checkpoint: serde_json::json!({
                    "completed": (0..n).map(|i| format!("step {i}")).collect::<Vec<_>>(),
                    "remaining": ["more"],
                    "next_action": "続ける",
                }),
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

fn adapter() -> Arc<GrowingYieldAdapter> {
    Arc::new(GrowingYieldAdapter {
        calls: AtomicUsize::new(0),
    })
}

fn continue_count(store: &Arc<dyn TaskStore>, task_id: TaskId) -> usize {
    store
        .events_for(task_id)
        .unwrap()
        .iter()
        .filter(|(_, e)| matches!(e, Event::Transitioned { reason, .. } if reason == "continue"))
        .count()
}

fn last_outcome(store: &Arc<dyn TaskStore>, task_id: TaskId) -> String {
    store
        .events_for(task_id)
        .unwrap()
        .iter()
        .rev()
        .find_map(|(_, e)| match e {
            Event::WorkerFinished {
                role: None,
                outcome,
                ..
            } => Some(outcome.clone()),
            _ => None,
        })
        .expect("some WorkerFinished")
}

fn assert_cap_question(q: &str) {
    assert!(q.starts_with("question: "), "{q}");
    assert!(!q.contains("進みません"), "{q}");
    for needle in ["上限", "continuation 6 回", "進捗あり"] {
        assert!(q.contains(needle), "{needle} not in {q}");
    }
}

/// task の経路: 毎回進捗があれば旧い上限（3 回）を超えて続き、固い上限で初めて人に聞く。
#[tokio::test]
async fn task_with_progress_continues_past_the_old_count_limit_and_asks_at_the_hard_limit() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        2,
    );
    let task_id = task.id;
    store.insert(&task).unwrap();
    let mut d = dispatcher(store.clone(), adapter(), 1);
    d.config.execution.continuation_hard_limit = HARD_LIMIT;
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    assert_eq!(store.get(task_id).unwrap().unwrap().status, Status::Blocked);
    assert_eq!(continue_count(&store, task_id), HARD_LIMIT as usize);
    assert_cap_question(&last_outcome(&store, task_id));
}

/// WU の経路: 同じ規則。固い上限の WU は `blocked(continuation_cap)` で replan されず、回答後に窓が 0 から数え直される。
#[tokio::test]
async fn work_unit_with_progress_asks_only_at_the_hard_limit_and_resumes_after_an_answer() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        5,
    );
    let task_id = task.id;
    store.insert(&task).unwrap();
    let spec = task_core::ExecutionPlanSpec {
        stages: Vec::new(),
        units: Vec::new(),
        decisions: Vec::new(),
        schema: task_core::EXECUTION_PLAN_SCHEMA.to_string(),
        rationale: "implement only".to_string(),
        work_units: vec![wu_spec("implement", &[])],
        phases: Vec::new(),
        children: Vec::new(),
    };
    task_ops::execution::adopt_plan(
        store.as_ref(),
        task_id,
        spec,
        task_core::PlanOrigin::Fixture,
        None,
        task_core::ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .expect("adopt plan");
    let mut d = dispatcher(store.clone(), adapter(), 1);
    d.config.execution.continuation_hard_limit = HARD_LIMIT;
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Blocked, "{stored:?}");
    let units = store.work_units_for(task_id).unwrap();
    let wu = units.iter().find(|u| u.key == "implement").unwrap();
    assert_eq!(wu.status, task_core::WorkUnitStatus::Blocked, "{wu:?}");
    assert_eq!(
        wu.blocked_reason,
        Some(task_core::WorkUnitBlockedReason::ContinuationCap)
    );
    assert_eq!(wu.continuations, HARD_LIMIT);
    assert_cap_question(&last_outcome(&store, task_id));
    assert_eq!(
        store.execution_plan_list(task_id).unwrap().len(),
        1,
        "固い上限では replan しない"
    );

    // 回答で再開し、窓は 0 から数え直す（もう 6 回続けてから、また聞く）。
    store
        .apply_transition_with_events(task_id, Trigger::Answer, vec![])
        .unwrap();
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");
    let units = store.work_units_for(task_id).unwrap();
    let wu = units.iter().find(|u| u.key == "implement").unwrap();
    assert_eq!(
        wu.blocked_reason,
        Some(task_core::WorkUnitBlockedReason::ContinuationCap)
    );
    assert_eq!(wu.continuations, HARD_LIMIT * 2, "{wu:?}");
    assert_eq!(
        task_ops::derive::wu_continuations_since_answer(
            &store.events_for(task_id).unwrap(),
            &wu.id
        ),
        HARD_LIMIT
    );
    assert_eq!(store.execution_plan_list(task_id).unwrap().len(), 1);
}

/// task の経路: 進捗なしが `no_progress_limit` 回続くと聞く。文言は進捗なしの回数を示す。
#[tokio::test]
async fn task_without_progress_asks_after_no_progress_limit_with_the_streak_in_the_question() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        2,
    );
    let task_id = task.id;
    store.insert(&task).unwrap();
    let mut d = dispatcher(store.clone(), Arc::new(AlwaysBudgetExhaustedAdapter), 1);
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(task_id).unwrap().unwrap().status, Status::Blocked);
    let q = last_outcome(&store, task_id);
    assert!(q.contains("進捗なしが 2 回続きました"), "{q}");
}
