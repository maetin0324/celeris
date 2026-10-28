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
use crate::dispatcher::route_select::RoutePick;

/// `task-api::cos::COS_BEARER_PREFIX` wire form. The store issues only the
/// random secret; the worker must present the prefixed bearer to the API.
const COS_RUN_BEARER_PREFIX: &str = "celeris-cos-run.";

/// The API base the worker's `celerisctl` reads when no `--api-url` is given.
pub const COS_API_URL_ENV: &str = "CELERIS_API_URL";
/// The policy version the daemon handed the run (`celerisctl` reads it; ADR 2026-10-28-cos-session-policy-key D2).
pub const COS_POLICY_VERSION_ENV: &str = "CELERIS_COS_POLICY_VERSION";

/// Environment for a CoS chat or triage run: the run credential, the daemon's own API
/// base (only when `[api] listen` is set) and a `PATH` that finds the daemon's own
/// release of `celerisctl` first. Without these the worker's `celerisctl` would read the
/// default `~/.config/celeris` and talk to a different daemon.
/// ADR 2026-10-28-cos-session-policy-key D2: `celerisctl`'s CoS path sends this as `policy_version`.
pub(crate) fn cos_run_env(
    api_base_url: &str,
    token: &str,
    policy_version: &str,
    run_tmp: Option<&task_worker::run_tmpdir::RunTmpDir>,
) -> Vec<(String, String)> {
    let mut env = vec![
        (COS_RUN_CREDENTIAL_ENV.to_owned(), token.to_owned()),
        (COS_POLICY_VERSION_ENV.to_owned(), policy_version.to_owned()),
    ];
    if !api_base_url.is_empty() {
        env.push((COS_API_URL_ENV.to_owned(), api_base_url.to_owned()));
    }
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf));
    if let Some(path) = path_with_first(exe_dir.as_deref(), std::env::var_os("PATH")) {
        env.push(("PATH".to_owned(), path));
    }
    if let Some(tmp) = run_tmp {
        env.extend(tmp.env());
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
/// `cos-operator` always; `cos-inbox-triage` in a general/project CoS thread or when the run is
/// handed inbox items, so a legacy thread does not get the triage procedure. Authorization does not
/// depend on this: `/cos/operations` and the inbox resolve API enforce it whatever was mounted.
pub(crate) fn cos_chat_skills(
    in_inbox_thread: bool,
    inbox_items: &[CosChatInboxItem],
) -> Vec<String> {
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
        thread_id: Some(item.thread_id.clone()),
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

/// Values resolved from `[cos]` by the daemon at startup or reload.
#[derive(Debug, Clone)]
pub struct CosChatLaunchConfig {
    pub enabled: bool,
    pub worker_reserve_five_hour: f64,
    pub harness: String,
    pub max_turns: u32,
    pub max_wall_secs: u64,
    pub unavailable_reason: Option<String>,
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub attachment_limits: task_core::chat::attachments::ChatAttachmentLimits,
    pub api_base_url: String,
    /// `[notify] web_base_url`: the links of the root-done summary (ADR 2026-10-15-root-done-cos-summary D5).
    pub web_base_url: Option<String>,
    /// `[cos.triage]` as a whole (ADR 2026-10-05 D3/D6). Later triage leaves read it here.
    pub triage: super::triage::CosTriageSettings,
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
    /// Per-thread snapshot for the whole run, including its fallback attempts.
    pub(super) run_configs: HashMap<String, CosChatLaunchConfig>,
    pub(super) retries: RouteRetries,
    pub running: HashMap<String, tokio::task::JoinHandle<()>>,
    pub(crate) route_reservations: HashMap<
        String,
        (
            task_core::llm_route::SourceId,
            task_core::llm_route::AccountId,
        ),
    >,
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
            run_configs: HashMap::new(),
            retries: Arc::new(Mutex::new(Vec::new())),
            running: HashMap::new(),
            route_reservations: HashMap::new(),
            triage: super::triage::CosTriageState::default(),
        });
    }

    /// Startup-only: verify owns smoke tasks, never snapshot chats or inbox intake.
    /// Unlike `enabled=false`, removing the pipeline also omits reconciliation,
    /// fallback and root-done processing. Call before the first tick.
    pub fn disable_cos_chat_for_verify(&mut self) {
        self.cos_chat_launch = None;
    }

    /// Change only future runs; keep handles, capacity reservations and triage state intact.
    pub fn reload_cos_chat_launch(&mut self, config: CosChatLaunchConfig) {
        if let Some(launch) = &mut self.cos_chat_launch {
            launch.config = config;
        }
    }

    /// One non-blocking tick: each eligible thread may start at most one run.
    pub(crate) fn tick_cos_chat_launch(&mut self) {
        let Some(mut launch) = self.cos_chat_launch.take() else {
            return;
        };
        launch.running.retain(|_, handle| !handle.is_finished());
        let running = &launch.running;
        let before = launch.triage.live_runs.len();
        launch.triage.live_runs.retain(|t| running.contains_key(t));
        if launch.triage.live_runs.len() < before {
            // A CoS run ended: a triage digest may be due (cheap DB check).
            launch.triage.digest_due = true;
        }
        launch
            .route_reservations
            .retain(|thread, _| launch.running.contains_key(thread));
        launch.retry_routes(self);
        // Reconcile stop and ownerless runs before a new claim, so an old
        // process cannot overlap a new one in the same thread.
        launch.control_hook(self);
        // Intake is DB-only and runs even when CoS is disabled, so the
        // unavailable fallback can still see every new wait.
        launch.triage_ingest(self);
        // ADR 2026-10-15-root-done-cos-summary: root task done → a summary thread and one CoS run.
        launch.root_done_tick(self);
        if self.accepting_new_work && self.disk_ready && launch.config.enabled {
            if let Err(error) = launch.claim_queued(self) {
                tracing::warn!(%error, "CoS chat launch tick failed");
            }
            // ADR 2026-10-27-relayed-instructions 付記 A2: after the queued messages (a relay's own
            // run goes first) and before new triage items.
            if let Err(error) = launch.relay_rejudge(self) {
                tracing::warn!(%error, "CoS relay re-judging failed");
            }
            if let Err(error) = launch.triage_launch(self) {
                tracing::warn!(%error, "CoS triage launch failed");
            }
        }
        {
            let retries = launch.retries.lock().unwrap_or_else(|e| e.into_inner());
            launch.run_configs.retain(|thread, _| {
                launch.running.contains_key(thread)
                    || retries.iter().any(|retry| &retry.thread_id == thread)
            });
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
            dispatcher.workers_in_flight() + in_flight < dispatcher.max_concurrency
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
        let run_id = ulid::Ulid::new().to_string();
        let resolved = json!({
            "harness": "claude_code",
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
        self.run_configs
            .insert(thread_id.to_owned(), self.config.clone());
        self.launch_routes(dispatcher, thread_id, &run, 0, None, now);
        Ok(())
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
        pick: RoutePick,
        selection_metadata: serde_json::Value,
        now: OffsetDateTime,
    ) -> Result<(), String> {
        // ADR 2026-10-22-cos-fixed-threads D3: a project CoS sees its project's unresolved items;
        // the general CoS overlooks every thread's. Legacy threads get none.
        let thread = self
            .store
            .chat_thread_get(thread_id)
            .map_err(|e| e.to_string())?;
        let kind = thread.as_ref().map(|t| t.kind);
        // ADR 2026-10-27-project-brief D5: the general CoS sees every project's summary; a project
        // CoS its own project's. Legacy threads get none.
        let projects = match kind {
            Some(task_core::chat::ChatThreadKind::General) => {
                super::projects::project_summaries(&self.store, None)?
            }
            Some(task_core::chat::ChatThreadKind::Project) => {
                match thread
                    .as_ref()
                    .and_then(|t| t.project_id.as_deref())
                    .and_then(|id| id.parse::<task_core::ProjectId>().ok())
                {
                    Some(id) => super::projects::project_summaries(&self.store, Some(id))?,
                    None => Vec::new(),
                }
            }
            _ => Vec::new(),
        };
        let scope = match kind {
            Some(task_core::chat::ChatThreadKind::General) => Some(None),
            Some(task_core::chat::ChatThreadKind::Project) => Some(Some(thread_id)),
            _ => None,
        };
        let in_inbox_thread = scope.is_some();
        let inbox_items = match scope {
            Some(scope) => self
                .store
                .cos_triage_open_items(scope, COS_INBOX_CONTEXT_ITEMS)
                .map_err(|e| format!("CoS inbox context unavailable: {e}"))?
                .into_iter()
                .map(inbox_context_item)
                .collect(),
            None => Vec::new(),
        };
        let skill_names = cos_chat_skills(in_inbox_thread, &inbox_items);
        if let Some(reason) = cfg.unavailable_reason.clone() {
            return Err(reason);
        }
        let (skills, missing_skills) =
            dispatcher.skills_context(&skill_names, task_ops::knowledge::SkillUse::Work);
        if !missing_skills.is_empty() {
            return Err(format!(
                "CoS skills unavailable: {}",
                missing_skills.join(", ")
            ));
        }
        // ADR 2026-10-28-cos-session-policy-key D1: the policy this run gets is part of the key.
        let policy_version = cfg.triage.policy_version.clone();
        let policy_key = super::policy::policy_key(
            &policy_version,
            &skills
                .iter()
                .map(|s| (s.name.clone(), PathBuf::from(&s.path)))
                .collect::<Vec<_>>(),
        )
        .map_err(|e| format!("CoS skills unavailable: {e}"))?;
        let provider = pick.candidate.source.to_string();
        let account = Some(pick.account.to_string());
        let selected_route = dispatcher
            .route_launch_target(&pick)
            .map_err(|e| e.to_string())?;
        let adapter = dispatcher.route_adapter(&pick).map_err(|e| e.to_string())?;
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
            harness: pick.harness.to_string(),
            provider: Some(provider.clone()),
            llm_source: Some(pick.candidate.source.to_string()),
            model: Some(pick.candidate.model.to_string()),
            account_id: account.clone(),
            cwd: Some(workspace.to_string_lossy().into_owned()),
            policy_key: Some(policy_key.clone()),
        };
        let choice = if route_index > 0 {
            // Even two configured routes with the same session key must not reuse the failed
            // route's session. Only the session that completes may advance its cursor.
            let session = super::rollover::rotate_fresh(&self.store, session_key, now)?;
            self.store
                .chat_run_record_session(
                    run_id,
                    ChatRunSessionMode::Fresh,
                    &session.id,
                    Some("route_changed"),
                    now,
                )
                .map_err(|e| e.to_string())?;
            super::rollover::SessionChoice {
                session,
                mode: ChatRunSessionMode::Fresh,
                reason: Some("route_changed"),
                delta_from: None,
            }
        } else {
            super::rollover::choose_session(
                &self.store,
                run_id,
                session_key,
                dispatcher.config.session_rollover_tokens,
                now,
            )?
        };
        let super::rollover::SessionChoice {
            session,
            mode,
            delta_from,
            ..
        } = choice;
        // A fallback route uses another session: it needs the full summary and history even
        // when the original attempt resumed with a delivery cursor.
        let delta_from = if route_index == 0 { delta_from } else { None };
        let mut history =
            super::rollover::history_for_run(&self.store, thread_id, input.seq, delta_from)?;
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
                history.summary = Some(format!(
                    "{}\nApplied operations from the previous route (do not reapply): {receipts}",
                    history.summary.unwrap_or_default()
                ));
            }
        }

        // ADR 2026-10-27-relayed-instructions D4: the unexpired relays of the thread.
        let relayed_instructions = self
            .store
            .chat_relayed_instructions(thread_id, now)
            .map_err(|e| format!("relayed instructions unavailable: {e}"))?
            .into_iter()
            .filter_map(|message| {
                let relay = message.relay?;
                Some(task_worker::protocol::CosChatRelayedInstruction {
                    message_id: message.id,
                    seq: i64::try_from(message.seq).unwrap_or(i64::MAX),
                    text: message.text,
                    relayed_by: relay.relayed_by,
                    said_at: relay.said_at,
                    task_ids: relay.scope.task_ids,
                    expires_at: relay.scope.expires_at,
                })
            })
            .collect();
        let chat = CosChatContext {
            thread_id: thread_id.to_string(),
            run_id: run_id.to_owned(),
            relayed_instructions,
            inputs: vec![CosChatInput {
                id: input.id.clone(),
                seq: input.seq as i64,
                text: input.text.clone(),
                interrupt: false,
                attachment_ids: input.attachment_ids.clone(),
            }],
            summary: history.summary,
            summary_through_seq: i64::try_from(history.summary_through_seq).unwrap_or(i64::MAX),
            unsummarized: history.history,
            delivered_through_seq: history
                .delivered_through_seq
                .map(|cursor| i64::try_from(cursor).unwrap_or(i64::MAX)),
            attachments,
            // These two concrete CLI adapters implement native image input. ACP abilities
            // are negotiated by the agent at runtime, so they stay unconfirmed here.
            harness_capabilities: match pick.harness.as_ref() {
                "claude_code" => {
                    task_worker::cos_chat::HarnessCapabilities::for_adapter("claude-code")
                }
                "claude-code" | "codex" => {
                    task_worker::cos_chat::HarnessCapabilities::for_adapter(pick.harness.as_ref())
                }
                _ => None,
            },
            skills: skill_names.clone(),
            policy_version: policy_version.clone(),
            credential_env: COS_RUN_CREDENTIAL_ENV.into(),
            api_base_url: cfg.api_base_url.clone(),
            inbox_items,
            projects,
        };
        let budget = task_core::Budget {
            max_turns: cfg.max_turns,
            max_wall_secs: cfg.max_wall_secs,
            max_retries: 0,
            max_tokens: None,
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
        let context = RunContext {
            cos_chat: Some(chat),
            skills,
            session: Some(SessionHandle {
                adapter: "claude-code".to_owned(),
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

            llm_route: Some(selected_route),
        };
        let ttl_secs = i64::try_from(cfg.max_wall_secs)
            .ok()
            .and_then(|secs| secs.checked_add(60))
            .ok_or_else(|| "CoS max_wall_secs exceeds credential lifetime range".to_string())?;
        // D2: the version handed to the run is what `/cos/operations` checks and records.
        self.store
            .chat_run_record_policy(run_id, &policy_version, &policy_key)
            .map_err(|e| format!("CoS policy record failed: {e}"))?;
        let secret = self
            .store
            .cos_run_credential_issue_at(thread_id, run_id, time::Duration::seconds(ttl_secs), now)
            .map_err(|e| format!("CoS credential unavailable: {e}"))?;
        let token = format!("{COS_RUN_BEARER_PREFIX}{secret}");
        // `$TMPDIR` for the CoS run (WORK_RULES tell it to use `$TMPDIR`). Without it a relative
        // `$TMPDIR/x` resolves to `/x`. Removed when the run's task ends (`Drop`).
        let run_tmp = match task_worker::run_tmpdir::RunTmpDir::create(&req.workspace, run_id) {
            Ok(tmp) => Some(tmp),
            Err(e) => {
                tracing::warn!(%run_id, error = %e, "CoS run tmpdir: could not create; TMPDIR is left unchanged");
                None
            }
        };
        let run_env = cos_run_env(&cfg.api_base_url, &token, &policy_version, run_tmp.as_ref());
        let Some(adapter) = adapter.with_env(&run_env) else {
            if let Err(error) = self.store.cos_run_credential_revoke(run_id, now) {
                tracing::warn!(%error, %run_id, "CoS credential revoke after launch failure failed");
            }
            return Err(format!(
                "CoS unavailable: harness {} cannot receive a run credential",
                pick.harness
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
                    "harness": pick.harness, "llm_source": pick.candidate.source,
                    "account_id": account, "model": pick.candidate.model,
                "selection_metadata": selection_metadata,
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
                pick.harness,
                pick.candidate.source,
                account.as_deref().unwrap_or("unknown"),
                pick.candidate.model
            ),
            now,
        );
        let retries = Arc::clone(&self.retries);
        self.running.insert(
            thread.clone(),
            tokio::spawn(async move {
                let _run_tmp = run_tmp;
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
        self.triage.live_runs.insert(thread_id.to_owned());
        self.route_reservations
            .insert(thread_id.to_owned(), (pick.candidate.source, pick.account));
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
    // ADR 2026-10-05 D2 付記: only a run recorded `completed` moves the delivery cursor. A run
    // that ends otherwise (or never reaches here) leaves it, so the next run redelivers.
    if let Err(error) = advance_delivery_cursor(&store, thread_id, run_id) {
        tracing::warn!(%error, %run_id, "CoS delivery cursor was not advanced");
    }
    if let Err(error) = store.cos_run_credential_revoke(run_id, OffsetDateTime::now_utc()) {
        tracing::warn!(%error, %run_id, "CoS chat credential revoke failed");
    }
}

/// After a run ends, raise its session's delivery cursor (ADR 2026-10-05 D2 付記). Reads the
/// recorded terminal state and session from the DB: nothing moves unless the run is `completed`.
/// The session then holds everything up to its input (earlier messages were in the summary,
/// the history or an earlier run of the session). The cursor also covers the run's own reply when
/// every message between the input and that reply is a user message: those are queued inputs,
/// which are delivered as inputs whatever the cursor says. Returns the new cursor, if moved.
pub(super) fn advance_delivery_cursor(
    store: &SqliteStore,
    thread_id: &str,
    run_id: &str,
) -> Result<Option<i64>, String> {
    let run = store
        .chat_run_get(thread_id, run_id)
        .map_err(|e| e.to_string())?;
    if run.state != ChatRunState::Completed {
        return Ok(None);
    }
    let Some(row_id) = store
        .chat_run_session_record(run_id)
        .map_err(|e| e.to_string())?
        .session_row_id
    else {
        return Ok(None);
    };
    let input_seq = store
        .chat_message_get(thread_id, &run.input_message_id)
        .map_err(|e| e.to_string())?
        .seq;
    let output_seq = match run.output_message_id.as_deref() {
        Some(id) => Some(
            store
                .chat_message_get(thread_id, id)
                .map_err(|e| e.to_string())?
                .seq,
        ),
        None => None,
    };
    let mut through = input_seq;
    if let Some(output_seq) = output_seq.filter(|seq| *seq > input_seq) {
        let mut users_only = true;
        let mut after = input_seq;
        while after + 1 < output_seq && users_only {
            let page = store
                .chat_message_list(
                    thread_id,
                    &ChatMessageQuery {
                        after_seq: Some(after),
                        limit: Some(200),
                        ..ChatMessageQuery::default()
                    },
                )
                .map_err(|e| e.to_string())?;
            let Some(last) = page.items.iter().map(|m| m.seq).max() else {
                break;
            };
            users_only = page
                .items
                .iter()
                .filter(|m| m.seq < output_seq)
                .all(|m| m.role == task_core::chat::ChatMessageRole::User);
            after = last;
        }
        if users_only {
            through = output_seq;
        }
    }
    let through = i64::try_from(through).unwrap_or(i64::MAX);
    store
        .chat_session_set_delivered_through(&row_id, through)
        .map_err(|e| e.to_string())?;
    Ok(Some(through))
}

#[cfg(test)]
mod tests;
