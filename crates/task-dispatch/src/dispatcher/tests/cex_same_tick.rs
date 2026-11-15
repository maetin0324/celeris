//! 健全性v2 R4 cex-tick: 同じ tick に届く run の Done と lease の期限、先に確定した人の操作の反例試験
//! （`agent-docs/progress/2026-11-11-health-v2-r4/cex-tick.md`、ADR 2026-11-11-dispatcher-state-ownership D7 の
//! 同時完了の行）。
//!
//! 規約（`cex_review_authority.rs`・`cex_lease_restart.rs` と同じ）: 試験は Dispatcher の private field に触らず
//! （`test_hooks` の既存の注入時計だけを使う）、store の行（task・lease・`runs` 索引・instance）と event と `tick()` の
//! 戻り値（`TickReport`）だけを見る。待ちは出来事待ち（adapter の通知と store の event。保険は [`STATE_WAIT_GUARD`]
//! の壁時計）で、tick の回数で待たない。run の終わりは fake adapter の gate（semaphore）で試験が決め、lease の失効は
//! `renew_lease(.., 0)` で store の lease 時刻を今にして作る（実時計を進めない）。CPU 負荷は使わない。
use super::orphan_takeover::{SELF_ID, instance};
use super::*;
use crate::orphan::{ORPHAN_TAKEOVER_REASON, OrphanTakeover};

/// worker run を試験が gate を開けるまで止め、開いたら成果物の markdown を 1 つ書いて `Done` を返す adapter
/// （run ごとに permit を 1 つ消費する）。`returned` は返る直前に鳴る。
struct DoneGateAdapter {
    started: StdMutex<Vec<String>>,
    gate: tokio::sync::Semaphore,
    returned: tokio::sync::Notify,
}

/// worker run が `artifacts_dir` に書く、未申告の成果物（run_worker の走査が `ArtifactProduced` を積む）。
const NOTE: &str = "same-tick-note.md";

impl DoneGateAdapter {
    fn new() -> Arc<Self> {
        Arc::new(DoneGateAdapter {
            started: StdMutex::new(Vec::new()),
            gate: tokio::sync::Semaphore::new(0),
            returned: tokio::sync::Notify::new(),
        })
    }

    fn started(&self) -> Vec<String> {
        self.started.lock().unwrap().clone()
    }
}

#[async_trait]
impl WorkerAdapter for DoneGateAdapter {
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
        self.started.lock().unwrap().push(run_id.to_string());
        if let Ok(permit) = self.gate.acquire().await {
            permit.forget();
        }
        std::fs::create_dir_all(&req.artifacts_dir).unwrap();
        std::fs::write(req.artifacts_dir.join(NOTE), format!("run {run_id}\n")).unwrap();
        self.returned.notify_one();
        Ok(done_outcome())
    }
}

/// 自分の instance の行を登録し、孤児の回収を有効にし、注入時計（`test_hooks.now`）を持つ dispatcher
/// （`orphan_takeover::new_active` と同じ構成に、周期の照合を進めるための時計を足したもの）。
fn active_with_clock(store: &Arc<dyn TaskStore>, adapter: Arc<dyn WorkerAdapter>) -> Dispatcher {
    store
        .instance_register(&instance(SELF_ID, InstanceRole::Active, std::process::id()))
        .unwrap();
    let mut d = dispatcher_with_test_clock(store.clone(), adapter, 1, true);
    d.set_orphan_takeover(OrphanTakeover {
        instance_id: SELF_ID.into(),
        freshness: Duration::from_secs(60),
        pid_alive: Arc::new(crate::orphan::proc_pid_alive),
    });
    d
}

fn status_of(store: &Arc<dyn TaskStore>, id: TaskId) -> Status {
    store.get(id).unwrap().unwrap().status
}

/// `run_id` の `WorkerFinished` の `(end, outcome)`（event の数 = 長さ）。
fn finishes(store: &Arc<dyn TaskStore>, id: TaskId, run_id: &str) -> Vec<(Option<RunEnd>, String)> {
    events_of(store, id)
        .into_iter()
        .filter_map(|e| match e {
            Event::WorkerFinished {
                run_id: r,
                end,
                outcome,
                ..
            } if r == run_id => Some((end, outcome)),
            _ => None,
        })
        .collect()
}

/// `Transitioned` の `(to, reason)`（起きた順）。
fn transitions(store: &Arc<dyn TaskStore>, id: TaskId) -> Vec<(Status, String)> {
    events_of(store, id)
        .into_iter()
        .filter_map(|e| match e {
            Event::Transitioned { to, reason, .. } => Some((to, reason)),
            _ => None,
        })
        .collect()
}

/// `WorkerStarted` の `(run_id, role)`（起きた順）。
fn starts(store: &Arc<dyn TaskStore>, id: TaskId) -> Vec<(String, Option<RunRole>)> {
    events_of(store, id)
        .into_iter()
        .filter_map(|e| match e {
            Event::WorkerStarted { run_id, role, .. } => Some((run_id, role)),
            _ => None,
        })
        .collect()
}

/// 孤児の回収の記録（`orphan_takeover` の `WorkerProgress` と、`orphan_takeover` で閉じた `WorkerFinished`）の数。
fn orphan_records(store: &Arc<dyn TaskStore>, id: TaskId) -> usize {
    events_of(store, id)
        .iter()
        .filter(|e| match e {
            Event::WorkerProgress { msg, .. } => msg.starts_with(ORPHAN_TAKEOVER_REASON),
            Event::WorkerFinished { outcome, .. } => outcome.contains(ORPHAN_TAKEOVER_REASON),
            _ => false,
        })
        .count()
}

fn note_recorded(store: &Arc<dyn TaskStore>, id: TaskId, run: &str) -> bool {
    events_of(store, id).iter().any(|e| {
        matches!(e, Event::ArtifactProduced { run_id, artifact }
            if run_id == run && artifact.path.ends_with(NOTE))
    })
}

/// gate を開け、run `run` の完了が完了の channel に載るまで待つ（dispatcher を tick しない。出来事待ち）。
///
/// adapter の通知（`returned`）で run が返ったことを確かめた後、`run_worker` が返る前に書く `ArtifactProduced`
/// （未申告の成果物の走査。この append から `Completion::Worker` の送信まで await が無い）を store で待つ。
/// adapter が返った後の `run_worker` は result.json の誤配置の確認（`tokio::fs`）で await するので、通知だけでは
/// 送信済みと言えない。current-thread の runtime では、試験の task がこの event を見た時点で worker の task は
/// 送信を終えている。
async fn release_until_completion_sent(
    adapter: &DoneGateAdapter,
    store: &Arc<dyn TaskStore>,
    id: TaskId,
    run: &str,
) {
    adapter.gate.add_permits(1);
    tokio::time::timeout(STATE_WAIT_GUARD, adapter.returned.notified())
        .await
        .expect("the gated worker returned");
    let deadline = Instant::now() + STATE_WAIT_GUARD;
    while !note_recorded(store, id, run) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        note_recorded(store, id, run),
        "run_worker did not reach the completion send of {run}"
    );
}

/// 不変条件（lease の単一保持者 ADR-0002 D9・ADR-0005 D4、終端の task の `runs` 行の閉じ ADR-0079 付記「R6-1」D4、
/// ADR 2026-11-11-dispatcher-state-ownership D7 の同時完了の行）: 人の cancel が確定した後で、run A の Done と
/// A の lease の期限切れを同じ 1 回の tick が処理しても（`drain_completions` が Done を受け、同じ tick の
/// `reclaim_expired_leases` が回収を判断する）、A は 1 回だけ閉じる。`WorkerFinished` は cancel の遷移が積んだ
/// `end: cancelled` の 1 件だけで、`runs` 行は cancelled のまま `finished_at` も変わらない。Done は Task を
/// Reviewing・Done に進めず、回収は `WorkerFinished{lease_expired}` も `orphan_takeover` の記録も書かない。Task は
/// Cancelled のまま lease を持たず、その後の tick（周期の照合〈終端の task の `runs` 行・持ち主の居ない run〉を
/// 注入時計で回したものを含む）でも worker・reviewer の run を起こさない。
///
/// 作る順序: A を gate で止める → store の lease 時刻を今にする（`renew_lease(.., 0)`。`reclaim_expired_leases` は
/// 注入時計を見ず `OffsetDateTime::now_utc()` を読むので、期限は store の時刻で作る。cancel は同じトランザクションで
/// lease を外すので、期限は cancel の前に作る。cancel の後の `renew_lease` は false を返す）→ 人の cancel を確定する
/// （`apply_transition(Cancel)`）→ gate を開け、A の Done が完了の channel に載ったことを出来事待ちで確かめる
/// （[`release_until_completion_sent`]）→ tick を 1 回呼ぶ（`finished == 1`・`reclaimed == 0`・`dispatched == 0`）。
///
/// 既存試験で検出できない理由: `cex_human_cancel_precedes_successful_worker_completion`（`cex_review_authority.rs`）は
/// cancel の後の完了を `run_until_state` の tick の繰り返しで受けるので、完了がどの tick に届くかを決めず、lease の
/// 期限も作らない（Done と回収の判断が同じ tick に揃わない）。`WorkerFinished` の件数・`runs` 行の閉じの回数・
/// `orphan_takeover` の記録も見ない。`cex_lease_restart.rs` の lease の試験
/// （`cex_stale_holder_late_finish_does_not_overwrite_the_reassigned_run`・
/// `cex_draining_instance_reclaims_its_expired_lease_without_redispatching`・
/// `cex_lease_renew_after_expired_snapshot_keeps_the_holder`）は人の cancel を挟まず、回収が先に lease を取る順序だけを
/// 作る。`cex_orphan_takeover_leaves_the_run_held_in_this_instance` は lease の残る run を tick させるだけで、Done も
/// cancel も同じ tick に置かない。どれも「cancel の確定 → Done が channel に溜まる → 期限切れ → 1 回の tick」の順を
/// 作らない。
#[tokio::test]
async fn cex_done_and_lease_expiry_in_one_tick_close_the_run_once() {
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
    let id = task.id;
    store.insert(&task).unwrap();
    let adapter = DoneGateAdapter::new();
    let mut d = active_with_clock(&store, adapter.clone());
    let (s, a) = (store.clone(), adapter.clone());
    assert!(
        run_until_state(&mut d, || a.started().len() == 1
            && s.get(id).unwrap().unwrap().lease.is_some())
        .await
    );
    let run_a = store.get(id).unwrap().unwrap().lease.unwrap().worker_run_id;
    assert_eq!(adapter.started(), vec![run_a.clone()]);
    assert_eq!(
        store.run_index_get(&run_a).unwrap().map(|r| r.status),
        Some(task_core::RunIndexStatus::Running)
    );

    // A の lease の期限を今にする（Running の間だけ書ける）。続けて人の cancel を確定する（await を挟まないので
    // worker の lease の更新は割り込まない）。
    assert!(store.renew_lease(id, &run_a, Duration::ZERO).unwrap());
    let expires = store.get(id).unwrap().unwrap().lease.unwrap().expires_at;
    assert!(expires <= OffsetDateTime::now_utc());
    store.apply_transition(id, Trigger::Cancel, None).unwrap();
    assert_eq!(status_of(&store, id), Status::Cancelled);
    assert!(store.get(id).unwrap().unwrap().lease.is_none());
    assert!(!store.renew_lease(id, &run_a, Duration::ZERO).unwrap());
    // cancel の遷移が同じトランザクションで A の `runs` 行を閉じた（R6-1）。これが A の唯一の閉じになる。
    let closed_by_cancel = finishes(&store, id, &run_a);
    assert_eq!(closed_by_cancel.len(), 1, "{closed_by_cancel:?}");
    assert_eq!(closed_by_cancel[0].0, Some(RunEnd::Cancelled));
    let row = store.run_index_get(&run_a).unwrap().unwrap();
    assert_eq!(row.status, task_core::RunIndexStatus::Cancelled);
    let closed_at = row.finished_at;
    assert!(closed_at.is_some());
    let transitions_before = transitions(&store, id);
    assert_eq!(
        transitions_before.last(),
        Some(&(Status::Cancelled, "cancel".to_string()))
    );

    // A の Done を完了の channel に載せる（tick しない）。
    release_until_completion_sent(&adapter, &store, id, &run_a).await;

    // 1 回の tick が Done（`drain_completions`）と回収の判断（`reclaim_expired_leases`）の両方を処理する。
    let report = d.tick().unwrap();
    assert_eq!(report.finished, 1, "{report:?}");
    assert_eq!(report.reclaimed, 0, "{report:?}");
    assert_eq!(report.dispatched, 0, "{report:?}");
    assert_eq!(report.in_flight, 0, "{report:?}");

    let assert_closed_once = |store: &Arc<dyn TaskStore>, when: &str| {
        assert_eq!(
            finishes(store, id, &run_a),
            closed_by_cancel,
            "A was closed again {when}"
        );
        let row = store.run_index_get(&run_a).unwrap().unwrap();
        assert_eq!(
            row.status,
            task_core::RunIndexStatus::Cancelled,
            "A's runs row {when}"
        );
        assert_eq!(row.finished_at, closed_at, "A's runs row reclosed {when}");
        assert_eq!(orphan_records(store, id), 0, "orphan takeover {when}");
        assert_eq!(status_of(store, id), Status::Cancelled, "{when}");
        assert!(store.get(id).unwrap().unwrap().lease.is_none(), "{when}");
        assert_eq!(transitions(store, id), transitions_before, "{when}");
        assert_eq!(
            starts(store, id),
            vec![(run_a.clone(), None)],
            "no worker or reviewer run started {when}"
        );
        assert!(store.delivery_get(id).unwrap().is_none(), "{when}");
        assert!(store.runs_running().unwrap().is_empty(), "{when}");
    };
    assert_closed_once(&store, "in the same tick");

    // その後の tick: 周期の照合（終端の task の `runs` 行・持ち主の居ない run）が回るよう注入時計を進める。
    if let Some(clock) = &d.test_hooks.now {
        let mut now = clock.lock().unwrap();
        *now += time::Duration::seconds(crate::dispatcher::RUNS_RECONCILE_INTERVAL_SECS + 1);
    }
    for _ in 0..3 {
        let report = d.tick().unwrap();
        assert_eq!(report.dispatched, 0, "{report:?}");
        assert_eq!(report.finished, 0, "{report:?}");
        assert_eq!(report.reclaimed, 0, "{report:?}");
        assert_eq!(report.reviewed, 0, "{report:?}");
        assert_closed_once(&store, "in a later tick");
    }
}
