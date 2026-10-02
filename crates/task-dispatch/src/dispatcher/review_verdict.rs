//! review の判定の適用（`on_review_finished`・`try_review_repair`）。Plan の子の挿入と `ReviewPass` は同じ transaction に入れる。ADR-0082 の L3。

use super::*;

impl Dispatcher {
    pub(super) fn on_review_finished(
        &mut self,
        task_id: TaskId,
        run_id: String,
        mut outcome: ReviewOutcome,
    ) -> Result<(), DispatchError> {
        let entry = self.reviewing.remove(&task_id);
        // ADR-0014 D1: Reviewer run の終わりを WorkerFinished{role: reviewer} として残す（判定の適用・延期・破棄のどれでも）。
        let completed_review_run = outcome.reviewer_run.as_ref().map(|r| r.run_id.clone());
        // ADR-0061（Phase 104）: `retries` は Reviewer run には無い概念（対象タスクの `attempts` とは別軸）
        // なので 0 固定。wall time は `ReviewEntry.since` から計算する。
        let review_metrics = entry.as_ref().map(|e| task_core::RunMetrics {
            wall_ms: wall_ms_since(e.since),
            retries: 0,
            peak_context_tokens: None,
            turns: None,
        });
        let mut reviewer_finished = outcome.reviewer_run.take().map(|r| Event::WorkerFinished {
            run_id: r.run_id,
            outcome: r.outcome,
            usage: r.usage,
            role: Some(RunRole::Reviewer),
            metrics: review_metrics,
            end: None,
        });
        // ADR-0076: Reviewer run の quota 消費も worker と同じ `resolve_quota_estimate` で一度だけ
        // 見積もる（対象タスクが消えた・stale でも `QuotaActivity` を閉じるため、分岐より前）。
        // Reviewer run を起こさなかったレビュー（command 等だけ）には作らない。
        let reviewer_quota = match (&completed_review_run, entry.as_ref()) {
            (Some(review_run_id), Some(e)) => e.provider.clone().map(|provider| {
                let model = self.started_model_of(task_id, review_run_id);
                let usage = worker_finished_usage(&reviewer_finished);
                self.resolve_quota_estimate(
                    task_id,
                    review_run_id,
                    None,
                    e.account.as_deref(),
                    e.account_adapter,
                    &provider,
                    &model,
                    usage.as_ref(),
                )
            }),
            _ => None,
        };
        let Some(task) = self.store.get(task_id)? else {
            return Ok(());
        };
        // ADR-0054 D1（Phase 67）: この run が Reviewer run を伴っていれば、対象タスクの部署の根ノード
        // （engineering/research/operations）の継続セッション（`kind = lead`）に usage を積む（rollover
        // 判定の材料。turns も 1 進む）。部署が無い・対応しないアダプタでは `node_session_touch` が no-op。
        if let Some(Event::WorkerFinished { usage, .. }) = &reviewer_finished
            && let Some(node_id) = task.assignee.as_deref()
        {
            match self.store.org_list() {
                Ok(org) => {
                    if let Some(department) = task_core::department_of(&org, node_id) {
                        let tokens = usage
                            .as_ref()
                            .map(|u| u.input_tokens.unwrap_or(0) + u.output_tokens.unwrap_or(0))
                            .unwrap_or(0);
                        if let Err(e) = self.store.node_session_touch(
                            &department,
                            task_core::SessionKind::Lead,
                            None,
                            tokens as i64,
                            OffsetDateTime::now_utc(),
                        ) {
                            tracing::warn!(%task_id, error = %e, "failed to record department lead session usage");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(%task_id, error = %e, "failed to list org for department lead session touch");
                }
            }
        }
        if task.status != Status::Reviewing {
            tracing::warn!(%task_id, status = ?task.status, "review result discarded (task no longer reviewing)");
            set_worker_finished_end(&mut reviewer_finished, task_core::RunEnd::Cancelled);
            for ev in reviewer_finished.iter().chain(reviewer_quota.iter()) {
                self.store.append_event(task_id, ev)?;
            }
            finish_reviewer_run_index(
                self.store.as_ref(),
                &completed_review_run,
                task_core::RunIndexStatus::Cancelled,
                worker_finished_usage(&reviewer_finished),
                review_metrics,
            );
            return Ok(());
        }
        // ADR-0118 D4 付記: 検査中に review snapshot が変わった判定は適用しない（stale は不合格ではない）。
        // attempts を消費せず reviewing のまま、次の tick の review 入口で再 sync → 再 check → 再 review。
        if let Some(stale) = outcome.target_stale.take() {
            let review_run = completed_review_run
                .clone()
                .unwrap_or_else(|| run_id.clone());
            self.defer_stale_review(
                task_id,
                &run_id,
                &review_run,
                stale.repo_id,
                &stale.reviewed_sha,
                &stale.target_sha,
                &stale.reason,
            )?;
            set_worker_finished_end(&mut reviewer_finished, task_core::RunEnd::Cancelled);
            for ev in reviewer_finished.iter().chain(reviewer_quota.iter()) {
                self.store.append_event(task_id, ev)?;
            }
            finish_reviewer_run_index(
                self.store.as_ref(),
                &completed_review_run,
                task_core::RunIndexStatus::Cancelled,
                worker_finished_usage(&reviewer_finished),
                review_metrics,
            );
            if let Some(entry) = entry {
                self.pending_subjects.insert(task_id, entry.subject);
            }
            return Ok(());
        }
        let mut throttled_events = Vec::new();
        if let Some(pf) = outcome.provider_failure.take() {
            // ADR-0054 D2（Phase 113）: `pf.outcome = Some(..)` はプロバイダが分類できた供給側失敗
            // （Throttled/Exhausted/AuthFailed）で、従来どおりプロバイダ/アカウントの cooldown に
            // 報告する。`None` は「reviewer run 自身のインフラ都合の失敗」（is_error の結果・
            // プロセス失敗・resume 拒否など）で、こちらは cooldown の対象にしない
            // （プロバイダ・アカウントの問題ではなく、たまたまこの run が失敗しただけのため）。
            if let Some(classified) = pf.outcome.clone() {
                let review_account = entry.as_ref().and_then(|e| e.account.clone());
                let review_account_adapter = entry.as_ref().and_then(|e| e.account_adapter);
                if let Some(provider) = entry.as_ref().and_then(|e| e.provider.clone()) {
                    // ADR-0024 D4: プール経由の Reviewer run の失敗もプロバイダを cooldown にせず、アカウントに向ける。
                    let policy_outcome = if review_account.is_some() {
                        ProviderOutcome::Ok
                    } else {
                        classified.clone()
                    };
                    self.policy.report(provider.clone(), &policy_outcome);
                    match (&review_account, review_account_adapter) {
                        (Some(acct), Some(adapter)) => self.record_account_failure(
                            adapter,
                            acct,
                            cooldown_reason_name(&classified),
                            &classified,
                        ),
                        _ => {
                            if let Some(ev) = self.provider_throttled_event(
                                &provider,
                                &classified,
                                cooldown_reason_name(&classified),
                            ) {
                                throttled_events.push(ev);
                            }
                        }
                    }
                }
            }
            // ADR-0054 D2（Phase 113）: 分類できた供給側失敗は従来どおり `max_requeues` /
            // `REVIEWER_REQUEUED_PREFIX` で数える。reviewer run 自身のインフラ都合の失敗は、
            // 別のカウンタ・別の上限（`[review] max_reviewer_retries`、既定 3。criterion ごとではなく
            // この reviewing 試行での連続失敗回数だが、Reviewer 条件は同じ run でまとめて判定される
            // ため実質的に criterion ごとの回数と一致する）で数え、プロバイダの cooldown 回数とは
            // 混ぜない。
            let is_infra_failure = pf.outcome.is_none();
            let (deferrals, limit, prefix) = if is_infra_failure {
                (
                    consecutive_reviewer_infra_failures(&self.store.events_for(task_id)?),
                    self.config.max_reviewer_retries,
                    REVIEWER_INFRA_FAILURE_PREFIX,
                )
            } else {
                (
                    consecutive_reviewer_requeues(&self.store.events_for(task_id)?),
                    self.config.max_requeues,
                    REVIEWER_REQUEUED_PREFIX,
                )
            };
            if deferrals < limit {
                // ADR-0010 D5（P-29）/ ADR-0054 D2: Reviewer run の供給側・インフラ失敗は判定しない。
                // reviewing のまま次 tick に回す（attempts を消費しない）。
                self.store.append_event(
                    task_id,
                    &Event::worker_progress(run_id.clone(), format!("{prefix}{}", pf.message)),
                )?;
                // (k): event 自身の `end` も `HarnessError` に揃える（供給側 = Supply、reviewer run
                // 自身のインフラ都合 = Infra）。
                set_worker_finished_end(
                    &mut reviewer_finished,
                    task_core::RunEnd::HarnessError {
                        class: if is_infra_failure {
                            task_core::HarnessErrorClass::Infra
                        } else {
                            task_core::HarnessErrorClass::Supply
                        },
                    },
                );
                for ev in reviewer_finished.iter().chain(reviewer_quota.iter()) {
                    self.store.append_event(task_id, ev)?;
                }
                for ev in &throttled_events {
                    self.store.append_event(task_id, ev)?;
                }
                finish_reviewer_run_index(
                    self.store.as_ref(),
                    &completed_review_run,
                    task_core::RunIndexStatus::HarnessError,
                    worker_finished_usage(&reviewer_finished),
                    review_metrics,
                );
                if let Some(entry) = entry {
                    self.pending_subjects.insert(task_id, entry.subject);
                }
                tracing::warn!(%task_id, %run_id, reason = %pf.message, infra = is_infra_failure, "reviewer run hit a provider/infra failure; review deferred");
                return Ok(());
            }
            // ADR-0011（P-38）/ ADR-0054 D2: 連続延期・連続インフラ失敗が上限に達したら、未判定の
            // Reviewer 条件を fail として通常どおり判定を適用する（人の承認・command・artifact_exists
            // の結果は `outcome.verdicts` に既に入っているのでそのまま残る）。
            let limit_message = if is_infra_failure {
                format!("reviewer infra failure ×{limit}: {}", pf.message)
            } else {
                format!("requeue limit ({limit}) reached: {}", pf.message)
            };
            tracing::warn!(%task_id, %run_id, reason = %pf.message, limit, infra = is_infra_failure, "reviewer run retry limit reached; failing reviewer criteria");
            if let Some(Event::WorkerFinished {
                outcome: finished_outcome,
                ..
            }) = reviewer_finished.as_mut()
            {
                *finished_outcome = format!("error(retryable=false): {limit_message}");
            }
            for (idx, criterion) in task.acceptance.iter().enumerate() {
                if matches!(criterion.check, Check::Reviewer)
                    && !outcome.verdicts.iter().any(|v| v.criterion_idx == idx)
                {
                    outcome.verdicts.push(Verdict {
                        criterion_idx: idx,
                        pass: false,
                        reason: limit_message.clone(),
                        repair_hint: None,
                    });
                }
            }
            outcome.verdicts.sort_by_key(|v| v.criterion_idx);
        }
        let all_pass = outcome.all_pass();
        if let Some(old) = self.store.delivery_get(task_id)?
            && old.worker_run == run_id
            && completed_review_run.as_deref() == Some(old.review_run.as_str())
            && old.state == task_core::DeliveryState::Reviewing
            && let Some(verdict) = outcome
                .verdicts
                .iter()
                .find(|v| v.criterion_idx == old.criterion_idx)
        {
            let mut next = old.clone();
            next.decision = Some(all_pass && verdict.pass);
            next.state = if all_pass && verdict.pass {
                task_core::DeliveryState::MergeQueued
            } else {
                task_core::DeliveryState::Blocked
            };
            next.detail = outcome
                .verdicts
                .iter()
                .filter(|v| !v.pass || v.criterion_idx == old.criterion_idx)
                .map(|v| v.reason.clone())
                .collect::<Vec<_>>()
                .join("\n");
            let review_reason = verdict
                .reason
                .strip_prefix(&format!("reviewer({}): ", old.review_run))
                .unwrap_or(&verdict.reason);
            if review_reason.trim_start().starts_with("[needs-human]") {
                next.detail = format!("{}\n{}", review_reason, next.detail);
            }
            self.store.delivery_save(Some(&old), &next)?;
        }

        // ADR-0034 D2（監査 M-1〜M-3）: レビュー不合格の理由（この run が `Status::Failed` に直結した場合の
        // bad_news の材料。`Status::Ready` に戻るだけの途中の失敗では使わない）。
        let review_fail_message = if all_pass {
            None
        } else {
            let reasons: Vec<String> = outcome
                .verdicts
                .iter()
                .filter(|v| !v.pass)
                .map(|v| v.reason.clone())
                .collect();
            Some(reasons.join("; "))
        };
        let reviewer_index_status = match &reviewer_finished {
            Some(Event::WorkerFinished { outcome, .. })
                if outcome.starts_with("error(retryable=false)") =>
            {
                task_core::RunIndexStatus::Failed
            }
            _ => task_core::RunIndexStatus::Completed,
        };
        // ADR-0074 §6 F1 (k): event 自身の `end` も同じ判定に揃える（`replay --check` が
        // `rebuild_work_units_and_runs` で正しく `status` を復元できるように）。
        set_worker_finished_end(
            &mut reviewer_finished,
            match reviewer_index_status {
                task_core::RunIndexStatus::Failed => task_core::RunEnd::Failed { retryable: false },
                _ => task_core::RunEnd::Completed,
            },
        );
        finish_reviewer_run_index(
            self.store.as_ref(),
            &completed_review_run,
            reviewer_index_status,
            worker_finished_usage(&reviewer_finished),
            review_metrics,
        );
        let mut events: Vec<Event> = reviewer_finished
            .into_iter()
            .chain(reviewer_quota)
            .chain(outcome.verdicts.iter().map(|v| Event::ReviewVerdict {
                run_id: run_id.clone(),
                criterion_idx: v.criterion_idx,
                pass: v.pass,
                reason: v.reason.clone(),
            }))
            .chain(throttled_events)
            .collect();
        // ADR-0016 D2 / M5: 全 pass でも委譲した子が終端でなければ、判定だけ記録して reviewing のまま待つ。
        if all_pass {
            let pending = pending_children(self.store.as_ref(), task_id).map_err(ops_to_store)?;
            if pending > 0 {
                for ev in &events {
                    self.store.append_event(task_id, ev)?;
                }
                self.store.append_event(
                    task_id,
                    &Event::worker_progress(
                        run_id.clone(),
                        format!("waiting for {pending} delegated child task(s) before completing"),
                    ),
                )?;
                self.awaiting_children.insert(
                    task_id,
                    AwaitingChildren {
                        run_id: run_id.clone(),
                        plan: outcome.plan,
                    },
                );
                tracing::info!(%task_id, %run_id, pending, "review passed; waiting for delegated children");
                return Ok(());
            }
            // ADR-0021 D1: 子が失敗していたら、集約・完了より先に「やり直す or 人に聞く」。
            if self.escalate_failed_children(&task, &run_id, &mut events)? {
                return Ok(());
            }
            // ADR-0016 D3 / M4: 子が全て終端で、まだ集約 run をしていなければ集約 run を予約する。
            if self.needs_aggregate_run(&task)? {
                return self.schedule_aggregate_run(task_id, &run_id, events);
            }
        }
        // ADR-0072 D16（Phase E4）: 修復できる不合格（class が揃っていて、repair の上限内）なら、
        // `ReviewFail` の代わりに `ReviewRepair`（attempts 据え置き）+ 最小の context の repair WU で
        // 直す。上限を超えている・修復できない種類が混ざっていれば `None`（従来どおり `ReviewFail` へ）。
        if !all_pass
            && task.kind == TaskKind::Execute
            && let Some(repair_outcome) =
                self.try_review_repair(&task, &outcome.verdicts, &mut events)?
        {
            tracing::info!(%task_id, %run_id, next = ?repair_outcome.next, "review failed but repaired locally (ADR-0072 D16)");
            return Ok(());
        }
        // ADR-0007 D3/D4: Plan が全 pass なら子タスクの挿入と ReviewPass を同一トランザクションで行う。
        let result = match (all_pass, task.kind, outcome.plan) {
            (true, TaskKind::Plan, Some(mut plan)) => {
                let org = self.store.org_list()?;
                self.fix_plan_for_harness(&task, &mut plan, &org);
                // ADR-0039 D2: 子の作業場所は 明示 > 案件 > 親。
                let project_workspace =
                    task_ops::delegate::project_workspace(self.store.as_ref(), &task)
                        .map_err(ops_to_store)?;
                // ADR-0043 D2: 子のリポジトリは 明示（計画の `repos`）> 親 > 案件の primary。
                let project_repos = task_ops::delegate::project_repos(self.store.as_ref(), &task)
                    .map_err(ops_to_store)?;
                let home = task_core::home_dir();
                let workspace = task_core::WorkspaceContext {
                    project: project_workspace.as_ref(),
                    home: home.as_deref(),
                    repos: &project_repos,
                };
                let children = materialize_logging(
                    &task,
                    &plan,
                    &org,
                    &self.config.roles,
                    &self.config.genres,
                    workspace,
                    OffsetDateTime::now_utc(),
                    &mut |child_id, reason| {
                        tracing::info!(task_id = %task_id, child_id = %child_id, %reason, "workspace downgraded to local (ADR-0062 B2)");
                    },
                );
                let n = children.len();
                let r = self.store.complete_plan(
                    task_id,
                    events,
                    children,
                    self.config.plan_auto_accept,
                );
                if r.is_ok() {
                    tracing::info!(%task_id, %run_id, children = n, auto_accept = self.config.plan_auto_accept, "plan completed; children inserted");
                }
                r
            }
            // ADR-0079 D13（Phase R5a）: 案件計画（マイルストーン DAG）の run の提案の取り込み
            // （`finish_project_plan_run`）は廃止。新しく作る入口が無く（`POST /projects/{id}/plan` は 410）、
            // 残っていても下の Plan kind の失敗と同じに扱う（提案を作らない）。
            (true, TaskKind::Plan, None) => {
                // review_task は Plan kind に必ず暗黙の判定を付けるので、ここには来ないはず。
                tracing::error!(%task_id, "plan review passed without a parsed plan; treating as failure");
                self.store
                    .apply_transition_with_events(task_id, Trigger::ReviewFail, events)
            }
            (true, _, _) => {
                self.store
                    .apply_transition_with_events(task_id, Trigger::ReviewPass, events)
            }
            (false, _, _) => {
                self.store
                    .apply_transition_with_events(task_id, Trigger::ReviewFail, events)
            }
        };
        match result {
            Ok(outcome) => {
                tracing::info!(%task_id, %run_id, all_pass, next = ?outcome.next, attempts = outcome.attempts, "review finished");
                // ADR-0034 D2（監査 M-1〜M-3）: `result` はレビューを通って `Status::Done` になったときだけ作る
                // (ワーカーの「できました」がここで差し戻された分は報告にしない。DESIGN 原則 4)。
                if outcome.next == Status::Done
                    && let Some(review_entry) = entry.as_ref()
                {
                    let terminal = crate::reports::TerminalReport::Done {
                        summary: review_entry.subject.summary.clone(),
                        evidence: crate::reports::format_evidence(&review_entry.subject.evidence),
                    };
                    // ADR-0034 D7: ワーカーが結果ファイルで宣言した `report.kind`（無ければ既定の `result`）。
                    let declared = self.task_dir(&task).and_then(|ws| {
                        task_worker::read_result_report_kind(&self.artifacts_dir(&task, &ws))
                    });
                    if let Err(e) = crate::reports::record_run_report(
                        self.store.as_ref(),
                        &task,
                        &run_id,
                        &terminal,
                        declared.as_deref(),
                        OffsetDateTime::now_utc(),
                    ) {
                        tracing::warn!(%task_id, %run_id, error = %e, "failed to record the report for this run");
                    }
                } else if outcome.next == Status::Failed
                    && let Some(message) = review_fail_message.as_ref()
                {
                    // レビュー不合格が retry を使い切って `Status::Failed` になった場合の bad_news（原因を問わない。監査 M-1）。
                    let terminal = crate::reports::TerminalReport::Error {
                        message: message.clone(),
                        retryable: false,
                    };
                    if let Err(e) = crate::reports::record_run_report(
                        self.store.as_ref(),
                        &task,
                        &run_id,
                        &terminal,
                        None,
                        OffsetDateTime::now_utc(),
                    ) {
                        tracing::warn!(%task_id, %run_id, error = %e, "failed to record the report for this run");
                    }
                }
            }
            Err(StoreError::InvalidTransition(e)) => {
                tracing::warn!(%task_id, error = %e, "review result could not be applied");
            }
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }

    /// `work_units.spec.title` の `"repair (<bucket>): …"` から bucket 名を読む（repair の per-class
    /// カウンタ用。D16 は `WorkUnitSpec` に専用の欄を足さない設計なので、`try_review_repair` が書いた
    /// title を決定的に読み戻す）。
    pub(super) fn repair_bucket_of_title(title: &str) -> Option<&str> {
        title.strip_prefix("repair (")?.split(')').next()
    }

    /// ADR-0072 D16（Phase E4）: 最終レビューの不合格を分類し、修復できて上限内なら repair WU を
    /// 実体化して `Trigger::ReviewRepair` を適用する（`Some` を返す）。修復できない・上限を超えて
    /// いれば `events` に触れずに `None` を返す（呼び出し側が従来どおり `ReviewFail` へ進む）。
    pub(super) fn try_review_repair(
        &self,
        task: &Task,
        verdicts: &[Verdict],
        events: &mut Vec<Event>,
    ) -> Result<Option<task_core::Outcome>, DispatchError> {
        let task_id = task.id;
        let failing: Vec<task_core::FailedCheck> = verdicts
            .iter()
            .filter(|v| !v.pass)
            .map(|v| {
                let check = task
                    .acceptance
                    .get(v.criterion_idx)
                    .map(|c| c.check.clone())
                    // 暗黙の条件（Plan/aggregate/repo_checks/research）は task.acceptance に無く、
                    // D16 の分類表にも無いので修復できない（`Check::Human` と同じ扱いに倒す）。
                    .unwrap_or(task_core::Check::Human);
                task_core::FailedCheck {
                    check,
                    reason: v.reason.clone(),
                    repair_hint: v.repair_hint.clone(),
                }
            })
            .collect();
        if failing.is_empty() {
            return Ok(None);
        }
        let class = match task_core::classify_review_failure(&failing) {
            task_core::RepairDecision::Repairable(c) => c,
            task_core::RepairDecision::Substantive => return Ok(None),
        };

        let units = self.store.work_units_for(task_id)?;
        let repairs: Vec<&task_core::WorkUnitRow> = units
            .iter()
            .filter(|u| u.kind == task_core::WorkUnitKind::Repair)
            .collect();
        if repairs.len() as u32 >= self.config.execution.max_repairs {
            return Ok(None);
        }
        let same_class = repairs
            .iter()
            .filter(|u| Self::repair_bucket_of_title(&u.spec.title) == Some(class.bucket()))
            .count();
        if same_class as u32 >= self.config.execution.max_repairs_per_class {
            return Ok(None);
        }

        let (max_turns, max_wall_secs) = class.budget();
        let failing_details: Vec<String> = failing.iter().map(|f| f.reason.clone()).collect();
        let workspaces = self.task_workspaces_for(task);
        let cwd = workspaces.as_ref().and_then(|w| w.cwd());
        let branch = workspaces
            .as_ref()
            .and_then(|w| w.repos.first())
            .and_then(|r| r.branch())
            .unwrap_or_default();
        let diff_stat = crate::checkpoint::gather_repo_facts(cwd, branch)
            .0
            .map(|r| r.diff_stat);
        let mut allowed_paths = std::collections::BTreeSet::new();
        let mut scope_checks = std::collections::BTreeSet::new();
        for unit in &units {
            allowed_paths.extend(unit.spec.context.paths.iter().cloned());
            scope_checks.extend(
                unit.spec
                    .checks
                    .iter()
                    .filter(|check| check.cmd.contains("git diff"))
                    .map(|check| check.cmd.clone()),
            );
        }
        scope_checks.extend(task.acceptance.iter().filter_map(|criterion| {
            if let task_core::Check::Command { cmd, .. } = &criterion.check
                && cmd.contains("git diff")
            {
                Some(cmd.clone())
            } else {
                None
            }
        }));
        let scope = task_core::RepairScope {
            allowed_paths: allowed_paths.into_iter().collect(),
            scope_checks: scope_checks.into_iter().collect(),
        };
        let objective = task_core::build_repair_objective(
            class,
            &failing_details,
            &task.title,
            &task.objective,
            diff_stat.as_deref(),
            Some(&scope),
        );
        let n = repairs.len() + 1;
        let spec = task_core::WorkUnitSpec {
            key: format!("repair-{n}"),
            kind: task_core::WorkUnitKind::Repair,
            title: format!("repair ({}): 修復", class.bucket()),
            objective,
            depends_on: vec![],
            done_when: vec![],
            checks: vec![],
            context: Default::default(),
            harness: None,
            features: None,
            budget: Some(task_core::WorkUnitBudget {
                max_turns: Some(max_turns),
                max_wall_secs: Some(max_wall_secs),
            }),
            outputs: vec![],
            phase: None,
        };

        let now = rfc3339(OffsetDateTime::now_utc());
        let active_plan = self.store.execution_plan_active(task_id)?;
        let (new_plan, work_units, extra) = match active_plan {
            Some(plan) => {
                // D6: 計画のある Task は最終レビューまでに全 WU が done なので、既存の seq の続き。
                let seq = units.iter().map(|u| u.seq).max().unwrap_or(0) + 1;
                let row = task_core::WorkUnitRow::new(
                    task_core::new_id(),
                    task_id.to_string(),
                    plan.id.clone(),
                    seq,
                    spec,
                    task_core::WorkUnitStatus::Ready,
                    now.clone(),
                );
                let ev = Event::WorkUnitTransitioned {
                    work_unit_id: row.id.clone(),
                    key: row.key.clone(),
                    from: task_core::WorkUnitStatus::Pending,
                    to: task_core::WorkUnitStatus::Ready,
                    reason: "review_repair".to_string(),
                    run_id: None,
                };
                // ADR-0074 D6.2（Phase F1）: この repair WU の class を events に残す
                // （`execution_metrics::summarize` が `repairs_by_class` を組み立てる材料。
                // `unknown` を無くす）。
                let scheduled = Event::RepairScheduled {
                    work_unit_id: row.id.clone(),
                    key: row.key.clone(),
                    class: class.bucket().to_string(),
                    origin: task_core::execution::RepairOrigin::Review,
                };
                (None, vec![row], vec![ev, scheduled])
            }
            None => {
                // D5: atomic な Task は、初めての WorkUnit で暗黙の WU を `main`（done）として実体化する。
                let plan_id = task_core::new_id();
                let main_spec = task_core::WorkUnitSpec {
                    key: "main".to_string(),
                    kind: task_core::WorkUnitKind::Implement,
                    title: task.title.clone(),
                    objective: task.objective.clone(),
                    depends_on: vec![],
                    done_when: vec![],
                    checks: vec![],
                    context: Default::default(),
                    harness: None,
                    features: None,
                    budget: None,
                    outputs: vec![],
                    phase: None,
                };
                let main_row = task_core::WorkUnitRow::new(
                    task_core::new_id(),
                    task_id.to_string(),
                    plan_id.clone(),
                    0,
                    main_spec.clone(),
                    task_core::WorkUnitStatus::Done,
                    now.clone(),
                );
                let repair_row = task_core::WorkUnitRow::new(
                    task_core::new_id(),
                    task_id.to_string(),
                    plan_id.clone(),
                    1,
                    spec.clone(),
                    task_core::WorkUnitStatus::Ready,
                    now.clone(),
                );
                let plan_spec = task_core::ExecutionPlanSpec {
                    stages: Vec::new(),
                    units: Vec::new(),
                    decisions: Vec::new(),
                    schema: task_core::EXECUTION_PLAN_SCHEMA.to_string(),
                    rationale: "reviewer repair: 暗黙の WorkUnit を実体化".to_string(),
                    work_units: vec![main_spec, spec],
                    phases: Vec::new(),
                    children: Vec::new(),
                };
                let plan_row = task_core::ExecutionPlanRow {
                    id: plan_id.clone(),
                    task_id: task_id.to_string(),
                    version: 1,
                    origin: task_core::PlanOrigin::Repair,
                    planner_run_id: None,
                    status: task_core::PlanStatus::Active,
                    spec: plan_spec.clone(),
                    created_at: now.clone(),
                    superseded_at: None,
                };
                let ev = Event::ExecutionPlanned {
                    plan_id,
                    version: 1,
                    origin: task_core::PlanOrigin::Repair,
                    supersedes: None,
                    reason: Some("review_repair".to_string()),
                    plan: Box::new(plan_spec),
                };
                let scheduled = Event::RepairScheduled {
                    work_unit_id: repair_row.id.clone(),
                    key: repair_row.key.clone(),
                    class: class.bucket().to_string(),
                    origin: task_core::execution::RepairOrigin::Review,
                };
                (
                    Some(plan_row),
                    vec![main_row, repair_row],
                    vec![ev, scheduled],
                )
            }
        };

        let mut all_events = std::mem::take(events);
        all_events.extend(extra);
        match self
            .store
            .review_repair_apply(task_id, all_events, new_plan, work_units)
        {
            Ok(outcome) => Ok(Some(outcome)),
            Err(e) => Err(e.into()),
        }
    }
}
