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
