//! ADR-0079 付記「R6-1: 人の gate は unit を止める（PlanGate 未承認・段階 review）、上限超過は人に聞く、索引の回収」:
//! - D1（P-R5b-4）: PlanGate を通っていない版の unit は起こさない。承認待ちの版に人が `replan` を求めて task が
//!   `ready` に戻っても、次の版が採用・承認されるまで子を作らない。
//! - D2（P-R5b-5）: 段階の `review: human` の間は次の段階の unit を `ready` に上げず、子も作らない。「続ける」で進む。
//! - D3（木の節点）: replan を使い切った後の leaf の失敗は `failed` にせず `limit:max_replans` の決定を出す。
//! - D4: 終端への遷移で `runs` 索引の `running` の行を閉じ、残っていた行は dispatcher の照合が閉じる。
//!
//! すべて偽のアダプタと一時ディレクトリだけで、外部ネットワークに出ない。

use std::collections::VecDeque;

use super::tree::{
    approve_root_plan, assert_replay_is_clean, leaf, stage, task_unit, tree_dispatcher, unit,
    v3_plan,
};
use super::*;

/// planner run には計画の列を順に書き（尽きたら `{}` = 不正な試行）、それ以外の run は `Done`。
struct GateAdapter {
    plans: StdMutex<VecDeque<String>>,
}

impl GateAdapter {
    fn new(plans: Vec<String>) -> Self {
        GateAdapter {
            plans: StdMutex::new(plans.into_iter().collect()),
        }
    }
}

#[async_trait]
impl WorkerAdapter for GateAdapter {
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
        if req.context.execution_planner.is_some() {
            // 列が尽きたら不正な計画（`{}`）を書く（前の run の計画の読み直しにしない）。
            let json = self
                .plans
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| "{}".to_string());
            std::fs::write(req.artifacts_dir.join("execution-plan.json"), json)
                .expect("write execution-plan.json");
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

/// 人の gate で止まった木は `idle` にならないことがある。run・判定・統合が 50 tick 続けて無ければ落ち着いたとみなす。
async fn run_until_quiet(d: &mut Dispatcher, store: &Arc<dyn TaskStore>, max_ticks: usize) {
    let mut quiet = 0;
    for _ in 0..max_ticks {
        let r = d.tick().unwrap();
        if r.idle {
            return;
        }
        let busy = r.reclaimed + r.dispatched + r.finished + r.reviewed + r.in_flight > 0
            || !store.list(Some(Status::Running)).unwrap().is_empty()
            || !store.list(Some(Status::Reviewing)).unwrap().is_empty();
        if busy {
            quiet = 0;
        } else {
            quiet += 1;
            if quiet >= 50 {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the dispatcher never settled");
}

fn root_task(dir: &std::path::Path) -> Task {
    let mut root = compound_task(dir);
    root.title = "browser capability".into();
    root.budget.max_turns = 30;
    root
}

fn children_created(store: &Arc<dyn TaskStore>, root: TaskId) -> Vec<String> {
    store
        .events_for(root)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::ChildTaskCreated { plan_id, .. } => Some(plan_id),
            _ => None,
        })
        .collect()
}

fn gate_state(store: &Arc<dyn TaskStore>, id: TaskId) -> task_ops::plan_gate::PlanGateState {
    let plan = store
        .execution_plan_active(id)
        .unwrap()
        .expect("active plan");
    task_ops::plan_gate::plan_gate_state(&plan, &store.events_for(id).unwrap())
}

/// D1（本番 2026-09-29 17:13Z / 21:22Z の P-R5b-4）: 承認待ちの v1 に人が `replan` を求めると task は `ready` に
/// 戻るが、v1 の kind task の unit から子を作らない。planner の v2 も承認待ちなら、v2 の承認までやはり作らない。
/// 承認の後は v2 の unit から子を作り、木は done まで進む。
#[tokio::test]
async fn plan_gate_replan_does_not_dispatch_the_unapproved_version() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = root_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let v1 = v3_plan(
        vec![stage("s1", true)],
        vec![task_unit("c", "s1", &[], "true")],
    );
    let v2 = v3_plan(
        vec![stage("s1", true)],
        vec![task_unit("c", "s1", &[], "true"), leaf("a", "s1", &[])],
    );
    let adapter = Arc::new(GateAdapter::new(vec![v1, v2]));
    let mut d = tree_dispatcher(&store, adapter);
    run_until_quiet(&mut d, &store, 400).await;
    assert_eq!(
        gate_state(&store, root_id),
        task_ops::plan_gate::PlanGateState::Pending
    );
    let v1_id = store.execution_plan_active(root_id).unwrap().unwrap().id;
    task_ops::plan_gate::plan_gate(
        store.as_ref(),
        root_id,
        task_ops::plan_gate::PlanGateAction::Replan,
        Some("add leaf a".into()),
        "human",
    )
    .unwrap();
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Ready);
    // 次の tick（旧: 同じ tick で v1 の c から子ができた）でも、v1 はまだ承認されていない。
    d.tick().unwrap();
    assert!(children_created(&store, root_id).is_empty());
    run_until_quiet(&mut d, &store, 800).await;
    let plans = store.execution_plan_list(root_id).unwrap();
    assert_eq!(plans.len(), 2, "{plans:?}");
    let v2_id = store.execution_plan_active(root_id).unwrap().unwrap().id;
    assert_ne!(v1_id, v2_id);
    assert_eq!(
        gate_state(&store, root_id),
        task_ops::plan_gate::PlanGateState::Pending,
        "v2 asks for approval too"
    );
    assert!(
        children_created(&store, root_id).is_empty(),
        "no child from an unapproved version"
    );
    assert!(store.children(root_id).unwrap().is_empty());
    let units = store.work_units_for(root_id).unwrap();
    assert_eq!(unit(&units, "a").runs, 0, "no leaf run either");

    approve_root_plan(&store, root_id);
    assert_eq!(
        gate_state(&store, root_id),
        task_ops::plan_gate::PlanGateState::Approved
    );
    let report = run_until_idle(&mut d, 1200).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(children_created(&store, root_id), vec![v2_id]);
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Done);
    // 人の replan（計画の承認の replan）は `max_replans` に数えない（D3）。
    assert_eq!(
        task_ops::plan_gate::counted_replans(&store.events_for(root_id).unwrap()),
        0
    );
    assert_replay_is_clean(&store);
}

/// D1: 承認待ちの版に人が `replan` を求めた後、planner が次の版を出せない（2 回とも不正 → `plan_invalid` の決定）
/// 間も、task は `ready` だが承認されていない v1 の unit から子を作らない。
#[tokio::test]
async fn an_unapproved_version_stays_parked_while_the_replan_is_pending() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = root_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let v1 = v3_plan(
        vec![stage("s1", true)],
        vec![task_unit("c", "s1", &[], "true")],
    );
    let adapter = Arc::new(GateAdapter::new(vec![v1]));
    let mut d = tree_dispatcher(&store, adapter);
    run_until_quiet(&mut d, &store, 400).await;
    task_ops::plan_gate::plan_gate(
        store.as_ref(),
        root_id,
        task_ops::plan_gate::PlanGateAction::Replan,
        Some("split c".into()),
        "human",
    )
    .unwrap();
    run_until_quiet(&mut d, &store, 800).await;
    assert_eq!(store.execution_plan_list(root_id).unwrap().len(), 1);
    let decisions = store.decisions_list(Some(root_id)).unwrap();
    assert!(
        decisions
            .iter()
            .any(|d| d.kind == task_core::DecisionKind::PlanInvalid),
        "{decisions:?}"
    );
    assert_eq!(
        gate_state(&store, root_id),
        task_ops::plan_gate::PlanGateState::Pending
    );
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Ready);
    assert!(children_created(&store, root_id).is_empty());
    assert!(store.children(root_id).unwrap().is_empty());
}

/// D2（本番 2026-09-29 17:44:54Z の P-R5b-5）: 段階 s1 の `review: human` で止まっている間、次の段階 s2 の kind task
/// の unit は `pending` のままで子を作らない。「続ける」の後に `ready` になり、子ができて木は done まで進む。
#[tokio::test]
async fn review_human_stage_holds_the_next_stage_child() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = root_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = v3_plan(
        vec![stage("s1", true), stage("s2", false)],
        vec![leaf("a", "s1", &[]), task_unit("c", "s2", &["a"], "true")],
    );
    let adapter = Arc::new(GateAdapter::new(vec![plan]));
    let mut d = tree_dispatcher(&store, adapter);
    run_until_quiet(&mut d, &store, 400).await;
    assert_eq!(
        approve_root_plan(&store, root_id),
        vec!["review_human:s1".to_string()]
    );
    run_until_quiet(&mut d, &store, 800).await;
    let stored = store.get(root_id).unwrap().unwrap();
    let events = store.events_for(root_id).unwrap();
    assert!(
        task_ops::phase_gate::is_awaiting_human(&stored, &events),
        "{:?}",
        stored.status
    );
    // 待っている間に何 tick 回っても子は作られない。
    for _ in 0..20 {
        d.tick().unwrap();
    }
    let units = store.work_units_for(root_id).unwrap();
    assert_eq!(unit(&units, "a").status, task_core::WorkUnitStatus::Done);
    assert_eq!(unit(&units, "c").status, task_core::WorkUnitStatus::Pending);
    assert!(children_created(&store, root_id).is_empty());
    assert!(store.children(root_id).unwrap().is_empty());

    task_ops::phase_gate::phase_gate(
        store.as_ref(),
        root_id,
        task_ops::phase_gate::PhaseGateAction::Continue,
        None,
    )
    .unwrap();
    let report = run_until_idle(&mut d, 1200).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(children_created(&store, root_id).len(), 1);
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Done);
    assert_replay_is_clean(&store);
}

/// D3（木の節点）: 節点の `max_replans` を使い切った後に leaf が失敗しても root を `failed` にせず、
/// `limit:max_replans` の決定（`needed_before: self`）で人に聞く（質問は作らない）。`raise-once` の回答で replan
/// が 1 回起き、直した版で done まで進む。
#[tokio::test]
async fn a_tree_node_leaf_failure_after_replans_are_exhausted_raises_the_limit_decision() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut root = root_task(dir.path());
    root.budget.max_retries = 0;
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let mut failing = leaf("a", "s1", &[]);
    failing["checks"] = serde_json::json!([{"cmd": "false", "expect_exit": 0}]);
    let v1 = v3_plan(vec![stage("s1", false)], vec![failing]);
    let v2 = v3_plan(vec![stage("s1", false)], vec![leaf("a", "s1", &[])]);
    let adapter = Arc::new(GateAdapter::new(vec![v1, v2]));
    let mut d = tree_dispatcher(&store, adapter);
    d.config.execution.max_replans = 0;
    run_until_quiet(&mut d, &store, 800).await;
    let stored = store.get(root_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Ready, "not failed: {stored:?}");
    let decisions = store.decisions_list(Some(root_id)).unwrap();
    assert_eq!(decisions.len(), 1, "{decisions:?}");
    assert_eq!(decisions[0].key, "limit:max_replans");
    assert_eq!(decisions[0].task_id, root_id);
    assert!(
        !store
            .events_for(root_id)
            .unwrap()
            .iter()
            .any(|(_, e)| matches!(e, Event::QuestionRaised { .. })),
        "a tree node asks with a decision, not a question"
    );
    task_ops::decision::answer(
        store.as_ref(),
        &decisions[0].id,
        Some("raise-once"),
        None,
        "human",
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    let report = run_until_idle(&mut d, 1200).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Done);
    assert_eq!(store.execution_plan_list(root_id).unwrap().len(), 2);
}

fn open_run_row(task_id: TaskId, run_id: &str, role: task_core::RunIndexRole) -> task_core::RunRow {
    task_core::RunRow {
        run_id: run_id.to_string(),
        task_id: task_id.to_string(),
        work_unit_id: None,
        role,
        seq: 1,
        status: task_core::RunIndexStatus::Running,
        adapter: Some("instant".into()),
        model: Some("m".into()),
        account: None,
        session_id: None,
        checkpoint: None,
        usage: None,
        metrics: None,
        started_at: "2026-09-30T00:00:00Z".into(),
        finished_at: None,
    }
}

/// D4（本番: failed の task 01M3PBAVFAYPDWMQMDBXPTE2V8 の reviewer run 01M3Q01QC6DQTG8XX62WJDC0M7 が `running` のまま）:
/// 終端への遷移は同じトランザクションで `running` の行を閉じる（`WorkerFinished{end: Cancelled}`）。R6-1 より前に
/// 残った行（終端の task の `running`）は dispatcher の照合が起動後の最初の tick で 1 回だけ閉じる。
#[tokio::test]
async fn runs_index_rows_of_terminal_tasks_are_closed() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    // 1. 終端への遷移（Ready → Cancelled）が閉じる。
    let live = compound_task(dir.path());
    store.insert(&live).unwrap();
    store
        .run_index_start(open_run_row(
            live.id,
            "run-review",
            task_core::RunIndexRole::Reviewer,
        ))
        .unwrap();
    store
        .apply_transition(live.id, Trigger::Cancel, None)
        .unwrap();
    let row = store.run_index_get("run-review").unwrap().unwrap();
    assert_eq!(row.status, task_core::RunIndexStatus::Cancelled, "{row:?}");
    assert!(row.finished_at.is_some());
    assert!(store.events_for(live.id).unwrap().iter().any(|(_, e)| matches!(
        e,
        Event::WorkerFinished { run_id, role: Some(RunRole::Reviewer), .. } if run_id == "run-review"
    )));

    // 2. 既に終端の task に残った行は照合が閉じる（1 回だけ）。
    let mut stale = compound_task(dir.path());
    stale.status = Status::Failed;
    store.insert(&stale).unwrap();
    store
        .run_index_start(open_run_row(
            stale.id,
            "run-stale",
            task_core::RunIndexRole::Reviewer,
        ))
        .unwrap();
    let adapter = Arc::new(GateAdapter::new(Vec::new()));
    let mut d = tree_dispatcher(&store, adapter);
    d.tick().unwrap();
    let row = store.run_index_get("run-stale").unwrap().unwrap();
    assert_eq!(row.status, task_core::RunIndexStatus::Cancelled, "{row:?}");
    d.terminal_records_reconciled_at = None;
    d.tick().unwrap();
    let closes = store
        .events_for(stale.id)
        .unwrap()
        .into_iter()
        .filter(|(_, e)| matches!(e, Event::WorkerFinished { run_id, .. } if run_id == "run-stale"))
        .count();
    assert_eq!(closes, 1, "closed once");
    assert!(store.close_runs_of_terminal_tasks().unwrap().is_empty());
}

/// D5（BenchFS v2 の `near_limit:max_child_tasks_per_plan:10/6`）: 承認の材料の子 task の数は、子を作る unit だけ
/// （`adopt` の unit と done の unit を除く。検証の `creates_child` と同じ）。
#[tokio::test]
async fn approval_facts_count_only_units_that_will_create_children() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = root_task(dir.path());
    store.insert(&root).unwrap();
    let mut adopted = task_unit("c2", "s1", &[], "true");
    adopted["adopt"] = serde_json::json!(TaskId::new().to_string());
    let spec: task_core::ExecutionPlanSpec = serde_json::from_str(&v3_plan(
        vec![stage("s1", false)],
        vec![
            task_unit("c1", "s1", &[], "true"),
            adopted,
            task_unit("c3", "s1", &[], "true"),
        ],
    ))
    .unwrap();
    let plan = task_core::ExecutionPlanRow {
        id: "p1".into(),
        task_id: root.id.to_string(),
        version: 1,
        origin: task_core::PlanOrigin::Human,
        planner_run_id: None,
        status: task_core::PlanStatus::Active,
        spec: spec.clone(),
        created_at: "2026-09-30T00:00:00Z".into(),
        superseded_at: None,
    };
    let c3 = spec.units.iter().find(|u| u.key == "c3").unwrap();
    let done = task_core::WorkUnitRow::new(
        task_core::new_id(),
        root.id.to_string(),
        "p1".into(),
        2,
        c3.to_work_unit_spec(),
        task_core::WorkUnitStatus::Done,
        "2026-09-30T00:00:00Z".into(),
    );
    store
        .work_units_apply(root.id, vec![done], Vec::new(), Vec::new())
        .unwrap();
    let facts = task_ops::plan_gate::approval_facts(store.as_ref(), &root, &plan).unwrap();
    assert_eq!(facts.child_task_units, 1, "c1 only: {facts:?}");
}

/// 旧版で残った段の依頼は最初の tick が追記だけで閉じる。配送の依頼は残す。再起動・定期回収とも冪等。
#[tokio::test]
async fn startup_closes_only_terminal_tasks_integration_requests_once() {
    use task_core::integration_request::{IntegrationRequest, TASK_TERMINAL_ANSWER};
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut snapshots = Vec::new();
    for status in [
        Status::Done,
        Status::Cancelled,
        Status::Failed,
        Status::Draft,
        Status::Blocked,
    ] {
        let mut task = compound_task(dir.path());
        task.status = status;
        store.insert(&task).unwrap();
        for origin in ["phase:impl", "phase:close", "delivery"] {
            // append_event で旧版の履歴を再現する（終端遷移の修正を通さない）。
            store
                .append_event(
                    task.id,
                    &Event::IntegrationRequested {
                        request: Box::new(IntegrationRequest {
                            target_branch: "main".into(),
                            target_sha: "target".into(),
                            source_branch: origin.into(),
                            source_sha: origin.into(),
                            merge_base: None,
                            conflict_files: vec!["src/lib.rs".into()],
                            intent: vec![],
                            reason: "conflict".into(),
                            recommendation: "review".into(),
                            actions: vec![],
                            candidate_sha: None,
                        }),
                        origin: origin.into(),
                    },
                )
                .unwrap();
        }
        snapshots.push((
            task.id,
            status,
            store.event_rows_for(task.id, None, 100).unwrap(),
        ));
    }
    assert_eq!(store.open_integration_requests().unwrap().len(), 15);
    for _ in 0..2 {
        let mut d = tree_dispatcher(&store, Arc::new(GateAdapter::new(Vec::new())));
        d.tick().unwrap();
        d.terminal_records_reconciled_at = None;
        d.tick().unwrap();
        for (id, status, before) in &snapshots {
            let after = store.event_rows_for(*id, None, 100).unwrap();
            assert_eq!(&after[..before.len()], before.as_slice());
            let answers = after
                .iter()
                .filter(|row| {
                    matches!(&row.event,
                        Event::IntegrationAnswered { answer, .. } if answer == TASK_TERMINAL_ANSWER
                    )
                })
                .count();
            // 段の依頼 2 件だけ。配送の依頼は配送が自分で閉じるので残す。
            assert_eq!(answers, if status.is_terminal() { 2 } else { 0 });
        }
        let open = store.open_integration_requests().unwrap();
        // 非終端 2 task × 3 件 + 終端 3 task の配送依頼 × 1 件。
        assert_eq!(open.len(), 9);
        assert!(open.iter().all(|row| {
            let terminal = store
                .get(row.task_id)
                .unwrap()
                .unwrap()
                .status
                .is_terminal();
            !terminal
                || matches!(&row.event,
                    Event::IntegrationRequested { origin, .. } if origin == "delivery")
        }));
    }
}
