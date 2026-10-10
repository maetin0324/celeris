//! ADR 2026-10-10-local-disk-growth-paths D1: task の repo checkout で走る run・WU の check・段の統合の検査に
//! scratch の `CARGO_TARGET_DIR`（lease 付き）が渡り、repo 直下に `target/` を作らない。`repos=[]` の子 task が
//! 親の checkout を使う形（自分の作業場所は git でない）も祖先の checkout に結び付ける。

use super::*;

/// 祖先の checkout を持つ親（`Blocked` で dispatch しない）と、git でない作業場所の `repos=[]` の子 task。
/// 親の checkout（`<root>/<parent>/tree`）は run を経ずに作る（`Cargo.toml` だけ）。
struct ChildFixture {
    repo: tempfile::TempDir,
    root: tempfile::TempDir,
    scratch: tempfile::TempDir,
    plain: tempfile::TempDir,
    logs: tempfile::TempDir,
    store: Arc<dyn TaskStore>,
    parent_checkout: PathBuf,
    child: Task,
}

fn child_fixture(check: Check) -> ChildFixture {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let plain = tempfile::tempdir().unwrap();
    let logs = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut parent = git_task(
        repo.path(),
        None,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
    );
    parent.status = Status::Blocked;
    store.insert(&parent).unwrap();
    let parent_checkout = root
        .path()
        .join(parent.id.to_string())
        .join(task_worker::WORKTREE_DIR_NAME);
    std::fs::create_dir_all(&parent_checkout).unwrap();
    std::fs::write(parent_checkout.join("Cargo.toml"), "[workspace]\n").unwrap();
    let mut child = new_task(plain.path(), check, 0);
    child.parent_id = Some(parent.id);
    assert!(child.repos.is_empty());
    store.insert(&child).unwrap();
    ChildFixture {
        repo,
        root,
        scratch,
        plain,
        logs,
        store,
        parent_checkout,
        child,
    }
}

fn record_target(log: &Path) -> String {
    format!("echo \"$CARGO_TARGET_DIR\" >> {}", log.display())
}

fn logged_lines(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// `repos=[]` の子 task の run（と受け入れの command check）は、親の checkout に結び付いた子の owner の
/// scratch target を受け取り、lease は親の repo を指す。親の checkout にも子の作業場所にも `target/` は無い。
#[tokio::test]
async fn target_env_repos_empty_child_run_uses_the_scratch_target() {
    let logs = tempfile::tempdir().unwrap();
    let log = logs.path().join("check.log");
    let f = child_fixture(Check::Command {
        cmd: record_target(&log),
        expect_exit: 0,
    });
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(TargetDirAdapter {
        env: Vec::new(),
        delay: Duration::from_millis(0),
        seen: seen.clone(),
    });
    let mut d = worktree_dispatcher(f.store.clone(), adapter, f.root.path(), None);
    let settings = scratch_on(&mut d, f.scratch.path());
    run_until_task_terminal(&mut d, &f.store, f.child.id).await;
    assert_eq!(
        f.store.get(f.child.id).unwrap().unwrap().status,
        Status::Done,
        "{:?}",
        events_of(&f.store, f.child.id)
    );
    let owner = task_worker::scratch::Owner::task(f.child.id.to_string());
    let expected = settings.pool().target_dir(&owner);
    let runs = seen.lock().unwrap().clone();
    assert_eq!(runs.len(), 1, "{runs:?}");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&runs[0].1).unwrap()).unwrap();
    assert_eq!(
        v["cargo_target_dir"].as_str().map(PathBuf::from),
        Some(expected.clone())
    );
    assert!(expected.starts_with(f.scratch.path()));
    let lines = logged_lines(&log);
    assert!(!lines.is_empty());
    assert!(
        lines.iter().all(|l| Path::new(l) == expected.as_path()),
        "{lines:?}"
    );
    let lease = task_worker::scratch::read_lease(&settings.pool().lease_path(&owner))
        .unwrap()
        .expect("lease");
    assert_eq!(
        lease.repo_key,
        task_worker::build_cache::repo_cache_key(f.repo.path())
    );
    assert!(!f.parent_checkout.join("target").exists());
    assert!(!f.plain.path().join("target").exists());
    // 結び付けられたので「渡せなかった」event は無い。
    assert!(!events_of(&f.store, f.child.id).iter().any(|e| matches!(
        e,
        Event::WorkerProgress { msg, .. } if msg.starts_with(super::super::worker_task::CARGO_TARGET_NOTE_PREFIX)
    )));
    drop(f.logs);
}

/// `repos=[]` の子 task の WU の check（自分の worktree を持たない WU）も子の owner の scratch target で走る。
#[tokio::test]
async fn target_env_repos_empty_child_work_unit_check_uses_the_scratch_target() {
    let f = child_fixture(Check::Command {
        cmd: "true".into(),
        expect_exit: 0,
    });
    let log = f.logs.path().join("wu-check.log");
    let mut unit = v2_wu("a", "build", &[]);
    unit.checks = vec![task_core::WorkUnitCheck {
        cmd: record_target(&log),
        expect_exit: 0,
        scope: false,
    }];
    adopt_v2_plan(&f.store, f.child.id, &["build"], vec![unit]);
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(TargetDirAdapter {
        env: Vec::new(),
        delay: Duration::from_millis(0),
        seen: seen.clone(),
    });
    let mut d = parallel_dispatcher(f.store.clone(), adapter, f.root.path(), 1, 1, 1);
    let settings = scratch_on(&mut d, f.scratch.path());
    run_until_task_terminal(&mut d, &f.store, f.child.id).await;
    assert_eq!(
        f.store.get(f.child.id).unwrap().unwrap().status,
        Status::Done,
        "{:?}",
        events_of(&f.store, f.child.id)
    );
    let expected = settings
        .pool()
        .target_dir(&task_worker::scratch::Owner::task(f.child.id.to_string()));
    let lines = logged_lines(&log);
    assert!(!lines.is_empty(), "the WU check did not run");
    assert!(
        lines.iter().all(|l| Path::new(l) == expected.as_path()),
        "{lines:?}"
    );
    let runs = seen.lock().unwrap().clone();
    assert!(!runs.is_empty());
    for (_, request) in &runs {
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(request).unwrap()).unwrap();
        assert_eq!(
            v["cargo_target_dir"].as_str().map(PathBuf::from),
            Some(expected.clone())
        );
    }
    assert!(!f.parent_checkout.join("target").exists());
    assert!(!f.plain.path().join("target").exists());
}

/// 段の統合の検査（Task の worktree で走る）は Task の owner の scratch target を受け取る。WU の check は
/// WU の owner。`IntegrationCheckStarted` の log に出た値で見分ける。
#[tokio::test]
async fn target_env_integration_check_uses_the_task_scratch_target() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    let mut unit = v2_wu("a", "build", &[]);
    unit.checks = vec![task_core::WorkUnitCheck {
        cmd: "echo \"TARGET=$CARGO_TARGET_DIR\"".into(),
        expect_exit: 0,
        scope: false,
    }];
    adopt_v2_plan(&store, task.id, &["build"], vec![unit]);
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(TargetDirAdapter {
        env: Vec::new(),
        delay: Duration::from_millis(0),
        seen,
    });
    let mut d = parallel_dispatcher(store.clone(), adapter, root.path(), 2, 2, 2);
    let settings = scratch_on(&mut d, scratch.path());
    run_until_task_terminal(&mut d, &store, task.id).await;
    let events = events_of(&store, task.id);
    assert_eq!(
        store.get(task.id).unwrap().unwrap().status,
        Status::Done,
        "{events:?}"
    );
    let expected = settings
        .pool()
        .target_dir(&task_worker::scratch::Owner::task(task.id.to_string()));
    let logs: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            Event::IntegrationCheckStarted { log_path, .. } => {
                Some(std::fs::read_to_string(log_path).unwrap_or_default())
            }
            _ => None,
        })
        .collect();
    assert!(!logs.is_empty(), "no integration check ran: {events:?}");
    let want = format!("TARGET={}", expected.display());
    assert!(logs.iter().all(|l| l.contains(&want)), "{logs:?} / {want}");
    let tree = root
        .path()
        .join(task.id.to_string())
        .join(task_worker::WORKTREE_DIR_NAME);
    assert!(!tree.join("target").exists());
}

/// 祖先の checkout がまだ無い `repos=[]` の子 task は、`CARGO_TARGET_DIR` を渡せなかった理由を
/// `cargo-target:` の `WorkerProgress`（status）に残す（黙って repo 直下 target に落とさない）。
#[tokio::test]
async fn target_env_missing_ancestor_checkout_is_recorded() {
    let f = child_fixture(Check::Command {
        cmd: "true".into(),
        expect_exit: 0,
    });
    // 親の repo は Rust（`Cargo.toml` を commit）だが、親の checkout はまだ無い。
    std::fs::remove_dir_all(&f.parent_checkout).unwrap();
    std::fs::write(f.repo.path().join("Cargo.toml"), "[workspace]\n").unwrap();
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(TargetDirAdapter {
        env: Vec::new(),
        delay: Duration::from_millis(0),
        seen: seen.clone(),
    });
    let mut d = worktree_dispatcher(f.store.clone(), adapter, f.root.path(), None);
    scratch_on(&mut d, f.scratch.path());
    run_until_task_terminal(&mut d, &f.store, f.child.id).await;
    let events = events_of(&f.store, f.child.id);
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::WorkerProgress { msg, kind: Some(task_core::ProgressKind::Status), .. }
                if msg.starts_with(super::super::worker_task::CARGO_TARGET_NOTE_PREFIX)
                    && msg.contains("does not exist yet")
        )),
        "{events:?}"
    );
    let runs = seen.lock().unwrap().clone();
    assert_eq!(runs.len(), 1, "{runs:?}");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&runs[0].1).unwrap()).unwrap();
    assert!(v["cargo_target_dir"].is_null(), "{v}");
}
