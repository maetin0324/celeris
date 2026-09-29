//! ADR-0074「F5-fix8 実装時の明確化」: 仕事の残っていない計画（WU がすべて done / 有効な WU が 1 つも無い）の
//! `ready` の Task は、その版がまだ最終レビューを受けていなければ次の tick で最終レビューに進む
//! （`Trigger::PlanComplete`）。本番の事故（2026-09-29、12 done の v1 の後の review_fail → 空の replan の v4 が
//! 採用され、`max_replans` を使い切っていたので `ready` のまま黙って止まった）の再現と、木でない Task の
//! 生存確認（`StallDetected` + 障害通知 `stall:<id>:<seq>`）。すべて偽のアダプタと一時ディレクトリだけで、外部
//! ネットワークに出ない。

use super::tree::assert_replay_is_clean;
use super::*;

/// planner run には空の差分（`add`/`modify`/`remove` がすべて空）を書き、`flag` を作る（2 回目の最終レビューの
/// 検査 `test -f flag` が通るようになる = 「指摘はもう解消している」planner の主張を模す）。それ以外の run は
/// `Done`。起きた run の種類を記録する。
struct EmptyReplanAdapter {
    flag: std::path::PathBuf,
    base_version: u32,
    planner_runs: AtomicUsize,
    worker_runs: AtomicUsize,
}

impl EmptyReplanAdapter {
    fn new(flag: std::path::PathBuf, base_version: u32) -> Self {
        EmptyReplanAdapter {
            flag,
            base_version,
            planner_runs: AtomicUsize::new(0),
            worker_runs: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl WorkerAdapter for EmptyReplanAdapter {
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
            self.planner_runs.fetch_add(1, Ordering::SeqCst);
            let delta = serde_json::json!({
                "schema": task_core::execution_plan::EXECUTION_PLAN_DELTA_SCHEMA,
                "base_version": self.base_version,
                "rationale": "done の WU だけで上限いっぱい。指摘はもう解消しているので WU は足さない",
                "add": [],
                "modify": [],
                "remove": []
            });
            std::fs::write(
                req.artifacts_dir.join("execution-plan.json"),
                delta.to_string(),
            )
            .expect("write execution-plan.json");
            std::fs::write(&self.flag, "fixed").expect("write flag");
        } else {
            self.worker_runs.fetch_add(1, Ordering::SeqCst);
        }
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: "done".into(),
                evidence: vec![],
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

fn reasons(store: &Arc<dyn TaskStore>, id: TaskId) -> Vec<String> {
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

fn stalls(store: &Arc<dyn TaskStore>, id: TaskId) -> Vec<String> {
    store
        .events_for(id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::StallDetected { reason, .. } => Some(reason),
            _ => None,
        })
        .collect()
}

/// 有効な WU をすべて `to`（done / cancelled）にする（統合 WU を含む）。
fn settle_all_units(store: &Arc<dyn TaskStore>, id: TaskId, to: task_core::WorkUnitStatus) {
    for u in store.work_units_for(id).unwrap() {
        if !u.status.is_active() || u.status == to {
            continue;
        }
        let mut row = u.clone();
        row.status = to;
        store
            .work_unit_transition(
                id,
                row,
                Event::WorkUnitTransitioned {
                    work_unit_id: u.id.clone(),
                    key: u.key.clone(),
                    from: u.status,
                    to,
                    reason: "test_settled".into(),
                    run_id: None,
                },
            )
            .unwrap();
    }
}

/// 事故の再現（F5-fix8 (3) の 1 本目）: v1 の WU（a → b → c）がすべて done → 最終レビューが不合格（`review_fail`）
/// → replan の planner が空の差分を出し（`added=0, changed=0, removed=0`）、v2 が採用される。`max_replans = 1` なので
/// 採用の時点で replan を使い切っている（本番の v4 と同じ）。修正前はここで `ready` のまま何も起きなかった
/// （`AllDone → replan_gate → Skip`）。修正後は数 tick のうちに `plan_complete` で最終レビューに進み、
/// 2 回目の審査が通って `done` になる。WU の run は 3 本だけ（done の WU をやり直さない）、planner は 1 本だけ。
#[tokio::test]
async fn empty_replan_after_review_fail_goes_to_final_review() {
    let dir = tempfile::tempdir().unwrap();
    let flag = dir.path().join("fixed.flag");
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(
        dir.path(),
        Check::Command {
            cmd: format!("test -f {}", flag.display()),
            expect_exit: 0,
        },
        2,
    );
    let task_id = task.id;
    store.insert(&task).unwrap();
    adopt_three_step_plan(&store, task_id);

    let adapter = Arc::new(EmptyReplanAdapter::new(flag.clone(), 1));
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.config.execution.planner.adapter = "instant".to_string();
    d.config.execution.max_replans = 1;
    d.config.retry_backoff_base = Duration::ZERO;
    d.config.retry_backoff_max = Duration::ZERO;
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(task_id).unwrap().unwrap();
    let reasons = reasons(&store, task_id);
    assert_eq!(stored.status, Status::Done, "{stored:?} {reasons:?}");
    assert_eq!(adapter.planner_runs.load(Ordering::SeqCst), 1);
    assert_eq!(adapter.worker_runs.load(Ordering::SeqCst), 3);
    let plans = store.execution_plan_list(task_id).unwrap();
    assert_eq!(plans.len(), 2, "{plans:?}");
    let v2_reason = store
        .events_for(task_id)
        .unwrap()
        .into_iter()
        .find_map(|(_, e)| match e {
            Event::ExecutionPlanned {
                version: 2, reason, ..
            } => reason,
            _ => None,
        })
        .expect("v2 adopted");
    assert!(
        v2_reason.contains("added=0, changed=0, removed=0"),
        "{v2_reason}"
    );
    // 順序: 1 回目の審査の不合格 → replan（planned）→ plan_complete → 2 回目の審査の合格。
    let fail = reasons.iter().position(|r| r == "review_fail").unwrap();
    let planned = reasons.iter().rposition(|r| r == "planned").unwrap();
    let complete = reasons.iter().position(|r| r == "plan_complete").unwrap();
    let pass = reasons.iter().position(|r| r == "review_pass").unwrap();
    assert!(
        fail < planned && planned < complete && complete < pass,
        "{reasons:?}"
    );
    assert_eq!(
        reasons.iter().filter(|r| *r == "plan_complete").count(),
        1,
        "{reasons:?}"
    );
    assert!(stalls(&store, task_id).is_empty());
    assert_replay_is_clean(&store);
}

/// F5-fix8 (3) の 2 本目: 採用の時点で WU がすべて done の /2 の計画（統合 WU も done）と、有効な WU が 1 つも
/// 無い計画（すべて cancelled）は、どちらも run を 1 本も起こさずに `plan_complete` → 最終レビュー → `done`。
#[tokio::test]
async fn a_plan_with_no_work_left_proceeds_to_review_without_a_run() {
    for cancelled in [false, true] {
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
        let task_id = task.id;
        store.insert(&task).unwrap();
        let mut a = wu_spec("a", &[]);
        a.phase = Some("build".into());
        let mut b = wu_spec("b", &["a"]);
        b.phase = Some("build".into());
        adopt_v2_plan(&store, task_id, &["build"], vec![a, b]);
        let to = if cancelled {
            task_core::WorkUnitStatus::Cancelled
        } else {
            task_core::WorkUnitStatus::Done
        };
        settle_all_units(&store, task_id, to);
        assert!(task_core::plan_work_finished(
            &store.work_units_for(task_id).unwrap()
        ));

        let adapter = Arc::new(EmptyReplanAdapter::new(dir.path().join("unused"), 1));
        let mut d = dispatcher(store.clone(), adapter.clone(), 1);
        d.config.execution.planner.adapter = "instant".to_string();
        let report = run_until_idle(&mut d, 50).await;
        assert!(report.idle, "{report:?}");

        let stored = store.get(task_id).unwrap().unwrap();
        let reasons = reasons(&store, task_id);
        assert_eq!(
            stored.status,
            Status::Done,
            "cancelled={cancelled} {stored:?} {reasons:?}"
        );
        assert!(
            reasons.iter().any(|r| r == "plan_complete"),
            "cancelled={cancelled} {reasons:?}"
        );
        assert_eq!(adapter.worker_runs.load(Ordering::SeqCst), 0);
        assert_eq!(adapter.planner_runs.load(Ordering::SeqCst), 0);
        assert_eq!(store.execution_plan_list(task_id).unwrap().len(), 1);
        assert_replay_is_clean(&store);
    }
}

/// F5-fix8 (3) の 3 本目（(d)）: 木でない Task（木は無効）の、仕事の残っていない計画が最終レビューで不合格になり、
/// `max_replans` も使い切っている（dispatcher は何もできない）。修正前は何も出なかった。修正後は生存確認が
/// `liveness_timeout_secs`（600 秒、偽の時計）を過ぎた最初の確認で `StallDetected{reason: replans_exhausted}` と
/// 障害通知（`TaskFailed`、key `stall:<id>:<seq>`）を 1 回だけ出す。run は 1 本も起きない。
#[tokio::test]
async fn a_stalled_non_tree_plan_is_detected() {
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
    adopt_three_step_plan(&store, task_id);
    settle_all_units(&store, task_id, task_core::WorkUnitStatus::Done);
    for trigger in [Trigger::Dispatch, Trigger::WorkerDone, Trigger::ReviewFail] {
        store.apply_transition(task_id, trigger, None).unwrap();
    }
    assert_eq!(store.get(task_id).unwrap().unwrap().status, Status::Ready);

    let adapter = Arc::new(EmptyReplanAdapter::new(dir.path().join("unused"), 1));
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.config.execution.planner.adapter = "instant".to_string();
    d.config.execution.max_replans = 0;
    assert!(!d.config.execution.limits.tree.enabled);
    let t0 = OffsetDateTime::now_utc() + time::Duration::minutes(5);
    let clock = Arc::new(StdMutex::new(t0));
    d.test_now = Some(clock.clone());
    d.tick().unwrap();
    *clock.lock().unwrap() = t0 + time::Duration::seconds(599);
    d.tick().unwrap();
    assert!(stalls(&store, task_id).is_empty(), "not before 600 s");
    *clock.lock().unwrap() = t0 + time::Duration::seconds(630);
    d.tick().unwrap();
    assert_eq!(
        stalls(&store, task_id),
        vec!["replans_exhausted".to_string()]
    );
    for secs in [700, 1300, 5000] {
        *clock.lock().unwrap() = t0 + time::Duration::seconds(secs);
        d.tick().unwrap();
    }
    assert_eq!(stalls(&store, task_id).len(), 1, "only once");
    let notes: Vec<_> = store
        .notification_pending()
        .unwrap()
        .into_iter()
        .filter(|n| n.key.starts_with(&format!("stall:{task_id}:")))
        .collect();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!(notes[0].kind, NotificationKind::TaskFailed);
    assert!(notes[0].body.contains("障害（stall）"), "{}", notes[0].body);
    assert!(notes[0].body.contains("F5-fix8"), "{}", notes[0].body);
    assert_eq!(adapter.worker_runs.load(Ordering::SeqCst), 0);
    assert_eq!(adapter.planner_runs.load(Ordering::SeqCst), 0);
    assert_eq!(store.get(task_id).unwrap().unwrap().status, Status::Ready);
}

/// 生存確認は、木でない Task でも名指しの待ち・走れる状態を検出しない: 走れる WU を持つ計画（`ready` の WU）と、
/// 仕事の残っていない未審査の計画（次の tick で最終レビュー）は、何秒たっても `StallDetected` にならない。
#[test]
fn liveness_does_not_flag_runnable_non_tree_plans() {
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
    store.insert(&task).unwrap();
    adopt_three_step_plan(&store, task.id);
    let facts = task_ops::tree::node_liveness_facts(store.as_ref(), &task, true, false).unwrap();
    let v = task_core::tree::liveness(&task_core::TreeSnapshot { nodes: vec![facts] });
    assert_eq!(v[0].class, task_core::LivenessClass::Runnable, "{v:?}");
    settle_all_units(&store, task.id, task_core::WorkUnitStatus::Done);
    let facts = task_ops::tree::node_liveness_facts(store.as_ref(), &task, true, false).unwrap();
    assert!(!facts.plan_reviewed);
    let v = task_core::tree::liveness(&task_core::TreeSnapshot { nodes: vec![facts] });
    assert_eq!(
        (v[0].class, v[0].reason.as_str()),
        (task_core::LivenessClass::Runnable, "completion"),
        "{v:?}"
    );
}
