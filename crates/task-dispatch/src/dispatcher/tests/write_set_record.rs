//! ADR-0130 D2: 実装 run と done になった WU の actual write-set（Git の確定差分）を store に残す。

use super::*;

/// 案件の Git repo 1 つ（`code`）を選んだ task。
fn project_git_task(
    store: &Arc<dyn TaskStore>,
    repo: &std::path::Path,
    check_cmd: &str,
) -> (Task, task_core::RepoId) {
    let (project, repos) = project_with_repos(store, &[("code", repo, RepoKind::Git)]);
    let mut task = parallel_task(repo, check_cmd);
    task.project_id = Some(project);
    task.repos = repos.iter().map(RepoRef::of).collect();
    (task, repos[0].id)
}

/// 並列 WU 2 つがそれぞれ別の file を書く。各 run の行と WU の最終 snapshot に、その WU の確定差分
/// （celeris の自動 commit 後の `base..HEAD`）だけが `complete` で残る。
#[tokio::test]
async fn write_set_record_stores_committed_paths_of_completed_work_unit_runs() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (task, repo_id) = project_git_task(&store, repo.path(), "test -f a.txt && test -f b.txt");
    store.insert(&task).unwrap();
    adopt_v2_plan(
        &store,
        task.id,
        &["impl"],
        vec![v2_wu("a", "impl", &[]), v2_wu("b", "impl", &[])],
    );
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::from_millis(20))
            .with_file("a", "a.txt", "a")
            .with_file("b", "b.txt", "b"),
    );
    let mut d = parallel_dispatcher(store.clone(), adapter, root.path(), 3, 3, 3);
    let report = run_until_idle(&mut d, 600).await;
    assert!(report.idle, "{report:?}");
    let stored = store.get(task.id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{stored:?}");
    assert!(
        d.run_write_bases.is_empty(),
        "終了した run の開始 HEAD は残さない"
    );

    for (key, path) in [("a", "a.txt"), ("b", "b.txt")] {
        let wu = store
            .work_units_for(task.id)
            .unwrap()
            .into_iter()
            .find(|u| u.key == key)
            .unwrap();
        let run_id = wu.last_run_id.clone().unwrap();
        let runs = store.run_write_sets(&run_id).unwrap();
        assert_eq!(runs.len(), 1, "{runs:?}");
        let run = &runs[0];
        assert_eq!(run.repo_id, repo_id);
        assert_eq!(run.task_id, task.id);
        assert_eq!(run.work_unit_id.as_deref(), Some(wu.id.as_str()));
        assert_eq!(
            run.status,
            task_core::write_set::WriteSetStatus::Complete,
            "{run:?}"
        );
        assert_eq!(run.paths, vec![path.to_string()]);
        assert_eq!(run.head_sha, wu.head_commit, "自動 commit の後に採る");

        let snapshot = store.work_unit_write_sets(&wu.id).unwrap();
        assert_eq!(snapshot.len(), 1, "{snapshot:?}");
        assert_eq!(snapshot[0].owner_id, wu.id);
        assert_eq!(snapshot[0].base_sha, wu.base_commit);
        assert_eq!(snapshot[0].head_sha, wu.head_commit);
        assert_eq!(snapshot[0].paths, vec![path.to_string()]);
        assert_eq!(
            snapshot[0].status,
            task_core::write_set::WriteSetStatus::Complete
        );
    }
}

/// atomic の run（celeris は commit しない）: worker が commit した path は確定差分、未コミットの
/// 編集が残れば `incomplete`（未コミットの path は実績に入れない）。
#[tokio::test]
async fn write_set_record_atomic_run_counts_only_commits_from_its_start_head() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (task, repo_id) = project_git_task(&store, repo.path(), "test -f src/lib.rs");
    store.insert(&task).unwrap();
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::from_millis(5)).with_action("atomic", |cwd| {
            std::fs::create_dir_all(cwd.join("src")).unwrap();
            std::fs::write(cwd.join("src/lib.rs"), "// lib\n").unwrap();
            git_out(cwd, &["add", "src/lib.rs"]);
            git_out(
                cwd,
                &[
                    "-c",
                    "user.email=w@example.com",
                    "-c",
                    "user.name=w",
                    "commit",
                    "-q",
                    "-m",
                    "lib",
                ],
            );
            std::fs::write(cwd.join("scratch.txt"), "draft\n").unwrap();
        }),
    );
    let mut d = parallel_dispatcher(store.clone(), adapter, root.path(), 1, 1, 1);
    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);

    let run_id = store.runs_for_task(task.id).unwrap()[0].run_id.clone();
    let records = store.run_write_sets(&run_id).unwrap();
    assert_eq!(records.len(), 1, "{records:?}");
    let r = &records[0];
    assert_eq!(r.repo_id, repo_id);
    assert_eq!(r.work_unit_id, None);
    assert_eq!(
        r.status,
        task_core::write_set::WriteSetStatus::Incomplete,
        "{r:?}"
    );
    assert_eq!(r.paths, vec!["src/lib.rs".to_string()]);
    let main = git_out(repo.path(), &["rev-parse", "main"]);
    assert_eq!(
        r.base_sha.as_deref(),
        Some(main.as_str()),
        "開始 HEAD は main から切った点"
    );
}

/// git が読めない（worker が作業ツリーの `.git` を消した）run でも、run は落ちずに task は done になり、
/// 記録は空配列ではなく `unavailable` と理由で残る。
#[tokio::test]
async fn write_set_record_git_failure_keeps_the_run_and_records_unavailable() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (task, repo_id) = project_git_task(&store, repo.path(), "true");
    store.insert(&task).unwrap();
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::from_millis(5)).with_action("atomic", |cwd| {
            std::fs::remove_file(cwd.join(".git")).unwrap();
        }),
    );
    let mut d = parallel_dispatcher(store.clone(), adapter, root.path(), 1, 1, 1);
    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle, "{report:?}");
    let stored = store.get(task.id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{stored:?}");
    assert_eq!(stored.attempts, 0, "git の失敗で試行を消費しない");

    let run_id = store.runs_for_task(task.id).unwrap()[0].run_id.clone();
    let records = store.run_write_sets(&run_id).unwrap();
    assert_eq!(records.len(), 1, "{records:?}");
    let r = &records[0];
    assert_eq!(r.repo_id, repo_id);
    assert_eq!(r.status, task_core::write_set::WriteSetStatus::Unavailable);
    assert!(r.paths.is_empty());
    assert_eq!((r.base_sha.as_ref(), r.head_sha.as_ref()), (None, None));
    assert!(r.reason.is_some(), "{r:?}");
}
