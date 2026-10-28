//! CoS inbox intake (ADR 2026-10-05 D3). Time is the injected test clock; runs
//! are awaited by joining the worker handle, never by sleeping.

use super::*;
use crate::dispatcher::cos_chat::launch::CosChatLaunchConfig;
use task_core::chat::ChatMessageQuery;
use task_worker::fake::FakeAdapter;

const DONE: &str =
    "cat >/dev/null; echo '{\"type\":\"done\",\"summary\":\"done\",\"evidence\":[]}'";

struct Fixture {
    dir: crate::test_support::WritableTempDir,
    db_path: PathBuf,
    store: Arc<SqliteStore>,
    d: Dispatcher,
}

fn fixture(enabled: bool) -> Fixture {
    fixture_script(enabled, DONE)
}

fn fixture_script(enabled: bool, script: &str) -> Fixture {
    let dir = crate::test_support::WritableTempDir::new();
    let db_path = dir.path().join("celeris.db");
    let store = Arc::new(SqliteStore::open(&db_path).expect("store"));
    let adapter: Arc<dyn WorkerAdapter> = Arc::new(FakeAdapter::new(vec![
        "sh".into(),
        "-c".into(),
        script.into(),
    ]));
    let mut d = dispatcher_with_test_clock(store.clone(), adapter, 2, true);
    d.test_now = Some(Arc::new(StdMutex::new(
        OffsetDateTime::from_unix_timestamp(1_790_000_000).expect("clock"),
    )));
    d.config.execution.max_cos_runs = 1;
    d.config.min_free_disk_mb = 0;
    d.config.knowledge.root = dir.path().join("knowledge");
    for name in ["cos-operator", "cos-inbox-triage"] {
        let skill_dir = d.config.knowledge.root.join("skills").join(name);
        std::fs::create_dir_all(&skill_dir).expect("skill dir");
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: test skill\n---\n# {name}\n"),
        )
        .expect("skill");
    }
    d.set_cos_chat_launch(
        store.clone(),
        CosChatLaunchConfig {
            worker_reserve_five_hour: 0.90,
            enabled,
            harness: "fake".into(),
            max_turns: 2,
            max_wall_secs: 30,
            unavailable_reason: None,
            data_dir: dir.path().to_path_buf(),
            db_path: db_path.clone(),
            attachment_limits: Default::default(),
            api_base_url: "http://127.0.0.1:7700/api/v1".into(),
            web_base_url: Some("http://celeris.test".into()),
            triage: Default::default(),
        },
    );
    Fixture {
        dir,
        db_path,
        store,
        d,
    }
}

impl Fixture {
    fn advance(&mut self, secs: i64) {
        if let Some(clock) = &self.d.test_now {
            let mut now = clock.lock().expect("clock");
            *now += time::Duration::seconds(secs);
        }
    }

    fn count(&self, sql: &str) -> i64 {
        let conn = rusqlite::Connection::open(&self.db_path).expect("db");
        conn.query_row(sql, [], |r| r.get(0)).expect("count")
    }

    fn count_text(&self, sql: &str) -> String {
        let conn = rusqlite::Connection::open(&self.db_path).expect("db");
        conn.query_row(sql, [], |r| r.get(0)).expect("text")
    }

    fn items(&self) -> i64 {
        self.count("SELECT COUNT(*) FROM cos_inbox_items")
    }

    fn inbox_runs(&self) -> i64 {
        self.count(
            "SELECT COUNT(*) FROM chat_runs r JOIN chat_threads t ON t.id=r.thread_id \
             WHERE t.kind='general'",
        )
    }

    fn inbox_thread(&self) -> Option<String> {
        let conn = rusqlite::Connection::open(&self.db_path).expect("db");
        conn.query_row(
            "SELECT id FROM chat_threads WHERE kind='general'",
            [],
            |r| r.get(0),
        )
        .ok()
    }

    /// System messages of the inbox thread that start a triage run.
    fn triage_messages(&self) -> Vec<String> {
        let Some(thread) = self.inbox_thread() else {
            return Vec::new();
        };
        self.store
            .chat_message_list(
                &thread,
                &ChatMessageQuery {
                    limit: Some(200),
                    ..Default::default()
                },
            )
            .expect("messages")
            .items
            .into_iter()
            .filter(|m| m.text.contains("item_ids="))
            .map(|m| m.text)
            .collect()
    }

    /// Await the inbox run started by the last tick, if any.
    async fn join_inbox(&mut self) -> bool {
        let Some(thread) = self.inbox_thread() else {
            return false;
        };
        let handle = self
            .d
            .cos_chat_launch
            .as_mut()
            .expect("launch")
            .running
            .remove(&thread);
        match handle {
            Some(handle) => {
                tokio::time::timeout(std::time::Duration::from_secs(30), handle)
                    .await
                    .expect("inbox run deadline")
                    .expect("worker did not panic");
                true
            }
            None => false,
        }
    }

    fn tick(&mut self) {
        self.advance(1);
        self.d.tick_cos_chat_launch();
    }

    /// A task blocked on a question: one `question` item of the derived inbox.
    fn question(&self, title: &str, extra: Vec<Event>) -> TaskId {
        self.question_in(title, None, None, extra)
    }

    /// A blocked question in `project`, optionally under `parent` (ADR 2026-10-22-cos-fixed-threads).
    fn question_in(
        &self,
        title: &str,
        project: Option<task_core::ProjectId>,
        parent: Option<TaskId>,
        extra: Vec<Event>,
    ) -> TaskId {
        let mut task = new_task(self.dir.path(), Check::Human, 0);
        task.title = title.into();
        task.project_id = project;
        task.parent_id = parent;
        task.status = Status::Blocked;
        let mut events = vec![Event::QuestionRaised {
            run_id: "r0".into(),
            text: format!("{title}?"),
        }];
        events.extend(extra);
        self.store.create_task(&task, events).expect("task");
        task.id
    }

    fn notice(&self, key: &str, kind: NoticeKind) -> NoticeId {
        self.store
            .notice_record(&NoticeEvent {
                source_key: key.into(),
                kind,
                group_key: format!("g:{key}"),
                title: key.into(),
                summary: key.into(),
                project_id: None,
                task_id: None,
                target: None,
                links: Vec::new(),
                at: self.d.now_utc(),
            })
            .expect("notice")
            .notice_id()
    }
}

fn cos_operation(op: &str) -> Event {
    cos_operation_in("t-human", op)
}

fn cos_operation_in(thread: &str, op: &str) -> Event {
    Event::CosOperation {
        actor: "cos".into(),
        thread_id: thread.into(),
        run_id: "r-cos".into(),
        operation_id: op.into(),
        reason: "CoS が作った".into(),
        policy_version: "1".into(),
        state: "applied".into(),
        target_kind: "task".into(),
        target_id: "x".into(),
    }
}

#[tokio::test]
async fn cos_chat_triage_ingest_one_wait_makes_one_item_one_message_one_run() {
    let mut f = fixture(true);
    f.tick(); // introduction: empty inbox, cursors start at the head
    assert_eq!(f.items(), 0);
    assert!(f.inbox_thread().is_none());

    f.question("deploy", vec![]);
    f.tick();
    assert_eq!(f.items(), 1);
    let messages = f.triage_messages();
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(f.inbox_runs(), 1);
    assert!(f.join_inbox().await, "the CoS run was started");
    for _ in 0..3 {
        f.tick();
    }
    assert_eq!(f.items(), 1);
    assert_eq!(f.triage_messages().len(), 1);
    assert_eq!(f.inbox_runs(), 1, "no second run for the same wait");
    assert!(!f.join_inbox().await);
}

#[tokio::test]
async fn cos_chat_triage_ingest_redelivery_title_and_read_do_not_duplicate() {
    let mut f = fixture(true);
    f.tick();
    let task = f.question("migrate", vec![]);
    let notice = f.notice("report:1", NoticeKind::Report);
    f.tick();
    assert_eq!(f.items(), 2, "one question and one notice");
    assert_eq!(f.inbox_runs(), 1);
    assert!(f.join_inbox().await);

    // Title edit: a new event on the task, same question revision.
    let mut edited = f.store.get(task).expect("get").expect("task");
    edited.title = "migrate (renamed)".into();
    f.store
        .update_task(
            &edited,
            Event::Edited {
                fields: vec!["title".into()],
                by: "human".into(),
            },
        )
        .expect("edit");
    // Read state of the notice.
    assert!(
        f.store
            .notice_mark_read(notice, f.d.now_utc())
            .expect("read")
    );
    f.tick();
    // Redelivery: rewind the event cursor and request a reconcile pass.
    f.store
        .cos_triage_ingest_batch("events", "0", &[], f.d.now_utc())
        .expect("rewind");
    f.d.request_cos_triage_reconcile();
    f.tick();
    f.tick();
    assert_eq!(f.items(), 2);
    assert_eq!(f.triage_messages().len(), 1);
    assert_eq!(f.inbox_runs(), 1);
    assert!(!f.join_inbox().await);
}

#[tokio::test]
async fn cos_chat_triage_ingest_twenty_one_waits_split_into_twenty_and_one() {
    let mut f = fixture(true);
    f.tick();
    for i in 0..21 {
        f.question(&format!("q{i}"), vec![]);
    }
    f.tick();
    assert_eq!(f.items(), 21);
    let first = f.triage_messages();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].matches("\",\"").count() + 1, 20, "{}", first[0]);
    assert!(f.join_inbox().await);
    f.tick();
    let both = f.triage_messages();
    assert_eq!(both.len(), 2, "{both:?}");
    assert_eq!(both[1].matches("\",\"").count(), 0, "one item: {}", both[1]);
    assert!(f.join_inbox().await);
    f.tick();
    f.tick();
    assert_eq!(f.inbox_runs(), 2);
    assert_eq!(
        f.count("SELECT COUNT(*) FROM cos_inbox_items WHERE state='pending'"),
        0
    );
}

#[tokio::test]
async fn cos_chat_triage_ingest_skips_cos_operation_origin() {
    let mut f = fixture(true);
    f.tick();
    // The wait and the audit event share the operation transaction.
    f.question(
        "by cos",
        vec![cos_operation(&ulid::Ulid::new().to_string())],
    );
    f.notice("secretary:1", NoticeKind::SecretaryReply);
    f.tick();
    f.tick();
    assert_eq!(f.items(), 0);
    assert_eq!(f.inbox_runs(), 0);
    // A genuine new wait in the same tick window is still taken.
    f.question("by human", vec![]);
    f.tick();
    assert_eq!(f.items(), 1);
    assert!(f.join_inbox().await);
}

#[tokio::test]
async fn cos_chat_triage_ingest_introduction_takes_old_waits_once_and_disabled_starts_no_run() {
    let mut f = fixture(false);
    // Waits that existed before introduction (no events after the cursor).
    f.question("old-1", vec![]);
    f.question("old-2", vec![]);
    f.tick();
    assert_eq!(f.items(), 2, "introduction compares the current inbox once");
    assert_eq!(f.inbox_runs(), 0, "cos.enabled=false starts no run");
    // A restart repeats the comparison without new rows.
    f.d.request_cos_triage_reconcile();
    f.tick();
    assert_eq!(f.items(), 2);
    assert_eq!(f.inbox_runs(), 0);
    assert_eq!(
        f.count("SELECT COUNT(*) FROM cos_inbox_items WHERE state='fallback'"),
        2,
        "cos.enabled=false hands the items to the fallback path"
    );
}

#[test]
fn cos_chat_triage_ingest_attributes_only_operation_transactions() {
    use crate::dispatcher::cos_chat::triage::touched_tasks;
    let a = TaskId::new();
    let b = TaskId::new();
    let row = |id: u64, task: TaskId, event: Event| EventRow {
        id,
        task_id: task,
        seq: id,
        ts: String::new(),
        event,
    };
    let progress = || -> Event {
        serde_json::from_value(serde_json::json!({
            "type": "worker_progress", "run_id": "r", "msg": "working"
        }))
        .expect("progress event")
    };
    let rows = vec![
        row(
            1,
            a,
            Event::QuestionRaised {
                run_id: "r".into(),
                text: "?".into(),
            },
        ),
        row(2, a, cos_operation("op-a")),
        row(
            3,
            b,
            Event::QuestionRaised {
                run_id: "r".into(),
                text: "?".into(),
            },
        ),
        row(4, b, progress()),
    ];
    let touched = touched_tasks(&rows);
    assert_eq!(touched.tasks.get(&a), Some(&Some("op-a".into())));
    assert_eq!(touched.tasks.get(&b), Some(&None));
    // Noise alone touches nothing.
    let only_noise = touched_tasks(&[row(5, b, progress())]);
    assert!(only_noise.tasks.is_empty());
}

#[tokio::test]
async fn cos_chat_triage_reconcile_resolves_closed_escalated_unintegrated_with_zero_current() {
    let mut f = fixture(true);
    f.tick();
    // 元の task は統合済みで、人の inbox に unintegrated は 0 件。escalated の行だけが残っている。
    let src = task_core::chat::triage::CosTriageSource {
        source_kind: "unintegrated".into(),
        source_key: "unintegrated:T1".into(),
        source_revision: "1".into(),
        source_event_id: Some(1),
        operation_id: None,
        summary: "unintegrated".into(),
        policy_version: "v1".into(),
        task_id: None,
    };
    f.store
        .cos_triage_reconcile("unintegrated", &[src], f.d.now_utc())
        .expect("seed");
    rusqlite::Connection::open(&f.db_path)
        .expect("db")
        .execute("UPDATE cos_inbox_items SET state='escalated'", [])
        .expect("escalate");
    f.d.request_cos_triage_reconcile();
    f.tick();
    f.tick();
    assert_eq!(
        f.count("SELECT COUNT(*) FROM cos_inbox_items WHERE state='resolved'"),
        1
    );
    assert_eq!(
        f.count("SELECT COUNT(*) FROM cos_inbox_items WHERE state='escalated'"),
        0
    );
}

fn stale_source(kind: &str, key: String) -> task_core::chat::triage::CosTriageSource {
    task_core::chat::triage::CosTriageSource {
        source_kind: kind.into(),
        source_key: key,
        source_revision: "1".into(),
        source_event_id: None,
        operation_id: None,
        summary: "stale".into(),
        policy_version: "v1".into(),
        task_id: None,
    }
}

#[tokio::test]
async fn cos_chat_triage_events_resolve_escalated_only_for_touched_tasks() {
    let mut f = fixture(true);
    f.tick();
    let a = f.question("a", Vec::new());
    let b = f.question("b", Vec::new());
    f.tick();
    f.tick();
    // 起動後に escalated になり、元の待ちは既に閉じた item（a と b の task）。
    for t in [a, b] {
        f.store
            .cos_triage_reconcile(
                "unintegrated",
                &[stale_source(
                    "unintegrated",
                    format!("unintegrated-{t}-repo"),
                )],
                f.d.now_utc(),
            )
            .expect("seed");
    }
    rusqlite::Connection::open(&f.db_path)
        .expect("db")
        .execute("UPDATE cos_inbox_items SET state='escalated'", [])
        .expect("escalate");
    let state = |f: &Fixture, key: String| {
        f.count_text(&format!(
            "SELECT state FROM cos_inbox_items WHERE source_key='{key}'"
        ))
    };
    // a にだけ event が入る。
    f.advance(60);
    f.store
        .append_event(
            a,
            &Event::QuestionRaised {
                run_id: "r1".into(),
                text: "again?".into(),
            },
        )
        .expect("touch a");
    f.tick();
    f.tick();
    assert_eq!(state(&f, format!("unintegrated-{a}-repo")), "resolved");
    // 触れていない task の item は escalated のまま。
    assert_eq!(state(&f, format!("unintegrated-{b}-repo")), "escalated");
    // 触れた task でも、元の待ちが開いたままの item（新 revision）は resolved にならない。
    assert_eq!(
        f.count(&format!(
            "SELECT COUNT(*) FROM cos_inbox_items WHERE source_key='question-{a}' AND state!='resolved'"
        )),
        1
    );
}

#[path = "cos_chat_triage_fallback.rs"]
mod fallback;
#[path = "cos_chat_triage_inbox_thread.rs"]
mod inbox_thread;
#[path = "cos_chat_root_done.rs"]
mod root_done;

/// ADR 2026-10-22-cos-fixed-threads D3: an inbox item of a task in a project (directly or through
/// its root) goes to that project's CoS thread, other items to the general CoS; each thread runs
/// its own triage, and no thread is created per item.
#[tokio::test]
async fn cos_chat_triage_routes_items_to_the_project_or_general_cos_thread() {
    let mut f = fixture(true);
    f.d.config.execution.max_cos_runs = 2;
    f.tick();
    let project_thread = crate::test_support::project_cos_thread(&f.store, "案件A");
    let project: task_core::ProjectId = f
        .count_text("SELECT project_id FROM chat_threads WHERE kind='project'")
        .parse()
        .expect("project id");
    let mut root = new_task(f.dir.path(), Check::Human, 0);
    root.project_id = Some(project);
    root.status = Status::Running;
    f.store.create_task(&root, vec![]).expect("root");
    let direct = f.question_in("deploy", Some(project), None, vec![]);
    let child = f.question_in("child", None, Some(root.id), vec![]);
    let loose = f.question("loose", vec![]);
    f.tick();
    let thread_of = |task: TaskId| -> String {
        f.count_text(&format!(
            "SELECT thread_id FROM cos_inbox_items WHERE source_key LIKE '%{task}%'"
        ))
    };
    assert_eq!(thread_of(direct), project_thread);
    assert_eq!(thread_of(child), project_thread);
    let general = f.inbox_thread().expect("general thread");
    assert_eq!(thread_of(loose), general);
    assert_eq!(
        f.count("SELECT COUNT(*) FROM chat_threads"),
        2,
        "one project thread and the general thread"
    );
    // Each thread got its own triage run over its own items.
    for (thread, items) in [(&project_thread, 2), (&general, 1)] {
        assert_eq!(
            f.count(&format!(
                "SELECT COUNT(*) FROM chat_runs WHERE thread_id='{thread}'"
            )),
            1,
            "{thread}"
        );
        assert_eq!(
            f.count(&format!(
                "SELECT COUNT(*) FROM cos_inbox_items WHERE thread_id='{thread}' AND run_id IN \
                 (SELECT run_id FROM chat_runs WHERE thread_id='{thread}')"
            )),
            items,
            "{thread}"
        );
    }
    let listed = f
        .store
        .chat_thread_get(&project_thread)
        .expect("get")
        .expect("thread");
    assert!(listed.attention_count >= 2, "{listed:?}");
    // A second wait in the same project reuses the thread.
    f.question_in("again", Some(project), None, vec![]);
    f.tick();
    assert_eq!(f.count("SELECT COUNT(*) FROM chat_threads"), 2);
    let handles: Vec<_> =
        f.d.cos_chat_launch
            .as_mut()
            .expect("launch")
            .running
            .drain()
            .map(|(_, h)| h)
            .collect();
    for handle in handles {
        handle.await.expect("worker did not panic");
    }
}

/// The `cos_inbox_items.id` of the item whose source is the question of `task`.
fn question_item(f: &Fixture, task: TaskId) -> String {
    f.count_text(&format!(
        "SELECT id FROM cos_inbox_items WHERE source_key='question-{task}'"
    ))
}

/// ADR 2026-10-27-relayed-instructions 付記 A2: a relay posted while the triage run runs reaches
/// that run's escalated items only after the run (R2): the relay's own queued run goes first,
/// then one re-judging run gets the covered item with the relay id (R1), never the item outside
/// the relay's tasks, and the same relay does not hand the item over twice.
#[tokio::test]
async fn cos_chat_triage_relay_after_escalation_rejudges_covered_items_after_the_running_run() {
    let mut f = fixture(true);
    f.tick();
    let covered = f.question("node-hours", vec![]);
    let other = f.question("unrelated", vec![]);
    f.tick();
    assert_eq!(f.inbox_runs(), 1, "the triage run is running");
    let thread = f.inbox_thread().expect("thread");
    let (covered_item, other_item) = (question_item(&f, covered), question_item(&f, other));
    for item in [&covered_item, &other_item] {
        assert!(
            f.store
                .cos_triage_resolve(item, "escalated", None, Some("人に回す"), f.d.now_utc())
                .expect("escalate")
        );
    }
    // The operator relays the person's instruction for `covered` while the run still runs.
    let said_at =
        f.d.now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .expect("rfc3339");
    let relay = f
        .store
        .chat_relay_post(
            &thread,
            &task_core::chat::ChatRelayRequest {
                client_message_id: "relay-1".into(),
                text: "node-hours は 40 まで使ってよい".into(),
                said_at,
                task_ids: vec![covered.to_string()],
                expires_at: None,
            },
            "operator:claude-code",
            f.d.now_utc(),
        )
        .expect("relay")
        .response
        .message
        .id;
    f.tick();
    assert_eq!(
        f.inbox_runs(),
        1,
        "R2: nothing starts while the triage run runs"
    );
    assert_eq!(f.triage_messages().len(), 1);

    assert!(f.join_inbox().await);
    f.tick();
    assert_eq!(f.inbox_runs(), 2, "the relay's own queued run goes first");
    assert_eq!(f.triage_messages().len(), 1, "no re-judging run beside it");
    assert!(f.join_inbox().await);

    f.tick();
    assert_eq!(f.inbox_runs(), 3);
    let messages = f.triage_messages();
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert_eq!(
        messages[1],
        format!(
            "CoS 受信箱の一次対応: item_ids=[\"{covered_item}\"]\n取り次ぎによる再判断: \
             [{{\"item_id\":\"{covered_item}\",\"source_key\":\"question-{covered}\",\
             \"relay_message_ids\":[\"{relay}\"]}}]"
        )
    );
    assert_eq!(
        f.count_text(&format!(
            "SELECT state FROM cos_inbox_items WHERE id='{other_item}'"
        )),
        "escalated",
        "outside the relay's task_ids"
    );
    assert!(f.join_inbox().await);

    // The re-judging run escalates it again: the same relay never hands it over twice.
    assert!(
        f.store
            .cos_triage_resolve(
                &covered_item,
                "escalated",
                None,
                Some("範囲外"),
                f.d.now_utc()
            )
            .expect("escalate")
    );
    for _ in 0..3 {
        f.tick();
        assert!(!f.join_inbox().await);
    }
    assert_eq!(f.inbox_runs(), 3);
    assert_eq!(f.triage_messages().len(), 2);
}
