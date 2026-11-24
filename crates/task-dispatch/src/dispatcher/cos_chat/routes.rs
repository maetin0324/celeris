//! Explicit CoS route failover; no model judgment and no new input/run on retry.
//!
//! ADR 2026-11-24-planner-supply-failure-cooldown D5: a route that ended in a supply-side failure
//! puts its `(source, account)` on cooldown with the same owner as worker runs, and the next
//! attempt of the same chat run selects again from all of `cos.route_candidates`. Admission skips
//! the cooled account, so another account of the same source (then the next candidate) runs.
use task_core::accounts::AccountAdapter;
use task_core::chat::{ChatPrelaunchReason, ChatRun, ChatRunState, ChatStatusPhase};
use task_core::llm_route::{AccountId, SourceId};
use task_worker::adapter::AdapterError;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::launch::CosChatLaunch;
use super::sink::ChatRunSink;
use super::slot_wait::{SLOT_WAIT_TEXT, SlotWait};
use crate::dispatcher::Dispatcher;
use crate::dispatcher::route_failure::{provider_failure_outcome, provider_failure_reason};
use crate::policy::ProviderOutcome;
use std::sync::Arc;

/// A supply-side failure of a CoS route (ADR 2026-11-24 D1): the adapter's `Err` classified by
/// the same functions as worker runs. `reason` is the `provider_failure_reason` vocabulary
/// (`spawn` included); `outcome` decides the cooldown length.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SupplyFailure {
    pub reason: &'static str,
    pub outcome: ProviderOutcome,
}

impl SupplyFailure {
    pub(crate) fn of(error: &AdapterError) -> Option<Self> {
        Some(Self {
            reason: provider_failure_reason(error)?,
            outcome: provider_failure_outcome(error)?,
        })
    }
}

/// One launched route of a chat run. `index` counts the attempts of this run (0 is the first);
/// it is not a position in the candidate list. A later attempt starts a fresh session.
#[derive(Debug, Clone)]
pub(super) struct RouteAttempt {
    pub index: usize,
    pub source: SourceId,
    pub account: AccountId,
    /// The account pool of the source (`None` for a source without one).
    pub account_adapter: Option<AccountAdapter>,
}

impl Dispatcher {
    /// Cool down the route that failed, with the owners worker runs use (`worker_finish.rs`): a
    /// pool account goes to the account book (an auth failure is held until re-login and notifies
    /// the human once); a source without a pool, or a spawn failure, cools the source.
    fn cool_down_cos_route(&mut self, route: &RouteAttempt, supply: &SupplyFailure) {
        match route.account_adapter {
            Some(adapter) if supply.reason != "spawn" => self.record_account_failure(
                adapter,
                route.account.as_ref(),
                supply.reason,
                &supply.outcome,
            ),
            _ => {
                let event = self.record_source_failure(
                    route.source.as_ref(),
                    supply.reason,
                    &supply.outcome,
                );
                tracing::info!(?event, source = %route.source, "CoS route source cooled down");
            }
        }
    }
}

pub(super) fn failure_text(reason: &str) -> String {
    format!(
        "CoS が返事できませんでした。理由: {reason}\n利用可能なアカウントの枠・ログイン状態を確認して再送してください。"
    )
}

/// Seconds since the run started; 0 when `started_at` is missing or unreadable.
pub(super) fn run_elapsed(run: &ChatRun, now: OffsetDateTime) -> u64 {
    run.started_at
        .as_deref()
        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
        .map_or(0, |start| (now - start).whole_seconds().max(0) as u64)
}

impl CosChatLaunch {
    pub(super) fn route_notice(
        &self,
        run: &str,
        thread: &str,
        route: usize,
        kind: &str,
        text: &str,
        now: OffsetDateTime,
    ) {
        let _ = self
            .store
            .chat_run_status(run, ChatStatusPhase::Waiting, text, now);
        if let Err(error) = self.store.chat_system_message_add_once(
            thread,
            &format!("cos-route:{run}:{route}:{kind}"),
            text,
            &[],
            now,
        ) {
            tracing::warn!(%error, %run, "CoS route notice failed");
        }
    }

    pub(super) fn retry_routes(&mut self, dispatcher: &mut Dispatcher) {
        // A worker enqueues only after it has exited. Do not race its final task instructions.
        let pending = std::mem::take(&mut *self.retries.lock().unwrap_or_else(|e| e.into_inner()));
        for mut retry in pending {
            if self
                .running
                .get(&retry.thread_id)
                .is_some_and(|h| !h.is_finished())
            {
                self.retries
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(retry);
                continue;
            }
            self.running.remove(&retry.thread_id);
            self.route_reservations.remove(&retry.thread_id);
            // Once per failure, even when the retry waits for capacity or the run was stopped.
            if let Some(supply) = retry.supply.take() {
                dispatcher.cool_down_cos_route(&retry.attempt, &supply);
            }
            let Ok(run) = self.store.chat_run_get(&retry.thread_id, &retry.run_id) else {
                continue;
            };
            let now = dispatcher.now_utc();
            if run.state == ChatRunState::Stopping {
                if let Err(error) = super::control::close_stopped(&self.store, &run, now) {
                    tracing::warn!(%error, run_id = %run.id, "CoS stopped run was not closed");
                }
                continue;
            }
            if run.state != ChatRunState::Running {
                continue;
            }
            let config = self
                .run_configs
                .get(&retry.thread_id)
                .unwrap_or(&self.config)
                .clone();
            if run
                .started_at
                .as_deref()
                .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
                .is_some_and(|start| {
                    (now - start).whole_seconds().max(0) as u64 >= config.max_wall_secs
                })
            {
                self.fail_routes(
                    &run,
                    "CoS run の実行期限に達しました",
                    ChatPrelaunchReason::WallExpiredBeforeLaunch,
                    Some(&retry.previous),
                    now,
                );
                continue;
            }
            let summary = format!(
                "CoS の経路が利用上限・認証で失敗しました。別の経路で再試行します。理由: {}",
                retry.reason
            );
            self.route_notice(
                &run.id,
                &run.thread_id,
                retry.attempt.index,
                "failed",
                &summary,
                now,
            );
            if !dispatcher.coord.accepting_new_work()
                || !dispatcher.maint.disk_ready()
                || !config.enabled
            {
                self.fail_routes(
                    &run,
                    "CoS の再試行を開始できません（停止中・ディスク・設定を確認してください）",
                    ChatPrelaunchReason::RouteUnavailable,
                    Some(&retry.previous),
                    now,
                );
            } else if !self.has_capacity(dispatcher) {
                self.retries
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(retry);
            } else {
                self.launch_routes(
                    dispatcher,
                    &retry.thread_id,
                    &run,
                    retry.attempt.index + 1,
                    Some(retry.previous),
                    now,
                );
            }
        }
    }

    /// Select a route from all CoS candidates and launch attempt `attempt` of `run`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn launch_routes(
        &mut self,
        dispatcher: &mut Dispatcher,
        thread: &str,
        run: &ChatRun,
        attempt: usize,
        previous: Option<Arc<ChatRunSink>>,
        now: OffsetDateTime,
    ) {
        let mut cfg = self.run_configs.get(thread).unwrap_or(&self.config).clone();
        cfg.max_wall_secs = cfg.max_wall_secs.saturating_sub(run_elapsed(run, now));
        if cfg.max_wall_secs == 0 {
            self.fail_routes(
                run,
                "CoS run の実行期限に達しました",
                ChatPrelaunchReason::WallExpiredBeforeLaunch,
                previous.as_deref(),
                now,
            );
            return;
        }
        let input = crate::dispatcher::route_select::RouteInput {
            task_id: run.id.clone(),
            harness: task_core::llm_route::HarnessRequest {
                harness: Some("claude_code".into()),
                ..Default::default()
            },
            cos: true,
            ..Default::default()
        };
        let (outcome, trace) = dispatcher.select_cos_route(&input);
        // ADR 2026-11-06-cos-run-launch-accounting-and-drain D2: 除外理由が全部「枠」なら失敗にせず待つ。
        // 理由は `RouteTrace` の構造化理由で決める（文字列は見ない）。起動済みの run の代替経路は従来どおり。
        if previous.is_none()
            && let Err(failure) = &outcome
            && crate::dispatcher::route_select::slot_only_wait(failure, &trace)
        {
            self.hold_slot_wait(run, thread, now);
            return;
        }
        let metadata = trace.metadata(&input, outcome.as_ref().ok(), outcome.as_ref().err());
        let failover = previous.clone();
        let result = match outcome {
            Err(error) => Err(super::launch::LaunchFailure::from(error.reason())),
            Ok(pick) => self.launch_claimed(
                dispatcher, &cfg, attempt, previous, thread, &run.id, run, pick, metadata, now,
            ),
        };
        if let Err(failure) = result {
            self.route_notice(
                &run.id,
                thread,
                attempt,
                "unavailable",
                &format!("CoS route unavailable: {}", failure.text),
                now,
            );
            self.fail_routes(run, &failure.text, failure.reason, failover.as_deref(), now);
        }
    }

    /// 枠だけで選べなかった run を待ちに入れる。すでに待っている run には何も書かない（tick ごとに event を増やさない）。
    fn hold_slot_wait(&mut self, run: &ChatRun, thread: &str, now: OffsetDateTime) {
        if self.slot_waits.contains_key(&run.id) {
            return;
        }
        self.slot_waits.insert(
            run.id.clone(),
            SlotWait {
                thread_id: thread.to_owned(),
            },
        );
        let _ = self.store.chat_run_status(
            &run.id,
            ChatStatusPhase::Waiting,
            &format!(
                "{SLOT_WAIT_TEXT}: 同時実行数の枠が空くまで待ち、空いたら同じ入力で起動します"
            ),
            now,
        );
    }

    /// Close a run that cannot (or can no longer) start a route. A run whose process never launched
    /// closes with a `prelaunch_reason` (ADR 2026-11-06-cos-run-launch-accounting-and-drain D1), so the
    /// budget does not count it as an unknown cost. A run that launched an earlier route keeps its
    /// known partial usage and is never marked prelaunch.
    fn fail_routes(
        &mut self,
        run: &ChatRun,
        reason: &str,
        prelaunch: ChatPrelaunchReason,
        previous: Option<&ChatRunSink>,
        now: OffsetDateTime,
    ) {
        let current = self.store.chat_run_get(&run.thread_id, &run.id).ok();
        if let Some(current) = current.as_ref()
            && current.state == ChatRunState::Stopping
        {
            if let Err(error) = super::control::close_stopped(&self.store, current, now) {
                tracing::warn!(%error, run_id = %run.id, "CoS stopped run was not closed");
            }
            return;
        }
        self.triage.digest_due = true;
        let launched = current.as_ref().unwrap_or(run).launched_at.is_some();
        let text = failure_text(reason);
        let result = if !launched {
            // The reader still sees why the CoS did not answer (the prelaunch close has no final text).
            if let Err(error) = self.store.chat_run_append_text(&run.id, &text, now) {
                tracing::debug!(%error, run_id = %run.id, "CoS failure text was not appended");
            }
            match self.store.chat_run_finish_prelaunch(
                &run.id,
                ChatRunState::Failed,
                prelaunch,
                Some(reason),
                now,
            ) {
                // A row from before launch tracking has no recordable boundary: close it plainly.
                Err(task_core::chat::ChatError::Conflict(_)) => self.store.chat_run_finish(
                    &run.id,
                    ChatRunState::Failed,
                    None,
                    Some(reason),
                    now,
                ),
                other => other,
            }
            .map(|_| ())
        } else if let Some(sink) = previous {
            sink.finish(
                &super::sink::ChatFinish {
                    state: ChatRunState::Failed,
                    final_text: Some(text),
                    reason: Some(reason.into()),
                    context_exhausted: false,
                    supply: None,
                },
                &task_worker::result_report::ParsedActions::default(),
            )
            .map(|_| ())
        } else {
            self.store
                .chat_run_finish(
                    &run.id,
                    ChatRunState::Failed,
                    Some(&text),
                    Some(reason),
                    now,
                )
                .map(|_| ())
        };
        if let Err(error) = result {
            tracing::warn!(%error, run_id = %run.id, "CoS unavailable finish failed");
        }
        let _ = self.store.cos_run_credential_revoke(&run.id, now);
    }
}
