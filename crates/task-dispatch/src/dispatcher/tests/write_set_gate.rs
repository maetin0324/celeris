//! ADR-0130 D3: 同じ repo の expected write-set が強く重なる run は次 tick に回す（待たせるだけ）。
//! 非重複・hint なしは従来どおり並列に起動する。tick は固定回数で回し、run の終了は
//! semaphore で明示的に起こす（時間に依存しない）。

use super::*;

/// `release` に permit が足されるまで終わらないアダプタ（起動した run の数を数える）。
struct HoldAdapter {
    release: tokio::sync::Semaphore,
    started: AtomicUsize,
}

impl HoldAdapter {
    fn new() -> Arc<Self> {
        Arc::new(HoldAdapter {
            release: tokio::sync::Semaphore::new(0),
            started: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl WorkerAdapter for HoldAdapter {
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
        self.started.fetch_add(1, Ordering::SeqCst);
        if let Ok(permit) = self.release.acquire().await {
            permit.forget();
        }
        Ok(done_outcome())
    }
}

fn hinted_task(
    store: &Arc<dyn TaskStore>,
    dir: &std::path::Path,
    hint: Option<&[&str]>,
    priority: i64,
) -> Task {
    let mut task = new_task(
        dir,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    task.priority = priority as _;
    store.insert(&task).unwrap();
    if let Some(paths) = hint {
        let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        store
            .set_task_expected_write_paths(task.id, Some(&paths), "2026-10-02T00:00:00Z")
            .unwrap();
    }
    task
}

fn is_running(d: &Dispatcher, id: TaskId) -> bool {
    d.running.contains_key(&RunKey {
        task: id,
        work_unit: None,
    })
}

fn ready_untouched(store: &Arc<dyn TaskStore>, id: TaskId) {
    let task = store.get(id).unwrap().unwrap();
    assert_eq!(task.status, Status::Ready, "held task stays ready");
    assert_eq!(task.attempts, 0, "a write-set hold consumes no attempt");
    assert!(
        transition_reasons(store, id).is_empty(),
        "a write-set hold makes no transition"
    );
}

/// 走っている run を 1 本終わらせ、その worker の後処理まで待つ。
async fn finish_one(d: &mut Dispatcher, adapter: &HoldAdapter, id: TaskId) {
    adapter.release.add_permits(1);
    let key = RunKey {
        task: id,
        work_unit: None,
    };
    (&mut d.running.get_mut(&key).expect("run is in flight").handle)
        .await
        .expect("worker task panicked");
}

#[tokio::test]
async fn write_set_gate_runs_disjoint_hints_in_parallel() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let a = hinted_task(&store, dir.path(), Some(&["src/a.rs"]), 0);
    let b = hinted_task(&store, dir.path(), Some(&["src/ab.rs", "docs/"]), 0);
    let adapter = HoldAdapter::new();
    let mut d = dispatcher(store.clone(), adapter.clone(), 2);

    let report = d.tick().unwrap();
    assert_eq!(report.dispatched, 2);
    assert!(is_running(&d, a.id) && is_running(&d, b.id));
    assert!(d.write_set_waits.is_empty());

    adapter.release.add_permits(2);
    run_until_idle(&mut d, 200).await;
    assert_eq!(adapter.started.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn write_set_gate_serializes_strong_overlap_until_the_first_run_finishes() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let a = hinted_task(&store, dir.path(), Some(&["crates/task-core/"]), 1);
    let b = hinted_task(&store, dir.path(), Some(&["crates/task-core/src/x.rs"]), 0);
    let adapter = HoldAdapter::new();
    let mut d = dispatcher(store.clone(), adapter.clone(), 2);

    // 固定 tick: 先行の a だけが走り、b は ready のまま（遷移も attempts もなし）。
    for _ in 0..4 {
        d.tick().unwrap();
        assert!(is_running(&d, a.id));
        assert!(!is_running(&d, b.id));
        ready_untouched(&store, b.id);
    }
    assert_eq!(d.running.len(), 1);
    let wait = d.write_set_waits[&RunKey {
        task: b.id,
        work_unit: None,
    }];
    assert_eq!(wait.consecutive, 4);

    // 先行が終わった tick で予約が外れ、同じ tick で後続が起動する。
    finish_one(&mut d, &adapter, a.id).await;
    let report = d.tick().unwrap();
    assert_eq!(report.finished, 1);
    assert!(!is_running(&d, a.id));
    assert!(is_running(&d, b.id));
    assert_eq!(store.get(b.id).unwrap().unwrap().attempts, 0);
    assert!(d.write_set_waits.is_empty());

    adapter.release.add_permits(1);
    run_until_idle(&mut d, 200).await;
    assert_eq!(store.get(a.id).unwrap().unwrap().status, Status::Done);
    assert_eq!(store.get(b.id).unwrap().unwrap().status, Status::Done);
    assert_eq!(adapter.started.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn write_set_gate_without_hints_keeps_the_previous_parallelism() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    // 両方 hint なし、片方だけ hint あり、の 2 組。どちらも従来どおり並列に起動する。
    let a = hinted_task(&store, dir.path(), None, 0);
    let b = hinted_task(&store, dir.path(), None, 0);
    let c = hinted_task(&store, dir.path(), Some(&["src/"]), 0);
    let adapter = HoldAdapter::new();
    let mut d = dispatcher(store.clone(), adapter.clone(), 3);

    let report = d.tick().unwrap();
    assert_eq!(report.dispatched, 3);
    assert!(is_running(&d, a.id) && is_running(&d, b.id) && is_running(&d, c.id));
    assert!(d.write_set_waits.is_empty());

    adapter.release.add_permits(3);
    run_until_idle(&mut d, 200).await;
}

#[tokio::test]
async fn write_set_gate_ignores_overlap_in_another_repo() {
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let a = hinted_task(&store, dir_a.path(), Some(&["src/"]), 0);
    let b = hinted_task(&store, dir_b.path(), Some(&["src/"]), 0);
    let adapter = HoldAdapter::new();
    let mut d = dispatcher(store.clone(), adapter.clone(), 2);

    assert_eq!(d.tick().unwrap().dispatched, 2);
    assert!(is_running(&d, a.id) && is_running(&d, b.id));

    adapter.release.add_permits(2);
    run_until_idle(&mut d, 200).await;
}

/// 連続 3 回以上待たされた候補は、後から来た優先度の高い重なる候補より先に起こす（飢餓させない）。
#[tokio::test]
async fn write_set_gate_wakes_the_longest_waiter_before_a_newer_higher_priority_task() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let a = hinted_task(&store, dir.path(), Some(&["src/"]), 5);
    let b = hinted_task(&store, dir.path(), Some(&["src/lib.rs"]), 0);
    let adapter = HoldAdapter::new();
    let mut d = dispatcher(store.clone(), adapter.clone(), 2);
    for _ in 0..3 {
        d.tick().unwrap();
    }
    assert!(is_running(&d, a.id) && !is_running(&d, b.id));
    ready_untouched(&store, b.id);

    // 後から来た、優先度の高い（ready_tasks では先頭の）重なる候補。
    let c = hinted_task(&store, dir.path(), Some(&["src/lib.rs"]), 9);
    d.tick().unwrap();
    assert!(!is_running(&d, c.id));

    finish_one(&mut d, &adapter, a.id).await;
    d.tick().unwrap();
    assert!(is_running(&d, b.id), "the starved waiter goes first");
    assert!(!is_running(&d, c.id));
    ready_untouched(&store, c.id);

    adapter.release.add_permits(2);
    run_until_idle(&mut d, 300).await;
    assert_eq!(store.get(c.id).unwrap().unwrap().status, Status::Done);
}

#[test]
fn write_set_gate_same_task_siblings_with_only_the_inherited_hint_do_not_block() {
    use super::super::write_set_gate::reservations_overlap;
    let task = TaskId::new();
    let other = TaskId::new();
    let inherited =
        super::super::write_set_gate::WriteReservation::for_test(&["repo:r"], &["src/"], true);
    let own =
        super::super::write_set_gate::WriteReservation::for_test(&["repo:r"], &["src/a.rs"], false);
    assert!(!reservations_overlap(task, &inherited, task, &inherited));
    assert!(reservations_overlap(task, &inherited, task, &own));
    assert!(reservations_overlap(task, &inherited, other, &inherited));
}
