//! Route failover with in-process fake adapters, explicit joins and injected clocks.
use super::*;
use crate::accounts::{AccountCooldownReason, ObservationSource};
use crate::dispatcher::cos_chat::launch::CosChatRoute;
use task_core::chat::{ChatMessageRole, ChatSessionMode};
use task_core::{AccountAdapter, RateLimitObservation, RateWindow};

type Attempts = Arc<StdMutex<Vec<(String, RunRequest, String, Duration)>>>;

#[derive(Clone)]
struct RouteFake {
    harness: &'static str,
    failure: Option<&'static str>,
    attempts: Attempts,
    token: String,
    store: Arc<SqliteStore>,
}

#[async_trait]
impl WorkerAdapter for RouteFake {
    fn id(&self) -> &str {
        self.harness
    }
    fn with_env(&self, env: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        let token = env
            .iter()
            .find(|(key, _)| key == "CELERIS_COS_RUN_CREDENTIAL")
            .map(|(_, value)| value.clone())
            .unwrap_or_default();
        Some(Arc::new(Self {
            token,
            ..self.clone()
        }))
    }
    fn with_model(&self, _: &str) -> Option<Arc<dyn WorkerAdapter>> {
        Some(Arc::new(self.clone()))
    }
    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let token = self.token.strip_prefix("celeris-cos-run.").expect("token");
        let identity = self
            .store
            .cos_run_credential_verify(token, OffsetDateTime::now_utc())
            .expect("valid credential on every route");
        assert_eq!(identity.run_id, run_id);
        self.attempts.lock().unwrap().push((
            self.harness.into(),
            req,
            self.token.clone(),
            limits.wall_clock,
        ));
        sink.session_established(&format!("{}-session", self.harness));
        Ok(RunOutcome {
            terminal: match self.failure {
                Some(message) => Terminal::Error {
                    message: message.into(),
                    retryable: false,
                },
                None => Terminal::Done {
                    summary: "代替からの返事".into(),
                    evidence: vec![],
                    usage: None,
                },
            },
            exit_code: Some(if self.failure.is_some() { 1 } else { 0 }),
        })
    }
}

fn setup(d: &mut Dispatcher, store: &Arc<SqliteStore>, failure: Option<&'static str>) -> Attempts {
    let attempts = Attempts::default();
    for (provider, harness, failure) in [
        ("p1", "claude-code", failure),
        ("p2", "codex", None),
        ("p3", "acp", None),
    ] {
        d.adapters.insert(
            provider.into(),
            Arc::new(RouteFake {
                harness,
                failure,
                attempts: attempts.clone(),
                token: String::new(),
                store: store.clone(),
            }),
        );
    }
    d.policy = Box::new(StaticPolicy::new(
        [("p1", "claude-code"), ("p2", "codex"), ("p3", "acp")]
            .into_iter()
            .map(|(id, adapter)| ProviderSpec {
                id: id.into(),
                adapter: adapter.into(),
                tiers: vec![Tier::Frontier],
                concurrency: 2,
                model: "m".into(),
            })
            .collect(),
        Duration::from_secs(1),
    ));
    let cfg = &mut d.cos_chat_launch.as_mut().unwrap().config;
    cfg.harness = "claude-code".into();
    cfg.llm_source = Some("claude_oauth".into());
    cfg.fallbacks = vec![
        CosChatRoute {
            harness: "codex".into(),
            llm_source: Some("codex_oauth".into()),
            provider: Some("p2".into()),
            account_id: None,
            model: Some("gpt-6.1-sol".into()),
            tier: Tier::Frontier,
            unavailable_reason: None,
        },
        CosChatRoute {
            harness: "acp".into(),
            llm_source: Some("openai_compatible:opencode_go".into()),
            provider: Some("p3".into()),
            account_id: None,
            model: None,
            tier: Tier::Frontier,
            unavailable_reason: None,
        },
    ];
    attempts
}

fn pool(d: &mut Dispatcher, root: &std::path::Path) {
    for id in ["a", "b"] {
        std::fs::create_dir_all(root.join(id)).unwrap();
        std::fs::write(root.join(id).join(".credentials.json"), "{}").unwrap();
    }
    d.config.accounts = Some(AccountsRuntimeConfig {
        roots: HashMap::from([(AccountAdapter::ClaudeCode, root.to_path_buf())]),
        max_runs_per_account: 1,
        check_model: "test".into(),
        fallback_cooldown_secs: 60,
    });
    d.account_books.insert(
        AccountAdapter::ClaudeCode,
        Arc::new(StdMutex::new(crate::accounts::AccountBook::load(
            &root.join(".celeris-usage.json"),
        ))),
    );
    d.account_pool_providers.insert("p1".into());
}

fn observe(d: &mut Dispatcher, id: &str, utilization: f64, now: i64, reset: i64) {
    d.account_book(AccountAdapter::ClaudeCode)
        .unwrap()
        .lock()
        .unwrap()
        .record_observation(
            id,
            RateLimitObservation {
                observed_at: now,
                five_hour: Some(RateWindow {
                    utilization,
                    resets_at: reset,
                }),
                seven_day: None,
                one_month: None,
                status: None,
                resets_at: None,
            },
            ObservationSource::Check,
        );
}

fn messages(store: &SqliteStore, thread: &str) -> Vec<task_core::chat::ChatMessage> {
    store
        .chat_message_list(
            thread,
            &ChatMessageQuery {
                limit: Some(200),
                ..Default::default()
            },
        )
        .unwrap()
        .items
}

#[tokio::test]
async fn cos_source_fallback_unavailable_accounts_select_next_route() {
    for reason in ["quota", "cooldown", "rejected", "logout"] {
        let (dir, store, mut d) = fixture(FakeAdapter::default_command(), 2);
        let attempts = setup(&mut d, &store, None);
        let root = dir.path().join("accounts");
        pool(&mut d, &root);
        let now = (d.now_unix_fn)();
        for id in ["a", "b"] {
            match reason {
                "quota" => observe(&mut d, id, 1.0, now, now + 300),
                "cooldown" => d
                    .account_book(AccountAdapter::ClaudeCode)
                    .unwrap()
                    .lock()
                    .unwrap()
                    .set_cooldown(
                        id,
                        crate::accounts::AccountCooldown {
                            until: now + 300,
                            reason: AccountCooldownReason::AuthFailed,
                        },
                        now,
                    ),
                "rejected" => d
                    .account_book(AccountAdapter::ClaudeCode)
                    .unwrap()
                    .lock()
                    .unwrap()
                    .record_observation(
                        id,
                        RateLimitObservation {
                            observed_at: now,
                            five_hour: None,
                            seven_day: None,
                            one_month: None,
                            status: Some("rejected".into()),
                            resets_at: Some(now + 300),
                        },
                        ObservationSource::Check,
                    ),
                _ => std::fs::remove_file(root.join(id).join(".credentials.json")).unwrap(),
            }
        }
        let t = thread(&store, reason);
        post(&store, &t, "question");
        d.tick_cos_chat_launch();
        let run = active_run(&store, &t);
        join(&mut d, &t).await;
        let run = store.chat_run_get(&t, &run.id).unwrap();
        assert_eq!(run.state, ChatRunState::Completed, "{reason}");
        assert_eq!(run.harness.as_deref(), Some("codex"));
        assert_eq!(run.llm_source.as_deref(), Some("codex_oauth"));
        assert_eq!(run.model.as_deref(), Some("gpt-6.1-sol"));
        assert_eq!(attempts.lock().unwrap().len(), 1);
        let msgs = messages(&store, &t);
        assert!(msgs.iter().any(|m| m.text.contains("次の代替経路で再試行")));
        assert!(msgs.iter().any(|m| m.text.contains("CoS 実行経路: codex")));
        assert_eq!(
            msgs.iter()
                .filter(|m| m.role == ChatMessageRole::User)
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn cos_source_fallback_limit_response_retries_same_run_and_fresh_session() {
    let (dir, store, mut d) = fixture(FakeAdapter::default_command(), 2);
    let attempts = setup(&mut d, &store, Some("You've hit your session limit"));
    let t = thread(&store, "limit");
    post(&store, &t, "original");
    d.tick_cos_chat_launch();
    let first = active_run(&store, &t);
    join(&mut d, &t).await;
    assert_eq!(
        store.chat_run_get(&t, &first.id).unwrap().state,
        ChatRunState::Running
    );
    let old_session = store.chat_session_active(&t).unwrap().unwrap();
    // Move the injected wall clock; next route must receive only the remaining budget.
    let started = OffsetDateTime::parse(
        first.started_at.as_deref().unwrap(),
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    d.test_now = Some(Arc::new(StdMutex::new(
        started + time::Duration::seconds(7),
    )));
    d.tick_cos_chat_launch();
    let second = active_run(&store, &t);
    assert_eq!(second.id, first.id);
    assert_eq!(second.input_message_id, first.input_message_id);
    assert_eq!(second.output_message_id, first.output_message_id);
    assert_eq!(second.session_mode, Some(ChatSessionMode::Fresh));
    join(&mut d, &t).await;
    let finished = store.chat_run_get(&t, &first.id).unwrap();
    assert_eq!(finished.state, ChatRunState::Completed);
    let attempts = attempts.lock().unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].0, "claude-code");
    assert_eq!(attempts[1].0, "codex");
    assert_eq!(attempts[1].3.as_secs(), 23);
    let resumed = attempts[1].1.context.session.as_ref().unwrap();
    assert!(!resumed.resume);
    assert!(resumed.session_id.is_empty());
    assert_ne!(
        old_session.id,
        store.chat_session_active(&t).unwrap().unwrap().id
    );
    assert_eq!(
        attempts[1].1.context.cos_chat.as_ref().unwrap().inputs[0].text,
        "original"
    );
    assert_ne!(attempts[0].2, attempts[1].2);
    for (_, _, token, _) in attempts.iter() {
        assert!(
            store
                .cos_run_credential_verify(
                    token.strip_prefix("celeris-cos-run.").unwrap(),
                    d.now_utc()
                )
                .is_err()
        );
    }
    let msgs = messages(&store, &t);
    assert!(msgs.iter().any(|m| m.text.contains("次の代替経路で再試行")));
    assert_eq!(
        msgs.iter()
            .find(|m| Some(&m.id) == first.output_message_id.as_ref())
            .unwrap()
            .text,
        "代替からの返事"
    );
    assert_eq!(runs(&store, &t).len(), 1);
    // Persisted route events carry both attempts; latest GET run carries the effective route.
    let events = store
        .chat_events_page(
            &t,
            &ChatEventQuery {
                limit: Some(200),
                ..Default::default()
            },
        )
        .unwrap();
    for harness in ["claude-code", "codex"] {
        assert!(events.items.iter().any(|e| matches!(&e.data, ChatEventData::Run(r) if r.run.harness.as_deref() == Some(harness))));
    }
    drop(dir);
}

#[tokio::test]
async fn cos_source_fallback_failure_has_reason_next_action_and_is_bounded() {
    for runtime_failure in [false, true] {
        let (_dir, store, mut d) = fixture(FakeAdapter::default_command(), 2);
        let attempts = setup(&mut d, &store, Some("You've hit your session limit"));
        let cfg = &mut d.cos_chat_launch.as_mut().unwrap().config;
        cfg.fallbacks.clear();
        if !runtime_failure {
            cfg.unavailable_reason = Some("CoS unavailable: no usable account".into());
        }
        let t = thread(&store, "no-route");
        post(&store, &t, "question");
        d.tick_cos_chat_launch();
        if runtime_failure {
            join(&mut d, &t).await;
            d.tick_cos_chat_launch();
        }
        let run = runs(&store, &t).pop().unwrap();
        assert_eq!(run.state, ChatRunState::Failed);
        let msgs = messages(&store, &t);
        let reply = msgs
            .iter()
            .find(|m| Some(&m.id) == run.output_message_id.as_ref())
            .unwrap();
        assert!(reply.text.contains("理由:"));
        assert!(reply.text.contains("再送してください"));
        d.tick_cos_chat_launch();
        assert_eq!(runs(&store, &t).len(), 1);
        assert_eq!(attempts.lock().unwrap().len(), usize::from(runtime_failure));
    }
}

#[tokio::test]
async fn cos_source_fallback_stop_and_non_supply_error_do_not_retry() {
    for stop in [false, true] {
        let (_dir, store, mut d) = fixture(FakeAdapter::default_command(), 2);
        let attempts = setup(
            &mut d,
            &store,
            Some(if stop {
                "usage limit"
            } else {
                "malformed result"
            }),
        );
        let t = thread(&store, "no-retry");
        post(&store, &t, "question");
        d.tick_cos_chat_launch();
        let run = active_run(&store, &t);
        join(&mut d, &t).await;
        if stop {
            store.chat_run_stop(&t, &run.id, d.now_utc()).unwrap();
        }
        d.tick_cos_chat_launch();
        assert_eq!(
            store.chat_run_get(&t, &run.id).unwrap().state,
            if stop {
                ChatRunState::Stopped
            } else {
                ChatRunState::Failed
            }
        );
        assert_eq!(attempts.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn cos_source_fallback_order_skips_unavailable_second_route() {
    let (_dir, store, mut d) = fixture(FakeAdapter::default_command(), 2);
    let attempts = setup(&mut d, &store, Some("usage limit"));
    d.cos_chat_launch.as_mut().unwrap().config.fallbacks[0].unavailable_reason =
        Some("not logged in".into());
    let t = thread(&store, "third");
    post(&store, &t, "question");
    d.tick_cos_chat_launch();
    join(&mut d, &t).await;
    d.tick_cos_chat_launch();
    join(&mut d, &t).await;
    let run = runs(&store, &t).pop().unwrap();
    assert_eq!(run.state, ChatRunState::Completed);
    assert_eq!(run.harness.as_deref(), Some("acp"));
    assert_eq!(
        attempts
            .lock()
            .unwrap()
            .iter()
            .map(|a| a.0.as_str())
            .collect::<Vec<_>>(),
        vec!["claude-code", "acp"]
    );
}

#[test]
fn cos_source_fallback_worker_reserve_covers_picks_sticky_reset_and_opt_out() {
    let (dir, store, mut d) = fixture(FakeAdapter::default_command(), 2);
    setup(&mut d, &store, None);
    pool(&mut d, &dir.path().join("accounts"));
    let now = (d.now_unix_fn)();
    for id in ["a", "b"] {
        observe(&mut d, id, 0.90, now, now + 300);
    }
    assert_eq!(
        d.pick_account(AccountAdapter::ClaudeCode, None, false),
        None
    );
    assert!(!d.account_usable(AccountAdapter::ClaudeCode, "a", false));
    assert_eq!(
        d.pick_account(AccountAdapter::ClaudeCode, None, true)
            .as_deref(),
        Some("a")
    );
    assert!(d.account_usable(AccountAdapter::ClaudeCode, "a", true));
    observe(&mut d, "a", 0.899, now, now + 300);
    assert_eq!(
        d.pick_account(AccountAdapter::ClaudeCode, None, false)
            .as_deref(),
        Some("a")
    );
    observe(&mut d, "a", 0.90, now, now);
    assert!(
        d.account_usable(AccountAdapter::ClaudeCode, "a", false),
        "reset releases the reservation"
    );
    observe(&mut d, "a", 0.90, now, now + 300);
    d.cos_chat_launch.as_mut().unwrap().config.enabled = false;
    assert!(d.account_usable(AccountAdapter::ClaudeCode, "a", false));
    d.cos_chat_launch.as_mut().unwrap().config.enabled = true;
    observe(&mut d, "a", 0.97, now, now + 300);
    assert!(
        !d.account_usable(AccountAdapter::ClaudeCode, "a", true),
        "CoS cannot bypass quota"
    );
}

#[tokio::test]
async fn cos_source_fallback_pending_operation_blocks_retry() {
    let (dir, store, mut d) = fixture(FakeAdapter::default_command(), 2);
    let attempts = setup(&mut d, &store, Some("usage limit"));
    let t = thread(&store, "pending-fallback");
    post(&store, &t, "question");
    d.tick_cos_chat_launch();
    let run = active_run(&store, &t);
    // The spawned adapter has not been polled; no sleep or scheduling race.
    let conn = rusqlite::Connection::open(dir.path().join("celeris.db")).unwrap();
    conn.execute("INSERT INTO cos_operations(id,thread_id,run_id,idempotency_key,request_hash,\
        target_kind,target_id,action,payload_json,reason,policy_version,state,created_at,updated_at)\
        VALUES('pending-op',?1,?2,'key','hash','task','target','update','{}','why','1','pending',?3,?3)",
        rusqlite::params![t, run.id, d.now_utc().to_string()]).unwrap();
    join(&mut d, &t).await;
    d.tick_cos_chat_launch();
    assert_eq!(
        store.chat_run_get(&t, &run.id).unwrap().state,
        ChatRunState::Interrupted
    );
    assert!(store.chat_thread_get(&t).unwrap().unwrap().queue_paused);
    assert_eq!(attempts.lock().unwrap().len(), 1);
    assert!(messages(&store, &t).iter().any(|m| !m.cards.is_empty()));
}

#[tokio::test]
async fn cos_source_fallback_preserves_summary_history_and_attachments() {
    let (dir, store, mut d) = fixture(FakeAdapter::default_command(), 2);
    setup(&mut d, &store, None);
    let t = thread(&store, "context");
    post(&store, &t, "earlier input");
    d.tick_cos_chat_launch();
    let earlier = active_run(&store, &t);
    store
        .chat_thread_checkpoint_at(&t, &earlier.id, "saved summary", 1, 0, d.now_utc())
        .unwrap();
    join(&mut d, &t).await;
    let attempts = setup(&mut d, &store, Some("usage limit"));
    let attachments = ChatAttachmentStore::open(
        dir.path(),
        &dir.path().join("celeris.db"),
        Default::default(),
    )
    .unwrap();
    let attachment = attachments
        .upload(
            &t,
            "upload",
            "notes.txt",
            None,
            std::io::Cursor::new(b"attached notes"),
            d.now_utc(),
        )
        .unwrap();
    store
        .chat_message_post(
            &t,
            &ChatPostMessageRequest {
                client_message_id: "attached".into(),
                text: "continue".into(),
                attachment_ids: vec![attachment.id.clone()],
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
            },
            d.now_utc(),
        )
        .unwrap();
    d.tick_cos_chat_launch();
    join(&mut d, &t).await;
    d.tick_cos_chat_launch();
    join(&mut d, &t).await;
    let attempts = attempts.lock().unwrap();
    assert_eq!(attempts.len(), 2);
    let first = attempts[0].1.context.cos_chat.as_ref().unwrap();
    let next = attempts[1].1.context.cos_chat.as_ref().unwrap();
    assert_eq!(next.summary.as_deref(), Some("saved summary"));
    assert_eq!(
        serde_json::to_value(&next.unsummarized).unwrap(),
        serde_json::to_value(&first.unsummarized).unwrap()
    );
    assert_eq!(next.inputs[0].attachment_ids, vec![attachment.id.clone()]);
    assert_eq!(next.attachments[0].id, attachment.id);
    assert_eq!(
        std::fs::read(&next.attachments[0].path).unwrap(),
        b"attached notes"
    );
    assert!(!attempts[1].1.context.session.as_ref().unwrap().resume);
}

#[tokio::test]
async fn cos_source_fallback_accounts_are_counted_by_actual_adapter() {
    let (dir, store, mut d) = fixture(FakeAdapter::default_command(), 2);
    setup(&mut d, &store, None);
    pool(&mut d, &dir.path().join("claude"));
    let codex = dir.path().join("codex");
    std::fs::create_dir_all(codex.join("a")).unwrap();
    std::fs::write(codex.join("a/auth.json"), "{}").unwrap();
    let accounts = d.config.accounts.as_mut().unwrap();
    accounts.roots.insert(AccountAdapter::Codex, codex.clone());
    accounts.max_runs_per_account = 0; // CoS has exactly one extra slot.
    d.account_books.insert(
        AccountAdapter::Codex,
        Arc::new(StdMutex::new(crate::accounts::AccountBook::load(
            &codex.join(".usage.json"),
        ))),
    );
    d.account_pool_providers.insert("p2".into());
    d.cos_chat_launch
        .as_mut()
        .unwrap()
        .config
        .unavailable_reason = Some("primary unavailable".into());
    let a = thread(&store, "first-account");
    let b = thread(&store, "second-account");
    post(&store, &a, "one");
    post(&store, &b, "two");
    d.tick_cos_chat_launch();
    assert_eq!(d.account_in_use(AccountAdapter::Codex, "a"), 1);
    assert_eq!(d.account_in_use(AccountAdapter::ClaudeCode, "a"), 0);
    let mut harnesses = vec![
        active_run(&store, &a).harness.unwrap(),
        active_run(&store, &b).harness.unwrap(),
    ];
    harnesses.sort();
    assert_eq!(harnesses, vec!["acp", "codex"]);
    join(&mut d, &a).await;
    join(&mut d, &b).await;
}
