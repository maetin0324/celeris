use super::*;
use task_core::TaskStore;

#[test]
fn checkpoint_once_runs_on_a_fresh_wal_db_without_error() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("celeris.sqlite3");
    // WAL journal_mode を持つファイルを作る（task-core の `SqliteStore::open` と同じ規約）。
    drop(task_core::SqliteStore::open(&db_path).unwrap());
    checkpoint_once(&db_path, Duration::from_millis(2000)).unwrap();
}

#[test]
fn backup_once_writes_a_restorable_copy_and_prune_keeps_only_the_newest() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("celeris.sqlite3");
    let store = task_core::SqliteStore::open(&db_path).unwrap();
    let task = sample_task();
    store.insert(&task).unwrap();
    drop(store);

    let backup_dir = dir.path().join("backups");
    std::fs::create_dir_all(&backup_dir).unwrap();

    // 3 回バックアップし、keep=2 なら最新 2 つだけ残る。
    let mut written = Vec::new();
    for i in 0..3 {
        let dest = backup_once(&db_path, &backup_dir, 2, Duration::from_millis(2000))
            .unwrap_or_else(|e| panic!("backup {i}: {e}"));
        written.push(dest);
        // ファイル名が unix 秒なので、同じ秒に 2 回書くと衝突する（テストのみの配慮）。
        std::thread::sleep(Duration::from_millis(1100));
    }

    let remaining: Vec<PathBuf> = std::fs::read_dir(&backup_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| is_backup_file_name(p))
        .collect();
    assert_eq!(remaining.len(), 2, "{remaining:?}");
    assert!(
        !remaining.contains(&written[0]),
        "the oldest generation should have been pruned"
    );

    // 残った最新のバックアップは復元して読める。
    let latest = written.last().unwrap();
    let restored = task_core::SqliteStore::open(latest).unwrap();
    assert_eq!(restored.get(task.id).unwrap().map(|t| t.id), Some(task.id));
}

#[test]
fn backup_once_fails_clearly_when_the_backup_dir_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("celeris.sqlite3");
    drop(task_core::SqliteStore::open(&db_path).unwrap());
    let missing = dir.path().join("does-not-exist");
    let err = backup_once(&db_path, &missing, 48, Duration::from_millis(2000)).unwrap_err();
    assert!(matches!(err, BackupError::MissingDir(_)), "{err}");
}

/// `spawn_backup_task`/`spawn_checkpoint_task` の tick→spawn_blocking→stop の配線そのものを
/// 確認する（中身のロジックは `checkpoint_once`/`backup_once` の単体テストで別途確認済み）。
#[tokio::test]
async fn spawned_tasks_run_on_their_interval_and_stop_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("celeris.sqlite3");
    drop(task_core::SqliteStore::open(&db_path).unwrap());
    let backup_dir = dir.path().join("backups");
    std::fs::create_dir_all(&backup_dir).unwrap();

    let checkpoint = spawn_checkpoint_task(
        db_path.clone(),
        Duration::from_millis(20),
        Duration::from_millis(2000),
    );
    let backup = spawn_backup_task(
        db_path.clone(),
        backup_dir.clone(),
        Duration::from_millis(20),
        48,
        Duration::from_millis(2000),
    );

    // 両方とも少なくとも 1 回は tick が回るだけ待つ（内容はロジック側のテストで確認済みなので、
    // ここでは「バックアップ tick が実際にファイルを作った」ことだけ見る）。
    tokio::time::sleep(Duration::from_millis(300)).await;

    checkpoint.stop().await;
    backup.stop().await;

    let backups: Vec<PathBuf> = std::fs::read_dir(&backup_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| is_backup_file_name(p))
        .collect();
    assert!(
        !backups.is_empty(),
        "expected at least one backup file to be written"
    );
}

fn sample_task() -> task_core::Task {
    let now = OffsetDateTime::now_utc();
    task_core::Task {
        expected_write_paths: None,
        tree: None,
        paused_at: None,
        routing: None,
        repos: Vec::new(),
        id: task_core::TaskId::new(),
        parent_id: None,
        kind: task_core::TaskKind::Execute,
        title: "db maintenance test task".into(),
        objective: "no-op".into(),
        acceptance: vec![task_core::Criterion {
            text: "n/a".into(),
            check: task_core::Check::Human,
        }],
        inputs: vec![],
        depends_on: vec![],
        status: task_core::Status::Draft,
        priority: 0,
        worker_hint: task_core::WorkerHint {
            tier: task_core::Tier::Standard,
            adapter: None,
        },
        workspace: task_core::WorkspaceSpec::Local {
            path: PathBuf::from("."),
            mode: None,
        },
        budget: task_core::Budget {
            max_turns: 1,
            max_wall_secs: 30,
            max_retries: 0,
        },
        attempts: 0,
        lease: None,
        created_at: now,
        updated_at: now,
        role: None,
        genre: None,
        aggregate: false,
        project_id: None,
        milestone_id: None,
        assignee: None,
        conversation: None,
        skills: Vec::new(),
        mode: task_core::TaskMode::default(),
        labels: Vec::new(),
        category: Default::default(),
    }
}
