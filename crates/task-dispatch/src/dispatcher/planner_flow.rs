//! ADR-0082 の責務分割。

use super::*;

impl Dispatcher {
    /// ADR-0072 D14（Phase E3）: task-local な planner run の終わり方を判定する。`current_wu` が無い
    /// （計画がまだ無い）Task の `RunRole::Planner` run はすべてここを通る。
    ///
    /// `artifacts/execution-plan.json` を D14 で検証し、
    /// - 妥当（かつ run が `Terminal::Done` で終わった）なら採用して `Trigger::Continue{Planned}`。
    /// - 不正・run 自体が異常終了（error/question/budget_exhausted/yielded）なら、1 回だけ再試行する
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
        // ADR-0013 D9 / S10 と同じ扱い（planner run は account pool のプロバイダを使わない前提。
        // `[execution.planner].adapter` は既定 claude-code で、アカウントの当たり外れは通常の
        // policy 報告に任せる）。
        let policy_outcome = match &result {
            Ok(_) => ProviderOutcome::Ok,
            Err(e) => provider_failure_outcome(e).unwrap_or(ProviderOutcome::Ok),
        };
        self.policy.report(provider.clone(), &policy_outcome);

        let (run_end, describe, usage): (
            Option<task_core::RunEnd>,
            String,
            Option<task_core::Usage>,
        ) = match &result {
            Ok(RunOutcome {
                terminal: Terminal::Done { summary, usage, .. },
                ..
            }) => (
                Some(task_core::RunEnd::Completed),
                format!("done: {summary}"),
                *usage,
            ),
            Ok(RunOutcome {
                terminal: Terminal::Question { text },
                ..
            }) => (
                Some(task_core::RunEnd::Question),
                format!("question: {text}"),
                None,
            ),
            Ok(RunOutcome {
                terminal: Terminal::Error { message, retryable },
                ..
            }) => (
                Some(task_core::RunEnd::Failed {
                    retryable: *retryable,
                }),
                format!("error(retryable={retryable}): {message}"),
                None,
            ),
            Ok(RunOutcome {
                terminal: Terminal::Yielded { usage, .. },
                ..
            }) => (
                Some(task_core::RunEnd::Yielded),
                "yielded (planner runs are not continued; treated as an invalid attempt)"
                    .to_string(),
                *usage,
            ),
            Ok(RunOutcome {
                terminal:
                    Terminal::BudgetExhausted {
                        kind,
                        message,
                        usage,
                    },
                ..
            }) => (
                Some(task_core::RunEnd::BudgetExhausted { kind: *kind }),
                format!("budget_exhausted({kind:?}): {message}"),
                *usage,
            ),
            Err(e) => (None, format!("infra error: {e}"), None),
        };

        // ADR-0076: planner run の quota 消費も worker と同じ `resolve_quota_estimate` で見積もる。
        // 以降の `?` で抜けても `QuotaActivity` が閉じているよう、分岐より前に一度だけ求め、
        // `WorkerFinished` を保存するすべての分岐で同じトランザクションに添える。
        let quota_event = {
            let model = self.started_model_of(task_id, &run_id);
            self.resolve_quota_estimate(
                task_id,
                &run_id,
                None,
                account,
                account_adapter,
                &provider,
                &model,
                usage.as_ref(),
            )
        };

        // ADR-0072 D17（Phase E4）: この run が replan（既に `active` な計画がある）なら、done の
        // WU の key/spec が変わっていないことも検証する（D14「replan のときは done の WU の key と
        // spec が変わっていないこと」）。
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
        // D14: 計画の検証は run が `Terminal::Done` で終わったときだけ試みる（それ以外は無条件に
        // 「不正な試行」として扱う）。
        // ADR-0079 D3 / D4 (3)（Phase R2a）: /3 の計画が計画の上限（段階の数・段階あたりの unit・子 task・
        // `max_depth`）だけで不正なら、1 回目は従来どおり不正な試行（planner に理由を渡して再試行）、最後の
        // 試行では上限を緩めて採用し、超えた分の unit を `kind: limit` の決定の要求で止める（黙って切らない・
        // atomic に倒さない）。採用に使う上限を `adopt_limits` に返す。
        let mut adopt_limits = self.config.execution.limits;
        let validation: Result<task_core::execution_plan::ValidatedPlan, String> = if matches!(
            result,
            Ok(RunOutcome {
                terminal: Terminal::Done { .. },
                ..
            })
        ) {
            let workspace_dir = self.task_dir(task);
            let artifacts_dir = workspace_dir.as_ref().map(|d| self.artifacts_dir(task, d));
            match artifacts_dir
                .as_deref()
                .map(|d| d.join("execution-plan.json"))
                .and_then(|p| std::fs::read_to_string(p).ok())
            {
                None => Err("artifacts/execution-plan.json が見つからない".to_string()),
                Some(text) => match parse_planner_output(
                    &text,
                    active_plan.as_ref(),
                    &done_work_units.iter().map(|(k, _)| k.clone()).collect(),
                ) {
                    Err(e) => Err(e),
                    Ok(spec) => match validate_plan_harnesses(&spec, &self.config.genres) {
                        Err(e) => Err(e),
                        Ok(()) => {
                            let ctx = task_core::PlanContext {
                                origin: task_core::PlanOrigin::Planner,
                                depth: task_core::tree::depth_of(task),
                            };
                            match task_core::execution_plan::validate_with(
                                &spec,
                                self.config.execution.limits,
                                &done_work_units,
                                ctx,
                            ) {
                                Ok(v) => Ok(v),
                                Err(errors)
                                    if spec.schema == task_core::EXECUTION_PLAN_SCHEMA_V3
                                        && errors.iter().all(is_tree_plan_limit_error)
                                        && self.planner_attempts_in_window(task_id)?
                                            >= MAX_PLANNER_ATTEMPTS =>
                                {
                                    let relaxed =
                                        relaxed_tree_plan_limits(self.config.execution.limits);
                                    match task_core::execution_plan::validate_with(
                                        &spec,
                                        relaxed,
                                        &done_work_units,
                                        ctx,
                                    ) {
                                        Ok(v) => {
                                            tracing::warn!(%task_id, %run_id, errors = %task_ops::execution::describe_validation_errors(&errors), "the /3 plan breaches only the plan limits on the last attempt; adopting it and holding the excess units for a limit decision (ADR-0079 D3)");
                                            adopt_limits = relaxed;
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
            }
        } else {
            Err(format!("planner run did not finish cleanly: {describe}"))
        };

        let metrics = run_since.map(|since| task_core::RunMetrics {
            wall_ms: wall_ms_since(since),
            retries: task.attempts,
            peak_context_tokens: None,
            turns: None,
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
            Ok(validated) => {
                // ADR-0079 D4 (3)（Phase R2a）: /3 の各 unit に unit の gate をかけ、leaf を task に上げる・
                // task を leaf に下げる（採用する計画の spec に当てる）。子 task にできない深さの compound な
                // leaf と、上限を超える unit は採用の直後に決定の要求で止める（`apply_tree_plan_holds`）。
                let (validated, tree_plan) =
                    self.tree_plan_gate(task, validated, &done_work_units, &mut adopt_limits);
                let outcome_str = format!(
                    "done: {} work unit(s) planned",
                    validated.spec.work_units.len()
                );
                let finished = Event::WorkerFinished {
                    run_id: run_id.clone(),
                    outcome: outcome_str,
                    usage,
                    role: Some(RunRole::Planner),
                    metrics,
                    end: run_end,
                };
                // ADR-0074 D3.7（Phase F4b (f)）: planner の `children` を既存の委譲の検証に通す
                // （初回の採用だけ。replan で新しい子を足すことはしない — 既存の子の key だけを許す）。
                let children = if active_plan.is_some() {
                    let existing: std::collections::BTreeSet<String> = self
                        .store
                        .children(task_id)?
                        .into_iter()
                        .flat_map(|c| c.labels.into_iter())
                        .collect();
                    match validated
                        .spec
                        .children
                        .iter()
                        .find(|c| !existing.contains(&task_core::child_label(&c.key)))
                    {
                        Some(c) => Err(format!(
                            "child {} is new; children can only be added when the plan is first adopted",
                            c.key
                        )),
                        None => Ok(task_ops::delegate::ChildrenPlan::Ready(Vec::new())),
                    }
                } else {
                    task_ops::delegate::plan_children(
                        self.store.as_ref(),
                        task,
                        &validated.spec.children,
                        &self.config.roles,
                        &self.config.genres,
                        &self.config.delegation,
                        now,
                    )
                };
                // ADR-0079 D2 / D4 (4)（Phase R1b）: /3 の kind task の unit は、repos が親の部分集合で
                // あること、部をまたぐ子の認可（ADR-0074 F4b と同じ規則）を採用の前に確かめる。
                let children = match children {
                    Ok(task_ops::delegate::ChildrenPlan::Ready(c))
                        if validated.spec.schema == task_core::EXECUTION_PLAN_SCHEMA_V3 =>
                    {
                        self.tree_plan_checks(task, &validated.spec, now)
                            .map(|plan| match plan {
                                task_ops::delegate::ChildrenPlan::Ready(_) => {
                                    task_ops::delegate::ChildrenPlan::Ready(c)
                                }
                                other => other,
                            })
                    }
                    other => other,
                };
                let children = match children {
                    Ok(task_ops::delegate::ChildrenPlan::Ready(children)) => children,
                    Ok(task_ops::delegate::ChildrenPlan::NeedsAuthorization(questions)) => {
                        // SPEC §3.1 / ADR-0033 D4・D5: 部をまたぐ子は秘書（人）への質問。認可されたら
                        // planner をもう一度走らせ、同じ子が認可済みとして通る。
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
                    Err(reason) => {
                        self.give_up_or_retry_planner(
                            task_id,
                            task,
                            &run_id,
                            vec![finished, quota_event],
                            format!("子 Task の提案が委譲の検証に通りませんでした: {reason}"),
                            now,
                        )?;
                        return Ok(());
                    }
                };
                let adopted = if active_plan.is_some() {
                    // ADR-0072 D17（Phase E4）: replan。done の WU は保持し、旧版を supersede する。
                    task_ops::execution::replan(
                        self.store.as_ref(),
                        task_id,
                        validated.spec,
                        "replan (planner run)".to_string(),
                        task_core::PlanOrigin::Planner,
                        Some(run_id.clone()),
                        adopt_limits,
                        now,
                    )
                    .map(|(plan, _diff)| plan)
                } else {
                    task_ops::execution::adopt_plan_with_children(
                        self.store.as_ref(),
                        task_id,
                        validated.spec,
                        task_core::PlanOrigin::Planner,
                        Some(run_id.clone()),
                        adopt_limits,
                        now,
                        children,
                    )
                };
                match adopted {
                    Ok(plan) => {
                        if let Some(tree_plan) = tree_plan
                            && let Err(e) =
                                self.apply_tree_plan_holds(task, &plan, &run_id, tree_plan, now)
                        {
                            tracing::warn!(%task_id, %run_id, error = %e, "failed to record the unit gate / tree limit holds of the adopted plan (ADR-0079 R2a)");
                        }
                        // ADR-0079 D8（Phase R3b）: root の /3 の計画は、決定を含む・`review: human` の段階・上限に
                        // 近い、のどれかなら人の承認を待つ（`PlanGate`、unit を 1 つも起こさない）。そうでなければ
                        // 進め、報告の流れに 1 件だけ残す（Discord は鳴らさない。U-R3）。
                        let approval = match self.root_plan_approval(task, &plan) {
                            Ok(a) => a,
                            Err(e) => {
                                tracing::warn!(%task_id, %run_id, error = %e, "failed to evaluate the root plan approval; asking a human to be safe (ADR-0079 D8)");
                                Some(task_core::PlanApproval {
                                    required: true,
                                    reasons: vec![format!("evaluation_failed:{e}")],
                                })
                            }
                        };
                        match approval {
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
                                if approval.is_some() {
                                    let (headline, body) =
                                        task_ops::plan_gate::plan_notice(&plan.spec);
                                    if let Err(e) = task_ops::plan_gate::record_plan_notice(
                                        self.store.as_ref(),
                                        task,
                                        &headline,
                                        &body,
                                        now,
                                    ) {
                                        tracing::warn!(%task_id, error = %e, "failed to record the plan notice report (ADR-0079 D8)");
                                    }
                                }
                            }
                        }
                    }
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
            Err(reason) => {
                let finished = Event::WorkerFinished {
                    run_id: run_id.clone(),
                    outcome: format!("error(retryable=true): invalid execution plan: {reason}"),
                    usage,
                    role: Some(RunRole::Planner),
                    metrics,
                    end: run_end,
                };
                self.give_up_or_retry_planner(
                    task_id,
                    task,
                    &run_id,
                    vec![finished, quota_event],
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
    /// `artifacts/execution-plan.rejected.json` に移す（上書き）。無ければ何もしない。失敗しても警告だけ。
    pub(super) fn set_aside_rejected_plan(&self, task: &Task) {
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
        let to = dir.join(REJECTED_PLAN_FILE);
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
        // 思い込みそのまま再提出する（dogfood 4 回目の 2 回目の試行）。`execution-plan.rejected.json` に移す。
        self.set_aside_rejected_plan(task);
        let attempts_so_far = self.planner_attempts_in_window(task_id)?;
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
    /// （`WorkerStarted{role: Planner}` の件数。この run 自身を含む）。
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
            .filter(|(i, (_, e))| {
                since_idx.is_none_or(|s| *i > s)
                    && matches!(
                        e,
                        Event::WorkerStarted {
                            role: Some(RunRole::Planner),
                            ..
                        }
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
            max_plan_json_bytes: limits.max_plan_json_bytes,
            max_children: limits.max_children,
            previous_attempt_errors,
            tree: if self.is_tree_planner(task)? {
                Some(self.tree_planner_context(task)?)
            } else {
                None
            },
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
        let counters =
            task_ops::tree::tree_counters(self.store.as_ref(), root_id).map_err(ops_to_store)?;
        let open_decisions = self
            .store
            .decisions_list(Some(root_id))?
            .into_iter()
            .filter(|d| d.status == task_core::DecisionStatus::Open)
            .count();
        let node_replans = self
            .store
            .execution_plan_list(task.id)?
            .len()
            .saturating_sub(1) as u64;
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
                objective_excerpt: t.objective.chars().take(300).collect(),
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
            node_replans_left: u64::from(self.effective_max_replans(task.id)?)
                .saturating_sub(node_replans),
            tokens_left: tree
                .max_tree_tokens
                .map(|max| max.saturating_sub(counters.tokens)),
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
    ///   <why>"` を書く。`crate::dispatcher` の `finish_worker_result` 参照）は、その `<why>` をそのまま使う。
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
