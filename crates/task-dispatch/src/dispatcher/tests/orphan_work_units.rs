//! run の終端と WU の回収の間のクラッシュ、および comment / pause の順序を固定する。
use super::orphan_takeover::{SELF_ID, instance, new_active};
use super::*;

struct WaitingWorker(Arc<tokio::sync::Notify>);

#[async_trait]
impl WorkerAdapter for WaitingWorker {
    fn id(&self) -> &str {
        "instant"
    }

    async fn run(
        &self,
        _req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        self.0.notify_one();
        std::future::pending().await
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    store: Arc<dyn TaskStore>,
    task: TaskId,
    run: String,
    started: Arc<tokio::sync::Notify>,
}

impl Fixture {
    fn new(v2: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn TaskStore> =
            Arc::new(SqliteStore::open(&dir.path().join("test.db")).unwrap());
        let task = new_task(dir.path(), Check::Reviewer, 2);
        store.insert(&task).unwrap();
        if v2 {
            adopt_v2_plan(&store, task.id, &["build"], vec![v2_wu("a", "build", &[])]);
        } else {
            adopt_three_step_plan(&store, task.id);
        }
        let run = ulid::Ulid::new().to_string();
        let holder = if v2 { "phase:test:build" } else { &run };
        assert!(
            store
                .acquire_lease(task.id, holder, Duration::from_secs(3600))
                .unwrap()
        );
        let mut wu = store
            .work_units_for(task.id)
            .unwrap()
            .into_iter()
            .find(|u| u.key == "a")
            .unwrap();
        wu.status = WorkUnitStatus::Running;
        wu.last_run_id = Some(run.clone());
        wu.runs = 1;
        if v2 {
            wu.lease_run_id = Some(run.clone());
            wu.lease_expires_at = Some(rfc3339(
                OffsetDateTime::now_utc() + Duration::from_secs(3600),
            ));
        }
        store
            .work_units_apply(task.id, vec![], vec![wu.clone()], vec![])
            .unwrap();
        store
            .append_event(
                task.id,
                &Event::WorkerStarted {
                    run_id: run.clone(),
                    adapter: "instant".into(),
                    model: "m".into(),
                    provider: Some("p1".into()),
                    account: None,
                    role: None,
                    task_role: None,
                },
            )
            .unwrap();
        store
            .run_index_start(RunRow {
                run_id: run.clone(),
                task_id: task.id.to_string(),
                work_unit_id: Some(wu.id),
                role: RunIndexRole::Worker,
                seq: 1,
                status: RunIndexStatus::Running,
                adapter: Some("instant".into()),
                model: Some("m".into()),
                account: None,
                session_id: None,
                checkpoint: None,
                usage: None,
                metrics: None,
                started_at: rfc3339(OffsetDateTime::now_utc()),
                finished_at: None,
            })
            .unwrap();
        Self {
            dir,
            store,
            task: task.id,
            run,
            started: Arc::new(tokio::sync::Notify::new()),
        }
    }

    fn active(&self) -> Dispatcher {
        new_active(&self.store, Arc::new(WaitingWorker(self.started.clone())))
    }

    fn unit(&self) -> WorkUnitRow {
        self.store
            .work_units_for(self.task)
            .unwrap()
            .into_iter()
            .find(|u| u.key == "a")
            .unwrap()
    }

    fn interrupt(&self) {
        self.store
            .apply_transition(self.task, Trigger::Interrupt, None)
            .unwrap();
        assert!(self.store.get(self.task).unwrap().unwrap().lease.is_none());
    }

    fn finish_only(&self) {
        // 旧版、または WorkerFinished 記録直後のクラッシュ（WU を更新する前）。
        self.store
            .append_event(
                self.task,
                &Event::WorkerFinished {
                    run_id: self.run.clone(),
                    outcome: "interrupted: orphan_takeover".into(),
                    usage: None,
                    role: None,
                    metrics: None,
                    end: Some(RunEnd::Cancelled),
                },
            )
            .unwrap();
    }

    async fn assert_redispatched(&self, d: &mut Dispatcher) {
        d.tick().unwrap();
        tokio::time::timeout(Duration::from_secs(10), self.started.notified())
            .await
            .expect("worker started");
        let wu = self.unit();
        assert_eq!(wu.status, WorkUnitStatus::Running);
        assert_ne!(wu.last_run_id.as_ref(), Some(&self.run));
        assert_eq!(wu.runs, 2);
        assert_eq!(self.store.get(self.task).unwrap().unwrap().attempts, 0);
        d.crash_for_test();
    }
}

#[tokio::test]
async fn comment_then_orphan_takeover_recovers_v1_and_phase_work_units() {
    for v2 in [false, true] {
        let f = Fixture::new(v2);
        f.interrupt();
        let mut d = f.active();
        assert!(matches!(
            d.wu_dispatch_gate(f.task).unwrap(),
            WuDispatchGate::Skip
        ));
        d.reconcile_ownerless_runs();
        assert_eq!(f.unit().status, WorkUnitStatus::Ready);
        assert!(f.unit().lease_run_id.is_none());
        assert_eq!(
            f.store.run_index_get(&f.run).unwrap().unwrap().status,
            RunIndexStatus::Cancelled
        );
        let before = f.store.events_for(f.task).unwrap().len();
        d.close_aborted_run(f.task, &f.run, None, crate::orphan::OWNERLESS_GONE_WHY);
        assert_eq!(
            f.store.events_for(f.task).unwrap().len(),
            before,
            "idempotent"
        );
        f.assert_redispatched(&mut d).await;
    }
}

#[tokio::test]
async fn restart_recovers_finished_missing_and_unassigned_runs() {
    for (loss, interrupted) in [
        ("finished", true),
        ("missing", true),
        ("no_id", true),
        ("finished", false),
        ("missing", false),
        ("no_id", false),
    ] {
        let mut f = Fixture::new(true);
        if interrupted {
            f.interrupt();
        }
        if loss == "finished" {
            f.finish_only();
        } else {
            // 一時 DB で、WU を開始したが run 索引を保存する前に落ちた状態を作る。
            let conn = rusqlite::Connection::open(f.dir.path().join("test.db")).unwrap();
            conn.execute("DELETE FROM runs WHERE run_id = ?1", [&f.run])
                .unwrap();
            if loss == "no_id" {
                let mut wu = f.unit();
                wu.last_run_id = None;
                wu.clear_lease();
                f.store
                    .work_units_apply(f.task, vec![], vec![wu], vec![])
                    .unwrap();
            }
        }
        f.store = Arc::new(SqliteStore::open(&f.dir.path().join("test.db")).unwrap());
        let mut d = f.active();
        d.reconcile_ownerless_runs();
        assert_eq!(f.unit().status, WorkUnitStatus::Ready, "{loss}");
        f.assert_redispatched(&mut d).await;
    }
}

#[tokio::test]
async fn pause_resume_before_and_after_orphan_recovery_preserves_pause_and_redispatches() {
    for order in ["pause_first", "resume_first", "recovery_first"] {
        let f = Fixture::new(true);
        let mut d = f.active();
        let now = OffsetDateTime::now_utc();
        if order == "pause_first" {
            task_ops::lifecycle::pause_task(f.store.as_ref(), f.task, now).unwrap();
        }
        f.interrupt();
        if order != "pause_first" {
            task_ops::lifecycle::pause_task(f.store.as_ref(), f.task, now).unwrap();
        }
        if order == "resume_first" {
            task_ops::lifecycle::resume_task(f.store.as_ref(), f.task, now).unwrap();
        }
        d.reconcile_ownerless_runs();
        assert_eq!(f.unit().status, WorkUnitStatus::Ready);
        if order != "resume_first" {
            d.tick().unwrap();
            assert!(
                d.running.is_empty(),
                "recovery must not resume a paused task"
            );
            task_ops::lifecycle::resume_task(f.store.as_ref(), f.task, now).unwrap();
        }
        f.assert_redispatched(&mut d).await;
    }
}

#[tokio::test]
async fn restart_recovery_preserves_other_daemons_scope_and_child_units() {
    for protection in [
        "active",
        "draining",
        "self_draining",
        "disabled",
        "filter",
        "child",
    ] {
        let f = Fixture::new(true);
        f.interrupt();
        f.finish_only();
        let mut d = f.active();
        match protection {
            "active" | "draining" => {
                let role = if protection == "active" {
                    InstanceRole::Active
                } else {
                    InstanceRole::Draining
                };
                f.store
                    .instance_register(&instance("old", role, std::process::id()))
                    .unwrap();
            }
            "self_draining" => d.accepting_new_work = false,
            "disabled" => d.orphan_takeover = None,
            "filter" => d.eligible = Some(Arc::new(|_| false)),
            "child" => {
                let mut wu = f.unit();
                wu.kind = WorkUnitKind::Task;
                // kind は通常更新不可なので、fixture の一時 DB だけで設定する。
                let conn = rusqlite::Connection::open(f.dir.path().join("test.db")).unwrap();
                conn.execute(
                    "UPDATE work_units SET kind = 'task' WHERE id = ?1",
                    [&wu.id],
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        d.reconcile_ownerless_runs();
        assert_eq!(f.unit().status, WorkUnitStatus::Running, "{protection}");
        assert_eq!(
            f.store
                .instance_list()
                .unwrap()
                .iter()
                .filter(|i| i.instance_id == SELF_ID)
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn restart_with_checkpoint_preserves_continuation_and_counters() {
    let f = Fixture::new(true);
    f.interrupt();
    let checkpoint: Checkpoint = serde_json::from_value(serde_json::json!({
        "schema": "celeris.checkpoint/1", "task_id": f.task.to_string(),
        "work_unit": f.unit().id, "run_id": f.run, "run_seq": 1,
        "end": "yielded", "source": "worker", "next_action": "continue",
        "created_at": rfc3339(OffsetDateTime::now_utc())
    }))
    .unwrap();
    f.store
        .run_index_finish(
            &f.run,
            RunIndexStatus::Cancelled,
            Some(checkpoint),
            None,
            None,
            OffsetDateTime::now_utc(),
        )
        .unwrap();
    let before = f.unit();
    let mut d = f.active();
    d.reconcile_ownerless_runs();
    let after = f.unit();
    assert_eq!(after.status, WorkUnitStatus::NeedsContinuation);
    assert_eq!(
        (after.runs, after.retries, after.continuations),
        (before.runs, before.retries, before.continuations)
    );
    assert_eq!(after.last_run_id, before.last_run_id);
    f.assert_redispatched(&mut d).await;
}

#[test]
fn stale_recovery_cannot_overwrite_a_new_run_replan_or_task_cancel() {
    for change in ["new_run", "replan", "cancel"] {
        let f = Fixture::new(true);
        let old = f.unit();
        match change {
            "new_run" | "replan" => {
                let mut new = old.clone();
                if change == "new_run" {
                    new.last_run_id = Some("new-run".into());
                    new.lease_run_id = Some("new-run".into());
                } else {
                    new.status = WorkUnitStatus::Cancelled;
                }
                f.store
                    .work_units_apply(f.task, vec![], vec![new], vec![])
                    .unwrap();
            }
            "cancel" => {
                f.store
                    .apply_transition(f.task, Trigger::Cancel, None)
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let before = f.unit();
        let events = f.store.events_for(f.task).unwrap().len();
        assert!(
            !f.store
                .recover_work_unit(f.task, &old, "restart_reconcile")
                .unwrap()
        );
        assert_eq!(f.unit(), before, "{change}");
        assert_eq!(f.store.events_for(f.task).unwrap().len(), events);
    }
}

#[tokio::test]
async fn local_run_is_not_recovered_and_task_cancel_does_not_requeue() {
    let f = Fixture::new(true);
    f.interrupt();
    let mut d = f.active();
    d.tick().unwrap();
    tokio::time::timeout(Duration::from_secs(10), f.started.notified())
        .await
        .unwrap();
    let live = f.unit();
    // 次の周期を待たず照合を呼ぶ。手元の run を terminal 索引だけから回収しない。
    f.store
        .run_index_finish(
            live.last_run_id.as_ref().unwrap(),
            RunIndexStatus::Completed,
            None,
            None,
            None,
            OffsetDateTime::now_utc(),
        )
        .unwrap();
    d.ownerless_reconciled_at = None;
    d.reconcile_ownerless_runs();
    assert_eq!(f.unit(), live);
    f.store
        .apply_transition(f.task, Trigger::Cancel, None)
        .unwrap();
    d.tick().unwrap();
    assert_eq!(f.unit().status, WorkUnitStatus::Cancelled);
    assert!(d.running.is_empty());
}

#[tokio::test]
async fn periodic_recovery_waits_for_check_owner_and_uses_the_injected_clock() {
    let f = Fixture::new(true);
    f.interrupt();
    f.finish_only();
    let mut d = f.active();
    let now = OffsetDateTime::now_utc();
    d.test_now = Some(Arc::new(std::sync::Mutex::new(now)));
    let wu = f.unit();
    d.checking.insert(
        f.run.clone(),
        CheckingEntry {
            task_id: f.task,
            work_unit_id: wu.id,
            key: wu.key,
            checks: 1,
            handle: tokio::spawn(std::future::pending()),
            terminal: Terminal::Done {
                summary: "ok".into(),
                evidence: vec![],
                usage: None,
            },
            account: None,
            account_adapter: None,
            provider: ProviderId::from("p1"),
        },
    );
    d.reconcile_ownerless_runs();
    assert_eq!(
        f.unit().status,
        WorkUnitStatus::Running,
        "a terminal run can still have live checks"
    );
    d.checking.remove(&f.run).unwrap().handle.abort();
    *d.test_now.as_ref().unwrap().lock().unwrap() =
        now + time::Duration::seconds(RUNS_RECONCILE_INTERVAL_SECS - 1);
    d.reconcile_ownerless_runs();
    assert_eq!(f.unit().status, WorkUnitStatus::Running, "not due yet");
    *d.test_now.as_ref().unwrap().lock().unwrap() += time::Duration::seconds(1);
    d.reconcile_ownerless_runs();
    assert_eq!(f.unit().status, WorkUnitStatus::Ready);
    f.assert_redispatched(&mut d).await;
}

#[tokio::test]
async fn interrupt_or_cancel_during_checks_recovers_the_work_unit_immediately() {
    for cancel in [false, true] {
        let f = Fixture::new(true);
        let mut d = f.active();
        let wu = f.unit();
        f.store
            .run_index_finish(
                &f.run,
                RunIndexStatus::Completed,
                None,
                None,
                None,
                OffsetDateTime::now_utc(),
            )
            .unwrap();
        d.checking.insert(
            f.run.clone(),
            CheckingEntry {
                task_id: f.task,
                work_unit_id: wu.id,
                key: wu.key,
                checks: 1,
                handle: tokio::spawn(std::future::pending()),
                terminal: Terminal::Done {
                    summary: "ok".into(),
                    evidence: vec![],
                    usage: None,
                },
                account: None,
                account_adapter: None,
                provider: "p1".into(),
            },
        );
        f.store
            .apply_transition(
                f.task,
                if cancel {
                    Trigger::Cancel
                } else {
                    Trigger::Interrupt
                },
                None,
            )
            .unwrap();
        d.abort_stale_runs().unwrap();
        assert!(d.checking.is_empty());
        assert!(d.just_aborted.contains(&f.task));
        assert_eq!(
            f.store.run_index_get(&f.run).unwrap().unwrap().status,
            RunIndexStatus::Completed
        );
        if cancel {
            assert_eq!(f.unit().status, WorkUnitStatus::Cancelled);
        } else {
            assert_eq!(f.unit().status, WorkUnitStatus::Ready);
            f.assert_redispatched(&mut d).await;
        }
    }
}
