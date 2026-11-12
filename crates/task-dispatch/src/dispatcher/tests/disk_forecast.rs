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

fn sample(owner: &str, repo: &str, running: bool, size: u64) -> Sample {
    Sample {
        owner: owner.into(),
        repo_key: repo.into(),
        running,
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
