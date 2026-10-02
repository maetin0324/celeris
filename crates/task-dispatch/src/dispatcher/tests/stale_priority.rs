//! ADR-0130 D5: the pre-review sync queue syncs the longest-stale task first.

use super::*;
use crate::dispatcher::stale_priority::{
    ReviewSyncWait, STALE_PRIORITY_RELIEF_TICKS, StaleCandidate, stale_priority_cmp,
};
use task_core::behind_target::BehindTargetObservation;
use time::format_description::well_known::Rfc3339;

fn commit_file(dir: &std::path::Path, path: &str, content: &str) -> String {
    std::fs::write(dir.join(path), content).unwrap();
    git_out(dir, &["add", path]);
    git_out(dir, &["commit", "-q", "-m", path]);
    git_out(dir, &["rev-parse", "HEAD"])
}

/// Two `reviewing` tasks on the same repo; `a` is listed (and enters the queue) first.
fn two_reviewing_tasks(repo: &std::path::Path, store: &Arc<dyn TaskStore>) -> (Task, Task) {
    let (project, repos) = project_with_repos(store, &[("code", repo, RepoKind::Git)]);
    let make = |offset: i64| {
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
        task.created_at += time::Duration::seconds(offset);
        task
    };
    let (a, b) = (make(0), make(1));
    store.insert(&a).unwrap();
    store.insert(&b).unwrap();
    (a, b)
}

/// Global event id of the task's first `ReviewTargetSynced` (the order of the syncs).
fn synced_at(store: &Arc<dyn TaskStore>, task: &Task) -> u64 {
    store
        .events_for_with_global_ids(task.id)
        .unwrap()
        .into_iter()
        .find_map(|(id, e)| matches!(e, Event::ReviewTargetSynced { .. }).then_some(id))
        .unwrap_or_else(|| panic!("{} was not synced", task.id))
}

fn fixed_clock(d: &mut Dispatcher) -> OffsetDateTime {
    let now = OffsetDateTime::parse("2026-10-02T12:00:00Z", &Rfc3339).unwrap();
    d.test_now = Some(Arc::new(std::sync::Mutex::new(now)));
    now
}

fn checks_adapter() -> Arc<InstantAdapter> {
    Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    })
}

#[tokio::test]
async fn behind_target_stale_priority_more_commits_syncs_first() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (a, b) = two_reviewing_tasks(repo.path(), &store);
    let mut d = worktree_dispatcher(store.clone(), checks_adapter(), ws.path(), None);
    fixed_clock(&mut d);
    // b branches first, so it ends up 2 commits behind main; a is 1 behind.
    d.local_worktree_for(&b).unwrap().ensure_blocking().unwrap();
    commit_file(repo.path(), "m1.txt", "1\n");
    d.local_worktree_for(&a).unwrap().ensure_blocking().unwrap();
    commit_file(repo.path(), "m2.txt", "2\n");

    // One fixed tick: both are offered, the staler one first.
    d.tick().unwrap();
    assert!(synced_at(&store, &b) < synced_at(&store, &a));
    let behind = store.behind_targets(b.id).unwrap();
    assert_eq!(behind.len(), 1);
    assert_eq!(behind[0].target_ref, "refs/heads/main");
    assert!(run_until_idle(&mut d, 100).await.idle);
    for t in [&a, &b] {
        assert_eq!(store.get(t.id).unwrap().unwrap().status, Status::Done);
    }
    assert!(d.review_sync_queue.is_empty());
}

#[tokio::test]
async fn behind_target_stale_priority_older_age_beats_more_commits() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (a, b) = two_reviewing_tasks(repo.path(), &store);
    let mut d = worktree_dispatcher(store.clone(), checks_adapter(), ws.path(), None);
    let now = fixed_clock(&mut d);
    // b is 2 behind, a only 1 behind but has been behind (same target) for 10 minutes.
    d.local_worktree_for(&b).unwrap().ensure_blocking().unwrap();
    commit_file(repo.path(), "m1.txt", "1\n");
    let wt_a = d.local_worktree_for(&a).unwrap();
    wt_a.ensure_blocking().unwrap();
    commit_file(repo.path(), "m2.txt", "2\n");
    store
        .record_behind_target(&BehindTargetObservation {
            task_id: a.id,
            repo_id: a.repos[0].repo_id,
            target_ref: "refs/heads/main".into(),
            target_sha: None,
            head_sha: None,
            commits: Some(1),
            observed_at: (now - time::Duration::minutes(10))
                .format(&Rfc3339)
                .unwrap(),
        })
        .unwrap();

    d.tick().unwrap();
    assert!(synced_at(&store, &a) < synced_at(&store, &b));
    assert!(run_until_idle(&mut d, 100).await.idle);
    // The stale path did not consume attempts.
    for t in [&a, &b] {
        let t = store.get(t.id).unwrap().unwrap();
        assert_eq!(t.status, Status::Done);
        assert_eq!(t.attempts, 0);
    }
}

#[tokio::test]
async fn behind_target_stale_priority_up_to_date_keeps_wait_order() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (a, b) = two_reviewing_tasks(repo.path(), &store);
    let mut d = worktree_dispatcher(store.clone(), checks_adapter(), ws.path(), None);
    fixed_clock(&mut d);
    // Both at main: behind 0 → the existing wait order (a first).
    d.local_worktree_for(&b).unwrap().ensure_blocking().unwrap();
    d.local_worktree_for(&a).unwrap().ensure_blocking().unwrap();
    d.tick().unwrap();
    assert!(synced_at(&store, &a) < synced_at(&store, &b));
    assert!(run_until_idle(&mut d, 100).await.idle);
}

fn candidate(seq: u64, skipped_ticks: u32, behind: Option<(u64, u64)>) -> StaleCandidate {
    StaleCandidate {
        task_id: TaskId::new(),
        wait: ReviewSyncWait { seq, skipped_ticks },
        behind_commits: behind.map(|(c, _)| c),
        behind_age_seconds: behind.map(|(_, age)| age),
    }
}

#[test]
fn behind_target_stale_priority_order_and_fifo_relief() {
    let fresh = candidate(0, 0, Some((0, 0)));
    let unreadable = candidate(1, 0, None);
    let young = candidate(2, 0, Some((5, 10)));
    let old = candidate(3, 0, Some((1, 600)));
    let old_more = candidate(4, 0, Some((2, 600)));
    let starved = candidate(5, STALE_PRIORITY_RELIEF_TICKS, Some((0, 0)));
    let starved_earlier = candidate(1, STALE_PRIORITY_RELIEF_TICKS, None);
    let mut all = vec![
        fresh,
        unreadable,
        young,
        old,
        old_more,
        starved,
        starved_earlier,
    ];
    all.sort_by(stale_priority_cmp);
    let order: Vec<TaskId> = all.iter().map(|c| c.task_id).collect();
    assert_eq!(
        order,
        vec![
            starved_earlier.task_id,
            starved.task_id,
            old_more.task_id,
            old.task_id,
            young.task_id,
            fresh.task_id,
            unreadable.task_id,
        ]
    );
    // Below the relief threshold the task stays in its class.
    let almost = candidate(0, STALE_PRIORITY_RELIEF_TICKS - 1, None);
    assert_eq!(
        stale_priority_cmp(&almost, &young),
        std::cmp::Ordering::Greater
    );
}

#[tokio::test]
async fn behind_target_stale_priority_counts_unselected_ticks() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (a, b) = two_reviewing_tasks(repo.path(), &store);
    let mut d = worktree_dispatcher(store.clone(), checks_adapter(), ws.path(), None);
    let ordered = d
        .order_review_sync_queue(vec![a.clone(), b.clone()])
        .unwrap();
    assert_eq!(
        ordered.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![a.id, b.id]
    );
    for _ in 0..STALE_PRIORITY_RELIEF_TICKS {
        d.note_review_sync_offer(b.id, false);
    }
    // A human-gate deferral is not an eligible tick.
    d.awaiting_human.insert(a.id);
    d.note_review_sync_offer(a.id, false);
    assert_eq!(d.review_sync_queue[&a.id].skipped_ticks, 0);
    assert_eq!(
        d.review_sync_queue[&b.id].skipped_ticks,
        STALE_PRIORITY_RELIEF_TICKS
    );
    // b is now in the FIFO relief slot, ahead of a.
    let ordered = d
        .order_review_sync_queue(vec![a.clone(), b.clone()])
        .unwrap();
    assert_eq!(
        ordered.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![b.id, a.id]
    );
    // Selection leaves the queue; a task that left `reviewing` is dropped.
    d.note_review_sync_offer(b.id, true);
    assert!(!d.review_sync_queue.contains_key(&b.id));
    d.order_review_sync_queue(vec![]).unwrap();
    assert!(d.review_sync_queue.is_empty());
}
