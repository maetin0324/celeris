use super::*;
use task_core::{
    ArtifactRef, Budget, Check, Criterion, Event, SqliteStore, Task, TaskKind, Tier, WorkerHint,
    WorkspaceSpec,
};
use time::format_description::well_known::Rfc3339;

fn sample_task(title: &str, status: Status) -> Task {
    let now = OffsetDateTime::now_utc();
    Task {
        harness_id: None,
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: title.into(),
        objective: "o".into(),
        acceptance: vec![Criterion {
            text: "c".into(),
            check: Check::Human,
        }],
        inputs: Vec::<ArtifactRef>::new(),
        depends_on: vec![],
        status,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
        },
        workspace: WorkspaceSpec::local("/tmp/ws"),
        repos: Vec::new(),
        budget: Budget {
            max_turns: 1,
            max_wall_secs: 1,
            max_retries: 2,
            max_tokens: None,
        },
        attempts: 3,
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
        labels: Vec::new(),
        category: Default::default(),
    }
}

/// 結びの作成時刻。試験の release はこれより後（`2026-10-26T*`・`2026-10-27T*`）に昇格させる。
fn linked_at() -> OffsetDateTime {
    OffsetDateTime::parse("2026-10-26T00:00:00Z", &Rfc3339).expect("linked_at")
}

/// 修正 task を `waiting` に結んで作る（`POST /tasks` と同じく作成と同じ transaction）。
fn create_fix(store: &SqliteStore, fix: &Task, waiting: &[TaskId]) {
    let now = linked_at();
    store
        .create_task_with(fix, "human:test", vec![], |tx| {
            SqliteStore::unblocks_insert_tx(tx, fix.id, waiting, now)
        })
        .expect("create fix task with unblocks");
}

fn write_release(dir: &Path, sha12: &str, promoted_at: &str, tasks: &[TaskId]) {
    let release = dir.join(sha12);
    std::fs::create_dir_all(&release).expect("release dir");
    let tasks: Vec<serde_json::Value> = tasks
        .iter()
        .map(|id| serde_json::json!({"task_id": id.to_string(), "title": "fix"}))
        .collect();
    std::fs::write(
        release.join("promoted.json"),
        serde_json::json!({
            "promoted_at": promoted_at,
            "mode": "stop-start",
            "from": null,
            "included": {"releases": [sha12], "tasks": tasks, "complete": true},
        })
        .to_string(),
    )
    .expect("promoted.json");
}

fn reopen_count(store: &SqliteStore, id: TaskId) -> usize {
    store
        .events_for(id)
        .expect("events")
        .iter()
        .filter(|(_, e)| matches!(e, Event::Transitioned { reason, .. } if reason == "reopen"))
        .count()
}

/// 受け入れ条件 0: 修正 task を含む release の昇格記録で、結んだ failed task が 1 度だけ reopen される。
/// 二度目の昇格（修正を含む後の release）でも、同じ release の再走査でも重ならない。
#[test]
fn unblocks_promotion_reopens_the_failed_waiting_task_exactly_once() {
    let store = SqliteStore::open_in_memory().expect("store");
    let tmp = tempfile::tempdir().expect("tempdir");
    let releases_dir = tmp.path();
    let waiting = sample_task("waits for the fix", Status::Failed);
    store.insert(&waiting).expect("insert waiting");
    let fix = sample_task("the fix", Status::Done);
    create_fix(&store, &fix, &[waiting.id]);

    // 修正を含まない release の昇格では何も起きない。
    let unrelated = sample_task("unrelated", Status::Done);
    store.insert(&unrelated).expect("insert unrelated");
    write_release(
        releases_dir,
        "aaaaaaaaaaaa",
        "2026-10-26T08:00:00Z",
        &[unrelated.id],
    );
    let now = OffsetDateTime::now_utc();
    let fired = sweep(&store, &read_promoted_releases(releases_dir), now).expect("sweep");
    assert!(fired.is_empty(), "{fired:?}");
    assert_eq!(
        store.get(waiting.id).unwrap().unwrap().status,
        Status::Failed
    );

    // 修正を含む release の昇格で reopen（attempts は 0、system コメントが同じ transaction で残る）。
    write_release(
        releases_dir,
        "bbbbbbbbbbbb",
        "2026-10-26T10:00:00Z",
        &[fix.id],
    );
    let fired = sweep(&store, &read_promoted_releases(releases_dir), now).expect("sweep");
    assert_eq!(
        fired,
        vec![UnblockOutcome {
            fix_task_id: fix.id,
            waiting_task_id: waiting.id,
            release: "bbbbbbbbbbbb".into(),
            action: UnblockAction::Reopened,
        }]
    );
    let after = store.get(waiting.id).unwrap().unwrap();
    assert_eq!(after.status, Status::Ready);
    assert_eq!(after.attempts, 0);
    assert_eq!(reopen_count(&store, waiting.id), 1);
    let comments = store.comments_for(waiting.id).expect("comments");
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].author_kind, CommentAuthorKind::System);
    assert!(comments[0].body.contains(&fix.id.to_string()));
    assert!(comments[0].body.contains("bbbbbbbbbbbb"));
    let link = &store.unblocks_of(fix.id).expect("links")[0];
    assert_eq!(link.fired_release.as_deref(), Some("bbbbbbbbbbbb"));
    assert_eq!(link.fired_action, Some(UnblockAction::Reopened));

    // 再開した task がまた失敗しても、二度目の昇格（修正を含む後の release）で重ねて reopen しない。
    store
        .apply_transition(
            waiting.id,
            task_core::transition::Trigger::TokenBudgetExhausted,
            None,
        )
        .expect("the reopened task fails again");
    assert_eq!(
        store.get(waiting.id).unwrap().unwrap().status,
        Status::Failed
    );
    write_release(
        releases_dir,
        "cccccccccccc",
        "2026-10-26T12:00:00Z",
        &[fix.id],
    );
    let fired = sweep(&store, &read_promoted_releases(releases_dir), now).expect("sweep");
    assert!(fired.is_empty(), "{fired:?}");
    let fired = sweep(&store, &read_promoted_releases(releases_dir), now).expect("sweep");
    assert!(fired.is_empty(), "{fired:?}");
    assert_eq!(reopen_count(&store, waiting.id), 1);
    assert_eq!(store.comments_for(waiting.id).unwrap().len(), 1);
    assert!(store.unblock_pending().expect("pending").is_empty());
}

/// D3: blocked はコメントだけ（状態を変えない）、待っていない task は skipped。子の修正 task は親が
/// release に入れば入ったとみなす。
#[test]
fn unblocks_blocked_waiters_get_a_comment_and_child_fixes_follow_their_parent() {
    let store = SqliteStore::open_in_memory().expect("store");
    let tmp = tempfile::tempdir().expect("tempdir");
    let blocked = sample_task("blocked waiter", Status::Blocked);
    let ready = sample_task("already reopened by a human", Status::Ready);
    store.insert(&blocked).expect("insert blocked");
    store.insert(&ready).expect("insert ready");
    let parent = sample_task("fix parent", Status::Done);
    store.insert(&parent).expect("insert parent");
    let mut child = sample_task("fix child", Status::Done);
    child.parent_id = Some(parent.id);
    create_fix(&store, &child, &[blocked.id, ready.id]);

    write_release(
        tmp.path(),
        "dddddddddddd",
        "2026-10-27T01:00:00Z",
        &[parent.id],
    );
    let now = OffsetDateTime::now_utc();
    let mut fired = sweep(&store, &read_promoted_releases(tmp.path()), now).expect("sweep");
    fired.sort_by_key(|o| o.waiting_task_id.to_string());
    let mut expected = vec![
        UnblockOutcome {
            fix_task_id: child.id,
            waiting_task_id: blocked.id,
            release: "dddddddddddd".into(),
            action: UnblockAction::Commented,
        },
        UnblockOutcome {
            fix_task_id: child.id,
            waiting_task_id: ready.id,
            release: "dddddddddddd".into(),
            action: UnblockAction::Skipped,
        },
    ];
    expected.sort_by_key(|o| o.waiting_task_id.to_string());
    assert_eq!(fired, expected);
    assert_eq!(
        store.get(blocked.id).unwrap().unwrap().status,
        Status::Blocked
    );
    assert_eq!(store.comments_for(blocked.id).unwrap().len(), 1);
    assert_eq!(store.get(ready.id).unwrap().unwrap().status, Status::Ready);
    assert!(store.comments_for(ready.id).unwrap().is_empty());
    assert_eq!(reopen_count(&store, blocked.id), 0);
}

/// `included` が `null` の昇格（preview が取れなかった）では検出しない。壊れた promoted.json は飛ばす。
#[test]
fn unblocks_read_promoted_releases_skips_missing_included_and_broken_files() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let null_dir = tmp.path().join("eeeeeeeeeeee");
    std::fs::create_dir_all(&null_dir).unwrap();
    std::fs::write(
        null_dir.join("promoted.json"),
        r#"{"promoted_at":"2026-10-27T02:00:00Z","included":null}"#,
    )
    .unwrap();
    let broken = tmp.path().join("ffffffffffff");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join("promoted.json"), "{").unwrap();
    std::fs::create_dir_all(tmp.path().join("not-promoted")).unwrap();
    let id = TaskId::new();
    write_release(tmp.path(), "111111111111", "2026-10-27T03:00:00Z", &[id]);
    let releases = read_promoted_releases(tmp.path());
    assert_eq!(
        releases,
        vec![
            PromotedRelease {
                sha12: "eeeeeeeeeeee".into(),
                promoted_at: "2026-10-27T02:00:00Z".into(),
                task_ids: vec![],
            },
            PromotedRelease {
                sha12: "111111111111".into(),
                promoted_at: "2026-10-27T03:00:00Z".into(),
                task_ids: vec![id.to_string()],
            },
        ]
    );
}

#[test]
fn unblocks_validation_rejects_unknown_duplicate_and_too_many() {
    let store = SqliteStore::open_in_memory().expect("store");
    let known = sample_task("known", Status::Failed);
    store.insert(&known).unwrap();
    assert!(validate_unblocks(&store, &[known.id]).is_ok());
    assert!(matches!(
        validate_unblocks(&store, &[TaskId::new()]),
        Err(OpsError::Validation(m)) if m.contains("does not exist")
    ));
    assert!(matches!(
        validate_unblocks(&store, &[known.id, known.id]),
        Err(OpsError::Validation(m)) if m.contains("twice")
    ));
    let many: Vec<TaskId> = (0..=MAX_UNBLOCKS).map(|_| TaskId::new()).collect();
    assert!(matches!(
        validate_unblocks(&store, &many),
        Err(OpsError::Validation(m)) if m.contains("at most")
    ));
}

/// 付記 A1: 修正 task の親が、修正（と結び）の作成より前に昇格した release に入っていても発火しない。
/// 修正を作った後に親か修正 task を含む release が昇格したときに初めて 1 度だけ reopen する。
#[test]
fn unblocks_release_promoted_before_the_link_does_not_fire() {
    let store = SqliteStore::open_in_memory().expect("store");
    let tmp = tempfile::tempdir().expect("tempdir");
    let waiting = sample_task("waits for the fix", Status::Failed);
    store.insert(&waiting).expect("insert waiting");
    let parent = sample_task("long-running parent", Status::Running);
    store.insert(&parent).expect("insert parent");
    // 親は修正の子を作る前に配送・昇格済み（結びの作成時刻より前）。
    write_release(
        tmp.path(),
        "000000000001",
        "2026-10-25T12:00:00Z",
        &[parent.id],
    );
    let mut fix = sample_task("fix child", Status::Done);
    fix.parent_id = Some(parent.id);
    create_fix(&store, &fix, &[waiting.id]);

    let now = OffsetDateTime::now_utc();
    let fired = sweep(&store, &read_promoted_releases(tmp.path()), now).expect("sweep");
    assert!(fired.is_empty(), "{fired:?}");
    assert_eq!(
        store.get(waiting.id).unwrap().unwrap().status,
        Status::Failed
    );
    assert_eq!(store.unblock_pending().expect("pending").len(), 1);

    // 修正を作った後の親の再配送で、修正を含む release が昇格したら発火する。
    write_release(
        tmp.path(),
        "000000000002",
        "2026-10-26T12:00:00Z",
        &[parent.id],
    );
    let fired = sweep(&store, &read_promoted_releases(tmp.path()), now).expect("sweep");
    assert_eq!(fired.len(), 1, "{fired:?}");
    assert_eq!(fired[0].release, "000000000002");
    assert_eq!(fired[0].action, UnblockAction::Reopened);
    assert_eq!(reopen_count(&store, waiting.id), 1);
}
