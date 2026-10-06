//! Durable CoS chat stop and ownerless-run reconciliation.
//!
//! The API only records an intent. The dispatcher signals the named worker's
//! process group; the worker task writes the terminal state after it exits.
//! Recovery uses the same daemon-instance evidence as task run takeover.

use std::time::Duration;

use task_core::SqliteStore;
use task_core::chat::{
    ChatActor, ChatCard, ChatCardKind, ChatPostMessageRequest, ChatRun, ChatRunState, ChatSendMode,
    ChatThreadQuery,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::launch::CosChatLaunch;
use super::sink::ChatStopIntent;
use crate::dispatcher::Dispatcher;

pub(super) fn stop_intent(run: &ChatRun) -> ChatStopIntent {
    if run.reason.as_deref() == Some("stopped by human") {
        ChatStopIntent::Stop
    } else {
        ChatStopIntent::Interrupt
    }
}

/// Pause the queue and publish one durable human-review card for a run whose
/// pending operation may have reached an external system.
pub(super) fn pending_review(
    store: &SqliteStore,
    run: &ChatRun,
    now: OffsetDateTime,
) -> Result<(), String> {
    store
        .chat_run_stop(&run.thread_id, &run.id, now)
        .map_err(|e| e.to_string())?;
    let operations = store
        .cos_operation_pending_details_for_run(&run.id)
        .map_err(|e| e.to_string())?;
    let cards = operations
        .into_iter()
        .map(|operation| ChatCard {
            kind: ChatCardKind::Operation,
            id: operation.id.clone(),
            title: format!(
                "Review {} on {}:{}",
                operation.action, operation.target_kind, operation.target_id
            ),
            state: "pending".into(),
            href: format!("/cos/operations/{}", operation.id),
            actor: ChatActor::System,
            reason: Some("external side effect outcome unknown".into()),
            operation_id: Some(operation.id),
        })
        .collect::<Vec<_>>();
    store.chat_system_message_add_once(&run.thread_id,
        &format!("cos-review:{}", run.id),
        "An operation from the interrupted run may have taken effect. Confirm its outcome before resuming the queue.",
        &cards, now).map_err(|e| e.to_string())?;
    Ok(())
}

impl CosChatLaunch {
    /// Inspect the durable state on every tick, including after daemon restart.
    /// A missing local handle alone is never proof that the former owner died.
    pub(super) fn control_hook(&mut self, dispatcher: &Dispatcher) {
        let mut before = None;
        let mut holders_gone = None;
        loop {
            let page = match self.store.chat_thread_list(&ChatThreadQuery {
                before: before.clone(),
                limit: Some(100),
                ..ChatThreadQuery::default()
            }) {
                Ok(page) => page,
                Err(error) => {
                    tracing::warn!(%error, "CoS control could not list threads");
                    return;
                }
            };
            for thread in &page.items {
                let Some(run_id) = thread.active_run_id.as_deref() else {
                    continue;
                };
                let run = match self.store.chat_run_get(&thread.id, run_id) {
                    Ok(run) => run,
                    Err(error) => {
                        tracing::warn!(%error, %run_id, "CoS control could not read run");
                        continue;
                    }
                };
                if let Some(handle) = self.running.get(&thread.id) {
                    if run.state == ChatRunState::Stopping && !handle.is_finished() {
                        // kill_tree is keyed by the exact run id. A late stop for an
                        // older run cannot signal a newer process in this thread.
                        task_worker::kill_tree(run_id, dispatcher.config.kill_grace);
                    }
                    continue;
                }
                if !dispatcher.accepting_new_work || dispatcher.orphan_takeover.is_none() {
                    continue;
                }
                let gone = dispatcher.lease_holders_gone(&mut holders_gone, dispatcher.now_utc());
                let started = run
                    .started_at
                    .as_deref()
                    .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
                    .unwrap_or_else(|| dispatcher.now_utc());
                let ttl = Duration::from_secs(self.config.max_wall_secs)
                    .saturating_add(dispatcher.config.lease_grace);
                if !matches!(
                    crate::orphan::ownerless_run_decision(gone, started, ttl, dispatcher.now_utc()),
                    crate::orphan::OwnerlessRun::Close { .. }
                ) {
                    continue;
                }
                if let Err(error) = self.recover_orphan(&run, dispatcher.now_utc()) {
                    tracing::warn!(%error, %run_id, "CoS orphan recovery failed");
                }
            }
            before = page.next_cursor;
            if before.is_none() {
                break;
            }
        }
    }

    fn recover_orphan(&self, run: &ChatRun, now: OffsetDateTime) -> Result<(), String> {
        let pending = self
            .store
            .cos_operation_pending_for_run(&run.id)
            .map_err(|e| e.to_string())?;
        if pending {
            // The operation may have escaped the process before its result was
            // recorded. Keep the queue paused and ask a human to reconcile it.
            pending_review(&self.store, run, now)?;
            self.store
                .chat_run_finish(
                    &run.id,
                    ChatRunState::Interrupted,
                    None,
                    Some("operation outcome unknown; human review required"),
                    now,
                )
                .map_err(|e| e.to_string())?;
            return Ok(());
        }
        // A human stop and an interrupt already have their intended queue state.
        // The latter's new message was committed in the same transaction as the
        // stop request and remains ahead of ordinary queued messages.
        if run.reason.as_deref() == Some("stopped by human") {
            self.store
                .chat_run_finish(&run.id, ChatRunState::Stopped, None, None, now)
                .map_err(|e| e.to_string())?;
            return Ok(());
        }
        if run.reason.as_deref() == Some("interrupt") {
            self.store
                .chat_run_finish(&run.id, ChatRunState::Interrupted, None, None, now)
                .map_err(|e| e.to_string())?;
            return Ok(());
        }
        let input = self.find_input(&run.thread_id, &run.input_message_id)?;
        let applied = self
            .store
            .cos_operation_applied_for_run(&run.id)
            .map_err(|e| e.to_string())?;
        let receipts = applied
            .iter()
            .take(50)
            .map(|operation| {
                format!(
                    "{} {} {}:{} (idempotency_key={})",
                    operation.id,
                    operation.action,
                    operation.target_kind,
                    operation.target_id,
                    operation.idempotency_key,
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        let more = if applied.len() > 50 {
            format!(
                "; {} more operation receipts remain in the thread audit",
                applied.len() - 50
            )
        } else {
            String::new()
        };
        if !applied.is_empty() {
            let receipts = format!(
                "Applied operations from interrupted run (do not reapply): {receipts}{more}"
            )
            .chars()
            .take(8_000)
            .collect::<String>();
            self.store
                .chat_system_message_add_once(
                    &run.thread_id,
                    &format!("cos-receipts:{}", run.id),
                    &receipts,
                    &[],
                    now,
                )
                .map_err(|e| e.to_string())?;
        }
        // The deterministic client id makes a retry after a crash idempotent.
        // `interrupt` gives this continuation priority over later queue input;
        // the full original input and attachments are delivered unchanged.
        // Partial output and operation receipts remain in DB history.
        self.store
            .chat_message_post(
                &run.thread_id,
                &ChatPostMessageRequest {
                    client_message_id: format!("cos-recover:{}", run.id),
                    text: input.text,
                    attachment_ids: input.attachment_ids,
                    reply_to_id: Some(run.input_message_id.clone()),
                    mode: ChatSendMode::Interrupt,
                    resume_queue: false,
                },
                now,
            )
            .map_err(|e| e.to_string())?;
        self.store
            .chat_run_finish(
                &run.id,
                ChatRunState::Interrupted,
                None,
                Some("orphan takeover; continuing in a new run"),
                now,
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}
