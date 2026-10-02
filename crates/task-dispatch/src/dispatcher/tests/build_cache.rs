use super::*;

/// 共有ビルドキャッシュが有効で git のリポジトリがある run の前置きに、`target/` は共有キャッシュに
/// あるという 1 行が足される。無効なら足されない。
#[tokio::test]
async fn the_preamble_notes_the_shared_build_cache_when_enabled() {
    let repo_dir = tempfile::tempdir().unwrap();
    init_test_repo(repo_dir.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = git_task(
        repo_dir.path(),
        None,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
    );
    store.insert(&task).unwrap();
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(WorkspaceNoteAdapter { seen: seen.clone() });
    let mut d = worktree_dispatcher(store.clone(), adapter, root.path(), None);
    d.config.shared_build_cache = true;
    run_until_idle(&mut d, 60).await;
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
    let notes = seen.lock().unwrap().clone();
    assert_eq!(notes.len(), 1);
    let note = notes[0].as_deref().unwrap_or_default();
    assert!(
        note.contains("target/") && note.contains("CARGO_TARGET_DIR"),
        "{note}"
    );
}

#[tokio::test]
async fn the_preamble_does_not_note_the_shared_build_cache_when_disabled() {
    let repo_dir = tempfile::tempdir().unwrap();
    init_test_repo(repo_dir.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = git_task(
        repo_dir.path(),
        None,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
    );
    store.insert(&task).unwrap();
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(WorkspaceNoteAdapter { seen: seen.clone() });
    let mut d = worktree_dispatcher(store.clone(), adapter, root.path(), None);
    d.config.shared_build_cache = false;
    run_until_idle(&mut d, 60).await;
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
    let notes = seen.lock().unwrap().clone();
    assert_eq!(notes.len(), 1);
    let note = notes[0].as_deref().unwrap_or_default();
    assert!(!note.contains("CARGO_TARGET_DIR"), "{note}");
}

/// `[workspace] shared_build_cache`（既定 true）が有効なローカルの git worktree のホスト実行に、
/// `CARGO_TARGET_DIR=<build_cache_dir>/cargo/<repo-key>` が渡る。
#[tokio::test]
async fn shared_build_cache_sets_cargo_target_dir_for_a_local_git_worktree_on_the_host() {
    let repo_dir = tempfile::tempdir().unwrap();
    init_test_repo(repo_dir.path());
    let root = tempfile::tempdir().unwrap();
    let cache_dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = git_task(
        repo_dir.path(),
        None,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
    );
    store.insert(&task).unwrap();
    let captured: CapturedEnvs = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(PoolAdapter {
        terminal_or_throttled: Ok(Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        }),
        delay: Duration::ZERO,
        observation: None,
        env: Vec::new(),
        captured: captured.clone(),
        spawn_failure: false,
    });
    let mut d = worktree_dispatcher(store.clone(), adapter, root.path(), None);
    d.config.shared_build_cache = true;
    d.config.build_cache_dir = cache_dir.path().to_path_buf();
    run_until_idle(&mut d, 60).await;

    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
    let runs = captured.lock().unwrap().clone();
    assert_eq!(runs.len(), 1, "{runs:?}");
    let expected =
        task_worker::build_cache::cargo_target_dir_env(cache_dir.path(), repo_dir.path());
    assert!(
        runs[0].contains(&expected),
        "expected {expected:?} in {:?}",
        runs[0]
    );
}

/// `[workspace] shared_build_cache = false` なら `CARGO_TARGET_DIR` は渡らない。
#[tokio::test]
async fn shared_build_cache_disabled_does_not_set_cargo_target_dir() {
    let repo_dir = tempfile::tempdir().unwrap();
    init_test_repo(repo_dir.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = git_task(
        repo_dir.path(),
        None,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
    );
    store.insert(&task).unwrap();
    let captured: CapturedEnvs = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(PoolAdapter {
        terminal_or_throttled: Ok(Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        }),
        delay: Duration::ZERO,
        observation: None,
        env: Vec::new(),
        captured: captured.clone(),
        spawn_failure: false,
    });
    let mut d = worktree_dispatcher(store.clone(), adapter, root.path(), None);
    d.config.shared_build_cache = false;
    run_until_idle(&mut d, 60).await;

    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
    let runs = captured.lock().unwrap().clone();
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert!(
        !runs[0].iter().any(|(k, _)| k == "CARGO_TARGET_DIR"),
        "{:?}",
        runs[0]
    );
}

/// リモート（クラスタ）実行には `CARGO_TARGET_DIR` を渡さない（ADR-0066 D1 は対象外と決めている）。
/// `run_worker` を直接呼ぶ（`remote_task`/`write_stub_ssh` は既存の Phase 99 のテストと同じ小道具）。
#[tokio::test]
async fn shared_build_cache_is_not_applied_to_remote_workspaces() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let tmp = tempfile::tempdir().unwrap();
    let stub = write_stub_ssh(tmp.path(), 0);
    let task = remote_task(tmp.path(), Some(WorkspaceMode::Shared), Vec::new());
    store.insert(&task).unwrap();

    let mut settings = SshSettings::new("pegasus", "pegasus", PathBuf::from("/work/proj"));
    settings.sync = SyncMode::None;
    settings.task_id = task.id.to_string();
    settings.ssh_command = vec![stub.to_string_lossy().into_owned()];

    let captured: CapturedEnvs = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(PoolAdapter {
        terminal_or_throttled: Ok(Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        }),
        delay: Duration::ZERO,
        observation: None,
        env: Vec::new(),
        captured: captured.clone(),
        spawn_failure: false,
    });
    let outcome = run_worker(
        store.clone(),
        adapter,
        Vec::new(),
        task.id,
        Tier::Standard,
        tmp.path().join("mirror"),
        "run1",
        RunLimits {
            wall_clock: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(5),
            kill_grace: Duration::from_millis(100),
        },
        LeaseRenewal {
            ttl: Duration::from_secs(60),
            every: Duration::from_secs(30),
        },
        Some(settings),
        None,
        RunExtras::default(),
        Vec::new(),
        Vec::new(),
        DelegationLimits::default(),
        None,
        None,
        ContainerDecision::Host,
        // scratch を渡しても、`remote.is_some()` なので適用されないことを確かめる（ADR-0075 D3）。
        CargoTargetPlan::Scratch {
            settings: Box::new(task_worker::scratch::ScratchSettings::with_dir(
                tmp.path().join("scratch"),
            )),
            candidates: Vec::new(),
        },
    )
    .await;
    assert!(outcome.is_ok(), "{:?}", outcome.err());

    let runs = captured.lock().unwrap().clone();
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert!(
        !runs[0].iter().any(|(k, _)| k == "CARGO_TARGET_DIR"),
        "{:?}",
        runs[0]
    );
}

/// ADR-0074 F5-fix（不具合 1 の再現、タスク 01M3HS2E19BRC021ZXMDZANP5B）: 並列の 2 WU の run は
/// 別々の `CARGO_TARGET_DIR`（`<repo-key>/wu-<id>`）を受け取り、`request.json` にその値が残る。
/// WU の checks も同じ値で走り、統合 WU の検査・reviewer の checks は `<repo-key>`。WU が done に
/// なると daemon がその target を消す。
#[tokio::test]
async fn parallel_work_units_get_their_own_cargo_target_dir_and_it_is_removed_when_done() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let logs = tempfile::tempdir().unwrap();
    let log = logs.path().join("check-env.log");
    let record = format!("echo \"$CARGO_TARGET_DIR\" >> {}", log.display());
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), &format!("{record} # review"));
    store.insert(&task).unwrap();
    let with_check = |key: &str| {
        let mut w = v2_wu(key, "build", &[]);
        w.checks = vec![task_core::WorkUnitCheck {
            cmd: format!("{record} # {key}"),
            expect_exit: 0,
        }];
        w
    };
    adopt_v2_plan(
        &store,
        task.id,
        &["build"],
        vec![with_check("a"), with_check("b")],
    );
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(TargetDirAdapter {
        env: Vec::new(),
        delay: Duration::from_millis(50),
        seen: seen.clone(),
    });
    let mut d = parallel_dispatcher(store.clone(), adapter, root.path(), 3, 3, 3);
    d.config.shared_build_cache = true;
    d.config.build_cache_dir = cache.path().to_path_buf();
    let s = store.clone();
    let id = task.id;
    assert!(
        run_until(&mut d, 800, || s
            .get(id)
            .ok()
            .flatten()
            .is_some_and(|t| t.status == Status::Done))
        .await,
        "{:?}",
        events_of(&store, task.id)
    );

    let units = store.work_units_for(task.id).unwrap();
    let id_of = |k: &str| units.iter().find(|u| u.key == k).unwrap().id.clone();
    let repo_target = task_worker::build_cache::cargo_target_dir(cache.path(), repo.path());
    let expected: std::collections::BTreeMap<String, PathBuf> = ["a", "b"]
        .iter()
        .map(|k| {
            (
                k.to_string(),
                task_worker::build_cache::work_unit_cargo_target_dir(
                    cache.path(),
                    repo.path(),
                    &id_of(k),
                ),
            )
        })
        .collect();
    let runs = seen.lock().unwrap().clone();
    let mut recorded = std::collections::BTreeMap::new();
    for (key, request) in &runs {
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(request).unwrap()).unwrap();
        recorded.insert(
            key.clone(),
            PathBuf::from(v["cargo_target_dir"].as_str().expect("cargo_target_dir")),
        );
    }
    assert_eq!(recorded, expected, "{runs:?}");
    assert_ne!(recorded["a"], recorded["b"]);

    // WU の checks は WU ごと、統合 WU の検査と reviewer の checks は `<repo-key>`。
    let lines = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = lines.lines().collect();
    for dir in expected.values() {
        assert!(
            lines.iter().any(|l| Path::new(l) == dir.as_path()),
            "{lines:?}"
        );
    }
    assert!(
        lines.iter().any(|l| Path::new(l) == repo_target.as_path()),
        "{lines:?}"
    );
    assert!(lines.iter().all(|l| !l.is_empty()), "{lines:?}");

    // done の WU の target は消える（rename は tick の中、中身の削除は別スレッド）。
    assert!(
        run_until(&mut d, 200, || expected.values().all(|p| !p.exists())).await,
        "done の WU の target が残っている"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::fs::read_dir(&repo_target)
        .map(|rd| {
            rd.flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with(".deleting-"))
        })
        .unwrap_or(false)
        && Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(20)).await;
        d.tick().unwrap();
    }
    let left: Vec<String> = std::fs::read_dir(&repo_target)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        left.iter()
            .all(|n| !n.starts_with("wu-") && !n.starts_with(".deleting-")),
        "{left:?}"
    );
}

/// ADR-0075 §5 G1 受け入れ条件 1: 全ての経路（Task 単位の run、v2 の WU の run と checks、統合 WU の検査、
/// reviewer の checks）の `CARGO_TARGET_DIR` が `<scratch>/targets/<owner>/target` で、`request.json` の
/// `cargo_target_dir` と一致する（`build_cache_dir` は使わない）。Remote には与えない
/// （`shared_build_cache_is_not_applied_to_remote_workspaces` が scratch の計画を渡して確かめる）。
#[tokio::test]
async fn every_cargo_path_uses_the_scratch_target_dir() {
    // (1) v2 の並列 WU の run と checks、統合 WU の検査、reviewer の checks。
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let scratch_dir = tempfile::tempdir().unwrap();
    let logs = tempfile::tempdir().unwrap();
    let log = logs.path().join("check-env.log");
    let record = format!("echo \"$CARGO_TARGET_DIR\" >> {}", log.display());
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), &format!("{record} # review"));
    store.insert(&task).unwrap();
    let with_check = |key: &str| {
        let mut w = v2_wu(key, "build", &[]);
        w.checks = vec![task_core::WorkUnitCheck {
            cmd: format!("{record} # {key}"),
            expect_exit: 0,
        }];
        w
    };
    adopt_v2_plan(
        &store,
        task.id,
        &["build"],
        vec![with_check("a"), with_check("b")],
    );
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(TargetDirAdapter {
        env: Vec::new(),
        delay: Duration::from_millis(50),
        seen: seen.clone(),
    });
    let mut d = parallel_dispatcher(store.clone(), adapter, root.path(), 3, 3, 3);
    let settings = scratch_on(&mut d, scratch_dir.path());
    d.config.build_cache_dir = cache.path().to_path_buf();
    let s = store.clone();
    let id = task.id;
    assert!(
        run_until(&mut d, 800, || s
            .get(id)
            .ok()
            .flatten()
            .is_some_and(|t| t.status == Status::Done))
        .await,
        "{:?}",
        events_of(&store, task.id)
    );
    let pool = settings.pool();
    let units = store.work_units_for(task.id).unwrap();
    let id_of = |k: &str| units.iter().find(|u| u.key == k).unwrap().id.clone();
    let expected: std::collections::BTreeMap<String, PathBuf> = ["a", "b"]
        .iter()
        .map(|k| {
            (
                k.to_string(),
                pool.target_dir(&task_worker::scratch::Owner::work_unit(
                    task.id.to_string(),
                    id_of(k),
                )),
            )
        })
        .collect();
    let task_target = pool.target_dir(&task_worker::scratch::Owner::task(task.id.to_string()));
    let runs = seen.lock().unwrap().clone();
    let mut recorded = std::collections::BTreeMap::new();
    for (key, request) in &runs {
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(request).unwrap()).unwrap();
        recorded.insert(
            key.clone(),
            PathBuf::from(v["cargo_target_dir"].as_str().expect("cargo_target_dir")),
        );
    }
    assert_eq!(recorded, expected, "{runs:?}");
    // WU の lease には key も写る（表示用）。
    for k in ["a", "b"] {
        let owner = task_worker::scratch::Owner::work_unit(task.id.to_string(), id_of(k));
        let lease = task_worker::scratch::read_lease(&pool.lease_path(&owner))
            .unwrap()
            .unwrap();
        assert_eq!(lease.work_unit_key.as_deref(), Some(k));
        assert_eq!(lease.kind, task_worker::scratch::OwnerKind::WorkUnit);
    }
    // WU の checks は WU の owner、統合 WU の検査と reviewer の checks は Task の owner。
    let lines = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = lines.lines().collect();
    for dir in expected.values() {
        assert!(
            lines.iter().any(|l| Path::new(l) == dir.as_path()),
            "{lines:?}"
        );
    }
    assert!(
        lines.iter().any(|l| Path::new(l) == task_target.as_path()),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .all(|l| Path::new(l).starts_with(scratch_dir.path())),
        "{lines:?}"
    );
    // 旧い `build_cache_dir/cargo/<repo-key>` は作られない。
    assert!(!cache.path().join("cargo").exists());

    // (2) Task 単位の run（v2 でない local git worktree の Task）は owner `task-<id>`。
    let repo_dir = tempfile::tempdir().unwrap();
    init_test_repo(repo_dir.path());
    let root = tempfile::tempdir().unwrap();
    let scratch_dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = git_task(
        repo_dir.path(),
        None,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
    );
    store.insert(&task).unwrap();
    let captured: CapturedEnvs = Arc::new(StdMutex::new(Vec::new()));
    let mut d = worktree_dispatcher(
        store.clone(),
        done_pool_adapter(&captured),
        root.path(),
        None,
    );
    let settings = scratch_on(&mut d, scratch_dir.path());
    run_until_idle(&mut d, 60).await;
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
    let runs = captured.lock().unwrap().clone();
    assert_eq!(runs.len(), 1, "{runs:?}");
    let owner = task_worker::scratch::Owner::task(task.id.to_string());
    assert_eq!(
        runs[0]
            .iter()
            .filter(|(k, _)| k == "CARGO_TARGET_DIR")
            .cloned()
            .collect::<Vec<_>>(),
        task_worker::scratch::target_env(&settings.pool(), &owner),
    );
    let lease = task_worker::scratch::read_lease(&settings.pool().lease_path(&owner))
        .unwrap()
        .unwrap();
    assert_eq!(
        lease.repo_key,
        task_worker::build_cache::repo_cache_key(repo_dir.path())
    );
}

/// ADR-0129 (1): scratch の run と reviewer の checks には `CARGO_TARGET_DIR` と `[scratch.cargo]`
/// （`CARGO_INCREMENTAL=0`・`CARGO_PROFILE_DEV_DEBUG=line-tables-only`）だけを与える。Celeris は sccache 系
/// （`RUSTC_WRAPPER` / `SCCACHE_*`）を足しも外しもしない: run の env に sccache の族は無く、何も外さず、checks の
/// 子プロセスは親（daemon = この test の process）の sccache の族をそのまま持つ。
/// `inherited_sccache_env_passes_through` がこのテストを `RUSTC_WRAPPER` などを持つ env で走らせ直す。
#[tokio::test]
async fn scratch_runs_get_target_and_cargo_tuning_but_no_sccache() {
    let (settings, owner, run_env, check) = run_scratch_env().await;
    let target = settings.pool().target_dir(&owner).display().to_string();
    // cargo の env（先頭の 3 つ）の後は後続 task の宣言先（`CELERIS_*`、ADR-0098 D6）だけ。
    assert_eq!(
        run_env[..run_env.len().min(3)],
        [
            ("CARGO_TARGET_DIR".to_string(), target.clone()),
            ("CARGO_INCREMENTAL".to_string(), "0".to_string()),
            (
                "CARGO_PROFILE_DEV_DEBUG".to_string(),
                "line-tables-only".to_string()
            ),
        ]
    );
    assert!(
        run_env[3..].iter().all(|(k, _)| k.starts_with("CELERIS_")),
        "{run_env:?}"
    );
    assert!(
        !run_env.iter().any(|(_, v)| v == ENV_REMOVED),
        "{run_env:?}"
    );
    let parent = |key: &str| std::env::var(key).unwrap_or_else(|_| "unset".to_string());
    assert_eq!(
        check.lines().next().unwrap_or_default(),
        format!(
            "{}|{}|{}|0|line-tables-only|{target}",
            parent("RUSTC_WRAPPER"),
            parent("SCCACHE_DIR"),
            parent("SCCACHE_SERVER_PORT")
        ),
        "{check}"
    );
    assert_eq!(
        check.lines().nth(1).unwrap_or_default().trim(),
        parent_sccache_family().trim(),
        "{check}"
    );
    // scratch の下に sccache の wrapper（`<scratch>/bin/sccache`）を作らない。
    assert!(!settings.pool().root().join("bin/sccache").exists());
}

/// ADR-0129 (1): `RUSTC_WRAPPER` / `RUSTC_WORKSPACE_WRAPPER` / `SCCACHE_*` を export した env（host の cargo 設定で
/// sccache を使う daemon の姿）でこの test binary を走らせ直し、継いだ値が checks の子プロセスへそのまま渡り、
/// run では外されないことを確かめる。
#[test]
fn inherited_sccache_env_passes_through() {
    let exe = std::env::current_exe().unwrap();
    let tests =
        ["dispatcher::tests::build_cache::scratch_runs_get_target_and_cargo_tuning_but_no_sccache"];
    let out = std::process::Command::new(exe)
        .args(tests)
        .arg("--exact")
        .env("RUSTC_WRAPPER", "/inherited/bin/sccache")
        .env("RUSTC_WORKSPACE_WRAPPER", "/inherited/bin/ws-wrapper")
        .env("SCCACHE_DIR", "/inherited/sccache")
        .env("SCCACHE_SERVER_PORT", "4226")
        .env("SCCACHE_REDIS_ENDPOINT", "redis://127.0.0.1:1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains(&format!("test result: ok. {} passed", tests.len())),
        "{stdout}"
    );
}

/// ADR-0075 §5 G1 受け入れ条件 5: 空きが `min_free_disk_mb` 未満なら、新しい run の前に緊急 GC（P0 以外を
/// 目標まで rename）→ 空きが戻るまで dispatch を保留し「ディスク不足 (infra)」を 1 回だけ通知 → 戻れば自動解除。
/// 消せるものが P0 だけなら通知の本文に pinned の一覧が付く。
#[tokio::test]
async fn low_disk_runs_emergency_gc_before_pausing_dispatch() {
    use task_worker::scratch::{AdoptCandidate, AllocateRequest, Owner};
    let none = |_: &AdoptCandidate| None;
    let lease = |pool: &task_worker::scratch::Pool, name: &str| {
        let owner = Owner::parse(name).unwrap();
        task_worker::scratch::allocate(
            pool,
            &AllocateRequest {
                owner: &owner,
                repo_path: Path::new("/repo/agent-platform"),
                base_commit: None,
                work_unit_key: None,
                checkout: None,
                candidates: &[],
                distance: &none,
                adopt: false,
                max_distance: 0,
            },
        )
        .unwrap();
        owner
    };
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let task = new_task(
        dir.path(),
        Check::Command {
            cmd: ":".into(),
            expect_exit: 0,
        },
        0,
    );
    store.insert(&task).unwrap();
    let scratch_dir = tempfile::tempdir().unwrap();
    let mut d = dispatcher(store.clone(), done_adapter(), 1);
    d.config.workspace_root = dir.path().to_path_buf();
    let settings = scratch_on(&mut d, scratch_dir.path());
    let pool = settings.pool();
    let live = lease(&pool, "agent-live");
    let released = lease(&pool, "agent-released");
    task_worker::scratch::release(&pool, &released).unwrap();
    d.config.min_free_disk_mb = u64::MAX;
    assert_eq!(d.tick().unwrap().dispatched, 0);
    // 緊急 GC: agent の P3（通常は watermark まで残る）も rename、P0 は残る。
    assert!(!pool.target_dir(&released).exists());
    assert!(pool.target_dir(&live).exists());
    assert!(d.disk_low);
    assert_eq!(
        d.scratch.view.as_ref().map(|v| v.pressure.as_str()),
        Some("emergency")
    );
    assert!(
        d.scratch
            .view
            .as_ref()
            .unwrap()
            .last_gc
            .as_ref()
            .unwrap()
            .emergency
    );
    assert_eq!(d.tick().unwrap().dispatched, 0);
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Ready);
    let notifications = store.notification_recent(10).unwrap();
    assert_eq!(notifications.len(), 1, "{notifications:?}");
    assert!(notifications[0].body.contains("ディスク不足 (infra)"));
    // 空きが戻れば自動で解除して dispatch する。
    d.config.min_free_disk_mb = 0;
    assert_eq!(d.tick().unwrap().dispatched, 1);
    assert!(!d.disk_low);
    assert!(pool.target_dir(&live).exists());

    // 消せるものが P0 だけ → 保留の通知に pinned の一覧。
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let scratch_dir = tempfile::tempdir().unwrap();
    let mut d = dispatcher(store.clone(), done_adapter(), 1);
    d.config.workspace_root = dir.path().to_path_buf();
    let settings = scratch_on(&mut d, scratch_dir.path());
    let live = lease(&settings.pool(), "agent-live");
    d.config.min_free_disk_mb = u64::MAX;
    d.tick().unwrap();
    assert!(settings.pool().target_dir(&live).exists());
    let notifications = store.notification_recent(10).unwrap();
    assert_eq!(notifications.len(), 1);
    assert!(
        notifications[0].body.contains("scratch pool: pinned")
            && notifications[0].body.contains("agent-live"),
        "{}",
        notifications[0].body
    );
}

/// ADR-0075 §5 G1 受け入れ条件 7: `[scratch] dir` が NFS 上（起動時の検査で無効化）、または `[scratch] enabled =
/// false` なら、ADR-0066 D1 / F5-fix の `build_cache_dir/cargo/<repo-key>` に戻る。
#[tokio::test]
async fn scratch_on_nfs_falls_back_to_build_cache_dir() {
    let scratch_dir = tempfile::tempdir().unwrap();
    let nfs = task_worker::scratch::apply_nfs_check(
        task_worker::scratch::ScratchSettings::with_dir(scratch_dir.path()),
        |_| Ok(true),
    );
    assert!(!nfs.enabled);
    assert!(nfs.disabled_reason.as_deref().unwrap_or("").contains("NFS"));
    let disabled = task_worker::scratch::ScratchSettings {
        enabled: false,
        ..task_worker::scratch::ScratchSettings::with_dir(scratch_dir.path())
    };
    for settings in [nfs, disabled] {
        let repo_dir = tempfile::tempdir().unwrap();
        init_test_repo(repo_dir.path());
        let root = tempfile::tempdir().unwrap();
        let cache_dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
        let task = git_task(
            repo_dir.path(),
            None,
            Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
        );
        store.insert(&task).unwrap();
        let captured: CapturedEnvs = Arc::new(StdMutex::new(Vec::new()));
        let mut d = worktree_dispatcher(
            store.clone(),
            done_pool_adapter(&captured),
            root.path(),
            None,
        );
        d.config.shared_build_cache = true;
        d.config.build_cache_dir = cache_dir.path().to_path_buf();
        d.config.scratch = settings.clone();
        run_until_idle(&mut d, 60).await;
        assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
        let runs = captured.lock().unwrap().clone();
        let expected =
            task_worker::build_cache::cargo_target_dir_env(cache_dir.path(), repo_dir.path());
        assert!(runs[0].contains(&expected), "{:?}", runs[0]);
        assert!(!scratch_dir.path().join("targets").exists());
        let view = d.scratch.view.clone().expect("disabled view");
        assert!(!view.enabled);
        assert_eq!(view.disabled_reason, settings.disabled_reason);
    }
}

/// ADR-0075 §5 G1 受け入れ条件 4（dispatcher 経由）: run の開始時、同じ repo の P3 の target が checkout より前に
/// 書かれたものなら rename で引き継ぐ（lease に `adopted_from`）。
#[tokio::test]
async fn run_start_adopts_a_finished_target_that_predates_the_checkout() {
    use task_worker::scratch::{AdoptCandidate, AllocateRequest, Owner};
    let repo_dir = tempfile::tempdir().unwrap();
    init_test_repo(repo_dir.path());
    let root = tempfile::tempdir().unwrap();
    let scratch_dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = git_task(
        repo_dir.path(),
        None,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
    );
    store.insert(&task).unwrap();
    let captured: CapturedEnvs = Arc::new(StdMutex::new(Vec::new()));
    let mut d = worktree_dispatcher(
        store.clone(),
        done_pool_adapter(&captured),
        root.path(),
        None,
    );
    let settings = scratch_on(&mut d, scratch_dir.path());
    let pool = settings.pool();
    // 前の release の target（released = P3）。最終書き込みは 1 時間前。
    let old = Owner::parse("release-0123456789ab").unwrap();
    let none = |_: &AdoptCandidate| None;
    let a = task_worker::scratch::allocate(
        &pool,
        &AllocateRequest {
            owner: &old,
            repo_path: repo_dir.path(),
            base_commit: None,
            work_unit_key: None,
            checkout: None,
            candidates: &[],
            distance: &none,
            adopt: false,
            max_distance: 0,
        },
    )
    .unwrap();
    std::fs::write(a.target_dir.join("warm.rlib"), "x").unwrap();
    task_worker::scratch::release(&pool, &old).unwrap();
    let hour_ago = std::time::SystemTime::now() - Duration::from_secs(3600);
    for p in [
        a.target_dir.join("warm.rlib"),
        a.target_dir.clone(),
        pool.lease_path(&old),
    ] {
        std::fs::File::open(&p)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(hour_ago))
            .unwrap();
    }
    // release の P3 は即回収の対象なので、seed になるよう同じ repo の P0（生きている agent の lease）を置く。
    let live = Owner::parse("agent-live").unwrap();
    task_worker::scratch::allocate(
        &pool,
        &AllocateRequest {
            owner: &live,
            repo_path: repo_dir.path(),
            base_commit: None,
            work_unit_key: None,
            checkout: None,
            candidates: &[],
            distance: &none,
            adopt: false,
            max_distance: 0,
        },
    )
    .unwrap();
    run_until_idle(&mut d, 60).await;
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
    let owner = Owner::task(task.id.to_string());
    let lease = task_worker::scratch::read_lease(&pool.lease_path(&owner))
        .unwrap()
        .unwrap();
    assert_eq!(lease.adopted_from.as_deref(), Some("release-0123456789ab"));
    assert!(pool.target_dir(&owner).join("warm.rlib").exists());
    assert!(!pool.target_dir(&old).exists());
}
