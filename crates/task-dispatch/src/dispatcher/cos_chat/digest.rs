//! ADR 2026-10-07-cos-inbox-thread-conversation D1/D4: the deterministic digest of one triage
//! run, written as an assistant message of the inbox thread after the run ends.
//!
//! S4 (ADR 2026-11-02-ts-control-layer-boundary S4-9): `digest_one` reads a snapshot
//! (`CosDigestInput`), decides with the pure `cos_digest_decide` (task-core), validates the
//! commands, then applies them. The text, cards and the requeue rule live in
//! `task_core::chat::digest`.

use task_core::chat::digest::{
    CosDigestCommand, CosDigestInput, cos_digest_decide, validate_cos_digest_commands,
};
use time::OffsetDateTime;

use super::launch::CosChatLaunch;

pub(crate) use task_core::chat::digest::cos_kind_label as kind_label;

/// Runs digested per pass (bounded work per tick).
const DIGEST_RUNS_PER_PASS: usize = 20;

impl CosChatLaunch {
    /// One digest pass: every finished triage run without a digest gets its message; stopped
    /// and interrupted runs give their unhandled items back to the queue.
    pub(crate) fn triage_digest(&mut self, now: OffsetDateTime) {
        // Keep the reservation until an empty scan succeeds. A bounded pass or any
        // lookup/write failure must continue on the next tick without another run.
        self.triage.digest_due = true;
        let runs = match self
            .store
            .cos_triage_runs_without_digest(DIGEST_RUNS_PER_PASS)
        {
            Ok(runs) => runs,
            Err(error) => {
                tracing::warn!(%error, "CoS triage digest scan failed");
                return;
            }
        };
        if runs.is_empty() {
            self.triage.digest_due = false;
            return;
        }
        for (thread, run_id, run_state) in runs {
            if let Err(error) = self.digest_one(&thread, &run_id, &run_state, now) {
                tracing::warn!(%error, %run_id, "CoS triage digest failed");
            }
        }
    }

    fn digest_one(
        &mut self,
        thread: &str,
        run_id: &str,
        run_state: &str,
        now: OffsetDateTime,
    ) -> Result<(), String> {
        let input = self.digest_snapshot(thread, run_id, run_state)?;
        let commands = cos_digest_decide(&input);
        validate_cos_digest_commands(&input, &commands).map_err(|e| e.to_string())?;
        self.apply_digest_commands(&commands, now)
    }

    /// The snapshot one digest decision reads: the run's items in report order and the chat
    /// run's reason (`None` when the run cannot be read).
    fn digest_snapshot(
        &self,
        thread: &str,
        run_id: &str,
        run_state: &str,
    ) -> Result<CosDigestInput, String> {
        let items = self
            .store
            .cos_triage_run_report(run_id)
            .map_err(|e| e.to_string())?;
        let reason = self
            .store
            .chat_run_get(thread, run_id)
            .ok()
            .and_then(|run| run.reason);
        Ok(CosDigestInput {
            thread: thread.to_owned(),
            run_id: run_id.to_owned(),
            run_state: run_state.to_owned(),
            reason,
            items,
        })
    }

    /// Apply validated commands in order; the first failure stops the rest (a failed message
    /// write leaves the items `running` for the next pass).
    fn apply_digest_commands(
        &mut self,
        commands: &[CosDigestCommand],
        now: OffsetDateTime,
    ) -> Result<(), String> {
        for command in commands {
            match command {
                CosDigestCommand::PostDigest {
                    thread,
                    idempotency_key,
                    text,
                    cards,
                    run_id,
                } => {
                    self.store
                        .chat_assistant_message_add_once(
                            thread,
                            idempotency_key,
                            text,
                            cards,
                            Some(run_id),
                            now,
                        )
                        .map_err(|e| e.to_string())?;
                }
                CosDigestCommand::RequeueRun { run_id } => {
                    let requeued = self
                        .store
                        .cos_triage_requeue_run(run_id, now)
                        .map_err(|e| e.to_string())?;
                    if requeued > 0 {
                        self.triage.dirty = true;
                    }
                }
            }
        }
        Ok(())
    }
}
