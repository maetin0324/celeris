//! ADR 2026-11-11-disk-aware-dispatch D1: dispatch の前の空きの見込み。空きと target の大きさは
//! `ScratchState.admission.probe` に注入した値で与える（大きな file は作らない）。
//!
//! 守る不変条件: 見込みが予備を割ると新しい target を作る run は `ready` のまま待ち（attempts を消費しない）、空きが
//! 戻れば始まる。同じ task の続き（owner の target がある）は増分だけで判定する。target を今書いている run が 0 本なら
//! 詰まらない。通した run は lease が走査に出るまで（本番で多くは 10〜30 秒）見込みに数え続ける。
//! 待たせた理由は待ち始めに 1 回だけ残り、通知・受信箱には出ない。既存の `cleanup_and_disk` の試験は今の空きの
//! 全体停止（`check_disk_space`）だけを見ていて、走っている target の増分と新しい target の見込みを検出できない。

use super::*;
use crate::disk_admission::{Inputs, Sample};
use std::sync::Mutex;

const GB: u64 = 1_000_000_000;

/// Rust の checkout（`Cargo.toml` がある）を作業場所に持つ task。
fn rust_task(root: &std::path::Path, name: &str) -> (Task, String) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
    let task = new_task(
        &dir,
        Check::Command {
            cmd: ":".into(),
            expect_exit: 0,
        },
        0,
    );
    (task, task_worker::build_cache::repo_cache_key(&dir))
}

struct Fixture {
    d: Dispatcher,
    store: Arc<dyn TaskStore>,
    inputs: Arc<Mutex<Inputs>>,
    _root: tempfile::TempDir,
}

fn fixture(free: u64, samples: Vec<Sample>) -> Fixture {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    // 走り続ける run（通した run が走っている状態を保つ）。
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: String::new(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::from_secs(3600),
    });
    let mut d = dispatcher(store.clone(), adapter, 4);
    d.config.min_free_disk_pct = 0.0;
    let inputs = Arc::new(Mutex::new(Inputs {
        free_bytes: Some(free),
        reserve_bytes: 30 * GB,
        ratio: 1.0,
        samples,
    }));
    let probe = inputs.clone();
    d.scratch.admission.probe = Some(Arc::new(move || probe.lock().unwrap().clone()));
    Fixture {
        d,
        store,
        inputs,
        _root: tempfile::tempdir().unwrap(),
    }
}

fn sample(owner: &str, repo: &str, grows: bool, size: u64) -> Sample {
    Sample {
        owner: owner.into(),
        repo_key: repo.into(),
        grows,
        size_bytes: Some(size),
    }
}

fn status(store: &Arc<dyn TaskStore>, id: TaskId) -> Status {
    store.get(id).unwrap().unwrap().status
}

#[tokio::test]
async fn a_new_target_waits_ready_until_the_forecast_fits_and_the_reason_is_noted_once() {
    let mut f = fixture(150 * GB, Vec::new());
    let root = f._root.path().to_path_buf();
    let (a, repo) = rust_task(&root, "a");
    let (b, _) = rust_task(&root, "a-copy");
    // 同じ repo（同じ path）の 2 本目の task。
    let b = Task {
        workspace: a.workspace.clone(),
        ..b
    };
    f.inputs.lock().unwrap().samples = vec![sample("task-old", &repo, false, 100 * GB)];
    f.store.insert(&a).unwrap();
    f.d.tick().unwrap();
    // target を書いている run が 0 本なので、見込み 100 GB でも 1 本目は通す。
    assert_eq!(status(&f.store, a.id), Status::Running);
    f.store.insert(&b).unwrap();
    // 走っている a の target（65 GB）は 35 GB 増える見込み: 150 − 35 − 100 = 15 < 30。
    f.inputs.lock().unwrap().samples = vec![
        sample("task-old", &repo, false, 100 * GB),
        sample(&format!("task-{}", a.id), &repo, true, 65 * GB),
    ];
    for _ in 0..3 {
        assert_eq!(f.d.tick().unwrap().dispatched, 0);
        assert_eq!(status(&f.store, b.id), Status::Ready);
    }
    assert_eq!(f.store.get(b.id).unwrap().unwrap().attempts, 0);
    assert_eq!(f.d.scratch.admission.notes(), 1, "the hold is noted once");
    let held = f.d.scratch.admission.held_views();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].task_id, b.id.to_string());
    assert_eq!(held[0].need_bytes, 100 * GB);
    assert!(held[0].reason.contains("reserve"), "{}", held[0].reason);
    // 受信箱・通知には出さない。
    assert_eq!(
        f.store
            .notice_list(&task_core::feed::NoticeQuery::default())
            .unwrap()
            .total,
        0
    );
    // 空きが戻れば始まる（記録は次の pass で消える）。
    f.inputs.lock().unwrap().free_bytes = Some(200 * GB);
    assert_eq!(f.d.tick().unwrap().dispatched, 1);
    assert_eq!(status(&f.store, b.id), Status::Running);
    f.d.tick().unwrap();
    assert!(f.d.scratch.admission.held_views().is_empty());
}

#[tokio::test]
async fn a_continuation_is_judged_by_its_increment_only() {
    let mut f = fixture(45 * GB, Vec::new());
    let root = f._root.path().to_path_buf();
    let (running, repo) = rust_task(&root, "r");
    f.store.insert(&running).unwrap();
    f.d.tick().unwrap();
    assert_eq!(status(&f.store, running.id), Status::Running);
    let (cont, _) = rust_task(&root, "c");
    let cont = Task {
        workspace: running.workspace.clone(),
        ..cont
    };
    let (fresh, _) = rust_task(&root, "f");
    let fresh = Task {
        workspace: running.workspace.clone(),
        ..fresh
    };
    // 続きの task は自分の target（92 GB）を持つ: 増分 8 GB。45 − 0 − 8 = 37 ≥ 30 で通す。
    // 新しい task は 100 GB 全体: 45 − 8 − 100 < 30 で待つ。
    f.inputs.lock().unwrap().samples = vec![
        sample("task-old", &repo, false, 100 * GB),
        sample(&format!("task-{}", running.id), &repo, true, 100 * GB),
        sample(&format!("task-{}", cont.id), &repo, false, 92 * GB),
    ];
    f.store.insert(&cont).unwrap();
    f.store.insert(&fresh).unwrap();
    f.d.tick().unwrap();
    assert_eq!(status(&f.store, cont.id), Status::Running);
    assert_eq!(status(&f.store, fresh.id), Status::Ready);
    assert_eq!(f.d.scratch.admission.notes(), 1);
}

#[tokio::test]
async fn the_only_run_is_never_held_and_unknown_repos_are_not_judged() {
    // 空きは予備を大きく割っているが、target を書いている run が 0 本なので 1 本は通す。
    let mut f = fixture(5 * GB, Vec::new());
    let root = f._root.path().to_path_buf();
    let (a, repo) = rust_task(&root, "a");
    f.inputs.lock().unwrap().samples = vec![sample("task-old", &repo, false, 100 * GB)];
    f.store.insert(&a).unwrap();
    f.d.tick().unwrap();
    assert_eq!(status(&f.store, a.id), Status::Running);
    // 測った target の無い repo の task は判定しない（Rust でない・初めての repo）。
    let (other, _) = rust_task(&root, "other");
    f.store.insert(&other).unwrap();
    f.d.tick().unwrap();
    assert_eq!(status(&f.store, other.id), Status::Running);
    assert_eq!(f.d.scratch.admission.notes(), 0);
}

/// attempt 5 の review の指摘: 同じ tick にまとめて来た run のうち、見込みに入る分だけが始まり、通した run の lease が
/// 走査に出るまでの tick でも残りは待ち続ける（通した量を pass ごとに 0 に戻すと次の tick で通ってしまう）。
#[tokio::test]
async fn runs_admitted_in_one_tick_keep_their_forecast_until_their_lease_appears() {
    let mut f = fixture(250 * GB, Vec::new());
    let root = f._root.path().to_path_buf();
    let (a, repo) = rust_task(&root, "a");
    f.inputs.lock().unwrap().samples = vec![sample("task-old", &repo, false, 100 * GB)];
    let mut ids = vec![a.id];
    f.store.insert(&a).unwrap();
    for name in ["b", "c"] {
        let (t, _) = rust_task(&root, name);
        let t = Task {
            workspace: a.workspace.clone(),
            ..t
        };
        ids.push(t.id);
        f.store.insert(&t).unwrap();
    }
    // 250 − 100（1 本目）− 100（2 本目）< 30 + 100: 2 本だけ始まる。
    assert_eq!(f.d.tick().unwrap().dispatched, 2);
    let ready: Vec<TaskId> = (ids.iter().copied())
        .filter(|id| status(&f.store, *id) == Status::Ready)
        .collect();
    assert_eq!(ready.len(), 1);
    // 走査は変わらない（通した 2 本の lease はまだ無い）: 3 本目は待ち続ける。
    for _ in 0..3 {
        assert_eq!(f.d.tick().unwrap().dispatched, 0);
        assert_eq!(status(&f.store, ready[0]), Status::Ready);
    }
    assert_eq!(f.d.scratch.admission.notes(), 1);
    // 通した 2 本が走査に書いている owner として出る（target 60 GB、増分 40 GB ずつ）: 250 − 80 − 100 = 70 ≥ 30 で始まる。
    let started: Vec<Sample> = (ids.iter())
        .filter(|id| **id != ready[0])
        .map(|id| sample(&format!("task-{id}"), &repo, true, 60 * GB))
        .collect();
    f.inputs.lock().unwrap().samples =
        [vec![sample("task-old", &repo, false, 100 * GB)], started].concat();
    assert_eq!(f.d.tick().unwrap().dispatched, 1);
    assert_eq!(status(&f.store, ready[0]), Status::Running);
}

/// attempt 7 の review の指摘: `reviewing` で子・人の承認・再同期の停止を待つ親の target は、走査では増分を持つ
/// （DB の状態が `task reviewing`）が、この daemon は親の run も review も走らせていないので「1 本は通す」の書き手に
/// 数えない。子の run は予備を割っていても通る（数えると、親は子を、子は親を待って詰まり、受信箱にも出ない）。親の
/// owner は走査の行（probe）だけで与える: 判定が見るのは走査の行と dispatcher の run・review・WU の検査の表だけ。
/// 走っている run が別にあれば、それが書き手になり子は待つ（`a_new_target_waits_…` と同じ）。
#[tokio::test]
async fn a_parent_waiting_in_review_does_not_stall_its_child() {
    // 本番の値（空き 69 GB、予備 30 GB、repo の見込み 100 GB、親の target 40 GB は 60 GB 増える見込み）。
    let mut f = fixture(69 * GB, Vec::new());
    let root = f._root.path().to_path_buf();
    let (child, repo) = rust_task(&root, "child");
    let parent = TaskId::new();
    f.inputs.lock().unwrap().samples = vec![
        sample("task-old", &repo, false, 100 * GB),
        sample(&format!("task-{parent}"), &repo, true, 40 * GB),
    ];
    f.store.insert(&child).unwrap();
    assert_eq!(f.d.tick().unwrap().dispatched, 1);
    assert_eq!(status(&f.store, child.id), Status::Running);
    assert_eq!(f.d.scratch.admission.notes(), 0);
    // 子が走っている間は、子が書き手: 同じ repo の次の新しい target は待つ。
    let (next, _) = rust_task(&root, "next");
    let next = Task {
        workspace: child.workspace.clone(),
        ..next
    };
    f.inputs
        .lock()
        .unwrap()
        .samples
        .push(sample(&format!("task-{}", child.id), &repo, true, GB));
    f.store.insert(&next).unwrap();
    assert_eq!(f.d.tick().unwrap().dispatched, 0);
    assert_eq!(status(&f.store, next.id), Status::Ready);
    assert_eq!(f.d.scratch.admission.notes(), 1);
}

/// attempt 9 の review の指摘: 待たせた task は待ちの理由 M12（`TaskWait::DiskForecast`）を持つ。理由が無いと、停滞の
/// 判定が「ready のまま run も待ちの理由も無い」task として `task_unscheduled_secs`（既定 1800 秒）の後に受信箱へ出し、
/// task の `runnable` も true のままになる。上の試験は通知（`notice_list`）しか見ず、停滞の判定と受信箱を検出できない。
/// 時刻は注入した時計（`test_now`）で 2 時間進める。
#[tokio::test]
async fn a_held_run_has_a_wait_reason_and_never_becomes_task_unscheduled() {
    use task_ops::stagnation::{StagnationKind, StagnationPolicy};
    let mut f = fixture(150 * GB, Vec::new());
    let policy = StagnationPolicy {
        task_unscheduled_secs: 1800,
        ..Default::default()
    };
    f.d.set_stagnation(policy.clone(), 60);
    let (tx, rx) = tokio::sync::watch::channel(None);
    f.d.set_snapshot_publisher(SnapshotPublisher {
        tx,
        instance_id: "inst-1".into(),
        hostname: "host-1".into(),
        started_at: "2026-11-12T00:00:00Z".into(),
        tick_ms: 50,
    });
    let clock = Arc::new(StdMutex::new(OffsetDateTime::now_utc()));
    f.d.test_now = Some(clock.clone());
    let root = f._root.path().to_path_buf();
    let (a, repo) = rust_task(&root, "a");
    let (b, _) = rust_task(&root, "b");
    let b = Task {
        workspace: a.workspace.clone(),
        ..b
    };
    f.inputs.lock().unwrap().samples = vec![sample("task-old", &repo, false, 100 * GB)];
    f.store.insert(&a).unwrap();
    f.d.tick().unwrap();
    assert_eq!(status(&f.store, a.id), Status::Running);
    f.inputs.lock().unwrap().samples = vec![
        sample("task-old", &repo, false, 100 * GB),
        sample(&format!("task-{}", a.id), &repo, true, 65 * GB),
    ];
    f.store.insert(&b).unwrap();
    f.d.tick().unwrap();
    let later = *clock.lock().unwrap() + time::Duration::hours(2);
    *clock.lock().unwrap() = later;
    f.d.tick().unwrap();
    assert_eq!(status(&f.store, b.id), Status::Ready);
    // task の待ち（snapshot・一覧・詳細と同じ合わせ方）に M12 が出て、走れるとは見えない。
    let memory = task_ops::wait::MemoryWaits::from_waits(
        f.d.snapshot_task_waits()
            .into_iter()
            .map(|w| (w.task_id, w.wait)),
    );
    let task_b = f.store.get(b.id).unwrap().unwrap();
    let waits = task_ops::wait::task_waits_one(f.store.as_ref(), &task_b, &memory).unwrap();
    assert!(
        matches!(&waits[..], [TaskWait::DiskForecast { need_bytes, detail }]
            if *need_bytes == 100 * GB && detail.contains("reserve")),
        "{waits:?}"
    );
    assert!(!task_ops::wait::is_runnable(&task_b, &waits));
    // 停滞の判定は b を拾わない。メモリの待ちが無ければ拾う（この試験が漏れを検出できること）。
    let unscheduled = |findings: &[task_ops::stagnation::StagnationFinding]| {
        findings
            .iter()
            .any(|x| x.kind == StagnationKind::TaskUnscheduled && x.task_id == Some(b.id))
    };
    assert!(!unscheduled(&f.d.stagnation.findings));
    let none = std::collections::HashSet::new();
    let facts = |memory: &task_ops::wait::MemoryWaits| {
        task_ops::stagnation::gather(f.store.as_ref(), None, &policy, later, &none, &none, memory)
            .unwrap()
    };
    let without_memory = facts(&task_ops::wait::MemoryWaits::default());
    assert!(unscheduled(&task_ops::stagnation::detect(
        &without_memory,
        &policy,
        later
    )));
    // 受信箱にも出ない。
    let snapshot = rx.borrow().clone();
    let inbox = task_ops::human_inbox::human_inbox(
        f.store.as_ref(),
        snapshot.as_ref(),
        &task_ops::view::ViewContext {
            workspace_roots: task_core::local_machine::WorkspaceRoots::new(
                PathBuf::from("/nonexistent"),
                None,
            ),
            retry_backoff_base: Duration::ZERO,
            retry_backoff_max: Duration::ZERO,
            max_requeues: 5,
            clusters: Default::default(),
        },
        later,
        &|_, _| Vec::new(),
    )
    .unwrap();
    assert!(
        !inbox.items.iter().any(|i| {
            i.kind == task_ops::human_inbox::InboxKind::TaskUnscheduled
                || i.task.as_ref().is_some_and(|t| t.id == b.id)
        }),
        "{:?}",
        inbox.items.iter().map(|i| i.kind).collect::<Vec<_>>()
    );
    assert_eq!(f.d.scratch.admission.notes(), 1);
}
