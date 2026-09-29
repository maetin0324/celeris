//! ADR-0079 §7 R1b: plan/3 の kind task の unit から子 task を作り、子の状態を unit に写し、段階の完了に
//! 子を含め、子だけを待つ親は lease を持たずに `awaiting_children` で待ち、親の中止は subtree に連鎖する。
//! `review: human` の段階は ADR-0074 D2 の途中確認で止まる。すべて偽のアダプタと一時ディレクトリだけで、
//! 外部ネットワークに出ない。

use super::*;

/// planner run には計画の列を順に書き、それ以外の run は `Done` で終わる。子 task の run
/// （計画を持たない task の run = `work_unit` が無い run）は `child_delay` だけ待ってから終わる。
pub(super) struct TreeAdapter {
    plans: StdMutex<std::collections::VecDeque<String>>,
    child_delay: Duration,
    seen: Arc<StdMutex<Vec<task_worker::RunContext>>>,
}

impl TreeAdapter {
    pub(super) fn new(plans: Vec<String>, child_delay: Duration) -> Self {
        TreeAdapter {
            plans: StdMutex::new(plans.into_iter().collect()),
            child_delay,
            seen: Arc::new(StdMutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl WorkerAdapter for TreeAdapter {
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
        self.seen.lock().unwrap().push(req.context.clone());
        if req.context.execution_planner.is_some() {
            if let Some(json) = self.plans.lock().unwrap().pop_front() {
                std::fs::create_dir_all(&req.artifacts_dir).ok();
                std::fs::write(req.artifacts_dir.join("execution-plan.json"), json)
                    .expect("write execution-plan.json");
            }
        } else if req.context.work_unit.is_none() && !self.child_delay.is_zero() {
            tokio::time::sleep(self.child_delay).await;
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

pub(super) fn command_criterion(cmd: &str) -> serde_json::Value {
    serde_json::to_value(task_core::Criterion {
        text: format!("{cmd} passes"),
        check: Check::Command {
            cmd: cmd.into(),
            expect_exit: 0,
        },
    })
    .unwrap()
}

pub(super) fn leaf(key: &str, stage: &str, deps: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "key": key,
        "stage": stage,
        "kind": "implement",
        "title": format!("Leaf {key}"),
        "objective": format!("Implement {key} thoroughly and completely"),
        "depends_on": deps,
        "done_when": [format!("{key} is done")],
        "checks": [{"cmd": "true", "expect_exit": 0}],
    })
}

pub(super) fn task_unit(
    key: &str,
    stage: &str,
    deps: &[&str],
    acceptance_cmd: &str,
) -> serde_json::Value {
    serde_json::json!({
        "key": key,
        "stage": stage,
        "kind": "task",
        // ADR-0079 D4 (3)（Phase R2a）: 小さな kind task の unit は unit の gate で atomic になり、構造上の
        // 理由が無ければ leaf に下げられる。ここでは子 task のまま残す理由として「親に無い skill（別の部署）」を
        // 持たせる（担当の無い試験の組織では matching に影響しない）。
        "skills": ["tree-fixture"],
        "title": format!("Child {key}"),
        "objective": format!("Deliver the {key} part as its own reviewed task"),
        "depends_on": deps,
        "acceptance": [command_criterion(acceptance_cmd)],
    })
}

pub(super) fn stage(key: &str, review_human: bool) -> serde_json::Value {
    let mut s =
        serde_json::json!({"key": key, "kind": "implement", "title": format!("Stage {key}")});
    if review_human {
        s["review"] = serde_json::json!("human");
    }
    s
}

pub(super) fn v3_plan(stages: Vec<serde_json::Value>, units: Vec<serde_json::Value>) -> String {
    serde_json::json!({
        "schema": task_core::EXECUTION_PLAN_SCHEMA_V3,
        "rationale": "the child part is reviewed on its own",
        "stages": stages,
        "units": units,
    })
    .to_string()
}

pub(super) fn tree_limits() -> task_core::ExecutionLimits {
    let mut limits = task_core::ExecutionLimits::default();
    limits.tree.enabled = true;
    limits
}

pub(super) fn tree_dispatcher(
    store: &Arc<dyn TaskStore>,
    adapter: Arc<dyn WorkerAdapter>,
) -> Dispatcher {
    let mut d = dispatcher(store.clone(), adapter, 3);
    d.config.execution.gate = task_core::GateMode::On;
    d.config.execution.planner.adapter = "instant".to_string();
    d.config.execution.limits = tree_limits();
    d
}

pub(super) fn unit<'a>(
    units: &'a [task_core::WorkUnitRow],
    key: &str,
) -> &'a task_core::WorkUnitRow {
    units
        .iter()
        .find(|u| u.key == key)
        .unwrap_or_else(|| panic!("no unit {key}: {units:?}"))
}

pub(super) fn unit_reasons(events: &[(u64, Event)], key: &str) -> Vec<String> {
    events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::WorkUnitTransitioned { key: k, reason, .. } if k == key => Some(reason.clone()),
            _ => None,
        })
        .collect()
}

fn view_ctx() -> task_ops::view::ViewContext {
    task_ops::view::ViewContext {
        workspace_root: PathBuf::from("/nonexistent"),
        retry_backoff_base: Duration::ZERO,
        retry_backoff_max: Duration::ZERO,
        max_requeues: 5,
        clusters: Default::default(),
    }
}

/// `celerisctl replay --check` の突き合わせ（task の状態と attempts、`work_units`〈子の結び付き
/// `child_task_id` と状態を含む〉、`execution_plans`、`decisions`）。`runs` の表は比べない: 偽のアダプタで
/// 走らせた /2・/3 の計画では、planner run の role と並列 WU の run の `work_unit_id` / `seq` が
/// events から復元しきれない（木と無関係に R1b 以前からある差。PROGRESS の未解決に記録）。
pub(super) fn assert_replay_is_clean(store: &Arc<dyn TaskStore>) {
    let report = task_ops::replay::replay(store.as_ref()).unwrap();
    assert!(report.mismatches.is_empty(), "{:?}", report.mismatches);
    let (wu, _runs, plans, _) =
        task_ops::replay::check_and_apply_execution(store.as_ref(), false).unwrap();
    assert!(wu.is_empty(), "{wu:?}");
    assert!(plans.is_empty(), "{plans:?}");
    let (decisions, _) =
        task_ops::replay::check_and_apply_decisions(store.as_ref(), false).unwrap();
    assert!(decisions.is_empty(), "{decisions:?}");
}

/// R1b (a) / (c) の done / 段階の完了 / 親の完了 / replay: planner の /3（段階 s1 = leaf a + task c、
/// 段階 s2 = leaf b）で、task の unit が ready になると 1 トランザクションで子 task ができ、子は自分の
/// gate（atomic）で 1 run 走って done → unit done → 段階 s1 の統合 → s2 → 親 done。
#[tokio::test]
async fn v3_task_unit_creates_child_when_ready() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = compound_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = v3_plan(
        vec![stage("s1", false), stage("s2", false)],
        vec![
            leaf("a", "s1", &[]),
            task_unit("c", "s1", &[], "true"),
            leaf("b", "s2", &[]),
        ],
    );
    let adapter = Arc::new(TreeAdapter::new(vec![plan], Duration::ZERO));
    let mut d = tree_dispatcher(&store, adapter.clone());
    let report = run_until_idle(&mut d, 800).await;
    assert!(report.idle, "{report:?}");

    let stored_root = store.get(root_id).unwrap().unwrap();
    assert_eq!(
        stored_root.status,
        Status::Done,
        "{:?}",
        store.events_for(root_id).unwrap()
    );
    let children = store.children(root_id).unwrap();
    assert_eq!(children.len(), 1, "{children:?}");
    let child = &children[0];
    assert_eq!(child.parent_id, Some(root_id));
    assert_eq!(child.kind, TaskKind::Execute);
    assert_eq!(child.title, "Child c");
    assert!(
        child
            .objective
            .starts_with("Deliver the c part as its own reviewed task"),
        "{}",
        child.objective
    );
    assert!(
        child.objective.contains(task_ops::tree::TREE_PATH_HEADING),
        "{}",
        child.objective
    );
    assert_eq!(child.labels, vec!["child-c".to_string()]);
    assert_eq!(
        child.workspace, root.workspace,
        "the child inherits the workspace"
    );
    assert_eq!(child.repos, root.repos);
    assert_eq!(child.budget, root.budget, "the child inherits the budget");
    assert_eq!(child.project_id, root.project_id);
    assert_eq!(child.assignee, None, "ADR-0069 D1: matching decides");
    let tree = child.tree.as_ref().expect("Task.tree");
    assert_eq!(tree.root_id, root_id);
    assert_eq!(tree.depth, 2);
    let parent_unit = tree.parent_unit.as_ref().expect("parent_unit");
    assert_eq!(parent_unit.task_id, root_id);
    assert_eq!(parent_unit.unit_key, "c");
    assert_eq!(parent_unit.stage, "s1");
    assert_eq!(child.status, Status::Done);
    // 子は自分で gate をやり直した（atomic → 1 run）。
    let child_gate = child
        .routing
        .as_ref()
        .and_then(|r| r.execution.as_ref())
        .expect("the child is gated on its own first dispatch");
    assert_eq!(child_gate.mode, task_core::ExecutionMode::Atomic);
    assert!(store.execution_plan_active(child.id).unwrap().is_none());

    // 子の Created は origin plan_unit、状態は ready（draft を挟まない）。
    let child_events = store.events_for(child.id).unwrap();
    match &child_events[0].1 {
        Event::Created { task, origin } => {
            assert_eq!(*origin, Some(task_core::CreatedOrigin::PlanUnit));
            assert_eq!(task.status, Status::Ready);
        }
        other => panic!("expected Created, got {other:?}"),
    }

    // 親の events: ChildTaskCreated、unit c は child_created → child_done、段階 s1 は統合された。
    let events = store.events_for(root_id).unwrap();
    let plan_id = store.execution_plan_active(root_id).unwrap().unwrap().id;
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            Event::ChildTaskCreated { plan_id: p, unit_key, child_task_id, depth: 2 }
                if *p == plan_id && unit_key == "c" && *child_task_id == child.id
        )),
        "{events:?}"
    );
    assert_eq!(
        unit_reasons(&events, "c"),
        vec!["child_created", "child_done"]
    );
    let units = store.work_units_for(root_id).unwrap();
    let c = unit(&units, "c");
    assert_eq!(c.status, task_core::WorkUnitStatus::Done);
    assert_eq!(c.child_task_id, Some(child.id.to_string()));
    assert_eq!(c.runs, 0, "a task unit never runs an LLM run itself");
    for key in ["a", "b", "integrate-s1", "integrate-s2"] {
        assert_eq!(
            unit(&units, key).status,
            task_core::WorkUnitStatus::Done,
            "{key}"
        );
    }
    let integrated: Vec<&str> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::PhaseIntegrated { phase, .. } => Some(phase.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(integrated, vec!["s1", "s2"]);
    // 段階 s2 の leaf は、s1 の子が done になるまで ready にならない（段階の障壁に子を含む）。
    let c_done_at = events
        .iter()
        .position(|(_, e)| matches!(e, Event::WorkUnitTransitioned { key, reason, .. } if key == "c" && reason == "child_done"))
        .unwrap();
    let b_ready_at = events
        .iter()
        .position(|(_, e)| matches!(e, Event::WorkUnitTransitioned { key, to: task_core::WorkUnitStatus::Ready, .. } if key == "b"))
        .unwrap();
    assert!(c_done_at < b_ready_at, "{events:?}");

    // (f): 子（木の節点）の run には委譲の分野が渡らない（テストの設定は分野 0 なので、ここでは
    // 別途 `tree_runs_cannot_delegate` で確かめる）。replay は子の結び付き・状態を同じに作り直す。
    assert_replay_is_clean(&store);
}

/// R1b (b): 依存（leaf a）と `needs_decisions`（h1）が満たされるまで子は作られない。回答で作られ、回答は
/// 子の objective の末尾に固定の書式で入る。
#[tokio::test]
async fn child_waits_for_dependencies_and_decisions() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = compound_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let mut c = task_unit("c", "s1", &["a"], "true");
    c["needs_decisions"] = serde_json::json!(["h1"]);
    let mut plan: serde_json::Value = serde_json::from_str(&v3_plan(
        vec![stage("s1", false)],
        vec![leaf("a", "s1", &[]), c],
    ))
    .unwrap();
    plan["decisions"] = serde_json::json!([{
        "key": "h1",
        "question": "which backend",
        "options": [{"key": "vault", "label": "org vault"}, {"key": "manual", "label": "manual"}],
        "recommended": "vault",
        "cost_of_reversal": "low",
        "needed_before": ["c"],
    }]);
    let adapter = Arc::new(TreeAdapter::new(vec![plan.to_string()], Duration::ZERO));
    let mut d = tree_dispatcher(&store, adapter);
    run_until_idle(&mut d, 150).await;

    let units = store.work_units_for(root_id).unwrap();
    assert_eq!(unit(&units, "a").status, task_core::WorkUnitStatus::Done);
    let c_row = unit(&units, "c");
    assert_eq!(c_row.needs_decisions, vec!["h1".to_string()]);
    assert_eq!(
        c_row.status,
        task_core::WorkUnitStatus::Ready,
        "the dependency is done"
    );
    assert!(c_row.child_task_id.is_none(), "h1 is unanswered");
    assert!(store.children(root_id).unwrap().is_empty());
    let stored_root = store.get(root_id).unwrap().unwrap();
    assert_eq!(stored_root.status, Status::Ready);
    assert!(stored_root.lease.is_none());

    // Phase R3a: 計画の採用で h1 が決定の要求になっている（planner の path 付き）。答えるまで子は作られない。
    let decisions = store.decisions_list(Some(root_id)).unwrap();
    assert_eq!(decisions.len(), 1, "{decisions:?}");
    let h1 = &decisions[0];
    assert_eq!(h1.key, "h1");
    assert_eq!(h1.status, task_core::DecisionStatus::Open);
    assert_eq!(
        h1.request.raised_by.origin,
        task_core::DecisionOrigin::Planner
    );
    d.tick().unwrap();
    assert!(
        store.children(root_id).unwrap().is_empty(),
        "an open decision still holds the unit"
    );
    // 人が h1 に答える（API / MCP と同じ `task_ops::decision::answer`）。
    task_ops::decision::answer(
        store.as_ref(),
        &h1.id,
        Some("manual"),
        Some("trial first"),
        "human",
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    let report = run_until_idle(&mut d, 800).await;
    assert!(report.idle, "{report:?}");
    let children = store.children(root_id).unwrap();
    assert_eq!(children.len(), 1);
    let objective = &children[0].objective;
    assert!(
        objective.contains(task_ops::tree::DECISIONS_HEADING),
        "{objective}"
    );
    assert!(
        objective.contains("- h1 which backend: manual（推奨と異なる） — trial first"),
        "{objective}"
    );
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Done);
    // 子の生成は a の done の後。
    let events = store.events_for(root_id).unwrap();
    let a_done = events
        .iter()
        .position(|(_, e)| matches!(e, Event::WorkUnitTransitioned { key, to: task_core::WorkUnitStatus::Done, .. } if key == "a"))
        .unwrap();
    let created = events
        .iter()
        .position(|(_, e)| matches!(e, Event::ChildTaskCreated { .. }))
        .unwrap();
    assert!(a_done < created);
    assert_replay_is_clean(&store);
}

/// 子を dispatch → running → 指定の終端へ（store の遷移だけ。LLM も adapter も使わない）。
pub(super) fn drive_child(store: &Arc<dyn TaskStore>, id: TaskId, to: Status) {
    match to {
        Status::Cancelled => {
            store.apply_transition(id, Trigger::Cancel, None).unwrap();
        }
        Status::Failed => {
            store.apply_transition(id, Trigger::Dispatch, None).unwrap();
            store
                .apply_transition(id, Trigger::WorkerError { retryable: false }, None)
                .unwrap();
        }
        Status::Done => {
            store.apply_transition(id, Trigger::Dispatch, None).unwrap();
            store
                .apply_transition(id, Trigger::WorkerDone, None)
                .unwrap();
            store
                .apply_transition(id, Trigger::ReviewPass, None)
                .unwrap();
        }
        other => panic!("unsupported {other:?}"),
    }
    assert_eq!(store.get(id).unwrap().unwrap().status, to);
}

pub(super) fn child_of(store: &Arc<dyn TaskStore>, root: TaskId, key: &str) -> TaskId {
    let units = store.work_units_for(root).unwrap();
    unit(&units, key)
        .child_task_id
        .as_deref()
        .unwrap_or_else(|| panic!("unit {key} has no child"))
        .parse()
        .unwrap()
}

/// R1b (c): 子が done → unit done、failed → unit failed（`child_failed`）、cancelled → unit failed
/// （`child_cancelled`。付記 2.）。`max_parallel_child_tasks` を超えては作らない。unit が failed の段階は
/// 完了せず、親は既存の失敗の経路（replan の planner run）に入る。
#[tokio::test]
async fn unit_mirrors_child_status() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = compound_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let spec: task_core::ExecutionPlanSpec = serde_json::from_str(&v3_plan(
        vec![stage("s1", false)],
        vec![
            task_unit("c1", "s1", &[], "true"),
            task_unit("c2", "s1", &[], "true"),
            task_unit("c3", "s1", &[], "true"),
        ],
    ))
    .unwrap();
    task_ops::execution::adopt_plan(
        store.as_ref(),
        root_id,
        spec,
        task_core::PlanOrigin::Human,
        None,
        tree_limits(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    let adapter = Arc::new(TreeAdapter::new(Vec::new(), Duration::ZERO));
    let mut d = tree_dispatcher(&store, adapter);
    assert_eq!(d.config.execution.limits.tree.max_parallel_child_tasks, 2);

    d.reconcile_tree_units().unwrap();
    let units = store.work_units_for(root_id).unwrap();
    assert_eq!(
        unit(&units, "c1").status,
        task_core::WorkUnitStatus::Running
    );
    assert_eq!(
        unit(&units, "c2").status,
        task_core::WorkUnitStatus::Running
    );
    assert_eq!(
        unit(&units, "c3").status,
        task_core::WorkUnitStatus::Ready,
        "max_parallel_child_tasks = 2"
    );
    assert!(unit(&units, "c3").child_task_id.is_none());
    assert_eq!(store.children(root_id).unwrap().len(), 2);
    // 冪等: もう一度照合しても増えない。
    d.reconcile_tree_units().unwrap();
    assert_eq!(store.children(root_id).unwrap().len(), 2);

    // 非終端の子（blocked を含む）は unit を running のままにする。
    let c1 = child_of(&store, root_id, "c1");
    let c2 = child_of(&store, root_id, "c2");
    store
        .apply_transition(c1, Trigger::Unroutable, None)
        .unwrap();
    d.reconcile_tree_units().unwrap();
    let units = store.work_units_for(root_id).unwrap();
    assert_eq!(
        unit(&units, "c1").status,
        task_core::WorkUnitStatus::Running
    );
    store.apply_transition(c1, Trigger::Answer, None).unwrap();

    drive_child(&store, c1, Status::Done);
    drive_child(&store, c2, Status::Failed);
    d.reconcile_tree_units().unwrap();
    let units = store.work_units_for(root_id).unwrap();
    assert_eq!(unit(&units, "c1").status, task_core::WorkUnitStatus::Done);
    assert_eq!(unit(&units, "c2").status, task_core::WorkUnitStatus::Failed);
    // 空きができたので c3 の子が作られた（兄弟の失敗は止めない）。
    assert_eq!(
        unit(&units, "c3").status,
        task_core::WorkUnitStatus::Running
    );
    let c3 = child_of(&store, root_id, "c3");
    drive_child(&store, c3, Status::Cancelled);
    d.reconcile_tree_units().unwrap();
    let units = store.work_units_for(root_id).unwrap();
    assert_eq!(unit(&units, "c3").status, task_core::WorkUnitStatus::Failed);

    let events = store.events_for(root_id).unwrap();
    assert_eq!(
        unit_reasons(&events, "c1"),
        vec!["child_created", "child_done"]
    );
    assert_eq!(
        unit_reasons(&events, "c2"),
        vec!["child_created", "child_failed"]
    );
    assert_eq!(
        unit_reasons(&events, "c3"),
        vec!["child_created", "child_cancelled"]
    );

    // 段階は完了しない（統合 WU は pending のまま）。親は既存の失敗の経路（replan の planner run）へ。
    assert_eq!(
        unit(&units, "integrate-s1").status,
        task_core::WorkUnitStatus::Pending
    );
    assert!(matches!(
        crate::execution_scheduler::settle_phase(&units),
        crate::execution_scheduler::PhaseSettle::Failure(_)
    ));
    assert!(matches!(
        d.wu_dispatch_gate(root_id).unwrap(),
        WuDispatchGate::RunPlanner { replan: true }
    ));
    assert_replay_is_clean(&store);
}

/// R1b (c) の実行の経路: 子の最終レビューが落ち続けて子が failed になると、unit は failed、段階は完了せず、
/// 親は done にならない（replan〈R2b〉の中身はまだ無いので、既存の失敗の経路に入ることだけを見る）。
#[tokio::test]
async fn child_failure_fails_the_unit_and_the_stage_does_not_complete() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = compound_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = v3_plan(
        vec![stage("s1", false)],
        vec![leaf("a", "s1", &[]), task_unit("c", "s1", &[], "false")],
    );
    let adapter = Arc::new(TreeAdapter::new(vec![plan], Duration::ZERO));
    let mut d = tree_dispatcher(&store, adapter);
    d.config.execution.max_replans = 0;
    run_until_idle(&mut d, 800).await;

    let child = child_of(&store, root_id, "c");
    assert_eq!(store.get(child).unwrap().unwrap().status, Status::Failed);
    let units = store.work_units_for(root_id).unwrap();
    assert_eq!(unit(&units, "c").status, task_core::WorkUnitStatus::Failed);
    assert_ne!(
        unit(&units, "integrate-s1").status,
        task_core::WorkUnitStatus::Done
    );
    assert_ne!(store.get(root_id).unwrap().unwrap().status, Status::Done);
    assert!(
        !store
            .events_for(root_id)
            .unwrap()
            .iter()
            .any(|(_, e)| matches!(e, Event::PhaseIntegrated { .. }))
    );
    assert_replay_is_clean(&store);
}

/// 子の run が走っている（`child_delay` の間）まで tick する。子の id を返す。
pub(super) async fn tick_until_child_runs(
    d: &mut Dispatcher,
    store: &Arc<dyn TaskStore>,
    root: TaskId,
) -> TaskId {
    for _ in 0..300 {
        d.tick().unwrap();
        if let Some(child) = store.children(root).unwrap().first()
            && child.status == Status::Running
            && child.lease.is_some()
        {
            return child.id;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!(
        "the child never started running: {:?}",
        store.events_for(root).unwrap()
    );
}

/// R1b (d): 親が子だけを待つ間は `Ready` のまま dispatch されず（lease なし、gate は Skip）、
/// `GET /tasks/{id}/execution` の phase が `awaiting_children`（待っている子の題名と状態つき）。
#[tokio::test]
async fn parent_waits_without_lease() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = compound_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = v3_plan(
        vec![stage("s1", false)],
        vec![task_unit("c", "s1", &[], "true")],
    );
    let adapter = Arc::new(TreeAdapter::new(vec![plan], Duration::from_secs(600)));
    let mut d = tree_dispatcher(&store, adapter);
    let child = tick_until_child_runs(&mut d, &store, root_id).await;
    for _ in 0..5 {
        d.tick().unwrap();
    }
    let stored_root = store.get(root_id).unwrap().unwrap();
    assert_eq!(stored_root.status, Status::Ready);
    assert!(stored_root.lease.is_none(), "the parent holds no lease");
    assert!(matches!(
        d.wu_dispatch_gate(root_id).unwrap(),
        WuDispatchGate::Skip
    ));
    let detail = task_ops::view::task_detail(
        store.as_ref(),
        root_id,
        &view_ctx(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    let execution = detail.execution.expect("execution view");
    assert_eq!(
        execution.phase,
        Some(task_ops::view::ExecutionPhase::AwaitingChildren)
    );
    assert_eq!(execution.awaiting_children.len(), 1);
    let waiting = &execution.awaiting_children[0];
    assert_eq!(waiting.unit_key, "c");
    assert_eq!(waiting.task_id, Some(child));
    assert_eq!(waiting.title, "Child c");
    assert_eq!(waiting.status, Some(Status::Running));
    // 親の worker / planner run はこの間 1 本も増えない（planner の 1 本だけ）。
    let runs = store.runs_for_task(root_id).unwrap();
    assert_eq!(runs.len(), 1, "{runs:?}");
}

/// R1b (e): root の Cancel で子・孫の非終端 task が `cancelled`（reason `parent_cancelled`）になり、
/// 走っている子の run が止まり、root の unit も cancelled になる。
#[tokio::test]
async fn cancel_cascades_to_subtree() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = compound_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = v3_plan(
        vec![stage("s1", false)],
        vec![task_unit("c", "s1", &[], "true")],
    );
    let adapter = Arc::new(TreeAdapter::new(vec![plan], Duration::from_secs(600)));
    let mut d = tree_dispatcher(&store, adapter);
    let child_id = tick_until_child_runs(&mut d, &store, root_id).await;
    let child = store.get(child_id).unwrap().unwrap();
    let child_run = child.lease.clone().expect("leased").worker_run_id;
    // 孫（深さ 3。子の計画の unit から作られたもの）を 1 つ足す（draft のまま）。
    let mut grandchild = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        1,
    );
    grandchild.status = Status::Draft;
    grandchild.parent_id = Some(child_id);
    grandchild.tree = Some(task_core::TreeInfo::child_of(
        &child,
        task_core::ParentUnit {
            task_id: child_id,
            plan_id: "child-plan".into(),
            unit_key: "g".into(),
            stage: "g1".into(),
            attempt: 1,
        },
        None,
    ));
    assert_eq!(grandchild.tree.as_ref().unwrap().depth, 3);
    store.insert(&grandchild).unwrap();

    let result = task_ops::gate::cancel(store.as_ref(), root_id, None).unwrap();
    assert_eq!(result.to, Status::Cancelled);
    let mut cascaded: Vec<TaskId> = result.cascaded.iter().map(|t| t.id).collect();
    cascaded.sort();
    let mut expected = vec![child_id, grandchild.id];
    expected.sort();
    assert_eq!(cascaded, expected);
    for id in [child_id, grandchild.id] {
        assert_eq!(store.get(id).unwrap().unwrap().status, Status::Cancelled);
        let reason = store
            .events_for(id)
            .unwrap()
            .iter()
            .rev()
            .find_map(|(_, e)| match e {
                Event::Transitioned { reason, .. } => Some(reason.clone()),
                _ => None,
            });
        assert_eq!(reason.as_deref(), Some("parent_cancelled"), "{id}");
    }
    d.tick().unwrap();
    let row = store.run_index_get(&child_run).unwrap().expect("runs row");
    assert_eq!(row.status, task_core::RunIndexStatus::Cancelled, "{row:?}");
    let units = store.work_units_for(root_id).unwrap();
    assert!(
        units
            .iter()
            .all(|u| u.status == task_core::WorkUnitStatus::Cancelled),
        "{units:?}"
    );
    assert_replay_is_clean(&store);
}

/// R1b (f): 木の節点の run の `RunContext.available_genres` は空（委譲できない）。木でない task は従来どおり。
#[tokio::test]
async fn tree_runs_cannot_delegate() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = compound_task(dir.path());
    store.create_task(&root, vec![]).unwrap();
    let mut child = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        1,
    );
    child.parent_id = Some(root.id);
    child.tree = Some(task_core::TreeInfo::child_of(
        &root,
        task_core::ParentUnit {
            task_id: root.id,
            plan_id: "p".into(),
            unit_key: "c".into(),
            stage: "s1".into(),
            attempt: 1,
        },
        None,
    ));
    store.insert(&child).unwrap();
    let adapter = Arc::new(TreeAdapter::new(Vec::new(), Duration::ZERO));
    let mut d = tree_dispatcher(&store, adapter);
    d.config.genres = vec![task_core::GenreSpec {
        id: "coding".into(),
        description: "write and fix code".into(),
        ..task_core::GenreSpec::default()
    }];
    let extras = d.run_extras(&child, None, None, "claude-code").unwrap();
    assert!(extras.available_genres.is_empty());
    let mut plain = child.clone();
    plain.tree = None;
    let extras = d.run_extras(&plain, None, None, "claude-code").unwrap();
    assert_eq!(
        extras.available_genres.len(),
        1,
        "non-tree tasks are unchanged"
    );
}

/// R1a から持ち越し（ADR-0079 D2 / D5）: `review: human` の段階は ADR-0074 D2 の途中確認になる。段階 s1 の
/// 統合の後に `awaiting_human` で止まり、「続ける」で s2 に進んで done。
#[tokio::test]
async fn review_human_stage_pauses_after_integration() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = compound_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = v3_plan(
        vec![stage("s1", true), stage("s2", false)],
        vec![leaf("a", "s1", &[]), leaf("b", "s2", &[])],
    );
    let adapter = Arc::new(TreeAdapter::new(vec![plan], Duration::ZERO));
    let mut d = tree_dispatcher(&store, adapter);
    let report = run_until_idle(&mut d, 600).await;
    assert!(report.idle, "{report:?}");
    let stored = store.get(root_id).unwrap().unwrap();
    let events = store.events_for(root_id).unwrap();
    assert_eq!(stored.status, Status::Blocked, "{events:?}");
    assert!(task_ops::phase_gate::is_awaiting_human(&stored, &events));
    let plan_id = store.execution_plan_active(root_id).unwrap().unwrap().id;
    assert!(events.iter().any(|(_, e)| matches!(
        e,
        Event::PausePointsResolved { plan_id: p, phases, .. } if *p == plan_id && *phases == vec!["s1".to_string()]
    )));
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, Event::PhaseReported { phase, .. } if phase == "s1"))
    );
    let units = store.work_units_for(root_id).unwrap();
    assert_eq!(unit(&units, "b").status, task_core::WorkUnitStatus::Ready);
    assert_eq!(unit(&units, "b").runs, 0, "s2 waits for the human");

    task_ops::phase_gate::phase_gate(
        store.as_ref(),
        root_id,
        task_ops::phase_gate::PhaseGateAction::Continue,
        None,
    )
    .unwrap();
    let report = run_until_idle(&mut d, 600).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Done);
    assert_replay_is_clean(&store);
}

/// R1a から持ち越し（ADR-0079 D2）: kind task の unit の `repos` は親の repos の部分集合でなければならない。
/// 外れた計画は不正な試行として拒否され（理由が進行に残る）、次の正しい計画が採用される。
#[tokio::test]
async fn task_unit_repos_must_be_a_subset_of_the_parents() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = compound_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let mut bad_unit = task_unit("c", "s1", &[], "true");
    bad_unit["repos"] = serde_json::json!(["elsewhere"]);
    let bad = v3_plan(vec![stage("s1", false)], vec![bad_unit]);
    let good = v3_plan(
        vec![stage("s1", false)],
        vec![task_unit("c", "s1", &[], "true")],
    );
    let adapter = Arc::new(TreeAdapter::new(vec![bad, good], Duration::ZERO));
    let mut d = tree_dispatcher(&store, adapter);
    let report = run_until_idle(&mut d, 800).await;
    assert!(report.idle, "{report:?}");
    let events = store.events_for(root_id).unwrap();
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            Event::WorkerProgress { msg, .. } if msg.contains("repos \"elsewhere\" is not one of the parent's repos")
        )),
        "{events:?}"
    );
    let plans = store.execution_plan_list(root_id).unwrap();
    assert_eq!(plans.len(), 1, "only the valid plan is adopted");
    assert!(plans[0].spec.units[0].repos.is_empty());
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Done);
    assert_eq!(store.children(root_id).unwrap().len(), 1);
}

/// R1b (g): `[execution.tree] enabled = false`（既定）では /3 は採用されず、子 task も作られない
/// （/1・/2 と plan/2 の children の既存テストはそのまま通る）。
#[tokio::test]
async fn tree_disabled_rejects_v3_and_creates_no_child() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = compound_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = v3_plan(
        vec![stage("s1", false)],
        vec![task_unit("c", "s1", &[], "true")],
    );
    let adapter = Arc::new(TreeAdapter::new(vec![plan.clone(), plan], Duration::ZERO));
    let mut d = tree_dispatcher(&store, adapter);
    d.config.execution.limits = task_core::ExecutionLimits::default();
    run_until_idle(&mut d, 600).await;
    assert!(store.children(root_id).unwrap().is_empty());
    assert!(store.tasks_with_open_task_units().unwrap().is_empty());
    let events = store.events_for(root_id).unwrap();
    assert!(!events.iter().any(|(_, e)| matches!(
        e,
        Event::ChildTaskCreated { .. } | Event::ExecutionPlanned { .. }
    )));
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            Event::WorkerProgress { msg, .. } if msg.contains("[execution.tree] enabled = true")
        )),
        "{events:?}"
    );
}

/// R1b の replay: 子の結び付き（`work_units.child_task_id`）と写した unit の状態は events だけから
/// 作り直せる（`celerisctl replay --check` は差分 0）。索引を壊すと `--check` が見つけ、`--apply` で戻る。
#[tokio::test]
async fn replay_rebuilds_child_links_and_unit_statuses() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = compound_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let spec: task_core::ExecutionPlanSpec = serde_json::from_str(&v3_plan(
        vec![stage("s1", false)],
        vec![
            task_unit("c1", "s1", &[], "true"),
            task_unit("c2", "s1", &[], "true"),
        ],
    ))
    .unwrap();
    task_ops::execution::adopt_plan(
        store.as_ref(),
        root_id,
        spec,
        task_core::PlanOrigin::Human,
        None,
        tree_limits(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    let adapter = Arc::new(TreeAdapter::new(Vec::new(), Duration::ZERO));
    let mut d = tree_dispatcher(&store, adapter);
    d.reconcile_tree_units().unwrap();
    let c1 = child_of(&store, root_id, "c1");
    let c2 = child_of(&store, root_id, "c2");
    drive_child(&store, c1, Status::Done);
    drive_child(&store, c2, Status::Failed);
    d.reconcile_tree_units().unwrap();
    assert_replay_is_clean(&store);

    // 索引を壊す: c1 の結び付きを消して pending に、c2 を running に戻す。
    let original = store.work_units_for(root_id).unwrap();
    let broken: Vec<task_core::WorkUnitRow> = original
        .iter()
        .cloned()
        .map(|mut u| {
            if u.key == "c1" {
                u.child_task_id = None;
                u.status = task_core::WorkUnitStatus::Pending;
            } else if u.key == "c2" {
                u.status = task_core::WorkUnitStatus::Running;
            }
            u
        })
        .collect();
    store.work_units_replace(root_id, broken).unwrap();
    let (wu, _, _, _) = task_ops::replay::check_and_apply_execution(store.as_ref(), false).unwrap();
    let fields: std::collections::BTreeSet<(String, String)> = wu
        .iter()
        .map(|m| (m.key.clone(), m.field.clone()))
        .collect();
    assert!(
        fields.contains(&("c1".to_string(), "child_task_id".to_string())),
        "{wu:?}"
    );
    assert!(
        fields.contains(&("c1".to_string(), "status".to_string())),
        "{wu:?}"
    );
    assert!(
        fields.contains(&("c2".to_string(), "status".to_string())),
        "{wu:?}"
    );
    let (_, _, _, applied) =
        task_ops::replay::check_and_apply_execution(store.as_ref(), true).unwrap();
    assert_eq!(applied, 1);
    let restored = store.work_units_for(root_id).unwrap();
    for key in ["c1", "c2"] {
        let (a, b) = (unit(&original, key), unit(&restored, key));
        assert_eq!(
            (a.status, &a.child_task_id),
            (b.status, &b.child_task_id),
            "{key}"
        );
    }
    assert_replay_is_clean(&store);
}
