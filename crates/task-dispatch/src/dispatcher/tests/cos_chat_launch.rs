//! CoS launch is driven by durable queue state; these tests await worker joins, not sleeps.

use super::*;
use crate::dispatcher::cos_chat::launch::CosChatLaunchConfig;
use task_core::chat::attachments::ChatAttachmentStore;
use task_core::chat::{
    ChatCreateThreadRequest, ChatEventData, ChatEventQuery, ChatMessageQuery,
    ChatPostMessageRequest, ChatRunState, ChatSendMode,
};
use task_worker::fake::FakeAdapter;

#[derive(Clone)]
struct NamedFake(Arc<dyn WorkerAdapter>);

#[async_trait]
impl WorkerAdapter for NamedFake {
    fn id(&self) -> &str {
        "claude-code"
    }

    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        self.0.run(req, run_id, limits, sink).await
    }

    fn with_env(&self, extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        Some(Arc::new(Self(self.0.with_env(extra)?)))
    }
}

fn fixture(
    command: Vec<String>,
    max_cos_runs: usize,
) -> (tempfile::TempDir, Arc<SqliteStore>, Dispatcher) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("celeris.db");
    let store = Arc::new(SqliteStore::open(&db_path).expect("store"));
    let adapter: Arc<dyn WorkerAdapter> = Arc::new(FakeAdapter::new(command));
    let mut d = dispatcher_with_adapter_id(store.clone(), adapter, 2, true, "fake");
    d.config.execution.max_cos_runs = max_cos_runs;
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
            db_path,
            attachment_limits: Default::default(),
            api_base_url: "http://127.0.0.1:7700/api/v1".into(),
            triage: Default::default(),
        },
    );
    (dir, store, d)
}

fn thread(store: &SqliteStore, key: &str) -> String {
    store
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

fn post(store: &SqliteStore, thread: &str, key: &str) {
    store
        .chat_message_post(
            thread,
            &ChatPostMessageRequest {
                client_message_id: key.into(),
                text: key.into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
            },
            OffsetDateTime::now_utc(),
        )
        .expect("post");
}

async fn join(d: &mut Dispatcher, thread: &str) {
    let task = d
        .cos_chat_launch
        .as_mut()
        .expect("launch configured")
        .running
        .remove(thread)
        .expect("worker launched");
    task.await.expect("worker did not panic");
}

fn active_run(store: &SqliteStore, thread: &str) -> task_core::chat::ChatRun {
    let id = store
        .chat_thread_get(thread)
        .expect("thread")
        .expect("exists")
        .active_run_id
        .expect("active run");
    store.chat_run_get(thread, &id).expect("run")
}

#[tokio::test]
async fn cos_chat_run_control_stop_kills_group_pauses_queue_and_late_stop_isolated() {
    // The shell reports its PID over a FIFO, then remains blocked in read. The
    // test stops it with SIGSTOP before requesting cancellation, so SIGKILL's
    // process-group fallback is exercised without any sleep-based timing.
    let command = vec!["sh".into(), "-c".into(),
        "cat >/dev/null; printf '%s' \"$CELERIS_COS_RUN_CREDENTIAL\" > .token; printf '%s' \"$$\" > ready.fifo; read x < release.fifo; echo '{\"type\":\"done\",\"summary\":\"done\",\"evidence\":[]}'".into()];
    let (dir, store, mut d) = fixture(command, 1);
    d.test_now = Some(Arc::new(StdMutex::new(OffsetDateTime::now_utc())));
    d.config.kill_grace = std::time::Duration::from_millis(20);
    let t = thread(&store, "control-stop");
    post(&store, &t, "first");
    post(&store, &t, "second");
    let workspace = dir.path().join("cos/threads").join(&t).join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    for name in ["ready.fifo", "release.fifo"] {
        assert!(
            std::process::Command::new("mkfifo")
                .arg(workspace.join(name))
                .status()
                .expect("mkfifo")
                .success()
        );
    }
    let ready = tokio::task::spawn_blocking({
        let path = workspace.join("ready.fifo");
        move || std::fs::read_to_string(path).expect("ready PID")
    });
    d.tick_cos_chat_launch();
    let first = active_run(&store, &t);
    let pid = tokio::time::timeout(std::time::Duration::from_secs(10), ready)
        .await
        .expect("ready deadline")
        .expect("ready task");
    assert!(
        std::process::Command::new("kill")
            .args(["-STOP", pid.trim()])
            .status()
            .expect("SIGSTOP")
            .success()
    );
    let stopped = store
        .chat_run_stop(&t, &first.id, d.now_utc())
        .expect("stop");
    assert!(stopped.accepted && stopped.response.queue_paused);
    d.tick_cos_chat_launch();
    tokio::time::timeout(std::time::Duration::from_secs(10), join(&mut d, &t))
        .await
        .expect("stopped worker deadline");
    assert_eq!(
        store.chat_run_get(&t, &first.id).expect("first").state,
        ChatRunState::Stopped
    );
    assert!(
        store
            .chat_thread_get(&t)
            .expect("thread")
            .expect("exists")
            .active_run_id
            .is_none(),
        "the per-thread live-run slot is released"
    );
    let bearer = std::fs::read_to_string(workspace.join(".token")).expect("credential");
    let secret = bearer
        .strip_prefix("celeris-cos-run.")
        .expect("bearer prefix");
    assert!(matches!(
        store.cos_run_credential_verify(secret, d.now_utc()),
        Err(task_core::chat::CosRunCredentialError::Revoked)
    ));
    d.tick_cos_chat_launch();
    assert!(
        store
            .chat_thread_get(&t)
            .expect("thread")
            .expect("exists")
            .queue_paused
    );
    assert_eq!(runs(&store, &t).len(), 1, "queued input stays paused");
    let revision = store
        .chat_thread_get(&t)
        .expect("thread")
        .expect("exists")
        .revision;
    store
        .chat_thread_resume_queue(&t, revision, d.now_utc())
        .expect("resume");
    // A new run is claimable; a stop addressed to the old run cannot pause it.
    d.tick_cos_chat_launch();
    let second = active_run(&store, &t);
    assert_ne!(first.id, second.id);
    assert!(
        !store
            .chat_run_stop(&t, &first.id, d.now_utc())
            .expect("late stop")
            .accepted
    );
    assert_eq!(
        store.chat_run_get(&t, &second.id).expect("second").state,
        ChatRunState::Running
    );
    assert!(
        !store
            .chat_thread_get(&t)
            .expect("thread")
            .expect("exists")
            .queue_paused
    );
    // Release the second shell through its FIFO (no timed poll).
    let release = tokio::task::spawn_blocking({
        let path = workspace.join("release.fifo");
        move || std::fs::write(path, b"go\n").expect("release")
    });
    let ready_second = tokio::task::spawn_blocking({
        let path = workspace.join("ready.fifo");
        move || std::fs::read_to_string(path).expect("second ready")
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), ready_second)
        .await
        .expect("second ready deadline")
        .expect("second ready task");
    tokio::time::timeout(std::time::Duration::from_secs(10), release)
        .await
        .expect("release deadline")
        .expect("release task");
    tokio::time::timeout(std::time::Duration::from_secs(10), join(&mut d, &t))
        .await
        .expect("second deadline");
}

#[tokio::test]
async fn cos_chat_run_control_interrupt_precedes_queue_without_changing_pause() {
    let (_dir, store, mut d) = fixture(FakeAdapter::default_command(), 1);
    d.test_now = Some(Arc::new(StdMutex::new(OffsetDateTime::now_utc())));
    let t = thread(&store, "control-interrupt");
    post(&store, &t, "first");
    post(&store, &t, "queued");
    let old = store
        .chat_run_claim_next(&t, "old-interrupt", &serde_json::json!({}), d.now_utc())
        .expect("claim")
        .expect("run");
    store
        .chat_message_post(
            &t,
            &ChatPostMessageRequest {
                client_message_id: "urgent".into(),
                text: "urgent".into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: ChatSendMode::Interrupt,
                resume_queue: false,
            },
            d.now_utc(),
        )
        .expect("interrupt");
    assert_eq!(
        store.chat_run_get(&t, &old.id).expect("old").state,
        ChatRunState::Stopping
    );
    d.set_orphan_takeover(crate::orphan::OrphanTakeover {
        instance_id: "this-daemon".into(),
        freshness: std::time::Duration::from_secs(60),
        pid_alive: Arc::new(|_| true),
    });
    d.tick_cos_chat_launch();
    assert_eq!(
        store.chat_run_get(&t, &old.id).expect("old").state,
        ChatRunState::Interrupted
    );
    let urgent = active_run(&store, &t);
    let input = store
        .chat_message_list(
            &t,
            &ChatMessageQuery {
                limit: Some(100),
                ..Default::default()
            },
        )
        .expect("messages")
        .items
        .into_iter()
        .find(|m| m.id == urgent.input_message_id)
        .expect("urgent input");
    assert_eq!(input.text, "urgent");
    join(&mut d, &t).await;
    d.tick_cos_chat_launch();
    let queued = active_run(&store, &t);
    let input = store
        .chat_message_list(
            &t,
            &ChatMessageQuery {
                limit: Some(100),
                ..Default::default()
            },
        )
        .expect("messages")
        .items
        .into_iter()
        .find(|m| m.id == queued.input_message_id)
        .expect("queued input");
    assert_eq!(input.text, "queued");
    join(&mut d, &t).await;
    assert!(
        !store
            .chat_thread_get(&t)
            .expect("thread")
            .expect("exists")
            .queue_paused
    );
}

#[tokio::test]
async fn cos_chat_run_control_live_interrupt_waits_for_old_worker_exit() {
    let command = vec![
        "sh".into(),
        "-c".into(),
        "cat >/dev/null; printf ready > ready.fifo; read x < release.fifo; echo '{\"type\":\"done\",\"summary\":\"done\",\"evidence\":[]}'".into(),
    ];
    let (dir, store, mut d) = fixture(command, 1);
    d.test_now = Some(Arc::new(StdMutex::new(OffsetDateTime::now_utc())));
    d.config.kill_grace = std::time::Duration::from_millis(20);
    let t = thread(&store, "control-live-interrupt");
    post(&store, &t, "first");
    post(&store, &t, "queued");
    let workspace = dir.path().join("cos/threads").join(&t).join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    for name in ["ready.fifo", "release.fifo"] {
        assert!(
            std::process::Command::new("mkfifo")
                .arg(workspace.join(name))
                .status()
                .expect("mkfifo")
                .success()
        );
    }
    let ready = tokio::task::spawn_blocking({
        let path = workspace.join("ready.fifo");
        move || std::fs::read_to_string(path).expect("ready")
    });
    d.tick_cos_chat_launch();
    let old = active_run(&store, &t);
    tokio::time::timeout(std::time::Duration::from_secs(10), ready)
        .await
        .expect("ready deadline")
        .expect("ready task");
    store
        .chat_message_post(
            &t,
            &ChatPostMessageRequest {
                client_message_id: "urgent-live".into(),
                text: "urgent-live".into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: ChatSendMode::Interrupt,
                resume_queue: false,
            },
            d.now_utc(),
        )
        .expect("interrupt");
    d.tick_cos_chat_launch();
    assert_eq!(
        runs(&store, &t).len(),
        1,
        "new run must await the old worker"
    );
    tokio::time::timeout(std::time::Duration::from_secs(10), join(&mut d, &t))
        .await
        .expect("old worker deadline");
    assert_eq!(
        store.chat_run_get(&t, &old.id).expect("old").state,
        ChatRunState::Interrupted
    );
    d.adapters
        .insert("p1".into(), Arc::new(FakeAdapter::default()));
    d.tick_cos_chat_launch();
    let urgent = active_run(&store, &t);
    let messages = store
        .chat_message_list(
            &t,
            &ChatMessageQuery {
                limit: Some(100),
                ..Default::default()
            },
        )
        .expect("messages")
        .items;
    assert_eq!(
        messages
            .iter()
            .find(|m| m.id == urgent.input_message_id)
            .expect("urgent input")
            .text,
        "urgent-live"
    );
    join(&mut d, &t).await;
    d.tick_cos_chat_launch();
    let queued = active_run(&store, &t);
    let messages = store
        .chat_message_list(
            &t,
            &ChatMessageQuery {
                limit: Some(100),
                ..Default::default()
            },
        )
        .expect("messages")
        .items;
    assert_eq!(
        messages
            .iter()
            .find(|m| m.id == queued.input_message_id)
            .expect("queued input")
            .text,
        "queued"
    );
    join(&mut d, &t).await;
}

#[tokio::test]
async fn cos_chat_run_control_restart_waits_for_orphan_then_continues_input() {
    let (dir, store, mut d) = fixture(FakeAdapter::default_command(), 1);
    d.test_now = Some(Arc::new(StdMutex::new(OffsetDateTime::now_utc())));
    let t = thread(&store, "control-orphan");
    post(&store, &t, "original");
    let old = store
        .chat_run_claim_next(&t, "orphan-run", &serde_json::json!({}), d.now_utc())
        .expect("claim")
        .expect("run");
    let conn = rusqlite::Connection::open(dir.path().join("celeris.db")).expect("db");
    conn.execute("INSERT INTO cos_operations(id,thread_id,run_id,idempotency_key,request_hash,\
        target_kind,target_id,action,payload_json,reason,policy_version,state,result_json,created_at,updated_at)\
        VALUES('applied-op',?1,?2,'applied-key','hash','task','target','update','{}','why','1','applied','{}',?3,?3)",
        rusqlite::params![t, old.id, d.now_utc().to_string()]).expect("applied operation");
    d.tick_cos_chat_launch();
    assert_eq!(
        store.chat_run_get(&t, &old.id).expect("old").state,
        ChatRunState::Running
    );
    assert!(
        d.cos_chat_launch
            .as_ref()
            .expect("launch")
            .running
            .is_empty()
    );
    d.set_orphan_takeover(crate::orphan::OrphanTakeover {
        instance_id: "this-daemon".into(),
        freshness: std::time::Duration::from_secs(60),
        pid_alive: Arc::new(|_| true),
    });
    let other = crate::dispatcher::tests::orphan_takeover::instance(
        "other-daemon",
        InstanceRole::Active,
        12345,
    );
    store.instance_register(&other).expect("other daemon");
    d.tick_cos_chat_launch();
    assert_eq!(
        store.chat_run_get(&t, &old.id).expect("old").state,
        ChatRunState::Running,
        "a live former owner prevents takeover"
    );
    store
        .instance_delete(&other.instance_id)
        .expect("owner disappeared");
    d.tick_cos_chat_launch();
    assert_eq!(
        store.chat_run_get(&t, &old.id).expect("old").state,
        ChatRunState::Interrupted
    );
    let continuation = active_run(&store, &t);
    assert_ne!(continuation.id, old.id);
    let input = store
        .chat_message_list(
            &t,
            &ChatMessageQuery {
                limit: Some(100),
                ..Default::default()
            },
        )
        .expect("messages")
        .items
        .into_iter()
        .find(|m| m.id == continuation.input_message_id)
        .expect("continuation input");
    assert_eq!(input.text, "original");
    let messages = store
        .chat_message_list(
            &t,
            &ChatMessageQuery {
                limit: Some(100),
                ..Default::default()
            },
        )
        .expect("messages")
        .items;
    assert!(
        messages
            .iter()
            .any(|m| m.role == task_core::chat::ChatMessageRole::System
                && m.text.contains("applied-op")
                && m.text.contains("applied-key"))
    );
    join(&mut d, &t).await;
}

#[tokio::test]
async fn cos_chat_run_control_pending_side_effect_waits_for_human() {
    let (dir, store, mut d) = fixture(FakeAdapter::default_command(), 1);
    d.test_now = Some(Arc::new(StdMutex::new(OffsetDateTime::now_utc())));
    let t = thread(&store, "control-pending");
    post(&store, &t, "original");
    let old = store
        .chat_run_claim_next(&t, "pending-run", &serde_json::json!({}), d.now_utc())
        .expect("claim")
        .expect("run");
    store
        .chat_message_post(
            &t,
            &ChatPostMessageRequest {
                client_message_id: "urgent-after-pending".into(),
                text: "urgent".into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: ChatSendMode::Interrupt,
                resume_queue: false,
            },
            d.now_utc(),
        )
        .expect("interrupt while operation pending");
    let conn = rusqlite::Connection::open(dir.path().join("celeris.db")).expect("db");
    conn.execute("INSERT INTO cos_operations(id,thread_id,run_id,idempotency_key,request_hash,\
        target_kind,target_id,action,payload_json,reason,policy_version,state,created_at,updated_at)\
        VALUES('pending-op',?1,?2,'key','hash','task','target','update','{}','why','1','pending',?3,?3)",
        rusqlite::params![t, old.id, d.now_utc().to_string()]).expect("pending operation");
    d.set_orphan_takeover(crate::orphan::OrphanTakeover {
        instance_id: "this-daemon".into(),
        freshness: std::time::Duration::from_secs(60),
        pid_alive: Arc::new(|_| true),
    });
    d.tick_cos_chat_launch();
    assert_eq!(
        store.chat_run_get(&t, &old.id).expect("old").state,
        ChatRunState::Interrupted
    );
    assert!(
        store
            .chat_thread_get(&t)
            .expect("thread")
            .expect("exists")
            .queue_paused
    );
    assert!(
        d.cos_chat_launch
            .as_ref()
            .expect("launch")
            .running
            .is_empty()
    );
    let messages = store
        .chat_message_list(
            &t,
            &ChatMessageQuery {
                limit: Some(100),
                ..Default::default()
            },
        )
        .expect("messages")
        .items;
    assert!(messages.iter().any(|m| {
        m.role == task_core::chat::ChatMessageRole::System
            && m.cards.iter().any(|c| {
                c.kind == task_core::chat::ChatCardKind::Operation
                    && c.operation_id.as_deref() == Some("pending-op")
                    && c.href == "/cos/operations/pending-op"
            })
    }));
    conn.execute(
        "UPDATE cos_operations SET state='applied' WHERE id='pending-op'",
        [],
    )
    .expect("human established outcome");
    d.tick_cos_chat_launch();
    assert!(
        d.cos_chat_launch
            .as_ref()
            .expect("launch")
            .running
            .is_empty(),
        "an explicit queue resume is still required"
    );
    let revision = store
        .chat_thread_get(&t)
        .expect("thread")
        .expect("exists")
        .revision;
    store
        .chat_thread_resume_queue(&t, revision, d.now_utc())
        .expect("resume after review");
    d.tick_cos_chat_launch();
    assert_eq!(active_run(&store, &t).state, ChatRunState::Running);
    join(&mut d, &t).await;
}

fn runs(store: &SqliteStore, thread: &str) -> Vec<task_core::chat::ChatRun> {
    let messages = store
        .chat_message_list(
            thread,
            &ChatMessageQuery {
                limit: Some(200),
                ..Default::default()
            },
        )
        .expect("messages");
    let mut seen = std::collections::HashSet::new();
    messages
        .items
        .into_iter()
        .filter_map(|m| m.run_id)
        .filter(|id| seen.insert(id.clone()))
        .filter_map(|id| store.chat_run_get(thread, &id).ok())
        .collect()
}

#[tokio::test]
async fn cos_chat_run_launch_fake_progress_credential_and_session() {
    let command = vec![
        "sh".into(), "-c".into(),
        "cat >/dev/null; test -n \"$CELERIS_COS_RUN_CREDENTIAL\" || exit 3; \
         printf '%s' \"$CELERIS_COS_RUN_CREDENTIAL\" > .token-for-test; \
         printf '%s\\n' \
         '{\"type\":\"progress\",\"msg\":\"hello\",\"kind\":\"text\",\"detail\":\"hello\"}' \
         '{\"type\":\"progress\",\"msg\":\"tool\",\"kind\":\"tool_use\",\"tool\":\"Read\",\"summary\":\"file\"}' \
         '{\"type\":\"progress\",\"msg\":\"working\",\"kind\":\"status\",\"summary\":\"working\"}' \
         '{\"type\":\"done\",\"summary\":\"done\",\"evidence\":[]}'".into(),
    ];
    let (_dir, store, mut d) = fixture(command, 2);
    let t = thread(&store, "first");
    post(&store, &t, "one");
    d.tick_cos_chat_launch();
    let run = runs(&store, &t).pop().expect("claimed run");
    assert_eq!(
        run.session_mode,
        Some(task_core::chat::ChatSessionMode::New)
    );
    join(&mut d, &t).await;
    let finished = store.chat_run_get(&t, &run.id).expect("finished");
    assert_eq!(finished.state, ChatRunState::Completed);
    let events = store
        .chat_events_page(
            &t,
            &ChatEventQuery {
                after: None,
                run_id: Some(run.id.clone()),
                limit: Some(200),
            },
        )
        .expect("events")
        .items;
    assert!(
        events
            .iter()
            .any(|e| matches!(e.data, ChatEventData::TextDelta(_)))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e.data, ChatEventData::Tool(_)))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e.data, ChatEventData::Status(_)))
    );
    let workspace = _dir.path().join("cos/threads").join(&t).join("workspace");
    let token_path = workspace.join(".token-for-test");
    let token = std::fs::read_to_string(&token_path).expect("credential reached fake harness");
    std::fs::remove_file(token_path).expect("remove test secret");
    let secret = token
        .strip_prefix("celeris-cos-run.")
        .expect("bearer prefix");
    assert_eq!(secret.len(), 64);
    assert!(secret.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert!(
        store
            .cos_run_credential_verify(secret, OffsetDateTime::now_utc())
            .is_err()
    );
    let run_dir = workspace.join("runs").join(&run.id);
    let request = std::fs::read_to_string(run_dir.join("request.json")).expect("request");
    let request_json: serde_json::Value = serde_json::from_str(&request).expect("request json");
    assert_eq!(
        request_json["context"]["skills"].as_array().map(Vec::len),
        Some(2)
    );
    assert_eq!(
        request_json["context"]["cos_chat"]["skills"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    for name in ["request.json", "stdout.jsonl", "stderr.log"] {
        let content = std::fs::read_to_string(run_dir.join(name)).expect("run file exists");
        assert!(!content.contains(&token), "credential leaked into {name}");
    }
    if let Ok(prompt) = std::fs::read_to_string(run_dir.join("prompt.txt")) {
        assert!(!prompt.contains(&token), "credential leaked into prompt");
    }
}

#[tokio::test]
async fn cos_chat_run_launch_sets_api_url_env_and_path() {
    let command = vec![
        "sh".into(),
        "-c".into(),
        "cat >/dev/null; printf '%s' \"$CELERIS_API_URL\" > .api-url-for-test; \
         printf '%s' \"$PATH\" > .path-for-test; \
         printf '%s\\n' '{\"type\":\"done\",\"summary\":\"done\",\"evidence\":[]}'"
            .into(),
    ];
    let (dir, store, mut d) = fixture(command, 2);
    let t = thread(&store, "env");
    post(&store, &t, "one");
    d.tick_cos_chat_launch();
    join(&mut d, &t).await;
    let workspace = dir.path().join("cos/threads").join(&t).join("workspace");
    let api_url = std::fs::read_to_string(workspace.join(".api-url-for-test")).expect("api url");
    assert_eq!(api_url, "http://127.0.0.1:7700/api/v1");
    let path = std::fs::read_to_string(workspace.join(".path-for-test")).expect("path");
    let exe_dir = std::env::current_exe()
        .expect("exe")
        .parent()
        .expect("exe dir")
        .to_path_buf();
    let entries: Vec<_> = std::env::split_paths(&path).collect();
    assert_eq!(entries.first(), Some(&exe_dir), "PATH={path}");
    assert_eq!(entries.iter().filter(|p| **p == exe_dir).count(), 1);
    // The inherited PATH stays behind it, so `sh` and the harness CLIs still resolve.
    assert!(entries.len() > 1, "PATH={path}");

    // No `[api] listen`: no CELERIS_API_URL; the prompt's own explanation applies.
    let env = crate::dispatcher::cos_chat::launch::cos_run_env("", "celeris-cos-run.x");
    assert!(env.iter().all(|(k, _)| k != "CELERIS_API_URL"));
    assert!(
        env.iter()
            .any(|(k, v)| k == "CELERIS_COS_RUN_CREDENTIAL" && v == "celeris-cos-run.x")
    );
    let already = crate::dispatcher::cos_chat::launch::path_with_first(
        Some(std::path::Path::new("/rel/bin")),
        Some("/rel/bin:/usr/bin:/rel/bin".into()),
    );
    assert_eq!(already.as_deref(), Some("/rel/bin:/usr/bin"));
}

#[tokio::test]
async fn cos_chat_run_launch_two_threads_fifo_and_capacity() {
    let (_dir, store, mut d) = fixture(FakeAdapter::default_command(), 1);
    let a = thread(&store, "a");
    let b = thread(&store, "b");
    post(&store, &a, "a1");
    post(&store, &a, "a2");
    post(&store, &b, "b1");
    d.tick_cos_chat_launch();
    assert_eq!(d.cos_chat_launch.as_ref().expect("launch").running.len(), 1);
    let first = d
        .cos_chat_launch
        .as_ref()
        .expect("launch")
        .running
        .keys()
        .next()
        .cloned()
        .expect("thread");
    join(&mut d, &first).await;
    d.tick_cos_chat_launch();
    assert_eq!(d.cos_chat_launch.as_ref().expect("launch").running.len(), 1);
    let second = d
        .cos_chat_launch
        .as_ref()
        .expect("launch")
        .running
        .keys()
        .next()
        .cloned()
        .expect("thread");
    join(&mut d, &second).await;
    d.tick_cos_chat_launch();
    let third = d
        .cos_chat_launch
        .as_ref()
        .expect("launch")
        .running
        .keys()
        .next()
        .cloned()
        .expect("thread");
    join(&mut d, &third).await;
    let a_runs = runs(&store, &a);
    assert_eq!(a_runs.len(), 2);
    assert_eq!(runs(&store, &b).len(), 1);
    let a_inputs: Vec<_> = store
        .chat_message_list(
            &a,
            &ChatMessageQuery {
                limit: Some(200),
                ..Default::default()
            },
        )
        .expect("messages")
        .items
        .into_iter()
        .filter(|message| message.role == task_core::chat::ChatMessageRole::User)
        .map(|message| message.id)
        .collect();
    assert_eq!(
        a_runs
            .iter()
            .map(|run| &run.input_message_id)
            .collect::<Vec<_>>(),
        a_inputs.iter().collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn cos_chat_run_launch_two_threads_start_in_one_tick() {
    let (_dir, store, mut d) = fixture(FakeAdapter::default_command(), 2);
    let a = thread(&store, "parallel-a");
    let b = thread(&store, "parallel-b");
    post(&store, &a, "one");
    post(&store, &b, "two");
    d.tick_cos_chat_launch();
    assert_eq!(d.cos_chat_launch.as_ref().expect("launch").running.len(), 2);
    assert_eq!(d.cos_in_flight(), 2);
    join(&mut d, &a).await;
    join(&mut d, &b).await;
    assert_eq!(runs(&store, &a)[0].state, ChatRunState::Completed);
    assert_eq!(runs(&store, &b)[0].state, ChatRunState::Completed);
}

#[tokio::test]
async fn cos_chat_run_launch_non_pool_provider_keeps_its_limit() {
    let (_dir, store, mut d) = fixture(FakeAdapter::default_command(), 2);
    d.policy = Box::new(StaticPolicy::new(
        vec![ProviderSpec {
            id: "p1".into(),
            adapter: "fake".into(),
            tiers: vec![Tier::Frontier],
            concurrency: 1,
            model: "m".into(),
        }],
        Duration::from_secs(1),
    ));
    let a = thread(&store, "non-pool-a");
    let b = thread(&store, "non-pool-b");
    post(&store, &a, "one");
    post(&store, &b, "two");
    d.tick_cos_chat_launch();
    assert_eq!(d.cos_chat_launch.as_ref().expect("launch").running.len(), 1);
    let first = d
        .cos_chat_launch
        .as_ref()
        .expect("launch")
        .running
        .keys()
        .next()
        .cloned()
        .expect("first");
    let second = if first == a { &b } else { &a };
    let unavailable = runs(&store, second).pop().expect("unavailable run");
    assert_eq!(unavailable.state, ChatRunState::Failed);
    assert!(
        unavailable
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("capacity"))
    );
    join(&mut d, &first).await;
}

#[tokio::test]
async fn cos_chat_run_launch_max_cos_zero_disables_global_exception() {
    let (_dir, store, mut d) = fixture(FakeAdapter::default_command(), 0);
    d.config.max_concurrency = 0;
    let t = thread(&store, "global-limit");
    post(&store, &t, "one");
    d.tick_cos_chat_launch();
    assert!(runs(&store, &t).is_empty(), "no capacity means no claim");
    d.config.execution.max_cos_runs = 1;
    d.tick_cos_chat_launch();
    join(&mut d, &t).await;
    assert_eq!(runs(&store, &t)[0].state, ChatRunState::Completed);
}

#[tokio::test]
async fn cos_chat_run_launch_no_candidate_records_unavailable() {
    let (_dir, store, mut d) = fixture(FakeAdapter::default_command(), 2);
    d.cos_chat_launch
        .as_mut()
        .expect("launch")
        .config
        .unavailable_reason = Some("no compatible provider".into());
    let t = thread(&store, "no-provider");
    post(&store, &t, "one");
    d.tick_cos_chat_launch();
    let run = runs(&store, &t).pop().expect("run");
    assert_eq!(run.state, ChatRunState::Failed);
    assert!(
        run.reason
            .as_deref()
            .is_some_and(|reason| reason.contains("no compatible provider"))
    );
}

#[tokio::test]
async fn cos_chat_run_launch_missing_required_skill_is_unavailable() {
    let (_dir, store, mut d) = fixture(FakeAdapter::default_command(), 2);
    std::fs::remove_file(d.config.knowledge.root.join("skills/cos-operator/SKILL.md"))
        .expect("remove skill");
    let t = thread(&store, "missing-skill");
    post(&store, &t, "one");
    d.tick_cos_chat_launch();
    assert!(
        d.cos_chat_launch
            .as_ref()
            .expect("launch")
            .running
            .is_empty()
    );
    let run = runs(&store, &t).pop().expect("failed run");
    assert_eq!(run.state, ChatRunState::Failed);
    assert!(
        run.reason
            .as_deref()
            .is_some_and(|reason| reason.contains("cos-operator"))
    );
}

#[tokio::test]
async fn cos_chat_run_launch_fixed_account_gets_one_extra_slot_then_waits() {
    let (_dir, store, mut d) = fixture(FakeAdapter::default_command(), 2);
    let account_root = _dir.path().join("accounts");
    let account_dir = account_root.join("a");
    std::fs::create_dir_all(&account_dir).expect("account dir");
    std::fs::write(account_dir.join(".credentials.json"), "{}").expect("login marker");
    d.config.accounts = Some(AccountsRuntimeConfig {
        roots: HashMap::from([(AccountAdapter::ClaudeCode, account_root.clone())]),
        max_runs_per_account: 0,
        check_model: "test".into(),
        fallback_cooldown_secs: 60,
    });
    d.account_books.insert(
        AccountAdapter::ClaudeCode,
        Arc::new(StdMutex::new(crate::accounts::AccountBook::load(
            &account_root.join(".celeris-usage.json"),
        ))),
    );
    d.account_pool_providers.insert("p1".into());
    d.adapters.insert(
        "p1".into(),
        Arc::new(NamedFake(Arc::new(FakeAdapter::default()))),
    );
    let launch = d.cos_chat_launch.as_mut().expect("launch");
    launch.config.harness = "claude-code".into();
    launch.config.account_id = Some("a".into());
    let a = thread(&store, "account-a");
    let b = thread(&store, "account-b");
    post(&store, &a, "one");
    post(&store, &b, "two");
    d.tick_cos_chat_launch();
    assert_eq!(d.cos_chat_launch.as_ref().expect("launch").running.len(), 1);
    let first = d
        .cos_chat_launch
        .as_ref()
        .expect("launch")
        .running
        .keys()
        .next()
        .cloned()
        .expect("first");
    let second = if first == a { &b } else { &a };
    assert_eq!(
        runs(&store, second).len(),
        0,
        "pinned account waits without claiming"
    );
    join(&mut d, &first).await;
    d.tick_cos_chat_launch();
    assert_eq!(d.cos_chat_launch.as_ref().expect("launch").running.len(), 1);
    join(&mut d, second).await;
    assert_eq!(runs(&store, &a).len(), 1);
    assert_eq!(runs(&store, &b).len(), 1);
}

#[tokio::test]
async fn cos_chat_run_launch_sticky_account_falls_back_when_logged_out() {
    let (_dir, store, mut d) = fixture(FakeAdapter::default_command(), 2);
    let account_root = _dir.path().join("accounts");
    for id in ["a", "b"] {
        let path = account_root.join(id);
        std::fs::create_dir_all(&path).expect("account dir");
        std::fs::write(path.join(".credentials.json"), "{}").expect("login marker");
    }
    d.config.accounts = Some(AccountsRuntimeConfig {
        roots: HashMap::from([(AccountAdapter::ClaudeCode, account_root.clone())]),
        max_runs_per_account: 1,
        check_model: "test".into(),
        fallback_cooldown_secs: 60,
    });
    d.account_books.insert(
        AccountAdapter::ClaudeCode,
        Arc::new(StdMutex::new(crate::accounts::AccountBook::load(
            &account_root.join(".celeris-usage.json"),
        ))),
    );
    d.account_pool_providers.insert("p1".into());
    d.adapters.insert(
        "p1".into(),
        Arc::new(NamedFake(Arc::new(FakeAdapter::default()))),
    );
    d.cos_chat_launch.as_mut().expect("launch").config.harness = "claude-code".into();
    let t = thread(&store, "sticky");
    post(&store, &t, "first");
    d.tick_cos_chat_launch();
    join(&mut d, &t).await;
    assert_eq!(runs(&store, &t)[0].account_id.as_deref(), Some("a"));
    std::fs::remove_file(account_root.join("a/.credentials.json")).expect("logout");
    post(&store, &t, "second");
    d.tick_cos_chat_launch();
    join(&mut d, &t).await;
    let runs = runs(&store, &t);
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[1].account_id.as_deref(), Some("b"));
    assert_eq!(
        runs[1].session_mode,
        Some(task_core::chat::ChatSessionMode::Fresh)
    );
}

#[tokio::test]
async fn cos_chat_run_launch_stages_image_manifest_for_worker() {
    let (dir, store, mut d) = fixture(FakeAdapter::default_command(), 2);
    let t = thread(&store, "image");
    let attachments = ChatAttachmentStore::open(
        dir.path(),
        &dir.path().join("celeris.db"),
        Default::default(),
    )
    .expect("attachments");
    let image = attachments
        .upload(
            &t,
            "upload",
            "screen.png",
            None,
            std::io::Cursor::new(b"\x89PNG\r\n\x1a\nimage"),
            OffsetDateTime::now_utc(),
        )
        .expect("image");
    store
        .chat_message_post(
            &t,
            &ChatPostMessageRequest {
                client_message_id: "image-message".into(),
                text: "inspect".into(),
                attachment_ids: vec![image.id.clone()],
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
            },
            OffsetDateTime::now_utc(),
        )
        .expect("post");
    d.tick_cos_chat_launch();
    let run = runs(&store, &t).pop().expect("run");
    join(&mut d, &t).await;
    assert_eq!(
        store.chat_run_get(&t, &run.id).expect("run").state,
        ChatRunState::Completed
    );
    let request = std::fs::read_to_string(
        dir.path()
            .join("cos/threads")
            .join(&t)
            .join("workspace/runs")
            .join(&run.id)
            .join("request.json"),
    )
    .expect("request");
    let value: serde_json::Value = serde_json::from_str(&request).expect("json");
    let entry = &value["context"]["cos_chat"]["attachments"][0];
    assert_eq!(entry["id"], image.id);
    assert_eq!(entry["delivery"], "image");
    let path = entry["path"].as_str().expect("path");
    assert_eq!(
        std::fs::read(path).expect("staged image"),
        b"\x89PNG\r\n\x1a\nimage"
    );
}

#[derive(Clone)]
struct SignallingFake {
    inner: Arc<dyn WorkerAdapter>,
    progress_sent: std::sync::mpsc::Sender<()>,
}

struct ProgressSignalSink<'a> {
    inner: &'a dyn EventSink,
    progress_sent: &'a std::sync::mpsc::Sender<()>,
}

impl EventSink for ProgressSignalSink<'_> {
    fn progress(&self, msg: &str) {
        self.inner.progress(msg);
    }

    fn progress_with(&self, msg: &str, fields: &task_core::ProgressFields) {
        self.inner.progress_with(msg, fields);
        if msg == "working" {
            let _ = self.progress_sent.send(());
        }
    }

    fn artifact(&self, artifact: &task_core::ArtifactRef) {
        self.inner.artifact(artifact);
    }
}

#[async_trait]
impl WorkerAdapter for SignallingFake {
    fn id(&self) -> &str {
        "fake"
    }

    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let signalling = ProgressSignalSink {
            inner: sink,
            progress_sent: &self.progress_sent,
        };
        self.inner.run(req, run_id, limits, &signalling).await
    }

    fn with_env(&self, extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        Some(Arc::new(Self {
            inner: self.inner.with_env(extra)?,
            progress_sent: self.progress_sent.clone(),
        }))
    }
}

#[tokio::test]
async fn cos_chat_run_e2e_send_stream_interrupt_stop_resume_and_restart() {
    // FIFO is the worker barrier: all progress is persisted before the test interrupts it.
    let command = vec![
        "sh".into(),
        "-c".into(),
        "cat >/dev/null; printf '%s\n' \
         '{\"type\":\"progress\",\"msg\":\"hé\",\"kind\":\"text\",\"detail\":\"hé\"}' \
         '{\"type\":\"progress\",\"msg\":\"tool\",\"kind\":\"tool_use\",\"tool\":\"Read\",\"summary\":\"file\"}' \
         '{\"type\":\"progress\",\"msg\":\"working\",\"kind\":\"status\",\"summary\":\"working\"}'; \
         printf ready > ready.fifo; read x < release.fifo; \
         echo '{\"type\":\"done\",\"summary\":\"done\",\"evidence\":[]}'".into(),
    ];
    let (dir, store, mut d) = fixture(command.clone(), 2);
    let (progress_sent, progress_ready) = std::sync::mpsc::channel();
    d.adapters.insert(
        "p1".into(),
        Arc::new(SignallingFake {
            inner: Arc::new(FakeAdapter::new(command)),
            progress_sent,
        }),
    );
    d.test_now = Some(Arc::new(StdMutex::new(OffsetDateTime::now_utc())));
    d.config.kill_grace = std::time::Duration::from_millis(20);
    let t = thread(&store, "e2e");
    let workspace = dir.path().join("cos/threads").join(&t).join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    for name in ["ready.fifo", "release.fifo"] {
        assert!(
            std::process::Command::new("mkfifo")
                .arg(workspace.join(name))
                .status()
                .expect("mkfifo")
                .success()
        );
    }
    post(&store, &t, "first");
    post(&store, &t, "queued");
    let ready = tokio::task::spawn_blocking({
        let path = workspace.join("ready.fifo");
        move || std::fs::read_to_string(path).expect("first barrier")
    });
    d.tick_cos_chat_launch();
    let first = active_run(&store, &t);
    tokio::time::timeout(std::time::Duration::from_secs(10), ready)
        .await
        .expect("first barrier deadline")
        .expect("first barrier task");
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::task::spawn_blocking(move || progress_ready.recv().expect("persisted status")),
    )
    .await
    .expect("progress deadline")
    .expect("progress task");

    // The store page is exactly the SSE replay source. Check the claim order and
    // cursor before interruption; check worker progress after its join.
    let page = store
        .chat_events_page(
            &t,
            &ChatEventQuery {
                after: Some("0".into()),
                run_id: None,
                limit: Some(200),
            },
        )
        .expect("SSE replay page");
    let events = &page.items;
    let start = events
        .iter()
        .position(|event| {
            event.run_id.as_deref() == Some(&first.id)
                && matches!(event.data, ChatEventData::Message(_))
        })
        .expect("claimed input event");
    assert!(matches!(events[start].data, ChatEventData::Message(_)));
    assert!(matches!(events[start + 1].data, ChatEventData::Message(_)));
    assert!(matches!(events[start + 2].data, ChatEventData::Run(_)));
    assert!(matches!(events[start + 3].data, ChatEventData::Queue(_)));
    for pair in events.windows(2) {
        assert!(
            pair[0].id.parse::<i64>().expect("cursor") < pair[1].id.parse::<i64>().expect("cursor")
        );
    }
    let cursor = events.last().expect("last event").id.clone();
    assert!(
        store
            .chat_events_page(
                &t,
                &ChatEventQuery {
                    after: Some(cursor.clone()),
                    run_id: None,
                    limit: Some(200),
                }
            )
            .expect("caught-up replay")
            .items
            .is_empty()
    );

    store
        .chat_message_post(
            &t,
            &ChatPostMessageRequest {
                client_message_id: "urgent".into(),
                text: "urgent".into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: ChatSendMode::Interrupt,
                resume_queue: false,
            },
            d.now_utc(),
        )
        .expect("interrupt");
    d.tick_cos_chat_launch();
    tokio::time::timeout(std::time::Duration::from_secs(10), join(&mut d, &t))
        .await
        .expect("interrupted worker deadline");
    assert_eq!(
        store.chat_run_get(&t, &first.id).expect("first").state,
        ChatRunState::Interrupted
    );
    let all = store
        .chat_events_page(
            &t,
            &ChatEventQuery {
                after: Some("0".into()),
                run_id: None,
                limit: Some(200),
            },
        )
        .expect("SSE replay after worker exit");
    let text_at = all
        .items
        .iter()
        .position(|e| {
            matches!(&e.data,
        ChatEventData::TextDelta(delta) if delta.offset == 0 && delta.text == "hé")
        })
        .expect("UTF-8 text delta");
    let tool_at = all
        .items
        .iter()
        .position(|e| {
            matches!(&e.data,
        ChatEventData::Tool(tool) if tool.name == "Read")
        })
        .expect("tool event");
    let status_at = all
        .items
        .iter()
        .position(|e| {
            matches!(&e.data,
        ChatEventData::Status(status) if status.summary == "working")
        })
        .expect("status event");
    assert!(
        start + 3 < text_at && text_at < tool_at && tool_at < status_at,
        "SSE replay preserves message/run/queue/text/tool/status order"
    );
    let replay = store
        .chat_events_page(
            &t,
            &ChatEventQuery {
                after: Some(cursor),
                run_id: None,
                limit: Some(200),
            },
        )
        .expect("SSE continuation");
    assert!(replay.items.iter().any(|e| matches!(&e.data,
        ChatEventData::Run(data) if data.run.id == first.id
            && data.run.state == ChatRunState::Interrupted)));

    let ready = tokio::task::spawn_blocking({
        let path = workspace.join("ready.fifo");
        move || std::fs::read_to_string(path).expect("second barrier")
    });
    d.tick_cos_chat_launch();
    let second = active_run(&store, &t);
    assert_ne!(first.id, second.id);
    tokio::time::timeout(std::time::Duration::from_secs(10), ready)
        .await
        .expect("second barrier deadline")
        .expect("second barrier task");
    assert!(
        store
            .chat_run_stop(&t, &second.id, d.now_utc())
            .expect("stop")
            .accepted
    );
    d.tick_cos_chat_launch();
    tokio::time::timeout(std::time::Duration::from_secs(10), join(&mut d, &t))
        .await
        .expect("stopped worker deadline");
    assert_eq!(
        store.chat_run_get(&t, &second.id).expect("second").state,
        ChatRunState::Stopped
    );
    assert!(
        store
            .chat_thread_get(&t)
            .expect("thread")
            .expect("exists")
            .queue_paused
    );
    d.adapters
        .insert("p1".into(), Arc::new(FakeAdapter::default()));
    let revision = store
        .chat_thread_get(&t)
        .expect("thread")
        .expect("exists")
        .revision;
    store
        .chat_thread_resume_queue(&t, revision, d.now_utc())
        .expect("resume queue");
    d.tick_cos_chat_launch();
    let third = active_run(&store, &t);
    join(&mut d, &t).await;
    assert_eq!(
        store.chat_run_get(&t, &third.id).expect("third").state,
        ChatRunState::Completed
    );
    // A second thread isolates restart recovery from the first thread's queue.
    let restarted_thread = thread(&store, "e2e-restart");
    post(&store, &restarted_thread, "after-restart");
    let orphan = store
        .chat_run_claim_next(
            &restarted_thread,
            "e2e-orphan",
            &serde_json::json!({}),
            d.now_utc(),
        )
        .expect("orphan claim")
        .expect("orphan run");
    d.set_orphan_takeover(crate::orphan::OrphanTakeover {
        instance_id: "restarted-daemon".into(),
        freshness: std::time::Duration::from_secs(60),
        pid_alive: Arc::new(|_| true),
    });
    d.tick_cos_chat_launch();
    assert_eq!(
        store
            .chat_run_get(&restarted_thread, &orphan.id)
            .expect("orphan")
            .state,
        ChatRunState::Interrupted
    );
    let resumed = active_run(&store, &restarted_thread);
    assert_ne!(resumed.id, orphan.id);
    let resumed_input = store
        .chat_message_list(
            &restarted_thread,
            &ChatMessageQuery {
                limit: Some(100),
                ..Default::default()
            },
        )
        .expect("recovered history")
        .items
        .into_iter()
        .find(|message| message.id == resumed.input_message_id)
        .expect("recovered input");
    assert_eq!(resumed_input.text, "after-restart");
    join(&mut d, &restarted_thread).await;
}
