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
async fn target_sync_pre_review_sync_root_reviews_the_rebased_sha() {
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
async fn target_sync_pre_review_sync_tree_child_uses_parent_branch() {
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

fn done_adapter() -> Arc<InstantAdapter> {
    Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    })
}

fn set_check(task: &mut Task, cmd: String) {
    task.acceptance[0].check = Check::Command {
        cmd,
        expect_exit: 0,
    };
}

fn events_of(store: &Arc<dyn TaskStore>, task: &Task) -> Vec<Event> {
    store
        .events_for(task.id)
        .unwrap()
        .into_iter()
        .map(|(_, e)| e)
        .collect()
}

fn progress_with(events: &[Event], needle: &str) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, Event::WorkerProgress { msg, .. } if msg.contains(needle)))
        .count()
}

fn no_review_fail(events: &[Event]) -> bool {
    !events
        .iter()
        .any(|e| matches!(e, Event::Transitioned { reason, .. } if reason == "review_fail"))
}

#[tokio::test]
async fn integration_repair_schedules_atomic_and_preserves_results() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = reviewing_task(repo.path(), &store);
    set_check(&mut task, ":".into());
    store.insert(&task).unwrap();
    let mut d = worktree_dispatcher(store.clone(), done_adapter(), ws.path(), None);
    let wt = d.local_worktree_for(&task).unwrap();
    wt.ensure_blocking().unwrap();
    let before = commit_file(&wt.dir, "README.md", "task\n");
    let target = commit_file(repo.path(), "README.md", "main\n");
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
    let current = store.get(task.id).unwrap().unwrap();
    assert_eq!(current.status, Status::Ready);
    assert_eq!(current.attempts, task.attempts);
    assert!(git_out(&wt.dir, &["status", "--porcelain"]).is_empty());
    let units = store.work_units_for(task.id).unwrap();
    assert_eq!(units.len(), 2);
    assert_eq!(units[0].key, "main");
    assert_eq!(units[0].status, task_core::WorkUnitStatus::Done);
    let repair = &units[1];
    assert_eq!(repair.key, "integration-repair-1");
    assert_eq!(repair.kind, task_core::WorkUnitKind::Repair);
    assert_eq!(repair.status, task_core::WorkUnitStatus::Ready);
    assert_eq!(repair.spec.phase, None);
    assert_eq!(repair.spec.checks.len(), 2);
    assert_eq!(
        repair.spec.checks[0].cmd,
        format!("git merge-base --is-ancestor {target} HEAD")
    );
    assert_eq!(
        repair.spec.checks[1].cmd,
        "test -z \"$(git status --porcelain)\""
    );
    assert!(repair.spec.objective.contains(&before));
    assert!(repair.spec.objective.contains("README.md"));
    let events = events_of(&store, &task);
    assert!(no_review_fail(&events));
    assert!(events.iter().any(|e| matches!(e, Event::IntegrationRepairScheduled { work_unit_id, target_sha, before_sha, attempt: 1, .. } if work_unit_id == &repair.id && target_sha == &target && before_sha == &before)));
    assert!(events.iter().any(
        |e| matches!(e, Event::RepairScheduled { class, .. } if class == "integration_repair")
    ));
    assert!(events.iter().any(|e| matches!(e, Event::WorkUnitTransitioned { reason, .. } if reason == "integration_repair")));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::ReviewTargetSynced { .. }))
    );
    assert_eq!(git_out(repo.path(), &["rev-parse", &wt.branch]), before);
}

/// ADR-0118 D6 付記: 未コミットの変更があれば何も触らず、同期を省いて従来経路で review へ進む。
#[tokio::test]
async fn pre_review_sync_dirty_keeps_attempts() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = reviewing_task(repo.path(), &store);
    set_check(&mut task, ":".into());
    store.insert(&task).unwrap();
    let mut d = worktree_dispatcher(store.clone(), done_adapter(), ws.path(), None);
    let wt = d.local_worktree_for(&task).unwrap();
    wt.ensure_blocking().unwrap();
    let before = commit_file(&wt.dir, "task.txt", "task\n");
    std::fs::write(wt.dir.join("wip.txt"), "wip\n").unwrap();
    commit_file(repo.path(), "main.txt", "main\n");
    assert!(
        d.spawn_review(task.id, "worker".into(), &ReviewSubject::default())
            .unwrap()
    );
    assert_eq!(git_out(&wt.dir, &["rev-parse", "HEAD"]), before);
    assert_eq!(
        std::fs::read_to_string(wt.dir.join("wip.txt")).unwrap(),
        "wip\n"
    );
    assert!(run_until_idle(&mut d, 100).await.idle);
    let done = store.get(task.id).unwrap().unwrap();
    assert_eq!(done.status, Status::Done);
    assert_eq!(done.attempts, task.attempts);
    let events = events_of(&store, &task);
    assert!(no_review_fail(&events));
    assert_eq!(progress_with(&events, "uncommitted changes"), 1);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::ReviewTargetSynced { .. }))
    );
}

/// ADR-0118 D4 付記: 検査中に target が進んだ（snapshot 変化）review は不合格にせず、attempts を
/// 消費しないで再 sync → 再 check → 再 review する。
#[tokio::test]
async fn pre_review_sync_target_advanced_keeps_attempts() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let ws = tempfile::tempdir().unwrap();
    let flag = tempfile::tempdir().unwrap();
    let flag = flag.path().join("advanced");
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = reviewing_task(repo.path(), &store);
    // 1 回目の検査の最中にだけ main を進める。
    set_check(
        &mut task,
        format!(
            "test -e {f} || {{ touch {f} && git update-ref refs/heads/main \"$(git commit-tree 'HEAD^{{tree}}' -p HEAD -m bump)\"; }}",
            f = flag.display()
        ),
    );
    store.insert(&task).unwrap();
    let mut d = worktree_dispatcher(store.clone(), done_adapter(), ws.path(), None);
    let wt = d.local_worktree_for(&task).unwrap();
    wt.ensure_blocking().unwrap();
    commit_file(&wt.dir, "task.txt", "task\n");
    commit_file(repo.path(), "main.txt", "main\n");
    assert!(
        d.spawn_review(task.id, "worker".into(), &ReviewSubject::default())
            .unwrap()
    );
    assert!(run_until_idle(&mut d, 200).await.idle);
    let done = store.get(task.id).unwrap().unwrap();
    assert_eq!(done.status, Status::Done);
    assert_eq!(done.attempts, task.attempts);
    let events = events_of(&store, &task);
    assert!(no_review_fail(&events));
    let advanced: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::ReviewTargetAdvanced {
                reviewed_sha,
                target_sha,
                attempt,
                ..
            } => Some((reviewed_sha.clone(), target_sha.clone(), *attempt)),
            _ => None,
        })
        .collect();
    assert_eq!(advanced.len(), 1);
    assert_eq!(advanced[0].2, 1);
    assert_ne!(advanced[0].0, advanced[0].1);
    let synced: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::ReviewTargetSynced {
                target_sha,
                reviewed_sha,
                ..
            } => Some((target_sha.clone(), reviewed_sha.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(synced.len(), 2);
    // 2 回目は進んだ target（1 回目の stale が記録した target）へ同期し直してから検査した。
    assert_eq!(synced[1].0, advanced[0].1);
    assert_eq!(git_out(&wt.dir, &["rev-parse", "HEAD"]), synced[1].1);
}

/// ADR-0118 D4 付記: target が動き続けると自動の再同期は上限で止まる。attempts は変えず、`failed` にも
/// せず、理由と両 SHA を一度だけ残して reviewing のまま人の再開を待つ。
#[tokio::test]
async fn pre_review_sync_target_advanced_limit_halts_without_attempts() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = reviewing_task(repo.path(), &store);
    set_check(
        &mut task,
        "git update-ref refs/heads/main \"$(git commit-tree 'HEAD^{tree}' -p HEAD -m bump)\""
            .into(),
    );
    store.insert(&task).unwrap();
    let mut d = worktree_dispatcher(store.clone(), done_adapter(), ws.path(), None);
    let wt = d.local_worktree_for(&task).unwrap();
    wt.ensure_blocking().unwrap();
    commit_file(&wt.dir, "task.txt", "task\n");
    assert!(
        d.spawn_review(task.id, "worker".into(), &ReviewSubject::default())
            .unwrap()
    );
    run_until_idle(&mut d, 100).await;
    let current = store.get(task.id).unwrap().unwrap();
    assert_eq!(current.status, Status::Reviewing);
    assert_eq!(current.attempts, task.attempts);
    let events = events_of(&store, &task);
    assert!(no_review_fail(&events));
    let advanced = events
        .iter()
        .filter(|e| matches!(e, Event::ReviewTargetAdvanced { .. }))
        .count() as u32;
    assert_eq!(advanced, task_ops::delivery::MAX_TARGET_RESYNCS + 1);
    assert_eq!(
        progress_with(
            &events,
            crate::dispatcher::review_spawn::TARGET_RESYNC_HALTED_PREFIX
        ),
        1
    );
    assert!(
        !d.spawn_review(task.id, "worker".into(), &ReviewSubject::default())
            .unwrap()
    );
    assert!(d.reviewing.is_empty());
}

#[tokio::test]
async fn integration_repair_schedules_tree_child_on_parent_target() {
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
    set_check(&mut child, ":".into());
    store.insert(&child).unwrap();
    let mut d = worktree_dispatcher(store.clone(), done_adapter(), ws.path(), None);
    let wt = d.local_worktree_for(&child).unwrap();
    wt.ensure_blocking().unwrap();
    let before = commit_file(&wt.dir, "README.md", "child\n");
    commit_file(&parent_dir, "README.md", "parent\n");
    assert!(
        d.spawn_review(child.id, "worker".into(), &ReviewSubject::default())
            .unwrap()
    );
    assert_eq!(git_out(&wt.dir, &["rev-parse", "HEAD"]), before);
    let current = store.get(child.id).unwrap().unwrap();
    assert_eq!(current.status, Status::Ready);
    assert_eq!(current.attempts, child.attempts);
    assert!(d.reviewing.is_empty());
    let events = events_of(&store, &child);
    assert!(no_review_fail(&events));
    assert!(events.iter().any(|e| matches!(e, Event::IntegrationRepairScheduled { target_ref, before_sha, .. } if target_ref == &format!("refs/heads/{parent_branch}") && before_sha == &before)));
    assert!(d.child_merge_candidates(child.id).is_empty());
    assert_eq!(git_out(repo.path(), &["rev-parse", &wt.branch]), before);
    assert!(!git_ok(
        repo.path(),
        &["merge-base", "--is-ancestor", &parent_branch, &before]
    ));
}

#[tokio::test]
async fn integration_repair_schedules_limit_falls_back_to_unsynced_review() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = reviewing_task(repo.path(), &store);
    set_check(&mut task, ":".into());
    store.insert(&task).unwrap();
    let mut d = worktree_dispatcher(store.clone(), done_adapter(), ws.path(), None);
    let wt = d.local_worktree_for(&task).unwrap();
    wt.ensure_blocking().unwrap();
    let before = commit_file(&wt.dir, "README.md", "task\n");
    let target = commit_file(repo.path(), "README.md", "main\n");
    assert!(
        d.spawn_review(task.id, "worker".into(), &ReviewSubject::default())
            .unwrap()
    );
    store
        .apply_transition(task.id, task_core::Trigger::Dispatch, None)
        .unwrap();
    store
        .apply_transition(task.id, task_core::Trigger::WorkerDone, None)
        .unwrap();
    assert!(
        d.spawn_review(task.id, "worker".into(), &ReviewSubject::default())
            .unwrap()
    );
    let units = store.work_units_for(task.id).unwrap();
    assert_eq!(units.len(), 3);
    assert_eq!(units[2].key, "integration-repair-2");
    assert_eq!(units[2].status, task_core::WorkUnitStatus::Ready);
    store
        .apply_transition(task.id, task_core::Trigger::Dispatch, None)
        .unwrap();
    store
        .apply_transition(task.id, task_core::Trigger::WorkerDone, None)
        .unwrap();
    let attempts = store.get(task.id).unwrap().unwrap().attempts;
    assert!(
        d.spawn_review(task.id, "worker".into(), &ReviewSubject::default())
            .unwrap()
    );
    assert!(d.reviewing.contains_key(&task.id));
    assert_eq!(
        store.get(task.id).unwrap().unwrap().status,
        Status::Reviewing
    );
    assert_eq!(store.get(task.id).unwrap().unwrap().attempts, attempts);
    assert_eq!(git_out(&wt.dir, &["rev-parse", "HEAD"]), before);
    let events = events_of(&store, &task);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::IntegrationRepairExhausted {
        reason: task_core::IntegrationRepairExhaustReason::LimitReached,
        target_sha, before_sha, fallback: true, ..
    } if target_sha == &target && before_sha == &before))
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::ReviewTargetSynced { .. }))
    );
    assert!(no_review_fail(&events));
}

#[tokio::test]
async fn target_sync_pre_review_sync_remote_records_skip_without_local_git() {
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
