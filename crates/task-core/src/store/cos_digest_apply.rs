//! ADR 2026-11-03-cos-digest-persist: a digest pass in one transaction, and the outbox of the
//! `cos_digest` compare shadow (migration 0107).
//!
//! - `cos_digest_apply_pass`: `BEGIN IMMEDIATE`, the S4-14 mode check of a TS output, then per run
//!   a SAVEPOINT with the message, the requeue and the record + script-guard count (or the outbox
//!   row). A failed run rolls back to its SAVEPOINT; an unverified effect (S5-6) rolls back its run
//!   and stops the pass. The test hook can drop the whole transaction at any stage.
//! - `cos_digest_shadow_finish`: delete the outbox row and append its record in one transaction;
//!   a row that is already gone writes nothing (one record per run).

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use time::OffsetDateTime;

use super::script_domains::read_mode_tx;
use super::script_guard::observe_cos_digest_record_tx;
use super::{SqliteStore, StoreError, format_rfc3339};
use crate::chat::ChatError;
use crate::chat::ChatMessageRole;
use crate::chat::digest::CosDigestCommand;
use crate::chat::digest_apply::{
    CosDigestApplyError, CosDigestApplyPass, CosDigestApplyRun, CosDigestApplyStage,
    CosDigestPassApplied, CosDigestRecordPlan, CosDigestRunApply,
};
use crate::chat::store::{CHAT_CLIENT_KEY_MAX_BYTES, DaemonMessage, daemon_message_add_keyed_tx};
use crate::chat::triage::requeue_run_tx;
use crate::script_compare::{
    CosDigestApplied, CosDigestCompareRecord, ScriptDomain, ScriptDomainError,
};
use crate::script_guard::ScriptGuardOutcome;

/// The test hook of [`SqliteStore::cos_digest_apply_pass_with_hook`].
pub type CosDigestApplyHook<'h> = dyn FnMut(CosDigestApplyStage<'_>) -> Result<(), String> + 'h;

/// Why one run stopped.
enum RunError {
    Failed(String),
    Unverified(String),
    Interrupted(String),
}

impl SqliteStore {
    /// Apply one digest pass in one transaction (ADR 2026-11-03-cos-digest-persist).
    pub fn cos_digest_apply_pass(
        &self,
        pass: &CosDigestApplyPass,
        now: OffsetDateTime,
    ) -> Result<CosDigestPassApplied, CosDigestApplyError> {
        self.cos_digest_apply_pass_with_hook(pass, now, &mut |_| Ok(()))
    }

    /// [`Self::cos_digest_apply_pass`] calling `hook` at each stage. A hook error drops the
    /// transaction without a commit (nothing of the pass is stored), like a process that dies there.
    pub fn cos_digest_apply_pass_with_hook(
        &self,
        pass: &CosDigestApplyPass,
        now: OffsetDateTime,
        hook: &mut CosDigestApplyHook<'_>,
    ) -> Result<CosDigestPassApplied, CosDigestApplyError> {
        let store_err = |e: &dyn std::fmt::Display| CosDigestApplyError::Store(e.to_string());
        let mut conn = self.lock().map_err(|e| store_err(&e))?;
        let mut tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| store_err(&e))?;
        hook(CosDigestApplyStage::Begin).map_err(CosDigestApplyError::Interrupted)?;
        if let Some(expect) = &pass.expect_mode {
            let row = read_mode_tx(&tx, ScriptDomain::CosDigest).map_err(|e| store_err(&e))?;
            match row {
                Some(r) if (r.mode, r.generation_id.clone(), r.version) == *expect => {}
                Some(r) => {
                    return Err(CosDigestApplyError::ModeChanged(format!(
                        "cos_digest mode changed to {} (version {}) during the pass",
                        r.mode.as_str(),
                        r.version
                    )));
                }
                None => {
                    return Err(CosDigestApplyError::ModeChanged(
                        "cos_digest mode row is gone".to_owned(),
                    ));
                }
            }
        }
        let mut out = CosDigestPassApplied::default();
        for run in &pass.runs {
            let savepoint = tx.savepoint().map_err(|e| store_err(&e))?;
            match apply_run_tx(&savepoint, pass, run, now, hook) {
                Ok(applied) => {
                    savepoint.commit().map_err(|e| store_err(&e))?;
                    out.runs.push((run.run_id.clone(), applied));
                }
                // Dropping the SAVEPOINT rolls it back: nothing of this run stays.
                Err(RunError::Failed(error)) => {
                    drop(savepoint);
                    out.runs
                        .push((run.run_id.clone(), CosDigestRunApply::Failed(error)));
                }
                Err(RunError::Unverified(detail)) => {
                    drop(savepoint);
                    out.unverified = Some(detail);
                    break;
                }
                Err(RunError::Interrupted(e)) => return Err(CosDigestApplyError::Interrupted(e)),
            }
        }
        hook(CosDigestApplyStage::BeforeCommit).map_err(CosDigestApplyError::Interrupted)?;
        tx.commit().map_err(|e| store_err(&e))?;
        Ok(out)
    }

    /// The compare shadow of (`pass_id`, `run_id`) ended: delete its outbox row and append
    /// `record` (or, for `None`, the `unavailable` record the row holds) with its count, in one
    /// transaction. `Ok(None)`: the row was already closed and nothing was written.
    pub fn cos_digest_shadow_finish(
        &self,
        pass_id: &str,
        run_id: &str,
        record: Option<&CosDigestCompareRecord>,
        now: OffsetDateTime,
    ) -> Result<Option<(bool, Option<ScriptGuardOutcome>)>, ScriptDomainError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored: Option<String> = tx
            .query_row(
                "SELECT json FROM cos_digest_shadow_outbox WHERE pass_id = ?1 AND run_id = ?2",
                params![pass_id, run_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(stored) = stored else {
            tx.commit()?;
            return Ok(None);
        };
        let record = match record {
            Some(r) => r.clone(),
            None => serde_json::from_str::<CosDigestCompareRecord>(&stored)?,
        };
        tx.execute(
            "DELETE FROM cos_digest_shadow_outbox WHERE pass_id = ?1 AND run_id = ?2",
            params![pass_id, run_id],
        )?;
        let appended = Self::script_compare_log_append_tx(&tx, &record, now)?;
        let outcome = observe_cos_digest_record_tx(&tx, &record, appended, false, now)?;
        tx.commit()?;
        Ok(Some((appended, outcome)))
    }

    /// The open outbox rows `(pass_id, run_id)`, oldest first.
    pub fn cos_digest_shadow_pending(&self) -> Result<Vec<(String, String)>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT pass_id, run_id FROM cos_digest_shadow_outbox ORDER BY created_at, pass_id, run_id",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
    }
}

fn apply_run_tx(
    conn: &Connection,
    pass: &CosDigestApplyPass,
    run: &CosDigestApplyRun,
    now: OffsetDateTime,
    hook: &mut CosDigestApplyHook<'_>,
) -> Result<CosDigestRunApply, RunError> {
    let mut applied = CosDigestApplied {
        message_id: String::new(),
        requeued: 0,
    };
    for command in &run.commands {
        match command {
            CosDigestCommand::PostDigest {
                thread,
                idempotency_key,
                text,
                cards,
                run_id,
            } => {
                if idempotency_key.len() > CHAT_CLIENT_KEY_MAX_BYTES {
                    return Err(RunError::Failed(format!(
                        "post_digest {idempotency_key}: assistant message key too large"
                    )));
                }
                let message = daemon_message_add_keyed_tx(
                    conn,
                    DaemonMessage {
                        thread_id: thread,
                        role: ChatMessageRole::Assistant,
                        key: Some(idempotency_key),
                        text,
                        cards,
                        run_id: Some(run_id),
                    },
                    now,
                )
                .map_err(|e| match e {
                    // S5-6: the key is stored with another text or cards: the record disagrees.
                    ChatError::Conflict(_) if pass.verify => {
                        RunError::Unverified(format!("post_digest {idempotency_key}: {e}"))
                    }
                    e => RunError::Failed(format!("post_digest {idempotency_key}: {e}")),
                })?;
                if pass.verify
                    && (message.thread_id != *thread || message.run_id.as_deref() != Some(run_id))
                {
                    return Err(RunError::Unverified(format!(
                        "post_digest {idempotency_key}: the stored message {} belongs to {}/{}",
                        message.id,
                        message.thread_id,
                        message.run_id.as_deref().unwrap_or("-")
                    )));
                }
                applied.message_id = message.id;
                hook(CosDigestApplyStage::AfterCommand {
                    run_id: &run.run_id,
                    kind: "post_digest",
                })
                .map_err(RunError::Interrupted)?;
            }
            CosDigestCommand::RequeueRun { run_id } => {
                let requeued = requeue_run_tx(conn, run_id, now)
                    .map_err(|e| RunError::Failed(format!("requeue_run {run_id}: {e}")))?;
                applied.requeued = u64::try_from(requeued).unwrap_or(u64::MAX);
                hook(CosDigestApplyStage::AfterCommand {
                    run_id: &run.run_id,
                    kind: "requeue_run",
                })
                .map_err(RunError::Interrupted)?;
            }
        }
    }
    let failed = |e: &dyn std::fmt::Display| RunError::Failed(format!("record: {e}"));
    let (record_appended, guard) = match &run.record {
        None => (false, None),
        Some(CosDigestRecordPlan::Append { record, ts_side }) => {
            let mut record = (**record).clone();
            record.applied = applied.clone();
            let appended = SqliteStore::script_compare_log_append_tx(conn, &record, now)
                .map_err(|e| failed(&e))?;
            let guard = observe_cos_digest_record_tx(conn, &record, appended, *ts_side, now)
                .map_err(|e| failed(&e))?;
            (appended, guard)
        }
        Some(CosDigestRecordPlan::Shadow { lost }) => {
            let mut lost = (**lost).clone();
            lost.applied = applied.clone();
            let json = serde_json::to_string(&lost).map_err(|e| failed(&e))?;
            let at = format_rfc3339(now).map_err(|e| failed(&e))?;
            conn.execute(
                "INSERT OR IGNORE INTO cos_digest_shadow_outbox (pass_id, run_id, thread_id, json, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![pass.pass_id, run.run_id, run.thread_id, json, at],
            )
            .map_err(|e| failed(&e))?;
            (false, None)
        }
    };
    hook(CosDigestApplyStage::AfterRecord {
        run_id: &run.run_id,
    })
    .map_err(RunError::Interrupted)?;
    Ok(CosDigestRunApply::Applied {
        applied,
        record_appended,
        guard,
    })
}
