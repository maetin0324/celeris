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

/// planner run には計画の列を順に書き、run の文脈を記録する。worker の run は、`script` に key（WU の key、
/// 子 task なら題名の `Child ` の後）の台本が残っていれば先頭の 1 run を使い、無ければ `declare` に key があれば
/// 最初の run の `result.json` にその `decisions` を書き、それ以外は `decisions` の無い `result.json` を書く。`ask` に
/// key があれば、その key の最初の run は `result.json` に `question` を書いて質問で終わる（`decisions` は上と同じ）。
/// `fail` に key があれば、その key の最初の run は `result.json` を書いた後に失敗（`Terminal::Error{retryable: true}`）で
/// 終わる。
struct DecisionAdapter {
    plans: StdMutex<VecDeque<String>>,
    seen: StdMutex<Vec<task_worker::RunContext>>,
    declare: StdMutex<BTreeMap<String, serde_json::Value>>,
    script: StdMutex<BTreeMap<String, VecDeque<ScriptedRun>>>,
    ask: StdMutex<BTreeMap<String, String>>,
    fail: StdMutex<BTreeMap<String, String>>,
}

/// `DecisionAdapter` の台本の 1 run。`yielded` があればその checkpoint で yield し（`result.json` の `yield`）、
/// 無ければ `summary` 付きの完了で終わる。`decisions` があれば `result.json` に書く。
struct ScriptedRun {
    yielded: Option<serde_json::Value>,
    decisions: Option<serde_json::Value>,
}

impl DecisionAdapter {
    fn new(plans: Vec<String>) -> Self {
        DecisionAdapter {
            plans: StdMutex::new(plans.into_iter().collect()),
            seen: StdMutex::new(Vec::new()),
            declare: StdMutex::new(BTreeMap::new()),
            script: StdMutex::new(BTreeMap::new()),
            ask: StdMutex::new(BTreeMap::new()),
            fail: StdMutex::new(BTreeMap::new()),
        }
    }

    fn asking(self, key: &str, question: &str) -> Self {
        self.ask
            .lock()
            .unwrap()
            .insert(key.to_string(), question.to_string());
        self
    }

    fn failing(self, key: &str, message: &str) -> Self {
        self.fail
            .lock()
            .unwrap()
            .insert(key.to_string(), message.to_string());
        self
    }

    fn scripting(self, key: &str, runs: Vec<ScriptedRun>) -> Self {
        self.script
            .lock()
            .unwrap()
            .insert(key.to_string(), runs.into_iter().collect());
        self
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
            let scripted = self
                .script
                .lock()
                .unwrap()
                .get_mut(&key)
                .and_then(VecDeque::pop_front);
            let run = scripted.unwrap_or_else(|| ScriptedRun {
                yielded: None,
                decisions: self.declare.lock().unwrap().remove(&key),
            });
            let asked = self.ask.lock().unwrap().remove(&key);
            let mut result = match (&asked, &run.yielded) {
                (Some(text), _) => serde_json::json!({"question": text}),
                (None, Some(checkpoint)) => serde_json::json!({"yield": checkpoint}),
                (None, None) => serde_json::json!({"summary": "ok", "evidence": []}),
            };
            if let Some(decisions) = run.decisions {
                result["decisions"] = decisions;
            }
            std::fs::write(req.artifacts_dir.join("result.json"), result.to_string())
                .expect("write result.json");
            if let Some(text) = asked {
                return Ok(RunOutcome {
                    terminal: Terminal::Question { text, usage: None },
                    exit_code: Some(0),
                });
            }
            if let Some(message) = self.fail.lock().unwrap().remove(&key) {
                return Ok(RunOutcome {
                    terminal: Terminal::Error {
                        message,
                        retryable: true,
                        usage: None,
                    },
                    exit_code: Some(1),
                });
            }
            if let Some(checkpoint) = run.yielded {
                return Ok(RunOutcome {
                    terminal: Terminal::Yielded {
                        checkpoint,
                        usage: None,
                    },
                    exit_code: Some(0),
                });
            }
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
async fn run_until_quiet(d: &mut Dispatcher, store: &Arc<SqliteStore>, max_ticks: usize) {
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
    store: &Arc<SqliteStore>,
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

fn decision(store: &Arc<SqliteStore>, root: TaskId, key: &str) -> task_core::DecisionRow {
    store
        .decisions_list(Some(root))
        .unwrap()
        .into_iter()
        .rfind(|r| r.key == key)
        .unwrap_or_else(|| panic!("no decision {key}"))
}

fn answer(
    store: &Arc<SqliteStore>,
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

fn child_named(store: &Arc<SqliteStore>, root: TaskId, key: &str) -> Option<Task> {
    store
        .children(root)
        .unwrap()
        .into_iter()
        .find(|c| c.title == format!("Child {key}"))
}

fn no_questions(store: &Arc<SqliteStore>, task: TaskId) {
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
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
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
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
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
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
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
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
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
        let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
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
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
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
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
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
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
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
                serde_json::json!([decision_spec("w1", &["self"]), decision_spec("w2", &["c"]),]),
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
        "w1 and w2 only"
    );
    let events = store.events_for(root_id).unwrap();
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
        Event::Answered { question, answer, .. }
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

/// ADR 2026-11-18-rejected-worker-decisions-reach-human: 本番の事象（01M7Q5ZABN9NFQCH6QBTTP3EZ8 の target-leaf）と
/// 同じく、leaf が `needed_before` に integrate の unit 名（`integrate-s1`）を書いた決定の要求を出す。要求は受け付け
/// られないが、黙って進まない: decisions に `rejected` の行（open・origin worker・`needed_before = [leaf]`、問いに元の
/// 問いと拒否の理由）ができて受信箱の未回答の決定に出て、leaf は done にならず `blocked(decision)`、integrate も進まない。
/// 答えると leaf が走り直し、前置きの「人の決定」節に答えが入り、木が終わる。修正の前は decisions が空で leaf は done、
/// 木は done まで進んだ（`worker_declared_decisions` は受け付けた要求しか見ないので、これを検出できない）。
#[tokio::test]
async fn rejected_worker_decision_reaches_the_inbox_and_holds_the_leaf() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = root_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = v3_plan(
        vec![stage("s1", false)],
        vec![leaf("a", "s1", &[]), leaf("b", "s1", &[])],
    );
    let adapter = Arc::new(DecisionAdapter::new(vec![plan]).declaring(
        "a",
        serde_json::json!([decision_spec("trim", &["integrate-s1"])]),
    ));
    let mut d = tree_dispatcher(&store, adapter.clone());
    run_until_quiet(&mut d, &store, 1500).await;

    // run の結果: leaf は done にならず止まり、integrate も root も進まない。
    let units = store.work_units_for(root_id).unwrap();
    assert!(blocked_by_decision(&units, "a"), "{units:?}");
    assert_eq!(unit(&units, "b").status, task_core::WorkUnitStatus::Done);
    assert_ne!(
        unit(&units, "integrate-s1").status,
        task_core::WorkUnitStatus::Done
    );
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Ready);
    // decisions の行: 拒否した要求をまとめた 1 件だけ。
    let rows = store.decisions_list(Some(root_id)).unwrap();
    assert_eq!(
        rows.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(),
        vec![task_core::decision::REJECTED_KEY]
    );
    let row = &rows[0];
    assert_eq!(row.status, task_core::DecisionStatus::Open);
    assert_eq!(row.needed_before, vec!["a".to_string()]);
    assert_eq!(
        row.request.raised_by.origin,
        task_core::DecisionOrigin::Worker
    );
    assert_eq!(
        row.request.raised_by.run_id.as_deref(),
        unit(&units, "a").last_run_id.as_deref()
    );
    for needle in [
        "question trim",
        "org-vault=org vault / manual=manual entry",
        r#"needed_before names an unknown unit "integrate-s1""#,
    ] {
        assert!(
            row.request.question.contains(needle),
            "{needle:?} in {}",
            row.request.question
        );
    }
    // 受信箱: 未回答の決定（受信箱・通知の対象）に出る。
    let inbox = task_ops::decision::open_rows(store.as_ref()).unwrap();
    assert_eq!(
        inbox.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        vec![row.id.as_str()]
    );
    // 進行の 1 行（理由）は今までどおり残る。
    let events = store.events_for(root_id).unwrap();
    assert!(events.iter().any(|(_, e)| matches!(
        e,
        Event::WorkerProgress { msg, .. } if msg.contains("worker の決定の要求を記録できませんでした")
            && msg.contains("integrate-s1")
    )));
    assert_eq!(unit_reasons(&events, "a"), vec!["dispatch", "decision"]);
    no_questions(&store, root_id);

    // 答える → a が走り直し、前置きに答え（元の問いと理由を含む）が入り、木が終わる。
    let outcome = answer(
        &store,
        &row.id,
        Some("see-note"),
        Some("trim は別 task にする"),
    );
    assert_eq!(outcome.resumed, vec!["a".to_string()]);
    let report = run_until_idle(&mut d, 1500).await;
    assert!(report.idle, "{report:?}");
    let units = store.work_units_for(root_id).unwrap();
    assert_eq!(unit(&units, "a").status, task_core::WorkUnitStatus::Done);
    assert_eq!(unit(&units, "a").runs, 2);
    let a_contexts = adapter.work_unit_contexts("a");
    assert_eq!(a_contexts.len(), 2);
    assert_eq!(a_contexts[1].human_decisions.len(), 1);
    let line = &a_contexts[1].human_decisions[0];
    assert!(
        line.starts_with("- rejected worker が出した決定の要求 1 件")
            && line.contains("question trim")
            && line.ends_with("note に書いた答えで進める（推奨と異なる） — trim は別 task にする"),
        "{line}"
    );
    assert_eq!(
        unit(&units, "integrate-s1").status,
        task_core::WorkUnitStatus::Done
    );
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Done);
    assert_replay_is_clean(&store);
}

/// ADR 2026-11-21-worker-decisions-fail-closed D2（不変条件 I1・I2・I4）: leaf が `result.json` の `decisions` に
/// 配列でなく決定 1 件の形の object を書く。要求は受け付けられない（1 件の配列に読み替えない）が、黙って進まない:
/// decisions に `rejected` の行（open・origin worker・`needed_before = [leaf]`、問いに `#field` と型の誤りの理由）が
/// できて受信箱の未回答の決定に出て、leaf は done にならず `blocked(decision)`、integrate も root も進まない。答えると
/// leaf が走り直し、木が終わる。修正の前は `worker_decisions_from_json` が配列でない欄を空にしたので、決定は 0 件で
/// leaf は done になった。既存の `rejected_worker_decision_reaches_the_inbox_and_holds_the_leaf`・
/// `worker_declared_decisions` は `decisions` に配列しか渡さないので、これを検出できない。
#[tokio::test]
async fn decision_fail_closed_non_array_decisions_hold_the_leaf() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = root_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = v3_plan(
        vec![stage("s1", false)],
        vec![leaf("a", "s1", &[]), leaf("b", "s1", &[])],
    );
    // 配列でない: 決定 1 件の形の object そのもの。
    let adapter =
        Arc::new(DecisionAdapter::new(vec![plan]).declaring("a", decision_spec("trim", &["self"])));
    let mut d = tree_dispatcher(&store, adapter.clone());
    run_until_quiet(&mut d, &store, 1500).await;

    // run の結果: leaf は done にならず止まり、integrate も root も進まない。
    let units = store.work_units_for(root_id).unwrap();
    assert!(blocked_by_decision(&units, "a"), "{units:?}");
    assert_eq!(unit(&units, "b").status, task_core::WorkUnitStatus::Done);
    assert_ne!(
        unit(&units, "integrate-s1").status,
        task_core::WorkUnitStatus::Done
    );
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Ready);
    // decisions の行: 型の誤りをまとめた 1 件だけ（object の中の trim は受け付けない）。
    let rows = store.decisions_list(Some(root_id)).unwrap();
    assert_eq!(
        rows.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(),
        vec![task_core::decision::REJECTED_KEY]
    );
    let row = &rows[0];
    assert_eq!(row.status, task_core::DecisionStatus::Open);
    assert_eq!(row.task_id, root_id);
    assert_eq!(row.needed_before, vec!["a".to_string()]);
    assert_eq!(
        row.request.raised_by.origin,
        task_core::DecisionOrigin::Worker
    );
    assert_eq!(
        row.request.raised_by.run_id.as_deref(),
        unit(&units, "a").last_run_id.as_deref()
    );
    assert_eq!(
        row.request.cost_of_reversal,
        task_core::CostOfReversal::High
    );
    for needle in [
        "決定の要求 1 件",
        "1) #field trim question trim",
        "decisions must be an array of decision objects (got object)",
    ] {
        assert!(
            row.request.question.contains(needle),
            "{needle:?} in {}",
            row.request.question
        );
    }
    // 受信箱: 未回答の決定（受信箱・通知の対象）に出る。
    let inbox = task_ops::decision::open_rows(store.as_ref()).unwrap();
    assert_eq!(
        inbox.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        vec![row.id.as_str()]
    );
    // 進行の 1 行（理由）も残る。
    let events = store.events_for(root_id).unwrap();
    assert!(events.iter().any(|(_, e)| matches!(
        e,
        Event::WorkerProgress { msg, .. } if msg.contains("worker の決定の要求を記録できませんでした")
            && msg.contains("(got object)")
    )));
    assert_eq!(unit_reasons(&events, "a"), vec!["dispatch", "decision"]);
    no_questions(&store, root_id);

    // 答える → a が走り直し（2 回目の run は `decisions` を書かない）、木が終わる。
    let outcome = answer(
        &store,
        &row.id,
        Some(task_core::decision::BUNDLE_OPTION_RECOMMENDED),
        None,
    );
    assert_eq!(outcome.resumed, vec!["a".to_string()]);
    let report = run_until_idle(&mut d, 1500).await;
    assert!(report.idle, "{report:?}");
    let units = store.work_units_for(root_id).unwrap();
    assert_eq!(unit(&units, "a").status, task_core::WorkUnitStatus::Done);
    assert_eq!(unit(&units, "a").runs, 2);
    assert_eq!(
        unit(&units, "integrate-s1").status,
        task_core::WorkUnitStatus::Done
    );
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Done);
    assert_replay_is_clean(&store);
}

/// ADR 2026-11-21-worker-decisions-fail-closed D2（不変条件 I1・I2）: 木の子の atomic の節点（WU の無い run）が
/// `decisions` に文字列を書く。`needed_before = [self]` の `rejected` の行（open・origin worker・節点の task）が
/// でき、受信箱の未回答の決定に出て、節点は答えるまで done にならない（`ready` のまま run 1 本）。答えると節点が
/// 走り直して done になり、root も終わる。修正の前は配列でない欄が空になり、決定は 0 件で節点は 1 run で done に
/// なった。既存の試験の atomic の場面（`worker_declared_decisions` の子 k）は配列しか渡さないので検出できない。
#[tokio::test]
async fn decision_fail_closed_non_array_decisions_hold_the_atomic_node() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = root_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = v3_plan(
        vec![stage("s1", false)],
        vec![leaf("a", "s1", &[]), task_unit("k", "s1", &[], "true")],
    );
    let adapter = Arc::new(
        DecisionAdapter::new(vec![plan])
            .declaring("k", serde_json::json!("which vault should the node use?")),
    );
    let mut d = tree_dispatcher(&store, adapter.clone());
    run_until_quiet(&mut d, &store, 1500).await;

    // 節点は止まる（ready、run 1 本）。root は終わらない。
    let child = child_named(&store, root_id, "k").expect("child k");
    assert_eq!(store.runs_for_task(child.id).unwrap().len(), 1);
    assert_eq!(store.get(child.id).unwrap().unwrap().status, Status::Ready);
    assert_ne!(store.get(root_id).unwrap().unwrap().status, Status::Done);
    let rows = store.decisions_list(Some(root_id)).unwrap();
    assert_eq!(
        rows.iter()
            .map(|r| (r.task_id, r.key.as_str()))
            .collect::<Vec<_>>(),
        vec![(child.id, task_core::decision::REJECTED_KEY)]
    );
    let row = &rows[0];
    assert_eq!(row.status, task_core::DecisionStatus::Open);
    assert_eq!(row.needed_before, vec!["self".to_string()]);
    assert_eq!(
        row.request.raised_by.origin,
        task_core::DecisionOrigin::Worker
    );
    assert!(
        row.request
            .question
            .contains("decisions must be an array of decision objects (got string)"),
        "{}",
        row.request.question
    );
    let inbox = task_ops::decision::open_rows(store.as_ref()).unwrap();
    assert_eq!(
        inbox.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        vec![row.id.as_str()]
    );
    no_questions(&store, child.id);

    // 答える → 節点が走り直して done、root も終わる。
    answer(
        &store,
        &row.id,
        Some(task_core::decision::BUNDLE_OPTION_RECOMMENDED),
        None,
    );
    let report = run_until_idle(&mut d, 1500).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.runs_for_task(child.id).unwrap().len(), 2);
    assert_eq!(store.get(child.id).unwrap().unwrap().status, Status::Done);
    let units = store.work_units_for(root_id).unwrap();
    assert_eq!(unit(&units, "k").status, task_core::WorkUnitStatus::Done);
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Done);
    assert_replay_is_clean(&store);
}

/// ADR-0079 D15: `[execution.tree] enabled = false`（既定）では何も変わらない: 木でない task の worker が
/// `result.json` に `decisions` を書いても記録されず止まらず、前置きに決定の要求の節も出ない。
#[tokio::test]
async fn tree_disabled_ignores_worker_decisions() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
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
) -> (Arc<DecisionAdapter>, Arc<SqliteStore>, TaskId) {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
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
        "human:test",
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

/// yield の checkpoint（`completed` が `n` 件）。同じ `n` の checkpoint を続けると進捗なしになる。
fn yielded_checkpoint(n: usize) -> serde_json::Value {
    serde_json::json!({
        "completed": (0..n).map(|i| format!("step {i}")).collect::<Vec<_>>(),
        "remaining": ["finish"],
        "next_action": "finish",
    })
}

/// 決定を待つ leaf a の行・決定の行・受信箱・events を確かめる（yield・進捗なしの試験の共通の前半）。`runs` は
/// 止まるまでの a の run の数。a は done にも continuation にもならず、integrate と root は進まず、質問も出ない。
fn assert_leaf_held_by_worker_decision(
    store: &Arc<SqliteStore>,
    root_id: TaskId,
    key: &str,
    runs: u32,
) -> task_core::DecisionRow {
    let units = store.work_units_for(root_id).unwrap();
    assert!(blocked_by_decision(&units, "a"), "{units:?}");
    assert_eq!(unit(&units, "a").runs, runs, "no continuation run");
    assert_eq!(unit(&units, "b").status, task_core::WorkUnitStatus::Done);
    assert_ne!(
        unit(&units, "integrate-s1").status,
        task_core::WorkUnitStatus::Done
    );
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Ready);
    let row = decision(store, root_id, key);
    assert_eq!(row.status, task_core::DecisionStatus::Open);
    assert_eq!(row.needed_before, vec!["a".to_string()]);
    assert_eq!(
        row.request.raised_by.origin,
        task_core::DecisionOrigin::Worker
    );
    assert_eq!(
        row.request.raised_by.run_id.as_deref(),
        unit(&units, "a").last_run_id.as_deref()
    );
    let inbox = task_ops::decision::open_rows(store.as_ref()).unwrap();
    assert_eq!(
        inbox.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        vec![row.id.as_str()]
    );
    let events = store.events_for(root_id).unwrap();
    assert_eq!(
        unit_reasons(&events, "a").last().map(String::as_str),
        Some("decision")
    );
    no_questions(store, root_id);
    row
}

/// 答えると a が最初から走り直し（continuation の文脈が無い）、木が done になる（yield・進捗なしの試験の共通の後半）。
async fn answer_and_finish(
    d: &mut Dispatcher,
    store: &Arc<SqliteStore>,
    adapter: &DecisionAdapter,
    root_id: TaskId,
    row: &task_core::DecisionRow,
    runs: u32,
) {
    let outcome = answer(store, &row.id, Some("org-vault"), None);
    assert_eq!(outcome.resumed, vec!["a".to_string()]);
    let report = run_until_idle(d, 1500).await;
    assert!(report.idle, "{report:?}");
    let units = store.work_units_for(root_id).unwrap();
    assert_eq!(unit(&units, "a").status, task_core::WorkUnitStatus::Done);
    assert_eq!(unit(&units, "a").runs, runs);
    let last = adapter
        .contexts()
        .into_iter()
        .rfind(|c| c.work_unit.as_ref().is_some_and(|w| w.key == "a"))
        .expect("a ran after the answer");
    assert!(
        last.continuation.is_none(),
        "the run after the answer starts over"
    );
    assert_eq!(last.work_unit.map(|w| w.human_decisions.len()), Some(1));
    assert_eq!(
        unit(&units, "integrate-s1").status,
        task_core::WorkUnitStatus::Done
    );
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Done);
    assert_replay_is_clean(store);
}

/// ADR 2026-11-21-worker-decisions-fail-closed D1・付記 2026-11-21（不変条件 I1・I2）: leaf が yield で終わる run の
/// `result.json` に `needed_before: [self]` の決定を書く。決定が受信箱の未回答の決定に出て、leaf は continuation に
/// 進まず `blocked(decision)` で止まり、integrate も root も進まない。答えると leaf が最初から走り、木が終わる。
/// 修正の前は done の遷移でしか `decisions` を読まなかったので、決定は 0 件で leaf は continuation の run で done に
/// なった。既存の試験の adapter は常に `summary` 付きの完了で終わるので、yield の run の要求を検出できない。
#[tokio::test]
async fn decision_fail_closed_yielded_leaf_records_decisions() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = root_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = v3_plan(
        vec![stage("s1", false)],
        vec![leaf("a", "s1", &[]), leaf("b", "s1", &[])],
    );
    let adapter = Arc::new(DecisionAdapter::new(vec![plan]).scripting(
        "a",
        vec![ScriptedRun {
            yielded: Some(yielded_checkpoint(1)),
            decisions: Some(serde_json::json!([decision_spec("vault", &["self"])])),
        }],
    ));
    let mut d = tree_dispatcher(&store, adapter.clone());
    run_until_quiet(&mut d, &store, 1500).await;

    let row = assert_leaf_held_by_worker_decision(&store, root_id, "vault", 1);
    let events = store.events_for(root_id).unwrap();
    assert_eq!(unit_reasons(&events, "a"), vec!["dispatch", "decision"]);
    answer_and_finish(&mut d, &store, &adapter, root_id, &row, 2).await;
}

/// ADR 2026-11-21-worker-decisions-fail-closed D1・付記 2026-11-21（不変条件 I1・I2・I7）: leaf の 2 回目の run が
/// 進捗なしの yield（`no_progress_limit = 1`）で終わり、`result.json` に `needed_before: [self]` の決定を書く。
/// 進捗なしの上限の質問（`blocked(limit)`）にせず、決定が受信箱に出て leaf は `blocked(decision)` で止まる
/// （止めは 1 か所）。答えると leaf が最初から走り、木が終わる。修正の前は `limit` の遷移で `decisions` を
/// 読まなかったので、決定は 0 件で leaf は `blocked(limit)`・task は質問になった。既存の試験の adapter は yield
/// しないので、進捗なしで終わる run の要求を検出できない。
#[tokio::test]
async fn decision_fail_closed_no_progress_leaf_records_decisions() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = root_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = v3_plan(
        vec![stage("s1", false)],
        vec![leaf("a", "s1", &[]), leaf("b", "s1", &[])],
    );
    let adapter = Arc::new(DecisionAdapter::new(vec![plan]).scripting(
        "a",
        vec![
            ScriptedRun {
                yielded: Some(yielded_checkpoint(1)),
                decisions: None,
            },
            ScriptedRun {
                yielded: Some(yielded_checkpoint(1)),
                decisions: Some(serde_json::json!([decision_spec("vault", &["self"])])),
            },
        ],
    ));
    let mut d = tree_dispatcher(&store, adapter.clone());
    d.config.execution.no_progress_limit = 1;
    d.config.execution.max_replans = 0;
    run_until_quiet(&mut d, &store, 1500).await;

    let row = assert_leaf_held_by_worker_decision(&store, root_id, "vault", 2);
    let units = store.work_units_for(root_id).unwrap();
    assert_ne!(
        unit(&units, "a").blocked_reason,
        Some(task_core::WorkUnitBlockedReason::Limit)
    );
    let events = store.events_for(root_id).unwrap();
    assert_eq!(
        unit_reasons(&events, "a"),
        vec!["dispatch", "continue", "dispatch", "decision"]
    );
    answer_and_finish(&mut d, &store, &adapter, root_id, &row, 3).await;
}

/// 木の leaf の run の `WorkerFinished.outcome`（`run_id` の run）。
fn finished_outcome(events: &[(u64, Event)], run_id: &str) -> String {
    events
        .iter()
        .find_map(|(_, e)| match e {
            Event::WorkerFinished {
                run_id: r, outcome, ..
            } if r == run_id => Some(outcome.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no worker_finished for {run_id}"))
}

/// a の最後の run（止めた run）の id。
fn last_run_of_a(store: &Arc<SqliteStore>, root_id: TaskId) -> String {
    let units = store.work_units_for(root_id).unwrap();
    unit(&units, "a").last_run_id.clone().expect("a has run")
}

/// root の決定の行の key（作った順）。
fn decision_keys(store: &Arc<SqliteStore>, root_id: TaskId) -> Vec<String> {
    store
        .decisions_list(Some(root_id))
        .unwrap()
        .into_iter()
        .map(|r| r.key)
        .collect()
}

fn auto_replans(events: &[(u64, Event)]) -> usize {
    events
        .iter()
        .filter(|(_, e)| matches!(e, Event::AutoReplanRequested { .. }))
        .count()
}

/// ADR 2026-11-21-worker-decisions-fail-closed D1・付記 2026-11-21（不変条件 I1・I2・I7。数えと checks の不合格の記録を
/// 変えない）: leaf a の check が落ち続ける。1 回目の run は `summary` だけで終わり check の不合格で再試行（1/2）になる。
/// 2 回目の run は `summary` と `needed_before: [self]` の決定を書き、check がまた落ちる。決定が受信箱の未回答の決定に
/// 出て a は `blocked(decision)` で止まり、再試行の run は答えまで起きない。1 回目で数えた retries は 1 のまま残り
/// （止めた run の遷移は `decision` で数えを動かさない）、`WorkUnitChecksFailed` は 2 run とも残る。答えると a が
/// 走り直し、その run の文脈に止めた run の check の不合格が載り、check が通れば木が終わる。修正の前は再試行の遷移で
/// `decisions` を読まなかったので、決定は 0 件で a は 3 回目の run に進んだ。既存の
/// `a_failed_work_unit_check_is_recorded_and_handed_to_the_next_run` は決定を書かず、
/// `decision_fail_closed_hold_clears_failure_side_effects` は settlement の関数だけで、check の不合格の記録・再試行の
/// run の抑止・答えの後の再開を通さないので、これを検出できない。
#[tokio::test]
async fn decision_fail_closed_check_failed_leaf_records_decisions() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut root = root_task(dir.path());
    root.budget.max_retries = 2;
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let marker = dir.path().join("fixed");
    let mut checked = leaf("a", "s1", &[]);
    checked["checks"] =
        serde_json::json!([{"cmd": format!("test -f {}", marker.display()), "expect_exit": 0}]);
    let plan = v3_plan(
        vec![stage("s1", false)],
        vec![checked, leaf("b", "s1", &[])],
    );
    let adapter = Arc::new(DecisionAdapter::new(vec![plan]).scripting(
        "a",
        vec![
            ScriptedRun {
                yielded: None,
                decisions: None,
            },
            ScriptedRun {
                yielded: None,
                decisions: Some(serde_json::json!([decision_spec("vault", &["self"])])),
            },
        ],
    ));
    let mut d = tree_dispatcher(&store, adapter.clone());
    run_until_quiet(&mut d, &store, 1500).await;

    let row = assert_leaf_held_by_worker_decision(&store, root_id, "vault", 2);
    let units = store.work_units_for(root_id).unwrap();
    assert_eq!(unit(&units, "a").retries, 1, "the counted retry stays");
    let events = store.events_for(root_id).unwrap();
    assert_eq!(
        unit_reasons(&events, "a"),
        vec!["dispatch", "retry", "dispatch", "decision"]
    );
    let held_run = last_run_of_a(&store, root_id);
    let failed_runs: Vec<&str> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::WorkUnitChecksFailed { run_id, key, .. } if key == "a" => Some(run_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(failed_runs.len(), 2, "both runs keep their check failure");
    assert_eq!(failed_runs[1], held_run.as_str());
    assert_eq!(
        finished_outcome(&events, &held_run),
        "decision: WorkUnit a は人の決定を待ちます（決定の要求 1 件。ADR-0079 D7）"
    );
    assert_eq!(auto_replans(&events), 0);
    // 答えるまで再試行の run は起きない。
    for _ in 0..10 {
        d.tick().unwrap();
    }
    assert_eq!(adapter.work_unit_contexts("a").len(), 2);
    assert_eq!(unit(&store.work_units_for(root_id).unwrap(), "a").runs, 2);

    // 答える → a が走り直し、止めた run の check の不合格を受け取る。check が通れば木が終わる。
    std::fs::write(&marker, "x").unwrap();
    let outcome = answer(&store, &row.id, Some("org-vault"), None);
    assert_eq!(outcome.resumed, vec!["a".to_string()]);
    let report = run_until_idle(&mut d, 1500).await;
    assert!(report.idle, "{report:?}");
    let units = store.work_units_for(root_id).unwrap();
    assert_eq!(unit(&units, "a").status, task_core::WorkUnitStatus::Done);
    assert_eq!(unit(&units, "a").runs, 3);
    assert_eq!(unit(&units, "a").retries, 1);
    let a_contexts = adapter.work_unit_contexts("a");
    assert_eq!(a_contexts.len(), 3);
    assert!(
        a_contexts[2]
            .previous_check_failures
            .iter()
            .any(|l| l.contains("test -f")),
        "{:?}",
        a_contexts[2].previous_check_failures
    );
    assert_eq!(a_contexts[2].human_decisions.len(), 1);
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Done);
    assert_replay_is_clean(&store);
}

/// ADR 2026-11-21-worker-decisions-fail-closed D1・付記 2026-11-21（不変条件 I1・I2・I7）: leaf a の run が yield の
/// `plan_issue`（計画の問題の申告）と `needed_before: [self]` の決定を書く。replan の余地（`max_replans` の既定）が
/// あっても `AutoReplanRequested` は無く、planner は走らず、決定 1 件が受信箱に出て a は `blocked(decision)` で止まる。
/// 申告の文は止めた run の outcome に残る。答えると a が最初から走り、木が終わる。修正の前は `plan_issue` の遷移で
/// `decisions` を読まず、決定は 0 件で自動 replan が起きた。既存の試験は計画の問題を申告する run に決定を書かせない
/// （`decision_fail_closed_hold_clears_failure_side_effects` は settlement の関数だけで、dispatcher が
/// `AutoReplanRequested` を残さないこと・planner を起こさないことを通さない）ので、これを検出できない。
#[tokio::test]
async fn decision_fail_closed_plan_issue_leaf_records_decisions_before_replan() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = root_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = v3_plan(
        vec![stage("s1", false)],
        vec![leaf("a", "s1", &[]), leaf("b", "s1", &[])],
    );
    let issue = "the check needs the vault choice first";
    let adapter = Arc::new(DecisionAdapter::new(vec![plan]).scripting(
        "a",
        vec![ScriptedRun {
            yielded: Some(serde_json::json!({
                "completed": ["step 0"],
                "remaining": ["finish"],
                "plan_issue": issue,
            })),
            decisions: Some(serde_json::json!([decision_spec("vault", &["self"])])),
        }],
    ));
    let mut d = tree_dispatcher(&store, adapter.clone());
    assert!(d.config.execution.max_replans > 0, "a replan is available");
    run_until_quiet(&mut d, &store, 1500).await;

    let events = store.events_for(root_id).unwrap();
    assert_eq!(auto_replans(&events), 0, "no AutoReplanRequested");
    let row = assert_leaf_held_by_worker_decision(&store, root_id, "vault", 1);
    assert_eq!(decision_keys(&store, root_id), vec!["vault".to_string()]);
    assert_eq!(unit_reasons(&events, "a"), vec!["dispatch", "decision"]);
    assert_eq!(adapter.planner_contexts().len(), 1, "no replan planner run");
    assert_eq!(store.execution_plan_list(root_id).unwrap().len(), 1);
    let held_run = last_run_of_a(&store, root_id);
    let outcome = finished_outcome(&events, &held_run);
    assert!(
        outcome.starts_with("decision: WorkUnit a は人の決定を待ちます（決定の要求 1 件。")
            && outcome.contains(issue),
        "{outcome}"
    );
    answer_and_finish(&mut d, &store, &adapter, root_id, &row, 2).await;
    assert_eq!(auto_replans(&store.events_for(root_id).unwrap()), 0);
    assert_eq!(store.execution_plan_list(root_id).unwrap().len(), 1);
}

/// ADR 2026-11-21-worker-decisions-fail-closed D1・付記 2026-11-21（不変条件 I1・I7。replan の上限を変えない）:
/// `max_replans = 0`（replan を使い切った木）で leaf a の check が落ち（`max_retries = 0` で失敗）、その run が
/// `needed_before: [self]` の決定を書く。leaf を止める決定は worker の決定 1 件だけで、`limit:max_replans` の決定は
/// 出ない（1 つの leaf を止めるのは 1 か所）。答えた後の run が決定を書かずにまた失敗すると、今までどおり
/// `limit:max_replans` の決定が出る（上限と数えは変わらない）。修正の前は失敗の遷移で `decisions` を読まず、
/// worker の決定は 0 件で `limit:max_replans` の決定だけが出た。既存の
/// `a_tree_node_leaf_failure_after_replans_are_exhausted_raises_the_limit_decision` は決定を書かない run しか通さない
/// ので、これを検出できない。
#[tokio::test]
async fn decision_fail_closed_replan_exhausted_leaf_records_one_decision() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut root = root_task(dir.path());
    root.budget.max_retries = 0;
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let mut failing = leaf("a", "s1", &[]);
    failing["checks"] = serde_json::json!([{"cmd": "false", "expect_exit": 0}]);
    let plan = v3_plan(
        vec![stage("s1", false)],
        vec![failing, leaf("b", "s1", &[])],
    );
    let adapter = Arc::new(
        DecisionAdapter::new(vec![plan])
            .declaring("a", serde_json::json!([decision_spec("vault", &["self"])])),
    );
    let mut d = tree_dispatcher(&store, adapter.clone());
    d.config.execution.max_replans = 0;
    run_until_quiet(&mut d, &store, 1500).await;

    let row = assert_leaf_held_by_worker_decision(&store, root_id, "vault", 1);
    assert_eq!(
        decision_keys(&store, root_id),
        vec!["vault".to_string()],
        "no limit:max_replans"
    );
    let events = store.events_for(root_id).unwrap();
    assert_eq!(unit_reasons(&events, "a"), vec!["dispatch", "decision"]);
    assert_eq!(auto_replans(&events), 0);
    let outcome = finished_outcome(&events, &last_run_of_a(&store, root_id));
    assert!(!outcome.contains("max_replans"), "{outcome}");
    assert_eq!(
        events
            .iter()
            .filter(|(_, e)| matches!(e, Event::WorkUnitChecksFailed { key, .. } if key == "a"))
            .count(),
        1
    );

    // 答えた後の run は決定を書かずにまた失敗する → replan の上限が今までどおり効き、`limit:max_replans` が出る。
    let answered = answer(&store, &row.id, Some("org-vault"), None);
    assert_eq!(answered.resumed, vec!["a".to_string()]);
    run_until_quiet(&mut d, &store, 1500).await;
    let units = store.work_units_for(root_id).unwrap();
    assert_eq!(unit(&units, "a").runs, 2);
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Ready);
    assert_eq!(
        decision_keys(&store, root_id),
        vec!["vault".to_string(), "limit:max_replans".to_string()]
    );
    let limit = decision(&store, root_id, "limit:max_replans");
    let inbox = task_ops::decision::open_rows(store.as_ref()).unwrap();
    assert_eq!(
        inbox.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        vec![limit.id.as_str()]
    );
    no_questions(&store, root_id);
    assert_replay_is_clean(&store);
}

/// ADR 2026-11-21-worker-decisions-fail-closed D1・付記 2026-11-21（不変条件 I1・I2・I7）: leaf a の run が
/// `result.json` に `question` と `needed_before: [self]` の決定を併記して質問で終わる。質問（`QuestionRaised`・承認の
/// 行）にせず、決定が受信箱に出て a は `blocked(decision)` で止まり、worker の質問の文は止めた run の outcome
/// （`。止めた質問: …`）に残る。答えると a が最初から走り、木が終わる。修正の前は質問の遷移で `decisions` を読まず、
/// 決定は 0 件で a は `blocked(question)`・task は質問になった。既存の試験の adapter は質問で終わらない
/// （`decision_fail_closed_hold_clears_failure_side_effects` は settlement の関数だけで、`QuestionRaised` を残さない
/// ことを通さない）ので、これを検出できない。
#[tokio::test]
async fn decision_fail_closed_question_leaf_records_decisions() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = root_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = v3_plan(
        vec![stage("s1", false)],
        vec![leaf("a", "s1", &[]), leaf("b", "s1", &[])],
    );
    let question = "which vault holds the deploy key?";
    let adapter = Arc::new(
        DecisionAdapter::new(vec![plan])
            .declaring("a", serde_json::json!([decision_spec("vault", &["self"])]))
            .asking("a", question),
    );
    let mut d = tree_dispatcher(&store, adapter.clone());
    run_until_quiet(&mut d, &store, 1500).await;

    let row = assert_leaf_held_by_worker_decision(&store, root_id, "vault", 1);
    assert_eq!(decision_keys(&store, root_id), vec!["vault".to_string()]);
    let events = store.events_for(root_id).unwrap();
    assert_eq!(unit_reasons(&events, "a"), vec!["dispatch", "decision"]);
    assert_eq!(
        finished_outcome(&events, &last_run_of_a(&store, root_id)),
        format!(
            "decision: WorkUnit a は人の決定を待ちます（決定の要求 1 件。ADR-0079 D7）。止めた質問: {question}"
        )
    );
    answer_and_finish(&mut d, &store, &adapter, root_id, &row, 2).await;
}

/// 段階 s1 に leaf a と子 k（atomic の節点）を置く計画。
fn atomic_node_plan() -> String {
    v3_plan(
        vec![stage("s1", false)],
        vec![leaf("a", "s1", &[]), task_unit("k", "s1", &[], "true")],
    )
}

/// task の `Transitioned.reason`（古い順）。
fn task_reasons(events: &[(u64, Event)]) -> Vec<String> {
    events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::Transitioned { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .collect()
}

/// 子 k（atomic の節点）が worker の決定 `vault`（`needed_before = [self]`）で止まったことを確かめる（yield・失敗の
/// 試験の共通の前半）。決定は open・origin worker・止めた run の行で受信箱の未回答の決定に出て、k は run 1 本で
/// `ready` のまま done にならず、unit k と root も終わらず、質問も出ない。tick を回しても答えまで次の run は起きない。
/// 子 task と決定の行と、止めた run の `WorkerFinished.outcome` を返す。
fn assert_atomic_node_held_by_worker_decision(
    d: &mut Dispatcher,
    store: &Arc<SqliteStore>,
    root_id: TaskId,
) -> (Task, task_core::DecisionRow, String) {
    let child = child_named(store, root_id, "k").expect("child k");
    let runs = store.runs_for_task(child.id).unwrap();
    assert_eq!(runs.len(), 1, "no next run before the answer");
    assert_eq!(store.get(child.id).unwrap().unwrap().status, Status::Ready);
    let units = store.work_units_for(root_id).unwrap();
    assert_ne!(unit(&units, "k").status, task_core::WorkUnitStatus::Done);
    assert_ne!(store.get(root_id).unwrap().unwrap().status, Status::Done);
    let row = decision(store, root_id, "vault");
    assert_eq!(row.task_id, child.id);
    assert_eq!(row.status, task_core::DecisionStatus::Open);
    assert_eq!(row.needed_before, vec!["self".to_string()]);
    assert_eq!(
        row.request.raised_by.origin,
        task_core::DecisionOrigin::Worker
    );
    assert_eq!(
        row.request.raised_by.run_id.as_deref(),
        Some(runs[0].run_id.as_str())
    );
    let inbox = task_ops::decision::open_rows(store.as_ref()).unwrap();
    assert_eq!(
        inbox.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        vec![row.id.as_str()]
    );
    no_questions(store, child.id);
    for _ in 0..10 {
        d.tick().unwrap();
    }
    assert_eq!(store.runs_for_task(child.id).unwrap().len(), 1);
    assert_eq!(store.get(child.id).unwrap().unwrap().status, Status::Ready);
    let outcome = finished_outcome(&store.events_for(child.id).unwrap(), &runs[0].run_id);
    (child, row, outcome)
}

/// 答えると k の次の run が走って done になり（答えは `Answered` の event と前置きの `answers` に入る）、root も終わる
/// （yield・失敗の試験の共通の後半）。答えの後の k の run の文脈（WU・planner・review でない最後の run。この木で
/// それに当たるのは k の run だけ）を返す。
async fn answer_atomic_node_and_finish(
    d: &mut Dispatcher,
    store: &Arc<SqliteStore>,
    adapter: &DecisionAdapter,
    root_id: TaskId,
    child: &Task,
    row: &task_core::DecisionRow,
) -> task_worker::RunContext {
    answer(store, &row.id, Some("org-vault"), None);
    let report = run_until_idle(d, 1500).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.runs_for_task(child.id).unwrap().len(), 2);
    assert_eq!(store.get(child.id).unwrap().unwrap().status, Status::Done);
    assert!(
        store
            .events_for(child.id)
            .unwrap()
            .iter()
            .any(|(_, e)| matches!(e, Event::Answered { answer, .. }
                if answer.starts_with("- vault question vault: org vault")))
    );
    let last = adapter
        .contexts()
        .into_iter()
        .rfind(|c| c.work_unit.is_none() && c.execution_planner.is_none() && c.review.is_none())
        .expect("k ran after the answer");
    assert!(
        last.answers
            .iter()
            .any(|a| a.answer.starts_with("- vault question vault: org vault")),
        "{:?}",
        last.answers
    );
    let units = store.work_units_for(root_id).unwrap();
    assert_eq!(unit(&units, "k").status, task_core::WorkUnitStatus::Done);
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Done);
    no_questions(store, root_id);
    assert_replay_is_clean(store);
    last
}

/// ADR 2026-11-21-worker-decisions-fail-closed D1・付記「atomic の規則」（不変条件 I1・I2。continuation の数えと
/// checkpoint を変えない）: 木の子の atomic の節点 k の run が yield で終わり、`result.json` に `needed_before: [self]`
/// の決定を書く。決定が受信箱の未回答の決定に出て、k は答えるまで次の run（continuation）を起こさず done にならない。
/// continuation の trigger（遷移 `continue`）と checkpoint は残り、止めた run の outcome は `continue: ` の文の末尾に
/// 決定を待つことを足したものになる。答えると k の次の run が checkpoint の続き（continuation）として走って done になり、
/// root も終わる。修正の前は atomic の run は done の run でしか `decisions` を読まなかったので、決定は 0 件で k は
/// continuation の run で done になった。既存の試験（`worker_declared_decisions` の子 k・
/// `decision_fail_closed_non_array_decisions_hold_the_atomic_node`）は atomic の節点を done の run でしか通さないので、
/// yield の run の要求を検出できない。
#[tokio::test]
async fn decision_fail_closed_yielded_atomic_node_records_decisions() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = root_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let adapter = Arc::new(DecisionAdapter::new(vec![atomic_node_plan()]).scripting(
        "k",
        vec![ScriptedRun {
            yielded: Some(yielded_checkpoint(1)),
            decisions: Some(serde_json::json!([decision_spec("vault", &["self"])])),
        }],
    ));
    let mut d = tree_dispatcher(&store, adapter.clone());
    run_until_quiet(&mut d, &store, 1500).await;

    let (child, row, outcome) = assert_atomic_node_held_by_worker_decision(&mut d, &store, root_id);
    let events = store.events_for(child.id).unwrap();
    assert_eq!(
        task_reasons(&events).last().map(String::as_str),
        Some("continue")
    );
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, Event::CheckpointSaved { .. })),
        "the checkpoint stays"
    );
    assert!(
        outcome.starts_with("continue: ")
            && outcome.ends_with("。人の決定を待ちます（決定の要求 1 件。ADR-0079 D7）"),
        "{outcome}"
    );
    let last = answer_atomic_node_and_finish(&mut d, &store, &adapter, root_id, &child, &row).await;
    assert!(
        last.continuation.is_some(),
        "the run after the answer continues from the checkpoint"
    );
}

/// ADR 2026-11-21-worker-decisions-fail-closed D1・付記「atomic の規則」（不変条件 I1・I2）: 木の子の atomic の節点 k の
/// run が `result.json` に `needed_before: [self]` の決定を書いて失敗（`Terminal::Error{retryable: true}`）で終わる。
/// 決定が受信箱の未回答の決定に出て、k は再試行の run を起こさず、`failed` にも done にもならない（遷移 `advance` で
/// `ready`。試行を消費しない）。止めた run の outcome は決定を待つ文になる。答えると k の次の run が最初から走って done
/// になり、root も終わる。修正の前は失敗の run で `decisions` を読まなかったので、決定は 0 件で k は再試行の run で
/// done になった。既存の試験は atomic の節点を done の run でしか通さないので、失敗の run の要求を検出できない。
#[tokio::test]
async fn decision_fail_closed_failed_atomic_node_records_decisions() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = root_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let adapter = Arc::new(
        DecisionAdapter::new(vec![atomic_node_plan()])
            .declaring("k", serde_json::json!([decision_spec("vault", &["self"])]))
            .failing("k", "the vault is not configured"),
    );
    let mut d = tree_dispatcher(&store, adapter.clone());
    run_until_quiet(&mut d, &store, 1500).await;

    let (child, row, outcome) = assert_atomic_node_held_by_worker_decision(&mut d, &store, root_id);
    let events = store.events_for(child.id).unwrap();
    assert_eq!(
        task_reasons(&events).last().map(String::as_str),
        Some("advance")
    );
    assert_eq!(store.get(child.id).unwrap().unwrap().attempts, 0);
    assert_eq!(
        outcome,
        "decision: 人の決定を待ちます（決定の要求 1 件。ADR-0079 D7）"
    );
    let last = answer_atomic_node_and_finish(&mut d, &store, &adapter, root_id, &child, &row).await;
    assert!(
        last.continuation.is_none(),
        "the run after the answer starts over"
    );
}
