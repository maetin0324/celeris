//! ADR 2026-11-07-health-review-integration D3.5（3〜5）・I9: 設計 review の結果で計画を採用・replan・人に回す。
//!
//! `design_gate.rs` が起こした設計 review の reviewer run の終わり（`Completion::DesignReview`）に、`review.json` の
//! `design`（`DesignVerdictOut`）を読む。無い・壊れていれば不合格。`Event::DesignReviewed { plan_sha256, review_run, pass,
//! findings }` を追記して待ち（`DesignReviewRequested` があり同じ `plan_sha256` の `DesignReviewed` が無い間）を閉じる:
//! - 合格: `DesignReviewRequested` の計画を低 risk と同じ経路（`plan_adoption`: unit の gate・子 task の検証・
//!   `adopt_plan_with_children` / `replan`・木の止め・root の承認）で採用する。
//! - 不合格: planner を replan で起こす。findings は planner の再試行の文（`planner_retry_message`）に入れて
//!   `WorkerProgress` に残し、既存の `previous_attempt_errors`・`replan_reason`・`planner_retry_pending` の経路で planner
//!   の prior になる（新しい欄は足さない）。
//! - 同じ task で不合格が [`MAX_DESIGN_REVIEW_FAILURES`] 回続いたら、その計画を採用したうえで `Trigger::PlanGate` で人に
//!   回す（`PlanApproval.reasons` に `design_review_failed:<n>`。人は承認か replan を選ぶ）。
//!
//! task は設計 review の間 `ready`（`design_review_hold`）なので、ここでの遷移は `PlanGate`（`ready` から有効）だけで、
//! 採用・replan は events の追記だけで表す（I9: dispatcher・worker のメモリに待ちを持たない）。run が `done` で終わら
//! なかった（adapter の失敗・中断・質問・予算切れ）ときは判定を書かず、次の dispatch の `design_review_hold` が
//! `[review] max_reviewer_retries` まで起こし直す。デーモンの再起動で失われた run は孤児の回収が
//! `WorkerFinished{interrupted}` で閉じ、同じ経路で起こし直す。
//!
//! この file は `dispatcher::design_gate` の子 module として `#[path]` で読む（`health_review/` に置く理由は
//! `design_gate.rs` と同じ）。LLM は呼ばない。

use super::super::*;

use super::super::planner_flow::plan_adoption::AdoptionHold;
use super::super::planner_flow::planner_retry_message;
/// `planner_flow::replan_trigger_reason` が使う（`dispatcher/` からは `design_gate::design_verdict::` 経由で読む）。
pub(in super::super) use crate::health_review::design::design_review_failed_reason;
use crate::health_review::design::{
    consecutive_design_review_failures, design_findings_line, pending_design_review,
    planner_run_before,
};
use task_core::ExecutionPlanSpec;
use task_worker::protocol::DesignVerdictOut;

/// D3.5 5.: 同じ task で続いた設計 review の不合格がこの数に達したら人に回す。
pub(in super::super) const MAX_DESIGN_REVIEW_FAILURES: usize = 2;

impl Dispatcher {
    /// 設計 review の run の終わり（`WorkerFinished`・quota・runs 索引の記録の後）。待っている設計 review が無ければ
    /// 何もしない（別の run が先に判定した、人が計画を変えた等）。run が `done` で終わらなければ判定を書かない。
    pub(in super::super) fn settle_design_review(
        &self,
        task_id: TaskId,
        run_id: &str,
        result: &Result<RunOutcome, AdapterError>,
    ) -> Result<(), DispatchError> {
        let Some(task) = self.store.get(task_id)? else {
            return Ok(());
        };
        let events = self.store.events_for(task_id)?;
        let Some((
            seq,
            Event::DesignReviewRequested {
                plan_sha256, plan, ..
            },
        )) = pending_design_review(&events)
        else {
            tracing::info!(%task_id, %run_id, "design review run finished but no design review is pending; ignoring its verdict");
            return Ok(());
        };
        if !matches!(
            result,
            Ok(RunOutcome {
                terminal: Terminal::Done { .. },
                ..
            })
        ) {
            tracing::warn!(%task_id, %run_id, "design review run did not finish with done; the next dispatch retries it (ADR 2026-11-07-health-review-integration D3.5)");
            return Ok(());
        }
        let planner_run = planner_run_before(&events, seq)
            .unwrap_or(run_id)
            .to_string();
        let verdict = self.read_design_verdict(&task);
        self.record_design_verdict(&task, &planner_run, plan_sha256, plan, run_id, verdict)
    }

    /// `artifacts/review.json` の `design`。無い・読めない・`design` が無いときはその理由。
    fn read_design_verdict(&self, task: &Task) -> Result<DesignVerdictOut, String> {
        let dir = self
            .task_dir(task)
            .ok_or_else(|| "task の作業場所が分からない".to_string())?;
        let path = self
            .artifacts_dir(task, &dir)
            .join(crate::review::REVIEW_FILE_NAME);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("review.json を読めない ({}): {e}", path.display()))?;
        let out: task_worker::ReviewOutput =
            serde_json::from_str(&text).map_err(|e| format!("review.json を解釈できない: {e}"))?;
        out.design
            .ok_or_else(|| "review.json に design（設計 review の判定）が無い".to_string())
    }

    /// D3.5 4.・5.: `DesignReviewed` を追記し、合格なら採用、不合格なら planner の再試行（findings を理由に）、続いた
    /// 不合格が上限に達したら採用して `PlanGate`。`verdict` の `Err` は判定が読めなかった理由（不合格として扱う）。
    pub(in super::super) fn record_design_verdict(
        &self,
        task: &Task,
        planner_run: &str,
        plan_sha256: &str,
        plan: &ExecutionPlanSpec,
        review_run: &str,
        verdict: Result<DesignVerdictOut, String>,
    ) -> Result<(), DispatchError> {
        let (pass, findings, why) = match verdict {
            Ok(v) => (v.pass, v.findings, "reviewer の判定が不合格".to_string()),
            Err(why) => (false, Vec::new(), why),
        };
        self.store.append_event(
            task.id,
            &Event::DesignReviewed {
                plan_sha256: plan_sha256.to_string(),
                review_run: review_run.to_string(),
                pass,
                findings: findings
                    .iter()
                    .map(|f| serde_json::to_value(f).unwrap_or(serde_json::Value::Null))
                    .collect(),
            },
        )?;
        if pass {
            tracing::info!(task_id = %task.id, %review_run, "design review passed; adopting the reviewed plan (ADR 2026-11-07-health-review-integration D3.5)");
            return self.adopt_reviewed_plan(task, planner_run, plan, Vec::new());
        }
        let failures = consecutive_design_review_failures(&self.store.events_for(task.id)?);
        let reason = format!(
            "design review failed: {}",
            design_findings_line(&findings, &why)
        );
        if failures >= MAX_DESIGN_REVIEW_FAILURES {
            tracing::warn!(task_id = %task.id, %review_run, failures, "design review failed again; adopting the plan under a human plan gate (ADR 2026-11-07-health-review-integration D3.5 5.)");
            return self.adopt_reviewed_plan(
                task,
                planner_run,
                plan,
                vec![format!("design_review_failed:{failures}")],
            );
        }
        tracing::info!(task_id = %task.id, %review_run, failures, "design review failed; the planner is run again with the findings as its prior");
        // Phase F5-fix3 と同じ: 拒否した計画の file を残すと次の planner run がそのまま再提出する。
        let attempt = self.planner_attempts_in_window(task.id)?;
        self.set_aside_rejected_plan(task, planner_run, attempt);
        self.store.append_event(
            task.id,
            &Event::worker_progress(planner_run, planner_retry_message(&reason)),
        )?;
        Ok(())
    }

    /// 設計 review を通った（または上限まで落ちた）計画の採用。検証（上限だけの違反は /3 の緩めた上限で再検証。planner の
    /// 最後の試行と同じ）→ `prepare_adoption` → `adopt_prepared` → root の承認。`gate_reasons` が空でなければ root の
    /// 判定に関わらず `PlanGate`。採用に失敗したら planner の再試行（理由を `WorkerProgress` に残す）。
    fn adopt_reviewed_plan(
        &self,
        task: &Task,
        planner_run: &str,
        spec: &ExecutionPlanSpec,
        mut gate_reasons: Vec<String>,
    ) -> Result<(), DispatchError> {
        let now = OffsetDateTime::now_utc();
        let active = self.store.execution_plan_active(task.id)?;
        let done_work_units = match active.as_ref() {
            Some(a) => {
                task_core::replan_done_work_units(&a.spec, &self.store.work_units_for(task.id)?)
            }
            None => Vec::new(),
        };
        let limits = self.config.execution.limits;
        let ctx = task_core::PlanContext {
            origin: task_core::PlanOrigin::Planner,
            depth: task_core::tree::depth_of(task),
        };
        let validated =
            match task_core::execution_plan::validate_with(spec, limits, &done_work_units, ctx) {
                Ok(v) => Ok((v, limits)),
                Err(errors)
                    if spec.schema == task_core::EXECUTION_PLAN_SCHEMA_V3
                        && errors
                            .iter()
                            .all(task_ops::tree_plan::is_tree_plan_limit_error) =>
                {
                    let relaxed = task_ops::tree_plan::relaxed_tree_plan_limits(limits);
                    task_core::execution_plan::validate_with(spec, relaxed, &done_work_units, ctx)
                        .map(|v| (v, relaxed))
                        .map_err(|e| task_ops::execution::describe_validation_errors(&e))
                }
                Err(errors) => Err(task_ops::execution::describe_validation_errors(&errors)),
            };
        let adopted = validated.and_then(|(validated, adopt_limits)| {
            let prepared = self
                .prepare_adoption(
                    task,
                    validated,
                    active.is_some(),
                    &done_work_units,
                    adopt_limits,
                    now,
                )
                .map_err(|hold| match hold {
                    AdoptionHold::NeedsAuthorization(questions) => {
                        format!("部をまたぐ子の認可が要る: {}", questions.join(" / "))
                    }
                    AdoptionHold::Invalid(reason) => reason,
                })?;
            self.adopt_prepared(task, planner_run, prepared, active.is_some(), now)
        });
        let plan = match adopted {
            Ok(plan) => plan,
            Err(reason) => {
                tracing::warn!(task_id = %task.id, %planner_run, error = %reason, "failed to adopt the reviewed plan; the planner is run again");
                self.store.append_event(
                    task.id,
                    &Event::worker_progress(
                        planner_run,
                        planner_retry_message(&format!(
                            "設計 review の後の採用に失敗しました: {reason}"
                        )),
                    ),
                )?;
                return Ok(());
            }
        };
        let approval = self.plan_approval_of(task, &plan, planner_run);
        if approval.as_ref().is_some_and(|a| a.required) || !gate_reasons.is_empty() {
            let mut reasons = approval.map(|a| a.reasons).unwrap_or_default();
            reasons.append(&mut gate_reasons);
            tracing::info!(task_id = %task.id, plan_id = %plan.id, ?reasons, "the reviewed plan needs a human approval (ADR-0079 D8 / ADR 2026-11-07-health-review-integration D3.5 5.)");
            self.store.apply_transition_with_events(
                task.id,
                Trigger::PlanGate {
                    plan_id: plan.id.clone(),
                },
                vec![Event::PlanApprovalRequested {
                    plan_id: plan.id.clone(),
                    reasons,
                }],
            )?;
        } else {
            self.record_plan_adopted_notice(task, &plan, approval, now);
        }
        Ok(())
    }
}
