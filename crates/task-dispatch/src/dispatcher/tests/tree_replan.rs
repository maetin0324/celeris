//! ADR-0079 §7 R2b: planner の再帰（/3 のプロンプトの入力）、/3 の計画が 2 回不正なら `plan_invalid` の決定の要求
//! （atomic に倒さない）、子の work の失敗 → 親の replan（子の失敗の理由と checkpoint を planner に渡し、同じ unit
//! から次の子を作る）、子の基盤の失敗 → 1 回だけ自動で作り直し、それでも失敗なら障害通知と unit `blocked(infra)`、
//! replan は木の上限（`max_tree_replans`）と節点の上限（`max_replans`）で止まり決定の要求になる。
//! すべて偽のアダプタと一時ディレクトリ（と一時の git）だけで、外部ネットワークに出ない。

use super::tree::{assert_replay_is_clean, leaf, stage, task_unit, tree_dispatcher, unit, v3_plan};
use super::*;

/// planner run には計画の列を順に書き、run の文脈を記録する。子 task の run（`work_unit` の無い run）は
/// 題名の `Child ` の後の key で `<key>.txt` を作業ツリーに書く（git の試験で子のブランチに成果を残す）。
/// `infra_fail` の key の子の run は分類できない失敗（`AdapterError::Other`。ADR-0070 D3 の基盤の失敗）で終わる。
struct ReplanAdapter {
    plans: StdMutex<std::collections::VecDeque<String>>,
    seen: StdMutex<Vec<task_worker::RunContext>>,
    infra_fail: Vec<String>,
}

impl ReplanAdapter {
    fn new(plans: Vec<String>) -> Self {
        ReplanAdapter {
            plans: StdMutex::new(plans.into_iter().collect()),
            seen: StdMutex::new(Vec::new()),
            infra_fail: Vec::new(),
        }
    }

    fn failing_infra(mut self, key: &str) -> Self {
        self.infra_fail.push(key.to_string());
        self
    }

    fn planner_contexts(&self) -> Vec<task_worker::protocol::ExecutionPlannerContext> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter_map(|c| c.execution_planner.clone())
            .collect()
    }
}

#[async_trait]
impl WorkerAdapter for ReplanAdapter {
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
            if req.context.work_unit.is_none() && self.infra_fail.contains(&key) {
                return Err(AdapterError::Other(format!(
                    "simulated harness crash in {key}"
                )));
            }
            let cwd = req
                .work_dir
                .clone()
                .unwrap_or_else(|| req.workspace.clone());
            std::fs::write(cwd.join(format!("{key}.txt")), &key).ok();
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

fn planner_runs(store: &Arc<dyn TaskStore>, task: TaskId) -> usize {
    store
        .runs_for_task(task)
        .unwrap()
        .iter()
        .filter(|r| r.role == task_core::RunIndexRole::Planner)
        .count()
}

fn worker_runs(store: &Arc<dyn TaskStore>, task: TaskId) -> usize {
    store
        .runs_for_task(task)
        .unwrap()
        .iter()
        .filter(|r| r.role == task_core::RunIndexRole::Worker)
        .count()
}

fn decisions_of(store: &Arc<dyn TaskStore>, root: TaskId) -> Vec<task_core::DecisionRow> {
    store.decisions_list(Some(root)).unwrap()
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

/// 子の題名の key（`Child <key>`）の子 task を、作られた順にすべて。
fn children_titled(store: &Arc<dyn TaskStore>, root: TaskId, key: &str) -> Vec<Task> {
    let mut out: Vec<Task> = store
        .children(root)
        .unwrap()
        .into_iter()
        .filter(|c| c.title == format!("Child {key}"))
        .collect();
    out.sort_by_key(|c| c.created_at);
    out
}

fn attempt_of(task: &Task) -> u32 {
    task.tree
        .as_ref()
        .and_then(|t| t.parent_unit.as_ref())
        .map(|u| u.attempt)
        .unwrap_or(0)
}

/// ADR-0079 §7 R2b (a) の配線: 木が有効な root の planner run の文脈に `tree` が入り、深さ 1・残りの深さ 2・
/// 計画の上限・木の残り（run を 1 本使った後）・人の段階の名指し（`routing.stages_hint`）が dispatcher の設定と
/// 木の数え上げの値のまま渡る。子（深さ 2）の planner には残りの深さ 1 と祖先（root の題名・段階）が渡る。
/// 木が無効なら `tree` は無い（/1・/2 のプロンプトは変わらない）。
#[tokio::test]
async fn tree_planner_context_is_wired_from_limits_and_counters() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut root = compound_task(dir.path());
    root.title = "browser capability".into();
    root.budget.max_turns = 30;
    if let Some(r) = root.routing.as_mut() {
        r.stages_hint = vec![task_core::StageHint {
            title: "Phase 1".into(),
            scope: "MVP".into(),
        }];
    }
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let mut cb = task_unit("cb", "s1", &[], "true");
    cb["features"] = serde_json::json!({"expected_length": "high", "cross_cutting": "high"});
    // ADR-0079 付記「R6-2: unit の gate 欄と kind task の既定（compound explicit）」: 共通 fixture の `gate: atomic` を外す
    // （kind task の既定 = 明示の compound。子は従来どおり自分の planner に進む）。
    cb.as_object_mut().unwrap().remove("gate");
    let root_plan = v3_plan(vec![stage("s1", false)], vec![cb]);
    let child_plan = v3_plan(vec![stage("t1", false)], vec![leaf("l", "t1", &[])]);
    let adapter = Arc::new(ReplanAdapter::new(vec![root_plan, child_plan]));
    let mut d = tree_dispatcher(&store, adapter.clone());
    d.config.execution.limits.tree.max_stages = 4;
    d.config.execution.limits.tree.max_tree_runs = 50;
    let report = run_until_idle(&mut d, 1500).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Done);

    let contexts = adapter.planner_contexts();
    assert_eq!(contexts.len(), 2, "{contexts:?}");
    let root_tree = contexts[0]
        .tree
        .clone()
        .expect("root planner gets the tree context");
    assert_eq!(
        (
            root_tree.depth,
            root_tree.max_depth,
            root_tree.remaining_depth
        ),
        (1, 3, 2)
    );
    assert_eq!(root_tree.max_stages, 4);
    assert_eq!(root_tree.max_units_per_stage, 6);
    assert_eq!(root_tree.max_child_tasks_per_plan, 6);
    assert_eq!(root_tree.max_decisions_per_plan, 8);
    assert_eq!(
        root_tree.runs_left, 50,
        "no run counted before the first planner run"
    );
    // ADR-0079 付記「R6-2」: 木の上限の既定は leaf 120 / 木の replan 30（40 / 10 から）。節点の replan は
    // `DispatchConfig` の既定（3）のまま（`celeris::config` の既定 5 は daemon の設定から渡る）。
    assert_eq!(root_tree.leaves_left, 120);
    assert_eq!(root_tree.replans_left, 30);
    assert_eq!(root_tree.node_replans_left, 3);
    assert_eq!(root_tree.open_decisions_left, 12);
    assert!(root_tree.ancestors.is_empty());
    assert_eq!(root_tree.stages_hint.len(), 1);
    assert_eq!(root_tree.stages_hint[0].title, "Phase 1");

    let child_tree = contexts[1].tree.clone().expect("the child planner too");
    assert_eq!((child_tree.depth, child_tree.remaining_depth), (2, 1));
    assert_eq!(child_tree.ancestors.len(), 1);
    assert_eq!(child_tree.ancestors[0].title, "browser capability");
    assert_eq!(child_tree.ancestors[0].stage.as_deref(), Some("s1"));
    assert!(child_tree.runs_left < 50, "the root planner run is counted");
    assert!(
        child_tree.stages_hint.is_empty(),
        "the hint belongs to the root"
    );
    assert_replay_is_clean(&store);

    // 木が無効: /2 の planner に `tree` は無い。
    let store2: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let plain = compound_task(dir.path());
    store2.create_task(&plain, vec![]).unwrap();
    let v1 = serde_json::json!({
        "schema": task_core::EXECUTION_PLAN_SCHEMA,
        "rationale": "one step",
        "work_units": [{"key": "a", "kind": "implement", "title": "A", "objective": "do a",
                        "done_when": ["a"], "checks": [{"cmd": "true", "expect_exit": 0}]}],
    })
    .to_string();
    let adapter2 = Arc::new(ReplanAdapter::new(vec![v1]));
    let mut d2 = tree_dispatcher(&store2, adapter2.clone());
    d2.config.execution.limits = task_core::ExecutionLimits::default();
    run_until_idle(&mut d2, 600).await;
    let contexts = adapter2.planner_contexts();
    assert!(!contexts.is_empty());
    assert!(contexts.iter().all(|c| c.tree.is_none()));
}

/// ADR-0079 §7 R2b (b) `v3_invalid_plan_asks_instead_of_atomic`: 偽の planner が 2 回とも不正な /3（checks の無い
/// leaf）を出すと、atomic に倒れず（gate は compound のまま、worker の run は 0）、`kind: plan_invalid` の決定の
/// 要求が 1 件出る（検証の理由つき、`needed_before: [self]`、path = root）。Task は `ready` のまま run を起こさない
/// （tick を重ねても planner run は 2、決定は 1）。質問・approvals は無い。木が無効な /2 は従来どおり atomic に倒れる。
#[tokio::test]
async fn v3_invalid_plan_asks_instead_of_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = compound_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let mut bad = leaf("a", "s1", &[]);
    bad.as_object_mut().unwrap().remove("checks");
    let bad_plan = v3_plan(vec![stage("s1", false)], vec![bad]);
    let adapter = Arc::new(ReplanAdapter::new(vec![bad_plan.clone(), bad_plan]));
    let mut d = tree_dispatcher(&store, adapter.clone());
    run_until_quiet(&mut d, &store, 800).await;
    for _ in 0..20 {
        d.tick().unwrap();
    }

    assert_eq!(planner_runs(&store, root_id), 2);
    assert_eq!(worker_runs(&store, root_id), 0, "no atomic run");
    let stored = store.get(root_id).unwrap().unwrap();
    assert_eq!(
        stored.status,
        Status::Ready,
        "{:?}",
        store.events_for(root_id)
    );
    let gate = stored
        .routing
        .as_ref()
        .and_then(|r| r.execution.clone())
        .unwrap();
    assert_eq!(gate.mode, task_core::ExecutionMode::Compound);
    assert_ne!(gate.rule_id, "atomic/planner-invalid");
    assert!(store.execution_plan_active(root_id).unwrap().is_none());
    let decisions = decisions_of(&store, root_id);
    assert_eq!(decisions.len(), 1, "{decisions:?}");
    let dr = &decisions[0];
    assert_eq!(dr.kind, task_core::DecisionKind::PlanInvalid);
    assert_eq!(dr.key, task_core::tree::PLAN_INVALID_DECISION_KEY);
    assert_eq!(dr.status, task_core::DecisionStatus::Open);
    assert_eq!(dr.task_id, root_id);
    assert_eq!(dr.needed_before, vec!["self".to_string()]);
    assert_eq!(
        dr.request.raised_by.origin,
        task_core::DecisionOrigin::Daemon
    );
    assert!(dr.request.raised_by.run_id.is_some());
    assert_eq!(dr.request.path.first().map(|p| p.task_id), Some(root_id));
    let keys: Vec<&str> = dr.request.options.iter().map(|o| o.key.as_str()).collect();
    assert_eq!(keys, vec!["replan", "atomic", "cancel"]);
    let note = dr.request.cost_note.clone().unwrap_or_default();
    assert!(note.contains("checks"), "{note}");
    // 2 回目の planner には 1 回目の拒否の理由が渡った（F5-fix3 のまま）。
    let contexts = adapter.planner_contexts();
    assert_eq!(contexts.len(), 2);
    assert!(
        contexts[1]
            .previous_attempt_errors
            .iter()
            .any(|e| e.contains("checks"))
    );
    no_questions(&store, root_id);
    assert_replay_is_clean(&store);

    // 木が無効（/2）: 2 回不正なら従来どおり atomic に倒れ、決定の要求は無い。
    let store2: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let plain = compound_task(dir.path());
    let plain_id = plain.id;
    store2.create_task(&plain, vec![]).unwrap();
    let bad2 = serde_json::json!({
        "schema": task_core::EXECUTION_PLAN_SCHEMA_V2,
        "rationale": "broken",
        "phases": [],
        "work_units": [],
    })
    .to_string();
    let adapter2 = Arc::new(ReplanAdapter::new(vec![bad2.clone(), bad2]));
    let mut d2 = tree_dispatcher(&store2, adapter2);
    d2.config.execution.limits = task_core::ExecutionLimits::default();
    d2.config.execution.parallel = true;
    run_until_idle(&mut d2, 600).await;
    let stored = store2.get(plain_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done);
    let gate = stored
        .routing
        .as_ref()
        .and_then(|r| r.execution.clone())
        .unwrap();
    assert_eq!(gate.mode, task_core::ExecutionMode::Atomic);
    assert_eq!(gate.rule_id, "atomic/planner-invalid");
    assert!(store2.decisions_list(None).unwrap().is_empty());
}

/// ADR-0079 §7 R2b (c) `child_failure_triggers_parent_replan`（一時の git）: 段階 s1 = leaf a + 子 c。子 c の最終
/// レビューが落ち続けて c が failed（work）になると、unit c は failed、親は replan の planner run を起こし、その
/// 入力（`replan_reason` と unit の要約）に子の題名・分類・不合格の理由がある。planner は /3 の全体を書き（done の
/// leaf a は省略 → daemon が持ち越す）、同じ key c の受け入れを直す。同じ unit から次の子（attempt 2）が作られて
/// done になり、統合は新しい子を merge し、root は done。元の子の task とブランチは残る。
#[tokio::test]
async fn child_failure_triggers_parent_replan() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut root = git_task(
        repo.path(),
        None,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
    );
    root.routing = Some(task_core::TaskRouting {
        execution_hint: Some(task_core::ExecutionHintSpec {
            mode: task_core::ExecutionMode::Compound,
            explicit: true,
        }),
        ..Default::default()
    });
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let mut a = leaf("a", "s1", &[]);
    a["checks"] = serde_json::json!([{"cmd": "test -f a.txt", "expect_exit": 0}]);
    let v1 = v3_plan(
        vec![stage("s1", false)],
        vec![a, task_unit("c", "s1", &[], "test -f never-written.txt")],
    );
    // replan: done の leaf a は書かない（daemon が持ち越す）。c は同じ key で受け入れを直す。
    let v2 = v3_plan(
        vec![stage("s1", false)],
        vec![task_unit("c", "s1", &[], "test -f c.txt")],
    );
    let adapter = Arc::new(ReplanAdapter::new(vec![v1, v2]));
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), ws.path(), 3, 3, 3);
    d.config.execution.gate = task_core::GateMode::On;
    d.config.execution.planner.adapter = "instant".to_string();
    d.config.execution.limits = super::tree::tree_limits();
    let report = run_until_idle(&mut d, 2500).await;
    assert!(report.idle, "{report:?}");
    let events = store.events_for(root_id).unwrap();
    assert_eq!(
        store.get(root_id).unwrap().unwrap().status,
        Status::Done,
        "{events:?}"
    );

    // 子は 2 つ: 1 回目（failed、attempt 1）と 2 回目（done、attempt 2）。どちらも同じ unit c から。
    let children = children_titled(&store, root_id, "c");
    assert_eq!(children.len(), 2, "{children:?}");
    assert_eq!(children[0].status, Status::Failed);
    assert_eq!(attempt_of(&children[0]), 1);
    assert_eq!(children[1].status, Status::Done);
    assert_eq!(attempt_of(&children[1]), 2);
    let units = store.work_units_for(root_id).unwrap();
    let c = unit(&units, "c");
    assert_eq!(c.status, task_core::WorkUnitStatus::Done);
    assert_eq!(c.child_task_id, Some(children[1].id.to_string()));
    assert_eq!(unit(&units, "a").status, task_core::WorkUnitStatus::Done);

    // 計画は 2 版。v2 には持ち越した done の a がある（planner は書かなかった）。
    let plans = store.execution_plan_list(root_id).unwrap();
    assert_eq!(plans.len(), 2);
    let v2_plan = plans.iter().find(|p| p.version == 2).unwrap();
    assert!(
        v2_plan.spec.units.iter().any(|u| u.key == "a"),
        "{v2_plan:?}"
    );
    assert_eq!(planner_runs(&store, root_id), 2);

    // replan の planner の入力: 子の失敗の理由と unit の要約（子の状態・分類）。
    let contexts = adapter.planner_contexts();
    let replan = contexts
        .iter()
        .find(|c| c.replan)
        .expect("a replan planner run");
    assert!(replan.tree.is_some());
    assert!(
        replan.replan_reason.contains("Child c")
            && replan.replan_reason.contains("attempt 1")
            && replan.replan_reason.contains("(work)")
            && replan.replan_reason.contains("never-written"),
        "{}",
        replan.replan_reason
    );
    assert!(
        replan
            .work_unit_summaries
            .iter()
            .any(|l| l.starts_with("c (task) status=failed") && l.contains("is failed (work)")),
        "{:?}",
        replan.work_unit_summaries
    );
    assert_eq!(replan.preserve_done_keys, vec!["a".to_string()]);

    // 統合 s1 は新しい子のブランチを merge した。元の子のブランチは残る。
    let merged: Vec<(String, String)> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::PhaseIntegrated { phase, merged, .. } if phase == "s1" => Some(
                merged
                    .iter()
                    .map(|m| (m.key.clone(), m.commit.clone()))
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .flatten()
        .collect();
    assert!(merged.iter().any(|(k, _)| k == "c"), "{merged:?}");
    assert!(
        crate::integration::rev_parse(
            repo.path(),
            &format!("refs/heads/celeris/{}", children[0].id)
        )
        .is_some(),
        "the failed child's branch is kept"
    );
    no_questions(&store, root_id);
    assert!(store.decisions_list(Some(root_id)).unwrap().is_empty());
    assert_replay_is_clean(&store);
}

/// ADR-0079 §7 R2b (d) `child_infra_failure_is_not_a_question`: 子 c の run が基盤の失敗（分類できない adapter の
/// 失敗、`max_infra_retries = 0` で `infra failure ×1`）で failed になると、親は replan せず同じ unit から 1 回だけ
/// 子を作り直す（attempt 2、`child_infra_retry`）。2 回目も失敗したら unit c は `blocked(infra)`、障害通知
/// （`TaskFailed`、基盤の分類）が 1 件、決定の要求・質問は無く、planner は 1 回だけ。同じ段階の兄弟 s（子）と
/// leaf a は done まで走り、段階は完了しない（root は ready のまま待つ）。
#[tokio::test]
async fn child_infra_failure_is_not_a_question() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = compound_task(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = v3_plan(
        vec![stage("s1", false)],
        vec![
            leaf("a", "s1", &[]),
            task_unit("c", "s1", &[], "true"),
            task_unit("s", "s1", &[], "true"),
        ],
    );
    let adapter = Arc::new(ReplanAdapter::new(vec![plan]).failing_infra("c"));
    let mut d = tree_dispatcher(&store, adapter.clone());
    d.config.max_infra_retries = 0;
    run_until_quiet(&mut d, &store, 1500).await;
    for _ in 0..10 {
        d.tick().unwrap();
    }

    let children = children_titled(&store, root_id, "c");
    assert_eq!(children.len(), 2, "one automatic retry only: {children:?}");
    for (i, child) in children.iter().enumerate() {
        assert_eq!(child.status, Status::Failed);
        assert_eq!(attempt_of(child), i as u32 + 1);
        let (class, _) =
            task_ops::derive::classify_task_failure(&store.events_for(child.id).unwrap());
        assert_eq!(class, task_ops::derive::FailureClass::Infra);
    }
    let units = store.work_units_for(root_id).unwrap();
    let c = unit(&units, "c");
    assert_eq!(c.status, task_core::WorkUnitStatus::Blocked);
    assert_eq!(
        c.blocked_reason,
        Some(task_core::WorkUnitBlockedReason::Infra)
    );
    assert_eq!(c.child_task_id, Some(children[1].id.to_string()));
    // 兄弟は止まらない。
    assert_eq!(unit(&units, "a").status, task_core::WorkUnitStatus::Done);
    assert_eq!(unit(&units, "s").status, task_core::WorkUnitStatus::Done);
    assert_eq!(
        children_titled(&store, root_id, "s")[0].status,
        Status::Done
    );
    // 段階は完了しない。root は ready のまま（replan も質問もしない）。
    assert_ne!(
        unit(&units, "integrate-s1").status,
        task_core::WorkUnitStatus::Done
    );
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Ready);
    assert_eq!(
        planner_runs(&store, root_id),
        1,
        "no replan for an infra failure"
    );
    assert_eq!(store.execution_plan_list(root_id).unwrap().len(), 1);
    assert!(
        store.decisions_list(Some(root_id)).unwrap().is_empty(),
        "not a decision"
    );
    no_questions(&store, root_id);
    let events = store.events_for(root_id).unwrap();
    let reasons = super::tree::unit_reasons(&events, "c");
    assert_eq!(
        reasons,
        vec!["child_created", "child_infra_retry", "child_infra_failed"],
        "{events:?}"
    );
    // 障害通知 1 件（基盤の分類、質問ではない）。
    let notes: Vec<task_core::Notification> = store
        .notification_recent(50)
        .unwrap()
        .into_iter()
        .filter(|n| n.key.starts_with("tree-infra:"))
        .collect();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!(notes[0].kind, NotificationKind::TaskFailed);
    assert!(notes[0].body.contains("infra"), "{}", notes[0].body);
    assert!(notes[0].body.contains("Child c"), "{}", notes[0].body);
    assert_replay_is_clean(&store);
}

/// ADR-0079 §7 R2b / D9: 子の失敗を吸収する replan は木の上限（`max_tree_replans`）に数え、超えるなら planner を
/// 起こさず `limit:max_tree_replans` の決定の要求を 1 件出す。節点の上限（`[execution] max_replans`）に先に当たった
/// ときも黙って止まらず `limit:max_replans` の決定の要求（節点ごとに 1 件）を出す。
#[tokio::test]
async fn child_replans_are_bounded_by_tree_and_node_limits() {
    for (name, tree_replans, node_replans, key) in [
        ("tree", 1u32, 5u32, "limit:max_tree_replans"),
        ("node", 10, 1, "limit:max_replans"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
        let root = compound_task(dir.path());
        let root_id = root.id;
        store.create_task(&root, vec![]).unwrap();
        let failing = v3_plan(
            vec![stage("s1", false)],
            vec![task_unit("c", "s1", &[], "false")],
        );
        let adapter = Arc::new(ReplanAdapter::new(vec![
            failing.clone(),
            failing.clone(),
            failing,
        ]));
        let mut d = tree_dispatcher(&store, adapter.clone());
        d.config.execution.limits.tree.max_tree_replans = tree_replans;
        d.config.execution.max_replans = node_replans;
        run_until_quiet(&mut d, &store, 2000).await;
        for _ in 0..20 {
            d.tick().unwrap();
        }
        assert_eq!(planner_runs(&store, root_id), 2, "{name}: one replan only");
        assert_eq!(
            store.execution_plan_list(root_id).unwrap().len(),
            2,
            "{name}"
        );
        let children = children_titled(&store, root_id, "c");
        assert_eq!(children.len(), 2, "{name}: {children:?}");
        assert!(
            children.iter().all(|c| c.status == Status::Failed),
            "{name}"
        );
        let decisions = decisions_of(&store, root_id);
        assert_eq!(decisions.len(), 1, "{name}: {decisions:?}");
        assert_eq!(decisions[0].key, key, "{name}");
        assert_eq!(decisions[0].kind, task_core::DecisionKind::Limit, "{name}");
        assert_eq!(decisions[0].task_id, root_id, "{name}");
        assert_eq!(
            store.get(root_id).unwrap().unwrap().status,
            Status::Ready,
            "{name}"
        );
        no_questions(&store, root_id);
        assert_replay_is_clean(&store);
    }
}
