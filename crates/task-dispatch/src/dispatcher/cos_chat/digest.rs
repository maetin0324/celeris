//! ADR 2026-10-07-cos-inbox-thread-conversation D1/D4: the deterministic digest of one triage
//! run, written as an assistant message of the inbox thread after the run ends.
//!
//! S4 (ADR 2026-11-02-ts-control-layer-boundary S4-9): `digest_one` reads a snapshot
//! (`CosDigestInput`), decides with the pure `cos_digest_decide` (task-core), validates the
//! commands, then applies them. The text, cards and the requeue rule live in
//! `task_core::chat::digest`.
//!
//! The pass returns what it applied (`CosDigestApplication`); the launch tick hands it to
//! `Dispatcher::compare_cos_digest`, which calls TS only in mode `compare` (S4-4). The applied
//! commands are always Rust's.

use task_core::chat::digest::{
    CosDigestCommand, CosDigestInput, cos_digest_decide, validate_cos_digest_commands,
};
use task_core::script_compare::CosDigestApplied;
use time::OffsetDateTime;

use super::launch::CosChatLaunch;
use crate::dispatcher::dispatch_run::CosDigestApplication;

pub(crate) use task_core::chat::digest::cos_kind_label as kind_label;

/// Runs digested per pass (bounded work per tick).
const DIGEST_RUNS_PER_PASS: usize = 20;

impl CosChatLaunch {
    /// One digest pass: every finished triage run without a digest gets its message; stopped
    /// and interrupted runs give their unhandled items back to the queue.
    /// Returns the runs whose commands were applied (for the compare of S4-4).
    pub(crate) fn triage_digest(&mut self, now: OffsetDateTime) -> Vec<CosDigestApplication> {
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
                return Vec::new();
            }
        };
        if runs.is_empty() {
            self.triage.digest_due = false;
            return Vec::new();
        }
        let mut applied = Vec::new();
        for (thread, run_id, run_state) in runs {
            match self.digest_one(&thread, &run_id, &run_state, now) {
                Ok(digest) => applied.push(digest),
                Err(error) => tracing::warn!(%error, %run_id, "CoS triage digest failed"),
            }
        }
        applied
    }

    fn digest_one(
        &mut self,
        thread: &str,
        run_id: &str,
        run_state: &str,
        now: OffsetDateTime,
    ) -> Result<CosDigestApplication, String> {
        let input = self.digest_snapshot(thread, run_id, run_state)?;
        let commands = cos_digest_decide(&input);
        validate_cos_digest_commands(&input, &commands).map_err(|e| e.to_string())?;
        let applied = self.apply_digest_commands(&commands, now)?;
        Ok(CosDigestApplication {
            input,
            commands,
            applied,
        })
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
    ) -> Result<CosDigestApplied, String> {
        let mut applied = CosDigestApplied {
            message_id: String::new(),
            requeued: 0,
        };
        for command in commands {
            match command {
                CosDigestCommand::PostDigest {
                    thread,
                    idempotency_key,
                    text,
                    cards,
                    run_id,
                } => {
                    let message = self
                        .store
                        .chat_assistant_message_add_once(
                            thread,
                            idempotency_key,
                            text,
                            cards,
                            Some(run_id),
                            now,
                        )
                        .map_err(|e| e.to_string())?;
                    applied.message_id = message.id;
                }
                CosDigestCommand::RequeueRun { run_id } => {
                    let requeued = self
                        .store
                        .cos_triage_requeue_run(run_id, now)
                        .map_err(|e| e.to_string())?;
                    if requeued > 0 {
                        self.triage.dirty = true;
                    }
                    applied.requeued = u64::try_from(requeued).unwrap_or(u64::MAX);
                }
            }
        }
        Ok(applied)
    }
}
