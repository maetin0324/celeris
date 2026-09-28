//! Phase F5-fix7: 依存 WU のブランチが無いと WU の準備が無音で失敗し続ける。本番 2026-09-28
//! （task 01M3MFS5T52FXA63W4V10XGC4S、plan v2 01M3MMTCWK98A11E6HAJ1YS0CN）: replan で足された
//! `remerge`（kind `repair`、Task の worktree で走り Task ブランチに commit した）の後、同じ工程の
//! `reship`（depends_on: remerge）が `celeris-wu/<task>/remerge` を探して見つからず、20 分間 tick ごとに
//! WARN を出すだけで Task は `ready` のまま止まった。

use super::*;

fn transitions_with_reason(events: &[Event], key: &str, reason: &str) -> usize {
    events
        .iter()
        .filter(|e| {
            matches!(e, Event::WorkUnitTransitioned { key: k, reason: r, .. } if k == key && r == reason)
        })
        .count()
}

fn repair_wu(key: &str, phase: &str, depends_on: &[&str]) -> task_core::WorkUnitSpec {
    let mut s = v2_wu(key, phase, depends_on);
    s.kind = task_core::WorkUnitKind::Repair;
    s
}

/// 本番の形: repair WU（Task の worktree で走り、Task ブランチに commit）に同じ工程の WU が依存する。
/// 依存先の WU ブランチは作らず、依存先の `head_commit` を基点にして dispatch する。
#[tokio::test]
async fn a_dependent_of_a_unit_that_committed_on_the_task_branch_dispatches() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "test -f remerge.txt && test -f reship.txt");
    store.insert(&task).unwrap();
    adopt_v2_plan(
        &store,
        task.id,
        &["remerge"],
        vec![
            repair_wu("remerge", "remerge", &[]),
            v2_wu("reship", "remerge", &["remerge"]),
        ],
    );
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::from_millis(20))
            .with_file("remerge", "remerge.txt", "merged")
            .with_action("reship", |cwd| {
                assert!(
                    cwd.join("remerge.txt").is_file(),
                    "reship は remerge の commit の上で始まる"
                );
                std::fs::write(cwd.join("reship.txt"), "shipped").unwrap();
            }),
    );
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 3, 3, 3);
    let report = run_until_idle(&mut d, 600).await;
    assert!(report.idle, "{report:?}");
    let stored = store.get(task.id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{stored:?}");
    assert_eq!(adapter.keys_seen(), vec!["remerge", "reship"]);
    let units = store.work_units_for(task.id).unwrap();
    let remerge = units.iter().find(|u| u.key == "remerge").unwrap();
    let reship = units.iter().find(|u| u.key == "reship").unwrap();
    assert_eq!(remerge.branch, None, "repair WU は Task の worktree で走る");
    assert!(remerge.head_commit.is_some());
    assert_eq!(reship.base_commit, remerge.head_commit);
    let events = events_of(&store, task.id);
    let task_branch = format!("celeris/{}", task.id);
    assert!(events.iter().any(|e| matches!(
        e,
        Event::WorkUnitCommitted { key, branch, .. } if key == "remerge" && branch == &task_branch
    )));
    // WU ブランチは作らない（`branch == None` は「Task の worktree で走った」の印のまま）。
    assert!(!git_ok(
        repo.path(),
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/celeris-wu/{}/remerge", task.id)
        ]
    ));
    assert_eq!(
        transitions_with_reason(&events, "reship", "prepare_failed"),
        0
    );
    let (wu_mismatches, _runs, plan_mismatches, _) =
        task_ops::replay::check_and_apply_execution(store.as_ref(), false).unwrap();
    assert!(wu_mismatches.is_empty(), "{wu_mismatches:?}");
    assert!(plan_mismatches.is_empty(), "{plan_mismatches:?}");
}

/// 依存先の repair WU が何も変えずに done（commit なし）: 依存する WU は Task ブランチの HEAD から切られる。
#[tokio::test]
async fn a_dependent_of_a_unit_that_made_no_commit_bases_on_the_task_branch() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "test -f reship.txt");
    store.insert(&task).unwrap();
    adopt_v2_plan(
        &store,
        task.id,
        &["remerge"],
        vec![
            repair_wu("remerge", "remerge", &[]),
            v2_wu("reship", "remerge", &["remerge"]),
        ],
    );
    let adapter = Arc::new(ParallelWuAdapter::new(Duration::from_millis(20)).with_file(
        "reship",
        "reship.txt",
        "s",
    ));
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 3, 3, 3);
    let report = run_until_idle(&mut d, 600).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
    assert_eq!(adapter.keys_seen(), vec!["remerge", "reship"]);
    let units = store.work_units_for(task.id).unwrap();
    let remerge = units.iter().find(|u| u.key == "remerge").unwrap();
    let reship = units.iter().find(|u| u.key == "reship").unwrap();
    // commit が無いので remerge の head は Task ブランチの基点のまま（= main）。
    let main = git_out(repo.path(), &["rev-parse", "main"]);
    assert_eq!(reship.base_commit.as_deref(), Some(main.as_str()));
    assert!(remerge.head_commit.is_none() || remerge.head_commit.as_deref() == Some(main.as_str()));
}

/// 依存先の成果が解決できない（WU ブランチが消え、記録した commit もリポジトリに無い）: 1 回目で WU を
/// `blocked(question)`（reason `prepare_failed`）にし、Task を blocked にして人に聞く。tick を重ねても
/// 同じ失敗を繰り返さない。人が直して回答すると WU は ready に戻り、dispatch される。
#[tokio::test]
async fn an_unresolvable_dependency_blocks_the_unit_once_and_asks_a_human() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "test -f b.txt");
    store.insert(&task).unwrap();
    adopt_v2_plan(
        &store,
        task.id,
        &["build"],
        vec![v2_wu("a", "build", &[]), v2_wu("b", "build", &["a"])],
    );
    // a は WU ブランチで done になった記録だけがあり、ブランチも commit も無い（人が消した等）。
    let units = store.work_units_for(task.id).unwrap();
    let a = units.iter().find(|u| u.key == "a").unwrap().clone();
    let b = units.iter().find(|u| u.key == "b").unwrap().clone();
    let a_branch = format!("celeris-wu/{}/a", task.id);
    let mut a_done = a.clone();
    a_done.status = task_core::WorkUnitStatus::Done;
    a_done.branch = Some(a_branch.clone());
    a_done.head_commit = Some("0123456789abcdef0123456789abcdef01234567".into());
    store
        .work_unit_transition(
            task.id,
            a_done,
            Event::WorkUnitTransitioned {
                work_unit_id: a.id.clone(),
                key: a.key.clone(),
                from: a.status,
                to: task_core::WorkUnitStatus::Done,
                reason: "fixture".into(),
                run_id: None,
            },
        )
        .unwrap();
    // replay が行を作り直せるよう、commit の記録も events に残す（本番の `WorkUnitCommitted` と同じ形）。
    store
        .append_event(
            task.id,
            &Event::WorkUnitCommitted {
                work_unit_id: a.id.clone(),
                key: a.key.clone(),
                branch: a_branch.clone(),
                base: None,
                commit: "0123456789abcdef0123456789abcdef01234567".into(),
            },
        )
        .unwrap();
    let mut b_ready = b.clone();
    b_ready.status = task_core::WorkUnitStatus::Ready;
    store
        .work_unit_transition(
            task.id,
            b_ready,
            Event::WorkUnitTransitioned {
                work_unit_id: b.id.clone(),
                key: b.key.clone(),
                from: b.status,
                to: task_core::WorkUnitStatus::Ready,
                reason: "dependency_ready".into(),
                run_id: None,
            },
        )
        .unwrap();
    let adapter =
        Arc::new(ParallelWuAdapter::new(Duration::from_millis(20)).with_file("b", "b.txt", "b"));
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 3, 3, 3);
    let report = run_until_idle(&mut d, 50).await;
    assert!(report.idle, "{report:?}");
    let stored = store.get(task.id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Blocked, "{stored:?}");
    assert!(adapter.keys_seen().is_empty(), "b は dispatch されない");
    let b_row = store
        .work_units_for(task.id)
        .unwrap()
        .into_iter()
        .find(|u| u.key == "b")
        .unwrap();
    assert_eq!(b_row.status, task_core::WorkUnitStatus::Blocked);
    assert_eq!(
        b_row.blocked_reason,
        Some(task_core::WorkUnitBlockedReason::Question)
    );
    let events = events_of(&store, task.id);
    assert_eq!(transitions_with_reason(&events, "b", "prepare_failed"), 1);
    assert!(events.iter().any(|e| matches!(
        e,
        Event::QuestionRaised { text, .. } if text.contains("dependency branch of a")
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        Event::WorkerProgress { msg, .. } if msg.starts_with("prepare_failed: work unit b")
    )));
    // tick を重ねても繰り返さない（Task は blocked、WU も blocked）。
    for _ in 0..20 {
        d.tick().unwrap();
    }
    let events = events_of(&store, task.id);
    assert_eq!(transitions_with_reason(&events, "b", "prepare_failed"), 1);
    // WU の行（blocked(question)）は events から作り直せる。
    let (wu_mismatches, _runs, plan_mismatches, _) =
        task_ops::replay::check_and_apply_execution(store.as_ref(), false).unwrap();
    assert!(wu_mismatches.is_empty(), "{wu_mismatches:?}");
    assert!(plan_mismatches.is_empty(), "{plan_mismatches:?}");

    // 人がブランチを作り直して回答する → b は ready に戻り、a のブランチから切られて走る。
    git_out(repo.path(), &["branch", &a_branch, "main"]);
    store
        .apply_transition_with_events(task.id, Trigger::Answer, vec![])
        .unwrap();
    let report = run_until_idle(&mut d, 600).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
    assert_eq!(adapter.keys_seen(), vec!["b"]);
    let b_row = store
        .work_units_for(task.id)
        .unwrap()
        .into_iter()
        .find(|u| u.key == "b")
        .unwrap();
    let main = git_out(repo.path(), &["rev-parse", "main"]);
    assert_eq!(b_row.base_commit.as_deref(), Some(main.as_str()));
}

/// 一時的な失敗（git の錠など）は上限まではバックオフしてやり直し（イベントは残さない）、上限の回で
/// blocked にする。
#[tokio::test]
async fn transient_prepare_failures_retry_with_backoff_then_block() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    adopt_v2_plan(&store, task.id, &["build"], vec![v2_wu("a", "build", &[])]);
    let a = store
        .work_units_for(task.id)
        .unwrap()
        .into_iter()
        .find(|u| u.key == "a")
        .unwrap();
    let adapter = Arc::new(ParallelWuAdapter::new(Duration::from_millis(20)));
    let mut d = parallel_dispatcher(store.clone(), adapter, root.path(), 3, 3, 3);
    let err = WuPrepareError::transient("Unable to create '.git/index.lock': File exists".into());
    for n in 1..MAX_WU_PREPARE_ATTEMPTS {
        d.on_work_unit_prepare_failed(&task, &a, &err, false)
            .unwrap();
        let f = d.wu_prepare_failures.get(&a.id).copied().unwrap();
        assert_eq!(f.count, n);
        assert!(f.retry_at > OffsetDateTime::now_utc());
    }
    let events = events_of(&store, task.id);
    assert_eq!(transitions_with_reason(&events, "a", "prepare_failed"), 0);
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Ready);
    d.on_work_unit_prepare_failed(&task, &a, &err, false)
        .unwrap();
    assert!(!d.wu_prepare_failures.contains_key(&a.id));
    let events = events_of(&store, task.id);
    assert_eq!(transitions_with_reason(&events, "a", "prepare_failed"), 1);
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Blocked);
    assert_eq!(wu_prepare_backoff(1), time::Duration::seconds(2));
    assert_eq!(wu_prepare_backoff(4), time::Duration::seconds(16));
    assert_eq!(wu_prepare_backoff(10), time::Duration::seconds(60));
}
