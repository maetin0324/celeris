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
    let wait = &d.write_set_waits[&RunKey {
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

// ---- 2026-10-03-write-set-no-starvation: 兄弟 WU の走査と容量切れの tick の公平性 ----

/// run ごとに `release(run_id)` されるまで終わらないアダプタ（どの run を終わらせるかを試験が選ぶ）。
struct KeyedHoldAdapter {
    gates: StdMutex<HashMap<String, Arc<tokio::sync::Semaphore>>>,
}

impl KeyedHoldAdapter {
    fn new() -> Arc<Self> {
        Arc::new(KeyedHoldAdapter {
            gates: StdMutex::new(HashMap::new()),
        })
    }

    fn gate(&self, run_id: &str) -> Arc<tokio::sync::Semaphore> {
        self.gates
            .lock()
            .unwrap()
            .entry(run_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(0)))
            .clone()
    }

    fn release(&self, run_id: &str) {
        self.gate(run_id).add_permits(1);
    }
}

#[async_trait]
impl WorkerAdapter for KeyedHoldAdapter {
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
        if let Ok(permit) = self.gate(run_id).acquire().await {
            permit.forget();
        }
        Ok(done_outcome())
    }
}

fn hinted_leaf(key: &str, paths: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "key": key,
        "stage": "build",
        "kind": "implement",
        "title": format!("Leaf {key}"),
        "objective": format!("Implement {key} thoroughly and completely"),
        "depends_on": [],
        "done_when": [format!("{key} is done")],
        "checks": [{"cmd": "true", "expect_exit": 0}],
        "expected_write_paths": paths,
    })
}

/// 容量（`max_concurrency = 3`）が満ちた状態で、(1) write-set で待たされた WU `a` の後ろの非重複 WU `b` が同じ
/// tick に走り、(2) 容量切れの tick に `a` の待機の数えが戻らず、(3) 毎 tick 後から来る優先度の高い重なる
/// ready task に追い越されず、先行の run が終わった最初の tick に `a` が走る。時刻は dispatcher の tick だけ
/// （試験が tick を回す。実時間の sleep・CPU 負荷なし）、run の終了は run ごとの semaphore で明示的に起こす。
#[tokio::test]
async fn write_set_gate_no_starvation() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let check = || Check::Command {
        cmd: "true".into(),
        expect_exit: 0,
    };
    let hinted = |paths: &[&str], priority: i64| {
        let mut task = git_task(repo.path(), None, check());
        task.priority = priority as _;
        store.insert(&task).unwrap();
        let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        store
            .set_task_expected_write_paths(task.id, Some(&paths), "2026-10-03T00:00:00Z")
            .unwrap();
        task
    };
    // 先行の run（a・c と強く重なる）。優先度が高いので tick 1 に先に起きる。
    let x = hinted(&["crates/core/"], 5);
    // 並列 WU の task（seq は key 順）: k0 = a0（非重複）→ k1 = a（x と重なる）→ k2 = b（非重複）→
    // k3 = c（x・a と重なる）。
    let t = parallel_task(repo.path(), "true");
    store.insert(&t).unwrap();
    let plan = tree::v3_plan(
        vec![tree::stage("build", false)],
        vec![
            hinted_leaf("k0", &["tests/"]),
            hinted_leaf("k1", &["crates/core/a.rs"]),
            hinted_leaf("k2", &["docs/"]),
            hinted_leaf("k3", &["crates/core/"]),
        ],
    );
    task_ops::execution::adopt_plan(
        store.as_ref(),
        t.id,
        serde_json::from_str(&plan).unwrap(),
        task_core::PlanOrigin::Fixture,
        None,
        tree::tree_limits(),
        OffsetDateTime::now_utc(),
    )
    .expect("adopt plan");
    let wu_key = |key: &str| RunKey {
        task: t.id,
        work_unit: Some(
            tree::unit(&store.work_units_for(t.id).unwrap(), key)
                .id
                .clone(),
        ),
    };
    let (a0, a, b, c) = (wu_key("k0"), wu_key("k1"), wu_key("k2"), wu_key("k3"));
    let x_key = RunKey {
        task: x.id,
        work_unit: None,
    };
    let adapter = KeyedHoldAdapter::new();
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 3, 3, 3);
    d.config.execution.limits = tree::tree_limits();

    // tick 1: x と a0 が起き、a は x に待たされる。a の後ろの非重複 b は同じ tick に走る（走査が a で止まらない）。
    d.tick().unwrap();
    assert!(d.running.contains_key(&x_key));
    assert!(d.running.contains_key(&a0));
    assert!(!d.running.contains_key(&a), "a overlaps x and waits");
    assert!(
        d.running.contains_key(&b),
        "the disjoint sibling behind the held WU runs in the same tick"
    );
    assert!(!d.running.contains_key(&c));
    assert_eq!(d.running.len(), 3, "max_concurrency is now full");
    let first = d.write_set_waits[&a].clone();
    assert_eq!(first.consecutive, 1);

    // tick 2..=5: 容量は満ちたまま。毎 tick、優先度の高い重なる ready task が後から来る。容量切れの tick でも
    // a の待機は数え直されず（since_tick を保ったまま）連続回数が積み上がる。
    let mut newcomers = Vec::new();
    for n in 2..=5u32 {
        newcomers.push(hinted(&["crates/core/"], 9));
        d.tick().unwrap();
        assert!(!d.running.contains_key(&a));
        let wait = &d.write_set_waits[&a];
        assert_eq!(
            wait.since_tick, first.since_tick,
            "a capacity-full tick does not reset the wait"
        );
        assert_eq!(wait.consecutive, n);
    }
    assert!(
        d.write_set_waits[&a].consecutive
            >= super::super::write_set_gate::WRITE_SET_STARVATION_TICKS
    );

    // x が終わった最初の tick（tick 6）に、長く待った a が走る。後から来た重なる ready task は a の先取りに
    // 止められ、c も a の後ろに並ぶ（先に待ち始めた a が先）。
    let x_run = d.running[&x_key].run_id.clone();
    adapter.release(&x_run);
    (&mut d.running.get_mut(&x_key).expect("x in flight").handle)
        .await
        .expect("worker task panicked");
    d.tick().unwrap();
    assert!(!d.running.contains_key(&x_key));
    assert!(
        d.running.contains_key(&a),
        "the starved WU runs within a bounded number of ticks"
    );
    assert!(!d.write_set_waits.contains_key(&a));
    assert!(!d.running.contains_key(&c));
    for y in &newcomers {
        assert!(
            !is_running(&d, y.id),
            "a newer overlapping task does not overtake a"
        );
        ready_untouched(&store, y.id);
    }
    assert_eq!(d.ticks, 6, "a ran in tick 6 = (held at tick 1) + 5");
}
