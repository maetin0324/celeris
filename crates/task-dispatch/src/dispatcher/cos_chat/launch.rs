//! Start CoS chat runs from the durable per-thread queue.
//!
//! Only this dispatcher path may grant the ADR-0089 CoS capacity exception. The
//! transient Task is never inserted into the task store.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use task_core::SqliteStore;
use task_core::chat::{
    ChatMessage, ChatMessageQuery, ChatRunSessionMode, ChatRunState, ChatSession, ChatSessionKey,
    ChatStatusPhase, ChatThreadQuery,
};
use task_worker::adapter::{RunLimits, WorkerAdapter};
use task_worker::protocol::{
    COS_RUN_CREDENTIAL_ENV, CosChatAttachment, CosChatContext, CosChatDelivery, CosChatHistory,
    CosChatHistoryMessage, CosChatInput, RunContext, RunRequest, SessionHandle,
};
use time::OffsetDateTime;

use super::attachments::{stage_message_attachments, workspace_dir};
use super::sink::{ChatClock, ChatRunSink, ChatStopIntent, chat_finish_for};
use crate::dispatcher::Dispatcher;

/// `task-api::cos::COS_BEARER_PREFIX` wire form. The store issues only the
/// random secret; the worker must present the prefixed bearer to the API.
const COS_RUN_BEARER_PREFIX: &str = "celeris-cos-run.";

/// Values resolved from `[cos]` by the daemon, before the dispatcher starts.
#[derive(Debug, Clone)]
pub struct CosChatLaunchConfig {
    pub enabled: bool,
    pub harness: String,
    pub llm_source: Option<String>,
    pub provider: Option<String>,
    pub account_id: Option<String>,
    pub model: Option<String>,
    pub tier: task_core::Tier,
    pub max_turns: u32,
    pub max_wall_secs: u64,
    pub unavailable_reason: Option<String>,
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub attachment_limits: task_core::chat::attachments::ChatAttachmentLimits,
    pub api_base_url: String,
}

pub(crate) struct CosChatLaunch {
    pub store: Arc<SqliteStore>,
    pub config: CosChatLaunchConfig,
    pub running: HashMap<String, tokio::task::JoinHandle<()>>,
    pub(crate) accounts_in_flight: HashMap<String, String>,
    pub(crate) providers_in_flight: HashMap<String, String>,
}

impl Dispatcher {
    /// Install the concrete chat store. It must be the same database as `TaskStore`.
    pub fn set_cos_chat_launch(&mut self, store: Arc<SqliteStore>, config: CosChatLaunchConfig) {
        self.cos_chat_launch = Some(CosChatLaunch {
            store,
            config,
            running: HashMap::new(),
            accounts_in_flight: HashMap::new(),
            providers_in_flight: HashMap::new(),
        });
    }

    /// One non-blocking tick: each eligible thread may start at most one run.
    pub(crate) fn tick_cos_chat_launch(&mut self) {
        let Some(mut launch) = self.cos_chat_launch.take() else {
            return;
        };
        launch.running.retain(|_, handle| !handle.is_finished());
        launch
            .accounts_in_flight
            .retain(|thread, _| launch.running.contains_key(thread));
        launch
            .providers_in_flight
            .retain(|thread, _| launch.running.contains_key(thread));
        // The later control leaf owns stop/interrupt/restart reconciliation. Its
        // hook runs before a new claim so an old process cannot overlap a new one.
        launch.control_hook();
        if self.accepting_new_work
            && self.disk_ready
            && launch.config.enabled
            && let Err(error) = launch.claim_queued(self)
        {
            tracing::warn!(%error, "CoS chat launch tick failed");
        }
        self.cos_chat_launch = Some(launch);
    }
}

impl CosChatLaunch {
    /// Control leaf hook: stop, interrupt and orphan confirmation run before claiming.
    fn control_hook(&mut self) {}

    /// Rollover leaf hook: inspect the sink's session signals before finalising.
    fn rollover_hook(
        _signals: &super::sink::ChatSessionSignals,
        _finish: &super::sink::ChatFinish,
    ) {
    }
    fn claim_queued(&mut self, dispatcher: &mut Dispatcher) -> Result<(), String> {
        let mut before = None;
        loop {
            let page = self
                .store
                .chat_thread_list(&ChatThreadQuery {
                    before: before.clone(),
                    limit: Some(100),
                    ..ChatThreadQuery::default()
                })
                .map_err(|e| e.to_string())?;
            for thread in &page.items {
                if thread.queued_count == 0 || thread.active_run_id.is_some() {
                    continue;
                }
                if self.running.contains_key(&thread.id) {
                    continue;
                }
                let in_flight =
                    self.running.len() + dispatcher.running.values().filter(|run| run.cos).count();
                let capacity = if dispatcher.config.execution.max_cos_runs == 0 {
                    dispatcher.workers_in_flight() + in_flight < dispatcher.config.max_concurrency
                } else {
                    in_flight < dispatcher.config.execution.max_cos_runs
                };
                if !capacity {
                    return Ok(());
                }
                self.start_thread(dispatcher, &thread.id)?;
            }
            before = page.next_cursor;
            if before.is_none() {
                break;
            }
        }
        Ok(())
    }

    fn start_thread(&mut self, dispatcher: &mut Dispatcher, thread_id: &str) -> Result<(), String> {
        let now = dispatcher.now_utc();
        let cfg = &self.config;
        let account = if let Some(provider) = cfg.provider.as_deref() {
            if dispatcher.account_pool_providers.contains(provider) {
                let Some(account_adapter) = task_core::AccountAdapter::parse(&cfg.harness) else {
                    return Err(format!("CoS account pool does not support {}", cfg.harness));
                };
                let existing = self
                    .store
                    .chat_session_active(thread_id)
                    .map_err(|e| e.to_string())?;
                let sticky = existing
                    .as_ref()
                    .filter(|session| {
                        session.key.provider.as_deref() == Some(provider)
                            && session.key.harness == cfg.harness
                    })
                    .and_then(|session| session.key.account_id.as_deref());
                let available = |dispatcher: &mut Dispatcher, id: &str| {
                    let chat_count = self
                        .accounts_in_flight
                        .values()
                        .filter(|account| account.as_str() == id)
                        .count();
                    let limit = crate::capacity::account_run_limit(
                        dispatcher
                            .config
                            .accounts
                            .as_ref()
                            .map_or(1, |accounts| accounts.max_runs_per_account),
                        dispatcher.config.execution.max_cos_runs > 0,
                    );
                    dispatcher.account_in_use(account_adapter, id) + chat_count < limit
                        && dispatcher.account_usable(
                            account_adapter,
                            id,
                            dispatcher.config.execution.max_cos_runs > 0,
                        )
                };
                if let Some(id) = cfg.account_id.as_deref() {
                    if !available(dispatcher, id) {
                        // A pinned account must not silently fall back to another one.
                        return Ok(());
                    }
                    Some(id.to_owned())
                } else if let Some(id) = sticky.filter(|id| available(dispatcher, id)) {
                    Some(id.to_owned())
                } else {
                    let dirs = dispatcher
                        .config
                        .accounts
                        .as_ref()
                        .and_then(|accounts| accounts.root_for(account_adapter))
                        .map(|root| crate::accounts::scan_accounts(root, account_adapter))
                        .unwrap_or_default();
                    let candidates: Vec<_> = dirs
                        .iter()
                        .map(|dir| crate::accounts::AccountCandidate {
                            id: &dir.id,
                            logged_in: dir.logged_in,
                            in_use: dispatcher.account_in_use(account_adapter, &dir.id)
                                + self
                                    .accounts_in_flight
                                    .values()
                                    .filter(|account| *account == &dir.id)
                                    .count(),
                        })
                        .collect();
                    let limit = crate::capacity::account_run_limit(
                        dispatcher
                            .config
                            .accounts
                            .as_ref()
                            .map_or(1, |accounts| accounts.max_runs_per_account),
                        dispatcher.config.execution.max_cos_runs > 0,
                    );
                    dispatcher.account_book(account_adapter).and_then(|book| {
                        let book = book.lock().ok()?;
                        crate::accounts::select_account_least_loaded(
                            &candidates,
                            &book,
                            limit,
                            (dispatcher.now_unix_fn)(),
                        )
                    })
                }
            } else {
                cfg.account_id.clone()
            }
        } else {
            cfg.account_id.clone()
        };
        let run_id = ulid::Ulid::new().to_string();
        let resolved = json!({
            "harness": cfg.harness,
            "llm_source": cfg.llm_source,
            "provider": cfg.provider,
            "account_id": account,
            "model": cfg.model,
            "tier": format!("{:?}", cfg.tier).to_lowercase(),
        });
        let Some(run) = self
            .store
            .chat_run_claim_next(thread_id, &run_id, &resolved, now)
            .map_err(|e| e.to_string())?
        else {
            return Ok(());
        };
        if let Err(reason) = self.launch_claimed(dispatcher, thread_id, &run_id, &run, account, now)
        {
            let reason = if reason.starts_with("CoS unavailable:") {
                reason
            } else {
                format!("CoS unavailable: {reason}")
            };
            if let Err(error) =
                self.store
                    .chat_run_status(&run_id, ChatStatusPhase::Waiting, &reason, now)
            {
                tracing::warn!(%error, %run_id, "CoS unavailable status failed");
            }
            if let Err(error) =
                self.store
                    .chat_run_finish(&run_id, ChatRunState::Failed, None, Some(&reason), now)
            {
                tracing::warn!(%error, %run_id, "CoS unavailable finish failed");
            }
        }
        Ok(())
    }

    fn launch_claimed(
        &mut self,
        dispatcher: &Dispatcher,
        thread_id: &str,
        run_id: &str,
        run: &task_core::chat::ChatRun,
        account: Option<String>,
        now: OffsetDateTime,
    ) -> Result<(), String> {
        let cfg = &self.config;
        if let Some(reason) = cfg.unavailable_reason.clone() {
            return Err(reason);
        }
        let Some(provider) = cfg.provider.as_ref() else {
            return Err("CoS unavailable: no provider candidate".into());
        };
        if dispatcher.account_pool_providers.contains(provider) && account.is_none() {
            return Err(format!(
                "CoS unavailable: no usable account for provider {provider} (quota, cooldown, or login)"
            ));
        }
        let Some(adapter) = dispatcher.adapters.get(provider).cloned() else {
            return Err(format!(
                "CoS unavailable: provider {provider} has no adapter"
            ));
        };
        if adapter.id() != cfg.harness {
            return Err(format!(
                "CoS unavailable: provider {provider} has harness {}, expected {}",
                adapter.id(),
                cfg.harness
            ));
        }
        if dispatcher
            .policy
            .cooldowns(dispatcher.monotonic_now())
            .iter()
            .any(|cooldown| &cooldown.provider == provider)
        {
            return Err(format!(
                "CoS unavailable: provider {provider} is cooling down"
            ));
        }
        let chat_provider_count = self
            .providers_in_flight
            .values()
            .filter(|active| *active == provider)
            .count();
        let provider_limit = dispatcher.policy.concurrency_limit(provider.clone());
        let exempt_pool = dispatcher.config.execution.max_cos_runs > 0
            && dispatcher.account_pool_providers.contains(provider);
        if dispatcher.provider_full(provider, dispatcher.config.execution.max_cos_runs > 0)
            || (!exempt_pool
                && dispatcher.provider_in_use(provider)
                    + dispatcher.provider_in_use_cos(provider)
                    + chat_provider_count
                    >= provider_limit)
        {
            return Err(format!(
                "CoS unavailable: provider {provider} is at capacity or cooling down"
            ));
        }
        let input = self.find_input(thread_id, &run.input_message_id)?;
        let workspace = match workspace_dir(&cfg.data_dir, thread_id) {
            Ok(path) => path,
            Err(e) => return Err(format!("CoS workspace unavailable: {e}")),
        };
        let attachments = match task_core::chat::attachments::ChatAttachmentStore::open(
            &cfg.data_dir,
            &cfg.db_path,
            cfg.attachment_limits,
        ) {
            Ok(store) => match stage_message_attachments(&store, &cfg.data_dir, &input) {
                Ok(manifest) => manifest
                    .into_iter()
                    .map(|item| CosChatAttachment {
                        id: item.id,
                        name: item.name,
                        media_type: item.media_type.clone(),
                        size_bytes: item.size_bytes,
                        sha256: item.sha256,
                        path: item.path,
                        delivery: CosChatDelivery::for_media_type(&item.media_type),
                    })
                    .collect(),
                Err(e) => return Err(format!("CoS attachment unavailable: {e}")),
            },
            Err(e) => return Err(format!("CoS attachment store unavailable: {e}")),
        };
        let session_key = ChatSessionKey {
            thread_id: thread_id.to_string(),
            harness: cfg.harness.clone(),
            provider: Some(provider.clone()),
            llm_source: cfg.llm_source.clone(),
            account_id: account.clone(),
            cwd: Some(workspace.to_string_lossy().into_owned()),
            model: cfg.model.clone(),
        };
        let existing = self
            .store
            .chat_session_active(thread_id)
            .map_err(|e| e.to_string())?;
        let (session, mode, session_reason) = if let Some(old) = existing
            .as_ref()
            .filter(|old| old.key == session_key && !old.session_id.is_empty())
        {
            (old.clone(), ChatRunSessionMode::Resumed, None)
        } else {
            let mode = if existing.is_some() {
                ChatRunSessionMode::Fresh
            } else {
                ChatRunSessionMode::New
            };
            let reason = existing.as_ref().map(|old| {
                if old.key == session_key {
                    "cache_missing"
                } else {
                    "key_changed"
                }
            });
            let next = ChatSession::new(
                session_key,
                crate::sessions::new_session_id(&cfg.harness),
                now,
            );
            self.store
                .chat_session_rotate(&next, now)
                .map_err(|e| e.to_string())?;
            (next, mode, reason)
        };
        self.store
            .chat_run_record_session(run_id, mode, &session.id, session_reason, now)
            .map_err(|e| e.to_string())?;
        let (summary, summary_through_seq) = self
            .store
            .chat_thread_summary(thread_id)
            .map_err(|e| e.to_string())?;
        let history_from = summary_through_seq.saturating_add(1);
        let history_through = input.seq.saturating_sub(1);
        let history = if history_from <= history_through {
            self.store
                .chat_message_list(
                    thread_id,
                    &ChatMessageQuery {
                        after_seq: Some(summary_through_seq),
                        limit: Some(200),
                        ..ChatMessageQuery::default()
                    },
                )
                .map_err(|e| e.to_string())?
                .items
        } else {
            Vec::new()
        };
        let chat = CosChatContext {
            thread_id: thread_id.to_string(),
            run_id: run_id.to_owned(),
            inputs: vec![CosChatInput {
                id: input.id.clone(),
                seq: input.seq as i64,
                text: input.text.clone(),
                interrupt: false,
                attachment_ids: input.attachment_ids.clone(),
            }],
            summary: (!summary.is_empty()).then_some(summary),
            summary_through_seq: summary_through_seq as i64,
            unsummarized: CosChatHistory {
                from_seq: history_from as i64,
                through_seq: history_through as i64,
                messages: history
                    .into_iter()
                    .filter(|m| m.seq <= history_through)
                    .map(|m| CosChatHistoryMessage {
                        id: m.id,
                        seq: m.seq as i64,
                        role: format!("{:?}", m.role).to_lowercase(),
                        text: m.text,
                    })
                    .collect(),
            },
            attachments,
            skills: vec!["cos-operator".into(), "cos-inbox-triage".into()],
            credential_env: COS_RUN_CREDENTIAL_ENV.into(),
            api_base_url: cfg.api_base_url.clone(),
        };
        let budget = task_core::Budget {
            max_turns: cfg.max_turns,
            max_wall_secs: cfg.max_wall_secs,
            max_retries: 0,
        };
        let task = chat.transient_task(&workspace, budget, now);
        let artifacts_dir = workspace.join(".taskd").join("chat-runs").join(run_id);
        if let Err(e) = std::fs::create_dir_all(&artifacts_dir) {
            return Err(format!("CoS artifacts unavailable: {e}"));
        }
        let (skills, missing_skills) = dispatcher.skills_context(
            &["cos-operator".into(), "cos-inbox-triage".into()],
            task_ops::knowledge::SkillUse::Work,
        );
        if !missing_skills.is_empty() {
            return Err(format!(
                "CoS skills unavailable: {}",
                missing_skills.join(", ")
            ));
        }
        let context = RunContext {
            cos_chat: Some(chat),
            skills,
            session: Some(SessionHandle {
                adapter: cfg.harness.clone(),
                session_id: session.session_id.clone(),
                resume: mode == ChatRunSessionMode::Resumed,
            }),
            ..RunContext::default()
        };
        let req = RunRequest {
            protocol: task_worker::protocol::PROTOCOL_VERSION,
            task,
            workspace,
            work_dir: None,
            artifacts_dir: artifacts_dir.clone(),
            context,
            cargo_target_dir: None,
        };
        let adapter = if let Some(account_id) = account.as_deref() {
            if let Some(account_adapter) = task_core::AccountAdapter::parse(&cfg.harness) {
                if dispatcher.account_pool_providers.contains(provider) {
                    match dispatcher.adapter_for_account(&adapter, account_adapter, account_id) {
                        Some(adapter) => adapter,
                        None => {
                            return Err(format!(
                                "CoS unavailable: account {account_id} could not be mounted"
                            ));
                        }
                    }
                } else {
                    adapter
                }
            } else {
                adapter
            }
        } else {
            adapter
        };
        let adapter = if let Some(model) = cfg.model.as_deref() {
            adapter.with_model(model).ok_or_else(|| {
                format!(
                    "CoS unavailable: harness {} cannot select model {model}",
                    cfg.harness
                )
            })?
        } else {
            adapter
        };
        let ttl_secs = i64::try_from(cfg.max_wall_secs)
            .ok()
            .and_then(|secs| secs.checked_add(60))
            .ok_or_else(|| "CoS max_wall_secs exceeds credential lifetime range".to_string())?;
        let secret = self
            .store
            .cos_run_credential_issue_at(thread_id, run_id, time::Duration::seconds(ttl_secs), now)
            .map_err(|e| format!("CoS credential unavailable: {e}"))?;
        let token = format!("{COS_RUN_BEARER_PREFIX}{secret}");
        let Some(adapter) = adapter.with_env(&[(COS_RUN_CREDENTIAL_ENV.into(), token.clone())])
        else {
            if let Err(error) = self.store.cos_run_credential_revoke(run_id, now) {
                tracing::warn!(%error, %run_id, "CoS credential revoke after launch failure failed");
            }
            return Err(format!(
                "CoS unavailable: harness {} cannot receive a run credential",
                cfg.harness
            ));
        };
        let store = Arc::clone(&self.store);
        let session_row_id = session.id;
        let thread = thread_id.to_string();
        let id = run_id.to_owned();
        let wall = cfg.max_wall_secs;
        let idle = dispatcher.config.idle_timeout;
        let grace = dispatcher.config.kill_grace;
        let clock: ChatClock = {
            #[cfg(test)]
            {
                if let Some(test_now) = &dispatcher.test_now {
                    let test_now = Arc::clone(test_now);
                    Arc::new(move || *test_now.lock().unwrap_or_else(|e| e.into_inner()))
                } else {
                    Arc::new(OffsetDateTime::now_utc)
                }
            }
            #[cfg(not(test))]
            {
                Arc::new(OffsetDateTime::now_utc)
            }
        };
        self.running.insert(
            thread.clone(),
            tokio::spawn(async move {
                run_claimed(
                    store,
                    adapter,
                    req,
                    &thread,
                    &id,
                    &session_row_id,
                    token,
                    artifacts_dir,
                    wall,
                    idle,
                    grace,
                    clock,
                )
                .await;
            }),
        );
        if let Some(account) = account {
            self.accounts_in_flight
                .insert(thread_id.to_owned(), account);
        }
        self.providers_in_flight
            .insert(thread_id.to_owned(), provider.clone());
        Ok(())
    }

    fn find_input(&self, thread_id: &str, message_id: &str) -> Result<ChatMessage, String> {
        let mut before = None;
        loop {
            let page = self
                .store
                .chat_message_list(
                    thread_id,
                    &ChatMessageQuery {
                        before_seq: before,
                        limit: Some(200),
                        ..ChatMessageQuery::default()
                    },
                )
                .map_err(|e| e.to_string())?;
            if let Some(message) = page.items.into_iter().find(|m| m.id == message_id) {
                return Ok(message);
            }
            before = page.next_before_seq;
            if before.is_none() {
                return Err(format!("claimed message {message_id} missing"));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_claimed(
    store: Arc<SqliteStore>,
    adapter: Arc<dyn WorkerAdapter>,
    req: RunRequest,
    thread_id: &str,
    run_id: &str,
    session_row_id: &str,
    token: String,
    artifacts_dir: PathBuf,
    wall: u64,
    idle: Duration,
    grace: Duration,
    clock: ChatClock,
) {
    let sink = ChatRunSink::new(Arc::clone(&store), thread_id, run_id, clock, vec![token]);
    let limits = RunLimits {
        wall_clock: Duration::from_secs(wall),
        idle_timeout: idle,
        kill_grace: grace,
    };
    let outcome = adapter.run(req, run_id, limits, &sink).await;
    if let Some(session_id) = sink.signals().established
        && let Err(error) = store.chat_session_set_id(session_row_id, &session_id)
    {
        tracing::warn!(%error, %run_id, "CoS session id was not saved");
    }
    if let Err(error) = store.chat_session_touch(session_row_id, 0, OffsetDateTime::now_utc()) {
        tracing::warn!(%error, %run_id, "CoS session usage was not saved");
    }
    let finish = chat_finish_for(&outcome, ChatStopIntent::None);
    CosChatLaunch::rollover_hook(&sink.signals(), &finish);
    let actions = task_worker::read_result_actions(&artifacts_dir);
    if let Err(error) = sink.finish(&finish, &actions) {
        tracing::warn!(%error, %run_id, "CoS chat run finish failed");
    }
    if let Err(error) = store.cos_run_credential_revoke(run_id, OffsetDateTime::now_utc()) {
        tracing::warn!(%error, %run_id, "CoS chat credential revoke failed");
    }
}
