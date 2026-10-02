use super::*;

fn reviewing_task(repo: &std::path::Path, store: &Arc<dyn TaskStore>) -> Task {
    let (project, repos) = project_with_repos(store, &[("code", repo, RepoKind::Git)]);
    let mut task = git_task(
        repo,
        None,
        Check::Command {
            cmd: "git merge-base --is-ancestor main HEAD".into(),
            expect_exit: 0,
        },
    );
    task.project_id = Some(project);
    task.repos = repos.iter().map(RepoRef::of).collect();
    task.status = Status::Reviewing;
    task
}

fn commit_file(repo: &std::path::Path, path: &str, content: &str) -> String {
    std::fs::write(repo.join(path), content).unwrap();
    git_out(repo, &["add", path]);
    git_out(repo, &["commit", "-q", "-m", path]);
    git_out(repo, &["rev-parse", "HEAD"])
}

fn check_snapshot(store: &Arc<dyn TaskStore>, task: &Task, target: &str, head: &str) {
    let snapshots: Vec<_> = store
        .events_for(task.id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| {
            if let Event::ReviewTargetSynced {
                target_sha,
                reviewed_sha,
                merge_candidate_sha,
                ..
            } = e
            {
                Some((target_sha, reviewed_sha, merge_candidate_sha))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(snapshots, vec![(target.into(), head.into(), head.into())]);
}

#[tokio::test]
async fn target_sync_root_reviews_the_rebased_sha() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = reviewing_task(repo.path(), &store);
    store.insert(&task).unwrap();
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = worktree_dispatcher(store.clone(), adapter, ws.path(), None);
    let wt = d.local_worktree_for(&task).unwrap();
    wt.ensure_blocking().unwrap();
    let before = commit_file(&wt.dir, "task.txt", "task\n");
    let target = commit_file(repo.path(), "main.txt", "main\n");
    assert!(
        d.spawn_review(task.id, "worker".into(), &ReviewSubject::default())
            .unwrap()
    );
    let reviewed = git_out(&wt.dir, &["rev-parse", "HEAD"]);
    assert_ne!(before, reviewed);
    assert!(git_ok(
        &wt.dir,
        &["merge-base", "--is-ancestor", &target, &reviewed]
    ));
    check_snapshot(&store, &task, &target, &reviewed);
    assert!(run_until_idle(&mut d, 100).await.idle);
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
}

#[tokio::test]
async fn target_sync_tree_child_uses_parent_branch() {
    let repo = tempfile::tempdir().unwrap();
    let base = init_test_repo(repo.path());
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let parent = git_task(
        repo.path(),
        None,
        Check::Command {
            cmd: ":".into(),
            expect_exit: 0,
        },
    );
    let parent_branch = format!("celeris/{}", parent.id);
    let parent_dir = ws.path().join("parent");
    git_out(
        repo.path(),
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            &parent_branch,
            parent_dir.to_str().unwrap(),
            "main",
        ],
    );
    let mut child = reviewing_task(repo.path(), &store);
    child.tree = Some(TreeInfo::child_of(
        &parent,
        ParentUnit {
            task_id: parent.id,
            plan_id: "plan".into(),
            unit_key: "child".into(),
            stage: "s1".into(),
            attempt: 1,
        },
        Some(base),
    ));
    child.acceptance[0].check = Check::Command {
        cmd: format!("git merge-base --is-ancestor {parent_branch} HEAD"),
        expect_exit: 0,
    };
    store.insert(&child).unwrap();
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = worktree_dispatcher(store.clone(), adapter, ws.path(), None);
    let wt = d.local_worktree_for(&child).unwrap();
    wt.ensure_blocking().unwrap();
    // Atomic tree children historically leave run output uncommitted until done.
    std::fs::write(wt.dir.join("child.txt"), "child\n").unwrap();
    let before = git_out(&wt.dir, &["rev-parse", "HEAD"]);
    let target = commit_file(&parent_dir, "parent.txt", "parent\n");
    assert!(
        d.spawn_review(child.id, "worker".into(), &ReviewSubject::default())
            .unwrap()
    );
    let reviewed = git_out(&wt.dir, &["rev-parse", "HEAD"]);
    assert_ne!(before, reviewed);
    assert!(git_ok(
        &wt.dir,
        &["ls-files", "--error-unmatch", "child.txt"]
    ));
    check_snapshot(&store, &child, &target, &reviewed);
    assert!(run_until_idle(&mut d, 100).await.idle);
    assert_eq!(store.get(child.id).unwrap().unwrap().status, Status::Done);
}

#[tokio::test]
async fn target_sync_conflict_stops_review_and_preserves_branch() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = reviewing_task(repo.path(), &store);
    store.insert(&task).unwrap();
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = worktree_dispatcher(store.clone(), adapter, ws.path(), None);
    let wt = d.local_worktree_for(&task).unwrap();
    wt.ensure_blocking().unwrap();
    let before = commit_file(&wt.dir, "README.md", "task\n");
    commit_file(repo.path(), "README.md", "main\n");
    assert!(
        d.spawn_review(task.id, "worker".into(), &ReviewSubject::default())
            .unwrap()
    );
    assert_eq!(git_out(&wt.dir, &["rev-parse", "HEAD"]), before);
    assert_eq!(git_out(repo.path(), &["rev-parse", &wt.branch]), before);
    assert_eq!(
        std::fs::read_to_string(wt.dir.join("README.md")).unwrap(),
        "task\n"
    );
    assert!(d.reviewing.is_empty());
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Failed);
    assert!(
        !store
            .events_for(task.id)
            .unwrap()
            .iter()
            .any(|(_, e)| matches!(e, Event::ReviewVerdict { .. }))
    );
}

#[tokio::test]
async fn target_sync_remote_records_skip_without_local_git() {
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = remote_task(ws.path(), None, Vec::new());
    task.status = Status::Reviewing;
    store.insert(&task).unwrap();
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = worktree_dispatcher(store.clone(), adapter, ws.path(), None);
    assert!(
        d.spawn_review(task.id, "worker".into(), &ReviewSubject::default())
            .unwrap()
    );
    let events = store.events_for(task.id).unwrap();
    assert!(events.iter().any(|(_, e)| matches!(e, Event::WorkerProgress { msg, .. } if msg == "review target sync skipped: remote workspace")));
    assert!(
        !events
            .iter()
            .any(|(_, e)| matches!(e, Event::ReviewTargetSynced { .. }))
    );
}
