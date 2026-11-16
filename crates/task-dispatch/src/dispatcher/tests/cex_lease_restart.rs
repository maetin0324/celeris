//! 健全性v2 R4 cex-lease: Dispatcher の状態所有を分割する前に、lease・restart・同時完了の不変条件を今の実装に
//! 対して固定する反例試験（`agent-docs/progress/2026-11-11-health-v2-r4/cex-lease.md`）。
//!
//! 規約: 試験は Dispatcher の private field に触らず、store の行（task・lease・WU・`runs` 索引）と event だけを
//! 見る。待ちは [`run_until_state`]（出来事待ち・壁時計の保険）と、tick の戻り値（`TickReport`）の出来事待ちで、
//! tick の回数の上限で待たない。run の終わりは fake adapter の gate（semaphore・`Notify`）で試験が決め、lease の
//! 失効は `renew_lease(.., 0)` で store の lease 時刻を今にして作る（実時計を進めない）。
use super::orphan_takeover::{SELF_ID, instance, new_active, running_orphan, slow_adapter};
use super::*;
use crate::orphan::ORPHAN_TAKEOVER_REASON;

/// run を試験が開けるまで止める adapter（run ごとに permit を 1 つ消費する）。
struct GatedAdapter {
    started: StdMutex<Vec<String>>,
    gate: tokio::sync::Semaphore,
    returned: AtomicUsize,
}

impl GatedAdapter {
    fn new() -> Arc<Self> {
        Arc::new(GatedAdapter {
            started: StdMutex::new(Vec::new()),
            gate: tokio::sync::Semaphore::new(0),
            returned: AtomicUsize::new(0),
        })
    }

    fn started(&self) -> Vec<String> {
        self.started.lock().unwrap().clone()
    }

    fn release_one(&self) {
        self.gate.add_permits(1);
    }

    fn returned(&self) -> usize {
        self.returned.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl WorkerAdapter for GatedAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        _req: RunRequest,
        run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        self.started.lock().unwrap().push(run_id.to_string());
        if let Ok(permit) = self.gate.acquire().await {
            permit.forget();
        }
        self.returned.fetch_add(1, Ordering::SeqCst);
        Ok(done_outcome())
    }
}

fn true_task(dir: &std::path::Path) -> Task {
    new_task(
        dir,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        2,
    )
}

fn lease_run(store: &Arc<SqliteStore>, id: TaskId) -> Option<String> {
    store
        .get(id)
        .unwrap()
        .and_then(|t| t.lease.map(|l| l.worker_run_id))
}

fn status_of(store: &Arc<SqliteStore>, id: TaskId) -> Status {
    store.get(id).unwrap().unwrap().status
}

/// `run_id` の `WorkerFinished` の終わり方（event の数 = 長さ）。
fn finishes(store: &Arc<SqliteStore>, id: TaskId, run_id: &str) -> Vec<Option<RunEnd>> {
    events_of(store, id)
        .into_iter()
        .filter_map(|e| match e {
            Event::WorkerFinished { run_id: r, end, .. } if r == run_id => Some(end),
            _ => None,
        })
        .collect()
}

fn worker_starts(store: &Arc<SqliteStore>, id: TaskId) -> Vec<String> {
    events_of(store, id)
        .into_iter()
        .filter_map(|e| match e {
            Event::WorkerStarted { run_id, .. } => Some(run_id),
            _ => None,
        })
        .collect()
}

fn index_status(store: &Arc<SqliteStore>, run_id: &str) -> Option<task_core::RunIndexStatus> {
    store.run_index_get(run_id).unwrap().map(|r| r.status)
}

fn took_over(store: &Arc<SqliteStore>, id: TaskId) -> bool {
    events_of(store, id).iter().any(|e| {
        matches!(e, Event::WorkerProgress { msg, .. } if msg.starts_with(ORPHAN_TAKEOVER_REASON))
    })
}

pub(super) fn wu_transitions(
    store: &Arc<SqliteStore>,
    id: TaskId,
    key: &str,
) -> Vec<(WorkUnitStatus, String)> {
    events_of(store, id)
        .into_iter()
        .filter_map(|e| match e {
            Event::WorkUnitTransitioned {
                key: k, to, reason, ..
            } if k == key => Some((to, reason)),
            _ => None,
        })
        .collect()
}

pub(super) fn wu_row(store: &Arc<SqliteStore>, id: TaskId, key: &str) -> task_core::WorkUnitRow {
    store
        .work_units_for(id)
        .unwrap()
        .into_iter()
        .find(|u| u.key == key)
        .unwrap()
}

/// 試験の task（current-thread の runtime）に worker の task を走らせる間（成立条件ではなく譲り）。
async fn yield_to_workers() {
    tokio::time::sleep(Duration::from_millis(200)).await;
}

/// `TickReport.in_flight` が 0 になるまで tick を駆動し、その間に drain した完了の数を返す（出来事待ち。保険は
/// [`STATE_WAIT_GUARD`] の壁時計）。
async fn drain_until_no_in_flight(d: &mut Dispatcher) -> Option<usize> {
    let deadline = Instant::now() + STATE_WAIT_GUARD;
    let mut finished = 0;
    loop {
        let report = d.tick().unwrap();
        finished += report.finished;
        if report.in_flight == 0 {
            return Some(finished);
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// 不変条件（lease の単一保持者、ADR-0002 D9・ADR-0005 D4）: lease を回収された旧持ち主 run A の完了が遅れて届いても、
/// 再割当の run B の lease・Task の状態・attempts・`runs` 行を書き換えない。
///
/// 既存試験で検出できない理由: `a_lease_expiry_requeue_closes_the_runs_row` は回収側だけを見て A の完了を届けず、
/// `stale_recovery_cannot_overwrite_a_new_run_replan_or_task_cancel` は WU 回復の比較更新だけを見る。旧持ち主の
/// dispatcher に A を手元に持たせたまま、別の dispatcher が回収して B を起こした後に、A の完了を `drain_completions`
/// （`on_worker_finished` の `run_holds_lease`）へ通す順序はどれも作らない。
#[tokio::test]
async fn cex_stale_holder_late_finish_does_not_overwrite_the_reassigned_run() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = true_task(dir.path());
    let id = task.id;
    store.insert(&task).unwrap();
    let old = GatedAdapter::new();
    let mut holder = dispatcher(store.clone(), old.clone(), 1);
    let (s, o) = (store.clone(), old.clone());
    assert!(
        run_until_state(&mut holder, || o.started().len() == 1
            && lease_run(&s, id).is_some())
        .await
    );
    let run_a = lease_run(&store, id).unwrap();
    assert_eq!(old.started(), vec![run_a.clone()]);

    // A の lease が切れた（store の lease 時刻を今にする）。手元に A を持たない別の dispatcher が回収する。
    assert!(store.renew_lease(id, &run_a, Duration::ZERO).unwrap());
    let mut reclaimer = dispatcher(store.clone(), slow_adapter(), 1);
    let report = reclaimer.tick().unwrap();
    assert_eq!(report.reclaimed, 1, "{report:?}");
    assert_eq!(status_of(&store, id), Status::Ready);
    assert_eq!(
        finishes(&store, id, &run_a),
        vec![Some(RunEnd::HarnessError {
            class: task_core::HarnessErrorClass::LeaseExpired
        })]
    );
    let attempts = store.get(id).unwrap().unwrap().attempts;

    // 新しい dispatcher が B を起こす（回収側の infra backoff は手元の memory なので、別の dispatcher で起こす）。
    let mut next = dispatcher(store.clone(), slow_adapter(), 1);
    let s = store.clone();
    let a = run_a.clone();
    assert!(run_until_state(&mut next, || lease_run(&s, id).is_some_and(|r| r != a)).await);
    let run_b = lease_run(&store, id).unwrap();
    assert_eq!(
        index_status(&store, &run_b),
        Some(task_core::RunIndexStatus::Running)
    );
    let events_before = events_of(&store, id).len();

    // A の完了が旧持ち主の channel に入ってから、旧持ち主が tick する（`drain_completions` が先に受け取る）。
    old.release_one();
    let o = old.clone();
    let deadline = Instant::now() + STATE_WAIT_GUARD;
    while o.returned() == 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(old.returned(), 1);
    yield_to_workers().await;
    let drained = drain_until_no_in_flight(&mut holder).await;
    assert_eq!(
        drained,
        Some(1),
        "the late completion of A must reach the old holder's drain"
    );

    let t = store.get(id).unwrap().unwrap();
    assert_eq!(t.status, Status::Running);
    assert_eq!(t.lease.map(|l| l.worker_run_id), Some(run_b.clone()));
    assert_eq!(t.attempts, attempts);
    assert_eq!(
        index_status(&store, &run_b),
        Some(task_core::RunIndexStatus::Running)
    );
    assert!(finishes(&store, id, &run_b).is_empty());
    assert_eq!(finishes(&store, id, &run_a).len(), 1);
    let after: Vec<Event> = events_of(&store, id).split_off(events_before);
    assert!(
        !after
            .iter()
            .any(|e| matches!(e, Event::Transitioned { .. })),
        "{after:?}"
    );
    next.abort_all_runs();
}

/// 不変条件（ADR-0040 D4・Phase F5-fix6 の孤児の定義 1.）: 孤児の回収が有効で他の instance が 1 つも無くても、
/// この instance の手元にある run は lease が残る間は横取りしない（`WorkerFinished` も `orphan_takeover` も書かず、
/// lease と `runs` 行を変えない）。run が終わると通常の完了で 1 回だけ閉じる。
///
/// 既存試験で検出できない理由: `orphan_takeover.rs` の試験は旧 instance が持っていた（手元に無い）run だけを
/// 種にし、回収する側の dispatcher 自身が起こした run を、他の instance が居ない状態で tick させない。
/// `holds_task_in_hand` / `holds_run_in_hand` を外しても既存試験は落ちない。
#[tokio::test]
async fn cex_orphan_takeover_leaves_the_run_held_in_this_instance() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = true_task(dir.path());
    let id = task.id;
    store.insert(&task).unwrap();
    let adapter = GatedAdapter::new();
    let mut d = new_active(&store, adapter.clone());
    let (s, a) = (store.clone(), adapter.clone());
    assert!(
        run_until_state(&mut d, || a.started().len() == 1
            && lease_run(&s, id).is_some())
        .await
    );
    let run_a = lease_run(&store, id).unwrap();

    // 1 回の tick で lease の回収（`reclaim_expired_leases`）と ownerless の照合（初回は周期を待たない）が同期に走る。
    let report = d.tick().unwrap();
    assert_eq!(report.reclaimed, 0, "{report:?}");
    assert!(!took_over(&store, id));
    assert!(finishes(&store, id, &run_a).is_empty());
    assert_eq!(lease_run(&store, id), Some(run_a.clone()));
    assert_eq!(
        index_status(&store, &run_a),
        Some(task_core::RunIndexStatus::Running)
    );
    assert_eq!(status_of(&store, id), Status::Running);

    adapter.release_one();
    let s = store.clone();
    assert!(run_until_state(&mut d, || status_of(&s, id) == Status::Done).await);
    assert!(!took_over(&store, id));
    assert_eq!(finishes(&store, id, &run_a).len(), 1);
    assert_eq!(worker_starts(&store, id), vec![run_a.clone()]);
    assert_ne!(
        index_status(&store, &run_a),
        Some(task_core::RunIndexStatus::Running)
    );
}

/// 不変条件（`crate::orphan::holder_gone`・ADR-0040 D4）: 他の active instance が生きていて（heartbeat が新しく
/// pid が居る）、lease も残る run は、手元に無くても孤児として回収しない。持ち主の instance が消えた後の tick で
/// 初めて 1 回だけ回収する。
///
/// 既存試験で検出できない理由: `a_run_held_by_a_live_draining_instance_is_left_alone` は持ち主が draining の場合だけで、
/// 持ち主が active の別 instance（引き継ぎ前の二重起動・standby からの昇格直後）である組合せを作らない。
#[tokio::test]
async fn cex_orphan_takeover_leaves_a_run_whose_active_holder_lives() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (id, run_a) = running_orphan(&store, dir.path(), None, false);
    store
        .instance_register(&instance(
            "01OTHERACTIVE00000000000000",
            InstanceRole::Active,
            std::process::id(),
        ))
        .unwrap();
    let mut d = new_active(&store, slow_adapter());
    let report = d.tick().unwrap();
    assert_eq!(report.reclaimed, 0, "{report:?}");
    assert!(!took_over(&store, id));
    assert!(finishes(&store, id, &run_a).is_empty());
    assert_eq!(lease_run(&store, id), Some(run_a.clone()));
    assert_eq!(
        index_status(&store, &run_a),
        Some(task_core::RunIndexStatus::Running)
    );

    // 持ち主の instance が消えた: 次の tick で lease を待たずに 1 回だけ回収する。
    store
        .instance_delete("01OTHERACTIVE00000000000000")
        .unwrap();
    let report = d.tick().unwrap();
    assert_eq!(report.reclaimed, 1, "{report:?}");
    assert!(took_over(&store, id));
    assert_eq!(finishes(&store, id, &run_a).len(), 1);
    d.abort_all_runs();
    assert_eq!(finishes(&store, id, &run_a).len(), 1);
    assert_ne!(SELF_ID, "01OTHERACTIVE00000000000000");
}

/// 不変条件（ADR-0040 D4・ADR-0070 D5）: draining の instance が持つ run の lease が切れたら、その instance が自分で
/// 1 回だけ回収し（`WorkerFinished{lease_expired}` 1 件、`runs` 行を閉じる）、draining の間は回収した Task を
/// 起こし直さない。起こし直すのは新しい active で、旧 run の終わりは増えない。
///
/// 既存試験で検出できない理由: draining の試験（`drain_hand_off.rs`・`a_run_held_by_a_live_draining_instance_is_left_alone`）
/// は lease が残る間の手離し・横取りしないことだけを見て、draining の instance の手元の run の lease が切れる順序を作らない。
#[tokio::test]
async fn cex_draining_instance_reclaims_its_expired_lease_without_redispatching() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = true_task(dir.path());
    let id = task.id;
    store.insert(&task).unwrap();
    let adapter = GatedAdapter::new();
    let mut draining = dispatcher(store.clone(), adapter.clone(), 1);
    let (s, a) = (store.clone(), adapter.clone());
    assert!(
        run_until_state(&mut draining, || a.started().len() == 1
            && lease_run(&s, id).is_some())
        .await
    );
    let run_a = lease_run(&store, id).unwrap();
    draining.set_accepting_new_work(false);
    assert!(store.renew_lease(id, &run_a, Duration::ZERO).unwrap());

    let report = draining.tick().unwrap();
    assert_eq!(report.reclaimed, 1, "{report:?}");
    assert_eq!(status_of(&store, id), Status::Ready);
    assert_eq!(
        finishes(&store, id, &run_a),
        vec![Some(RunEnd::HarnessError {
            class: task_core::HarnessErrorClass::LeaseExpired
        })]
    );
    assert_ne!(
        index_status(&store, &run_a),
        Some(task_core::RunIndexStatus::Running)
    );
    // draining の間は Ready の Task を起こさない。回収した Task は手元の infra backoff でも止まるので、backoff の無い
    // 新しい Task も置き、それも起こさないことを確かめる（tick の結果が dispatched 0・in_flight 0 のまま）。
    let other = true_task(dir.path());
    let other_id = other.id;
    store.insert(&other).unwrap();
    let report = draining.tick().unwrap();
    assert_eq!((report.dispatched, report.in_flight), (0, 0), "{report:?}");
    assert_eq!(worker_starts(&store, id), vec![run_a.clone()]);
    assert!(worker_starts(&store, other_id).is_empty());
    assert_eq!(status_of(&store, other_id), Status::Ready);

    let mut active = dispatcher(store.clone(), done_adapter(), 1);
    let s = store.clone();
    assert!(
        run_until_state(&mut active, || status_of(&s, id) == Status::Done
            && status_of(&s, other_id) == Status::Done)
        .await
    );
    let starts = worker_starts(&store, id);
    assert_eq!(starts.len(), 2, "{starts:?}");
    assert_eq!(finishes(&store, id, &run_a).len(), 1);
    assert_eq!(adapter.started(), vec![run_a]);
}

/// reviewer run を試験が開けるまで止め、開いたら `pass` の判定を `review.json` に書く adapter（reviewer run ごとに
/// permit を 1 つ消費する）。worker run は返らない（Task が ready に戻った後の run は試験の終わりに止める）。
struct VerdictGate {
    pass: bool,
    started: StdMutex<Vec<String>>,
    gate: tokio::sync::Semaphore,
    returned: AtomicUsize,
}

impl VerdictGate {
    fn new(pass: bool) -> Arc<Self> {
        Arc::new(VerdictGate {
            pass,
            started: StdMutex::new(Vec::new()),
            gate: tokio::sync::Semaphore::new(0),
            returned: AtomicUsize::new(0),
        })
    }

    fn started(&self) -> Vec<String> {
        self.started.lock().unwrap().clone()
    }
}

#[async_trait]
impl WorkerAdapter for VerdictGate {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        if req.task.kind != TaskKind::Review {
            return std::future::pending().await;
        }
        self.started.lock().unwrap().push(run_id.to_string());
        if let Ok(permit) = self.gate.acquire().await {
            permit.forget();
        }
        std::fs::create_dir_all(&req.artifacts_dir).unwrap();
        let reason = if self.pass {
            "reviewer: looks complete"
        } else {
            "reviewer: not complete"
        };
        std::fs::write(
            req.artifacts_dir.join("review.json"),
            serde_json::json!({
                "verdicts": [{"criterion": 0, "pass": self.pass, "reason": reason}],
                "health": {"findings": []},
            })
            .to_string(),
        )
        .unwrap();
        self.returned.fetch_add(1, Ordering::SeqCst);
        Ok(done_outcome())
    }
}

/// `WorkerStarted{role: reviewer}` の run id（起きた順）。
fn reviewer_runs(store: &Arc<SqliteStore>, id: TaskId) -> Vec<String> {
    events_of(store, id)
        .into_iter()
        .filter_map(|e| match e {
            Event::WorkerStarted {
                run_id,
                role: Some(RunRole::Reviewer),
                ..
            } => Some(run_id),
            _ => None,
        })
        .collect()
}

/// `run_id` の `WorkerFinished` の outcome（event の数 = 長さ）。
fn finish_outcomes(store: &Arc<SqliteStore>, id: TaskId, run_id: &str) -> Vec<String> {
    events_of(store, id)
        .into_iter()
        .filter_map(|e| match e {
            Event::WorkerFinished {
                run_id: r, outcome, ..
            } if r == run_id => Some(outcome),
            _ => None,
        })
        .collect()
}

/// review の完了を `drain_completions` が受け取る（`TickReport.reviewed` が 1 以上）まで `d` の tick を駆動する
/// （出来事待ち。保険は [`STATE_WAIT_GUARD`] の壁時計）。
async fn tick_until_review_delivered(d: &mut Dispatcher) -> bool {
    let deadline = Instant::now() + STATE_WAIT_GUARD;
    loop {
        if d.tick().unwrap().reviewed > 0 {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// 不変条件（ADR 2026-10-03-ownerless-running-runs・ADR 2026-11-11-dispatcher-state-ownership D7 の restart の行）:
/// 新しい active が reviewer run A を持ち主の居ない run として閉じた（`WorkerFinished{end: cancelled, outcome:
/// "interrupted: <ownerless>"}`）後に、A を手元に持っていた旧 draining instance から A の合格が遅れて届いても、A の
/// `runs` 行は Running に戻らず、A の合格は `ReviewVerdict`・Task の遷移に使われない。Task は新しい active が始めた
/// reviewer run B の結果だけで決まる（A は合格・B は不合格にして区別する。Task は Done にならず、`review_fail` で
/// ready に戻る）。
///
/// 作る順序: 旧 instance が A の `ReviewEntry` と一緒に task ごとの review lock
/// （`runs/.review-<id>.lock`）を判定の保存まで持つので、新しい active は A を閉じた後も B を起こせない
/// （`spawn_review` の `try_lock` が WouldBlock）。B が起きるのは、旧 instance が A の完了を `drain_completions` で
/// 受けて lock を放した後になる。そこで D7 の「B を始めた後に A の合格が届く」順序のうち lock の下で作れる順序
/// （A を閉じる → B は lock で起きない → A の合格が旧 instance の drain に届く → B が起きて不合格）を作り、lock が
/// B を止めることも assert する。持ち主の居ない run として閉じるのは、旧 instance の行を消して作る（注入時計を進めるには
/// private field の `test_hooks` に触る必要がある）。
///
/// 既存試験で検出できない理由: `tests/ownerless_runs.rs` の T1a（`startup_closes_an_ownerless_reviewer_run_and_rereviews`）
/// と T1b（`ownerless_runs_are_reconciled_again_after_the_interval`）は、持ち主の居ない reviewer run の行を
/// `seed_reviewer_run` で store に直接作るので、その run を走らせて後から合格を届けうる instance が無い。T1c
/// （`an_ownerless_run_is_kept_while_another_instance_lives_until_its_deadline`）は draining の他の instance の行を
/// 登録するが、その instance は dispatcher を持たず reviewer run を走らせないので、閉じた後に A の完了が届く順序を
/// 作らない（Task も blocked で、レビューをやり直さない）。T0a・T0b は旧 instance の停止・クラッシュで A の完了が
/// 届かない順序だけを作る。
#[tokio::test]
async fn cex_restart_old_reviewer_pass_after_ownerless_close_is_ignored() {
    use crate::orphan::OWNERLESS_GONE_WHY;
    use task_core::RunIndexStatus;
    const OLD_ID: &str = "01OLDREVIEWER00000000000000";

    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = new_task(dir.path(), Check::Reviewer, 2);
    task.status = Status::Reviewing;
    let id = task.id;
    store.insert(&task).unwrap();
    store
        .append_event(
            id,
            &Event::Created {
                created_by: None,
                task: Box::new(task.clone()),
                origin: None,
            },
        )
        .unwrap();

    // 旧 instance（active）が reviewer run A を起こし、A は gate で止まる。その後ライブ切替で draining になる。
    store
        .instance_register(&instance(OLD_ID, InstanceRole::Active, std::process::id()))
        .unwrap();
    let old_reviewer = VerdictGate::new(true);
    let mut old = dispatcher(store.clone(), old_reviewer.clone(), 1);
    let (s, o) = (store.clone(), old_reviewer.clone());
    assert!(
        run_until_state(&mut old, || o.started().len() == 1
            && reviewer_runs(&s, id).len() == 1)
        .await
    );
    let run_a = reviewer_runs(&store, id).remove(0);
    assert_eq!(old_reviewer.started(), vec![run_a.clone()]);
    assert_eq!(index_status(&store, &run_a), Some(RunIndexStatus::Running));
    old.set_accepting_new_work(false);
    store
        .instance_set_role(OLD_ID, InstanceRole::Draining, OffsetDateTime::now_utc())
        .unwrap();

    // 旧 instance の行が消えた（旧 process は A を手元に持ったまま）。新しい active の照合が A を持ち主の居ない run
    // として閉じる。
    store.instance_delete(OLD_ID).unwrap();
    let new_reviewer = VerdictGate::new(false);
    let mut new = new_active(&store, new_reviewer.clone());
    let (s, a) = (store.clone(), run_a.clone());
    assert!(
        run_until_state(&mut new, || index_status(&s, &a)
            != Some(RunIndexStatus::Running))
        .await
    );
    assert_eq!(finishes(&store, id, &run_a), vec![Some(RunEnd::Cancelled)]);
    assert_eq!(
        finish_outcomes(&store, id, &run_a),
        vec![format!("interrupted: {OWNERLESS_GONE_WHY}")]
    );
    // 旧 instance が A とともに review lock を持つ間、新しい active は B を起こさない（tick の中の `recover_reviews` の
    // `spawn_review` が lock を取れない）。
    new.tick().unwrap();
    assert_eq!(
        reviewer_runs(&store, id),
        vec![run_a.clone()],
        "the new active started a reviewer while the old instance still holds review A"
    );
    assert!(new_reviewer.started().is_empty());
    assert_eq!(status_of(&store, id), Status::Reviewing);
    let events_before = events_of(&store, id).len();

    // A の合格が旧 instance の channel に入ってから、旧 instance が tick する（`drain_completions` が受け取る）。
    old_reviewer.gate.add_permits(1);
    let o = old_reviewer.clone();
    let deadline = Instant::now() + STATE_WAIT_GUARD;
    while o.returned.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(old_reviewer.returned.load(Ordering::SeqCst), 1);
    assert!(
        tick_until_review_delivered(&mut old).await,
        "the late completion of A must reach the old instance's drain"
    );
    let after: Vec<Event> = events_of(&store, id).split_off(events_before);
    assert_ne!(index_status(&store, &run_a), Some(RunIndexStatus::Running));
    assert!(
        !after
            .iter()
            .any(|e| matches!(e, Event::ReviewVerdict { .. } | Event::Transitioned { .. })),
        "A's late pass was used after A was closed as an ownerless run (A's runs row {:?}, \
         A's WorkerFinished {:?}): {after:?}",
        index_status(&store, &run_a),
        finish_outcomes(&store, id, &run_a),
    );
    assert_eq!(status_of(&store, id), Status::Reviewing);

    // 旧 instance が lock を放した後に新しい active が B を起こし、B は不合格で終わる。
    let (s, n) = (store.clone(), new_reviewer.clone());
    assert!(
        run_until_state(&mut new, || n.started().len() == 1
            && reviewer_runs(&s, id).len() == 2)
        .await
    );
    let run_b = reviewer_runs(&store, id).remove(1);
    assert_ne!(run_b, run_a);
    assert_eq!(new_reviewer.started(), vec![run_b.clone()]);
    new_reviewer.gate.add_permits(1);
    let s = store.clone();
    assert!(run_until_state(&mut new, || status_of(&s, id) != Status::Reviewing).await);

    let events = events_of(&store, id);
    let verdicts: Vec<bool> = events
        .iter()
        .filter_map(|e| match e {
            Event::ReviewVerdict { pass, .. } => Some(*pass),
            _ => None,
        })
        .collect();
    assert_eq!(verdicts, vec![false], "{events:?}");
    let transitions: Vec<(Status, String)> = events
        .iter()
        .filter_map(|e| match e {
            Event::Transitioned { to, reason, .. } => Some((*to, reason.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        transitions.first(),
        Some(&(Status::Ready, "review_fail".to_string())),
        "{transitions:?}"
    );
    assert!(
        !transitions
            .iter()
            .any(|(to, reason)| *to == Status::Done || reason == "review_pass"),
        "{transitions:?}"
    );
    assert_ne!(status_of(&store, id), Status::Done);
    assert_eq!(finishes(&store, id, &run_b), vec![Some(RunEnd::Completed)]);
    assert_ne!(index_status(&store, &run_a), Some(RunIndexStatus::Running));
    new.abort_all_runs();
    old.abort_all_runs();
}

/// 不変条件（ADR-0072 D15・`CLAUDE.md` の SQLite 正本）: WU を done にした書き込みの後、Task の遷移・`runs` の閉じ・
/// 依存 WU の更新の前で落ちた（WU は done、Task は Running のまま、run A に `WorkerFinished` 無し）状態から再起動しても、
/// 回収は A を 1 回だけ閉じ、done の WU を作り直さず（A の WU を走らせ直さない）、もう一度再起動しても何も足さない。
///
/// 範囲: 種の状態（WU a の done を store に直に書く）からの回収の冪等性を固定する。依存 WU の昇格の数と、実際の finish を
/// `halt_after_wu_transition` で止めた状態からの前進は [`cex_restart_after_wu_done_promotes_dependents_once`] が固定する。
///
/// 変えた理由（ADR 2026-11-14-dependency-promotion-every-plan-version）: 以前は v1 の計画の dispatch gate が依存 WU を
/// 上げず、この状態から Task が ready のまま進まなかった。この試験はその停止を前提に、最初の再起動の 1 tick の後で
/// dispatcher を落として 2 回目の再起動を比べていた。修正の後は最初の tick で b が上がって走るので、その時点で落とすと
/// b の run が新しい孤児になり、2 回目の再起動が正しく回収する（`reclaimed == 1`）。そこで最初の再起動は Task が Done に
/// 届くまで回し、その後の再起動で何も足さないことを見る。
///
/// 既存試験で検出できない理由: `test_hooks.halt_after_wu_transition` を使う試験（`cluster_job_wait.rs`）は cluster の待ちの
/// WU 遷移だけを見る。`orphan_work_units.rs` は WU が running のまま落ちた状態だけを種にし、WU が先に done になった
/// 状態からの再起動・その再実行を作らない。
#[tokio::test]
async fn cex_restart_after_crash_between_wu_and_task_transitions_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (id, run_a) = running_orphan(&store, dir.path(), None, true);
    let a = wu_row(&store, id, "a");
    let mut done_a = a.clone();
    done_a.status = WorkUnitStatus::Done;
    store
        .work_unit_transition(
            id,
            done_a,
            Event::WorkUnitTransitioned {
                work_unit_id: a.id.clone(),
                key: "a".into(),
                from: WorkUnitStatus::Running,
                to: WorkUnitStatus::Done,
                reason: "completed".into(),
                run_id: Some(run_a.clone()),
            },
        )
        .unwrap();

    let mut d = new_active(&store, done_adapter());
    let report = d.tick().unwrap();
    assert_eq!(report.reclaimed, 1, "{report:?}");
    assert_eq!(finishes(&store, id, &run_a).len(), 1);
    assert_eq!(wu_row(&store, id, "a").status, WorkUnitStatus::Done);
    let s = store.clone();
    assert!(run_until_state(&mut d, || status_of(&s, id) == Status::Done).await);
    let snapshot = events_of(&store, id).len();
    drop(d);

    // もう一度再起動（同じ store に新しい active）。同じ回収を 2 回目に書かない。
    let mut again = new_active(&store, done_adapter());
    let report = again.tick().unwrap();
    assert_eq!(report.reclaimed, 0, "{report:?}");
    assert_eq!(finishes(&store, id, &run_a).len(), 1);
    assert_eq!(
        wu_transitions(&store, id, "a"),
        vec![
            (WorkUnitStatus::Running, "dispatch".to_string()),
            (WorkUnitStatus::Done, "completed".to_string())
        ]
    );
    assert_eq!(wu_row(&store, id, "a").last_run_id, Some(run_a.clone()));
    let starts = worker_starts(&store, id);
    assert_eq!(starts.len(), 3, "{starts:?}");
    assert_eq!(starts.iter().filter(|r| **r == run_a).count(), 1);
    assert_eq!(events_of(&store, id).len(), snapshot);
    assert!(store.runs_running().unwrap().is_empty());
}

/// `key` の WU の `dependency_ready` の遷移の数。
pub(super) fn dependency_ready_count(store: &Arc<SqliteStore>, id: TaskId, key: &str) -> usize {
    wu_transitions(store, id, key)
        .into_iter()
        .filter(|(to, reason)| *to == WorkUnitStatus::Ready && reason == "dependency_ready")
        .count()
}

/// 不変条件（ADR 2026-11-14-dependency-promotion-every-plan-version・ADR-0072 D15）: v1 の計画（a → b → c）で、
/// finish が WU a の done を書いた直後（依存 WU の `dependency_ready`・Task の遷移・`runs` の閉じの前）で落ちても、
/// 再起動の後の dispatch gate が SQLite の依存の状態から b を ready に上げ、b・c は `dependency_ready` で 1 回ずつ
/// ready に上がり、Task は Done に届く。a は走らせ直さない。もう一度再起動しても何も足さない。
///
/// 既存試験で検出できない理由: [`cex_restart_after_crash_between_wu_and_task_transitions_is_idempotent`] は WU a の done を
/// store に直に書いた種から始め、finish の実際の書き込みの順（a の done の後の打ち切り）を通らず、依存 WU の
/// `dependency_ready` の数を見ない（gate と finish の両方が b を上げても落ちない）。`halt_after_wu_transition` を使う
/// `cluster_job_wait.rs` の試験は v3 の計画の cluster の待ちの遷移だけを見る。`orphan_work_units.rs` は WU が running の
/// まま落ちた状態だけを種にする。
#[tokio::test]
async fn cex_restart_after_wu_done_promotes_dependents_once() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = true_task(dir.path());
    let id = task.id;
    store.insert(&task).unwrap();
    adopt_three_step_plan(&store, id);

    // 旧い instance: 実際の finish を WU a の done の書き込みの直後で止める（`daemon_instances` に行を持たない =
    // 再起動の後は持ち主が居ない）。
    let mut old = dispatcher(store.clone(), done_adapter(), 1);
    old.test_hooks.halt_after_wu_transition = true;
    let s = store.clone();
    assert!(
        run_until_state(&mut old, || wu_row(&s, id, "a").status
            == WorkUnitStatus::Done)
        .await
    );
    drop(old);
    let run_a = wu_row(&store, id, "a").last_run_id.unwrap();
    assert_eq!(status_of(&store, id), Status::Running);
    assert!(finishes(&store, id, &run_a).is_empty());
    assert_eq!(wu_row(&store, id, "b").status, WorkUnitStatus::Pending);
    assert_eq!(wu_row(&store, id, "c").status, WorkUnitStatus::Pending);

    // 再起動: 新しい active が A を回収して Task を ready に戻し、gate が b を上げて b・c を走らせる。
    let mut active = new_active(&store, done_adapter());
    let s = store.clone();
    let reached = run_until_state(&mut active, || status_of(&s, id) == Status::Done).await;
    assert!(
        reached,
        "status={:?} b={:?} c={:?}",
        status_of(&store, id),
        wu_transitions(&store, id, "b"),
        wu_transitions(&store, id, "c")
    );
    assert_eq!(dependency_ready_count(&store, id, "b"), 1);
    assert_eq!(dependency_ready_count(&store, id, "c"), 1);
    assert_eq!(
        wu_transitions(&store, id, "a"),
        vec![
            (WorkUnitStatus::Running, "dispatch".to_string()),
            (WorkUnitStatus::Done, "completed".to_string())
        ]
    );
    for key in ["b", "c"] {
        assert_eq!(wu_row(&store, id, key).status, WorkUnitStatus::Done);
    }
    assert_eq!(finishes(&store, id, &run_a).len(), 1);
    assert_eq!(worker_starts(&store, id).len(), 3);
    assert!(store.runs_running().unwrap().is_empty());
    let snapshot = events_of(&store, id).len();
    drop(active);

    // もう一度再起動（同じ store に新しい active）。回収も昇格も 2 回目に書かない。
    let mut again = new_active(&store, done_adapter());
    let report = again.tick().unwrap();
    assert_eq!(report.reclaimed, 0, "{report:?}");
    assert_eq!(events_of(&store, id).len(), snapshot);
    assert_eq!(dependency_ready_count(&store, id, "b"), 1);
    assert_eq!(dependency_ready_count(&store, id, "c"), 1);
}

/// 不変条件（ADR 2026-10-03-ownerless-running-runs D3・`close_aborted_run` の再実行の注記）: `WorkerFinished` を
/// 記録した後、WU の回収の前に落ちた run を再起動後に閉じ直すとき、`WorkerFinished` を二重に書かず、WU の回収は
/// 省略しない（WU を running のまま残さない）。
///
/// 既存試験で検出できない理由: ownerless の試験（`ownerless_runs.rs`）は `WorkerFinished` の無い run 行だけを閉じ、
/// `orphan_work_units.rs` の回収試験は `WorkerFinished` を書く前の状態だけを種にする。記録済みの run を閉じ直す
/// 経路の重複防止（`close_aborted_run` の `already`）を外しても既存試験は落ちない。
#[tokio::test]
async fn cex_reclosing_a_run_after_worker_finished_does_not_write_twice() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (id, run_a) = running_orphan(&store, dir.path(), None, true);
    // 人の割り込みで Task が Ready に戻り、A の `WorkerFinished` を書いた直後に落ちた（WU a は running のまま）。
    store
        .apply_transition_with_events(
            id,
            Trigger::Interrupt,
            vec![Event::WorkerFinished {
                run_id: run_a.clone(),
                outcome: "interrupted: comment".into(),
                usage: None,
                role: None,
                metrics: None,
                end: Some(RunEnd::Cancelled),
            }],
        )
        .unwrap();
    assert_eq!(status_of(&store, id), Status::Ready);
    assert_eq!(wu_row(&store, id, "a").status, WorkUnitStatus::Running);
    assert_eq!(finishes(&store, id, &run_a).len(), 1);

    let mut d = new_active(&store, done_adapter());
    d.tick().unwrap();
    assert_eq!(finishes(&store, id, &run_a).len(), 1);
    assert_ne!(
        index_status(&store, &run_a),
        Some(task_core::RunIndexStatus::Running)
    );
    let a = wu_row(&store, id, "a");
    assert!(
        a.status != WorkUnitStatus::Running || a.last_run_id.as_deref() != Some(run_a.as_str()),
        "{a:?}"
    );

    let s = store.clone();
    assert!(run_until_state(&mut d, || status_of(&s, id) == Status::Done).await);
    assert_eq!(finishes(&store, id, &run_a).len(), 1);
    for key in ["a", "b", "c"] {
        assert_eq!(
            wu_row(&store, id, key).status,
            WorkUnitStatus::Done,
            "{key}"
        );
    }
    assert!(store.runs_running().unwrap().is_empty());
}

/// 不変条件（ADR 2026-10-03-ownerless-running-runs D3・`close_aborted_run` の再実行の注記）: 手元の run A の
/// `WorkerFinished` を別の経路（人の割り込みと同時の閉じ・新しい active の照合）が先に記録した後で、持ち主の
/// dispatcher が A を止めて閉じ直しても、`WorkerFinished` を二重に書かず、WU の回収は省略しない（WU a を A の
/// running のまま残さず、別の run で走らせ直して Task を完了させる）。
///
/// 既存試験で検出できない理由: 割り込み・cancel の試験（`a_run_aborted_by_cancel_closes_its_runs_row` など）は
/// `WorkerFinished` を持ち主の閉じ直しで初めて書く順序だけを作り、記録済みの run を閉じ直す順序を作らない。
/// `close_aborted_run` の `already` を外しても既存試験は落ちない。
#[tokio::test]
async fn cex_holder_reclosing_an_already_finished_run_does_not_write_twice() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = true_task(dir.path());
    let id = task.id;
    store.insert(&task).unwrap();
    adopt_three_step_plan(&store, id);
    let adapter = GatedAdapter::new();
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    let (s, a) = (store.clone(), adapter.clone());
    assert!(
        run_until_state(&mut d, || a.started().len() == 1
            && wu_row(&s, id, "a").status == WorkUnitStatus::Running)
        .await
    );
    let run_a = wu_row(&store, id, "a").last_run_id.unwrap();
    assert_eq!(lease_run(&store, id), Some(run_a.clone()));

    // 別の経路が A の終わりを先に記録し、Task を Ready に戻した（WU a は running のまま）。
    store
        .apply_transition_with_events(
            id,
            Trigger::Interrupt,
            vec![Event::WorkerFinished {
                run_id: run_a.clone(),
                outcome: "interrupted: comment".into(),
                usage: None,
                role: None,
                metrics: None,
                end: Some(RunEnd::Cancelled),
            }],
        )
        .unwrap();
    assert_eq!(finishes(&store, id, &run_a).len(), 1);

    // 持ち主の tick: `abort_stale_runs` が A を止めて `close_aborted_run` で閉じ直す（同期）。
    d.tick().unwrap();
    assert_eq!(finishes(&store, id, &run_a).len(), 1);
    let a_row = wu_row(&store, id, "a");
    assert!(
        a_row.status != WorkUnitStatus::Running
            || a_row.last_run_id.as_deref() != Some(run_a.as_str()),
        "{a_row:?}"
    );

    adapter.gate.add_permits(8);
    let s = store.clone();
    assert!(run_until_state(&mut d, || status_of(&s, id) == Status::Done).await);
    assert_eq!(finishes(&store, id, &run_a).len(), 1);
    for key in ["a", "b", "c"] {
        assert_eq!(
            wu_row(&store, id, key).status,
            WorkUnitStatus::Done,
            "{key}"
        );
    }
    assert_ne!(wu_row(&store, id, "a").last_run_id, Some(run_a.clone()));
    assert!(store.runs_running().unwrap().is_empty());
}

/// 2 件の WU（同じ工程 build、互いに独立）の v2 計画。`a` に検査 `a_check` があれば付ける。
fn adopt_pair_plan(store: &Arc<SqliteStore>, id: TaskId, a_check: Option<String>) {
    let mut a = v2_wu("a", "build", &[]);
    if let Some(cmd) = a_check {
        a.checks = vec![task_core::WorkUnitCheck {
            cmd,
            expect_exit: 0,
            scope: false,
        }];
    }
    adopt_v2_plan(store, id, &["build"], vec![a, v2_wu("b", "build", &[])]);
}

/// 試験が gate を開け、`active` が 0 になる（run が返る）まで dispatcher を tick しない（完了は channel に溜まる）。
async fn release_held_runs_without_ticking(adapter: &ParallelWuAdapter) {
    let deadline = Instant::now() + STATE_WAIT_GUARD;
    while adapter.active.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
        adapter.gate.notify_waiters();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(adapter.active.load(Ordering::SeqCst), 0);
    yield_to_workers().await;
}

/// 不変条件（ADR-0074 D1.5/D1.6、`CLAUDE.md` の SQLite 正本）: 同じ Task の並列 WU の run 2 本の完了を同じ tick の
/// `drain_completions` が受け取っても、両方の WU の done・`WorkerFinished`・`runs` 行の閉じが失われず、工程は統合へ
/// 進み、events からの作り直し（replay）と行が一致する。
///
/// 既存試験で検出できない理由: 並列 WU の試験（`run_three_independent_units` など）は run の長さ（sleep）で終わりを
/// ずらすか、終わりの順を決めず、2 本の完了を同じ tick に揃えることを確かめない（`TickReport.finished` を見ない）。
#[tokio::test]
async fn cex_two_parallel_work_unit_runs_finishing_in_one_tick_lose_no_update() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "test -f a.txt && test -f b.txt");
    let id = task.id;
    store.insert(&task).unwrap();
    adopt_pair_plan(&store, id, None);
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::ZERO)
            .with_file("a", "a.txt", "a")
            .with_file("b", "b.txt", "b")
            .holding("a")
            .holding("b"),
    );
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 2, 2, 2);
    let ad = adapter.clone();
    assert!(run_until_state(&mut d, || ad.active.load(Ordering::SeqCst) == 2).await);
    let runs: Vec<String> = ["a", "b"]
        .iter()
        .map(|k| wu_row(&store, id, k).last_run_id.unwrap())
        .collect();

    release_held_runs_without_ticking(&adapter).await;
    let report = d.tick().unwrap();
    assert_eq!(report.finished, 2, "{report:?}");

    let s = store.clone();
    assert!(run_until_state(&mut d, || status_of(&s, id) == Status::Done).await);
    for (key, run_id) in ["a", "b"].iter().zip(&runs) {
        assert_eq!(
            wu_row(&store, id, key).status,
            WorkUnitStatus::Done,
            "{key}"
        );
        assert_eq!(finishes(&store, id, run_id).len(), 1, "{key}");
        assert_eq!(wu_transitions(&store, id, key).len(), 2, "{key}");
        assert_eq!(
            index_status(&store, run_id),
            Some(task_core::RunIndexStatus::Completed),
            "{key}"
        );
    }
    assert!(store.runs_running().unwrap().is_empty());
    let (wu_mismatches, _runs, plan_mismatches, _) =
        task_ops::replay::check_and_apply_execution(store.as_ref(), false).unwrap();
    assert!(wu_mismatches.is_empty(), "{wu_mismatches:?}");
    assert!(plan_mismatches.is_empty(), "{plan_mismatches:?}");
}

/// 不変条件（ADR-0074 D1.6・ADR-0072 D14 (g)）: WU `a` の検査の完了と、並列の WU `b` の run の完了が同じ tick に
/// 届いても、`a` は検査の結果で done、`b` は run の結果で done になり、どちらの更新も失われず、`WorkerFinished` は各 1 件、
/// 工程は 1 回だけ統合されて Task が done になる。
///
/// 既存試験で検出できない理由: WU の検査の試験（`drain_hand_off.rs`・`integration_check_progress.rs`）は検査の完了と
/// 別の run の完了を同じ tick に揃えず、検査中の WU（`checking`）と走っている兄弟の WU が同時に終わる順序を作らない。
///
/// 待ち方（出来事待ち）: tick の前に `a` の検査の完了を、検査の列の最後の項目の `WorkUnitCheckFinished`
/// （`index + 1 == total`）が store に載ることで待つ。検査の task は最後の項目の `record_finish` から
/// `Completion::WorkUnitChecks` の送信まで await を挟まないので、current-thread の runtime の試験の task がこの event を
/// 見た時点で完了は channel に入っている。以前は flag を置いた後に run `b` の完了（`release_held_runs_without_ticking`
/// の 200 ms の譲り）だけを待って tick したので、`sleep 0.05` の loop の検査がその間に終わらないと `report.finished`
/// が 1 になって落ちた（load average 19.6 で 1 回観測）。
#[tokio::test]
async fn cex_run_finish_and_work_unit_check_finish_in_one_tick() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let flags = tempfile::tempdir().unwrap();
    let flag = flags.path().join("open");
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "test -f a.txt && test -f b.txt");
    let id = task.id;
    store.insert(&task).unwrap();
    // 検査は試験が flag を置くまで待つ（sleep の待ち。CPU を回さない）。
    adopt_pair_plan(
        &store,
        id,
        Some(format!(
            "while [ ! -f {} ]; do sleep 0.05; done; test -f a.txt",
            flag.display()
        )),
    );
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::ZERO)
            .with_file("a", "a.txt", "a")
            .with_file("b", "b.txt", "b")
            .holding("b"),
    );
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 2, 2, 2);
    let (s, ad) = (store.clone(), adapter.clone());
    assert!(
        run_until_state(&mut d, || {
            ad.active.load(Ordering::SeqCst) == 1
                && events_of(&s, id)
                    .iter()
                    .any(|e| matches!(e, Event::WorkUnitCheckStarted { .. }))
        })
        .await
    );
    let run_a = wu_row(&store, id, "a").last_run_id.unwrap();
    let run_b = wu_row(&store, id, "b").last_run_id.unwrap();
    assert_eq!(wu_row(&store, id, "a").status, WorkUnitStatus::Running);

    // 検査と run を同時に終わらせ、両方の完了が channel に溜まってから 1 回 tick する。
    std::fs::write(&flag, "").unwrap();
    release_held_runs_without_ticking(&adapter).await;
    let a_checks_done = |store: &Arc<SqliteStore>| {
        events_of(store, id).iter().any(|e| {
            matches!(e, Event::WorkUnitCheckFinished { key, index, total, .. }
                if key == "a" && index + 1 == *total)
        })
    };
    let deadline = Instant::now() + STATE_WAIT_GUARD;
    while !a_checks_done(&store) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(a_checks_done(&store), "the checks of a did not finish");
    let report = d.tick().unwrap();
    assert_eq!(report.finished, 2, "{report:?}");

    let s = store.clone();
    assert!(run_until_state(&mut d, || status_of(&s, id) == Status::Done).await);
    for (key, run_id) in [("a", &run_a), ("b", &run_b)] {
        assert_eq!(
            wu_row(&store, id, key).status,
            WorkUnitStatus::Done,
            "{key}"
        );
        assert_eq!(finishes(&store, id, run_id).len(), 1, "{key}");
        assert_eq!(
            index_status(&store, run_id),
            Some(task_core::RunIndexStatus::Completed),
            "{key}"
        );
    }
    let integrations = events_of(&store, id)
        .iter()
        .filter(|e| matches!(e, Event::WorkUnitTransitioned { key, to: WorkUnitStatus::Done, .. } if key == "integrate-build"))
        .count();
    assert_eq!(integrations, 1);
    assert!(store.runs_running().unwrap().is_empty());
}

/// 不変条件（lease の単一保持者、ADR-0002 D9・ADR-0070 D5・ADR 2026-10-03-ownerless-running-runs）: 回収側が期限切れの
/// snapshot で lease の回収を決めた後、書き込む前に、保持者が別の接続から `renew_lease` で lease を延ばしたら（本番では保持者の
/// instance の `StoreSink::heartbeat`）、回収側はその lease を回収しない。保持者・lease の run id と延ばした期限・`runs` 行・
/// Task の状態と attempts は変わらず、回収側は `WorkerFinished` も `orphan_takeover` の event も書かない。保持者の run は
/// その後の通常の完了で 1 回だけ閉じる。
///
/// 既存試験で検出できない理由: `reclaim_expired_leases` は同期で await を持たないので、試験の task は snapshot
/// （`store.list(Running)`）と回収の書き込みの間に他の接続の書き込みを挟めない。
/// [`cex_stale_holder_late_finish_does_not_overwrite_the_reassigned_run`] は回収が済んだ後の旧保持者の完了だけを、
/// `orphan_takeover.rs` は持ち主の居ない run の回収だけを、store の `renew_lease_extends_only_the_matching_running_lease`
/// は renew 単独を見る。回収側自身の延長（ADR-0070 D5 の `alive_task_run`）は回収側の手元の run にしか効かない。どれも
/// 「期限切れの snapshot → 別の接続の renew → 回収の書き込み」の順を作らないので、回収の書き込みが lease を照合しなくても
/// 通る。この順は `TestHooks::after_reclaim_decision`（回収を決めた直後の一回フック）で作る。
#[tokio::test]
async fn cex_lease_renew_after_expired_snapshot_keeps_the_holder() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lease.db");
    // 保持者と回収側は同じ DB の別の接続（別の `SqliteStore`）を持つ（同じ DB の 2 instance）。
    let holder_store: Arc<SqliteStore> = Arc::new(SqliteStore::open(&db).unwrap());
    let reclaim_store: Arc<SqliteStore> = Arc::new(SqliteStore::open(&db).unwrap());
    let task = true_task(dir.path());
    let id = task.id;
    holder_store.insert(&task).unwrap();
    let gated = GatedAdapter::new();
    let mut holder = dispatcher(holder_store.clone(), gated.clone(), 1);
    let (s, g) = (holder_store.clone(), gated.clone());
    assert!(
        run_until_state(&mut holder, || g.started().len() == 1
            && lease_run(&s, id).is_some())
        .await
    );
    let run_a = lease_run(&holder_store, id).unwrap();
    let attempts = holder_store.get(id).unwrap().unwrap().attempts;

    // A の lease を切らす（store の lease 時刻を今にする）。回収側は A を手元に持たない。
    assert!(
        holder_store
            .renew_lease(id, &run_a, Duration::ZERO)
            .unwrap()
    );
    let events_before = events_of(&reclaim_store, id).len();
    let renewed: Arc<StdMutex<Option<(bool, Option<task_core::Lease>)>>> = Arc::default();
    let (slot, hs, a) = (renewed.clone(), holder_store.clone(), run_a.clone());
    let mut reclaimer = dispatcher(reclaim_store.clone(), slow_adapter(), 1);
    reclaimer.test_hooks.after_reclaim_decision = Some(Box::new(move |task_id| {
        // 回収を決めた直後に、保持者の接続が lease を延ばす。
        let ok = hs
            .renew_lease(task_id, &a, Duration::from_secs(3600))
            .unwrap();
        let lease = hs.get(task_id).unwrap().and_then(|t| t.lease);
        *slot.lock().unwrap() = Some((ok, lease));
    }));
    let report = reclaimer.tick().unwrap();

    let decided = renewed.lock().unwrap().clone();
    let Some((ok, Some(extended))) = decided else {
        panic!(
            "the reclaimer did not decide to reclaim the expired lease, or the lease vanished: {decided:?}"
        );
    };
    assert!(
        ok,
        "the holder's renew_lease must succeed before the reclaim writes"
    );
    assert_eq!(extended.worker_run_id, run_a);
    let t = reclaim_store.get(id).unwrap().unwrap();
    let after: Vec<Event> = events_of(&reclaim_store, id).split_off(events_before);
    let observed = format!(
        "report {report:?}; status {:?}; attempts {} (before {attempts}); lease {:?} (extended {extended:?}); \
         A's runs row {:?}; events after the expiry {after:?}",
        t.status,
        t.attempts,
        t.lease,
        index_status(&reclaim_store, &run_a),
    );
    assert_eq!(report.reclaimed, 0, "{observed}");
    assert_eq!(t.status, Status::Running, "{observed}");
    assert_eq!(t.lease.as_ref(), Some(&extended), "{observed}");
    assert_eq!(t.attempts, attempts, "{observed}");
    assert_eq!(
        index_status(&reclaim_store, &run_a),
        Some(task_core::RunIndexStatus::Running),
        "{observed}"
    );
    assert!(
        finishes(&reclaim_store, id, &run_a).is_empty(),
        "{observed}"
    );
    assert!(!took_over(&reclaim_store, id), "{observed}");
    assert!(
        !after
            .iter()
            .any(|e| matches!(e, Event::Transitioned { .. })),
        "{observed}"
    );

    // 保持者は A を持ち続け、A の通常の完了で 1 回だけ閉じる。
    gated.release_one();
    let s = holder_store.clone();
    assert!(run_until_state(&mut holder, || status_of(&s, id) == Status::Done).await);
    assert_eq!(finishes(&holder_store, id, &run_a).len(), 1);
    assert_eq!(worker_starts(&holder_store, id), vec![run_a]);
}
