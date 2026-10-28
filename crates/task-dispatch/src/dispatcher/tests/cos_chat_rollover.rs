//! CoS chat session rollover: resume refusal → one fresh retry in the same run, and fresh runs
//! after `[sessions] rollover_tokens` or a context exhaustion. Driven by the FakeAdapter sh
//! scripts and worker joins; no sleeps.

use super::*;
use crate::dispatcher::cos_chat::launch::CosChatLaunchConfig;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use task_core::chat::{
    AuditContext, ChatActor, ChatMessageQuery, ChatMessageState, ChatOpenThreadRequest,
    ChatPostMessageRequest, ChatRunSessionMode, ChatRunState, ChatSendMode,
};
use task_worker::fake::FakeAdapter;

/// Fake harness around FakeAdapter: from attempt `refuse_from` on, a resumed attempt (or every
/// attempt when `refuse_fresh`) reports `session_resume_failed` instead of running the script.
/// Every attempt applies one CoS operation keyed by its input message, like a worker would.
#[derive(Clone)]
struct RefusingFake {
    inner: Arc<dyn WorkerAdapter>,
    store: Arc<SqliteStore>,
    refuse_from: usize,
    refuse_fresh: bool,
    attempts: Arc<Mutex<Vec<RunRequest>>>,
    credential_ok: Arc<Mutex<Vec<bool>>>,
    applied: Arc<AtomicUsize>,
    token: Option<String>,
}

impl RefusingFake {
    fn apply_operation(&self, req: &RunRequest) {
        let Some(chat) = req.context.cos_chat.as_ref() else {
            return;
        };
        let applied = Arc::clone(&self.applied);
        let ctx = AuditContext {
            actor: ChatActor::Cos,
            thread_id: chat.thread_id.clone(),
            run_id: chat.run_id.clone(),
            operation_id: ulid::Ulid::new().to_string(),
            reason: "test".into(),
            policy_version: "v1".into(),
        };
        self.store
            .cos_operation_apply(
                &ctx,
                &format!("{}:note", chat.inputs[0].id),
                "hash-1",
                "task",
                "T1",
                None,
                "note",
                &serde_json::json!({}),
                move |_, _| {
                    applied.fetch_add(1, Ordering::SeqCst);
                    Ok(serde_json::json!({}))
                },
            )
            .expect("operation");
    }
}

#[async_trait]
impl WorkerAdapter for RefusingFake {
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
        let n = {
            let mut attempts = self.attempts.lock().expect("attempts");
            attempts.push(req.clone());
            attempts.len()
        };
        let secret = self
            .token
            .as_deref()
            .and_then(|t| t.strip_prefix("celeris-cos-run."))
            .unwrap_or("");
        self.credential_ok.lock().expect("cred").push(
            self.store
                .cos_run_credential_verify(secret, OffsetDateTime::now_utc())
                .is_ok(),
        );
        self.apply_operation(&req);
        let resume = req.context.session.as_ref().is_some_and(|s| s.resume);
        if n > self.refuse_from && (resume || self.refuse_fresh) {
            if let Some(chat) = req.context.cos_chat.as_ref()
                && chat.summary.is_none()
            {
                // The worker summarised before failing: the watermark moves, the input does not.
                self.store
                    .chat_thread_checkpoint_at(
                        &chat.thread_id,
                        &chat.run_id,
                        "summary-A",
                        2,
                        0,
                        OffsetDateTime::now_utc(),
                    )
                    .expect("checkpoint");
            }
            sink.session_resume_failed("No conversation found with session ID");
            return Ok(RunOutcome {
                terminal: Terminal::Error {
                    message: "resume refused".into(),
                    retryable: false,
                    usage: None,
                },
                exit_code: Some(1),
            });
        }
        if !resume {
            // codex/acp style: the harness decides the session id on a fresh start.
            sink.session_established(&format!("fake-session-{n}"));
        }
        self.inner.run(req, run_id, limits, sink).await
    }

    fn with_env(&self, extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        let mut next = self.clone();
        next.inner = self.inner.with_env(extra)?;
        next.token = extra.first().map(|(_, v)| v.clone());
        Some(Arc::new(next))
    }
}

fn fixture(adapter: Arc<dyn WorkerAdapter>, store: Arc<SqliteStore>, dir: &Path) -> Dispatcher {
    let mut d = dispatcher_with_test_clock(store.clone(), adapter, 2, true);
    d.config.execution.max_cos_runs = 2;
    d.config.min_free_disk_mb = 0;
    d.config.knowledge.root = dir.join("knowledge");
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
        store,
        CosChatLaunchConfig {
            enabled: true,
            worker_reserve_five_hour: 0.90,
            harness: "fake".into(),
            max_turns: 2,
            max_wall_secs: 30,
            unavailable_reason: None,
            data_dir: dir.to_path_buf(),
            db_path: dir.join("celeris.db"),
            attachment_limits: Default::default(),
            api_base_url: "http://127.0.0.1:7700/api/v1".into(),
            web_base_url: None,
            triage: Default::default(),
        },
    );
    d
}

fn open_store() -> (crate::test_support::WritableTempDir, Arc<SqliteStore>) {
    let dir = crate::test_support::WritableTempDir::new();
    let store = Arc::new(SqliteStore::open(&dir.path().join("celeris.db")).expect("store"));
    (dir, store)
}

fn thread(store: &SqliteStore) -> String {
    store
        .cos_thread_open(
            &ChatOpenThreadRequest { project_id: None },
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

/// Ticks once, joins the started worker and returns the run it served.
async fn tick_and_join(d: &mut Dispatcher, store: &SqliteStore, thread: &str) -> String {
    d.tick_cos_chat_launch();
    let task = d
        .cos_chat_launch
        .as_mut()
        .expect("launch configured")
        .running
        .remove(thread)
        .expect("worker launched");
    task.await.expect("worker did not panic");
    run_ids(store, thread).pop().expect("run")
}

fn run_ids(store: &SqliteStore, thread: &str) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for m in store
        .chat_message_list(
            thread,
            &ChatMessageQuery {
                limit: Some(200),
                ..Default::default()
            },
        )
        .expect("messages")
        .items
    {
        if let Some(id) = m.run_id
            && !ids.contains(&id)
        {
            ids.push(id);
        }
    }
    ids
}

fn refusing(
    store: &Arc<SqliteStore>,
    refuse_from: usize,
    refuse_fresh: bool,
) -> (RefusingFake, Arc<dyn WorkerAdapter>) {
    harness(
        store,
        refuse_from,
        refuse_fresh,
        FakeAdapter::default_command(),
    )
}

fn harness(
    store: &Arc<SqliteStore>,
    refuse_from: usize,
    refuse_fresh: bool,
    command: Vec<String>,
) -> (RefusingFake, Arc<dyn WorkerAdapter>) {
    let fake = RefusingFake {
        inner: Arc::new(FakeAdapter::new(command)),
        store: Arc::clone(store),
        refuse_from,
        refuse_fresh,
        attempts: Arc::default(),
        credential_ok: Arc::default(),
        applied: Arc::default(),
        token: None,
    };
    (fake.clone(), Arc::new(fake))
}

#[tokio::test]
async fn cos_chat_run_rollover_resume_refusal_retries_fresh_exactly_once() {
    let (dir, store) = open_store();
    let (probe, adapter) = refusing(&store, 1, false);
    let mut d = fixture(adapter, store.clone(), dir.path());
    let t = thread(&store);
    post(&store, &t, "one");
    let first = tick_and_join(&mut d, &store, &t).await;
    let old = store
        .chat_session_active(&t)
        .expect("session")
        .expect("live session");
    post(&store, &t, "two");
    let second = tick_and_join(&mut d, &store, &t).await;
    assert_ne!(first, second);

    let attempts = probe.attempts.lock().expect("attempts").clone();
    assert_eq!(attempts.len(), 3, "one attempt for run 1, two for run 2");
    let refused = attempts[1].context.session.as_ref().expect("session");
    let fresh = attempts[2].context.session.as_ref().expect("session");
    assert!(refused.resume);
    assert!(!fresh.resume);
    assert_ne!(refused.session_id, fresh.session_id);

    let run = store.chat_run_get(&t, &second).expect("run");
    assert_eq!(run.state, ChatRunState::Completed);
    let record = store.chat_run_session_record(&second).expect("record");
    assert_eq!(
        record.detail.as_deref(),
        Some(ChatRunSessionMode::FreshAfterRefusal.as_str())
    );
    assert_eq!(record.reason.as_deref(), Some("resume_refused"));
    let retired = store
        .chat_session_get(&old.id)
        .expect("old row")
        .expect("old row exists");
    assert!(retired.retired_at.is_some());
    let live = store
        .chat_session_active(&t)
        .expect("session")
        .expect("live");
    assert_eq!(record.session_row_id.as_deref(), Some(live.id.as_str()));
    // Claude Code gets a fresh UUID before spawn; the worker then reports the live session id.
    assert!(task_worker::provider::is_valid_uuid(&fresh.session_id));
    assert_eq!(live.session_id, "fake-session-3");
}

#[tokio::test]
async fn cos_chat_run_rollover_second_refusal_fails_with_reason() {
    let (dir, store) = open_store();
    let (probe, adapter) = refusing(&store, 1, true);
    let mut d = fixture(adapter, store.clone(), dir.path());
    let t = thread(&store);
    post(&store, &t, "one");
    tick_and_join(&mut d, &store, &t).await;
    post(&store, &t, "two");
    let second = tick_and_join(&mut d, &store, &t).await;

    assert_eq!(probe.attempts.lock().expect("attempts").len(), 3);
    let run = store.chat_run_get(&t, &second).expect("run");
    assert_eq!(run.state, ChatRunState::Failed);
    let reason = run.reason.unwrap_or_default();
    assert!(
        reason.contains("again after the single fresh retry"),
        "{reason}"
    );
    // No third attempt: nothing is queued and the next tick starts nothing.
    d.tick_cos_chat_launch();
    assert!(
        d.cos_chat_launch
            .as_ref()
            .expect("launch")
            .running
            .is_empty()
    );
    assert_eq!(probe.attempts.lock().expect("attempts").len(), 3);
    assert_eq!(run_ids(&store, &t).len(), 2);
}

#[tokio::test]
async fn cos_chat_run_rollover_retry_does_not_reapply_input_or_operation() {
    let (dir, store) = open_store();
    let (probe, adapter) = refusing(&store, 1, false);
    let mut d = fixture(adapter, store.clone(), dir.path());
    let t = thread(&store);
    post(&store, &t, "one");
    tick_and_join(&mut d, &store, &t).await;
    post(&store, &t, "two");
    let second = tick_and_join(&mut d, &store, &t).await;

    let attempts = probe.attempts.lock().expect("attempts").clone();
    let refused = attempts[1].context.cos_chat.as_ref().expect("chat");
    let fresh = attempts[2].context.cos_chat.as_ref().expect("chat");
    // Same input (delivery record), same chat run: the message was not claimed again.
    assert_eq!(refused.inputs, fresh.inputs);
    assert_eq!(refused.run_id, fresh.run_id);
    let run = store.chat_run_get(&t, &second).expect("run");
    assert_eq!(run.input_message_id, refused.inputs[0].id);
    let input = store
        .chat_message_list(
            &t,
            &ChatMessageQuery {
                limit: Some(200),
                ..Default::default()
            },
        )
        .expect("messages")
        .items
        .into_iter()
        .find(|m| m.id == run.input_message_id)
        .expect("input");
    assert_eq!(input.state, ChatMessageState::Completed);
    assert_eq!(input.run_id.as_deref(), Some(second.as_str()));
    assert_eq!(run_ids(&store, &t).len(), 2);
    assert_eq!(
        store
            .chat_thread_get(&t)
            .expect("thread")
            .expect("exists")
            .queued_count,
        0
    );
    // The operation keyed by the input was applied once (one per input across 3 attempts).
    assert_eq!(probe.applied.load(Ordering::SeqCst), 2);
    let op = store
        .cos_operation_find(&t, &format!("{}:note", refused.inputs[0].id))
        .expect("find")
        .expect("recorded");
    assert_eq!(op.run_id, second);
    // The run credential stayed valid for the retry.
    assert_eq!(
        probe.credential_ok.lock().expect("cred").as_slice(),
        &[true, true, true]
    );
    // The summary watermark moved independently of the delivery cursor.
    assert_eq!(refused.summary, None);
    assert_eq!(fresh.summary.as_deref(), Some("summary-A"));
    assert_eq!(fresh.summary_through_seq, 2);
    assert_eq!(fresh.unsummarized.from_seq, 3);
    assert!(
        fresh
            .unsummarized
            .messages
            .iter()
            .all(|m| m.seq > 2 && m.seq < fresh.inputs[0].seq)
    );
}

fn script(lines: &str) -> Vec<String> {
    vec!["sh".into(), "-c".into(), format!("cat >/dev/null; {lines}")]
}

#[tokio::test]
async fn cos_chat_run_rollover_token_limit_makes_next_run_fresh() {
    let (dir, store) = open_store();
    let (_, adapter) = harness(
        &store,
        usize::MAX,
        false,
        script(
            "printf '%s\\n' '{\"type\":\"done\",\"summary\":\"ok\",\"evidence\":[],\"usage\":{\"input_tokens\":80,\"output_tokens\":30}}'",
        ),
    );
    let mut d = fixture(adapter, store.clone(), dir.path());
    d.config.session_rollover_tokens = 100;
    let t = thread(&store);
    post(&store, &t, "one");
    tick_and_join(&mut d, &store, &t).await;
    let first_session = store.chat_session_active(&t).expect("s").expect("live");
    assert_eq!(first_session.approx_tokens, 110);
    post(&store, &t, "two");
    let second = tick_and_join(&mut d, &store, &t).await;
    let record = store.chat_run_session_record(&second).expect("record");
    assert_eq!(
        record.detail.as_deref(),
        Some(ChatRunSessionMode::Fresh.as_str())
    );
    assert_eq!(record.reason.as_deref(), Some("token_rollover"));
    assert_ne!(
        record.session_row_id.as_deref(),
        Some(first_session.id.as_str())
    );
}

fn usage_script(usage: &str) -> Vec<String> {
    script(&format!(
        "printf '%s\\n' '{{\"type\":\"done\",\"summary\":\"ok\",\"evidence\":[],\"usage\":{usage}}}'"
    ))
}

#[tokio::test]
async fn cos_chat_run_rollover_cache_heavy_run_is_not_underestimated() {
    // input+output is only 15, but the last call held 600 tokens of context (mostly cache reads).
    let (dir, store) = open_store();
    let (_, adapter) = harness(
        &store,
        usize::MAX,
        false,
        usage_script(
            "{\"input_tokens\":10,\"output_tokens\":5,\"cache_read_tokens\":550,\"cache_creation_tokens\":40,\"context_tokens\":600}",
        ),
    );
    let mut d = fixture(adapter, store.clone(), dir.path());
    d.config.session_rollover_tokens = 500;
    let t = thread(&store);
    post(&store, &t, "one");
    tick_and_join(&mut d, &store, &t).await;
    let first = store.chat_session_active(&t).expect("s").expect("live");
    assert_eq!(first.approx_tokens, 15);
    assert_eq!(first.last_context_tokens, Some(600));
    assert_eq!(first.billed_input_tokens, 10);
    post(&store, &t, "two");
    let second = tick_and_join(&mut d, &store, &t).await;
    let record = store.chat_run_session_record(&second).expect("record");
    assert_eq!(record.reason.as_deref(), Some("token_rollover"));
    assert_ne!(record.session_row_id.as_deref(), Some(first.id.as_str()));
}

#[tokio::test]
async fn cos_chat_run_rollover_occupancy_is_replaced_not_accumulated() {
    // Each run reports 300 of context; cumulative billing grows (10 → 20) but occupancy stays 300,
    // below the 500 threshold, so the next runs resume.
    let (dir, store) = open_store();
    let (_, adapter) = harness(
        &store,
        usize::MAX,
        false,
        usage_script(
            "{\"input_tokens\":10,\"output_tokens\":5,\"cache_read_tokens\":290,\"context_tokens\":300}",
        ),
    );
    let mut d = fixture(adapter, store.clone(), dir.path());
    d.config.session_rollover_tokens = 500;
    let t = thread(&store);
    post(&store, &t, "one");
    tick_and_join(&mut d, &store, &t).await;
    post(&store, &t, "two");
    let second = tick_and_join(&mut d, &store, &t).await;
    let live = store.chat_session_active(&t).expect("s").expect("live");
    assert_eq!(live.turns, 2);
    assert_eq!(live.last_context_tokens, Some(300));
    assert_eq!(live.billed_input_tokens, 20);
    post(&store, &t, "three");
    let third = tick_and_join(&mut d, &store, &t).await;
    for run in [&second, &third] {
        let record = store.chat_run_session_record(run).expect("record");
        assert_eq!(
            record.detail.as_deref(),
            Some(ChatRunSessionMode::Resumed.as_str())
        );
        assert_eq!(record.session_row_id.as_deref(), Some(live.id.as_str()));
    }
}

#[tokio::test]
async fn cos_chat_run_rollover_unknown_occupancy_falls_back_to_cumulative() {
    // No context_tokens (e.g. codex/aider): approx_tokens (input+output) accumulates as before.
    let (dir, store) = open_store();
    let (_, adapter) = harness(
        &store,
        usize::MAX,
        false,
        usage_script("{\"input_tokens\":40,\"output_tokens\":20,\"cache_read_tokens\":9000}"),
    );
    let mut d = fixture(adapter, store.clone(), dir.path());
    d.config.session_rollover_tokens = 100;
    let t = thread(&store);
    post(&store, &t, "one");
    tick_and_join(&mut d, &store, &t).await;
    let first = store.chat_session_active(&t).expect("s").expect("live");
    assert_eq!(first.last_context_tokens, None);
    assert_eq!(first.approx_tokens, 60);
    post(&store, &t, "two");
    let second = tick_and_join(&mut d, &store, &t).await;
    assert_eq!(
        store
            .chat_run_session_record(&second)
            .expect("record")
            .detail
            .as_deref(),
        Some(ChatRunSessionMode::Resumed.as_str())
    );
    post(&store, &t, "three");
    let third = tick_and_join(&mut d, &store, &t).await;
    assert_eq!(
        store
            .chat_run_session_record(&third)
            .expect("record")
            .reason
            .as_deref(),
        Some("token_rollover")
    );
}

#[tokio::test]
async fn cos_chat_run_rollover_context_exhaustion_makes_next_run_fresh() {
    let (dir, store) = open_store();
    let (_, adapter) = harness(
        &store,
        usize::MAX,
        false,
        script(
            "if [ -f .exhausted-once ]; then \
           printf '%s\\n' '{\"type\":\"done\",\"summary\":\"ok\",\"evidence\":[]}'; \
         else touch .exhausted-once; \
           printf '%s\\n' '{\"type\":\"budget_exhausted\",\"kind\":\"context\",\"message\":\"context window full\"}'; \
         fi",
        ),
    );
    let mut d = fixture(adapter, store.clone(), dir.path());
    let t = thread(&store);
    post(&store, &t, "one");
    let first = tick_and_join(&mut d, &store, &t).await;
    let run = store.chat_run_get(&t, &first).expect("run");
    assert!(
        run.reason
            .as_deref()
            .is_some_and(|r| r.starts_with("context exhausted")),
        "{:?}",
        run.reason
    );
    post(&store, &t, "two");
    let second = tick_and_join(&mut d, &store, &t).await;
    let record = store.chat_run_session_record(&second).expect("record");
    assert_eq!(
        record.detail.as_deref(),
        Some(ChatRunSessionMode::Fresh.as_str())
    );
    assert_eq!(record.reason.as_deref(), Some("context_exhausted"));
    // After a normal run on the fresh session, the next run resumes it.
    post(&store, &t, "three");
    let third = tick_and_join(&mut d, &store, &t).await;
    let record3 = store.chat_run_session_record(&third).expect("record");
    assert_eq!(
        record3.detail.as_deref(),
        Some(ChatRunSessionMode::Resumed.as_str())
    );
    assert_eq!(record3.session_row_id, record.session_row_id);
}

#[tokio::test]
async fn cos_chat_usage_fake_adapter_persists_usage_and_omits_missing_usage() {
    let (dir, store) = open_store();
    let adapter = Arc::new(FakeAdapter::new(vec![
        "sh".into(), "-c".into(), r#"cat >/dev/null
echo '{"type":"progress","msg":"skill","kind":"tool_use","tool":"Skill","detail":"{\"skill\":\"cos-operator\"}"}'
echo '{"type":"progress","msg":"read","kind":"tool_use","tool":"Read","detail":"{\"file_path\":\"/workspace/.claude/skills/cos-operator/SKILL.md\"}"}'
echo '{"type":"progress","msg":"answer","kind":"text","detail":"answer"}'
echo '{"type":"done","summary":"answer","evidence":[],"usage":{"input_tokens":11,"output_tokens":7,"cache_read_tokens":23,"cache_creation_tokens":5,"cost_usd":0.012,"duplicate_reads":2,"session_resumed":false}}'
"#.into(),
    ]));
    let mut d = fixture(adapter, store.clone(), dir.path());
    let t = thread(&store);
    post(&store, &t, "usage");
    let id = tick_and_join(&mut d, &store, &t).await;
    let run = store.chat_run_get(&t, &id).expect("run");
    let usage = run.usage.as_deref().expect("usage");
    assert_eq!(usage.input_tokens, Some(11));
    assert_eq!(usage.output_tokens, Some(7));
    assert_eq!(usage.cache_read_tokens, Some(23));
    assert_eq!(usage.cache_creation_tokens, Some(5));
    assert_eq!(usage.cost_usd, Some(0.012));
    assert_eq!(usage.duplicate_reads, Some(2));
    assert_eq!(usage.session_resumed, Some(false));
    assert_eq!(run.skill_reads, Some(2));
    assert!(run.first_output_at.is_some());
    assert!(run.latency_ms.is_some());
    assert!(run.time_to_first_output_ms.is_some());
    assert_eq!(run.harness.as_deref(), Some("claude_code"));
    assert!(run.session_mode.is_some());
    let reopened = SqliteStore::open(&dir.path().join("celeris.db")).expect("reopen");
    assert_eq!(reopened.chat_run_get(&t, &id).expect("persisted"), run);
    assert_eq!(
        reopened.chat_run_list(&t, None, None).expect("list").items,
        vec![run]
    );

    let mut d = fixture(Arc::new(FakeAdapter::default()), store.clone(), dir.path());
    post(&store, &t, "no-usage");
    let id = tick_and_join(&mut d, &store, &t).await;
    let run = store.chat_run_get(&t, &id).expect("run");
    let wire = serde_json::to_value(&run).expect("json");
    assert!(wire.get("usage").is_none());
    assert!(wire.get("first_output_at").is_none());
    assert!(wire.get("time_to_first_output_ms").is_none());
    assert_eq!(run.skill_reads, Some(0));
}

// These are the unchanged worker parser results for the codex turn.completed fixture
// (input_tokens=10, cached_input_tokens=4, output_tokens=20) and Anthropic result.usage.
fn billed_input_outcome(usage: task_core::Usage) -> Result<RunOutcome, AdapterError> {
    Ok(RunOutcome {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: Some(usage),
        },
        exit_code: Some(0),
    })
}

#[test]
fn cos_chat_billed_input_codex_cached_input_is_not_added_twice() {
    let outcome = billed_input_outcome(task_core::Usage {
        input_tokens: Some(10),
        cache_read_tokens: Some(4),
        output_tokens: Some(20),
        ..Default::default()
    });
    let usage = crate::dispatcher::cos_chat::rollover::session_usage("codex", &outcome);
    assert_eq!(usage.billed_input, 10);
    assert_eq!(usage.add_tokens, 30);
    assert_eq!(usage.context_tokens, None);
    assert_billed_input_persisted("codex", usage, 10);
}

#[test]
fn cos_chat_billed_input_claude_adds_external_cache_tokens() {
    let outcome = billed_input_outcome(task_core::Usage {
        input_tokens: Some(10),
        cache_read_tokens: Some(4),
        cache_creation_tokens: Some(2),
        output_tokens: Some(20),
        context_tokens: Some(12),
        ..Default::default()
    });
    let usage = crate::dispatcher::cos_chat::rollover::session_usage("claude-code", &outcome);
    assert_eq!(usage.billed_input, 16);
    assert_eq!(usage.add_tokens, 30);
    assert_eq!(usage.context_tokens, Some(12));
    assert_billed_input_persisted("claude-code", usage, 16);
}

// pi fixture: two calls of input=100, cacheRead=40, cacheWrite=5 (cache is outside input).
// The worker sums the calls; occupancy is the last call: 100+40+5.
#[test]
fn cos_chat_pi_cache_is_external_occupancy_and_billed_input() {
    let outcome = billed_input_outcome(task_core::Usage {
        input_tokens: Some(200),
        cache_read_tokens: Some(80),
        cache_creation_tokens: Some(10),
        output_tokens: Some(20),
        context_tokens: Some(145),
        ..Default::default()
    });
    let usage = crate::dispatcher::cos_chat::rollover::session_usage("pi", &outcome);
    assert_eq!(usage.context_tokens, Some(145));
    assert_eq!(usage.billed_input, 290);
    assert_eq!(usage.add_tokens, 220);
}

fn assert_billed_input_persisted(
    harness: &str,
    usage: task_core::chat::ChatSessionUsage,
    expected: i64,
) {
    let store = SqliteStore::open_in_memory().expect("store");
    let t = thread(&store);
    let now = OffsetDateTime::now_utc();
    let row = task_core::chat::ChatSession::new(
        task_core::chat::ChatSessionKey {
            thread_id: t.clone(),
            harness: harness.into(),
            ..Default::default()
        },
        "fixture-session",
        now,
    );
    store.chat_session_rotate(&row, now).expect("rotate");
    for runs in 1..=2 {
        store
            .chat_session_touch(&row.id, usage, now)
            .expect("touch");
        let saved = store
            .chat_session_active(&t)
            .expect("session")
            .expect("live");
        assert_eq!(saved.billed_input_tokens, expected * runs);
        assert_eq!(saved.approx_tokens, 30 * runs);
        assert_eq!(saved.last_context_tokens, usage.context_tokens);
    }
}

#[test]
fn cos_chat_billed_input_unknown_cache_definition_uses_input_only() {
    let outcome = billed_input_outcome(task_core::Usage {
        input_tokens: Some(10),
        cache_read_tokens: Some(4),
        cache_creation_tokens: Some(2),
        output_tokens: Some(20),
        context_tokens: Some(16),
        ..Default::default()
    });
    for harness in ["acp", "opencode", "fake", "unknown"] {
        let usage = crate::dispatcher::cos_chat::rollover::session_usage(harness, &outcome);
        assert_eq!(usage.billed_input, 10, "{harness}");
        assert_eq!(usage.add_tokens, 30, "{harness}");
        assert_eq!(usage.context_tokens, Some(16), "{harness}");
    }
}

fn thread_count(dir: &Path) -> i64 {
    rusqlite::Connection::open(dir.join("celeris.db"))
        .expect("db")
        .query_row("SELECT COUNT(*) FROM chat_threads", [], |r| r.get(0))
        .expect("count")
}

/// ADR 2026-10-22-cos-fixed-threads D4: the CoS thread's harness session lives in SQLite, so a
/// new daemon (new store handle and dispatcher, nothing kept in memory) resumes the same session
/// id in the same thread.
#[tokio::test]
async fn cos_chat_session_survives_daemon_restart_in_the_same_thread() {
    let (dir, store) = open_store();
    let t = thread(&store);
    let live = {
        let (_probe, adapter) = refusing(&store, usize::MAX, false);
        let mut d = fixture(adapter, store.clone(), dir.path());
        post(&store, &t, "one");
        tick_and_join(&mut d, &store, &t).await;
        store
            .chat_session_active(&t)
            .expect("session")
            .expect("live session")
    };
    drop(store);

    // "Restart": a fresh store handle on the same DB file and a fresh dispatcher.
    let store = Arc::new(SqliteStore::open(&dir.path().join("celeris.db")).expect("reopen"));
    let (probe, adapter) = refusing(&store, usize::MAX, false);
    let mut d = fixture(adapter, store.clone(), dir.path());
    post(&store, &t, "two");
    let run = tick_and_join(&mut d, &store, &t).await;
    let attempts = probe.attempts.lock().expect("attempts").clone();
    assert_eq!(attempts.len(), 1);
    let session = attempts[0].context.session.as_ref().expect("session");
    assert!(session.resume, "the restarted daemon resumes");
    assert_eq!(session.session_id, live.session_id);
    let record = store.chat_run_session_record(&run).expect("record");
    assert_eq!(record.session_row_id.as_deref(), Some(live.id.as_str()));
    let after = store
        .chat_session_active(&t)
        .expect("session")
        .expect("live");
    assert_eq!(after.id, live.id);
    assert_eq!(thread_count(dir.path()), 1, "no new thread");
}

/// D4: an expired session (resume refused) after a restart restarts in the same thread with a
/// fresh session that gets the thread summary and the unsummarized history.
#[tokio::test]
async fn cos_chat_session_expiry_after_restart_starts_fresh_from_summary_in_the_same_thread() {
    let (dir, store) = open_store();
    let t = thread(&store);
    {
        let (_probe, adapter) = refusing(&store, usize::MAX, false);
        let mut d = fixture(adapter, store.clone(), dir.path());
        post(&store, &t, "one");
        tick_and_join(&mut d, &store, &t).await;
    }
    let old = store
        .chat_session_active(&t)
        .expect("session")
        .expect("live");
    drop(store);

    let store = Arc::new(SqliteStore::open(&dir.path().join("celeris.db")).expect("reopen"));
    // Every resumed attempt is refused: the session has expired on the harness side. The worker
    // checkpoints a summary ("summary-A" through seq 2) before failing, like a real CoS run.
    let (probe, adapter) = refusing(&store, 0, false);
    let mut d = fixture(adapter, store.clone(), dir.path());
    post(&store, &t, "two");
    let run = tick_and_join(&mut d, &store, &t).await;
    let attempts = probe.attempts.lock().expect("attempts").clone();
    assert_eq!(attempts.len(), 2, "refused resume, then one fresh attempt");
    assert!(attempts[0].context.session.as_ref().expect("s").resume);
    let fresh = &attempts[1];
    assert!(!fresh.context.session.as_ref().expect("s").resume);
    let chat = fresh.context.cos_chat.as_ref().expect("cos chat");
    assert_eq!(chat.thread_id, t);
    assert_eq!(chat.summary.as_deref(), Some("summary-A"));
    assert_eq!(chat.summary_through_seq, 2);
    assert_eq!(
        store.chat_run_get(&t, &run).expect("run").state,
        ChatRunState::Completed
    );
    let live = store
        .chat_session_active(&t)
        .expect("session")
        .expect("live");
    assert_ne!(live.id, old.id);
    assert_eq!(live.key.thread_id, t);
    assert_eq!(thread_count(dir.path()), 1, "no new thread");
}

#[tokio::test]
async fn cos_chat_launch_delivers_stored_relay_evidence_to_later_runs() {
    let (dir, store) = open_store();
    let (probe, adapter) = refusing(&store, usize::MAX, false);
    let mut d = fixture(adapter, store.clone(), dir.path());
    let t = thread(&store);
    let now = OffsetDateTime::now_utc();
    let posted = store
        .chat_relay_post(
            &t,
            &task_core::chat::ChatRelayRequest {
                client_message_id: "relayed".into(),
                text: "この案件は進めて".into(),
                said_at: (now - time::Duration::minutes(1))
                    .format(&time::format_description::well_known::Rfc3339)
                    .expect("time"),
                task_ids: vec![],
                expires_at: None,
            },
            "operator:test",
            now,
        )
        .expect("relay");
    tick_and_join(&mut d, &store, &t).await;
    post(&store, &t, "later");
    tick_and_join(&mut d, &store, &t).await;
    let attempts = probe.attempts.lock().expect("attempts");
    let chat = attempts
        .last()
        .expect("later run")
        .context
        .cos_chat
        .as_ref()
        .expect("chat");
    assert_ne!(chat.inputs[0].id, posted.response.message.id);
    let relay = &chat.relayed_instructions[0];
    assert_eq!(relay.message_id, posted.response.message.id);
    assert_eq!(relay.text, "この案件は進めて");
    assert_eq!(relay.relayed_by, "operator:test");
}

/// ADR 2026-10-28-cos-session-policy-key D1/D2: a change of `[cos.triage] policy_version` or of a
/// mounted skill's content makes the next run fresh (`policy_changed`, summary + unsummarized
/// history instead of the old harness session), and the run gets and records the new version.
/// An unchanged policy resumes. Deterministic: worker joins, no sleeps.
#[tokio::test]
async fn cos_chat_policy_change_makes_next_run_fresh_and_records_new_version() {
    let (dir, store) = open_store();
    let (probe, adapter) = refusing(&store, usize::MAX, false);
    let mut d = fixture(adapter, store.clone(), dir.path());
    let t = thread(&store);
    let last = |probe: &RefusingFake| {
        probe
            .attempts
            .lock()
            .expect("attempts")
            .last()
            .cloned()
            .expect("attempt")
    };

    post(&store, &t, "one");
    let first = tick_and_join(&mut d, &store, &t).await;
    assert_eq!(
        store.chat_run_policy_version(&first).expect("policy"),
        Some("1".into()),
        "the fixture's CosTriageSettings::default() version"
    );
    let live = store.chat_session_active(&t).expect("s").expect("live");
    assert!(
        live.key
            .policy_key
            .as_deref()
            .is_some_and(|k| k.starts_with("policy_version=1;cos-inbox-triage=")),
        "{:?}",
        live.key.policy_key
    );
    post(&store, &t, "two");
    let second = tick_and_join(&mut d, &store, &t).await;
    let record = store.chat_run_session_record(&second).expect("record");
    assert_eq!(
        record.detail.as_deref(),
        Some("resumed"),
        "same policy resumes"
    );

    // The operator raises the version and reloads.
    let mut cfg = d.cos_chat_launch.as_ref().expect("launch").config.clone();
    cfg.triage.policy_version = "7".into();
    d.reload_cos_chat_launch(cfg);
    post(&store, &t, "three");
    let third = tick_and_join(&mut d, &store, &t).await;
    let record = store.chat_run_session_record(&third).expect("record");
    assert_eq!(record.detail.as_deref(), Some("fresh"));
    assert_eq!(record.reason.as_deref(), Some("policy_changed"));
    assert_ne!(record.session_row_id.as_deref(), Some(live.id.as_str()));
    assert_eq!(
        store.chat_run_policy_version(&third).expect("policy"),
        Some("7".into())
    );
    let attempt = last(&probe);
    assert!(!attempt.context.session.as_ref().expect("session").resume);
    let chat = attempt.context.cos_chat.as_ref().expect("chat");
    assert_eq!(chat.policy_version, "7");
    let texts: Vec<_> = chat
        .unsummarized
        .messages
        .iter()
        .map(|m| m.text.as_str())
        .collect();
    assert!(
        texts.contains(&"one") && texts.contains(&"two"),
        "a fresh run gets the unsummarized history: {texts:?}"
    );

    // The KB skill is replaced (same version string): fresh again.
    let fresh = store.chat_session_active(&t).expect("s").expect("live");
    std::fs::write(
        d.config
            .knowledge
            .root
            .join("skills/cos-inbox-triage/SKILL.md"),
        "---\nname: cos-inbox-triage\ndescription: test skill\n---\n# v8 body\n",
    )
    .expect("skill");
    post(&store, &t, "four");
    let fourth = tick_and_join(&mut d, &store, &t).await;
    let record = store.chat_run_session_record(&fourth).expect("record");
    assert_eq!(record.reason.as_deref(), Some("policy_changed"));
    assert_ne!(record.session_row_id.as_deref(), Some(fresh.id.as_str()));
    assert_eq!(
        store.chat_run_policy_version(&fourth).expect("policy"),
        Some("7".into())
    );

    post(&store, &t, "five");
    let fifth = tick_and_join(&mut d, &store, &t).await;
    let record = store.chat_run_session_record(&fifth).expect("record");
    assert_eq!(record.detail.as_deref(), Some("resumed"));
}

/// D1: a session row written before migration 0100 (NULL `policy_key`) is never resumed.
#[tokio::test]
async fn cos_chat_session_without_policy_key_is_retired_as_policy_changed() {
    let (dir, store) = open_store();
    let (_probe, adapter) = refusing(&store, usize::MAX, false);
    let mut d = fixture(adapter, store.clone(), dir.path());
    let t = thread(&store);
    post(&store, &t, "one");
    tick_and_join(&mut d, &store, &t).await;
    let live = store.chat_session_active(&t).expect("s").expect("live");
    rusqlite::Connection::open(dir.path().join("celeris.db"))
        .expect("db")
        .execute(
            "UPDATE node_sessions SET policy_key=NULL WHERE id=?1",
            [&live.id],
        )
        .expect("legacy row");
    post(&store, &t, "two");
    let second = tick_and_join(&mut d, &store, &t).await;
    let record = store.chat_run_session_record(&second).expect("record");
    assert_eq!(record.reason.as_deref(), Some("policy_changed"));
}
