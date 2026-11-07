//! Explicit CoS route failover; no model judgment and no new input/run on retry.
use task_core::chat::{ChatPrelaunchReason, ChatRun, ChatRunState, ChatStatusPhase};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::launch::CosChatLaunch;
use super::sink::ChatRunSink;
use crate::dispatcher::Dispatcher;
use std::sync::Arc;

pub(super) fn supply_failure(reason: &str) -> bool {
    reason.contains("provider exhausted:")
        || reason.contains("provider throttled")
        || reason.contains("provider auth failed:")
        || task_worker::provider::classify_provider_failure(reason).is_some()
}

pub(super) fn failure_text(reason: &str) -> String {
    format!(
        "CoS が返事できませんでした。理由: {reason}\n利用可能なアカウントの枠・ログイン状態を確認して再送してください。"
    )
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
        for retry in pending {
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
            let summary = if retry.next_route == 0 {
                format!(
                    "CoS の経路が利用上限・認証で失敗しました。次の代替経路で再試行します。理由: {}",
                    retry.reason
                )
            } else {
                format!(
                    "CoS の利用可能な経路がなくなりました。理由: {}",
                    retry.reason
                )
            };
            self.route_notice(
                &run.id,
                &run.thread_id,
                retry.next_route - 1,
                "failed",
                &summary,
                now,
            );
            if !dispatcher.accepting_new_work || !dispatcher.disk_ready || !config.enabled {
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
                    retry.next_route,
                    Some(retry.previous),
                    now,
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn launch_routes(
        &mut self,
        dispatcher: &mut Dispatcher,
        thread: &str,
        run: &ChatRun,
        first: usize,
        previous: Option<Arc<ChatRunSink>>,
        now: OffsetDateTime,
    ) {
        let config = self.run_configs.get(thread).unwrap_or(&self.config).clone();
        let elapsed = run
            .started_at
            .as_deref()
            .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
            .map_or(0, |start| (now - start).whole_seconds().max(0) as u64);
        let mut cfg = config;
        cfg.max_wall_secs = cfg.max_wall_secs.saturating_sub(elapsed);
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
        let (outcome, trace) = dispatcher.select_cos_route_from(&input, first);
        let metadata = trace.metadata(&input, outcome.as_ref().ok(), outcome.as_ref().err());
        let failover = previous.clone();
        let result = match outcome {
            Err(error) => Err(super::launch::LaunchFailure::from(error.reason())),
            Ok(pick) => self.launch_claimed(
                dispatcher, &cfg, first, previous, thread, &run.id, run, pick, metadata, now,
            ),
        };
        if let Err(failure) = result {
            self.route_notice(
                &run.id,
                thread,
                first,
                "unavailable",
                &format!("CoS route unavailable: {}", failure.text),
                now,
            );
            self.fail_routes(run, &failure.text, failure.reason, failover.as_deref(), now);
        }
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
