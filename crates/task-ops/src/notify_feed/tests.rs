use super::*;
use task_core::{
    Budget, NoticeQuery, SqliteStore, Task, TaskId, TaskKind, Tier, WorkerHint, WorkspaceSpec,
};

fn task(status: Status) -> Task {
    let now = OffsetDateTime::now_utc();
    Task {
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: vec![],
        repos: vec![],
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "仕事".into(),
        objective: "確認".into(),
        acceptance: vec![],
        inputs: vec![],
        depends_on: vec![],
        status,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::Local {
            path: "workspace".into(),
            mode: None,
        },
        budget: Budget {
            max_turns: 10,
            max_wall_secs: 600,
            max_retries: 2,
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
        labels: vec![],
        category: Default::default(),
    }
}
fn notices(store: &SqliteStore) -> Vec<task_core::Notice> {
    store.notice_list(&NoticeQuery::default()).unwrap().items
}

#[test]
fn notify_feed_same_event_twice_syncs_once_and_bundles_roots() {
    let store = SqliteStore::open_in_memory().unwrap();
    let first = task(Status::Done);
    store.insert(&first).unwrap();
    store
        .append_event(
            first.id,
            &Event::Transitioned {
                from: Status::Running,
                to: Status::Done,
                reason: "done".into(),
            },
        )
        .unwrap();
    let now = OffsetDateTime::now_utc() + time::Duration::seconds(1);
    assert_eq!(sync_notifications(&store, now).unwrap(), 1);
    assert_eq!(sync_notifications(&store, now).unwrap(), 0);
    assert_eq!(notices(&store).len(), 1);
    assert_eq!(notices(&store)[0].count, 1);
    let second = task(Status::Done);
    store.insert(&second).unwrap();
    store
        .append_event(
            second.id,
            &Event::Transitioned {
                from: Status::Running,
                to: Status::Done,
                reason: "done".into(),
            },
        )
        .unwrap();
    assert_eq!(sync_notifications(&store, now).unwrap(), 1);
    assert_eq!(notices(&store)[0].count, 2);
}

#[test]
fn notify_feed_inbox_events_and_child_completion_do_not_notify() {
    let store = SqliteStore::open_in_memory().unwrap();
    let parent = task(Status::Running);
    store.insert(&parent).unwrap();
    let mut child = task(Status::Done);
    child.parent_id = Some(parent.id);
    store.insert(&child).unwrap();
    for (id, event) in [
        (parent.id, Event::ApprovalRequested),
        (
            parent.id,
            Event::QuestionRaised {
                run_id: "r".into(),
                text: "判断は？".into(),
            },
        ),
        (
            parent.id,
            Event::Transitioned {
                from: Status::Running,
                to: Status::Failed,
                reason: "failed".into(),
            },
        ),
        (
            child.id,
            Event::Transitioned {
                from: Status::Running,
                to: Status::Done,
                reason: "done".into(),
            },
        ),
    ] {
        store.append_event(id, &event).unwrap();
    }
    assert_eq!(
        sync_notifications(
            &store,
            OffsetDateTime::now_utc() + time::Duration::seconds(1)
        )
        .unwrap(),
        0
    );
    assert!(notices(&store).is_empty());
}

#[test]
fn notify_feed_reports_route_only_level_zero_progress_result_bad_news() {
    use task_core::{OrgKind, OrgNode, Report, ReportId, ReportKind};
    let store = SqliteStore::open_in_memory().unwrap();
    let now = OffsetDateTime::now_utc();
    store
        .org_upsert(&OrgNode {
            profile: Default::default(),
            id: "secretary".into(),
            parent_id: None,
            name: "秘書".into(),
            kind: OrgKind::Secretary,
            genre: None,
            brief: String::new(),
            position: 0,
            created_at: now,
            updated_at: now,
        })
        .unwrap();
    for (kind, level) in [
        (ReportKind::Progress, 0),
        (ReportKind::Result, 0),
        (ReportKind::BadNews, 0),
        (ReportKind::Question, 0),
        (ReportKind::Proposal, 0),
        (ReportKind::BadNews, 1),
    ] {
        store
            .report_append(&Report {
                id: ReportId::new(),
                project_id: None,
                node_id: "secretary".into(),
                task_id: None,
                kind,
                level,
                headline: format!("{kind:?}"),
                body: String::new(),
                sources: vec![],
                read_at: None,
                created_at: now,
            })
            .unwrap();
    }
    assert_eq!(sync_notifications(&store, now).unwrap(), 3);
    assert_eq!(sync_notifications(&store, now).unwrap(), 0);
    let kinds: std::collections::HashSet<_> = notices(&store).iter().map(|n| n.kind).collect();
    assert!(kinds.contains(&NoticeKind::Report));
    assert!(kinds.contains(&NoticeKind::BadNews));
    assert_eq!(notices(&store).iter().map(|n| n.count).sum::<u32>(), 3);
}

#[test]
fn notify_feed_secretary_reply_only_for_node_messages() {
    use task_core::{Message, MessageId, MessageRole};
    let store = SqliteStore::open_in_memory().unwrap();
    let now = OffsetDateTime::now_utc();
    for role in [MessageRole::User, MessageRole::Node] {
        store
            .message_append(&Message {
                id: MessageId::new(),
                node_id: "secretary".into(),
                project_id: None,
                role,
                text: "返事".into(),
                run_id: None,
                task_id: None,
                metadata: None,
                created_at: now,
            })
            .unwrap();
    }
    assert_eq!(sync_notifications(&store, now).unwrap(), 1);
    assert_eq!(notices(&store)[0].kind, NoticeKind::SecretaryReply);
}

#[test]
fn notify_feed_observed_release_cron_recovery_and_warning_are_idempotent() {
    let store = SqliteStore::open_in_memory().unwrap();
    let now = OffsetDateTime::now_utc();
    let sources = [
        FeedObservation::Release {
            id: "abc123".into(),
            summary: "昇格しました".into(),
            at: now,
        },
        FeedObservation::CronRun {
            run_id: "run-1".into(),
            job_id: "job-1".into(),
            summary: "日次整理完了".into(),
            at: now,
        },
        FeedObservation::AutoRecovered {
            task_id: "child-1".into(),
            root_id: "root-1".into(),
            summary: "自動で解決".into(),
            at: now,
        },
        FeedObservation::RequeueLimitNear {
            task_id: "task-1".into(),
            summary: "再試行の上限が近い".into(),
            at: now,
        },
    ];
    assert_eq!(record_observations(&store, now, &sources).unwrap(), 4);
    assert_eq!(record_observations(&store, now, &sources).unwrap(), 0);
    let found: std::collections::HashSet<_> = notices(&store).iter().map(|n| n.kind).collect();
    for kind in [
        NoticeKind::Release,
        NoticeKind::CronRun,
        NoticeKind::AutoRecovered,
        NoticeKind::RequeueLimitNear,
    ] {
        assert!(found.contains(&kind), "missing {kind:?}");
    }
    assert_eq!(notices(&store).len(), 4);
}

#[test]
fn notify_feed_delivery_ready_and_release_creation() {
    use task_core::{Delivery, DeliveryState, DeliveryStore, ProjectId, RepoId};
    let store = SqliteStore::open_in_memory().unwrap();
    let task = task(Status::Done);
    store.insert(&task).unwrap();
    let delivery = Delivery {
        task_id: task.id,
        project_id: ProjectId::new(),
        repo_id: RepoId::new(),
        repo: "repo".into(),
        branch: "branch".into(),
        base: "base".into(),
        head: "head".into(),
        target_sha: None,
        reviewed_sha: None,
        merge_candidate_sha: None,
        default_branch: "main".into(),
        department: "engineering".into(),
        review_run: "review".into(),
        worker_run: "worker".into(),
        criterion_idx: 0,
        decision: Some(true),
        state: DeliveryState::Ready,
        detail: String::new(),
        release: Some("abc123".into()),
        prepare_pid: None,
        notification: None,
        pushed_at: None,
        push_error: None,
    };
    assert!(store.delivery_save(None, &delivery).unwrap());
    let now = OffsetDateTime::now_utc() + time::Duration::seconds(1);
    assert_eq!(sync_notifications(&store, now).unwrap(), 2);
    assert_eq!(sync_notifications(&store, now).unwrap(), 0);
    let found: std::collections::HashSet<_> = notices(&store).iter().map(|n| n.kind).collect();
    assert!(found.contains(&NoticeKind::Delivery));
    assert!(found.contains(&NoticeKind::Release));
}

/// ADR-0133 付記: 同期は前回の位置より後だけを読み、新しい出来事が無ければ書き込み接続を取らない。
/// 時計ではなく読んだ行（`FeedSyncStats`）と接続を取った回数（`lock_counts`）で固定する。
#[test]
fn notify_feed_sync_reads_only_new_sources_and_writes_nothing_when_idle() {
    use task_core::{
        Message, MessageId, MessageRole, OrgKind, OrgNode, Report, ReportId, ReportKind,
    };
    let store = SqliteStore::open_in_memory().unwrap();
    let now = OffsetDateTime::now_utc();
    store
        .org_upsert(&OrgNode {
            profile: Default::default(),
            id: "secretary".into(),
            parent_id: None,
            name: "秘書".into(),
            kind: OrgKind::Secretary,
            genre: None,
            brief: String::new(),
            position: 0,
            created_at: now,
            updated_at: now,
        })
        .unwrap();
    let report = |i: i64| Report {
        id: ReportId::new(),
        project_id: None,
        node_id: "secretary".into(),
        task_id: None,
        kind: ReportKind::Progress,
        level: 0,
        headline: format!("報告 {i}"),
        body: String::new(),
        sources: vec![],
        read_at: None,
        created_at: now - time::Duration::seconds(10_000 - i),
    };
    let message = |i: i64| Message {
        id: MessageId::new(),
        node_id: "secretary".into(),
        project_id: None,
        role: MessageRole::Node,
        text: "返事".into(),
        run_id: None,
        task_id: None,
        metadata: None,
        created_at: now - time::Duration::seconds(10_000 - i),
    };
    for i in 0..600 {
        store.report_append(&report(i)).unwrap();
    }
    for i in 0..300 {
        store.message_append(&message(i)).unwrap();
    }
    // 根の完了 100 件（events は 1 件あたり 40 行の履歴 + 完了）。
    for _ in 0..100 {
        let mut history: Vec<Event> = (0..40)
            .map(|_| Event::Transitioned {
                from: Status::Ready,
                to: Status::Ready,
                reason: "history".into(),
            })
            .collect();
        history.push(Event::Transitioned {
            from: Status::Running,
            to: Status::Done,
            reason: "done".into(),
        });
        store.create_task(&task(Status::Done), history).unwrap();
    }

    // events の時刻は書き込んだ時点なので、その後の時刻で同期する。
    let sync_at = OffsetDateTime::now_utc() + time::Duration::seconds(1);
    let mut recorded = 0;
    let mut rounds = 0;
    let idle = loop {
        rounds += 1;
        assert!(rounds < 50, "sync never settled");
        let before = store.lock_counts();
        let stats = sync_notifications_counted(&store, sync_at).unwrap();
        let locks = store.lock_counts().since(before);
        assert!(stats.events_scanned <= EVENT_BUDGET as u64, "{stats:?}");
        assert!(stats.reports_scanned <= SOURCE_PAGE as u64, "{stats:?}");
        assert!(stats.messages_scanned <= SOURCE_PAGE as u64, "{stats:?}");
        // 書き込みは記録と位置の更新をまとめた高々 1 回の transaction だけ。
        assert!(locks.writer <= 1, "{stats:?} {locks:?}");
        assert_eq!(
            locks.writer, stats.write_transactions,
            "{stats:?} {locks:?}"
        );
        recorded += stats.recorded;
        if stats.submitted == 0 && stats.events_scanned == 0 {
            break (stats, locks);
        }
    };
    // 取りこぼし無く全て数えた（報告 600・返事 300・根の完了 100）。
    assert_eq!(recorded, 1000);
    // 新しい出来事が無い同期は書き込み接続を一度も取らず、読むのは位置の行（閉区間の 1 件）だけ。
    let (stats, locks) = idle;
    assert_eq!(locks.writer, 0, "{stats:?} {locks:?}");
    assert!(
        stats.reports_scanned <= 1 && stats.messages_scanned <= 1,
        "{stats:?}"
    );
    assert!(locks.reader <= 10, "{locks:?}");

    // 1 件ずつ足すと、その分だけ読んで記録する。
    store.report_append(&report(9_000)).unwrap();
    store.message_append(&message(9_000)).unwrap();
    let stats = sync_notifications_counted(&store, sync_at).unwrap();
    assert_eq!(stats.submitted, 2, "{stats:?}");
    assert_eq!(stats.write_transactions, 1, "{stats:?}");
    assert_eq!(stats.recorded, 2, "{stats:?}");
    assert!(
        stats.reports_scanned <= 2 && stats.messages_scanned <= 2,
        "{stats:?}"
    );
}
