//! ADR-0079 §7 R3a: 決定の要求の流れ。計画の決定は採用で path 付きの要求になり、答えの無い決定はそれに依存する
//! unit だけを止める。回答（`task_ops::decision::answer`。API / MCP と同じ関数）は待っていた unit を進め、答えを
//! 子の objective の末尾と leaf の前置きに固定の書式で入れる。limit の `raise-once` は止めた仕事を続きから走らせ、
//! `plan_invalid` の replan は人の note を planner に渡し、atomic は節点を 1 run で走らせ、取り下げは止めた unit を
//! 取り下げる。worker の `result.json` の `decisions` は path 付きで記録され、`self` だけがその unit を止める。
//! すべて偽のアダプタと一時ディレクトリだけで、外部ネットワークに出ない。

use std::collections::{BTreeMap, VecDeque};

use super::tree::{
    approve_root_plan, assert_replay_is_clean, leaf, stage, task_unit, tree_dispatcher, unit,
    unit_reasons, v3_plan,
};
use super::*;

/// planner run には計画の列を順に書き、run の文脈を記録する。worker の run は、`declare` に key（WU の key、
/// 子 task なら題名の `Child ` の後）があれば最初の run の `result.json` にその `decisions` を書き、それ以外は
/// `decisions` の無い `result.json` を書く。
struct DecisionAdapter {
    plans: StdMutex<VecDeque<String>>,
    seen: StdMutex<Vec<task_worker::RunContext>>,
    declare: StdMutex<BTreeMap<String, serde_json::Value>>,
}

impl DecisionAdapter {
    fn new(plans: Vec<String>) -> Self {
        DecisionAdapter {
            plans: StdMutex::new(plans.into_iter().collect()),
            seen: StdMutex::new(Vec::new()),
            declare: StdMutex::new(BTreeMap::new()),
        }
    }

    fn declaring(self, key: &str, decisions: serde_json::Value) -> Self {
        self.declare
            .lock()
            .unwrap()
            .insert(key.to_string(), decisions);
        self
    }

    fn contexts(&self) -> Vec<task_worker::RunContext> {
        self.seen.lock().unwrap().clone()
    }

    fn planner_contexts(&self) -> Vec<task_worker::protocol::ExecutionPlannerContext> {
        self.contexts()
            .into_iter()
            .filter_map(|c| c.execution_planner)
            .collect()
    }

    fn work_unit_contexts(&self, key: &str) -> Vec<task_worker::protocol::WorkUnitPromptContext> {
        self.contexts()
            .into_iter()
            .filter_map(|c| c.work_unit)
            .filter(|w| w.key == key)
            .collect()
    }
}

#[async_trait]
impl WorkerAdapter for DecisionAdapter {
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
        std::fs::create_dir_all(&req.artifacts_dir).ok();
        if req.context.execution_planner.is_some() {
            if let Some(json) = self.plans.lock().unwrap().pop_front() {
                std::fs::write(req.artifacts_dir.join("execution-plan.json"), json)
                    .expect("write execution-plan.json");
            }
        } else {
            let key = match &req.context.work_unit {
                Some(w) => w.key.clone(),
                None => req
                    .task
                    .title
                    .strip_prefix("Child ")
                    .unwrap_or("atomic")
                    .to_string(),
            };
            let declared = self.declare.lock().unwrap().remove(&key);
            let result = match declared {
                Some(decisions) => {
                    serde_json::json!({"summary": "ok", "evidence": [], "decisions": decisions})
                }
                None => serde_json::json!({"summary": "ok", "evidence": []}),
            };
            std::fs::write(req.artifacts_dir.join("result.json"), result.to_string())
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

/// 決定を待って止めた仕事がある木は `ready` の task を残すので `idle` にならない。run・判定・統合が 50 tick
/// 続けて無ければ落ち着いたとみなす（tree_gate と同じ）。
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

/// Phase R3b（ADR-0079 D8）: 落ち着くまで回し、root の計画の承認（決定を含む・上限に近い）を確かめて承認し、
/// もう一度落ち着くまで回す。承認の理由を返す。
async fn quiet_then_approve(
    d: &mut Dispatcher,
    store: &Arc<dyn TaskStore>,
    root_id: TaskId,
    max_ticks: usize,
) -> Vec<String> {
    run_until_quiet(d, store, max_ticks).await;
    let reasons = approve_root_plan(store, root_id);
    run_until_quiet(d, store, max_ticks).await;
    reasons
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

fn decision(store: &Arc<dyn TaskStore>, root: TaskId, key: &str) -> task_core::DecisionRow {
    store
        .decisions_list(Some(root))
        .unwrap()
        .into_iter()
        .rfind(|r| r.key == key)
        .unwrap_or_else(|| panic!("no decision {key}"))
}

fn answer(
    store: &Arc<dyn TaskStore>,
    id: &str,
    option: Option<&str>,
    note: Option<&str>,
) -> task_ops::decision::DecisionOutcome {
    task_ops::decision::answer(
        store.as_ref(),
        id,
        option,
        note,
        "human",
        OffsetDateTime::now_utc(),
    )
    .unwrap_or_else(|e| panic!("answer {id}: {e}"))
}

fn blocked_by_decision(units: &[task_core::WorkUnitRow], key: &str) -> bool {
    let u = unit(units, key);
    u.status == task_core::WorkUnitStatus::Blocked
        && u.blocked_reason == Some(task_core::WorkUnitBlockedReason::Decision)
}

fn child_named(store: &Arc<dyn TaskStore>, root: TaskId, key: &str) -> Option<Task> {
    store
        .children(root)
        .unwrap()
        .into_iter()
        .find(|c| c.title == format!("Child {key}"))
}

fn no_questions(store: &Arc<dyn TaskStore>, task: TaskId) {
    assert!(
        store.approval_list(None, None, None).unwrap().is_empty(),
        "no questions / approvals"
    );
    assert!(
        !store
            .events_for(task)
            .unwrap()
            .iter()
            .any(|(_, e)| matches!(e, Event::QuestionRaised { .. })),
        "no QuestionRaised"
    );
}

/// ADR-0079 §7 R3a (a) `unanswered_decision_blocks_only_dependents` と (b) `answer_flows_into_child_objective`:
/// 計画の決定 2 件（h1 は子 task の unit p2-b、h2 は `stage:phase-2`）。採用で 2 件とも planner の path 付きの決定の
/// 要求になり、決定に依存しない phase-1 の leaf は走り、phase-2 の leaf p2-a は `blocked(decision)`、p2-b は子を
/// 作らずに待つ。h2 に答えると p2-a が進み（前置きの「人の決定」節に h2 の答え）、p2-b は h1 を待ち続ける。
/// h1 に note 付きで答えると子ができ、その objective の末尾に固定の書式で h1 と h2 の答えが入る。root は done。
#[tokio::test]
async fn unanswered_decision_blocks_only_dependents_and_answers_flow_into_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = root_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = with_decisions(
        v3_plan(
            vec![stage("phase-1", false), stage("phase-2", false)],
            vec![
                leaf("p1", "phase-1", &[]),
                leaf("other", "phase-1", &[]),
                leaf("p2-a", "phase-2", &[]),
                task_unit("p2-b", "phase-2", &[], "true"),
            ],
        ),
        vec![
            decision_spec("h1", &["p2-b"]),
            decision_spec("h2", &["stage:phase-2"]),
        ],
    );
    let adapter = Arc::new(DecisionAdapter::new(vec![plan]));
    let mut d = tree_dispatcher(&store, adapter.clone());
    // Phase R3b: 決定を含む root の計画は承認を挟む（決定への回答とは別。承認の後も決定に依存しない unit は進む）。
    assert_eq!(
        quiet_then_approve(&mut d, &store, root_id, 1500).await,
        vec!["decisions:h1,h2".to_string()]
    );

    // (a) 決定に依存しない unit は走り、依存する unit だけが待つ。
    let units = store.work_units_for(root_id).unwrap();
    for key in ["p1", "other", "integrate-phase-1"] {
        assert_eq!(
            unit(&units, key).status,
            task_core::WorkUnitStatus::Done,
            "{key}"
        );
    }
    assert!(blocked_by_decision(&units, "p2-a"), "{units:?}");
    assert_eq!(unit(&units, "p2-a").runs, 0);
    let p2b = unit(&units, "p2-b");
    assert_eq!(p2b.needs_decisions, vec!["h1", "h2"]);
    assert!(p2b.child_task_id.is_none());
    assert!(store.children(root_id).unwrap().is_empty());
    let plan_row = store.execution_plan_active(root_id).unwrap().unwrap();
    for key in ["h1", "h2"] {
        let row = decision(&store, root_id, key);
        assert_eq!(row.status, task_core::DecisionStatus::Open, "{key}");
        assert_eq!(row.kind, task_core::DecisionKind::Choice);
        assert_eq!(row.task_id, root_id);
        assert_eq!(
            row.request.raised_by.origin,
            task_core::DecisionOrigin::Planner
        );
        assert_eq!(
            row.request.raised_by.run_id, plan_row.planner_run_id,
            "raised by the planner run that produced the plan"
        );
        assert_eq!(row.request.path.first().map(|p| p.task_id), Some(root_id));
    }
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Ready);
    no_questions(&store, root_id);
    // 受信箱と件数（task_ops の受信箱の節）。
    let items = task_ops::decision::inbox_items(store.as_ref(), OffsetDateTime::now_utc()).unwrap();
    assert_eq!(
        items.iter().map(|i| i.key.as_str()).collect::<Vec<_>>(),
        vec!["h1", "h2"]
    );
    assert_eq!(task_ops::decision::open_count(store.as_ref()).unwrap(), 2);

    // h2 に答える: p2-a が進む（p2-b はまだ h1 を待つ）。
    let h2 = decision(&store, root_id, "h2");
    let outcome = answer(&store, &h2.id, Some("org-vault"), None);
    assert_eq!(outcome.effect, task_core::DecisionEffect::Resume);
    assert_eq!(outcome.resumed, vec!["p2-a".to_string()]);
    run_until_quiet(&mut d, &store, 1500).await;
    let units = store.work_units_for(root_id).unwrap();
    assert_eq!(unit(&units, "p2-a").status, task_core::WorkUnitStatus::Done);
    assert!(store.children(root_id).unwrap().is_empty(), "h1 is open");
    // leaf の前置きの「人の決定」節（固定の書式）。
    let wu = adapter.work_unit_contexts("p2-a");
    assert_eq!(wu.len(), 1);
    assert_eq!(
        wu[0].human_decisions,
        vec!["- h2 question h2: org vault（推奨どおり）".to_string()]
    );
    // 木の節点の worker の run には決定の要求の出し方が渡る。
    assert!(adapter.contexts().iter().any(|c| c.decision_requests));

    // h1 に note 付きで答える: 子ができ、objective の末尾に固定の書式で入る。
    let h1 = decision(&store, root_id, "h1");
    answer(&store, &h1.id, Some("manual"), Some("trial first"));
    let report = run_until_idle(&mut d, 1500).await;
    assert!(report.idle, "{report:?}");
    let child = child_named(&store, root_id, "p2-b").expect("the child is created");
    let objective = &child.objective;
    let tail = format!(
        "{}\n- h1 question h1: manual entry（推奨と異なる） — trial first\n- h2 question h2: org vault（推奨どおり）",
        task_core::decision::DECISIONS_HEADING
    );
    assert!(objective.ends_with(&tail), "{objective}");
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Done);
    assert_eq!(task_ops::decision::open_count(store.as_ref()).unwrap(), 0);
    let events = store.events_for(root_id).unwrap();
    assert!(events.iter().any(|(_, e)| matches!(
        e,
        Event::DecisionAnswered { id, option, by, .. } if *id == h1.id && option == "manual" && by == "human"
    )));
    assert_eq!(
        unit_reasons(&events, "p2-a"),
        vec!["decision", "decision_answered", "dispatch", "completed"]
    );
    no_questions(&store, root_id);
    assert_replay_is_clean(&store);
}

/// ADR-0079 §7 R2a (e) `limit_raise_answer_resumes_subtree`: 計画の採用時の上限（木の leaf）で止めた unit は
/// `raise-once` の回答で続きから走り、run 時の上限（木の run）で止めた子は `raise-once` の回答で上限に余裕が
/// 足されて走る（新しい決定は出ない）。どちらも root は done まで進む。
#[tokio::test]
async fn limit_raise_answer_resumes_subtree() {
    // 計画の採用時の上限（limit:max_tree_leaves）。
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = root_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = v3_plan(
        vec![stage("s1", false)],
        vec![
            leaf("a", "s1", &[]),
            leaf("b", "s1", &[]),
            leaf("x", "s1", &[]),
        ],
    );
    let adapter = Arc::new(DecisionAdapter::new(vec![plan]));
    let mut d = tree_dispatcher(&store, adapter);
    d.config.execution.limits.tree.max_tree_leaves = 2;
    quiet_then_approve(&mut d, &store, root_id, 1500).await;
    let units = store.work_units_for(root_id).unwrap();
    assert!(blocked_by_decision(&units, "x"));
    let limit = decision(&store, root_id, "limit:max_tree_leaves");
    assert_eq!(limit.needed_before, vec!["x".to_string()]);
    let outcome = answer(&store, &limit.id, Some("raise-once"), None);
    assert_eq!(outcome.effect, task_core::DecisionEffect::RaiseOnce);
    assert_eq!(outcome.resumed, vec!["x".to_string()]);
    let report = run_until_idle(&mut d, 1500).await;
    assert!(report.idle, "{report:?}");
    let units = store.work_units_for(root_id).unwrap();
    assert_eq!(unit(&units, "x").status, task_core::WorkUnitStatus::Done);
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Done);
    assert_eq!(
        store.decisions_list(Some(root_id)).unwrap().len(),
        1,
        "no new decision"
    );
    assert_replay_is_clean(&store);

    // run 時の上限（limit:max_tree_runs、needed_before: self）。
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = root_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = v3_plan(
        vec![stage("s1", false)],
        vec![
            task_unit("c1", "s1", &[], "true"),
            task_unit("c2", "s1", &[], "true"),
        ],
    );
    let adapter = Arc::new(DecisionAdapter::new(vec![plan]));
    let mut d = tree_dispatcher(&store, adapter);
    // root の planner の 1 本 + 子の 1 本で 2 本。もう一方の子の run は 3 本目になる。
    d.config.execution.limits.tree.max_tree_runs = 2;
    quiet_then_approve(&mut d, &store, root_id, 1500).await;
    let limit = decision(&store, root_id, "limit:max_tree_runs");
    assert_eq!(limit.status, task_core::DecisionStatus::Open);
    assert_eq!(limit.needed_before, vec!["self".to_string()]);
    let stopped = store.get(limit.task_id).unwrap().unwrap();
    assert_eq!(stopped.status, Status::Ready);
    assert!(store.runs_for_task(stopped.id).unwrap().is_empty());
    answer(&store, &limit.id, Some("raise-once"), None);
    let report = run_until_idle(&mut d, 1500).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(stopped.id).unwrap().unwrap().status, Status::Done);
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Done);
    assert_eq!(
        store.decisions_list(Some(root_id)).unwrap().len(),
        1,
        "the raised limit (2 + 1) is not breached again"
    );
    let allowances = task_ops::decision::limit_allowances(store.as_ref(), root_id, None).unwrap();
    assert_eq!(
        allowances.get(&task_core::TreeLimitKind::TreeRuns),
        Some(&1)
    );
    no_questions(&store, root_id);
    assert_replay_is_clean(&store);
}

/// ADR-0079 R3a: 計画を持つ節点の replan の planner が 2 回不正（`plan_invalid`、選択肢は replan / cancel）→ 人が note
/// 付きで replan と答えると、planner がもう一度走り（試行の窓を開け直す）、その入力（前の試行の理由の末尾と
/// replan の理由）に人の note があり、計画の版 2 が採用されて root は done まで進む。replan の起点は v1 の leaf a の
/// 失敗（検査が落ち続ける）。
#[tokio::test]
async fn plan_invalid_replan_feeds_the_note_and_adopts_plan_v2() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = root_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let mut failing = leaf("a", "s1", &[]);
    failing["checks"] = serde_json::json!([{"cmd": "false", "expect_exit": 0}]);
    let v1 = v3_plan(vec![stage("s1", false)], vec![failing]);
    let mut bad = leaf("d", "s1", &[]);
    bad.as_object_mut().unwrap().remove("checks");
    let bad_plan = v3_plan(vec![stage("s1", false)], vec![bad]);
    let v2 = v3_plan(vec![stage("s1", false)], vec![leaf("d", "s1", &[])]);
    let adapter = Arc::new(DecisionAdapter::new(vec![
        v1,
        bad_plan.clone(),
        bad_plan,
        v2,
    ]));
    let mut d = tree_dispatcher(&store, adapter.clone());
    run_until_quiet(&mut d, &store, 1500).await;
    assert!(
        store
            .decisions_list(Some(root_id))
            .unwrap()
            .iter()
            .any(|r| r.kind == task_core::DecisionKind::PlanInvalid),
        "{:#?}",
        store.events_for(root_id).unwrap()
    );
    let invalid = decision(&store, root_id, task_core::tree::PLAN_INVALID_DECISION_KEY);
    assert_eq!(invalid.status, task_core::DecisionStatus::Open);
    let keys: Vec<&str> = invalid
        .request
        .options
        .iter()
        .map(|o| o.key.as_str())
        .collect();
    assert_eq!(
        keys,
        vec!["replan", "cancel"],
        "no atomic once a plan exists"
    );
    assert_eq!(adapter.planner_contexts().len(), 3);
    // 答えるまでは planner は走らない。
    for _ in 0..10 {
        d.tick().unwrap();
    }
    assert_eq!(adapter.planner_contexts().len(), 3);
    let outcome = answer(
        &store,
        &invalid.id,
        Some("replan"),
        Some("write the docs leaf with a check"),
    );
    assert_eq!(outcome.effect, task_core::DecisionEffect::Replan);
    assert!(outcome.replan_requested);
    let report = run_until_idle(&mut d, 1500).await;
    assert!(report.idle, "{report:?}");
    let plans = store.execution_plan_list(root_id).unwrap();
    assert_eq!(plans.len(), 2, "plan v2 adopted");
    let active = store.execution_plan_active(root_id).unwrap().unwrap();
    assert_eq!(active.version, 2);
    let contexts = adapter.planner_contexts();
    assert_eq!(contexts.len(), 4);
    let last = &contexts[3];
    assert!(last.replan);
    assert!(
        last.previous_attempt_errors
            .iter()
            .any(|e| e.contains("write the docs leaf with a check")),
        "{:?}",
        last.previous_attempt_errors
    );
    assert!(
        last.replan_reason
            .contains("write the docs leaf with a check"),
        "{}",
        last.replan_reason
    );
    let units = store.work_units_for(root_id).unwrap();
    assert_eq!(unit(&units, "d").status, task_core::WorkUnitStatus::Done);
    assert_eq!(
        unit(&units, "a").status,
        task_core::WorkUnitStatus::Superseded
    );
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Done);
    no_questions(&store, root_id);
    // replay（ADR-0079 R4a）: replan で計画から消えた unit（superseded の a）も含めて `work_units` を events から
    // 同じに作り直せる（R3a では presence の差を許していた）。
    assert_replay_is_clean(&store);
}

/// ADR-0079 R3a: 初回の計画の `plan_invalid` に atomic と答えると、節点は計画を作らずに 1 run で走る（gate の判定は
/// `atomic/decision`）。cancel と答えると節点は中止になる。
#[tokio::test]
async fn plan_invalid_atomic_runs_the_node_once_and_cancel_cancels_it() {
    for option in ["atomic", "cancel"] {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
        let root = root_task(dir.path());
        let root_id = root.id;
        store.create_task(&root, vec![]).unwrap();
        let mut bad = leaf("a", "s1", &[]);
        bad.as_object_mut().unwrap().remove("checks");
        let bad_plan = v3_plan(vec![stage("s1", false)], vec![bad]);
        let adapter = Arc::new(DecisionAdapter::new(vec![bad_plan.clone(), bad_plan]));
        let mut d = tree_dispatcher(&store, adapter.clone());
        run_until_quiet(&mut d, &store, 800).await;
        let invalid = decision(&store, root_id, task_core::tree::PLAN_INVALID_DECISION_KEY);
        answer(&store, &invalid.id, Some(option), None);
        let report = run_until_idle(&mut d, 800).await;
        assert!(report.idle, "{option}: {report:?}");
        let stored = store.get(root_id).unwrap().unwrap();
        let workers = store
            .runs_for_task(root_id)
            .unwrap()
            .iter()
            .filter(|r| r.role == task_core::RunIndexRole::Worker)
            .count();
        assert_eq!(adapter.planner_contexts().len(), 2, "{option}");
        if option == "atomic" {
            assert_eq!(stored.status, Status::Done, "{option}");
            assert_eq!(workers, 1, "one atomic run");
            let gate = stored.routing.as_ref().unwrap().execution.clone().unwrap();
            assert_eq!(gate.mode, task_core::ExecutionMode::Atomic);
            assert_eq!(gate.rule_id, "atomic/decision");
            assert!(store.execution_plan_active(root_id).unwrap().is_none());
        } else {
            assert_eq!(stored.status, Status::Cancelled, "{option}");
            assert_eq!(workers, 0);
        }
        no_questions(&store, root_id);
        assert_replay_is_clean(&store);
    }
}

/// ADR-0079 R3a: 取り下げ。`leaf_too_large` の決定に withdraw と答えると止めた unit が取り下げられ（`cancelled`）、
/// 段階はその unit 抜きで完了して root は done。人の取り下げ（`POST /decisions/{id}/withdraw`）は、計画の決定を
/// 待っていた leaf と、それに依存する leaf を取り下げる。
#[tokio::test]
async fn withdraw_cancels_the_held_units() {
    // withdraw の回答（leaf_too_large）。
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = root_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let mut big = leaf("big", "s1", &[]);
    big["features"] = serde_json::json!({"expected_length": "high", "cross_cutting": "high"});
    let plan = v3_plan(vec![stage("s1", false)], vec![leaf("a", "s1", &[]), big]);
    let adapter = Arc::new(DecisionAdapter::new(vec![plan]));
    let mut d = tree_dispatcher(&store, adapter);
    d.config.execution.limits.tree.max_depth = 1;
    d.config.execution.limits.tree.auto_leaf = false;
    quiet_then_approve(&mut d, &store, root_id, 800).await;
    let dec = decision(&store, root_id, "leaf_too_large:big");
    let outcome = answer(&store, &dec.id, Some("withdraw"), None);
    assert_eq!(outcome.effect, task_core::DecisionEffect::Withdraw);
    assert_eq!(outcome.cancelled, vec!["big".to_string()]);
    let report = run_until_idle(&mut d, 800).await;
    assert!(report.idle, "{report:?}");
    let units = store.work_units_for(root_id).unwrap();
    assert_eq!(
        unit(&units, "big").status,
        task_core::WorkUnitStatus::Cancelled
    );
    assert_eq!(unit(&units, "big").runs, 0);
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Done);
    assert_replay_is_clean(&store);

    // 人の取り下げ（計画の決定を待つ leaf とその依存）。
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = root_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = with_decisions(
        v3_plan(
            vec![stage("s1", false)],
            vec![
                leaf("a", "s1", &[]),
                leaf("w", "s1", &[]),
                leaf("after-w", "s1", &["w"]),
            ],
        ),
        vec![decision_spec("h1", &["w"])],
    );
    let adapter = Arc::new(DecisionAdapter::new(vec![plan]));
    let mut d = tree_dispatcher(&store, adapter);
    quiet_then_approve(&mut d, &store, root_id, 800).await;
    let units = store.work_units_for(root_id).unwrap();
    assert!(blocked_by_decision(&units, "w"));
    assert_eq!(
        unit(&units, "after-w").status,
        task_core::WorkUnitStatus::Pending
    );
    let h1 = decision(&store, root_id, "h1");
    let outcome = task_ops::decision::withdraw(
        store.as_ref(),
        &h1.id,
        Some("not needed any more"),
        "human",
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    assert_eq!(
        outcome.cancelled,
        vec!["w".to_string(), "after-w".to_string()]
    );
    assert_eq!(
        decision(&store, root_id, "h1").status,
        task_core::DecisionStatus::Withdrawn
    );
    let report = run_until_idle(&mut d, 800).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Done);
    assert_replay_is_clean(&store);
}

/// ADR-0079 §7 R3a (c) `worker_declared_decisions`: leaf a の run が `result.json` に決定 2 件（w1 は `self`、w2 は
/// 次の段階の leaf c）を書く。2 件とも path（root › 段階 › leaf a）付きで記録され（origin worker、raised_by = その
/// run）、a は done にならず `blocked(decision)`、c は `blocked(decision)`、同じ段階の b は止まらない。w1 に答えると
/// a がもう一度走り（前置きに w1 の答え）、w2 に答えると c が走って root は done。atomic の子の run の `self` は
/// 子を止め、答えは次の run の `answers` に入る。
#[tokio::test]
async fn worker_declared_decisions() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = root_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = v3_plan(
        vec![stage("s1", false), stage("s2", false)],
        vec![
            leaf("a", "s1", &[]),
            leaf("b", "s1", &[]),
            leaf("c", "s2", &[]),
            task_unit("k", "s2", &[], "true"),
        ],
    );
    let adapter = Arc::new(
        DecisionAdapter::new(vec![plan])
            .declaring(
                "a",
                serde_json::json!([
                    decision_spec("w1", &["self"]),
                    decision_spec("w2", &["c"]),
                    {"key": "broken"},
                ]),
            )
            .declaring("k", serde_json::json!([decision_spec("wk", &["self"])])),
    );
    let mut d = tree_dispatcher(&store, adapter.clone());
    run_until_quiet(&mut d, &store, 1500).await;

    let units = store.work_units_for(root_id).unwrap();
    assert!(blocked_by_decision(&units, "a"), "{units:?}");
    assert_eq!(unit(&units, "b").status, task_core::WorkUnitStatus::Done);
    assert!(blocked_by_decision(&units, "c"), "{units:?}");
    let a_run = unit(&units, "a").last_run_id.clone().unwrap();
    for (key, needed) in [("w1", "a"), ("w2", "c")] {
        let row = decision(&store, root_id, key);
        assert_eq!(row.status, task_core::DecisionStatus::Open);
        assert_eq!(row.needed_before, vec![needed.to_string()], "{key}");
        assert_eq!(
            row.request.raised_by.origin,
            task_core::DecisionOrigin::Worker
        );
        assert_eq!(
            row.request.raised_by.run_id.as_deref(),
            Some(a_run.as_str())
        );
        let path: Vec<(TaskId, Option<String>, Option<String>)> = row
            .request
            .path
            .iter()
            .map(|p| (p.task_id, p.stage.clone(), p.unit.clone()))
            .collect();
        assert_eq!(
            path,
            vec![
                (root_id, Some("s1".to_string()), None),
                (root_id, None, Some("a".to_string())),
            ]
        );
    }
    assert!(
        store.decisions_list(Some(root_id)).unwrap().len() == 2,
        "the malformed one is dropped"
    );
    let events = store.events_for(root_id).unwrap();
    assert!(events.iter().any(|(_, e)| matches!(
        e,
        Event::WorkerProgress { msg, .. } if msg.contains("worker の決定の要求を記録できませんでした")
    )));
    assert_eq!(unit_reasons(&events, "a"), vec!["dispatch", "decision"]);
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Ready);
    no_questions(&store, root_id);

    // w1 に答える → a がもう一度走り、前置きに答えが入る。段階 s1 が揃う。
    let w1 = decision(&store, root_id, "w1");
    let outcome = answer(&store, &w1.id, None, Some("keep the old API"));
    assert_eq!(outcome.resumed, vec!["a".to_string()]);
    run_until_quiet(&mut d, &store, 1500).await;
    let units = store.work_units_for(root_id).unwrap();
    assert_eq!(unit(&units, "a").status, task_core::WorkUnitStatus::Done);
    assert_eq!(unit(&units, "a").runs, 2);
    let a_contexts = adapter.work_unit_contexts("a");
    assert_eq!(a_contexts.len(), 2);
    assert!(a_contexts[0].human_decisions.is_empty());
    assert_eq!(
        a_contexts[1].human_decisions,
        vec!["- w1 question w1: 自由記述（推奨と異なる） — keep the old API".to_string()]
    );
    assert_eq!(
        unit(&units, "integrate-s1").status,
        task_core::WorkUnitStatus::Done
    );
    assert!(blocked_by_decision(&units, "c"), "w2 is still open");

    // 子 k（atomic の run）の `self`: 子は止まり（ready、run 1 本）、答えると次の run で `answers` に入る。
    let child = child_named(&store, root_id, "k").expect("child k");
    let wk = decision(&store, root_id, "wk");
    assert_eq!(wk.task_id, child.id);
    assert_eq!(wk.needed_before, vec!["self".to_string()]);
    let stored_child = store.get(child.id).unwrap().unwrap();
    assert_eq!(stored_child.status, Status::Ready);
    assert_eq!(store.runs_for_task(child.id).unwrap().len(), 1);
    answer(&store, &wk.id, Some("manual"), None);
    let w2 = decision(&store, root_id, "w2");
    answer(&store, &w2.id, Some("org-vault"), None);
    let report = run_until_idle(&mut d, 1500).await;
    assert!(report.idle, "{report:?}");
    let child_events = store.events_for(child.id).unwrap();
    assert!(child_events.iter().any(|(_, e)| matches!(
        e,
        Event::Answered { question, answer }
            if question.starts_with(task_ops::decision::ANSWERED_QUESTION_PREFIX)
                && answer == "- wk question wk: manual entry（推奨と異なる）"
    )));
    assert_eq!(
        store.runs_for_task(child.id).unwrap().len(),
        2,
        "the first run and the rerun after the answer"
    );
    assert_eq!(store.get(child.id).unwrap().unwrap().status, Status::Done);
    let units = store.work_units_for(root_id).unwrap();
    assert_eq!(unit(&units, "c").status, task_core::WorkUnitStatus::Done);
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Done);
    no_questions(&store, root_id);
    assert_replay_is_clean(&store);
}

/// ADR-0079 D15: `[execution.tree] enabled = false`（既定）では何も変わらない: 木でない task の worker が
/// `result.json` に `decisions` を書いても記録されず止まらず、前置きに決定の要求の節も出ない。
#[tokio::test]
async fn tree_disabled_ignores_worker_decisions() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        1,
    );
    let id = task.id;
    store.create_task(&task, vec![]).unwrap();
    let adapter = Arc::new(DecisionAdapter::new(Vec::new()).declaring(
        "atomic",
        serde_json::json!([decision_spec("w1", &["self"])]),
    ));
    let mut d = dispatcher(store.clone(), adapter.clone(), 3);
    d.config.execution.limits = task_core::ExecutionLimits::default();
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(id).unwrap().unwrap().status, Status::Done);
    assert!(store.decisions_list(None).unwrap().is_empty());
    assert!(adapter.contexts().iter().all(|c| !c.decision_requests));
    assert_eq!(task_ops::decision::open_count(store.as_ref()).unwrap(), 0);
}

/// ADR-0074 付記（2026-10-02）: 段階 s1 の途中確認を `continue` で返し、メモを付けたときだけ、次の段階 s2 の
/// leaf の前置き（`WorkUnitPromptContext.human_decisions`）と s2 の kind task の unit から作る子の objective の
/// 「人の決定」節（D7 の回答と同じ節・同じ形の行）にメモが入る。前の段階 s1 の leaf には入らない。メモが無ければ
/// 何も足さない（節も出ない）。
async fn continue_note_scenario(
    note: Option<&str>,
) -> (Arc<DecisionAdapter>, Arc<dyn TaskStore>, TaskId) {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = root_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = v3_plan(
        vec![stage("s1", true), stage("s2", false)],
        vec![
            leaf("a", "s1", &[]),
            leaf("b", "s2", &[]),
            task_unit("c", "s2", &[], "true"),
        ],
    );
    let adapter = Arc::new(DecisionAdapter::new(vec![plan]));
    let mut d = tree_dispatcher(&store, adapter.clone());
    assert_eq!(
        quiet_then_approve(&mut d, &store, root_id, 1500).await,
        vec!["review_human:s1".to_string()]
    );
    let events = store.events_for(root_id).unwrap();
    let stored = store.get(root_id).unwrap().unwrap();
    assert!(
        task_ops::phase_gate::is_awaiting_human(&stored, &events),
        "{events:?}"
    );
    task_ops::phase_gate::phase_gate(
        store.as_ref(),
        root_id,
        task_ops::phase_gate::PhaseGateAction::Continue,
        note.map(str::to_string),
    )
    .unwrap();
    let report = run_until_idle(&mut d, 1500).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Done);
    assert_replay_is_clean(&store);
    (adapter, store, root_id)
}

#[tokio::test]
async fn phase_gate_continue_note_reaches_next_stage_leaf_and_child() {
    let (adapter, store, root_id) = continue_note_scenario(Some("  14 node-h までに留める ")).await;
    let line = "- 途中確認: 工程『s1』の後: 続ける — 14 node-h までに留める".to_string();
    let a = adapter.work_unit_contexts("a");
    assert_eq!(a.len(), 1);
    assert!(a[0].human_decisions.is_empty(), "s1 は前の段階: {a:?}");
    let b = adapter.work_unit_contexts("b");
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].human_decisions, vec![line.clone()]);
    let child = child_named(&store, root_id, "c").expect("the child is created");
    let tail = format!("{}\n{line}", task_core::decision::DECISIONS_HEADING);
    assert!(child.objective.ends_with(&tail), "{}", child.objective);
}

#[tokio::test]
async fn phase_gate_continue_without_note_adds_nothing() {
    let (adapter, store, root_id) = continue_note_scenario(None).await;
    let b = adapter.work_unit_contexts("b");
    assert_eq!(b.len(), 1);
    assert!(b[0].human_decisions.is_empty(), "{b:?}");
    let child = child_named(&store, root_id, "c").expect("the child is created");
    assert!(
        !child
            .objective
            .contains(task_core::decision::DECISIONS_HEADING),
        "{}",
        child.objective
    );
}
