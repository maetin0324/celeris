//! planner run の結果の採用・やり直し・replan と planner の文脈（ADR-0072、ADR-0079）。ADR-0082 の L3。

use super::*;
use crate::run_settlement::run_end_record;
use task_ops::tree::{
    SUPPLY_FAILED_END, plan_invalid_answer_position, plan_invalid_replan_note,
    planner_blocked_question, planner_rejections_since_last_plan, planner_retry_message,
    planner_supply_requeue_message,
};

/// 計画の採用の共通部（設計 review を通った計画と共有）。file は size の制約で `health_review/` に置く。
#[path = "../health_review/plan_adoption.rs"]
pub(super) mod plan_adoption;

impl Dispatcher {
    /// ADR-0072 D14（Phase E3）: task-local な planner run の終わり方を判定する。`current_wu` が無い
    /// （計画がまだ無い）Task の `RunRole::Planner` run はすべてここを通る。
    ///
    /// `artifacts/execution-plan.json` を D14 で検証し、
    /// - 妥当（かつ run が `Terminal::Done` か turn の上限で終わった。ADR 2026-11-09-planner-turn-limit-and-comment-interrupt
    ///   D1）なら採用して `Trigger::Continue{Planned}`（設計 review・root の承認の待ちを経る場合がある）。
    /// - 不正・run 自体が異常終了（error/question/turn 以外の budget_exhausted/yielded）なら、1 回だけ再試行する
    ///   （もう一度 planner run を起こす。2 回目もだめなら計画を作らず atomic に倒す。D14「1 回だけ
    ///   再試行し、それでも不正なら atomic に倒す」）。Task は失敗させない（D12）。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn on_planner_finished(
        &mut self,
        task_id: TaskId,
        task: &Task,
        run_id: String,
        run_since: Option<OffsetDateTime>,
        provider: ProviderId,
        (account, account_adapter): (Option<&str>, Option<AccountAdapter>),
        result: Result<RunOutcome, AdapterError>,
    ) -> Result<(), DispatchError> {
        let now = OffsetDateTime::now_utc();

        let (run_end, describe, usage) = design_gate::run_end_of(&result, "planner");
        // ADR 2026-11-24-planner-supply-failure-cooldown D1–D3: 供給側の失敗は worker と同じ関数で cooldown にし、数えない
        // ものは `end = HarnessError{Supply}` で閉じる（`planner_attempts_in_window`・`planner_retry_pending` が読む）。
        let supply = planner_supply_failure(
            self,
            task_id,
            (account, account_adapter),
            &provider,
            &result,
        )?;
        let requeue = supply.as_ref().is_some_and(|(requeue, ..)| *requeue);
        let run_end = if requeue {
            Some(SUPPLY_FAILED_END)
        } else {
            run_end
        };

        // ADR-0076: planner run の quota 消費も worker と同じ `RoutingRuntime::settle_quota` で見積もる。以降の `?` で抜けても
        // `QuotaActivity` が閉じているよう、分岐より前に一度だけ求め、`WorkerFinished` を保存するすべての分岐で同じトランザクションに添える。
        let quota_event = {
            let model = self.started_model_of(task_id, &run_id);
            self.routing.settle_quota(
                &self.store,
                task_id,
                &run_id,
                None,
                account.zip(account_adapter),
                provider.to_string(),
                &model,
                usage.as_ref(),
            )
        };

        // ADR-0072 D17（Phase E4）: この run が replan（既に `active` な計画がある）なら、done の WU の key/spec が変わっていないことも
        // 検証する（D14「replan のときは done の WU の key と spec が変わっていないこと」）。
        let active_plan = self.store.execution_plan_active(task_id)?;
        // ADR-0074 F5-fix（不具合 2）: daemon が足した WU（`kind = integrate`・統合の repair WU）は
        // planner の視野に無いので対象から外す（`task_ops::execution::replan` と同じ規則）。
        let done_work_units: Vec<(String, task_core::WorkUnitSpec)> = if let Some(active) =
            active_plan.as_ref()
        {
            task_core::replan_done_work_units(&active.spec, &self.store.work_units_for(task_id)?)
        } else {
            Vec::new()
        };
        // D14: 計画の検証は run が `Terminal::Done` か turn の上限で終わったときだけ試みる（ADR 2026-11-09-planner-turn-limit-and-comment-interrupt
        // D1。それ以外は無条件に「不正な試行」として扱う）。
        // ADR-0079 D3 / D4 (3)（Phase R2a）: /3 の計画が計画の上限（段階の数・段階あたりの unit・子 task・`max_depth`）だけで不正なら、
        // 1 回目は従来どおり不正な試行（planner に理由を渡して再試行）、最後の試行では上限を緩めて採用し、超えた分の unit を
        // `kind: limit` の決定の要求で止める（黙って切らない・atomic に倒さない）。採用に使う上限を `adopt_limits` に返す。
        let mut adopt_limits = self.config.execution.limits;
        let validation: Result<_, String> = match &supply {
            Some((_, reason, _)) => Err(reason.clone()),
            None => planner_plan_validation(
                self,
                task,
                &result,
                &describe,
                (active_plan.as_ref(), &done_work_units),
                &mut adopt_limits,
            )?,
        };
        // ADR-0079 付記「R7-3」D3: replan の計画が退役した（superseded / cancelled の）行の key を再利用していれば、採用の前に検証の理由として
        // planner に返す（再試行・`plan_invalid` の経路）。以前は段階の統合 WU の key の重なりが採用の中で sqlite の `UNIQUE constraint failed: work_units.task_id, key` に落ちていた（08:15Z）。
        let validation = match validation {
            Ok(v) if let Some(active) = active_plan.as_ref() => {
                let rows = self.store.work_units_for(task_id)?;
                let mut retired = task_core::execution_plan::retired_key_errors(&v.spec, &rows);
                // ADR-0079 付記「R7-12」D3: daemon の足した WU（配送 / 最終レビュー / 統合の repair WU）の key も同じく返す。
                retired.extend(task_core::execution_plan::daemon_added_key_errors(
                    &active.spec,
                    &v.spec,
                    &rows,
                ));
                if retired.is_empty() {
                    Ok(v)
                } else {
                    Err(task_ops::execution::describe_validation_errors(&retired))
                }
            }
            other => other,
        };
        // ADR 2026-11-07-health-review-integration D3.3: 高 risk の class の path を書く planner の計画に brief が無ければ再試行に回す。
        let validation = self.design_brief_gate(task, &run_id, validation);

        let metrics = run_since.map(|since| task_core::RunMetrics {
            wall_ms: wall_ms_since(since),
            retries: task.attempts,
            ..Default::default()
        });
        let index_status = run_end
            .map(task_core::RunIndexStatus::from_run_end)
            .unwrap_or(task_core::RunIndexStatus::HarnessError);
        if let Err(e) =
            self.store
                .run_index_finish(&run_id, index_status, None, usage, metrics, now)
        {
            tracing::warn!(%task_id, %run_id, error = %e, "failed to finish the runs index row for the planner run");
        }

        match validation {
            Ok((validated, gate)) => {
                let replan = active_plan.is_some();
                let outcome = format!(
                    "done: {} work unit(s) planned",
                    validated.spec.work_units.len()
                );
                let role = Some(RunRole::Planner);
                let finished = run_end_record(&run_id, outcome, usage, role, metrics, run_end);
                // unit の gate・子 task の検証・採用・root の承認の判定は `plan_adoption`（設計 review を通った計画と共有）。
                let prepared = match self.prepare_adoption(
                    task,
                    validated,
                    replan,
                    &done_work_units,
                    adopt_limits,
                    now,
                ) {
                    Ok(prepared) => prepared,
                    Err(plan_adoption::AdoptionHold::NeedsAuthorization(questions)) => {
                        let question = questions.join("\n");
                        let progress = Event::worker_progress(run_id.clone(), question.clone());
                        self.store.apply_transition_with_events(
                            task_id,
                            Trigger::WorkerQuestion,
                            vec![finished, quota_event, progress],
                        )?;
                        for q in &questions {
                            if let Err(e) = crate::approvals::record_question_approval(
                                self.store.as_ref(),
                                task,
                                q,
                                now,
                            ) {
                                tracing::warn!(%task_id, error = %e, "failed to record the cross-department approval for planner children");
                            }
                        }
                        return Ok(());
                    }
                    Err(plan_adoption::AdoptionHold::Invalid(reason)) => {
                        self.give_up_or_retry_planner(
                            task_id,
                            task,
                            &run_id,
                            vec![finished, quota_event],
                            format!("計画が採用の前の検証に通りませんでした: {reason}"),
                            now,
                        )?;
                        return Ok(());
                    }
                };
                // D3.5・D3.6: 高 risk・unknown の計画は採用せず、設計 review を待つ（`health_review/design_gate.rs`）。
                if self.design_gate(
                    task,
                    gate,
                    &prepared.validated.spec,
                    [&finished, &quota_event],
                )? {
                    return Ok(());
                }
                match self.adopt_prepared(task, &run_id, prepared, replan, now) {
                    Ok(plan) => match self.plan_approval_of(task, &plan, &run_id) {
                        Some(a) if a.required => {
                            tracing::info!(%task_id, plan_id = %plan.id, reasons = ?a.reasons, "the root plan needs a human approval (ADR-0079 D8)");
                            self.store.apply_transition_with_events(
                                task_id,
                                Trigger::PlanGate {
                                    plan_id: plan.id.clone(),
                                },
                                vec![
                                    finished,
                                    quota_event,
                                    Event::PlanApprovalRequested {
                                        plan_id: plan.id.clone(),
                                        reasons: a.reasons,
                                    },
                                ],
                            )?;
                        }
                        approval => {
                            self.store.apply_transition_with_events(
                                task_id,
                                Trigger::Continue {
                                    why: task_core::ContinueWhy::Planned,
                                },
                                vec![finished, quota_event],
                            )?;
                            self.record_plan_adopted_notice(task, &plan, approval, now);
                        }
                    },
                    Err(e) => {
                        // 採用そのものが失敗した（既に有効な計画がある等、通常起きない）: 不正な
                        // 試行として retry/give-up の判断に合流させる。
                        tracing::warn!(%task_id, %run_id, error = %e, "failed to adopt the validated plan; treating as an invalid attempt");
                        self.give_up_or_retry_planner(
                            task_id,
                            task,
                            &run_id,
                            vec![finished, quota_event],
                            format!("計画の採用に失敗しました: {e}"),
                            now,
                        )?;
                    }
                }
            }
            Err(reason) if requeue => {
                let outcome = format!("requeue: adapter: {reason}");
                let finished = run_end_record(
                    &run_id,
                    outcome,
                    usage,
                    Some(RunRole::Planner),
                    metrics,
                    run_end,
                );
                let progress =
                    Event::worker_progress(run_id.clone(), planner_supply_requeue_message(&reason));
                let events = [finished, quota_event]
                    .into_iter()
                    .chain(supply.and_then(|s| s.2));
                self.store.apply_transition_with_events(
                    task_id,
                    Trigger::Requeue,
                    events.chain([progress]).collect(),
                )?;
            }
            Err(reason) => {
                let outcome = format!("error(retryable=true): invalid execution plan: {reason}");
                let role = Some(RunRole::Planner);
                let finished = run_end_record(&run_id, outcome, usage, role, metrics, run_end);
                self.give_up_or_retry_planner(
                    task_id,
                    task,
                    &run_id,
                    [finished, quota_event]
                        .into_iter()
                        .chain(supply.and_then(|s| s.2))
                        .collect(),
                    reason,
                    now,
                )?;
            }
        }
        Ok(())
    }

    /// ADR-0072 D14（Phase E3）/ D17（Phase E4）: planner の出力が不正だった（または run が異常終了
    /// した）ときの、「1 回だけ再試行、それでも駄目なら諦める」の判断。**この「試行」の窓は直近の
    /// `Event::ExecutionPlanned`（無ければ Task の最初）から数える**（E4 の注記: 最初の gate 判定の
    /// planner 試行と、後の replan の planner 試行を混同しない。`Event::WorkerStarted{role: Planner}`
    /// の件数〈この run 自身を含む〉を events から純粋に導出する。D5 と同じ考え方）。
    /// 諦めたときの振る舞いは呼び出し時点の状態で決める: 既に `active` な計画が無ければ fresh
    /// planning の give up（atomic に倒す。D14）、既に `active` な計画があれば replan の give up
    /// （`blocked`。D12「失敗にしないもの」、D17/D18）。Task を `failed` にはしない。
    // `finished` は `WorkerFinished` とそれに添える Event（ADR-0076 の `QuotaEstimated`）。
    /// Phase F5-fix3: 拒否した planner の計画（`artifacts/execution-plan.json`）を
    /// `artifacts/execution-plan.rejected.<run>.attempt-<n>.json` に移す。無ければ何もしない。失敗しても警告だけ。
    pub(super) fn set_aside_rejected_plan(&self, task: &Task, run_id: &str, attempt: usize) {
        let Some(dir) = self
            .task_dir(task)
            .map(|d| self.artifacts_dir(task, d.as_path()))
        else {
            return;
        };
        let from = dir.join("execution-plan.json");
        if !from.exists() {
            return;
        }
        let to = dir.join(rejected_plan_file(run_id, attempt));
        if let Err(e) = std::fs::rename(&from, &to) {
            tracing::warn!(task_id = %task.id, error = %e, "failed to set the rejected execution plan aside");
        }
    }

    pub(super) fn give_up_or_retry_planner(
        &self,
        task_id: TaskId,
        task: &Task,
        run_id: &str,
        finished: Vec<Event>,
        reason: String,
        now: OffsetDateTime,
    ) -> Result<(), DispatchError> {
        // Phase F5-fix3: 拒否した計画のファイルを残すと、次の planner run はそれを見つけて「検証済み」と
        // 思い込みそのまま再提出する。run ID と試行番号で再計画の窓をまたいでも保存する。
        let attempts_so_far = self.planner_attempts_in_window(task_id)?;
        self.set_aside_rejected_plan(task, run_id, attempts_so_far);
        let tree_planner = self.is_tree_planner(task)?;
        if attempts_so_far < MAX_PLANNER_ATTEMPTS {
            let progress = Event::worker_progress(run_id, planner_retry_message(&reason));
            self.store.apply_transition_with_events(
                task_id,
                Trigger::Continue {
                    why: task_core::ContinueWhy::Planned,
                },
                finished.into_iter().chain([progress]).collect(),
            )?;
        } else if tree_planner {
            // ADR-0079 D9（Phase R2b）: /3 の planner（木の節点）の計画が 2 回とも不正だった。atomic に倒さず
            // （分けると決めた仕事を黙って 1 run に潰さない）、replan でも自由文の質問にせず、`kind: plan_invalid`
            // の決定の要求を出す。Task は `ready` に戻し、決定が開いている間は run を起こさない
            // （`plan_invalid_hold`。R2a の木の上限の止め方と同じ）。回答の入口は R3a。
            let mut errors = planner_rejections_since_last_plan(&self.store.events_for(task_id)?);
            if !errors.iter().any(|e| e == &reason) {
                errors.push(reason.clone());
            }
            let replan = self.store.execution_plan_active(task_id)?.is_some();
            let path =
                task_ops::tree::decision_path(self.store.as_ref(), task).map_err(ops_to_store)?;
            let request = task_core::tree::plan_invalid_decision(
                task_id,
                replan,
                &errors,
                path,
                task_core::DecisionRaisedBy {
                    task_id,
                    run_id: Some(run_id.to_string()),
                    origin: task_core::DecisionOrigin::Daemon,
                },
            );
            tracing::warn!(%task_id, %run_id, decision = %request.id, replan, "the /3 plan was invalid twice; asking a human (plan_invalid) instead of falling back to atomic (ADR-0079 D9)");
            let progress = Event::worker_progress(
                run_id,
                format!(
                    "計画（/3）を 2 回とも採用できませんでした（{reason}）。atomic には倒さず、人の決定（plan_invalid）を待ちます（ADR-0079 D9）。"
                ),
            );
            let mut events: Vec<Event> = finished.into_iter().chain([progress]).collect();
            if self.open_plan_invalid(task)?.is_none() {
                events.push(Event::DecisionRequested {
                    decision: Box::new(request),
                });
            }
            self.store.apply_transition_with_events(
                task_id,
                Trigger::Continue {
                    why: task_core::ContinueWhy::Planned,
                },
                events,
            )?;
        } else if self.store.execution_plan_active(task_id)?.is_some() {
            // ADR-0072 D17（Phase E4）: これは replan の planner run（既に `active` な計画がある）。
            // 2 回とも不正だったので、直せないまま突き進まず人に聞く（`blocked`。D12「失敗にしない
            // もの」の一つ。atomic への書き換えはしない — 既に WU の履歴がある計画を捨てるのは
            // 安全ではない）。
            let question = planner_blocked_question(&reason);
            let progress = Event::worker_progress(run_id, question.clone());
            self.store.apply_transition_with_events(
                task_id,
                Trigger::WorkerQuestion,
                finished.into_iter().chain([progress]).collect(),
            )?;
            if let Err(e) = crate::approvals::record_question_approval(
                self.store.as_ref(),
                task,
                &question,
                now,
            ) {
                tracing::warn!(%task_id, error = %e, "failed to record the approval for the replan question");
            }
        } else {
            // D14: それでも不正なら atomic に倒す（暗黙の WU で実行する）。gate の判定を Atomic に
            // 書き換えて監査に残す（`Task.routing.execution` を書き換えないと、次の dispatch でまた
            // planner run を起こそうとしてしまう）。
            if let Some(mut routing) = task.routing.clone()
                && let Some(mut decision) = routing.execution.clone()
            {
                decision.mode = task_core::ExecutionMode::Atomic;
                decision.rule_id = "atomic/planner-invalid".to_string();
                decision.signals.push(task_core::GateSignal {
                    name: "planner_retry_exhausted".to_string(),
                    weight: 0,
                    detail: format!(
                        "{attempts_so_far} planner attempts failed; falling back to atomic: {reason}"
                    ),
                });
                routing.execution = Some(decision.clone());
                let mut fresh = task.clone();
                fresh.routing = Some(routing);
                fresh.updated_at = now;
                if let Err(e) = self.store.update_task(
                    &fresh,
                    Event::ExecutionGated {
                        decision: Box::new(decision),
                    },
                ) {
                    tracing::warn!(%task_id, error = %e, "failed to record the atomic fallback after planner retries were exhausted");
                }
            }
            let progress = Event::worker_progress(
                run_id,
                format!("計画を採用できず直接実行に切り替えました（{reason}）。"),
            );
            self.store.apply_transition_with_events(
                task_id,
                Trigger::Continue {
                    why: task_core::ContinueWhy::Planned,
                },
                finished.into_iter().chain([progress]).collect(),
            )?;
        }
        Ok(())
    }

    /// ADR-0007 D2: その Plan 自身を含む祖先 Plan の数。
    pub(super) fn plan_depth(&self, task: &Task) -> Result<u32, DispatchError> {
        let mut depth = 0;
        let mut current = Some(task.clone());
        let mut hops = 0;
        while let Some(t) = current {
            if t.kind == TaskKind::Plan {
                depth += 1;
            }
            hops += 1;
            if hops > 64 {
                break;
            }
            current = match t.parent_id {
                Some(p) => self.store.get(p)?,
                None => None,
            };
        }
        Ok(depth)
    }

    /// D15/ADR-0074 D1.4: 最終レビューに渡す決定的な要約（WU ごとの最終 checkpoint の `completed` を
    /// 1 段落ずつ。統合 WU は除く）。
    pub(super) fn plan_summary(&self, units: &[task_core::WorkUnitRow]) -> String {
        let mut paragraphs = Vec::new();
        for u in units {
            if u.kind == task_core::WorkUnitKind::Integrate
                || u.status != task_core::WorkUnitStatus::Done
            {
                continue;
            }
            let completed = u
                .last_run_id
                .as_deref()
                .and_then(|rid| self.store.run_index_get(rid).ok().flatten())
                .and_then(|r| r.checkpoint)
                .map(|cp| cp.completed.join("; "))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "完了".to_string());
            paragraphs.push(format!("{}: {}", u.spec.title, completed));
        }
        paragraphs.join("\n")
    }

    /// ADR-0072 D14: 直近の `ExecutionPlanned`（無ければ Task の最初）から数えた planner run の試行
    /// （`WorkerStarted{role: Planner}` の件数。この run 自身を含む。人の comment の割り込みで止まった run と、供給側の
    /// 失敗で終わり試行に数えなかった run〈ADR 2026-11-24-planner-supply-failure-cooldown D2〉は除く）。
    pub(super) fn planner_attempts_in_window(
        &self,
        task_id: TaskId,
    ) -> Result<usize, DispatchError> {
        let events = self.store.events_for(task_id)?;
        // ADR-0079 D7（Phase R3a）: `plan_invalid` への回答（replan）も窓を開け直す（人の指示つきで planner を
        // もう一度 2 回まで試す）。
        let since_idx = events
            .iter()
            .rposition(|(_, e)| matches!(e, Event::ExecutionPlanned { .. }))
            .max(plan_invalid_answer_position(&events));
        Ok(events
            .iter()
            .enumerate()
            // ADR 2026-11-09-planner-turn-limit-and-comment-interrupt D2: 人の comment の割り込みで止まった run は計画の
            // 試行ではないので数えない（判定は ADR-0140 付記の comment-resume と同じ `interrupted_by_comment`）。
            .filter(|(i, (_, e))| {
                since_idx.is_none_or(|s| *i > s)
                    && matches!(
                        e,
                        Event::WorkerStarted {
                            run_id,
                            role: Some(RunRole::Planner),
                            ..
                        } if !continuation_session::interrupted_by_comment(&events, run_id)
                            && !task_ops::tree::planner_run_supply_failed(&events, run_id)
                    )
            })
            .count())
    }

    /// ADR-0072 D14（Phase E3）/ D17（Phase E4b 項目1）: planner run のプロンプトに渡す gate の
    /// 根拠と D18 の上限。`replan` のときは、今の計画（版・WU ごとの状態・完了/失敗の要約）・起こした
    /// 理由・保持すべき `done` の WU の key も添える。`original_budget` は planner 用に上書きする
    /// **前**の Task の budget（WU の既定の計算に使う）。
    pub(super) fn execution_planner_context(
        &self,
        task: &Task,
        original_budget: task_core::Budget,
        replan: bool,
    ) -> Result<task_worker::protocol::ExecutionPlannerContext, DispatchError> {
        let decision = task.routing.as_ref().and_then(|r| r.execution.clone());
        let limits = self.config.execution.limits;
        let tree_planner = self.is_tree_planner(task)?;
        // Phase F5-fix3: 同じ計画の回で前の planner run の計画が拒否されていれば、その理由を渡す。
        let planner_events = self.store.events_for(task.id)?;
        let mut previous_attempt_errors = planner_rejections_since_last_plan(&planner_events);
        // ADR-0079 D7（Phase R3a）: `plan_invalid` に replan と答えた人の note を planner に渡す（末尾に 1 行）。
        if let Some(note) = plan_invalid_replan_note(&planner_events) {
            previous_attempt_errors.push(note);
        }
        let (replan_reason, current_plan_version, work_unit_summaries, preserve_done_keys) =
            if replan {
                let version = self
                    .store
                    .execution_plan_active(task.id)?
                    .map(|p| p.version);
                let units = self.store.work_units_for(task.id)?;
                let summaries = units
                    .iter()
                    .map(|u| self.work_unit_plan_summary_line(u))
                    .collect();
                let preserve = units
                    .iter()
                    .filter(|u| u.status == task_core::WorkUnitStatus::Done)
                    .map(|u| u.key.clone())
                    .collect();
                (
                    self.replan_trigger_reason(task.id)?,
                    version,
                    summaries,
                    preserve,
                )
            } else {
                (String::new(), None, Vec::new(), Vec::new())
            };
        Ok(task_worker::protocol::ExecutionPlannerContext {
            gate_rule_id: decision
                .as_ref()
                .map(|d| d.rule_id.clone())
                .unwrap_or_default(),
            gate_score: decision.as_ref().map(|d| d.score).unwrap_or(0),
            gate_signals: decision
                .as_ref()
                .map(|d| {
                    d.signals
                        .iter()
                        .map(|s| format!("{}: {} (+{})", s.name, s.detail, s.weight))
                        .collect()
                })
                .unwrap_or_default(),
            max_work_units: if self.config.execution.parallel {
                limits.max_work_units_v2
            } else {
                limits.max_work_units
            },
            work_unit_max_turns: limits.work_unit_max_turns,
            work_unit_max_wall_secs: limits.work_unit_max_wall_secs,
            default_max_turns: original_budget.max_turns.max(30),
            default_max_wall_secs: original_budget.max_wall_secs.max(1800),
            replan,
            replan_reason,
            current_plan_version,
            work_unit_summaries,
            preserve_done_keys,
            parallel: self.config.execution.parallel,
            max_phases: if self.config.execution.parallel {
                limits.max_phases
            } else {
                0
            },
            max_title_chars: limits.max_title_chars,
            max_objective_chars: limits.max_objective_chars,
            max_done_when_items: limits.max_done_when_items,
            max_done_when_chars: limits.max_done_when_chars,
            max_checks: limits.max_checks,
            max_rationale_chars: limits.max_rationale_chars,
            // ADR-0079 R7-2: /3 の planner には /3 の検証と同じ大きさの上限（既定 64 KiB）を渡す。
            max_plan_json_bytes: if tree_planner {
                limits.max_plan_json_bytes_v3
            } else {
                limits.max_plan_json_bytes
            },
            max_children: limits.max_children,
            previous_attempt_errors,
            tree: tree_planner
                .then(|| self.tree_planner_context(task))
                .transpose()?,
        })
    }

    /// ADR-0079 D2 / D4 (2)（Phase R2b）: この task の planner に /3 を書かせるか（`[execution.tree] enabled` で、
    /// まだ計画が無いか、今の計画が /3）。/1・/2 の計画の replan は従来どおり（/2 の形と差分）。
    pub(super) fn is_tree_planner(&self, task: &Task) -> Result<bool, DispatchError> {
        if !self.config.execution.limits.tree.enabled {
            return Ok(false);
        }
        Ok(match self.store.execution_plan_active(task.id)? {
            None => true,
            Some(plan) => plan.spec.schema == task_core::EXECUTION_PLAN_SCHEMA_V3,
        })
    }

    /// ADR-0079 D4 (2) / D12（Phase R2b）: /3 の planner に渡す木の中の位置（深さ・残りの深さ・祖先）、計画と木の
    /// 上限の残り（`[execution.tree]` と `task_ops::tree::tree_counters`。検証と同じ値）、人の段階の名指し。
    pub(super) fn tree_planner_context(
        &self,
        task: &Task,
    ) -> Result<task_worker::protocol::TreePlannerContext, DispatchError> {
        let depth = task_core::tree::depth_of(task);
        let root_id = task_core::tree::root_id_of(task);
        // ADR-0079 D7（Phase R3a）: 回答（`raise-once`）で足した余裕も残りに含める。
        let tree = self.effective_tree_limits(root_id)?;
        let counters = task_ops::tree::tree_counters(
            self.store.as_ref(),
            root_id,
            self.orchestration.budget_rates(),
        )
        .map_err(ops_to_store)?;
        let open_decisions = self
            .store
            .decisions_list(Some(root_id))?
            .into_iter()
            .filter(|d| d.status == task_core::DecisionStatus::Open)
            .count();
        let node = self.replan_limits(task.id, None, None)?;
        let chain =
            task_ops::tree::ancestors_with_self(self.store.as_ref(), task).map_err(ops_to_store)?;
        let ancestors = chain
            .iter()
            .enumerate()
            .take(chain.len().saturating_sub(1))
            .map(|(i, t)| task_worker::protocol::TreeAncestorContext {
                title: t.title.clone(),
                stage: chain
                    .get(i + 1)
                    .and_then(|next| next.tree.as_ref())
                    .and_then(|tr| tr.parent_unit.as_ref())
                    .map(|u| u.stage.clone()),
            })
            .collect();
        Ok(task_worker::protocol::TreePlannerContext {
            depth,
            max_depth: tree.max_depth,
            remaining_depth: task_core::tree::remaining_depth(depth, tree.max_depth),
            max_stages: tree.max_stages,
            max_units_per_stage: tree.max_units_per_stage,
            max_child_tasks_per_plan: tree.max_child_tasks_per_plan,
            max_decisions_per_plan: tree.max_open_decisions_per_plan,
            max_parallel_child_tasks: tree.max_parallel_child_tasks,
            leaves_left: u64::from(tree.max_tree_leaves.saturating_sub(counters.leaves)),
            runs_left: u64::from(tree.max_tree_runs.saturating_sub(counters.runs)),
            replans_left: u64::from(tree.max_tree_replans.saturating_sub(counters.replans)),
            node_replans_left: u64::from(node.max_replans.saturating_sub(node.replans)),
            usd_left: tree
                .max_tree_usd_micros
                .map(|max| max.saturating_sub(counters.usd_micros) as f64 / 1e6),
            open_decisions_left: tree
                .max_open_decisions_per_tree
                .saturating_sub(open_decisions) as u64,
            ancestors,
            stages_hint: task
                .routing
                .as_ref()
                .map(|r| r.stages_hint.clone())
                .unwrap_or_default(),
        })
    }

    /// ADR-0072 D17（Phase E4b 項目1）: 今の計画の 1 つの WorkUnit を、replan run のプロンプトに
    /// 載せる 1 行に要約する。`done`/`failed` は最新の checkpoint（`last_run_id` から `runs` 索引を
    /// 引く）の `completed`/`known_failures` を使う。checkpoint が無ければ状態だけを出す
    /// （`runs`/`checkpoint` の欠落は既存の run でも起こりうる。D5/D8 の合成規則と同じく「無ければ
    /// 状態だけ」に倒す）。
    pub(super) fn work_unit_plan_summary_line(&self, wu: &task_core::WorkUnitRow) -> String {
        // ADR-0079 D9（Phase R2b）: kind task の unit は子 task の状態と、失敗なら理由と checkpoint の要約。
        if wu.kind == task_core::WorkUnitKind::Task {
            let status = match wu.blocked_reason {
                Some(reason) => format!("{} ({})", wu.status.as_str(), reason.as_str()),
                None => wu.status.as_str().to_string(),
            };
            let child = wu
                .child_task_id
                .as_deref()
                .and_then(|id| id.parse::<TaskId>().ok())
                .and_then(|id| self.store.get(id).ok().flatten());
            let Some(child) = child else {
                return format!("{} (task) status={status}: no child task yet", wu.key);
            };
            let mut line = format!(
                "{} (task) status={status}: child task {} \"{}\" is {}",
                wu.key,
                child.id,
                child.title,
                format!("{:?}", child.status).to_lowercase()
            );
            if matches!(child.status, Status::Failed | Status::Cancelled)
                && let Ok(f) = task_ops::tree::child_failure(self.store.as_ref(), &child)
            {
                line.push_str(&format!(" ({}): {}", f.class.as_str(), f.reason));
                if let Some(cp) = f.checkpoint {
                    line.push_str(&format!("; last checkpoint: {cp}"));
                }
            }
            return line;
        }
        let checkpoint = wu
            .last_run_id
            .as_deref()
            .and_then(|rid| self.store.run_index_get(rid).ok().flatten())
            .and_then(|r| r.checkpoint);
        let status = match wu.blocked_reason {
            Some(reason) => format!("{} ({})", wu.status.as_str(), reason.as_str()),
            None => wu.status.as_str().to_string(),
        };
        let detail = match wu.status {
            task_core::WorkUnitStatus::Done => checkpoint
                .as_ref()
                .map(|cp| cp.completed.join("; "))
                .filter(|s| !s.is_empty()),
            task_core::WorkUnitStatus::Failed => checkpoint.as_ref().and_then(|cp| {
                let joined = cp
                    .known_failures
                    .iter()
                    .map(|f| f.what.clone())
                    .collect::<Vec<_>>()
                    .join("; ");
                if joined.is_empty() {
                    None
                } else {
                    Some(joined)
                }
            }),
            _ => checkpoint.as_ref().and_then(|cp| {
                if cp.next_action.is_empty() {
                    None
                } else {
                    Some(cp.next_action.clone())
                }
            }),
        };
        match detail {
            Some(detail) => format!(
                "{} ({}) status={}: {}",
                wu.key,
                wu.kind.as_str(),
                status,
                detail
            ),
            None => format!("{} ({}) status={}", wu.key, wu.kind.as_str(), status),
        }
    }

    /// ADR-0072 D17（Phase E4b 項目1）: replan の planner run に渡す「起こした理由」。events を
    /// 新しい方から辿り、決定的に文字列化する（events が正本。D5）。
    /// - WU の failed/limit からの replan（`execution_scheduler::decide` が `outcome_str = "replan:
    ///   <why>"` を書く。`crate::dispatcher` の `finish_worker_result_with` 参照）は、その `<why>` をそのまま使う。
    /// - 実質的な review 不合格（D17 4.、`Trigger::ReviewFail` の後の再 dispatch）は、直近の
    ///   `Event::ReviewVerdict{pass:false}` の理由を添える。
    /// - どちらでもなければ（人の依頼 D17 5. など）決定的な既定文を返す。
    pub(super) fn replan_trigger_reason(&self, task_id: TaskId) -> Result<String, DispatchError> {
        let events = self.store.events_for(task_id)?;
        for (_, ev) in events.iter().rev() {
            match ev {
                // ADR-0079 D9（Phase R2b）: 子 task が失敗（work）・中止で終わり、unit が failed になった。理由は
                // 子の分類と理由（最終レビューの不合格の理由を含む）と最後の checkpoint の要約。
                Event::WorkUnitTransitioned {
                    work_unit_id,
                    key,
                    reason,
                    ..
                } if reason == "child_failed" || reason == "child_cancelled" => {
                    let child = self
                        .store
                        .work_unit_get(work_unit_id)?
                        .and_then(|u| u.child_task_id)
                        .and_then(|id| id.parse::<TaskId>().ok())
                        .and_then(|id| self.store.get(id).ok().flatten());
                    let Some(child) = child else {
                        return Ok(format!("child task of unit {key} failed"));
                    };
                    let failure = task_ops::tree::child_failure(self.store.as_ref(), &child)
                        .map_err(ops_to_store)?;
                    let attempt = child
                        .tree
                        .as_ref()
                        .and_then(|t| t.parent_unit.as_ref())
                        .map(|u| u.attempt)
                        .unwrap_or(1);
                    let mut out = format!(
                        "child task \"{}\" (unit {key}, attempt {attempt}, {}) failed ({}): {}",
                        child.title,
                        child.id,
                        failure.class.as_str(),
                        failure.reason
                    );
                    if let Some(cp) = failure.checkpoint {
                        out.push_str(&format!("; the child's last checkpoint: {cp}"));
                    }
                    return Ok(out);
                }
                // ADR 2026-11-07-health-review-integration D3.5 4.: 設計 review の不合格からの replan（findings が理由）。
                Event::DesignReviewed {
                    pass: false,
                    findings,
                    ..
                } => {
                    return Ok(design_gate::design_verdict::design_review_failed_reason(
                        findings,
                    ));
                }
                // ADR-0072「Phase F6 実装時の決定」: 人が後から依頼した replan（「人の指示: <note>」）。
                Event::ExecutionHintSet {
                    replan: true,
                    note,
                    source,
                    ..
                } => {
                    return Ok(match note.as_deref() {
                        Some(n) if !n.is_empty() => format!("人の指示（{source}）: {n}"),
                        _ => format!("a human ({source}) requested a replan of this task"),
                    });
                }
                Event::WorkerFinished { outcome, .. } if outcome.starts_with("replan: ") => {
                    return Ok(outcome
                        .strip_prefix("replan: ")
                        .unwrap_or(outcome)
                        .to_string());
                }
                // ADR-0074 D2.4（Phase F3 途中確認）: 途中確認で人が選んだ replan。「人の指示: <note>」。
                Event::Transitioned { reason, .. }
                    if reason == task_core::PhaseResumeMode::Replan.name() =>
                {
                    return Ok(task_ops::phase_gate::phase_replan_instruction(&events)
                        .unwrap_or_else(|| {
                            "a human requested a replan at a phase checkpoint".to_string()
                        }));
                }
                Event::Transitioned { reason, .. } if reason == "review_fail" => {
                    let reasons: Vec<String> = events
                        .iter()
                        .rev()
                        .filter_map(|(_, e)| match e {
                            Event::ReviewVerdict {
                                pass: false,
                                reason,
                                ..
                            } => Some(reason.clone()),
                            _ => None,
                        })
                        .take(3)
                        .collect();
                    return Ok(if reasons.is_empty() {
                        "the final review failed and could not be repaired locally".to_string()
                    } else {
                        format!(
                            "the final review failed and could not be repaired locally: {}",
                            reasons.join("; ")
                        )
                    });
                }
                _ => {}
            }
        }
        Ok("a human or the daemon requested a replan".to_string())
    }
}

/// Phase F5-fix3: 拒否した planner の計画の移し先（`artifacts/` の中）。
pub(super) fn rejected_plan_file(run_id: &str, attempt: usize) -> String {
    format!("execution-plan.rejected.{run_id}.attempt-{attempt}.json")
}
// Phase F5-fix3: `give_up_or_retry_planner` の進捗・質問の文面と、それを読む `planner_rejections_since_last_plan` は、
// 生存確認（`planner_retry_pending`）と同じ所（`task_ops::tree`）に置く（ADR-0079 D10、ADR 2026-11-24-planner-supply-failure-cooldown）。
/// ADR-0072 D14（Phase E3）: 計画の各 WorkUnit の `harness` が `[[genres]]` にある id だけかを確かめる
/// （担当の profile が許す harness に限る、の簡易版。`genres` が空の設定では検証しない。既存の
/// `[[genres]] roles` の検証と同じ考え方）。
/// planner の計画の採用前の検査: harness（[`validate_plan_harnesses`]）と、unit の `task:<id>` 依存
/// （ADR 2026-11-11-unit-task-dependencies D4。存在・自分の木の外・同じ案件・循環なし。人・CoS の採用は task-ops の
/// `validate_for_task` が同じ関数で見る）。違えば planner に返す理由。
fn plan_preflight(
    d: &Dispatcher,
    task: &Task,
    spec: &task_core::ExecutionPlanSpec,
) -> Result<Result<(), String>, DispatchError> {
    if let Err(e) = validate_plan_harnesses(spec, &d.config.genres) {
        return Ok(Err(e));
    }
    let errors = task_ops::unit_task_deps::plan_errors(d.store.as_ref(), task, spec)?;
    Ok(task_ops::execution::describe_task_dependency_errors(&errors).map_or(Ok(()), Err))
}

pub(super) fn validate_plan_harnesses(
    spec: &task_core::ExecutionPlanSpec,
    genres: &[GenreSpec],
) -> Result<(), String> {
    if genres.is_empty() {
        return Ok(());
    }
    for wu in &spec.work_units {
        if let Some(harness) = &wu.harness
            && !genres.iter().any(|g| &g.id == harness)
        {
            return Err(format!(
                "work unit {} has unknown harness {harness:?} (not in [[genres]])",
                wu.key
            ));
        }
    }
    Ok(())
}

/// ADR 2026-11-24-planner-supply-failure-cooldown D1–D3: run が供給側の失敗（`provider_failure_outcome`）で終わったなら、
/// worker と同じ `record_supply_failure` で account（AuthFailed は再ログインまで保持し人に通知）か source を cooldown にし、
/// (計画の試行に数えず requeue するか, 理由, source の cooldown の event) を返す。requeue は AuthFailed なら常に、Throttled・
/// Exhausted は続いた requeue が `max_requeues` 未満の間（worker の `settle_adapter_failure` と同じ規則）。上限に達したら
/// 理由を不正な試行の文にする。供給側でなければ `None`。
fn planner_supply_failure(
    d: &mut Dispatcher,
    task_id: TaskId,
    account: (Option<&str>, Option<AccountAdapter>),
    provider: &str,
    result: &Result<RunOutcome, AdapterError>,
) -> Result<Option<(bool, String, Option<Event>)>, DispatchError> {
    let Some(e) = result.as_ref().err() else {
        return Ok(None);
    };
    let (Some(reason), Some(outcome)) = (provider_failure_reason(e), provider_failure_outcome(e))
    else {
        return Ok(None);
    };
    let cooldown = record_supply_failure(d, account, provider, reason, &outcome);
    let max = d.config.max_requeues;
    let requeue = matches!(outcome, ProviderOutcome::AuthFailed)
        || task_ops::derive::consecutive_requeues(&d.store.events_for(task_id)?) < max;
    let reason = match requeue {
        true => e.to_string(),
        false => format!("requeue limit ({max}) reached: adapter: {e}"),
    };
    Ok(Some((requeue, reason, cooldown)))
}

/// ADR-0072 D14: planner の `artifacts/execution-plan.json` の検証。run が `Terminal::Done` か turn の上限で終わったときだけ
/// 試みる（ADR 2026-11-09-planner-turn-limit-and-comment-interrupt D1。それ以外は「不正な試行」）。
/// ADR-0079 D3 / D4 (3)（Phase R2a）: /3 の計画が計画の上限（段階の数・段階あたりの unit・子 task・`max_depth`）だけで不正なら、
/// 1 回目は従来どおり不正な試行（planner に理由を渡して再試行）、最後の試行では上限を緩めて採用し、超えた分の unit を
/// `kind: limit` の決定の要求で止める（黙って切らない・atomic に倒さない）。採用に使う上限を `adopt_limits` に返す。
fn planner_plan_validation(
    d: &Dispatcher,
    task: &Task,
    result: &Result<RunOutcome, AdapterError>,
    describe: &str,
    (active_plan, done_work_units): (
        Option<&task_core::ExecutionPlanRow>,
        &[(String, task_core::WorkUnitSpec)],
    ),
    adopt_limits: &mut task_core::ExecutionLimits,
) -> Result<Result<task_core::execution_plan::ValidatedPlan, String>, DispatchError> {
    if !design_gate::plan_file_checked(result) {
        return Ok(Err(format!(
            "planner run did not finish cleanly: {describe}"
        )));
    }

    let workspace_dir = d.task_dir(task);
    let artifacts_dir = workspace_dir.as_ref().map(|dir| d.artifacts_dir(task, dir));
    Ok(
        match artifacts_dir
            .as_deref()
            .map(|dir| dir.join("execution-plan.json"))
            .and_then(|p| std::fs::read_to_string(p).ok())
        {
            // D1: run の終わり方（turn の上限か）を理由に添え、次の planner に渡す。
            None => Err(format!(
                "artifacts/execution-plan.json が見つからない（{describe}）"
            )),
            Some(text) => match task_ops::execution::parse_planner_output(
                &text,
                active_plan,
                &done_work_units.iter().map(|(k, _)| k.clone()).collect(),
            ) {
                Err(e) => Err(e),
                Ok(spec) => match plan_preflight(d, task, &spec)? {
                    Err(e) => Err(e),
                    Ok(()) => {
                        let ctx = task_core::PlanContext {
                            origin: task_core::PlanOrigin::Planner,
                            depth: task_core::tree::depth_of(task),
                        };
                        match task_core::execution_plan::validate_with(
                            &spec,
                            d.config.execution.limits,
                            done_work_units,
                            ctx,
                        ) {
                            Ok(v) => Ok(v),
                            Err(errors)
                                if spec.schema == task_core::EXECUTION_PLAN_SCHEMA_V3
                                    && errors.iter().all(is_tree_plan_limit_error)
                                    && d.planner_attempts_in_window(task.id)?
                                        >= MAX_PLANNER_ATTEMPTS =>
                            {
                                let relaxed = relaxed_tree_plan_limits(d.config.execution.limits);
                                match task_core::execution_plan::validate_with(
                                    &spec,
                                    relaxed,
                                    done_work_units,
                                    ctx,
                                ) {
                                    Ok(v) => {
                                        tracing::warn!(task_id = %task.id, errors = %task_ops::execution::describe_validation_errors(&errors), "the /3 plan breaches only the plan limits on the last attempt; adopting it and holding the excess units for a limit decision (ADR-0079 D3)");
                                        *adopt_limits = relaxed;
                                        Ok(v)
                                    }
                                    Err(e) => {
                                        Err(task_ops::execution::describe_validation_errors(&e))
                                    }
                                }
                            }
                            Err(errors) => {
                                Err(task_ops::execution::describe_validation_errors(&errors))
                            }
                        }
                    }
                },
            },
        },
    )
}
