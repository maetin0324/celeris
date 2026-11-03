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

use std::sync::Arc;
use std::time::Instant;

use task_core::SqliteStore;
use task_core::chat::digest::{
    CosDigestCommand, CosDigestInput, CosDigestListRunsReply, CosDigestListRunsRequest,
    CosDigestPassOutput, CosDigestPassRead, CosDigestReadRunRequest, CosDigestRunRef,
    cos_digest_pass_decide,
};
use task_core::script_compare::{
    CosDigestApplied, CosDigestAppliedBy, CosDigestFallback, CosDigestOpLog, ScriptMode,
    sha256_digest,
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

/// What one pass applied for one run.
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

/// An applied command S5-6 could not check against its idempotency key and record.
#[derive(Debug)]
struct Unverified(String);

/// Why a verified apply of one run stopped.
#[derive(Debug)]
enum VerifiedApplyError {
    /// The write failed inside its own transaction: nothing of that command was stored, and the
    /// next pass checks the same key again (the run stays as it was, like a Rust apply).
    Failed(String),
    /// The stored record disagrees with the command (S5-6: stop and roll back).
    Unverified(Unverified),
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
            let _ = self.apply_digest_pass(launch, &pass_reads, &rust, &rust, false, now);
            return;
        };
        match script.draft.canary_selected {
            true => {
                let classified = match (&script.call, &ts_result, &budget_fallback) {
                    (_, _, Some(budget)) => {
                        CosDigestPassClassified::unavailable(&rust, &budget.message)
                    }
                    (Ok(_), Some((result, _)), None) => {
                        classify_cos_digest_pass(&pass_reads, &rust, result)
                    }
                    (Err(reason), _, None) => CosDigestPassClassified::unavailable(&rust, reason),
                    (Ok(_), None, None) => {
                        CosDigestPassClassified::unavailable(&rust, "no script result")
                    }
                };
                let mode_changed = self.cos_digest_mode_changed(&script.row);
                // S5-6: a TS output that read through a denied op is never used (no fail-open).
                let denied = reads.denied().then(|| {
                    CosDigestFallback::new("denied", "a cos_digest op of the pass was denied")
                });
                let (apply, applied_by, fallback) =
                    match (&classified.output, mode_changed, denied, budget_fallback) {
                        (_, Some(changed), _, _) => {
                            (&rust, CosDigestAppliedBy::Rust, Some(changed))
                        }
                        (_, None, _, Some(budget)) => {
                            (&rust, CosDigestAppliedBy::Rust, Some(budget))
                        }
                        (_, None, Some(denied), None) => {
                            (&rust, CosDigestAppliedBy::Rust, Some(denied))
                        }
                        (Some(ts), None, None, None) => (ts, CosDigestAppliedBy::Ts, None),
                        (None, None, None, None) => {
                            (&rust, CosDigestAppliedBy::Rust, classified.fallback.clone())
                        }
                    };
                let verify = applied_by == CosDigestAppliedBy::Ts;
                let (applied, unverified) =
                    self.apply_digest_pass(launch, &pass_reads, &rust, apply, verify, now);
                if let Some(Unverified(detail)) = unverified {
                    // S5-6: stop, keep `digest_due`, roll back with `system:script-guard`; the
                    // inbox item comes from that event (script-inbox).
                    self.script_rollback(
                        task_core::script_compare::ScriptDomain::CosDigest,
                        script.row.2,
                        &format!("script_effect_unverified: {detail}"),
                    );
                }
                let (epoch, elapsed) = ts_result
                    .as_ref()
                    .map(|(r, e)| (ts_isolate_epoch(r), *e))
                    .unwrap_or_default();
                let records = pass_records(
                    &script.draft,
                    &reads,
                    &applied,
                    &classified,
                    applied_by,
                    fallback,
                    epoch,
                    elapsed,
                );
                if let Some(log) = self.script_compare_log_store() {
                    for record in records {
                        append_pass_record(log.as_ref(), record, true);
                    }
                }
            }
            false => {
                let (applied, _) =
                    self.apply_digest_pass(launch, &pass_reads, &rust, &rust, false, now);
                self.compare_digest_pass(script, reads, pass_reads, rust, applied);
            }
        }
    }

    /// Mode `compare`: after Rust applied, freeze the memo and run TS on a separate tokio task.
    fn compare_digest_pass(
        &mut self,
        script: PassScript,
        mut reads: CosDigestPassReads,
        pass_reads: Vec<CosDigestPassRead>,
        rust: CosDigestPassOutput,
        applied: Vec<AppliedRun>,
    ) {
        let Some(log) = self.script_compare_log_store() else {
            return;
        };
        reads.frozen = true;
        let draft = script.draft;
        let (script, runtime) = match script.call {
            Err(reason) => {
                let classified = CosDigestPassClassified::unavailable(&rust, &reason);
                for record in pass_records(
                    &draft,
                    &reads,
                    &applied,
                    &classified,
                    CosDigestAppliedBy::Rust,
                    None,
                    0,
                    std::time::Duration::ZERO,
                ) {
                    append_pass_record(log.as_ref(), record, false);
                }
                return;
            }
            Ok(call) => call,
        };
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
                for record in pass_records(
                    &draft,
                    &reads,
                    &applied,
                    &classified,
                    CosDigestAppliedBy::Rust,
                    None,
                    ts_isolate_epoch(&result),
                    elapsed,
                ) {
                    append_pass_record(log.as_ref(), record, false);
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

    /// Apply `apply` run by run (`post_digest` → `requeue_run`); a failed write skips the rest of
    /// that run (its items stay `running` for the next pass). Returns the runs fully applied.
    /// `verify` (a TS output, S5-6): every effect is checked against its idempotency key and the
    /// stored record; one that cannot be checked stops the whole pass (`Unverified`).
    fn apply_digest_pass(
        &mut self,
        launch: &mut CosChatLaunch,
        reads: &[CosDigestPassRead],
        rust: &CosDigestPassOutput,
        apply: &CosDigestPassOutput,
        verify: bool,
        now: OffsetDateTime,
    ) -> (Vec<AppliedRun>, Option<Unverified>) {
        let mut done = Vec::new();
        for run in &apply.runs {
            let Some(input) = reads
                .iter()
                .find(|r| r.run.run_id == run.run_id)
                .and_then(|r| r.input.clone())
            else {
                continue;
            };
            let result = if verify {
                match launch.apply_digest_commands_verified(&run.commands, now) {
                    Err(VerifiedApplyError::Unverified(unverified)) => {
                        tracing::warn!(run_id = %run.run_id, detail = %unverified.0, "CoS triage digest effect unverified; pass stopped");
                        return (done, Some(unverified));
                    }
                    Err(VerifiedApplyError::Failed(error)) => Err(error),
                    Ok(applied) => Ok(applied),
                }
            } else {
                launch.apply_digest_commands(&run.commands, now)
            };
            match result {
                Ok(applied) => done.push(AppliedRun {
                    rust: rust
                        .runs
                        .iter()
                        .find(|r| r.run_id == run.run_id)
                        .map(|r| r.commands.clone())
                        .unwrap_or_default(),
                    input,
                    applied,
                }),
                Err(error) => {
                    tracing::warn!(%error, run_id = %run.run_id, "CoS triage digest failed");
                }
            }
        }
        (done, None)
    }
}

/// One record per fully applied run (S4-15).
#[allow(clippy::too_many_arguments)]
fn pass_records(
    draft: &CosDigestPassDraft,
    reads: &CosDigestPassReads,
    applied: &[AppliedRun],
    classified: &CosDigestPassClassified,
    applied_by: CosDigestAppliedBy,
    fallback: Option<CosDigestFallback>,
    isolate_epoch: u64,
    elapsed: std::time::Duration,
) -> Vec<Result<task_core::script_compare::CosDigestCompareRecord, serde_json::Error>> {
    applied
        .iter()
        .filter_map(|run| {
            let mut c = classified.run(&run.input.run_id)?.clone();
            if reads.denied() && !c.safety_violations.iter().any(|v| v == "denied_scope") {
                c.safety_violations.push("denied_scope".to_owned());
            }
            Some(draft.record(
                CosDigestRunRecord {
                    input: &run.input,
                    rust: &run.rust,
                    classified: &c,
                    applied_by,
                    fallback: fallback.clone(),
                    applied: run.applied.clone(),
                    ops: reads.ops_of(&run.input.run_id),
                },
                isolate_epoch,
                elapsed,
            ))
        })
        .collect()
}

/// Append a record and count it in the same transaction (S5-7); a failure only warns (the
/// digest pass goes on, S4-4). `ts_side`: the pass was selected for TS (canary/ts).
fn append_pass_record(
    store: &dyn crate::script_policy::ScriptDomainStore,
    record: Result<task_core::script_compare::CosDigestCompareRecord, serde_json::Error>,
    ts_side: bool,
) {
    let result = record.map_err(|e| e.to_string()).and_then(|r| {
        store
            .script_compare_log_append_observed(&r, ts_side, OffsetDateTime::now_utc())
            .map_err(|e| e.to_string())
    });
    match result {
        Ok((_, Some(outcome))) if !outcome.rolled_back.is_empty() => {
            tracing::warn!(stop = ?outcome.stop, "script-guard rolled cos_digest back to rust");
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "script_compare_log append failed"),
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

impl CosChatLaunch {
    /// S5-6: apply a TS output's commands, checking each effect against its idempotency key and
    /// the stored record. `post_digest` writes once per key and the stored message must belong to
    /// the same thread and run; `requeue_run` is the conditional `WHERE state='running'` update.
    /// A failed write is `Failed` (its transaction stored nothing; the rest of the run is not
    /// written and the next pass checks the key again). A stored record that disagrees with the
    /// command is `Unverified`.
    fn apply_digest_commands_verified(
        &mut self,
        commands: &[CosDigestCommand],
        now: OffsetDateTime,
    ) -> Result<CosDigestApplied, VerifiedApplyError> {
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
                        .map_err(|e| match e {
                            // The key is stored with another text or cards: the record disagrees.
                            task_core::chat::ChatError::Conflict(_) => {
                                VerifiedApplyError::Unverified(Unverified(format!(
                                    "post_digest {idempotency_key}: {e}"
                                )))
                            }
                            e => VerifiedApplyError::Failed(format!(
                                "post_digest {idempotency_key}: {e}"
                            )),
                        })?;
                    if message.thread_id != *thread || message.run_id.as_deref() != Some(run_id) {
                        return Err(VerifiedApplyError::Unverified(Unverified(format!(
                            "post_digest {idempotency_key}: the stored message {} belongs to {}/{}",
                            message.id,
                            message.thread_id,
                            message.run_id.as_deref().unwrap_or("-")
                        ))));
                    }
                    applied.message_id = message.id;
                }
                CosDigestCommand::RequeueRun { run_id } => {
                    let requeued = self
                        .store
                        .cos_triage_requeue_run(run_id, now)
                        .map_err(|e| {
                            VerifiedApplyError::Failed(format!("requeue_run {run_id}: {e}"))
                        })?;
                    if requeued > 0 {
                        self.triage.dirty = true;
                    }
                    applied.requeued = u64::try_from(requeued).unwrap_or(u64::MAX);
                }
            }
        }
        Ok(applied)
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
