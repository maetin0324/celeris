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
    let mut d = dispatcher_with_adapter_id(store.clone(), adapter, 2, true, "fake");
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
            fallbacks: Vec::new(),
            worker_reserve_five_hour: 0.90,
            enabled,
            harness: "fake".into(),
            llm_source: Some("test".into()),
            provider: Some("p1".into()),
            account_id: None,
            model: None,
            tier: Tier::Frontier,
            max_turns: 2,
            max_wall_secs: 30,
            unavailable_reason: None,
            data_dir: dir.path().to_path_buf(),
            db_path: db_path.clone(),
            attachment_limits: Default::default(),
            api_base_url: "http://127.0.0.1:7700/api/v1".into(),
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

    fn items(&self) -> i64 {
        self.count("SELECT COUNT(*) FROM cos_inbox_items")
    }

    fn inbox_runs(&self) -> i64 {
        self.count(
            "SELECT COUNT(*) FROM chat_runs r JOIN chat_threads t ON t.id=r.thread_id \
             WHERE t.kind='inbox'",
        )
    }

    fn inbox_thread(&self) -> Option<String> {
        let conn = rusqlite::Connection::open(&self.db_path).expect("db");
        conn.query_row("SELECT id FROM chat_threads WHERE kind='inbox'", [], |r| {
            r.get(0)
        })
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
        let mut task = new_task(self.dir.path(), Check::Human, 0);
        task.title = title.into();
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
async fn cos_chat_triage_ingest_places_reference_card_in_origin_thread() {
    let mut f = fixture(true);
    f.tick();
    let origin = f
        .store
        .chat_thread_create(
            "admin",
            &task_core::chat::ChatCreateThreadRequest {
                title: "依頼".into(),
                project_id: None,
                client_thread_id: "origin".into(),
            },
            f.d.now_utc(),
        )
        .expect("thread")
        .thread
        .id;
    // CoS created the task from the human thread (operation transaction):
    // that wait is CoS's own output and is not taken.
    let task = f.question(
        "from chat",
        vec![cos_operation_in(&origin, &ulid::Ulid::new().to_string())],
    );
    f.tick();
    assert_eq!(f.items(), 0);
    // Later the task asks again from a run: a genuine new revision.
    f.advance(60);
    f.store
        .append_event(
            task,
            &Event::QuestionRaised {
                run_id: "r1".into(),
                text: "which?".into(),
            },
        )
        .expect("ask");
    f.tick();
    f.tick();
    assert_eq!(f.items(), 1);
    let cards: Vec<_> = f
        .store
        .chat_message_list(
            &origin,
            &ChatMessageQuery {
                limit: Some(50),
                ..Default::default()
            },
        )
        .expect("messages")
        .items
        .into_iter()
        .flat_map(|m| m.cards)
        .collect();
    assert_eq!(cards.len(), 1, "{cards:?}");
    assert_eq!(cards[0].kind, task_core::chat::ChatCardKind::Question);
    let inbox = f.inbox_thread().expect("inbox thread");
    assert_eq!(cards[0].href, format!("/?thread={inbox}"));
    assert_eq!(f.inbox_runs(), 1, "triage runs only in the inbox thread");
    assert!(f.join_inbox().await);
}

#[path = "cos_chat_triage_fallback.rs"]
mod fallback;
#[path = "cos_chat_triage_inbox_thread.rs"]
mod inbox_thread;
