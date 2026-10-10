use super::*;
use task_core::TaskStore;

fn populated_wal_db(path: &Path) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE pages(id INTEGER PRIMARY KEY, payload BLOB);
         WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<256)
         INSERT INTO pages SELECT x, zeroblob(4096) FROM n;",
    )
    .unwrap();
}

/// Measure the old 1-page stepping mechanism: each committed update resets its copied-page
/// count. The production setting had 100 pages followed by a 250 ms gap for writers to do this.
#[test]
fn incremental_backup_progress_restarts_after_other_connection_writes() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("source.sqlite3");
    populated_wal_db(&src);
    let source = rusqlite::Connection::open(&src).unwrap();
    let writer = rusqlite::Connection::open(&src).unwrap();
    let mut destination = rusqlite::Connection::open(dir.path().join("old.sqlite3")).unwrap();
    let backup = rusqlite::backup::Backup::new(&source, &mut destination).unwrap();
    assert!(matches!(
        backup.step(1).unwrap(),
        rusqlite::backup::StepResult::More
    ));
    let first = backup.progress();
    assert!(first.pagecount > 100);
    let mut restarts = 0;
    for _ in 1..=4 {
        writer
            .execute("UPDATE pages SET payload = randomblob(4096) WHERE id=1", [])
            .unwrap();
        assert!(matches!(
            backup.step(1).unwrap(),
            rusqlite::backup::StepResult::More
        ));
        let next = backup.progress();
        if next.remaining >= first.remaining {
            restarts += 1;
        }
    }
    eprintln!(
        "online backup measurement: pagecount={}, steps=5, restarts={restarts}",
        first.pagecount
    );
    assert_eq!(
        restarts, 4,
        "each separate-connection write resets the copied pages"
    );
}

/// The writer commits at a progress callback inside VACUUM, so the overlap is event-driven.
#[test]
fn vacuum_backup_finishes_with_a_concurrent_writer_and_integrity_ok() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("source.sqlite3");
    let dest = dir.path().join("copy.sqlite3");
    populated_wal_db(&src);
    let (go_tx, go_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let writer_path = src.clone();
    let writer = std::thread::spawn(move || {
        let conn = rusqlite::Connection::open(writer_path).unwrap();
        let mut commits = 0;
        for _ in 0..4 {
            if go_rx.recv().is_err() {
                break;
            }
            conn.execute("UPDATE pages SET payload = randomblob(4096) WHERE id=1", [])
                .unwrap();
            commits += 1;
            done_tx.send(()).unwrap();
        }
        commits
    });
    let reader =
        rusqlite::Connection::open_with_flags(&src, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let mut progress_calls = 0;
    reader.progress_handler(
        100,
        Some(move || {
            progress_calls += 1;
            if progress_calls <= 4 {
                go_tx.send(()).unwrap();
                done_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            }
            false
        }),
    );
    reader
        .execute("VACUUM INTO ?1", [dest.to_string_lossy().as_ref()])
        .unwrap();
    reader.progress_handler(0, None::<fn() -> bool>);
    assert_eq!(writer.join().unwrap(), 4);
    assert!(task_core::integrity_check(&dest).unwrap());
}

#[test]
fn incomplete_backups_are_cleaned_and_cancel_does_not_publish() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("source.sqlite3");
    populated_wal_db(&src);
    let backups = dir.path().join("backups");
    std::fs::create_dir(&backups).unwrap();
    let old = backups.join("celeris-1.sqlite3");
    std::fs::write(&old, b"incomplete").unwrap();
    let journal = backups.join("celeris-1.sqlite3-journal");
    std::fs::write(&journal, b"hot").unwrap();
    let partial = backups.join("celeris-2.sqlite3.partial");
    std::fs::write(&partial, b"incomplete").unwrap();
    let old_time = std::time::SystemTime::now() - Duration::from_secs(600);
    for path in [&old, &journal, &partial] {
        std::fs::File::open(path)
            .unwrap()
            .set_modified(old_time)
            .unwrap();
    }
    let cancelled = Arc::new(AtomicBool::new(true));
    assert!(matches!(
        backup_once(
            &src,
            &backups,
            BackupRetentionPolicy {
                keep: 48,
                daily_keep: 7,
                weekly_keep: 4,
                max_total_bytes: u64::MAX
            },
            Duration::from_secs(2),
            cancelled
        ),
        Err(BackupError::Cancelled)
    ));
    assert!(!old.exists());
    assert!(!partial.exists());
    assert!(std::fs::read_dir(&backups).unwrap().next().is_none());
}

#[test]
fn vacuum_progress_handler_interrupts_an_active_copy() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("source.sqlite3");
    let partial = dir.path().join("celeris-1.sqlite3.partial");
    populated_wal_db(&src);
    let reader =
        rusqlite::Connection::open_with_flags(&src, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    reader.progress_handler(1, Some(|| true));
    let error = reader
        .execute("VACUUM INTO ?1", [partial.to_string_lossy().as_ref()])
        .unwrap_err();
    assert!(
        matches!(error, rusqlite::Error::SqliteFailure(e, _) if e.code == rusqlite::ErrorCode::OperationInterrupted)
    );
    remove_backup_files(&partial);
    assert!(!partial.exists());
}

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
        let dest = backup_once(
            &db_path,
            &backup_dir,
            BackupRetentionPolicy {
                keep: 2,
                daily_keep: 7,
                weekly_keep: 4,
                max_total_bytes: u64::MAX,
            },
            Duration::from_millis(2000),
            Arc::new(AtomicBool::new(false)),
        )
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
    assert!(task_core::integrity_check(latest).unwrap());
    let restored = task_core::SqliteStore::open(latest).unwrap();
    assert_eq!(restored.get(task.id).unwrap().map(|t| t.id), Some(task.id));
}

#[test]
fn backup_once_fails_clearly_when_the_backup_dir_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("celeris.sqlite3");
    drop(task_core::SqliteStore::open(&db_path).unwrap());
    let missing = dir.path().join("does-not-exist");
    let err = backup_once(
        &db_path,
        &missing,
        BackupRetentionPolicy {
            keep: 48,
            daily_keep: 7,
            weekly_keep: 4,
            max_total_bytes: u64::MAX,
        },
        Duration::from_millis(2000),
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap_err();
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
        BackupRetentionPolicy {
            keep: 48,
            daily_keep: 7,
            weekly_keep: 4,
            max_total_bytes: u64::MAX,
        },
        Duration::from_millis(2000),
    );

    // 公開済みの backup（atomic rename 後）が現れてから停止する。
    // 固定時間で stop すると、遅いファイル I/O では最初のコピー自体を cancel してしまう。
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if std::fs::read_dir(&backup_dir)
                .unwrap()
                .any(|entry| entry.is_ok_and(|entry| is_backup_file_name(&entry.path())))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("backup tick did not publish a file");

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

fn retention_test_entry(name: &str, ts: i64, size: u64, kind: BackupKind) -> RetentionEntry {
    RetentionEntry {
        name: name.to_owned(),
        timestamp: ts,
        size,
        kind,
    }
}

#[test]
fn backup_retention_daily_and_weekly_generations_are_kept() {
    let now = 1_800_000_000;
    let entries: Vec<_> = (0..70)
        .map(|days| {
            retention_test_entry(
                &format!("celeris-{}.sqlite3", now - days * 86400),
                now - days * 86400,
                1,
                BackupKind::Periodic,
            )
        })
        .collect();
    let deleted = plan_backup_retention(now, &entries, 0, 7, 4, u64::MAX);
    assert!(!deleted.iter().any(|n| n == &entries[0].name));
    assert!(!deleted.iter().any(|n| n == &entries[1].name));
    assert!(deleted.len() < entries.len());
}

#[test]
fn backup_retention_keeps_every_backup_inside_48_hours() {
    let now = 1_800_000_000;
    let entries: Vec<_> = (0..60)
        .map(|hours| {
            retention_test_entry(
                &format!("celeris-{}.sqlite3", now - hours * 3600),
                now - hours * 3600,
                1,
                BackupKind::Periodic,
            )
        })
        .collect();
    let deleted = plan_backup_retention(now, &entries, 0, 0, 0, u64::MAX);
    assert!(entries[..48].iter().all(|e| !deleted.contains(&e.name)));
    assert!(deleted.contains(&entries[50].name));
}

#[test]
fn backup_retention_total_limit_prunes_periodic_before_promote() {
    let now = 1_800_000_000;
    let entries = vec![
        retention_test_entry(
            "celeris-1799990000.sqlite3",
            now - 10000,
            80,
            BackupKind::Periodic,
        ),
        retention_test_entry(
            "celeris-1799999900.sqlite3",
            now - 100,
            80,
            BackupKind::Periodic,
        ),
        retention_test_entry(
            "20261009-010203-pre-abcdef012345.sqlite3",
            now - 200,
            80,
            BackupKind::Promote,
        ),
        retention_test_entry(
            "20261010-010203-pre-abcdef012346.sqlite3",
            now,
            80,
            BackupKind::Promote,
        ),
    ];
    let deleted = plan_backup_retention(now, &entries, 4, 7, 4, 250);
    assert!(deleted.contains(&entries[0].name));
    assert!(!deleted.contains(&entries[2].name));
    assert!(!deleted.contains(&entries[3].name));
}

#[test]
fn backup_retention_protects_latest_per_kind_and_three_rollbacks() {
    let now = 1_800_000_000;
    let mut entries = vec![retention_test_entry(
        "celeris-1.sqlite3",
        1,
        1,
        BackupKind::Periodic,
    )];
    for i in 1..=2 {
        entries.push(retention_test_entry(
            &format!("2026100{i}-010203-pre-abcdef01234{i}.sqlite3"),
            i,
            1,
            BackupKind::Promote,
        ));
    }
    for i in 1..=4 {
        entries.push(retention_test_entry(
            &format!("2026100{i}-010203-pre-rollback.sqlite3"),
            i,
            1,
            BackupKind::Rollback,
        ));
    }
    let deleted = plan_backup_retention(now, &entries, 0, 0, 0, 0);
    assert!(!deleted.iter().any(|n| n == "celeris-1.sqlite3"));
    assert!(
        !deleted
            .iter()
            .any(|n| n == "20261002-010203-pre-abcdef012342.sqlite3")
    );
    assert!(
        !deleted
            .iter()
            .any(|n| n == "20261002-010203-pre-rollback.sqlite3")
    );
    assert!(
        deleted
            .iter()
            .any(|n| n == "20261001-010203-pre-rollback.sqlite3")
    );
}

#[test]
fn backup_retention_integrity_failure_skips_all_deletions() {
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("celeris-1700000000.sqlite3");
    let latest = dir.path().join("celeris-1800000000.sqlite3");
    std::fs::write(&old, b"db").unwrap();
    std::fs::write(&latest, b"corrupt").unwrap();
    prune_backups_with_check(dir.path(), 0, 0, 0, 0, integrity_check).unwrap();
    assert!(old.exists());
    assert!(latest.exists());
}

#[test]
fn backup_retention_ignores_other_file_names() {
    assert!(backup_entry("celeris-1800000000.sqlite3", 1).is_some());
    for name in [
        "health.json",
        "backup.log",
        "pre-celeris.sqlite3",
        "celeris-1.sqlite3.partial",
    ] {
        assert!(backup_entry(name, 1).is_none(), "{name}");
    }
}

fn sample_task() -> task_core::Task {
    let now = OffsetDateTime::now_utc();
    task_core::Task {
        requirements: Default::default(),
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
