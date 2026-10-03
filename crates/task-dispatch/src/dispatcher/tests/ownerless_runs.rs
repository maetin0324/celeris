//! 持ち主の居ない `running` の `runs` 行（lease を持たない reviewer run 等）。本番 2026-10（reviewer run
//! 01M4129NX8R264QXMQVEZRDKHM）: `WorkerStarted{role: reviewer}` の直後に `Transitioned{reviewing → ready,
//! review_fail}`（「not evaluated: a deterministic check failed」）が続き、reviewer run の `WorkerFinished` が
//! 無いまま `runs` 行が `running` で残った。取り残しの経路は 3 つ:
//!
//! - A: 決定的な検査が落ちて reviewer を起こさなかった（`on_review_finished` が閉じない）。
//! - B: SIGTERM / SIGINT の停止（`interrupt_runs_on_shutdown`）がレビューを閉じない。
//! - C: クラッシュ等。lease を持たない run は誰も照合しない。
//!
//! 同じ instance の中でも起きる（本番 2026-10-03 17:57 の reviewer run 01M41EKM2QGD4T65F8ZDS4RFF7 は
//! 引き継ぎの後に経路 A で残った）。回収は引き継ぎに限らない: 経路 A は `on_review_finished`、手元の
//! レビューの取りこぼし（tokio task が判定を届けずに終わった・期限越え）は毎 tick の `reap_lost_reviews`、
//! 手元に無い行は起動時と定期の `reconcile_ownerless_runs`。

use super::super::ownerless_runs::{LOST_REVIEW_WHY, STUCK_REVIEW_WHY};
use super::orphan_takeover::{SELF_ID, instance, running_orphan, slow_adapter};
use super::*;
use crate::orphan::{OWNERLESS_DEADLINE_WHY, OWNERLESS_GONE_WHY, OrphanTakeover};
use std::sync::atomic::AtomicBool;

const OLD_ID: &str = "01OLDDAEMON0000000000000000";
/// 旧デーモンの pid（実在しない値。生死は `pid_alive` の差し替えで決める）。
const OLD_PID: u32 = u32::MAX - 7;
const PASS_REVIEW: &str = r#"{"verdicts":[{"criterion":0,"pass":true,"reason":"ok"}]}"#;

/// reviewer run を起こしたことを知らせ、そのまま返らない（止められるまで走り続ける reviewer）。
/// worker の run はすぐ done を返す。
struct BlockingReviewer {
    started: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl WorkerAdapter for BlockingReviewer {
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
        if req.task.kind == TaskKind::Review {
            self.started.notify_one();
            return std::future::pending().await;
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

fn pass_reviewer() -> Arc<dyn WorkerAdapter> {
    Arc::new(FileAdapter {
        plan_json: String::new(),
        review_json: PASS_REVIEW.into(),
        delay: Duration::ZERO,
    })
}

/// 孤児の回収を有効にした active（自分の行を登録する）。`alive` が pid の生死を決める。
fn active(
    store: &Arc<dyn TaskStore>,
    adapter: Arc<dyn WorkerAdapter>,
    test_clock: bool,
    alive: Arc<dyn Fn(u32) -> bool + Send + Sync>,
) -> Dispatcher {
    store
        .instance_register(&instance(SELF_ID, InstanceRole::Active, std::process::id()))
        .unwrap();
    let mut d = dispatcher_with_test_clock(store.clone(), adapter, 1, test_clock);
    d.set_orphan_takeover(OrphanTakeover {
        instance_id: SELF_ID.into(),
        freshness: Duration::from_secs(60),
        pid_alive: alive,
    });
    d
}

fn all_alive() -> Arc<dyn Fn(u32) -> bool + Send + Sync> {
    Arc::new(|_| true)
}

fn clock_now(d: &Dispatcher) -> OffsetDateTime {
    *d.test_now.as_ref().expect("test clock").lock().unwrap()
}

fn advance(d: &Dispatcher, secs: i64) {
    let clock = d.test_now.as_ref().expect("test clock");
    let mut now = clock.lock().unwrap();
    *now += time::Duration::seconds(secs);
}

fn reviewer_task(dir: &std::path::Path, status: Status) -> Task {
    let mut task = new_task(dir, Check::Reviewer, 2);
    task.status = status;
    task
}

fn insert_with_created(store: &Arc<dyn TaskStore>, task: &Task) {
    store.insert(task).unwrap();
    store
        .append_event(
            task.id,
            &Event::Created {
                task: Box::new(task.clone()),
                origin: None,
            },
        )
        .unwrap();
}

/// 持ち主の居ない reviewer run（`WorkerStarted{role: reviewer}` と `running` の `runs` 行だけ）を足す。
fn seed_reviewer_run(
    store: &Arc<dyn TaskStore>,
    task_id: TaskId,
    started_at: OffsetDateTime,
) -> String {
    let run_id = ulid::Ulid::new().to_string();
    store
        .append_event(
            task_id,
            &Event::WorkerStarted {
                run_id: run_id.clone(),
                adapter: "instant".into(),
                model: "m".into(),
                provider: Some("p1".into()),
                account: None,
                role: Some(RunRole::Reviewer),
                task_role: None,
            },
        )
        .unwrap();
    store
        .run_index_start(task_core::RunRow {
            run_id: run_id.clone(),
            task_id: task_id.to_string(),
            work_unit_id: None,
            role: task_core::RunIndexRole::Reviewer,
            seq: 1,
            status: task_core::RunIndexStatus::Running,
            adapter: Some("instant".into()),
            model: Some("m".into()),
            account: None,
            session_id: None,
            checkpoint: None,
            usage: None,
            metrics: None,
            started_at: rfc3339(started_at),
            finished_at: None,
        })
        .unwrap();
    run_id
}

fn row_status(store: &Arc<dyn TaskStore>, run_id: &str) -> task_core::RunIndexStatus {
    store
        .run_index_get(run_id)
        .unwrap()
        .expect("runs row")
        .status
}

/// その run の `WorkerFinished`（outcome, role, end）。
fn finished(
    store: &Arc<dyn TaskStore>,
    task_id: TaskId,
    run_id: &str,
) -> Vec<(String, Option<RunRole>, Option<RunEnd>)> {
    store
        .events_for(task_id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::WorkerFinished {
                run_id: r,
                outcome,
                role,
                end,
                ..
            } if r == run_id => Some((outcome, role, end)),
            _ => None,
        })
        .collect()
}

fn reviewer_starts(store: &Arc<dyn TaskStore>, task_id: TaskId) -> Vec<String> {
    store
        .events_for(task_id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::WorkerStarted {
                run_id,
                role: Some(RunRole::Reviewer),
                ..
            } => Some(run_id),
            _ => None,
        })
        .collect()
}

fn running_rows(store: &Arc<dyn TaskStore>) -> Vec<String> {
    store
        .runs_running()
        .unwrap()
        .into_iter()
        .map(|r| r.run_id)
        .collect()
}

/// 旧デーモン（`daemon_instances` に行を持つ）が reviewing の task のレビューを始め、reviewer run が
/// 走っている（返らない）状態まで進める。旧の reviewer run の id を返す。
async fn old_daemon_reviewing(store: &Arc<dyn TaskStore>, task: &Task) -> (Dispatcher, String) {
    store
        .instance_register(&instance(OLD_ID, InstanceRole::Active, OLD_PID))
        .unwrap();
    let started = Arc::new(tokio::sync::Notify::new());
    let mut old = dispatcher(
        store.clone(),
        Arc::new(BlockingReviewer {
            started: started.clone(),
        }),
        1,
    );
    old.config.review_timeout = Duration::from_secs(600);
    old.tick().unwrap();
    assert!(old.reviewing.contains_key(&task.id));
    tokio::time::timeout(Duration::from_secs(30), started.notified())
        .await
        .expect("the old daemon's reviewer run started");
    let review_run = old
        .reviewing
        .get(&task.id)
        .and_then(|e| e.review_run_id.clone())
        .expect("a reviewer run was provisioned");
    assert_eq!(
        row_status(store, &review_run),
        task_core::RunIndexStatus::Running
    );
    // ライブ切替: 旧は draining になり、新しい仕事を受けない（手元のレビューは面倒を見続ける）。
    old.set_accepting_new_work(false);
    store
        .instance_set_role(OLD_ID, InstanceRole::Draining, OffsetDateTime::now_utc())
        .unwrap();
    (old, review_run)
}

/// T0a（経路 B）: ライブ切替で draining の旧が reviewer run を走らせている間、新しい active は横取りも
/// 閉じもしない。旧が SIGTERM で止まる（`interrupt_runs_on_shutdown`）と、その reviewer run は
/// `cancelled` で閉じ、新しい active がレビューをやり直して done にする。`running` の行は残らない。
#[tokio::test]
async fn handoff_shutdown_closes_the_old_reviewer_run_and_the_new_active_rereviews() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = reviewer_task(dir.path(), Status::Reviewing);
    insert_with_created(&store, &task);
    let (mut old, old_review) = old_daemon_reviewing(&store, &task).await;

    let old_alive = Arc::new(AtomicBool::new(true));
    let alive = old_alive.clone();
    let mut new = active(
        &store,
        pass_reviewer(),
        false,
        Arc::new(move |pid| pid != OLD_PID || alive.load(Ordering::SeqCst)),
    );
    for _ in 0..3 {
        new.tick().unwrap();
        tokio::task::yield_now().await;
    }
    assert!(
        new.reviewing.is_empty(),
        "the review lock is still held by the old daemon"
    );
    assert_eq!(
        row_status(&store, &old_review),
        task_core::RunIndexStatus::Running
    );
    assert!(finished(&store, task.id, &old_review).is_empty());
    assert_eq!(
        store.get(task.id).unwrap().unwrap().status,
        Status::Reviewing
    );

    // 旧が SIGTERM で止まる（`celeris` は停止の直前に `interrupt_runs_on_shutdown` を呼び、自分の行を消す）。
    old.interrupt_runs_on_shutdown();
    assert_eq!(
        row_status(&store, &old_review),
        task_core::RunIndexStatus::Cancelled,
        "the old reviewer run is still running after the shutdown"
    );
    assert!(old.reviewing.is_empty());
    drop(old);
    store.instance_delete(OLD_ID).unwrap();
    old_alive.store(false, Ordering::SeqCst);
    let closed = finished(&store, task.id, &old_review);
    assert_eq!(closed.len(), 1, "{closed:?}");
    assert_eq!(closed[0].1, Some(RunRole::Reviewer));
    assert_eq!(closed[0].2, Some(RunEnd::Cancelled));
    assert!(closed[0].0.contains("daemon shutdown"), "{closed:?}");
    assert_eq!(
        store.get(task.id).unwrap().unwrap().status,
        Status::Reviewing
    );

    let s = store.clone();
    let id = task.id;
    assert!(
        run_until_state(&mut new, || s.get(id).unwrap().unwrap().status
            == Status::Done)
        .await,
        "{:?}",
        store.events_for(task.id).unwrap()
    );
    let reviewers = reviewer_starts(&store, task.id);
    assert_eq!(reviewers.len(), 2, "{reviewers:?}");
    assert_eq!(
        row_status(&store, &reviewers[1]),
        task_core::RunIndexStatus::Completed
    );
    assert!(
        running_rows(&store).is_empty(),
        "{:?}",
        store.runs_running()
    );
}

/// T0b（経路 C）: 旧デーモンが停止のフックを通らずに消える（クラッシュ）。新しい active の最初の照合が
/// 旧の reviewer run を `interrupted: orphan_takeover …` で閉じ、`recover_reviews` がやり直して done。
#[tokio::test]
async fn a_crashed_old_daemons_reviewer_run_is_closed_and_the_task_rereviewed() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = reviewer_task(dir.path(), Status::Reviewing);
    insert_with_created(&store, &task);
    let (mut old, old_review) = old_daemon_reviewing(&store, &task).await;

    // クラッシュ: DB には何も書かない。旧の行は残る（heartbeat もまだ新しい）が、pid は死んでいる。
    old.crash_for_test();
    drop(old);
    let mut new = active(
        &store,
        pass_reviewer(),
        false,
        Arc::new(|pid| pid != OLD_PID),
    );
    new.tick().unwrap();
    assert_eq!(
        row_status(&store, &old_review),
        task_core::RunIndexStatus::Cancelled,
        "the crashed daemon's reviewer run is still running after the new active's first tick"
    );
    let closed = finished(&store, task.id, &old_review);
    assert_eq!(closed.len(), 1, "{closed:?}");
    assert_eq!(closed[0].0, format!("interrupted: {OWNERLESS_GONE_WHY}"));
    assert_eq!(closed[0].1, Some(RunRole::Reviewer));
    assert_eq!(closed[0].2, Some(RunEnd::Cancelled));

    let s = store.clone();
    let id = task.id;
    assert!(
        run_until_state(&mut new, || s.get(id).unwrap().unwrap().status
            == Status::Done)
        .await,
        "{:?}",
        store.events_for(task.id).unwrap()
    );
    assert_eq!(reviewer_starts(&store, task.id).len(), 2);
    assert!(
        running_rows(&store).is_empty(),
        "{:?}",
        store.runs_running()
    );
}

/// T0c（経路 A、本番の event 列）: 決定的な検査（`false`）が落ちたので reviewer を起こさなかった。
/// 用意した reviewer run の行は `cancelled` で閉じ、`WorkerFinished{role: reviewer, end: cancelled}` が残る。
/// Task は通常どおり `review_fail` で ready に戻る。
#[tokio::test]
async fn a_reviewer_run_not_launched_because_a_check_failed_is_closed() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = new_task(
        dir.path(),
        Check::Command {
            cmd: "false".into(),
            expect_exit: 0,
        },
        2,
    );
    task.acceptance.push(Criterion {
        text: "looks good".into(),
        check: Check::Reviewer,
    });
    task.status = Status::Reviewing;
    insert_with_created(&store, &task);
    // 孤児の回収の設定は無い（経路 A は on_review_finished だけで閉じる）。
    let mut d = dispatcher(store.clone(), slow_adapter(), 1);
    d.tick().unwrap();
    let review_run = d
        .reviewing
        .get(&task.id)
        .and_then(|e| e.review_run_id.clone())
        .expect("a reviewer run was provisioned");
    (&mut d.reviewing.get_mut(&task.id).unwrap().handle)
        .await
        .unwrap();
    assert_eq!(d.tick().unwrap().reviewed, 1);

    let events: Vec<Event> = store
        .events_for(task.id)
        .unwrap()
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    assert!(
        events.iter().any(
            |e| matches!(e, Event::ReviewVerdict { pass: false, reason, .. }
            if reason == "not evaluated: a deterministic check failed")
        ),
        "{events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(e, Event::Transitioned { from: Status::Reviewing, to: Status::Ready, reason }
            if reason == "review_fail")),
        "{events:?}"
    );
    assert_eq!(
        row_status(&store, &review_run),
        task_core::RunIndexStatus::Cancelled,
        "the provisioned reviewer run is still running: {events:?}"
    );
    let closed = finished(&store, task.id, &review_run);
    assert_eq!(closed.len(), 1, "{closed:?}");
    assert_eq!(closed[0].1, Some(RunRole::Reviewer));
    assert_eq!(closed[0].2, Some(RunEnd::Cancelled));
    d.abort_all_runs();
}

/// T1a（起動時）: DB に reviewing の task と持ち主の居ない reviewer run の行がある。他に生きている
/// インスタンスは無い。新しい active の最初の tick がその行を閉じ、レビューをやり直して done。
#[tokio::test]
async fn startup_closes_an_ownerless_reviewer_run_and_rereviews() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = reviewer_task(dir.path(), Status::Reviewing);
    insert_with_created(&store, &task);
    let stale = seed_reviewer_run(&store, task.id, OffsetDateTime::now_utc());
    let mut d = active(&store, pass_reviewer(), false, all_alive());
    d.tick().unwrap();
    assert_eq!(
        row_status(&store, &stale),
        task_core::RunIndexStatus::Cancelled,
        "the ownerless reviewer run is still running after the first tick"
    );
    assert_eq!(
        finished(&store, task.id, &stale)[0].0,
        format!("interrupted: {OWNERLESS_GONE_WHY}")
    );
    let s = store.clone();
    let id = task.id;
    assert!(
        run_until_state(&mut d, || s.get(id).unwrap().unwrap().status
            == Status::Done)
        .await,
        "{:?}",
        store.events_for(task.id).unwrap()
    );
    assert!(
        running_rows(&store).is_empty(),
        "{:?}",
        store.runs_running()
    );
}

/// T1b（定期、時計の差し替え）: 最初の照合の後に現れた行は、`RUNS_RECONCILE_INTERVAL_SECS` が経つまで
/// 残り、経った後の tick で閉じる。
#[tokio::test]
async fn ownerless_runs_are_reconciled_again_after_the_interval() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut d = active(&store, slow_adapter(), true, all_alive());
    d.tick().unwrap();
    // 人の回答待ち（blocked）の task: dispatcher は他に何もしない。
    let task = reviewer_task(dir.path(), Status::Blocked);
    insert_with_created(&store, &task);
    let stale = seed_reviewer_run(&store, task.id, clock_now(&d));
    advance(&d, RUNS_RECONCILE_INTERVAL_SECS - 1);
    d.tick().unwrap();
    assert_eq!(
        row_status(&store, &stale),
        task_core::RunIndexStatus::Running
    );
    advance(&d, 2);
    d.tick().unwrap();
    assert_eq!(
        row_status(&store, &stale),
        task_core::RunIndexStatus::Cancelled,
        "the ownerless run is still running after the reconcile interval"
    );
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Blocked);
}

/// T1c（他に生きているインスタンスがある）: draining の他のインスタンスが生きている間は、持ち主の居ない
/// reviewer run の行を期限（`started_at + review_timeout + lease_grace`）まで残し、過ぎたら閉じる。
#[tokio::test]
async fn an_ownerless_run_is_kept_while_another_instance_lives_until_its_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut d = active(&store, slow_adapter(), true, all_alive());
    d.config.review_timeout = Duration::from_secs(1000);
    let t0 = clock_now(&d);
    store
        .instance_register(&instance(OLD_ID, InstanceRole::Draining, OLD_PID))
        .unwrap();
    store.instance_heartbeat(OLD_ID, t0).unwrap();
    let task = reviewer_task(dir.path(), Status::Blocked);
    insert_with_created(&store, &task);
    let stale = seed_reviewer_run(&store, task.id, t0);
    // 期限 = started_at + ownerless_ttl（照合の間隔 600 秒より十分長い）。
    let ttl = d.ownerless_ttl(task_core::RunIndexRole::Reviewer, &task);
    let ttl_secs = i64::try_from(ttl.as_secs()).unwrap();
    assert!(ttl_secs > RUNS_RECONCILE_INTERVAL_SECS + 100);
    d.tick().unwrap();
    assert_eq!(
        row_status(&store, &stale),
        task_core::RunIndexStatus::Running
    );
    // 期限の 10 秒前の照合ではまだ閉じない。
    advance(&d, ttl_secs - 10);
    store.instance_heartbeat(OLD_ID, clock_now(&d)).unwrap();
    d.tick().unwrap();
    assert_eq!(
        row_status(&store, &stale),
        task_core::RunIndexStatus::Running
    );
    advance(&d, RUNS_RECONCILE_INTERVAL_SECS + 1);
    store.instance_heartbeat(OLD_ID, clock_now(&d)).unwrap();
    d.tick().unwrap();
    assert_eq!(
        row_status(&store, &stale),
        task_core::RunIndexStatus::Cancelled,
        "the ownerless run outlived its deadline but is still running"
    );
    assert_eq!(
        finished(&store, task.id, &stale)[0].0,
        format!("interrupted: {OWNERLESS_DEADLINE_WHY}")
    );
}

/// T1d（worker run）: 居なくなったデーモンの run が Task の lease を持っている → 従来の lease の経路
/// （`InfraRequeue`）で ready に戻り、その run の行は閉じる。`running` の行は手元の run のものだけ。
#[tokio::test]
async fn a_leased_worker_run_of_a_gone_daemon_is_requeued_by_the_lease_path() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (task_id, run_id) = running_orphan(&store, dir.path(), None, false);
    let mut d = active(&store, slow_adapter(), false, all_alive());
    d.tick().unwrap();
    assert!(
        store
            .events_for(task_id)
            .unwrap()
            .iter()
            .any(|(_, e)| matches!(
                e,
                Event::Transitioned { from: Status::Running, to: Status::Ready, reason }
                    if reason == Trigger::InfraRequeue.name()
            )),
        "{:?}",
        store.events_for(task_id).unwrap()
    );
    assert_ne!(
        row_status(&store, &run_id),
        task_core::RunIndexStatus::Running
    );
    // 同じ tick で再 dispatch された run（手元にある）以外に `running` の行は無い。
    let in_hand: Vec<String> = d.running.values().map(|e| e.run_id.clone()).collect();
    for r in running_rows(&store) {
        assert!(in_hand.contains(&r), "{r} is running but not in hand");
    }
    d.abort_all_runs();
}

/// T1e（閉じないもの）: 手元のレビューの reviewer run、生きている他のインスタンスの lease の下の run、
/// 面倒を見ない task の run、孤児の回収の設定が無い dispatcher。
#[tokio::test]
async fn runs_in_hand_under_a_live_lease_or_of_ineligible_tasks_are_not_closed() {
    // (1) 手元のレビュー（期限も照合の間隔も過ぎる。他に生きているインスタンスは無い）。
    {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
        let task = reviewer_task(dir.path(), Status::Reviewing);
        insert_with_created(&store, &task);
        let started = Arc::new(tokio::sync::Notify::new());
        let mut d = active(
            &store,
            Arc::new(BlockingReviewer {
                started: started.clone(),
            }),
            true,
            all_alive(),
        );
        d.config.review_timeout = Duration::from_secs(600);
        d.tick().unwrap();
        tokio::time::timeout(Duration::from_secs(30), started.notified())
            .await
            .expect("reviewer run started");
        let review_run = d.reviewing[&task.id].review_run_id.clone().unwrap();
        for _ in 0..3 {
            advance(&d, 2 * RUNS_RECONCILE_INTERVAL_SECS);
            d.tick().unwrap();
        }
        assert_eq!(
            row_status(&store, &review_run),
            task_core::RunIndexStatus::Running
        );
        assert!(finished(&store, task.id, &review_run).is_empty());
        d.abort_all_runs();
    }
    // (2) 生きている draining の他のインスタンスの lease の下の worker run（期限は過ぎている）。
    {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
        let (task_id, run_id) = running_orphan(&store, dir.path(), None, false);
        store
            .instance_register(&instance(OLD_ID, InstanceRole::Draining, OLD_PID))
            .unwrap();
        let mut d = active(&store, slow_adapter(), true, all_alive());
        d.tick().unwrap();
        advance(&d, 2 * RUNS_RECONCILE_INTERVAL_SECS);
        store.instance_heartbeat(OLD_ID, clock_now(&d)).unwrap();
        d.tick().unwrap();
        assert_eq!(
            row_status(&store, &run_id),
            task_core::RunIndexStatus::Running
        );
        assert!(finished(&store, task_id, &run_id).is_empty());
        assert_eq!(store.get(task_id).unwrap().unwrap().status, Status::Running);
    }
    // (3) 面倒を見ない task、(4) 孤児の回収の設定が無い dispatcher（他に生きているインスタンスは無い）。
    {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
        let task = reviewer_task(dir.path(), Status::Blocked);
        insert_with_created(&store, &task);
        let stale = seed_reviewer_run(&store, task.id, OffsetDateTime::now_utc());
        let mut d = active(&store, slow_adapter(), false, all_alive());
        let id = task.id;
        d.set_eligible_tasks(Arc::new(move |t: &Task| t.id != id));
        d.tick().unwrap();
        assert_eq!(
            row_status(&store, &stale),
            task_core::RunIndexStatus::Running
        );
        let mut plain = dispatcher(store.clone(), slow_adapter(), 1);
        plain.tick().unwrap();
        assert_eq!(
            row_status(&store, &stale),
            task_core::RunIndexStatus::Running
        );
        assert!(finished(&store, task.id, &stale).is_empty());
    }
}

/// T1e 続き: 新しい仕事を受けていない（draining / standby）間は照合しない。active になった最初の tick で
/// 照合する（受けていない間の tick は照合の時刻を記録しない）。
#[tokio::test]
async fn a_dispatcher_not_accepting_new_work_does_not_reconcile_until_it_becomes_active() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = reviewer_task(dir.path(), Status::Blocked);
    insert_with_created(&store, &task);
    let stale = seed_reviewer_run(&store, task.id, OffsetDateTime::now_utc());
    let mut d = active(&store, slow_adapter(), true, all_alive());
    d.set_accepting_new_work(false);
    d.tick().unwrap();
    d.tick().unwrap();
    assert_eq!(
        row_status(&store, &stale),
        task_core::RunIndexStatus::Running
    );
    d.set_accepting_new_work(true);
    d.tick().unwrap();
    assert_eq!(
        row_status(&store, &stale),
        task_core::RunIndexStatus::Cancelled,
        "the first tick after becoming active did not reconcile"
    );
}

/// 同じ instance の中でレビューを始め、reviewer run が走っている（返らない）状態まで進める。
/// 他の instance は無い。reviewer run の id を返す。
async fn self_reviewing(
    store: &Arc<dyn TaskStore>,
    task: &Task,
    test_clock: bool,
) -> (Dispatcher, String) {
    let started = Arc::new(tokio::sync::Notify::new());
    let mut d = active(
        store,
        Arc::new(BlockingReviewer {
            started: started.clone(),
        }),
        test_clock,
        all_alive(),
    );
    d.config.review_timeout = Duration::from_secs(600);
    d.tick().unwrap();
    tokio::time::timeout(Duration::from_secs(30), started.notified())
        .await
        .expect("the reviewer run started");
    let review_run = d
        .reviewing
        .get(&task.id)
        .and_then(|e| e.review_run_id.clone())
        .expect("a reviewer run was provisioned");
    assert_eq!(
        row_status(store, &review_run),
        task_core::RunIndexStatus::Running
    );
    (d, review_run)
}

/// T2a（同じ instance、完了の取りこぼし）: レビューの tokio task が判定を届けずに終わった（abort / panic）。
/// 1 tick 目は様子を見るだけ、2 tick 目で reviewer run を `cancelled` で閉じ、同じ tick でレビューを
/// やり直す（新しい reviewer run が起き、古い行は running で残らない）。
#[tokio::test]
async fn a_review_that_ended_without_a_verdict_is_closed_and_redone_in_the_same_instance() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = reviewer_task(dir.path(), Status::Reviewing);
    insert_with_created(&store, &task);
    let (mut d, lost_run) = self_reviewing(&store, &task, false).await;

    // 判定を届けずに終わる tokio task（abort で模す。完了は送られない）。entry は手元に残ったまま。
    let handle = &mut d.reviewing.get_mut(&task.id).unwrap().handle;
    handle.abort();
    assert!(
        (&mut *handle).await.is_err(),
        "the aborted review task ended"
    );
    assert!(handle.is_finished());

    d.tick().unwrap();
    assert!(
        d.reviewing.contains_key(&task.id),
        "the first tick only watches the finished review"
    );
    assert_eq!(
        row_status(&store, &lost_run),
        task_core::RunIndexStatus::Running
    );
    d.tick().unwrap();
    assert_eq!(
        row_status(&store, &lost_run),
        task_core::RunIndexStatus::Cancelled,
        "the lost reviewer run is still running: {:?}",
        store.events_for(task.id).unwrap()
    );
    let closed = finished(&store, task.id, &lost_run);
    assert_eq!(closed.len(), 1, "{closed:?}");
    assert_eq!(closed[0].0, format!("interrupted: {LOST_REVIEW_WHY}"));
    assert_eq!(closed[0].1, Some(RunRole::Reviewer));
    assert_eq!(closed[0].2, Some(RunEnd::Cancelled));
    assert_eq!(
        store.get(task.id).unwrap().unwrap().status,
        Status::Reviewing
    );
    let starts = reviewer_starts(&store, task.id);
    assert_eq!(starts.len(), 2, "the review was not redone: {starts:?}");
    assert_ne!(starts[1], lost_run);
    assert_eq!(running_rows(&store), vec![starts[1].clone()]);
    assert!(d.lost_review_watch.is_empty());
    d.abort_all_runs();
}

/// T2b（同じ instance、期限越え。レビューの `since` の差し替え）: 走り続けるレビューは期限
/// （`since + ownerless_ttl`）までは触らず、過ぎた tick で止めて reviewer run を閉じ、レビューをやり直す。
/// 期限は `since` と同じ実時計で測るので、`since` を過去へずらして決定的に再現する（待たない）。
#[tokio::test]
async fn a_review_that_outlives_its_deadline_is_stopped_and_redone_in_the_same_instance() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = reviewer_task(dir.path(), Status::Reviewing);
    insert_with_created(&store, &task);
    let (mut d, stuck_run) = self_reviewing(&store, &task, false).await;
    let ttl = d.ownerless_ttl(task_core::RunIndexRole::Reviewer, &task);
    // 期限の 1 時間手前（ttl は review_timeout 600 s × 9 + lease_grace なので余裕がある）。
    d.reviewing.get_mut(&task.id).unwrap().since =
        OffsetDateTime::now_utc() - ttl + time::Duration::hours(1);
    d.tick().unwrap();
    assert_eq!(
        row_status(&store, &stuck_run),
        task_core::RunIndexStatus::Running,
        "a review inside its deadline was touched"
    );
    d.reviewing.get_mut(&task.id).unwrap().since =
        OffsetDateTime::now_utc() - ttl - time::Duration::seconds(1);
    d.tick().unwrap();
    assert_eq!(
        row_status(&store, &stuck_run),
        task_core::RunIndexStatus::Cancelled,
        "the stuck reviewer run is still running: {:?}",
        store.events_for(task.id).unwrap()
    );
    let closed = finished(&store, task.id, &stuck_run);
    assert_eq!(closed.len(), 1, "{closed:?}");
    assert_eq!(closed[0].0, format!("interrupted: {STUCK_REVIEW_WHY}"));
    assert_eq!(closed[0].2, Some(RunEnd::Cancelled));
    assert_eq!(
        store.get(task.id).unwrap().unwrap().status,
        Status::Reviewing
    );
    let s = store.clone();
    let id = task.id;
    assert!(
        run_until_state(&mut d, || reviewer_starts(&s, id).len() == 2).await,
        "the review was not redone: {:?}",
        store.events_for(task.id).unwrap()
    );
    let starts = reviewer_starts(&store, task.id);
    assert_ne!(starts[1], stuck_run);
    assert_eq!(running_rows(&store), vec![starts[1].clone()]);
    d.abort_all_runs();
}
