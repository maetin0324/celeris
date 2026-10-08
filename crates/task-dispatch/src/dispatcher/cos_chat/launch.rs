//! Start CoS chat runs from the durable per-thread queue.
//!
//! Only this dispatcher path may grant the ADR-0089 CoS capacity exception. The
//! transient Task is never inserted into the task store.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use task_core::SqliteStore;
use task_core::chat::{
    ChatMessage, ChatMessageQuery, ChatRunSessionMode, ChatRunState, ChatSessionKey,
    ChatThreadQuery,
};
use task_worker::adapter::{RunLimits, WorkerAdapter};
use task_worker::protocol::{
    COS_RUN_CREDENTIAL_ENV, CosChatAttachment, CosChatContext, CosChatDelivery, CosChatInput,
    RunContext, RunRequest, SessionHandle,
};
use time::OffsetDateTime;

use task_core::chat::triage::CosTriageItemReport;
use task_worker::protocol::{CosChatInboxDecision, CosChatInboxItem, CosChatInboxOption};

use super::attachments::{stage_message_attachments, workspace_dir};
use super::sink::{ChatClock, ChatRunSink, chat_finish_for};
use crate::dispatcher::Dispatcher;

/// `task-api::cos::COS_BEARER_PREFIX` wire form. The store issues only the
/// random secret; the worker must present the prefixed bearer to the API.
const COS_RUN_BEARER_PREFIX: &str = "celeris-cos-run.";

/// The API base the worker's `celerisctl` reads when no `--api-url` is given.
pub const COS_API_URL_ENV: &str = "CELERIS_API_URL";

/// Environment for a CoS chat or triage run: the run credential, the daemon's own API
/// base (only when `[api] listen` is set) and a `PATH` that finds the daemon's own
/// release of `celerisctl` first. Without these the worker's `celerisctl` would read the
/// default `~/.config/celeris` and talk to a different daemon.
pub(crate) fn cos_run_env(api_base_url: &str, token: &str) -> Vec<(String, String)> {
    let mut env = vec![(COS_RUN_CREDENTIAL_ENV.to_owned(), token.to_owned())];
    if !api_base_url.is_empty() {
        env.push((COS_API_URL_ENV.to_owned(), api_base_url.to_owned()));
    }
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf));
    if let Some(path) = path_with_first(exe_dir.as_deref(), std::env::var_os("PATH")) {
        env.push(("PATH".to_owned(), path));
    }
    env
}

/// `dir` prepended to `inherited`, once. `None` when there is no dir or the result is not UTF-8.
pub(crate) fn path_with_first(
    dir: Option<&std::path::Path>,
    inherited: Option<std::ffi::OsString>,
) -> Option<String> {
    let dir = dir?;
    let mut entries = vec![dir.to_path_buf()];
    if let Some(inherited) = inherited {
        entries.extend(std::env::split_paths(&inherited).filter(|p| p != dir));
    }
    std::env::join_paths(entries).ok()?.into_string().ok()
}

/// Open inbox items handed to a run of the inbox thread (newest first).
const COS_INBOX_CONTEXT_ITEMS: usize = 50;

/// ADR 2026-10-08-cos-chat-prompt-cache T5: the skills mounted for a CoS chat run.
/// `cos-operator` always; `cos-inbox-triage` only in the inbox thread or when the run is handed
/// inbox items, so an ordinary thread does not get the triage procedure. Authorization does not
/// depend on this: `/cos/operations` and the inbox resolve API enforce it whatever was mounted.
pub(crate) fn cos_chat_skills(in_inbox_thread: bool, inbox_items: &[CosChatInboxItem]) -> Vec<String> {
    let mut skills = vec!["cos-operator".to_string()];
    if in_inbox_thread || !inbox_items.is_empty() {
        skills.push("cos-inbox-triage".to_string());
    }
    skills
}

/// `cos_inbox_items` row → worker context item (no judgment; the packet is copied as recorded).
pub(crate) fn inbox_context_item(item: CosTriageItemReport) -> CosChatInboxItem {
    let answer_path = (item.source_kind != super::triage::COS_TRIAGE_NOTICE_KIND)
        .then(|| format!("/api/v1/inbox/items/{}/answer", item.source_key));
    let summary = item.title();
    CosChatInboxItem {
        item_id: item.item_id,
        source_kind: item.source_kind,
        source_key: item.source_key,
        source_revision: item.source_revision,
        state: item.state,
        summary,
        reason: item.reason,
        created_at: item.created_at,
        decision: item.decision.map(|d| CosChatInboxDecision {
            summary: d.summary,
            options: d
                .options
                .into_iter()
                .map(|o| CosChatInboxOption {
                    key: o.key,
                    label: o.label,
                })
                .collect(),
            recommended: d.recommended,
            recommendation_reason: d.recommendation_reason,
            web_path: d.web_path,
        }),
        answer_path,
    }
}

/// Values resolved from `[cos]` by the daemon, before the dispatcher starts.
#[derive(Debug, Clone)]
pub struct CosChatLaunchConfig {
    pub enabled: bool,
    pub fallbacks: Vec<CosChatRoute>,
    pub worker_reserve_five_hour: f64,
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
    /// `[cos.triage]` as a whole (ADR 2026-10-05 D3/D6). Later triage leaves read it here.
    pub triage: super::triage::CosTriageSettings,
}

/// One resolved, explicitly configured route.
#[derive(Debug, Clone)]
pub struct CosChatRoute {
    pub harness: String,
    pub llm_source: Option<String>,
    pub provider: Option<String>,
    pub account_id: Option<String>,
    pub model: Option<String>,
    pub tier: task_core::Tier,
    pub unavailable_reason: Option<String>,
}

pub(super) struct RouteRetry {
    pub thread_id: String,
    pub run_id: String,
    pub next_route: usize,
    pub reason: String,
    pub previous: Arc<ChatRunSink>,
}

pub(super) type RouteRetries = Arc<Mutex<Vec<RouteRetry>>>;

pub(crate) struct CosChatLaunch {
    pub store: Arc<SqliteStore>,
    pub config: CosChatLaunchConfig,
    pub(super) retries: RouteRetries,
    pub running: HashMap<String, tokio::task::JoinHandle<()>>,
    pub(crate) accounts_in_flight: HashMap<String, (task_core::AccountAdapter, String)>,
    pub(crate) providers_in_flight: HashMap<String, String>,
    pub(crate) triage: super::triage::CosTriageState,
}

/// Which durable claim binds the new run to its input message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ClaimSource {
    /// The oldest queued user message of the thread.
    Queue,
    /// Up to twenty pending triage items, as one system message in the inbox thread.
    Triage,
}

impl Dispatcher {
    /// Install the concrete chat store. It must be the same database as `TaskStore`.
    pub fn set_cos_chat_launch(&mut self, store: Arc<SqliteStore>, config: CosChatLaunchConfig) {
        self.cos_chat_launch = Some(CosChatLaunch {
            store,
            config,
            retries: Arc::new(Mutex::new(Vec::new())),
            running: HashMap::new(),
            accounts_in_flight: HashMap::new(),
            providers_in_flight: HashMap::new(),
            triage: super::triage::CosTriageState::default(),
        });
    }

    /// One non-blocking tick: each eligible thread may start at most one run.
    pub(crate) fn tick_cos_chat_launch(&mut self) {
        let Some(mut launch) = self.cos_chat_launch.take() else {
            return;
        };
        launch.running.retain(|_, handle| !handle.is_finished());
        if launch.triage.inbox_run_live
            && !launch
                .triage
                .inbox_thread
                .as_ref()
                .is_some_and(|thread| launch.running.contains_key(thread))
        {
            // The inbox thread's run ended: its digest may be due (cheap DB check).
            launch.triage.inbox_run_live = false;
            launch.triage.digest_due = true;
        }
        launch
            .accounts_in_flight
            .retain(|thread, _| launch.running.contains_key(thread));
        launch
            .providers_in_flight
            .retain(|thread, _| launch.running.contains_key(thread));
        launch.retry_routes(self);
        // Reconcile stop and ownerless runs before a new claim, so an old
        // process cannot overlap a new one in the same thread.
        launch.control_hook(self);
        // Intake is DB-only and runs even when CoS is disabled, so the
        // unavailable fallback can still see every new wait.
        launch.triage_ingest(self);
        if self.accepting_new_work && self.disk_ready && launch.config.enabled {
            if let Err(error) = launch.claim_queued(self) {
                tracing::warn!(%error, "CoS chat launch tick failed");
            }
            if let Err(error) = launch.triage_launch(self) {
                tracing::warn!(%error, "CoS triage launch failed");
            }
        }
        self.cos_chat_launch = Some(launch);
    }
}

impl CosChatLaunch {
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
                if !self.has_capacity(dispatcher) {
                    return Ok(());
                }
                self.start_thread(dispatcher, &thread.id, ClaimSource::Queue)?;
            }
            before = page.next_cursor;
            if before.is_none() {
                break;
            }
        }
        Ok(())
    }

    /// ADR-0089: the CoS slot (or the general slot when `max_cos_runs = 0`).
    pub(super) fn has_capacity(&self, dispatcher: &Dispatcher) -> bool {
        let in_flight =
            self.running.len() + dispatcher.running.values().filter(|run| run.cos).count();
        if dispatcher.config.execution.max_cos_runs == 0 {
            dispatcher.workers_in_flight() + in_flight < dispatcher.config.max_concurrency
        } else {
            in_flight < dispatcher.config.execution.max_cos_runs
        }
    }

    pub(super) fn start_thread(
        &mut self,
        dispatcher: &mut Dispatcher,
        thread_id: &str,
        source: ClaimSource,
    ) -> Result<(), String> {
        let now = dispatcher.now_utc();
        let cfg = &self.config;
        let run_id = ulid::Ulid::new().to_string();
        let resolved = json!({
            "harness": cfg.harness,
            "llm_source": cfg.llm_source,
            "provider": cfg.provider,
            "account_id": cfg.account_id,
            "model": cfg.model,
            "tier": format!("{:?}", cfg.tier).to_lowercase(),
        });
        let run = match source {
            ClaimSource::Queue => self
                .store
                .chat_run_claim_next(thread_id, &run_id, &resolved, now)
                .map_err(|e| e.to_string())?,
            ClaimSource::Triage => self.triage_claim(thread_id, &run_id, now)?,
        };
        let Some(run) = run else {
            return Ok(());
        };
        self.launch_routes(dispatcher, thread_id, &run, 0, None, now);
        Ok(())
    }

    pub(super) fn select_route_account(
        &self,
        dispatcher: &mut Dispatcher,
        thread_id: &str,
        cfg: &CosChatLaunchConfig,
    ) -> Result<Option<String>, String> {
        let account = if let Some(provider) = cfg.provider.as_ref() {
            if dispatcher.account_pool_providers.contains(provider) {
                let Some(account_adapter) = dispatcher
                    .adapters
                    .get(provider)
                    .and_then(|a| dispatcher.pool_adapter_of(provider, a.id()))
                else {
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
                        .filter(|(adapter, account)| *adapter == account_adapter && account == id)
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
                        return Ok(None);
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
                                    .filter(|(adapter, account)| {
                                        *adapter == account_adapter && account == &dir.id
                                    })
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
        Ok(account)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn launch_claimed(
        &mut self,
        dispatcher: &Dispatcher,
        cfg: &CosChatLaunchConfig,
        route_index: usize,
        previous: Option<Arc<ChatRunSink>>,
        thread_id: &str,
        run_id: &str,
        run: &task_core::chat::ChatRun,
        account: Option<String>,
        now: OffsetDateTime,
    ) -> Result<(), String> {
        // ADR 2026-10-07-cos-inbox-thread-conversation D3: a run in the inbox thread (triage or
        // a human message) sees the unresolved items.
        let in_inbox_thread = self.inbox_thread()?.as_deref() == Some(thread_id);
        let inbox_items = if in_inbox_thread {
            self.store
                .cos_triage_open_items(COS_INBOX_CONTEXT_ITEMS)
                .map_err(|e| format!("CoS inbox context unavailable: {e}"))?
                .into_iter()
                .map(inbox_context_item)
                .collect()
        } else {
            Vec::new()
        };
        let skill_names = cos_chat_skills(in_inbox_thread, &inbox_items);
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
        let super::rollover::SessionChoice { session, mode, .. } = super::rollover::choose_session(
            &self.store,
            run_id,
            session_key,
            dispatcher.config.session_rollover_tokens,
            now,
        )?;
        let (mut summary, summary_through_seq, unsummarized) =
            super::rollover::history_since_summary(&self.store, thread_id, input.seq)?;
        if route_index > 0 {
            let applied = self
                .store
                .cos_operation_applied_for_run(run_id)
                .map_err(|e| format!("CoS operation receipts unavailable: {e}"))?;
            if !applied.is_empty() {
                let receipts = applied
                    .iter()
                    .map(|op| {
                        format!(
                            "{} {} {}:{} (idempotency_key={})",
                            op.id, op.action, op.target_kind, op.target_id, op.idempotency_key
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                summary = Some(format!(
                    "{}\nApplied operations from the previous route (do not reapply): {receipts}",
                    summary.unwrap_or_default()
                ));
            }
        }
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
            summary,
            summary_through_seq: i64::try_from(summary_through_seq).unwrap_or(i64::MAX),
            unsummarized,
            attachments,
            // These two concrete CLI adapters implement native image input. ACP abilities
            // are negotiated by the agent at runtime, so they stay unconfirmed here.
            harness_capabilities: match cfg.harness.as_str() {
                "claude-code" | "codex" => {
                    task_worker::cos_chat::HarnessCapabilities::for_adapter(&cfg.harness)
                }
                _ => None,
            },
            skills: skill_names.clone(),
            credential_env: COS_RUN_CREDENTIAL_ENV.into(),
            api_base_url: cfg.api_base_url.clone(),
            inbox_items,
        };
        let budget = task_core::Budget {
            max_turns: cfg.max_turns,
            max_wall_secs: cfg.max_wall_secs,
            max_retries: 0,
        };
        let capture = super::workspace_files::WorkspaceCapture {
            before: super::workspace_files::snapshot(&workspace),
            workspace: workspace.clone(),
            data_dir: cfg.data_dir.clone(),
            db_path: cfg.db_path.clone(),
            limits: cfg.attachment_limits,
        };
        let task = chat.transient_task(&workspace, budget, now);
        let artifacts_dir = workspace.join(".taskd").join("chat-runs").join(run_id);
        if let Err(e) = std::fs::create_dir_all(&artifacts_dir) {
            return Err(format!("CoS artifacts unavailable: {e}"));
        }
        let (skills, missing_skills) = dispatcher.skills_context(
            &skill_names,
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
            if let Some(account_adapter) = dispatcher.pool_adapter_of(provider, adapter.id()) {
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
        let Some(adapter) = adapter.with_env(&cos_run_env(&cfg.api_base_url, &token)) else {
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
        self.store
            .chat_run_route(
                run_id,
                &json!({
                    "harness": cfg.harness, "llm_source": cfg.llm_source,
                    "provider": cfg.provider, "account_id": account, "model": cfg.model,
                    "tier": format!("{:?}", cfg.tier).to_lowercase(),
                }),
                now,
            )
            .map_err(|e| e.to_string())?;
        self.route_notice(
            run_id,
            thread_id,
            route_index,
            "started",
            &format!(
                "CoS 実行経路: {} / {} / {} / {}",
                cfg.harness,
                cfg.llm_source.as_deref().unwrap_or("unknown"),
                provider,
                cfg.model.as_deref().unwrap_or("default")
            ),
            now,
        );
        let retries = Arc::clone(&self.retries);
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
                    capture,
                    wall,
                    idle,
                    grace,
                    clock,
                    retries,
                    route_index,
                    previous,
                )
                .await;
            }),
        );
        if self.triage.inbox_thread.as_deref() == Some(thread_id) {
            self.triage.inbox_run_live = true;
        }
        if let Some(account) = account
            && let Some(account_adapter) = dispatcher
                .adapters
                .get(provider)
                .and_then(|a| dispatcher.pool_adapter_of(provider, a.id()))
        {
            self.accounts_in_flight
                .insert(thread_id.to_owned(), (account_adapter, account));
        }
        self.providers_in_flight
            .insert(thread_id.to_owned(), provider.clone());
        Ok(())
    }

    pub(super) fn find_input(
        &self,
        thread_id: &str,
        message_id: &str,
    ) -> Result<ChatMessage, String> {
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
    capture: super::workspace_files::WorkspaceCapture,
    wall: u64,
    idle: Duration,
    grace: Duration,
    clock: ChatClock,
    retries: RouteRetries,
    route_index: usize,
    previous: Option<Arc<ChatRunSink>>,
) {
    // A stop may be committed between the claim and the spawned task's first poll.
    match store.chat_run_get(thread_id, run_id) {
        Ok(run) if run.state == ChatRunState::Stopping => {
            let sink = ChatRunSink::new(
                Arc::clone(&store),
                thread_id,
                run_id,
                Arc::clone(&clock),
                vec![token],
            );
            let finish = chat_finish_for(
                &Err(task_worker::adapter::AdapterError::Other(
                    "stopped before launch".into(),
                )),
                super::control::stop_intent(&run),
            );
            if let Err(error) = sink.finish(
                &finish,
                &task_worker::result_report::ParsedActions::default(),
            ) {
                tracing::warn!(%error, %run_id, "CoS chat pre-launch stop failed");
            }
            return;
        }
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(%error, %run_id, "CoS chat state unavailable before launch");
            let _ = store.cos_run_credential_revoke(run_id, (clock)());
            return;
        }
    }
    let limits = RunLimits {
        wall_clock: Duration::from_secs(wall),
        idle_timeout: idle,
        kill_grace: grace,
    };
    // Rollover leaf: a refused resume is retried once with a fresh session inside this run.
    // run_attempts owns the sink(s) and saves the session id and usage per attempt.
    let (sink, mut finish) = super::rollover::run_attempts(super::rollover::ChatAttempt {
        store: Arc::clone(&store),
        previous,
        adapter,
        req,
        thread_id: thread_id.to_owned(),
        run_id: run_id.to_owned(),
        session_row_id: session_row_id.to_owned(),
        secrets: vec![token],
        limits,
        clock: Arc::clone(&clock),
    })
    .await;
    // Control leaf: a human stop or interrupt committed during the run decides the terminal.
    let current = match store.chat_run_get(thread_id, run_id) {
        Ok(run) => run,
        Err(error) => {
            tracing::warn!(%error, %run_id, "CoS chat state unavailable at completion");
            let _ = store.cos_run_credential_revoke(run_id, (clock)());
            return;
        }
    };
    if current.state == ChatRunState::Stopping {
        // A stop intent overrides the worker outcome (same mapping as before rollover).
        finish = chat_finish_for(
            &Err(task_worker::adapter::AdapterError::Other("stopped".into())),
            super::control::stop_intent(&current),
        );
    }
    let pending = match store.cos_operation_pending_for_run(run_id) {
        Ok(pending) => pending,
        Err(error) => {
            tracing::warn!(%error, %run_id, "CoS pending operation check failed");
            let _ = store.cos_run_credential_revoke(run_id, (clock)());
            return; // a later ownerless pass can resolve the operation safely
        }
    };
    if pending {
        match super::control::pending_review(&store, &current, (clock)()) {
            Ok(()) => {
                finish.state = ChatRunState::Interrupted;
                finish.final_text = None;
                finish.reason = Some("operation outcome unknown; human review required".into());
            }
            Err(error) => {
                tracing::warn!(%error, %run_id, "CoS pending operation review failed");
                let _ = store.cos_run_credential_revoke(run_id, (clock)());
                return;
            }
        }
    }
    if finish.state == ChatRunState::Failed
        && !pending
        && super::routes::supply_failure(finish.reason.as_deref().unwrap_or(""))
    {
        sink.close_attempt_tools();
        let reason = sink.redact_reason(finish.reason.as_deref().unwrap_or("利用上限"));
        let _ = store.cos_run_credential_revoke(run_id, (clock)());
        retries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(RouteRetry {
                thread_id: thread_id.to_owned(),
                run_id: run_id.to_owned(),
                next_route: route_index + 1,
                reason,
                previous: Arc::new(sink),
            });
        return;
    }
    if finish.state == ChatRunState::Failed {
        finish.final_text = Some(super::routes::failure_text(
            &sink.redact_reason(finish.reason.as_deref().unwrap_or("worker failed")),
        ));
    }
    // ADR 2026-10-08-cos-workspace-files-in-chat D1: files written in the thread workspace and
    // paths the reply mentions become attachments of the reply, before its terminal event.
    if let Some(output) = current.output_message_id.as_deref() {
        let text = finish
            .final_text
            .clone()
            .or_else(|| super::workspace_files::message_text(&store, thread_id, output))
            .unwrap_or_default();
        if let Err(error) = super::workspace_files::attach(
            &store,
            &capture,
            thread_id,
            output,
            run_id,
            &text,
            (clock)(),
        ) {
            tracing::warn!(%error, %run_id, "CoS workspace files not attached");
        }
    }
    let actions = task_worker::read_result_actions(&artifacts_dir);
    if let Err(error) = sink.finish(&finish, &actions) {
        tracing::warn!(%error, %run_id, "CoS chat run finish failed");
    }
    if let Err(error) = store.cos_run_credential_revoke(run_id, OffsetDateTime::now_utc()) {
        tracing::warn!(%error, %run_id, "CoS chat credential revoke failed");
    }
}

#[cfg(test)]
mod tests;
