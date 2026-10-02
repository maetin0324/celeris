use super::*;
use task_core::{
    ArtifactRef, Budget, Check, Criterion, SqliteStore, Task, TaskId, TaskKind, Tier, WorkerHint,
    WorkspaceSpec,
};

fn sample_task(status: Status) -> Task {
    let now = OffsetDateTime::now_utc();
    Task {
        expected_write_paths: None,
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "t".into(),
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
            adapter: None,
        },
        workspace: WorkspaceSpec::local("/tmp/ws"),
        repos: Vec::new(),
        budget: Budget {
            max_turns: 1,
            max_wall_secs: 1,
            max_retries: 2,
        },
        attempts: 1,
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

#[test]
fn rereview_reuses_done_output_without_starting_an_implementation() {
    let store = SqliteStore::open_in_memory().unwrap();
    let mut task = sample_task(Status::Done);
    task.acceptance[0].check = Check::Reviewer;
    store.insert(&task).unwrap();
    let result = rereview(&store, task.id, Some(Status::Done)).unwrap();
    assert_eq!(result.to, Status::Reviewing);
    assert_eq!(store.get(task.id).unwrap().unwrap().attempts, task.attempts);
    assert!(rereview(&store, task.id, Some(Status::Done)).is_err());
    assert!(
        !store
            .events_for(task.id)
            .unwrap()
            .iter()
            .any(|(_, e)| matches!(e, Event::WorkerStarted { .. }))
    );
}

/// ADR-0054 Phase 113 D3/D4(d): `failed` は「直前の実装 run が `done` で、review 判定だけが
/// 不合格だった」（最後の遷移が `review_fail`）ときだけ再レビューできる。それ以外の理由で
/// `failed`（例: 実装 run 自体が requeue 上限を使い切った `WorkerError`）は拒否する。
#[test]
fn rereview_from_failed_requires_the_last_transition_to_be_review_fail() {
    let store = SqliteStore::open_in_memory().unwrap();
    let mut task = sample_task(Status::Running);
    task.acceptance[0].check = Check::Reviewer;
    task.budget.max_retries = 0;
    store.insert(&task).unwrap();
    // 実装 run 自体が失敗（供給側失敗の requeue 上限。`review_fail` ではない）で `failed` になった
    // ケースは対象外。
    store
        .apply_transition(task.id, Trigger::WorkerError { retryable: true }, None)
        .unwrap();
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Failed);
    let err = rereview(&store, task.id, None).unwrap_err();
    assert!(matches!(err, OpsError::Validation(_)), "{err:?}");

    // review 判定の不合格（`review_fail`）で `failed` になったケースは許す。
    let store2 = SqliteStore::open_in_memory().unwrap();
    let mut task2 = sample_task(Status::Reviewing);
    task2.acceptance[0].check = Check::Reviewer;
    task2.budget.max_retries = 0;
    store2.insert(&task2).unwrap();
    store2
        .apply_transition(task2.id, Trigger::ReviewFail, None)
        .unwrap();
    assert_eq!(
        store2.get(task2.id).unwrap().unwrap().status,
        Status::Failed
    );
    let result = rereview(&store2, task2.id, None).unwrap();
    assert_eq!(result.to, Status::Reviewing);
}

/// `running` のタスクは**必ずリースを持つ**（`acquire_lease` がそう作る）。割り込みの
/// `WorkerFinished` はそのリースの run にだけ付くので、テストでも同じ形にする。
fn store_with(status: Status) -> (SqliteStore, TaskId) {
    let store = SqliteStore::open_in_memory().expect("store");
    let mut task = sample_task(status);
    if status == Status::Running {
        task.lease = Some(task_core::Lease {
            worker_run_id: "run-1".into(),
            expires_at: OffsetDateTime::now_utc() + time::Duration::seconds(60),
        });
    }
    let id = task.id;
    store.insert(&task).expect("insert");
    (store, id)
}

/// ADR-0044 D2 の表を状態ごとに全部確かめる（`running` / `reviewing` は割り込み、`blocked` は回答、
/// `ready` / `draft` は記録だけ、終端は記録だけ + `cancelled` は再開できない）。
#[test]
fn the_human_comment_effect_table_holds_for_every_status() {
    let now = OffsetDateTime::now_utc();
    for (status, expected, expect_status, can_reopen) in [
        (Status::Draft, CommentEffect::Stored, Status::Draft, false),
        (Status::Ready, CommentEffect::Stored, Status::Ready, false),
        (
            Status::Running,
            CommentEffect::Interrupted,
            Status::Ready,
            false,
        ),
        (
            Status::Reviewing,
            CommentEffect::Interrupted,
            Status::Ready,
            false,
        ),
        (
            Status::Blocked,
            CommentEffect::Answered,
            Status::Ready,
            false,
        ),
        (Status::Done, CommentEffect::Terminal, Status::Done, true),
        (
            Status::Failed,
            CommentEffect::Terminal,
            Status::Failed,
            true,
        ),
        (
            Status::Cancelled,
            CommentEffect::Terminal,
            Status::Cancelled,
            false,
        ),
    ] {
        let (store, id) = store_with(status);
        let result = post_human_comment(&store, id, "見てほしい".into(), now)
            .unwrap_or_else(|e| panic!("{status:?}: {e}"));
        assert_eq!(result.effect, expected, "{status:?}");
        assert_eq!(result.can_reopen, can_reopen, "{status:?}");
        let after = store.get(id).expect("get").expect("task");
        assert_eq!(after.status, expect_status, "{status:?}");
        // どの状態でもコメントは残る。
        let comments = store.comments_for(id).expect("comments");
        assert_eq!(comments.len(), 1, "{status:?}");
        assert_eq!(comments[0].author_kind, CommentAuthorKind::Human);

        if expected == CommentEffect::Interrupted {
            // attempts は据え置き、走っている run にだけ
            // `WorkerFinished{outcome:"interrupted: comment"}` が残る（`reviewing` はリースが
            // 無いので足さない。既に終わった run の記録を壊さないため）。
            assert_eq!(after.attempts, 1, "割り込みは試行を消費しない");
            let events = store.events_for(id).expect("events");
            let finished = events.iter().any(|(_, e)| {
                matches!(e, Event::WorkerFinished { outcome, .. } if outcome == INTERRUPTED_OUTCOME)
            });
            assert_eq!(
                finished,
                status == Status::Running,
                "{status:?}: WorkerFinished の有無"
            );
            assert!(
                events.iter().any(
                    |(_, e)| matches!(e, Event::Transitioned { reason, .. } if reason == "comment")
                ),
                "{status:?}: Transitioned{{reason:\"comment\"}} が無い"
            );
            // 次の run の前置きの先頭に載る（割り込んだコメントとして引ける）。
            let found = interrupting_comment(&events, &comments).expect("interrupting comment");
            assert_eq!(found.body, "見てほしい");
        }
        if expected == CommentEffect::Answered {
            let events = store.events_for(id).expect("events");
            assert!(events.iter().any(|(_, e)| matches!(
                e,
                Event::Answered { answer, .. } if answer == "見てほしい"
            )));
        }
    }
}

/// ADR-0044 D2: `done` / `failed` は再開できて attempts が 0 に戻る。`cancelled` は 409 相当。
#[test]
fn reopen_resets_attempts_for_done_and_failed_but_refuses_cancelled() {
    for status in [Status::Done, Status::Failed] {
        let (store, id) = store_with(status);
        let result = reopen(&store, id, None).unwrap_or_else(|e| panic!("{status:?}: {e}"));
        assert_eq!(result.from, status);
        assert_eq!(result.to, Status::Ready);
        assert_eq!(result.reason, "reopen");
        let after = store.get(id).expect("get").expect("task");
        assert_eq!(after.attempts, 0);
    }
    let (store, id) = store_with(Status::Cancelled);
    assert!(matches!(
        reopen(&store, id, None),
        Err(OpsError::InvalidState { .. })
    ));
    let (store, id) = store_with(Status::Ready);
    assert!(matches!(
        reopen(&store, id, None),
        Err(OpsError::InvalidState { .. })
    ));
}

/// ワーカーのコメントは記録だけ（状態を変えない）。空の本文は拒否する。
#[test]
fn a_node_comment_only_records_and_blank_bodies_are_rejected() {
    let now = OffsetDateTime::now_utc();
    let (store, id) = store_with(Status::Running);
    let comment = post_node_comment(
        &store,
        id,
        Some("impl".into()),
        Some("run-1".into()),
        "ビルドが通った".into(),
        now,
    )
    .expect("node comment");
    assert_eq!(comment.author.as_deref(), Some("impl"));
    assert_eq!(
        store.get(id).expect("get").expect("task").status,
        Status::Running
    );
    assert!(matches!(
        post_node_comment(&store, id, None, None, "  ".into(), now),
        Err(OpsError::Validation(_))
    ));
    assert!(matches!(
        post_human_comment(&store, id, "".into(), now),
        Err(OpsError::Validation(_))
    ));
}

/// ADR-0044 D2（Phase 53 の監査）: 供給側の requeue（ADR-0010 P-21）とリース切れは
/// 「割り込みを受け取った run が走った」ことにならないので、割り込みは**消えない**。
#[test]
fn a_requeue_or_a_lease_expiry_does_not_consume_the_interruption() {
    let now = OffsetDateTime::now_utc();
    for outcome in ["requeue: adapter: throttled", "lease_expired"] {
        let (store, id) = store_with(Status::Running);
        post_human_comment(&store, id, "止めて".into(), now).expect("comment");
        let comments = store.comments_for(id).expect("comments");
        store
            .apply_transition(id, Trigger::Dispatch, None)
            .expect("dispatch");
        store
            .apply_transition_with_events(
                id,
                Trigger::Requeue,
                vec![Event::WorkerFinished {
                    run_id: "run-2".into(),
                    outcome: outcome.to_string(),
                    usage: None,
                    role: None,
                    metrics: None,
                    end: None,
                }],
            )
            .expect("requeue");
        let events = store.events_for(id).expect("events");
        assert!(
            interrupting_comment(&events, &comments).is_some(),
            "{outcome}: モデルは前置きを見ていないので割り込みは残る"
        );
    }
}

/// 割り込みは**次の run にだけ**載る。dispatch（`Transitioned{to: running}`）では消えず、
/// その run が終わった（`WorkerFinished`）ら消える。
#[test]
fn the_interruption_is_only_carried_into_the_next_run() {
    let now = OffsetDateTime::now_utc();
    let (store, id) = store_with(Status::Running);
    post_human_comment(&store, id, "止めて".into(), now).expect("comment");
    let comments = store.comments_for(id).expect("comments");
    let events = store.events_for(id).expect("events");
    assert!(interrupting_comment(&events, &comments).is_some());

    // 次の run が始まっただけでは消えない（この run が前置きを受け取る）。
    store
        .apply_transition(id, Trigger::Dispatch, None)
        .expect("dispatch");
    let events = store.events_for(id).expect("events");
    assert!(
        interrupting_comment(&events, &comments).is_some(),
        "次の run はまだ受け取る"
    );

    // その run が終わったら消える。
    store
        .apply_transition_with_events(
            id,
            Trigger::WorkerDone,
            vec![Event::WorkerFinished {
                run_id: "run-2".into(),
                outcome: "done: 直した".into(),
                usage: None,
                role: None,
                metrics: None,
                end: None,
            }],
        )
        .expect("worker_done");
    let events = store.events_for(id).expect("events");
    assert!(
        interrupting_comment(&events, &comments).is_none(),
        "消化済み"
    );
}
