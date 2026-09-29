//! ADR-0079 §7 R3b: root の計画の承認（D8）、木の生存確認（D10）、R3a 付記 15. の「人の replan の 1 回目が不正だと
//! 2 回目が起きない」の修正。承認の要否は決定的（決定を含む・`review: human` の段階・上限の 0.8 以上）で、承認まで
//! unit を 1 つも起こさない。承認の要らない root の計画は報告の流れに 1 件だけ残して進む。生存確認は名指しの待ちを
//! 検出せず、理由なく止まった節点だけを偽の時計で 600 秒後に 1 回だけ `StallDetected` と障害通知にする。
//! すべて偽のアダプタと一時ディレクトリだけで、外部ネットワークに出ない。

use std::collections::VecDeque;

use super::tree::{
    approve_root_plan, assert_replay_is_clean, leaf, stage, task_unit, tick_until_child_runs,
    tree_dispatcher, unit, v3_plan,
};
use super::*;

/// planner run には計画の列を順に書き（列が尽きたら何も書かない = 不正な試行）、planner の文脈を記録する。
/// それ以外の run は `child_delay` だけ待ってから `Done`（子 task の run = `work_unit` の無い run だけ待つ）。
struct ScriptAdapter {
    plans: StdMutex<VecDeque<String>>,
    planners: StdMutex<Vec<task_worker::protocol::ExecutionPlannerContext>>,
    child_delay: Duration,
}

impl ScriptAdapter {
    fn new(plans: Vec<&str>, child_delay: Duration) -> Self {
        ScriptAdapter {
            plans: StdMutex::new(plans.into_iter().map(str::to_string).collect()),
            planners: StdMutex::new(Vec::new()),
            child_delay,
        }
    }

    fn planner_contexts(&self) -> Vec<task_worker::protocol::ExecutionPlannerContext> {
        self.planners.lock().unwrap().clone()
    }
}

#[async_trait]
impl WorkerAdapter for ScriptAdapter {
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
        if let Some(ctx) = req.context.execution_planner.clone() {
            self.planners.lock().unwrap().push(ctx);
            if let Some(json) = self.plans.lock().unwrap().pop_front() {
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

/// 決定を待って止めた仕事・承認待ちの木は `idle` にならないことがある。run・判定・統合が 50 tick 続けて無ければ
/// 落ち着いたとみなす（tree_gate / tree_decisions と同じ）。
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

/// root（人の明示の compound）。子が継ぐ予算を 30 turns にする（tree_gate の `gate_root` と同じ）。
fn root_task(dir: &std::path::Path) -> Task {
    let mut root = compound_task(dir);
    root.title = "browser capability".into();
    root.budget.max_turns = 30;
    root
}

fn decision_spec(key: &str, needed_before: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "key": key,
        "question": format!("question {key}"),
        "options": [{"key": "org-vault", "label": "org vault"}, {"key": "manual", "label": "manual entry"}],
        "recommended": "org-vault",
        "cost_of_reversal": "medium",
        "needed_before": needed_before,
    })
}

fn with_decisions(plan: String, decisions: Vec<serde_json::Value>) -> String {
    let mut value: serde_json::Value = serde_json::from_str(&plan).unwrap();
    value["decisions"] = serde_json::Value::Array(decisions);
    value.to_string()
}

fn seed_secretary(store: &Arc<dyn TaskStore>) {
    let now = OffsetDateTime::now_utc();
    store
        .org_upsert(&OrgNode {
            profile: Default::default(),
            id: "secretary".into(),
            parent_id: None,
            name: "secretary".into(),
            kind: OrgKind::Secretary,
            genre: None,
            brief: String::new(),
            position: 0,
            created_at: now,
            updated_at: now,
        })
        .unwrap();
}

fn plan_approval(store: &Arc<dyn TaskStore>, id: TaskId) -> Option<Vec<String>> {
    let task = store.get(id).unwrap().unwrap();
    let events = store.events_for(id).unwrap();
    task_ops::plan_gate::latest_plan_approval(&task, &events).map(|i| i.reasons)
}

fn reports_of(store: &Arc<dyn TaskStore>, id: TaskId) -> Vec<task_core::report::Report> {
    store
        .report_list(&task_core::ReportFilter {
            task_id: Some(id),
            ..Default::default()
        })
        .unwrap()
}

fn stalls(store: &Arc<dyn TaskStore>, id: TaskId) -> Vec<(String, String)> {
    store
        .events_for(id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::StallDetected { reason, since, .. } => Some((reason, since)),
            _ => None,
        })
        .collect()
}

fn stall_notifications(store: &Arc<dyn TaskStore>) -> Vec<task_core::notify::Notification> {
    store
        .notification_pending()
        .unwrap()
        .into_iter()
        .filter(|n| n.key.starts_with("tree-stall:"))
        .collect()
}

/// ADR-0079 §7 R3b (a): D8 の 3 つの条件（決定を含む / `review: human` の段階 / 上限の 0.8 以上）はそれぞれ単独で
/// root の計画を `awaiting_plan_approval` で止める。止まっている間は planner の 1 本だけで、unit は 1 つも走らず、
/// 子も作られず、報告（承認不要の報告）も残らない。質問ではない（`Answer` は 409）。
#[tokio::test]
async fn each_approval_trigger_holds_the_root_plan() {
    let cases: Vec<(&str, String, Vec<&str>)> = vec![
        (
            "decisions",
            with_decisions(
                v3_plan(
                    vec![stage("s1", false)],
                    vec![leaf("a", "s1", &[]), task_unit("c", "s1", &[], "true")],
                ),
                vec![decision_spec("h1", &["c"])],
            ),
            vec!["decisions:h1"],
        ),
        (
            "review human",
            v3_plan(
                vec![stage("s1", true), stage("s2", false)],
                vec![leaf("a", "s1", &[]), leaf("b", "s2", &[])],
            ),
            vec!["review_human:s1"],
        ),
        (
            "near the stage limit (4 of 5)",
            v3_plan(
                vec![
                    stage("s1", false),
                    stage("s2", false),
                    stage("s3", false),
                    stage("s4", false),
                ],
                vec![
                    leaf("a", "s1", &[]),
                    leaf("b", "s2", &[]),
                    leaf("c", "s3", &[]),
                    leaf("d", "s4", &[]),
                ],
            ),
            vec!["near_limit:max_stages:4/5"],
        ),
    ];
    for (name, plan, reasons) in cases {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
        seed_secretary(&store);
        let root = root_task(dir.path());
        let root_id = root.id;
        store.create_task(&root, vec![]).unwrap();
        let adapter = Arc::new(ScriptAdapter::new(vec![&plan], Duration::ZERO));
        let mut d = tree_dispatcher(&store, adapter);
        let report = run_until_idle(&mut d, 400).await;
        assert!(report.idle, "{name}: {report:?}");
        let stored = store.get(root_id).unwrap().unwrap();
        assert_eq!(stored.status, Status::Blocked, "{name}");
        assert!(stored.lease.is_none(), "{name}");
        assert_eq!(
            plan_approval(&store, root_id),
            Some(reasons.iter().map(|r| r.to_string()).collect()),
            "{name}"
        );
        let plan_id = store.execution_plan_active(root_id).unwrap().unwrap().id;
        let events = store.events_for(root_id).unwrap();
        assert!(
            events.iter().any(|(_, e)| matches!(
                e,
                Event::PlanApprovalRequested { plan_id: p, .. } if *p == plan_id
            )),
            "{name}"
        );
        assert_eq!(
            task_ops::phase_gate::last_transition_reason(&events),
            Some(task_ops::plan_gate::AWAITING_PLAN_APPROVAL),
            "{name}"
        );
        // planner の 1 本だけ。unit は走らず、子も作らない。tick を重ねても変わらない。
        for _ in 0..5 {
            d.tick().unwrap();
        }
        assert_eq!(store.runs_for_task(root_id).unwrap().len(), 1, "{name}");
        assert!(
            store
                .work_units_for(root_id)
                .unwrap()
                .iter()
                .all(|u| u.runs == 0 && u.child_task_id.is_none()),
            "{name}"
        );
        assert!(store.children(root_id).unwrap().is_empty(), "{name}");
        assert!(reports_of(&store, root_id).is_empty(), "{name}");
        // 質問ではない。
        let err = task_ops::gate::answer(store.as_ref(), root_id, "go".into(), None).unwrap_err();
        assert!(
            matches!(err, task_ops::OpsError::InvalidState { .. }),
            "{name}: {err}"
        );
        let view = task_ops::view::actions_with_events(&stored, &events);
        assert!(view.contains(&task_ops::view::Action::PlanGate), "{name}");
        assert!(!view.contains(&task_ops::view::Action::Answer), "{name}");
        assert_replay_is_clean(&store);
    }
}

/// ADR-0079 §7 R3b (a): approve で unit が起きる（決定への回答とは別: 承認しても答えの無い決定に依存する unit は
/// 待ち、回答で進む）。replan（note）で planner が note を受け取って新しい版を書く（その版にも同じ規則。
/// 条件が無ければ承認なしで進む）。withdraw で root を中止する。
#[tokio::test]
async fn approve_replan_and_withdraw_have_their_effects() {
    // approve。
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = root_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = with_decisions(
        v3_plan(
            vec![stage("s1", false)],
            vec![leaf("a", "s1", &[]), leaf("b", "s1", &[])],
        ),
        vec![decision_spec("h1", &["b"])],
    );
    let adapter = Arc::new(ScriptAdapter::new(vec![&plan], Duration::ZERO));
    let mut d = tree_dispatcher(&store, adapter);
    run_until_idle(&mut d, 400).await;
    assert_eq!(
        approve_root_plan(&store, root_id),
        vec!["decisions:h1".to_string()]
    );
    let events = store.events_for(root_id).unwrap();
    assert_eq!(
        task_ops::phase_gate::last_transition_reason(&events),
        Some("plan_approved")
    );
    run_until_quiet(&mut d, &store, 800).await;
    let units = store.work_units_for(root_id).unwrap();
    assert_eq!(unit(&units, "a").status, task_core::WorkUnitStatus::Done);
    assert_eq!(unit(&units, "b").status, task_core::WorkUnitStatus::Blocked);
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Ready);
    let h1 = store
        .decisions_list(Some(root_id))
        .unwrap()
        .into_iter()
        .find(|r| r.key == "h1")
        .unwrap();
    task_ops::decision::answer(
        store.as_ref(),
        &h1.id,
        Some("manual"),
        None,
        "human",
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    let report = run_until_idle(&mut d, 800).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Done);
    assert_replay_is_clean(&store);

    // replan（note 必須）。v2 は条件を持たないので承認なしで進み、報告が 1 件残る。
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_secretary(&store);
    let root = root_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let v1 = v3_plan(
        vec![stage("s1", true), stage("s2", false)],
        vec![leaf("a", "s1", &[]), leaf("b", "s2", &[])],
    );
    let v2 = v3_plan(
        vec![stage("s1", false)],
        vec![leaf("a", "s1", &[]), leaf("b", "s1", &[])],
    );
    let adapter = Arc::new(ScriptAdapter::new(vec![&v1, &v2], Duration::ZERO));
    let mut d = tree_dispatcher(&store, adapter.clone());
    run_until_idle(&mut d, 400).await;
    assert_eq!(
        plan_approval(&store, root_id),
        Some(vec!["review_human:s1".to_string()])
    );
    let err = task_ops::plan_gate::plan_gate(
        store.as_ref(),
        root_id,
        task_ops::plan_gate::PlanGateAction::Replan,
        Some("  ".into()),
        "human",
    )
    .unwrap_err();
    assert!(matches!(err, task_ops::OpsError::Validation(_)), "{err}");
    let r = task_ops::plan_gate::plan_gate(
        store.as_ref(),
        root_id,
        task_ops::plan_gate::PlanGateAction::Replan,
        Some("merge the two stages".into()),
        "human",
    )
    .unwrap();
    assert_eq!(r.reason, "plan_replan");
    let report = run_until_idle(&mut d, 800).await;
    assert!(report.idle, "{report:?}");
    let contexts = adapter.planner_contexts();
    assert_eq!(contexts.len(), 2);
    assert!(contexts[1].replan);
    assert!(
        contexts[1].replan_reason.contains("merge the two stages"),
        "{}",
        contexts[1].replan_reason
    );
    assert_eq!(store.execution_plan_list(root_id).unwrap().len(), 2);
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Done);
    let notices = reports_of(&store, root_id);
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(notices[0].kind, task_core::report::ReportKind::Progress);
    assert!(
        notices[0]
            .headline
            .starts_with("計画を採用して進めます: Stage s1"),
        "{}",
        notices[0].headline
    );
    // replay: task・計画・決定は一致する。`work_units` は /3 の replan で同じ key の unit を作り直したときの既存の差
    // （R2b / R3a 付記 14. の「superseded の unit を最後の版から作り直せない」と同じ系統。R4a）があるので比べない。
    let report = task_ops::replay::replay(store.as_ref()).unwrap();
    assert!(report.mismatches.is_empty(), "{:?}", report.mismatches);
    let (_wu, _runs, plans, _) =
        task_ops::replay::check_and_apply_execution(store.as_ref(), false).unwrap();
    assert!(plans.is_empty(), "{plans:?}");
    let (decisions, _) =
        task_ops::replay::check_and_apply_decisions(store.as_ref(), false).unwrap();
    assert!(decisions.is_empty(), "{decisions:?}");

    // withdraw。
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = root_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let adapter = Arc::new(ScriptAdapter::new(vec![&v1], Duration::ZERO));
    let mut d = tree_dispatcher(&store, adapter);
    run_until_idle(&mut d, 400).await;
    assert!(plan_approval(&store, root_id).is_some());
    let r = task_ops::plan_gate::plan_gate(
        store.as_ref(),
        root_id,
        task_ops::plan_gate::PlanGateAction::Withdraw,
        None,
        "human",
    )
    .unwrap();
    assert_eq!(r.to, Status::Cancelled);
    run_until_idle(&mut d, 200).await;
    assert_eq!(
        store.get(root_id).unwrap().unwrap().status,
        Status::Cancelled
    );
    assert_eq!(store.runs_for_task(root_id).unwrap().len(), 1);
    assert_replay_is_clean(&store);
}

/// ADR-0079 §7 R3b (b) `small_root_plan_proceeds_with_notice`: 決定も `review: human` も無く上限の 0.8 未満の root の
/// /3 は承認なしで進み、報告の流れに「計画を採用して進めます: <段階の一覧>」が 1 件だけ残る（通知は作らない）。
/// 子の計画は決定を含んでも承認を求めない（子の決定の要求は出る）。
#[tokio::test]
async fn small_root_plan_proceeds_with_notice() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_secretary(&store);
    let root = root_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let mut child_unit = task_unit("c", "s1", &[], "true");
    // 強制規則で compound（深さ 2 でも planner に進む。tree_gate の `broad`）。
    child_unit["features"] =
        serde_json::json!({"expected_length": "high", "cross_cutting": "high"});
    let root_plan = v3_plan(
        vec![stage("s1", false), stage("s2", false)],
        vec![leaf("a", "s1", &[]), child_unit, leaf("z", "s2", &[])],
    );
    let child_plan = with_decisions(
        v3_plan(
            vec![stage("t1", false)],
            vec![leaf("x", "t1", &[]), leaf("y", "t1", &[])],
        ),
        vec![decision_spec("k1", &["y"])],
    );
    let adapter = Arc::new(ScriptAdapter::new(
        vec![&root_plan, &child_plan],
        Duration::ZERO,
    ));
    let mut d = tree_dispatcher(&store, adapter);
    run_until_quiet(&mut d, &store, 1500).await;
    assert!(plan_approval(&store, root_id).is_none());
    let root_events = store.events_for(root_id).unwrap();
    assert!(
        !root_events
            .iter()
            .any(|(_, e)| matches!(e, Event::PlanApprovalRequested { .. }))
    );
    let notices = reports_of(&store, root_id);
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(notices[0].kind, task_core::report::ReportKind::Progress);
    assert_eq!(
        notices[0].headline,
        "計画を採用して進めます: Stage s1 → Stage s2"
    );
    assert!(
        notices[0].body.contains("c: Child c（子 task）"),
        "{}",
        notices[0].body
    );
    assert!(
        store.notification_pending().unwrap().is_empty(),
        "no notification from the daemon"
    );
    // 子は自分の計画（決定 k1 を含む）を承認なしで採用し、k1 に依存しない x は走る。
    let child_id = super::tree::child_of(&store, root_id, "c");
    let child = store.get(child_id).unwrap().unwrap();
    let child_events = store.events_for(child_id).unwrap();
    assert!(store.execution_plan_active(child_id).unwrap().is_some());
    assert!(
        !child_events
            .iter()
            .any(|(_, e)| matches!(e, Event::PlanApprovalRequested { .. })),
        "a child's plan never asks for approval"
    );
    assert_ne!(child.status, Status::Blocked);
    assert!(
        reports_of(&store, child_id).is_empty(),
        "no notice for a child plan"
    );
    let child_units = store.work_units_for(child_id).unwrap();
    assert_eq!(
        unit(&child_units, "x").status,
        task_core::WorkUnitStatus::Done
    );
    assert!(
        store
            .decisions_list(Some(root_id))
            .unwrap()
            .iter()
            .any(|r| r.key == "k1" && r.task_id == child_id)
    );
}

/// ADR-0079 §7 R3b (c) `liveness_flags_only_unexplained_stalls`（偽の時計）:
/// - 名指しの待ち（答えの無い決定を待つ unit・計画の承認待ち）は何秒たっても検出しない。
/// - 子を待つ unit の子が消えた親（「ready だが何も予定されていない」）は、600 秒たったところで `StallDetected` と
///   障害通知（`TaskFailed`、key `tree-stall:<id>:<seq>`）を 1 回だけ出し、その後は繰り返さない。
#[tokio::test]
async fn liveness_flags_only_unexplained_stalls() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    // held: 決定 h1 を待つ leaf b（承認済み）。awaiting: 承認待ちの root。
    let held = root_task(dir.path());
    let held_id = held.id;
    store.create_task(&held, vec![]).unwrap();
    let held_plan = with_decisions(
        v3_plan(
            vec![stage("s1", false)],
            vec![leaf("a", "s1", &[]), leaf("b", "s1", &[])],
        ),
        vec![decision_spec("h1", &["b"])],
    );
    let adapter = Arc::new(ScriptAdapter::new(vec![&held_plan], Duration::ZERO));
    let mut d = tree_dispatcher(&store, adapter);
    run_until_idle(&mut d, 400).await;
    approve_root_plan(&store, held_id);
    run_until_quiet(&mut d, &store, 800).await;
    let units = store.work_units_for(held_id).unwrap();
    assert_eq!(unit(&units, "b").status, task_core::WorkUnitStatus::Blocked);

    let dir2 = tempfile::tempdir().unwrap();
    let awaiting = root_task(dir2.path());
    let awaiting_id = awaiting.id;
    store.create_task(&awaiting, vec![]).unwrap();
    let adapter = Arc::new(ScriptAdapter::new(vec![&held_plan], Duration::ZERO));
    let mut d2 = tree_dispatcher(&store, adapter);
    run_until_idle(&mut d2, 400).await;
    assert!(plan_approval(&store, awaiting_id).is_some());

    // 偽の時計: 0 → 599 → 630 → 1,300 → 2,000 秒。
    let t0 = OffsetDateTime::now_utc() + time::Duration::minutes(5);
    let clock = Arc::new(StdMutex::new(t0));
    d.test_now = Some(clock.clone());
    for secs in [0, 599, 630, 1300, 2000] {
        *clock.lock().unwrap() = t0 + time::Duration::seconds(secs);
        d.tick().unwrap();
    }
    assert!(stalls(&store, held_id).is_empty());
    assert!(stalls(&store, awaiting_id).is_empty());
    assert!(stall_notifications(&store).is_empty());

    // 子が消えた親（子を待つ unit の `child_task_id` が存在しない task を指す）。
    let dir3 = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = root_task(dir3.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = v3_plan(
        vec![stage("s1", false)],
        vec![task_unit("c", "s1", &[], "true")],
    );
    let adapter = Arc::new(ScriptAdapter::new(vec![&plan], Duration::from_secs(3600)));
    let mut d = tree_dispatcher(&store, adapter);
    tick_until_child_runs(&mut d, &store, root_id).await;
    let units = store.work_units_for(root_id).unwrap();
    let c = unit(&units, "c");
    let mut row = c.clone();
    row.child_task_id = Some(TaskId::new().to_string());
    store
        .work_unit_transition(
            root_id,
            row,
            Event::WorkUnitTransitioned {
                work_unit_id: c.id.clone(),
                key: c.key.clone(),
                from: c.status,
                to: c.status,
                reason: "test_lost_child".into(),
                run_id: None,
            },
        )
        .unwrap();
    let t0 = OffsetDateTime::now_utc() + time::Duration::minutes(5);
    let clock = Arc::new(StdMutex::new(t0));
    d.test_now = Some(clock.clone());
    d.tick().unwrap();
    *clock.lock().unwrap() = t0 + time::Duration::seconds(599);
    d.tick().unwrap();
    assert!(stalls(&store, root_id).is_empty(), "not before 600 s");
    // 生存確認は 30 秒ごと（`LIVENESS_CHECK_INTERVAL_SECS`）なので、600 秒を過ぎた最初の確認で出る。
    *clock.lock().unwrap() = t0 + time::Duration::seconds(630);
    d.tick().unwrap();
    let found = stalls(&store, root_id);
    let task_now = store.get(root_id).unwrap().unwrap();
    let facts = task_ops::tree::node_liveness_facts(store.as_ref(), &task_now, true, true).unwrap();
    assert_eq!(
        found.len(),
        1,
        "{facts:?} {:?} {:?}",
        task_core::tree::liveness(&task_core::TreeSnapshot {
            nodes: vec![facts.clone()]
        }),
        store
            .events_for(root_id)
            .unwrap()
            .iter()
            .rev()
            .take(5)
            .collect::<Vec<_>>()
    );
    assert_eq!(found[0].0, "child_missing");
    assert_eq!(found[0].1, rfc3339(t0));
    for secs in [700, 1300, 5000] {
        *clock.lock().unwrap() = t0 + time::Duration::seconds(secs);
        d.tick().unwrap();
    }
    assert_eq!(stalls(&store, root_id).len(), 1, "only once");
    let notes = stall_notifications(&store);
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!(notes[0].kind, NotificationKind::TaskFailed);
    assert!(
        notes[0].key.starts_with(&format!("tree-stall:{root_id}:")),
        "{}",
        notes[0].key
    );
    assert!(notes[0].body.contains("障害（stall）"), "{}", notes[0].body);
    assert!(notes[0].body.contains("child_missing"), "{}", notes[0].body);
    assert!(
        notes[0].body.contains("browser capability"),
        "{}",
        notes[0].body
    );
    let path = store
        .events_for(root_id)
        .unwrap()
        .into_iter()
        .find_map(|(_, e)| match e {
            Event::StallDetected { path, task_id, .. } => Some((path, task_id)),
            _ => None,
        })
        .unwrap();
    assert_eq!(path.1, root_id);
    assert_eq!(path.0.first().map(|p| p.task_id), Some(root_id));
}

/// ADR-0079 R3a 付記 15. の回帰（Phase R3b で修正）: 人の replan（ここでは `limit:max_tree_leaves` への「replan」の
/// 回答 = `ExecutionHintSet{replan: true}`）で起きた planner の 1 回目が不正だと、依頼は 1 回目の
/// `Transitioned{to: running}` で消費済みで 2 回目が起きず、止めた unit は答えの済んだ決定の後ろで理由なく止まって
/// いた。修正後は「もう一度だけ試します」の約束が次の run を planner にし（生存確認の分類は「走れる: planner」）、
/// 2 回目の計画が採用される。修正前の形（再試行の印が無い）なら生存確認が「理由なし」として捕まえる。
#[tokio::test]
async fn human_replan_whose_first_attempt_is_invalid_gets_its_second_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = root_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let v1 = v3_plan(
        vec![stage("s1", false)],
        vec![
            leaf("a", "s1", &[]),
            leaf("b", "s1", &[]),
            leaf("x", "s1", &[]),
        ],
    );
    let v2 = v3_plan(
        vec![stage("s1", false)],
        vec![leaf("a", "s1", &[]), leaf("b", "s1", &[])],
    );
    // 2 本目（replan の 1 回目）は計画を書かない（不正な試行）。
    let adapter = Arc::new(ScriptAdapter::new(
        vec![&v1, "{not json", &v2],
        Duration::ZERO,
    ));
    let mut d = tree_dispatcher(&store, adapter.clone());
    d.config.execution.limits.tree.max_tree_leaves = 2;
    run_until_idle(&mut d, 400).await;
    approve_root_plan(&store, root_id);
    run_until_quiet(&mut d, &store, 800).await;
    let limit = store
        .decisions_list(Some(root_id))
        .unwrap()
        .into_iter()
        .find(|r| r.key == "limit:max_tree_leaves")
        .unwrap();
    task_ops::decision::answer(
        store.as_ref(),
        &limit.id,
        Some("replan"),
        Some("drop x"),
        "human",
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    // replan の planner の 1 回目が dispatch されたら新しい仕事を止め（draining。run の完了は処理する）、1 回目が
    // 不正で終わって ready に戻ったところで様子を見る（止めないと同じ tick で 2 回目が始まる）。
    for _ in 0..300 {
        d.tick().unwrap();
        if store.get(root_id).unwrap().unwrap().status == Status::Running {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    d.accepting_new_work = false;
    for _ in 0..300 {
        d.tick().unwrap();
        if store.get(root_id).unwrap().unwrap().status == Status::Ready {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        task_ops::tree::planner_retry_pending(&store.events_for(root_id).unwrap()),
        "{:#?}",
        store
            .events_for(root_id)
            .unwrap()
            .iter()
            .rev()
            .take(8)
            .collect::<Vec<_>>()
    );
    let task = store.get(root_id).unwrap().unwrap();
    assert_eq!(task.status, Status::Ready);
    assert_eq!(adapter.planner_contexts().len(), 2);
    let facts = task_ops::tree::node_liveness_facts(store.as_ref(), &task, true, true).unwrap();
    assert!(facts.planner_pending, "the retry is promised");
    let verdict = task_core::tree::liveness(&task_core::TreeSnapshot {
        nodes: vec![facts.clone()],
    });
    assert_eq!(
        (verdict[0].class, verdict[0].reason.as_str()),
        (task_core::LivenessClass::Runnable, "planner")
    );
    // 修正前の形（再試行の印を読まない）なら、x は答えの済んだ決定の後ろで理由なく止まっている。
    let mut before_fix = facts;
    before_fix.planner_pending = false;
    let verdict = task_core::tree::liveness(&task_core::TreeSnapshot {
        nodes: vec![before_fix],
    });
    assert_eq!(
        (verdict[0].class, verdict[0].reason.as_str()),
        (task_core::LivenessClass::Unexplained, "decision_released")
    );

    // 2 回目の試行が起き、v2 が採用される（v2 も木の leaf の上限に近いので承認を挟む）。
    d.accepting_new_work = true;
    run_until_idle(&mut d, 800).await;
    let contexts = adapter.planner_contexts();
    assert_eq!(contexts.len(), 3, "the second replan attempt ran");
    assert!(contexts[2].replan);
    assert!(
        contexts[2].replan_reason.contains("drop x"),
        "{}",
        contexts[2].replan_reason
    );
    assert_eq!(store.execution_plan_list(root_id).unwrap().len(), 2);
    let reasons = approve_root_plan(&store, root_id);
    assert!(
        reasons
            .iter()
            .all(|r| r.starts_with("near_limit:max_tree_leaves:")),
        "{reasons:?}"
    );
    let report = run_until_idle(&mut d, 800).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Done);
    assert!(stalls(&store, root_id).is_empty());
}

/// ADR-0079 D15: `[execution.tree] enabled = false` なら承認も報告も生存確認も無く、従来どおり進む。
#[tokio::test]
async fn tree_disabled_is_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_secretary(&store);
    let task = root_task(dir.path());
    let task_id = task.id;
    store.create_task(&task, vec![]).unwrap();
    let plan = plan_json(vec![wu_spec("a", &[]), wu_spec("b", &["a"])]);
    let adapter = Arc::new(ScriptAdapter::new(vec![&plan], Duration::ZERO));
    let mut d = tree_dispatcher(&store, adapter);
    d.config.execution.limits.tree.enabled = false;
    let t0 = OffsetDateTime::now_utc() + time::Duration::minutes(5);
    d.test_now = Some(Arc::new(StdMutex::new(t0)));
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(task_id).unwrap().unwrap().status, Status::Done);
    let events = store.events_for(task_id).unwrap();
    assert!(!events.iter().any(|(_, e)| matches!(
        e,
        Event::PlanApprovalRequested { .. } | Event::StallDetected { .. }
    )));
    assert!(reports_of(&store, task_id).is_empty());
    assert!(d.stall_watch.is_empty());
    assert!(d.liveness_checked_at.is_none());
}
