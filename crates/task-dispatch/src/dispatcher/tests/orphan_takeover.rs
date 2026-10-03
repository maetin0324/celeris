//! Phase F5-fix6: 再起動直後に、居なくなったデーモンが抱えていた run（孤児）を lease の失効を待たずに
//! 回収する。本番 2026-09-28（task 01M3MCA20JSAKGENH571NZES1F、run 01M3MDZF188TRHFZ9554F7H7KN）:
//! `systemctl --user restart` で止まったデーモンは SIGTERM で子を止め（exit 143）、`result.json` =
//! `{"type":"error","message":"worker exited without a result message (exit=143)","retryable":true}` を
//! 残したが、`worker_finished` を書かずに自分の `daemon_instances` の行を消して exit した。新しいデーモンは
//! lease（17:20:03Z まで）が切れるまで 15 分何もしなかった。

use super::*;
use crate::orphan::{ORPHAN_TAKEOVER_REASON, OrphanTakeover};

pub(super) const SELF_ID: &str = "01NEWDAEMON0000000000000000";
/// 本番の result.json（SIGTERM で止められた claude-code の run）そのまま。
const SIGTERM_RESULT: &str = r#"{"type":"error","message":"worker exited without a result message (exit=143)","retryable":true}"#;

pub(super) fn instance(id: &str, role: InstanceRole, pid: u32) -> DaemonInstance {
    let now = OffsetDateTime::now_utc();
    DaemonInstance {
        instance_id: id.into(),
        release: "1bb7fd6473a4".into(),
        pid,
        role,
        started_at: now,
        heartbeat_at: now,
        handoff_requested_at: None,
        drained_at: None,
    }
}

/// 新しい active（自分の行を登録し、孤児の回収を有効にした dispatcher）。
pub(super) fn new_active(
    store: &Arc<dyn TaskStore>,
    adapter: Arc<dyn WorkerAdapter>,
) -> Dispatcher {
    store
        .instance_register(&instance(SELF_ID, InstanceRole::Active, std::process::id()))
        .unwrap();
    let mut d = dispatcher(store.clone(), adapter, 1);
    d.set_orphan_takeover(OrphanTakeover {
        instance_id: SELF_ID.into(),
        freshness: Duration::from_secs(60),
        pid_alive: Arc::new(crate::orphan::proc_pid_alive),
    });
    d
}

/// 旧デーモンが run を走らせている最中の DB（Task は Running、lease はまだ 14 分残っている、
/// `WorkerStarted` と `runs` の行あり、`worker_finished` 無し）。`result` があれば
/// `runs/<run_id>/result.json` に書く。`plan` なら v1 の計画の WU `a` の run にする。
pub(super) fn running_orphan(
    store: &Arc<dyn TaskStore>,
    dir: &std::path::Path,
    result: Option<&str>,
    plan: bool,
) -> (TaskId, String) {
    let mut task = new_task(
        dir,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        2,
    );
    task.status = Status::Running;
    let run_id = ulid::Ulid::new().to_string();
    task.lease = Some(task_core::Lease {
        worker_run_id: run_id.clone(),
        expires_at: OffsetDateTime::now_utc() + Duration::from_secs(14 * 60),
    });
    let task_id = task.id;
    store.insert(&task).unwrap();
    store
        .append_event(
            task_id,
            &Event::Created {
                task: Box::new(task.clone()),
                origin: None,
            },
        )
        .unwrap();
    let mut work_unit_id = None;
    if plan {
        adopt_three_step_plan(store, task_id);
        let a = store
            .work_units_for(task_id)
            .unwrap()
            .into_iter()
            .find(|u| u.key == "a")
            .unwrap();
        let mut running_a = a.clone();
        running_a.status = task_core::WorkUnitStatus::Running;
        running_a.runs = 1;
        running_a.last_run_id = Some(run_id.clone());
        store
            .work_unit_transition(
                task_id,
                running_a,
                Event::WorkUnitTransitioned {
                    work_unit_id: a.id.clone(),
                    key: a.key.clone(),
                    from: task_core::WorkUnitStatus::Ready,
                    to: task_core::WorkUnitStatus::Running,
                    reason: "dispatch".into(),
                    run_id: Some(run_id.clone()),
                },
            )
            .unwrap();
        work_unit_id = Some(a.id);
    }
    store
        .append_event(
            task_id,
            &Event::WorkerStarted {
                run_id: run_id.clone(),
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
        .run_index_start(task_core::RunRow {
            run_id: run_id.clone(),
            task_id: task_id.to_string(),
            work_unit_id,
            role: task_core::RunIndexRole::Worker,
            seq: 1,
            status: task_core::RunIndexStatus::Running,
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
    if let Some(text) = result {
        let run_dir = dir.join("runs").join(&run_id);
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(run_dir.join("result.json"), text).unwrap();
    }
    (task_id, run_id)
}

fn events(store: &Arc<dyn TaskStore>, id: TaskId) -> Vec<Event> {
    store
        .events_for(id)
        .unwrap()
        .into_iter()
        .map(|(_, e)| e)
        .collect()
}

fn finished_of(
    store: &Arc<dyn TaskStore>,
    id: TaskId,
    run_id: &str,
) -> Vec<(String, Option<RunEnd>)> {
    events(store, id)
        .into_iter()
        .filter_map(|e| match e {
            Event::WorkerFinished {
                run_id: r,
                outcome,
                end,
                ..
            } if r == run_id => Some((outcome, end)),
            _ => None,
        })
        .collect()
}

fn took_over(store: &Arc<dyn TaskStore>, id: TaskId, run_id: &str) -> bool {
    events(store, id).iter().any(|e| {
        matches!(e, Event::WorkerProgress { run_id: r, msg, .. }
            if r == run_id && msg.starts_with(ORPHAN_TAKEOVER_REASON))
    })
}

/// 本番と同じ形なので、ここで作るアダプタは再 dispatch された run を長く走らせる（テストの
/// 観察の邪魔をしない）。
pub(super) fn slow_adapter() -> Arc<dyn WorkerAdapter> {
    Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "s".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::from_secs(30),
    })
}

/// (a) 旧デーモンの行が無く（SIGTERM で自分の行を消して exit）、result.json が error（exit=143、
/// retryable）→ 最初の tick で、lease を待たずに `on_worker_finished` と同じ通常の再試行に乗る。
#[tokio::test]
async fn an_orphaned_run_with_an_error_result_is_retried_without_waiting_for_the_lease() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (task_id, run_id) = running_orphan(&store, dir.path(), Some(SIGTERM_RESULT), false);
    let mut d = new_active(&store, slow_adapter());
    d.tick().unwrap();

    assert!(
        took_over(&store, task_id, &run_id),
        "{:?}",
        events(&store, task_id)
    );
    let finished = finished_of(&store, task_id, &run_id);
    assert_eq!(finished.len(), 1, "{finished:?}");
    assert!(
        finished[0].0.contains("exit=143") && !finished[0].0.contains("lease expired"),
        "{finished:?}"
    );
    // 通常の worker_error（retryable）の再試行: Running → Ready（attempts を 1 消費）。
    assert!(
        events(&store, task_id).iter().any(|e| matches!(
            e,
            Event::Transitioned { from: Status::Running, to: Status::Ready, reason }
                if reason == Trigger::WorkerError { retryable: true }.name()
        )),
        "{:?}",
        events(&store, task_id)
    );
    let row = store.run_index_get(&run_id).unwrap().unwrap();
    assert_ne!(row.status, task_core::RunIndexStatus::Running, "{row:?}");
    assert!(row.finished_at.is_some());
    d.abort_all_runs();
}

/// (b) 同じく旧デーモンが居ない、result.json が done → その内容で確定（検査 `true` を走らせて done）。
#[tokio::test]
async fn an_orphaned_run_with_a_done_result_is_finalised() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let done = serde_json::to_string(&WorkerMessage::Done {
        summary: "会話形式の表示を実装した".into(),
        evidence: vec![],
        usage: None,
    })
    .unwrap();
    let (task_id, run_id) = running_orphan(&store, dir.path(), Some(&done), false);
    let mut d = new_active(&store, slow_adapter());
    let s = store.clone();
    assert!(
        run_until(&mut d, 300, || {
            s.get(task_id).unwrap().unwrap().status == Status::Done
        })
        .await,
        "{:?}",
        events(&store, task_id)
    );
    assert!(took_over(&store, task_id, &run_id));
    let finished = finished_of(&store, task_id, &run_id);
    assert_eq!(finished.len(), 1, "{finished:?}");
    assert!(finished[0].0.starts_with("done: "), "{finished:?}");
    // 新しい run は起きていない（やり直しではなく確定）。
    let started = events(&store, task_id)
        .iter()
        .filter(|e| matches!(e, Event::WorkerStarted { .. }))
        .count();
    assert_eq!(started, 1);
}

/// (c) ライブ切替（ADR-0040 D4）: draining の旧デーモンが heartbeat を打っている間は、その run を
/// 横取りしない（lease がまだ有効なので何もしない）。draining 側（新しい仕事を受けない）も他の
/// インスタンスの run を回収しない。孤児の回収の設定が無い dispatcher も従来どおり。
#[tokio::test]
async fn a_run_held_by_a_live_draining_instance_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (task_id, run_id) = running_orphan(&store, dir.path(), Some(SIGTERM_RESULT), false);
    // 旧デーモン: draining、heartbeat は新しく、pid は生きている（このテストのプロセス）。
    store
        .instance_register(&instance(
            "01OLDDAEMON0000000000000000",
            InstanceRole::Draining,
            std::process::id(),
        ))
        .unwrap();
    let mut d = new_active(&store, slow_adapter());
    for _ in 0..3 {
        d.tick().unwrap();
    }
    let t = store.get(task_id).unwrap().unwrap();
    assert_eq!(t.status, Status::Running);
    assert_eq!(t.lease.unwrap().worker_run_id, run_id);
    assert!(finished_of(&store, task_id, &run_id).is_empty());
    assert!(!took_over(&store, task_id, &run_id));

    // 旧の行が消えても、draining 中の dispatcher（accepting_new_work = false）は回収しない。
    store
        .instance_delete("01OLDDAEMON0000000000000000")
        .unwrap();
    d.set_accepting_new_work(false);
    d.tick().unwrap();
    assert!(finished_of(&store, task_id, &run_id).is_empty());
    // 孤児の回収の設定が無い dispatcher（verify・テスト・従来の組み立て）も lease を待つ。
    let mut plain = dispatcher(store.clone(), slow_adapter(), 1);
    plain.tick().unwrap();
    assert!(finished_of(&store, task_id, &run_id).is_empty());
    assert_eq!(store.get(task_id).unwrap().unwrap().status, Status::Running);

    // 旧の pid が死んでいる（heartbeat はまだ新しい。SIGKILL 直後）なら、持ち主は居ない。
    store
        .instance_register(&instance(
            "01OLDDAEMON0000000000000000",
            InstanceRole::Draining,
            u32::MAX - 1,
        ))
        .unwrap();
    d.set_accepting_new_work(true);
    d.tick().unwrap();
    assert!(took_over(&store, task_id, &run_id));
    assert_eq!(finished_of(&store, task_id, &run_id).len(), 1);
    d.abort_all_runs();
}

/// (d) result.json が無く、プロセスも居ない（旧デーモンの行が無い）→ ハーネス側の中断
/// （`interrupted: orphan_takeover …`、`harness_error(infra)`）として attempts を消費せず requeue し、
/// WU は ready に戻る（reason `orphan_takeover`。checkpoint があれば needs_continuation）。
#[tokio::test]
async fn an_orphaned_run_without_a_result_is_requeued_as_interrupted() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (task_id, run_id) = running_orphan(&store, dir.path(), None, true);
    let adapter = Arc::new(WuScriptAdapter::new(HashMap::new()));
    let mut d = new_active(&store, adapter);
    d.tick().unwrap();

    assert!(took_over(&store, task_id, &run_id));
    let finished = finished_of(&store, task_id, &run_id);
    assert_eq!(finished.len(), 1, "{finished:?}");
    assert!(
        finished[0].0.starts_with("interrupted: orphan_takeover"),
        "{finished:?}"
    );
    assert_eq!(
        finished[0].1,
        Some(RunEnd::HarnessError {
            class: HarnessErrorClass::Infra
        })
    );
    let evs = events(&store, task_id);
    assert!(
        evs.iter().any(|e| matches!(
            e,
            Event::Transitioned { from: Status::Running, to: Status::Ready, reason }
                if reason == Trigger::InfraRequeue.name()
        )),
        "{evs:?}"
    );
    assert!(
        evs.iter().any(|e| matches!(
            e,
            Event::WorkUnitTransitioned { key, to: WorkUnitStatus::Ready, reason, run_id: Some(r), .. }
                if key == "a" && reason == ORPHAN_TAKEOVER_REASON && r == &run_id
        )),
        "{evs:?}"
    );
    let row = store.run_index_get(&run_id).unwrap().unwrap();
    assert_eq!(
        row.status,
        task_core::RunIndexStatus::HarnessError,
        "{row:?}"
    );
    // 孤児（人の再起動）はバックオフしない。
    assert!(!d.infra_backoff.contains_key(&task_id));
    assert_eq!(store.get(task_id).unwrap().unwrap().attempts, 0);
}

/// F5-fix6 (2): SIGTERM で止まるデーモンは、手元の run を止めて `interrupted: daemon shutdown …`
/// （`cancelled`）を残し、Task を InfraRequeue で ready に戻してから exit する（新しいデーモンは
/// lease を待たずにすぐ dispatch できる）。
#[tokio::test]
async fn a_shutdown_records_the_interrupted_runs_before_exiting() {
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
    let mut d = new_active(&store, slow_adapter());
    d.tick().unwrap();
    let run_id = d
        .running
        .values()
        .next()
        .expect("dispatched")
        .run_id
        .clone();
    assert_eq!(store.get(task_id).unwrap().unwrap().status, Status::Running);

    assert_eq!(d.interrupt_runs_on_shutdown(), 1);
    assert!(d.running.is_empty());
    let finished = finished_of(&store, task_id, &run_id);
    assert_eq!(finished.len(), 1, "{finished:?}");
    assert!(
        finished[0].0.starts_with("interrupted: daemon shutdown"),
        "{finished:?}"
    );
    assert_eq!(finished[0].1, Some(RunEnd::Cancelled));
    let t = store.get(task_id).unwrap().unwrap();
    assert_eq!(t.status, Status::Ready);
    assert_eq!(t.attempts, 0);
    assert!(t.lease.is_none());
    // 2 回呼んでも二重に書かない。
    assert_eq!(d.interrupt_runs_on_shutdown(), 0);
}

/// v2（工程の lease）の WU の run も同じ: 旧デーモンが WU の run を走らせたまま消えた（行も無い）→
/// 新しい active は WU の lease を待たずにその run を閉じ（`interrupted: orphan_takeover`）、WU を
/// reason `orphan_takeover` で戻す（result.json が無いので確定はできない）。
#[tokio::test]
async fn an_orphaned_parallel_work_unit_run_is_reconciled_without_waiting_for_its_lease() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let flags = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    adopt_gate_plan(&store, task.id, &flags.path().join("second"), "0.1");
    let mut old = parallel_dispatcher(store.clone(), slow_adapter(), root.path(), 2, 2, 2);
    let s = store.clone();
    let id = task.id;
    assert!(
        run_until(&mut old, 300, || gate_row(&s, id).status
            == task_core::WorkUnitStatus::Running)
        .await
    );
    let run_id = gate_row(&store, task.id).lease_run_id.expect("wu lease");
    // 旧デーモンが消える（完了は届かない。行は SIGTERM の exit で消えた）。
    old.abort_all_runs();
    drop(old);
    let holder = store.get(task.id).unwrap().unwrap().lease.unwrap();
    assert!(is_phase_lease_holder(&holder.worker_run_id));
    assert!(holder.expires_at > OffsetDateTime::now_utc());

    store
        .instance_register(&instance(SELF_ID, InstanceRole::Active, std::process::id()))
        .unwrap();
    let mut active = parallel_dispatcher(store.clone(), slow_adapter(), root.path(), 2, 2, 2);
    active.set_orphan_takeover(OrphanTakeover {
        instance_id: SELF_ID.into(),
        freshness: Duration::from_secs(60),
        pid_alive: Arc::new(crate::orphan::proc_pid_alive),
    });
    active.tick().unwrap();
    assert!(took_over(&store, task.id, &run_id));
    let finished = finished_of(&store, task.id, &run_id);
    assert_eq!(finished.len(), 1, "{finished:?}");
    assert!(
        finished[0].0.starts_with("interrupted: orphan_takeover"),
        "{finished:?}"
    );
    assert!(
        events(&store, task.id).iter().any(|e| matches!(
            e,
            Event::WorkUnitTransitioned { key, from: WorkUnitStatus::Running, reason, run_id: Some(r), .. }
                if key == "gate" && reason == ORPHAN_TAKEOVER_REASON && r == &run_id
        )),
        "{:?}",
        events(&store, task.id)
    );
    let row = store.run_index_get(&run_id).unwrap().unwrap();
    assert_eq!(
        row.status,
        task_core::RunIndexStatus::HarnessError,
        "{row:?}"
    );
    active.abort_all_runs();
}
