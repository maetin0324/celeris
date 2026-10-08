//! Explicit CoS route failover; no model judgment and no new input/run on retry.
use task_core::chat::{ChatRun, ChatRunState, ChatStatusPhase};
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
            self.accounts_in_flight.remove(&retry.thread_id);
            self.providers_in_flight.remove(&retry.thread_id);
            let Ok(run) = self.store.chat_run_get(&retry.thread_id, &retry.run_id) else {
                continue;
            };
            let now = dispatcher.now_utc();
            if run.state == ChatRunState::Stopping {
                let state =
                    if super::control::stop_intent(&run) == super::sink::ChatStopIntent::Stop {
                        ChatRunState::Stopped
                    } else {
                        ChatRunState::Interrupted
                    };
                let _ =
                    self.store
                        .chat_run_finish(&run.id, state, None, run.reason.as_deref(), now);
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
                    Some(&retry.previous),
                    now,
                );
                continue;
            }
            let summary = if retry.next_route <= config.fallbacks.len() {
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
            if retry.next_route > config.fallbacks.len() {
                self.fail_routes(&run, &retry.reason, Some(&retry.previous), now);
            } else if !dispatcher.accepting_new_work || !dispatcher.disk_ready || !config.enabled {
                self.fail_routes(
                    &run,
                    "CoS の再試行を開始できません（停止中・ディスク・設定を確認してください）",
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
        let mut last = "CoS unavailable: no usable route".to_string();
        for index in first..=config.fallbacks.len() {
            let mut cfg = config.clone();
            if index > 0 {
                let route = &config.fallbacks[index - 1];
                cfg.harness.clone_from(&route.harness);
                cfg.llm_source.clone_from(&route.llm_source);
                cfg.provider.clone_from(&route.provider);
                cfg.account_id.clone_from(&route.account_id);
                cfg.model.clone_from(&route.model);
                cfg.tier = route.tier;
                cfg.unavailable_reason.clone_from(&route.unavailable_reason);
            }
            let elapsed = run
                .started_at
                .as_deref()
                .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
                .map_or(0, |start| (now - start).whole_seconds().max(0) as u64);
            cfg.max_wall_secs = cfg.max_wall_secs.saturating_sub(elapsed);
            if cfg.max_wall_secs == 0 {
                last = "CoS run の実行期限に達しました".into();
                break;
            }
            let result = self
                .select_route_account(dispatcher, thread, &cfg)
                .and_then(|account| {
                    self.launch_claimed(
                        dispatcher,
                        &cfg,
                        index,
                        previous.clone(),
                        thread,
                        &run.id,
                        run,
                        account,
                        now,
                    )
                });
            match result {
                Ok(()) => return,
                Err(reason) => {
                    let _ = self.store.cos_run_credential_revoke(&run.id, now);
                    let next = if index < config.fallbacks.len() {
                        "次の代替経路で再試行します。"
                    } else {
                        "再送してください。"
                    };
                    self.route_notice(
                        &run.id,
                        thread,
                        index,
                        "unavailable",
                        &format!(
                            "CoS 経路 {} は利用できません。{next} 理由: {reason}",
                            cfg.harness
                        ),
                        now,
                    );
                    last = reason;
                }
            }
        }
        self.fail_routes(run, &last, previous.as_deref(), now);
    }

    fn fail_routes(
        &mut self,
        run: &ChatRun,
        reason: &str,
        previous: Option<&ChatRunSink>,
        now: OffsetDateTime,
    ) {
        if let Ok(current) = self.store.chat_run_get(&run.thread_id, &run.id)
            && current.state == ChatRunState::Stopping
        {
            let state =
                if super::control::stop_intent(&current) == super::sink::ChatStopIntent::Stop {
                    ChatRunState::Stopped
                } else {
                    ChatRunState::Interrupted
                };
            let _ =
                self.store
                    .chat_run_finish(&run.id, state, None, current.reason.as_deref(), now);
            return;
        }
        if self.triage.inbox_thread.as_deref() == Some(&run.thread_id) {
            self.triage.digest_due = true;
        }
        let result = if let Some(sink) = previous {
            sink.finish(
                &super::sink::ChatFinish {
                    state: ChatRunState::Failed,
                    final_text: Some(failure_text(reason)),
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
                    Some(&failure_text(reason)),
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
