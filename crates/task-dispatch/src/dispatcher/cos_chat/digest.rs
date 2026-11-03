//! ADR 2026-10-07-cos-inbox-thread-conversation D1/D4: the deterministic digest of each finished
//! triage run, written as an assistant message of the inbox thread after the run ends.
//!
//! ADR 2026-11-02-ts-control-layer-boundary S4-12/S4-14: one digest pass is
//! list → read each run → decide → validate → apply. Every read goes through the pass memo
//! ([`CosDigestPassReads`]): the store is read at most once for the list and once per run. The
//! mode of `cos_digest` is read once at the start of the pass:
//! - `rust`: Rust reads through the memo, decides (`cos_digest_pass_decide`) and applies.
//! - `compare`: the same as `rust`; then the memo is frozen and TS runs the pass on a separate
//!   tokio task, its ops answered only from the frozen memo. Records only.
//! - `ts`: TS runs the pass first (the dispatcher waits up to `COS_DIGEST_PASS_DEADLINE`); its
//!   ops read the store through the memo. Rust reads the runs TS did not read, decides with the
//!   same memo, and validates the TS output (rules 1–9). A TS failure, a timeout, a rule failure
//!   or a mode change before the apply apply Rust's commands for the whole pass.
//!
//! - `canary` (S5-3): the pass id decides by the deterministic hash; a selected pass runs as `ts`,
//!   the others as `compare`. A `ts`/selected pass waits at most the rest of the tick budget (S5-8),
//!   ignores a TS output that read through a denied op (S5-6, never fail open) and checks every
//!   applied command of a TS output against its idempotency key and the stored record; an effect it
//!   cannot check stops the pass, keeps `digest_due` and rolls the domain back to `rust` with
//!   actor `system:script-guard` (`script_effect_unverified`, S5-6).
//!
//! `digest_due`, `dirty`, the bound of 20 runs, the idempotency key and the SQL writes stay in Rust.
//!
//! ADR 2026-11-03-cos-digest-persist: the apply of a pass is one SQLite transaction
//! (`SqliteStore::cos_digest_apply_pass`): per run the message, the requeue and the record with its
//! script-guard count (or, for the compare shadow, an outbox row the shadow or the reconcile closes
//! with exactly one record). A TS output is applied only if the mode row read inside the
//! transaction is the one it was validated under.

use std::sync::Arc;
use std::time::Instant;

use task_core::SqliteStore;
use task_core::chat::digest::{
    CosDigestCommand, CosDigestInput, CosDigestListRunsReply, CosDigestListRunsRequest,
    CosDigestPassOutput, CosDigestPassRead, CosDigestReadRunRequest, CosDigestRunRef,
    cos_digest_pass_decide,
};
use task_core::chat::digest_apply::{
    CosDigestApplyError, CosDigestApplyPass, CosDigestApplyRun, CosDigestPassApplied,
    CosDigestRecordPlan, CosDigestRunApply,
};
use task_core::script_compare::{
    CosDigestApplied, CosDigestAppliedBy, CosDigestCompareRecord, CosDigestFallback,
    CosDigestOpLog, ScriptMode, sha256_digest,
};
use time::OffsetDateTime;

use super::launch::CosChatLaunch;
use crate::dispatcher::Dispatcher;
use crate::dispatcher::cluster::run_cluster_hooks_off_async;
use crate::script_policy::{
    COS_DIGEST_OP_SINK_CAPACITY, COS_DIGEST_PASS_DEADLINE, COS_DIGEST_PASS_MAX_OPS, CosDigestOp,
    CosDigestOpCall, CosDigestOpReply, CosDigestPassCall, CosDigestPassClassified,
    CosDigestPassDraft, CosDigestRunRecord, PolicyScript, TsResult, call_cos_digest_pass,
    classify_cos_digest_pass, ts_isolate_epoch,
};

pub(crate) use task_core::chat::digest::cos_kind_label as kind_label;

/// Runs digested per pass (bounded work per tick).
const DIGEST_RUNS_PER_PASS: usize = 20;

/// One op call the memo answered (the source of the `ops` of the log, S4-15).
#[derive(Debug, Clone)]
pub(crate) struct CosDigestOpEntry {
    /// The run a `read_run` names (`None` for `list_runs`).
    pub run_id: Option<String>,
    pub log: CosDigestOpLog,
}

/// The pass memo (S4-12): the answers in first-read order and the log of the TS op calls.
#[derive(Debug, Default)]
pub(crate) struct CosDigestPassReads {
    list: Option<Result<Vec<CosDigestRunRef>, String>>,
    reads: Vec<(CosDigestRunRef, Result<CosDigestInput, String>)>,
    /// After the apply of mode `compare`: answer only from the memo.
    frozen: bool,
    pub ops: Vec<CosDigestOpEntry>,
    calls: usize,
}

impl CosDigestPassReads {
    /// Rust's list read (through the memo).
    fn list(&mut self, store: &SqliteStore) -> Result<Vec<CosDigestRunRef>, String> {
        if self.list.is_none() {
            self.list = Some(
                store
                    .cos_triage_runs_without_digest(DIGEST_RUNS_PER_PASS)
                    .map(|runs| {
                        runs.into_iter()
                            .map(|(thread, run_id, run_state)| CosDigestRunRef {
                                thread,
                                run_id,
                                run_state,
                            })
                            .collect()
                    })
                    .map_err(|e| e.to_string()),
            );
        }
        self.list.clone().unwrap_or_else(|| Ok(Vec::new()))
    }

    /// Rust's read of a listed run (through the memo).
    fn read(
        &mut self,
        store: &SqliteStore,
        run: &CosDigestRunRef,
    ) -> Result<CosDigestInput, String> {
        if let Some((_, read)) = self.reads.iter().find(|(r, _)| r.run_id == run.run_id) {
            return read.clone();
        }
        let read = digest_snapshot(store, run);
        self.reads.push((run.clone(), read.clone()));
        read
    }

    /// Every listed run with its read, in list order (reads the runs not read yet).
    fn reads_in_list_order(
        &mut self,
        store: &SqliteStore,
        list: &[CosDigestRunRef],
    ) -> Vec<CosDigestPassRead> {
        list.iter()
            .map(|run| CosDigestPassRead {
                run: run.clone(),
                input: self.read(store, run).ok(),
            })
            .collect()
    }

    /// Answer one TS op (S4-13 scope, S4-18 errors). `store` is `None` for a frozen memo.
    fn answer(
        &mut self,
        store: Option<&SqliteStore>,
        op: CosDigestOp,
        request: &serde_json::Value,
    ) -> CosDigestOpReply {
        let started = Instant::now();
        self.calls += 1;
        let (run_id, reply, outcome) = if self.calls > COS_DIGEST_PASS_MAX_OPS {
            (
                None,
                CosDigestOpReply::error("busy", "too many cos_digest ops in this pass"),
                "error",
            )
        } else {
            match op {
                CosDigestOp::ListRuns => self.answer_list(store, request),
                CosDigestOp::ReadRun => self.answer_read(store, request),
            }
        };
        self.ops.push(CosDigestOpEntry {
            run_id,
            log: CosDigestOpLog {
                op: op.as_str().to_owned(),
                request_digest: json_digest(request),
                reply_digest: json_digest(&reply.to_json()),
                outcome: outcome.to_owned(),
                elapsed_us: u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
            },
        });
        reply
    }

    fn answer_list(
        &mut self,
        store: Option<&SqliteStore>,
        request: &serde_json::Value,
    ) -> (Option<String>, CosDigestOpReply, &'static str) {
        if let Err(e) = serde_json::from_value::<CosDigestListRunsRequest>(request.clone()) {
            return (
                None,
                CosDigestOpReply::error("invalid_output", e.to_string()),
                "error",
            );
        }
        let memo = self.list.is_some() || self.frozen;
        let list = match (store, memo) {
            (Some(store), false) => self.list(store),
            _ => match &self.list {
                Some(list) => list.clone(),
                None => {
                    return (
                        None,
                        CosDigestOpReply::error("denied", "list_runs is outside the request scope"),
                        "denied",
                    );
                }
            },
        };
        let outcome = if memo { "memo" } else { "ok" };
        match list {
            Ok(runs) => match serde_json::to_value(CosDigestListRunsReply { runs }) {
                Ok(v) => (None, CosDigestOpReply::Ok(v), outcome),
                Err(e) => (
                    None,
                    CosDigestOpReply::error("exception", e.to_string()),
                    "error",
                ),
            },
            Err(e) => (None, CosDigestOpReply::error("exception", e), "error"),
        }
    }

    fn answer_read(
        &mut self,
        store: Option<&SqliteStore>,
        request: &serde_json::Value,
    ) -> (Option<String>, CosDigestOpReply, &'static str) {
        let request: CosDigestReadRunRequest = match serde_json::from_value(request.clone()) {
            Ok(r) => r,
            Err(e) => {
                return (
                    None,
                    CosDigestOpReply::error("invalid_output", e.to_string()),
                    "error",
                );
            }
        };
        let run_id = Some(request.run_id.clone());
        let listed = match &self.list {
            Some(Ok(runs)) => runs
                .iter()
                .find(|r| r.thread == request.thread && r.run_id == request.run_id)
                .cloned(),
            _ => None,
        };
        let Some(run) = listed else {
            let message = format!(
                "run {}/{} is outside the request scope",
                request.thread, request.run_id
            );
            return (run_id, CosDigestOpReply::error("denied", message), "denied");
        };
        let memo = self.reads.iter().any(|(r, _)| r.run_id == run.run_id);
        let read = match (store, memo) {
            (Some(store), false) => self.read(store, &run),
            (_, true) => self.read_memo(&run),
            (None, false) => {
                let message = format!(
                    "run {}/{} is outside the request scope",
                    request.thread, request.run_id
                );
                return (run_id, CosDigestOpReply::error("denied", message), "denied");
            }
        };
        let outcome = if memo { "memo" } else { "ok" };
        match read.map(|input| serde_json::to_value(input).map_err(|e| e.to_string())) {
            Ok(Ok(v)) => (run_id, CosDigestOpReply::Ok(v), outcome),
            Ok(Err(e)) | Err(e) => (run_id, CosDigestOpReply::error("exception", e), "error"),
        }
    }

    fn read_memo(&self, run: &CosDigestRunRef) -> Result<CosDigestInput, String> {
        self.reads
            .iter()
            .find(|(r, _)| r.run_id == run.run_id)
            .map(|(_, read)| read.clone())
            .unwrap_or_else(|| Err("not read".to_owned()))
    }

    #[cfg(test)]
    pub(crate) fn answer_for_test(
        &mut self,
        store: Option<&SqliteStore>,
        op: CosDigestOp,
        request: serde_json::Value,
    ) -> CosDigestOpReply {
        self.answer(store, op, &request)
    }

    /// The op log of one run: the pass's `list_runs` calls and the run's `read_run` calls.
    fn ops_of(&self, run_id: &str) -> Vec<CosDigestOpLog> {
        self.ops
            .iter()
            .filter(|e| e.run_id.is_none() || e.run_id.as_deref() == Some(run_id))
            .map(|e| e.log.clone())
            .collect()
    }

    /// `denied_scope` once per pass when an op was denied (S4-13).
    fn denied(&self) -> bool {
        self.ops.iter().any(|e| e.log.outcome == "denied")
    }
}

fn json_digest(value: &serde_json::Value) -> String {
    sha256_digest(&serde_json::to_vec(value).unwrap_or_default())
}

/// The snapshot one digest decision reads: the run's items in report order and the chat run's
/// reason (`None` when the run cannot be read).
fn digest_snapshot(store: &SqliteStore, run: &CosDigestRunRef) -> Result<CosDigestInput, String> {
    let items = store
        .cos_triage_run_report(&run.run_id)
        .map_err(|e| e.to_string())?;
    let reason = store
        .chat_run_get(&run.thread, &run.run_id)
        .ok()
        .and_then(|r| r.reason);
    Ok(CosDigestInput {
        thread: run.thread.clone(),
        run_id: run.run_id.clone(),
        run_state: run.run_state.clone(),
        reason,
        items,
    })
}

/// Serve the TS ops from `reads` while the pass call runs; the server stops with the call (the
/// deadline drops the receiver, so no store read happens after it, S4-18).
async fn serve_pass(
    script: Arc<dyn PolicyScript>,
    pass_id: String,
    reads: &mut CosDigestPassReads,
    store: Option<&SqliteStore>,
    deadline: std::time::Duration,
) -> (TsResult, std::time::Duration) {
    let (sink, mut rx) = tokio::sync::mpsc::channel::<CosDigestOpCall>(COS_DIGEST_OP_SINK_CAPACITY);
    let call = call_cos_digest_pass(
        script.as_ref(),
        CosDigestPassCall { pass_id, sink },
        deadline,
    );
    tokio::pin!(call);
    loop {
        tokio::select! {
            result = &mut call => return result,
            Some(op) = rx.recv() => {
                let reply = reads.answer(store, op.op, &op.request);
                let _ = op.reply.send(reply);
            }
        }
    }
}

/// What one pass applied for one run (the input of the compare shadow).
struct AppliedRun {
    input: CosDigestInput,
    rust: Vec<CosDigestCommand>,
    applied: CosDigestApplied,
}

/// The TS side of a mode `compare`/`ts` pass.
struct PassScript {
    draft: CosDigestPassDraft,
    /// `Err`: TS cannot be called (`unavailable`).
    call: Result<(Arc<dyn PolicyScript>, tokio::runtime::Handle), String>,
    /// The mode row at the start of the pass (mode, generation, version).
    row: (ScriptMode, Option<String>, u64),
}

impl Dispatcher {
    /// One digest pass (S4-12): every finished triage run without a digest gets its message;
    /// stopped and interrupted runs give their unhandled items back to the queue.
    pub(crate) fn cos_digest_pass(&mut self, launch: &mut CosChatLaunch, now: OffsetDateTime) {
        // Keep the reservation until an empty scan succeeds. A bounded pass or any
        // lookup/write failure must continue on the next tick without another run.
        launch.triage.digest_due = true;
        let script = self.cos_digest_pass_script();
        let store = launch.store.clone();
        let mut reads = CosDigestPassReads::default();
        let mut ts_result = None;
        let mut budget_fallback = None;
        if let Some(PassScript {
            draft,
            call: Ok((script, _)),
            ..
        }) = script.as_ref().filter(|s| s.draft.canary_selected)
        {
            // S5-8: the pass waits at most the rest of this tick's budget.
            let remaining = self.script_tick_wait_remaining();
            let deadline = remaining.min(COS_DIGEST_PASS_DEADLINE);
            if deadline.is_zero() {
                self.script_tick_budget_hit();
                budget_fallback = Some(CosDigestFallback::new(
                    "tick_budget",
                    "tick wait budget used up before the pass",
                ));
            } else {
                let (script, pass_id, store) =
                    (script.clone(), draft.pass_id.clone(), store.clone());
                let (back, result) = run_cluster_hooks_off_async(move || {
                    block_on_pass(async move {
                        let result =
                            serve_pass(script, pass_id, &mut reads, Some(&store), deadline).await;
                        (reads, result)
                    })
                });
                reads = back;
                if let Some((r, elapsed)) = &result {
                    self.script_tick_charge((*elapsed).min(deadline));
                    if matches!(r, TsResult::DeadlineExceeded(_))
                        && deadline < COS_DIGEST_PASS_DEADLINE
                    {
                        self.script_tick_budget_hit();
                        budget_fallback = Some(CosDigestFallback::new(
                            "tick_budget",
                            "tick wait budget used up during the pass",
                        ));
                    }
                }
                ts_result = result.filter(|_| budget_fallback.is_none());
            }
        }
        let list = match reads.list(&store) {
            Ok(list) => list,
            Err(error) => {
                tracing::warn!(%error, "CoS triage digest scan failed");
                return;
            }
        };
        if list.is_empty() {
            launch.triage.digest_due = false;
            return;
        }
        let pass_reads = reads.reads_in_list_order(&store, &list);
        for read in &pass_reads {
            if read.input.is_none() {
                tracing::warn!(run_id = %read.run.run_id, "CoS triage digest read failed");
            }
        }
        let rust = cos_digest_pass_decide(&pass_reads);
        let Some(script) = script else {
            // Mode `rust`: one transaction, no record.
            let (plan, runs) = apply_plan(
                "",
                &pass_reads,
                &rust,
                &rust,
                false,
                None,
                &PassRecords::None,
            );
            if let Err(error) = self.apply_digest_pass(launch, &plan, runs, now) {
                tracing::warn!(%error, "CoS triage digest pass not applied");
            }
            return;
        };
        match script.draft.canary_selected {
            true => self.ts_side_digest_pass(
                launch,
                script,
                &reads,
                &pass_reads,
                &rust,
                ts_result,
                budget_fallback,
                now,
            ),
            false => self.compare_digest_pass(launch, script, reads, pass_reads, rust, now),
        }
    }

    /// A pass on the TS side (mode `ts`, canary selected): the chosen output, its records and the
    /// script-guard count in one transaction. A mode change caught inside the transaction applies
    /// Rust's commands with fallback `mode_changed` instead (ADR 2026-11-03-cos-digest-persist 2).
    #[allow(clippy::too_many_arguments)]
    fn ts_side_digest_pass(
        &mut self,
        launch: &mut CosChatLaunch,
        script: PassScript,
        reads: &CosDigestPassReads,
        pass_reads: &[CosDigestPassRead],
        rust: &CosDigestPassOutput,
        ts_result: Option<(TsResult, std::time::Duration)>,
        budget_fallback: Option<CosDigestFallback>,
        now: OffsetDateTime,
    ) {
        let classified = match (&script.call, &ts_result, &budget_fallback) {
            (_, _, Some(budget)) => CosDigestPassClassified::unavailable(rust, &budget.message),
            (Ok(_), Some((result, _)), None) => classify_cos_digest_pass(pass_reads, rust, result),
            (Err(reason), _, None) => CosDigestPassClassified::unavailable(rust, reason),
            (Ok(_), None, None) => CosDigestPassClassified::unavailable(rust, "no script result"),
        };
        let mode_changed = self.cos_digest_mode_changed(&script.row);
        // S5-6: a TS output that read through a denied op is never used (no fail-open).
        let denied = reads
            .denied()
            .then(|| CosDigestFallback::new("denied", "a cos_digest op of the pass was denied"));
        let (mut apply, mut applied_by, mut fallback) =
            match (&classified.output, mode_changed, denied, budget_fallback) {
                (_, Some(changed), _, _) => (rust, CosDigestAppliedBy::Rust, Some(changed)),
                (_, None, _, Some(budget)) => (rust, CosDigestAppliedBy::Rust, Some(budget)),
                (Some(_), None, Some(denied), None) => {
                    (rust, CosDigestAppliedBy::Rust, Some(denied))
                }
                (Some(ts), None, None, None) => (ts, CosDigestAppliedBy::Ts, None),
                (None, None, _, None) => {
                    (rust, CosDigestAppliedBy::Rust, classified.fallback.clone())
                }
            };
        let (isolate_epoch, elapsed) = ts_result
            .as_ref()
            .map(|(r, e)| (ts_isolate_epoch(r), *e))
            .unwrap_or_default();
        loop {
            let ts = applied_by == CosDigestAppliedBy::Ts;
            let records = PassRecords::Append(
                RecordInputs {
                    draft: &script.draft,
                    reads,
                    classified: &classified,
                    applied_by,
                    fallback: fallback.clone(),
                    isolate_epoch,
                    elapsed,
                },
                true,
            );
            let (plan, runs) = apply_plan(
                &script.draft.pass_id,
                pass_reads,
                rust,
                apply,
                ts,
                ts.then(|| script.row.clone()),
                &records,
            );
            match self.apply_digest_pass(launch, &plan, runs, now) {
                Err(CosDigestApplyError::ModeChanged(message)) if ts => {
                    // The mode row moved between the check above and the transaction.
                    (apply, applied_by) = (rust, CosDigestAppliedBy::Rust);
                    fallback = Some(CosDigestFallback::new("mode_changed", &message));
                }
                Err(error) => {
                    tracing::warn!(%error, "CoS triage digest pass not applied");
                    return;
                }
                Ok((_, Some(detail))) => {
                    // S5-6: stop, keep `digest_due`, roll back with `system:script-guard`; the
                    // inbox item comes from that event (script-inbox).
                    self.script_rollback(
                        task_core::script_compare::ScriptDomain::CosDigest,
                        script.row.2,
                        &format!("script_effect_unverified: {detail}"),
                    );
                    return;
                }
                Ok((_, None)) => return,
            }
        }
    }

    /// Mode `compare` (and canary's Rust side): Rust's commands and the shadow's outbox rows in one
    /// transaction; then the memo is frozen and TS runs on a separate tokio task, which closes the
    /// rows with its records (ADR 2026-11-03-cos-digest-persist 3).
    fn compare_digest_pass(
        &mut self,
        launch: &mut CosChatLaunch,
        script: PassScript,
        mut reads: CosDigestPassReads,
        pass_reads: Vec<CosDigestPassRead>,
        rust: CosDigestPassOutput,
        now: OffsetDateTime,
    ) {
        let draft = script.draft;
        let call = match script.call {
            // TS cannot be called: the `unavailable` records go with the apply.
            Err(reason) => {
                let classified = CosDigestPassClassified::unavailable(&rust, &reason);
                let inputs = RecordInputs {
                    draft: &draft,
                    reads: &reads,
                    classified: &classified,
                    applied_by: CosDigestAppliedBy::Rust,
                    fallback: None,
                    isolate_epoch: 0,
                    elapsed: std::time::Duration::ZERO,
                };
                let (plan, runs) = apply_plan(
                    &draft.pass_id,
                    &pass_reads,
                    &rust,
                    &rust,
                    false,
                    None,
                    &PassRecords::Append(inputs, false),
                );
                if let Err(error) = self.apply_digest_pass(launch, &plan, runs, now) {
                    tracing::warn!(%error, "CoS triage digest pass not applied");
                }
                return;
            }
            Ok(call) => call,
        };
        let inputs = RecordInputs {
            draft: &draft,
            reads: &reads,
            classified: &CosDigestPassClassified::unavailable(&rust, SHADOW_LOST),
            applied_by: CosDigestAppliedBy::Rust,
            fallback: None,
            isolate_epoch: 0,
            elapsed: std::time::Duration::ZERO,
        };
        let (plan, runs) = apply_plan(
            &draft.pass_id,
            &pass_reads,
            &rust,
            &rust,
            false,
            None,
            &PassRecords::Shadow(inputs),
        );
        let applied = match self.apply_digest_pass(launch, &plan, runs, now) {
            Ok((applied, _)) => applied,
            Err(error) => {
                tracing::warn!(%error, "CoS triage digest pass not applied");
                return;
            }
        };
        if applied.is_empty() {
            return;
        }
        reads.frozen = true;
        let (script, runtime) = call;
        let store = launch.store.clone();
        let ticket = self.script_compare.digest_shadows.ticket(&draft.pass_id);
        self.spawn_compare(
            task_core::script_compare::ScriptDomain::CosDigest,
            &runtime,
            async move {
                let (result, elapsed) = serve_pass(
                    script,
                    draft.pass_id.clone(),
                    &mut reads,
                    None,
                    crate::script_policy::COS_DIGEST_PASS_DEADLINE,
                )
                .await;
                let classified = classify_cos_digest_pass(&pass_reads, &rust, &result);
                let inputs = RecordInputs {
                    draft: &draft,
                    reads: &reads,
                    classified: &classified,
                    applied_by: CosDigestAppliedBy::Rust,
                    fallback: None,
                    isolate_epoch: ts_isolate_epoch(&result),
                    elapsed,
                };
                let mut closed = true;
                for run in &applied {
                    let record = match inputs.record(&run.input, &run.rust, run.applied.clone()) {
                        Some(Ok(r)) => Some(r),
                        Some(Err(error)) => {
                            tracing::warn!(%error, "cos_digest shadow record unbuildable");
                            None
                        }
                        None => None,
                    };
                    let finished = store.cos_digest_shadow_finish(
                        &draft.pass_id,
                        &run.input.run_id,
                        record.as_ref(),
                        OffsetDateTime::now_utc(),
                    );
                    match finished {
                        Ok(Some((_, Some(outcome)))) if !outcome.rolled_back.is_empty() => {
                            tracing::warn!(stop = ?outcome.stop, "script-guard rolled cos_digest back to rust");
                        }
                        Ok(_) => {}
                        Err(error) => {
                            tracing::warn!(%error, "cos_digest shadow record failed");
                            closed = false;
                        }
                    }
                }
                if closed {
                    ticket.finished();
                }
            },
        );
    }

    /// The mode `compare`/`ts` target of this pass (`None`: mode `rust`, no record, no TS).
    fn cos_digest_pass_script(&mut self) -> Option<PassScript> {
        let target = self.script_target(
            task_core::script_compare::ScriptDomain::CosDigest,
            &[ScriptMode::Compare, ScriptMode::Canary, ScriptMode::Ts],
        )?;
        let pass_id = ulid::Ulid::new().to_string();
        // S5-3: the unit of `cos_digest` is the pass.
        let canary_selected = crate::dispatcher::dispatch_run::script_canary_unit_selected(
            &target,
            task_core::script_compare::ScriptDomain::CosDigest,
            &pass_id,
        );
        Some(PassScript {
            draft: CosDigestPassDraft {
                generation: target.generation_id,
                bundle_sha256: target.bundle_sha256,
                host_api: target.host_api,
                mode: target.mode,
                pass_id,
                canary_selected,
                canary_bps: target.canary_bps,
            },
            call: target.call,
            row: (target.mode, target.row_generation, target.version),
        })
    }

    /// S4-14: re-read the mode row right before the apply; any change → `mode_changed`.
    fn cos_digest_mode_changed(
        &self,
        row: &(ScriptMode, Option<String>, u64),
    ) -> Option<CosDigestFallback> {
        let now = self
            .script_compare_log_store()
            .map(|s| s.script_domain_modes())
            .transpose()
            .map(|rows| {
                rows.and_then(|rows| {
                    rows.into_iter()
                        .find(|r| r.domain == task_core::script_compare::ScriptDomain::CosDigest)
                })
            });
        match now {
            Ok(Some(r)) if (r.mode, r.generation_id.clone(), r.version) == *row => None,
            Ok(Some(r)) => Some(CosDigestFallback::new(
                "mode_changed",
                &format!(
                    "cos_digest mode changed to {} (version {}) during the pass",
                    r.mode.as_str(),
                    r.version
                ),
            )),
            Ok(None) => Some(CosDigestFallback::new(
                "mode_changed",
                "cos_digest mode row is gone",
            )),
            Err(e) => Some(CosDigestFallback::new(
                "mode_changed",
                &format!("cos_digest mode unreadable before the apply: {e}"),
            )),
        }
    }
}

/// The record of one run before the apply (`applied` is filled in by the store).
struct RecordInputs<'a> {
    draft: &'a CosDigestPassDraft,
    reads: &'a CosDigestPassReads,
    classified: &'a CosDigestPassClassified,
    applied_by: CosDigestAppliedBy,
    fallback: Option<CosDigestFallback>,
    isolate_epoch: u64,
    elapsed: std::time::Duration,
}

impl RecordInputs<'_> {
    /// The record of `input` (`None`: the classification has no row for the run).
    fn record(
        &self,
        input: &CosDigestInput,
        rust: &[CosDigestCommand],
        applied: CosDigestApplied,
    ) -> Option<Result<CosDigestCompareRecord, serde_json::Error>> {
        let mut c = self.classified.run(&input.run_id)?.clone();
        if self.reads.denied() && !c.safety_violations.iter().any(|v| v == "denied_scope") {
            c.safety_violations.push("denied_scope".to_owned());
        }
        Some(self.draft.record(
            CosDigestRunRecord {
                input,
                rust,
                classified: &c,
                applied_by: self.applied_by,
                fallback: self.fallback.clone(),
                applied,
                ops: self.reads.ops_of(&input.run_id),
            },
            self.isolate_epoch,
            self.elapsed,
        ))
    }
}

/// How the runs of a pass are recorded.
enum PassRecords<'a> {
    /// Mode `rust`: no record.
    None,
    /// Append each record (and count it) in the apply transaction.
    Append(RecordInputs<'a>, bool),
    /// Mode `compare` / canary's Rust side: an outbox row for the shadow (ADR
    /// 2026-11-03-cos-digest-persist 3).
    Shadow(RecordInputs<'a>),
}

/// The record a shadow that never finishes leaves (outcome `unavailable`, not counted).
const SHADOW_LOST: &str = "compare shadow lost before the TS pass finished";

/// Build the pass to apply: the runs of `apply` with a readable input, in order.
fn apply_plan(
    pass_id: &str,
    reads: &[CosDigestPassRead],
    rust: &CosDigestPassOutput,
    apply: &CosDigestPassOutput,
    verify: bool,
    expect_mode: Option<(ScriptMode, Option<String>, u64)>,
    records: &PassRecords<'_>,
) -> (CosDigestApplyPass, Vec<AppliedRun>) {
    let mut plan = CosDigestApplyPass {
        pass_id: pass_id.to_owned(),
        verify,
        expect_mode,
        runs: Vec::new(),
    };
    let mut inputs = Vec::new();
    let empty = CosDigestApplied {
        message_id: String::new(),
        requeued: 0,
    };
    for run in &apply.runs {
        let Some(input) = reads
            .iter()
            .find(|r| r.run.run_id == run.run_id)
            .and_then(|r| r.input.clone())
        else {
            continue;
        };
        let rust_commands = rust
            .runs
            .iter()
            .find(|r| r.run_id == run.run_id)
            .map(|r| r.commands.clone())
            .unwrap_or_default();
        let record = match records {
            PassRecords::None => Ok(None),
            PassRecords::Append(r, ts_side) => r
                .record(&input, &rust_commands, empty.clone())
                .transpose()
                .map(|rec| {
                    rec.map(|record| CosDigestRecordPlan::Append {
                        record: Box::new(record),
                        ts_side: *ts_side,
                    })
                }),
            PassRecords::Shadow(r) => {
                let lost = CosDigestPassClassified::unavailable(rust, SHADOW_LOST);
                RecordInputs {
                    classified: &lost,
                    fallback: None,
                    isolate_epoch: 0,
                    elapsed: std::time::Duration::ZERO,
                    ..*r
                }
                .record(&input, &rust_commands, empty.clone())
                .transpose()
                .map(|rec| {
                    rec.map(|lost| CosDigestRecordPlan::Shadow {
                        lost: Box::new(lost),
                    })
                })
            }
        };
        let record = match record {
            Ok(record) => record,
            Err(error) => {
                // No record means no apply: the run stays listed for the next pass.
                tracing::warn!(%error, run_id = %run.run_id, "CoS triage digest record unbuildable; run skipped");
                continue;
            }
        };
        plan.runs.push(CosDigestApplyRun {
            thread_id: input.thread.clone(),
            run_id: run.run_id.clone(),
            commands: run.commands.clone(),
            record,
        });
        inputs.push(AppliedRun {
            input,
            rust: rust_commands,
            applied: empty.clone(),
        });
    }
    (plan, inputs)
}

/// The compare shadows of this process (ADR 2026-11-03-cos-digest-persist 3): the passes whose
/// TS task may still close its outbox rows, and whether the outbox needs a reconcile.
#[derive(Debug, Default)]
pub(crate) struct CosDigestShadows {
    live: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// `false` at start and whenever a shadow ended without closing its rows.
    clean: Arc<std::sync::atomic::AtomicBool>,
}

/// A live shadow; dropping it before [`ShadowTicket::finished`] asks for a reconcile.
struct ShadowTicket {
    pass_id: String,
    live: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    clean: Arc<std::sync::atomic::AtomicBool>,
    finished: bool,
}

impl ShadowTicket {
    fn finished(mut self) {
        self.finished = true;
    }
}

impl Drop for ShadowTicket {
    fn drop(&mut self) {
        if let Ok(mut live) = self.live.lock() {
            live.remove(&self.pass_id);
        }
        if !self.finished {
            self.clean.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

impl CosDigestShadows {
    /// Ask the next tick to reconcile the outbox.
    #[cfg(test)]
    pub(crate) fn force_reconcile_for_test(&self) {
        self.clean.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    fn ticket(&self, pass_id: &str) -> ShadowTicket {
        if let Ok(mut live) = self.live.lock() {
            live.insert(pass_id.to_owned());
        }
        ShadowTicket {
            pass_id: pass_id.to_owned(),
            live: self.live.clone(),
            clean: self.clean.clone(),
            finished: false,
        }
    }

    /// Close the outbox rows of passes no task of this process will close (after a restart, a
    /// dropped or failed shadow) with the `unavailable` record each row holds.
    pub(crate) fn reconcile(&self, store: &SqliteStore) {
        use std::sync::atomic::Ordering;
        if self.clean.swap(true, Ordering::SeqCst) {
            return;
        }
        let pending = match store.cos_digest_shadow_pending() {
            Ok(p) => p,
            Err(error) => {
                tracing::warn!(%error, "cos_digest shadow outbox unreadable");
                self.clean.store(false, Ordering::SeqCst);
                return;
            }
        };
        for (pass_id, run_id) in pending {
            let live = self
                .live
                .lock()
                .map(|l| l.contains(&pass_id))
                .unwrap_or(false);
            if live {
                continue;
            }
            if let Err(error) =
                store.cos_digest_shadow_finish(&pass_id, &run_id, None, OffsetDateTime::now_utc())
            {
                tracing::warn!(%error, %pass_id, %run_id, "cos_digest shadow record failed");
                self.clean.store(false, Ordering::SeqCst);
            }
        }
    }
}

/// Run the mode `ts` pass on its own current-thread runtime (the tick is synchronous; the op
/// server needs the store, which stays on this side and never reaches the V8 thread).
fn block_on_pass<F>(pass: F) -> (CosDigestPassReads, Option<(TsResult, std::time::Duration)>)
where
    F: std::future::Future<Output = (CosDigestPassReads, (TsResult, std::time::Duration))>,
{
    match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => {
            let (reads, result) = runtime.block_on(pass);
            (reads, Some(result))
        }
        Err(error) => {
            tracing::warn!(%error, "no runtime for the cos_digest pass; Rust applies");
            (CosDigestPassReads::default(), None)
        }
    }
}

/// The test hook of the apply transaction (ADR 2026-11-03-cos-digest-persist 5).
#[cfg(test)]
type TestApplyHook =
    Box<dyn FnMut(task_core::chat::digest_apply::CosDigestApplyStage<'_>) -> Result<(), String>>;

/// Runs right before the apply transaction starts (a mode CAS of another actor lands there).
#[cfg(test)]
type TestBeforeApply = Box<dyn FnMut(&SqliteStore)>;

#[cfg(test)]
thread_local! {
    static APPLY_HOOK: std::cell::RefCell<Option<TestApplyHook>> =
        std::cell::RefCell::new(None);
    static BEFORE_APPLY: std::cell::RefCell<Option<TestBeforeApply>> =
        std::cell::RefCell::new(None);
}

/// Set (or clear) the apply hook of this thread's digest passes.
#[cfg(test)]
pub(crate) fn set_test_apply_hook(hook: Option<TestApplyHook>) {
    APPLY_HOOK.with(|h| *h.borrow_mut() = hook);
}

/// Set (or clear) the hook that runs before each apply transaction of this thread.
#[cfg(test)]
pub(crate) fn set_test_before_apply(hook: Option<TestBeforeApply>) {
    BEFORE_APPLY.with(|h| *h.borrow_mut() = hook);
}

fn apply_pass_tx(
    store: &SqliteStore,
    plan: &CosDigestApplyPass,
    now: OffsetDateTime,
) -> Result<CosDigestPassApplied, CosDigestApplyError> {
    #[cfg(test)]
    {
        BEFORE_APPLY.with(|h| {
            if let Some(hook) = h.borrow_mut().as_mut() {
                hook(store);
            }
        });
        let hooked = APPLY_HOOK.with(|h| {
            h.borrow_mut()
                .as_mut()
                .map(|hook| store.cos_digest_apply_pass_with_hook(plan, now, hook))
        });
        if let Some(result) = hooked {
            return result;
        }
    }
    store.cos_digest_apply_pass(plan, now)
}

impl Dispatcher {
    /// Apply one pass in one transaction; returns the applied runs (with the store's `applied`)
    /// and the S5-6 stop. `Err`: nothing of the pass was written (`digest_due` stays).
    fn apply_digest_pass(
        &mut self,
        launch: &mut CosChatLaunch,
        plan: &CosDigestApplyPass,
        mut runs: Vec<AppliedRun>,
        now: OffsetDateTime,
    ) -> Result<(Vec<AppliedRun>, Option<String>), CosDigestApplyError> {
        let result = apply_pass_tx(&launch.store, plan, now)?;
        let mut done = Vec::new();
        let mut rolled_back = false;
        for (run_id, outcome) in result.runs {
            match outcome {
                CosDigestRunApply::Applied { applied, guard, .. } => {
                    if applied.requeued > 0 {
                        launch.triage.dirty = true;
                    }
                    if let Some(g) = guard.filter(|g| !g.rolled_back.is_empty()) {
                        tracing::warn!(stop = ?g.stop, "script-guard rolled cos_digest back to rust");
                        rolled_back = true;
                    }
                    if let Some(pos) = runs.iter().position(|r| r.input.run_id == run_id) {
                        let mut run = runs.swap_remove(pos);
                        run.applied = applied;
                        done.push(run);
                    }
                }
                CosDigestRunApply::Failed(error) => {
                    tracing::warn!(%error, %run_id, "CoS triage digest failed");
                }
            }
        }
        if rolled_back {
            // The rest of this tick reads the mode row again.
            self.script_compare_invalidate_modes();
        }
        if let Some(detail) = &result.unverified {
            tracing::warn!(%detail, "CoS triage digest effect unverified; pass stopped");
        }
        Ok((done, result.unverified))
    }
}
