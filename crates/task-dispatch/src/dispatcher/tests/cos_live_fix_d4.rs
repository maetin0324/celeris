//! ADR 2026-10-07-cos-live-fixes D4: a CoS run that already recorded its terminal
//! state is never taken over as an orphan, and a run's terminal state is recorded
//! before its handle and in-flight slot are released. Time is the injected test
//! clock; runs are awaited by joining the worker handle, never by sleeping.

use super::*;
use crate::dispatcher::cos_chat::launch::CosChatLaunchConfig;
use task_core::chat::{
    ChatCreateThreadRequest, ChatMessageQuery, ChatPostMessageRequest, ChatRunState, ChatSendMode,
};
use task_worker::fake::FakeAdapter;

/// A worker that streams assistant text before it finishes. A triage run has no
/// output message, so the text append is rejected by the store (live2 D4).
const TEXT_THEN_DONE: &str = "cat >/dev/null; \
     echo '{\"type\":\"progress\",\"msg\":\"triaged\",\"kind\":\"text\",\"detail\":\"triaged\"}'; \
     echo '{\"type\":\"done\",\"summary\":\"triaged\",\"evidence\":[]}'";

struct Fixture {
    dir: crate::test_support::WritableTempDir,
    db_path: PathBuf,
    store: Arc<SqliteStore>,
    d: Dispatcher,
}

fn fixture(script: &str) -> Fixture {
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
            enabled: true,
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
    // A single daemon: no other live instance, so `holders_gone` is true and an
    // ownerless run would be taken over on the very next tick.
    d.set_orphan_takeover(crate::orphan::OrphanTakeover {
        instance_id: "this-daemon".into(),
        freshness: std::time::Duration::from_secs(60),
        pid_alive: Arc::new(|_| true),
    });
    Fixture {
        dir,
        db_path,
        store,
        d,
    }
}

impl Fixture {
    fn tick(&mut self) {
        if let Some(clock) = &self.d.test_now {
            let mut now = clock.lock().expect("clock");
            *now += time::Duration::seconds(1);
        }
        self.d.tick_cos_chat_launch();
    }

    /// Move the clock past every lease and wall-clock deadline of a CoS run.
    fn expire_leases(&mut self) {
        if let Some(clock) = &self.d.test_now {
            let mut now = clock.lock().expect("clock");
            *now += time::Duration::hours(2);
        }
    }

    fn rows(&self, sql: &str) -> Vec<(String, String, Option<String>)> {
        let conn = rusqlite::Connection::open(&self.db_path).expect("db");
        let mut stmt = conn.prepare(sql).expect("prepare");
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("rows")
    }

    /// `(run_id, state, reason)` of every run of a thread, oldest first.
    fn runs(&self, thread: &str) -> Vec<(String, String, Option<String>)> {
        self.rows(&format!(
            "SELECT run_id,state,reason FROM chat_runs WHERE thread_id='{thread}' \
             ORDER BY started_at, run_id"
        ))
    }

    fn recover_messages(&self, thread: &str) -> usize {
        self.store
            .chat_message_list(
                thread,
                &ChatMessageQuery {
                    limit: Some(200),
                    ..Default::default()
                },
            )
            .expect("messages")
            .items
            .iter()
            .filter(|m| {
                m.client_message_id
                    .as_deref()
                    .is_some_and(|k| k.starts_with("cos-recover:"))
            })
            .count()
    }

    fn inbox_thread(&self) -> Option<String> {
        let conn = rusqlite::Connection::open(&self.db_path).expect("db");
        conn.query_row("SELECT id FROM chat_threads WHERE kind='inbox'", [], |r| {
            r.get(0)
        })
        .ok()
    }

    /// A task blocked on a question: one `question` item of the derived inbox.
    fn question(&self, title: &str) {
        let mut task = new_task(self.dir.path(), Check::Human, 0);
        task.title = title.into();
        task.status = Status::Blocked;
        self.store
            .create_task(
                &task,
                vec![Event::QuestionRaised {
                    run_id: "r0".into(),
                    text: format!("{title}?"),
                }],
            )
            .expect("task");
    }

    /// Await the worker of `thread` without removing its handle, so the next
    /// tick still sees the finished handle and releases the slot itself.
    async fn wait_finished(&mut self, thread: &str) {
        let handle = self
            .d
            .cos_chat_launch
            .as_mut()
            .expect("launch")
            .running
            .get_mut(thread)
            .expect("worker launched");
        tokio::time::timeout(std::time::Duration::from_secs(30), handle)
            .await
            .expect("worker deadline")
            .expect("worker did not panic");
    }

    fn slot_held(&self, thread: &str) -> bool {
        let launch = self.d.cos_chat_launch.as_ref().expect("launch");
        launch.running.contains_key(thread) || launch.providers_in_flight.contains_key(thread)
    }

    fn chat_thread(&self, key: &str) -> String {
        self.store
            .chat_thread_create(
                "admin",
                &ChatCreateThreadRequest {
                    title: key.into(),
                    project_id: None,
                    client_thread_id: key.into(),
                },
                OffsetDateTime::now_utc(),
            )
            .expect("create thread")
            .thread
            .id
    }

    fn post(&self, thread: &str, key: &str) {
        self.store
            .chat_message_post(thread, &continuation(key, None), OffsetDateTime::now_utc())
            .expect("post");
    }
}

fn continuation(key: &str, reply_to: Option<&str>) -> ChatPostMessageRequest {
    ChatPostMessageRequest {
        client_message_id: key.into(),
        text: key.into(),
        attachment_ids: vec![],
        reply_to_id: reply_to.map(str::to_string),
        mode: if reply_to.is_some() {
            ChatSendMode::Interrupt
        } else {
            ChatSendMode::Queue
        },
        resume_queue: false,
    }
}

/// live2 D4 regression: the triage run streams text (rejected: no output
/// message) and then finishes. Its terminal state must still be recorded, and
/// the following ticks of a single daemon must not interrupt it or queue a
/// `cos-recover:` continuation run.
#[tokio::test]
async fn cos_live_fix_d4_finished_triage_run_is_not_taken_over() {
    let mut f = fixture(TEXT_THEN_DONE);
    f.tick(); // introduction: cursors start at the head
    f.question("deploy");
    f.tick();
    let inbox = f.inbox_thread().expect("inbox thread");
    assert_eq!(f.runs(&inbox).len(), 1, "one triage run started");
    f.wait_finished(&inbox).await;
    let runs = f.runs(&inbox);
    assert_eq!(
        runs[0].1, "completed",
        "terminal recorded by the run: {runs:?}"
    );
    f.expire_leases();
    for _ in 0..3 {
        f.tick();
    }
    let runs = f.runs(&inbox);
    assert_eq!(runs.len(), 1, "no continuation run: {runs:?}");
    assert_eq!(runs[0].1, "completed");
    assert_ne!(
        runs[0].2.as_deref(),
        Some("orphan takeover; continuing in a new run")
    );
    assert_eq!(f.recover_messages(&inbox), 0);
}

/// A chat run whose terminal state is recorded and whose lease deadline has
/// passed: ticks leave it completed and start nothing.
#[tokio::test]
async fn cos_live_fix_d4_terminal_run_with_expired_lease_is_left_alone() {
    let mut f = fixture(TEXT_THEN_DONE);
    let t = f.chat_thread("d4-terminal");
    f.post(&t, "original");
    let now = f.d.now_utc();
    let run = f
        .store
        .chat_run_claim_next(&t, "done-run", &serde_json::json!({}), now)
        .expect("claim")
        .expect("run");
    f.store
        .chat_run_finish(&run.id, ChatRunState::Completed, Some("ok"), None, now)
        .expect("finished by its owner");
    f.expire_leases();
    for _ in 0..3 {
        f.tick();
    }
    let runs = f.runs(&t);
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(runs[0].1, "completed");
    assert_eq!(f.recover_messages(&t), 0);
    assert!(!f.slot_held(&t));
}

/// The takeover itself re-checks the run in its transaction: a run that became
/// terminal after it was listed is not interrupted and gets no continuation.
#[tokio::test]
async fn cos_live_fix_d4_takeover_of_terminal_run_writes_nothing() {
    let f = fixture(TEXT_THEN_DONE);
    let t = f.chat_thread("d4-race");
    f.post(&t, "original");
    let now = f.d.now_utc();
    let listed = f
        .store
        .chat_run_claim_next(&t, "racing-run", &serde_json::json!({}), now)
        .expect("claim")
        .expect("run");
    assert_eq!(listed.state, ChatRunState::Running);
    // The owner records its terminal state between the list and the takeover.
    f.store
        .chat_run_finish(&listed.id, ChatRunState::Completed, None, None, now)
        .expect("finish");
    let taken = f
        .store
        .chat_run_takeover(
            &listed.id,
            &continuation(
                &format!("cos-recover:{}", listed.id),
                Some(&listed.input_message_id),
            ),
            "orphan takeover; continuing in a new run",
            now,
        )
        .expect("takeover");
    assert!(taken.is_none());
    assert_eq!(
        f.store.chat_run_get(&t, &listed.id).expect("run").state,
        ChatRunState::Completed
    );
    assert_eq!(f.recover_messages(&t), 0);
    let thread = f
        .store
        .chat_thread_get(&t)
        .expect("thread")
        .expect("exists");
    assert_eq!(thread.queued_count, 0);
    assert!(thread.active_run_id.is_none());
}

/// A real orphan (running in the store, no handle in this daemon, no other live
/// instance) is still interrupted and continued in a new run, once.
#[tokio::test]
async fn cos_live_fix_d4_true_orphan_is_still_taken_over_once() {
    let mut f = fixture(TEXT_THEN_DONE);
    let t = f.chat_thread("d4-orphan");
    f.post(&t, "original");
    let now = f.d.now_utc();
    let old = f
        .store
        .chat_run_claim_next(&t, "orphan-run", &serde_json::json!({}), now)
        .expect("claim")
        .expect("run");
    f.tick();
    let old_run = f.store.chat_run_get(&t, &old.id).expect("old");
    assert_eq!(old_run.state, ChatRunState::Interrupted);
    assert_eq!(
        old_run.reason.as_deref(),
        Some("orphan takeover; continuing in a new run")
    );
    assert_eq!(f.recover_messages(&t), 1);
    let runs = f.runs(&t);
    assert_eq!(runs.len(), 2, "one continuation run: {runs:?}");
    f.wait_finished(&t).await;
    for _ in 0..3 {
        f.tick();
    }
    let runs = f.runs(&t);
    assert_eq!(
        runs.len(),
        2,
        "the continuation is not taken over: {runs:?}"
    );
    assert_eq!(runs[1].1, "completed");
    assert_eq!(f.recover_messages(&t), 1);
}

/// Order: when the worker handle is finished the terminal state is already
/// committed, and the in-flight slot (handle, provider slot) is released only
/// by the next tick, after that commit.
#[tokio::test]
async fn cos_live_fix_d4_terminal_is_recorded_before_slot_release() {
    let mut f = fixture(TEXT_THEN_DONE);
    let t = f.chat_thread("d4-order");
    f.post(&t, "hello");
    f.tick();
    assert!(f.slot_held(&t), "run launched and holds its slot");
    f.wait_finished(&t).await;
    // Handle finished, slot not yet released: the terminal is already recorded.
    assert!(f.slot_held(&t));
    let runs = f.runs(&t);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].1, "completed", "{runs:?}");
    f.tick();
    assert!(!f.slot_held(&t), "slot released after the terminal record");
    let runs = f.runs(&t);
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(runs[0].1, "completed");
    assert_eq!(f.recover_messages(&t), 0);
}
