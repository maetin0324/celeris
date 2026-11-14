//! worker run の終了処理（`finish_worker_result` と finalise の失敗の後始末）。同じ transaction の組と `release_quota_if_tracked` はそのまま。ADR-0082 の L3。

use super::*;
use crate::run_settlement;

fn repair_git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Only move a repair branch when its original commit is in that branch's reflog and
/// there is no uncommitted work to discard. A failed abort/reset is a manual stop.
fn rollback_integration_repair(dir: &Path, branch: &str, before: &str) -> bool {
    let Some(current_branch) = repair_git(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"])
    else {
        return false;
    };
    if current_branch != branch
        || repair_git(dir, &["status", "--porcelain"]).as_deref() != Some("")
    {
        return false;
    }
    let Some(log) = repair_git(dir, &["reflog", "show", "--format=%H", branch]) else {
        return false;
    };
    if !log.lines().any(|sha| sha == before) {
        return false;
    }
    let in_rebase = ["rebase-merge", "rebase-apply"].iter().any(|name| {
        repair_git(dir, &["rev-parse", "--git-path", name])
            .is_some_and(|path| Path::new(&path).exists())
    });
    if in_rebase && repair_git(dir, &["rebase", "--abort"]).is_none() {
        return false;
    }
    repair_git(dir, &["reset", "--hard", before]).is_some()
        && repair_git(dir, &["rev-parse", "HEAD"]).as_deref() == Some(before)
        && repair_git(dir, &["status", "--porcelain"]).as_deref() == Some("")
}

impl Dispatcher {
    pub(super) fn integration_repair_exhaustion(
        &self,
        task: &Task,
        wu: &task_core::WorkUnitRow,
        reason: task_core::IntegrationRepairExhaustReason,
    ) -> Result<(bool, Option<Event>), DispatchError> {
        let events = self.store.events_for(task.id)?;
        if let Some(Event::IntegrationRepairExhausted { fallback, .. }) = events.iter().rev().map(|(_, e)| e).find(|e| {
            matches!(e, Event::IntegrationRepairExhausted { work_unit_id: Some(id), .. } if id == &wu.id)
        }) {
            return Ok((*fallback, None));
        }
        let Some((repo_id, target_sha, before_sha, attempt)) =
            events.iter().rev().find_map(|(_, e)| {
                if let Event::IntegrationRepairScheduled {
                    work_unit_id,
                    repo_id,
                    target_sha,
                    before_sha,
                    attempt,
                    ..
                } = e
                    && work_unit_id == &wu.id
                {
                    Some((*repo_id, target_sha.clone(), before_sha.clone(), *attempt))
                } else {
                    None
                }
            })
        else {
            return Err(
                StoreError::Invalid("integration repair has no scheduled event".into()).into(),
            );
        };
        let repo_name = task
            .repos
            .iter()
            .find(|r| r.repo_id == repo_id)
            .map(|r| r.name.as_str());
        let worktree = self.task_workspaces_for(task).and_then(|ws| {
            ws.repos
                .into_iter()
                .find(|r| Some(r.name.as_str()) == repo_name)
                .and_then(|r| r.worktree)
        });
        let mut rollback_to_sha = None;
        let fallback = if let Some(wt) = worktree.filter(|w| w.dir.is_dir()) {
            let head = repair_git(&wt.dir, &["rev-parse", "HEAD"]);
            let on_task_branch =
                repair_git(&wt.dir, &["symbolic-ref", "--quiet", "--short", "HEAD"]).as_deref()
                    == Some(wt.branch.as_str());
            let rebase_in_progress = ["rebase-merge", "rebase-apply"].iter().any(|part| {
                repair_git(&wt.dir, &["rev-parse", "--git-path", part])
                    .is_some_and(|path| Path::new(&path).exists())
            });
            if head.as_deref() == Some(before_sha.as_str())
                && on_task_branch
                && !rebase_in_progress
                && repair_git(&wt.dir, &["status", "--porcelain"]).as_deref() == Some("")
            {
                true
            } else if rollback_integration_repair(&wt.dir, &wt.branch, &before_sha) {
                rollback_to_sha = Some(before_sha.clone());
                true
            } else {
                false
            }
        } else {
            false
        };
        Ok((
            fallback,
            Some(Event::IntegrationRepairExhausted {
                work_unit_id: Some(wu.id.clone()),
                repo_id,
                target_sha,
                before_sha,
                attempt,
                reason,
                rollback_to_sha,
                fallback,
            }),
        ))
    }

    fn integration_repair_result_trusted(
        &self,
        task: &Task,
        wu: &task_core::WorkUnitRow,
    ) -> Result<bool, DispatchError> {
        let events = self.store.events_for(task.id)?;
        let Some((repo_id, before, target)) = events.iter().rev().find_map(|(_, e)| {
            if let Event::IntegrationRepairScheduled {
                work_unit_id,
                repo_id,
                before_sha,
                target_sha,
                ..
            } = e
                && work_unit_id == &wu.id
            {
                Some((*repo_id, before_sha.as_str(), target_sha.as_str()))
            } else {
                None
            }
        }) else {
            return Ok(false);
        };
        let Some(name) = task
            .repos
            .iter()
            .find(|r| r.repo_id == repo_id)
            .map(|r| r.name.as_str())
        else {
            return Ok(false);
        };
        let Some(wt) = self.task_workspaces_for(task).and_then(|ws| {
            ws.repos
                .into_iter()
                .find(|r| r.name == name)
                .and_then(|r| r.worktree)
        }) else {
            return Ok(false);
        };
        let clean = repair_git(&wt.dir, &["status", "--porcelain"]).as_deref() == Some("");
        let on_task_branch = repair_git(&wt.dir, &["symbolic-ref", "--quiet", "--short", "HEAD"])
            .as_deref()
            == Some(wt.branch.as_str());
        let in_rebase = ["rebase-merge", "rebase-apply"].iter().any(|part| {
            repair_git(&wt.dir, &["rev-parse", "--git-path", part])
                .is_some_and(|p| Path::new(&p).exists())
        });
        let preserves_before =
            repair_git(&wt.dir, &["merge-base", "--is-ancestor", before, "HEAD"]).is_some();
        let rebased_onto_target =
            repair_git(&wt.dir, &["merge-base", "--is-ancestor", target, "HEAD"]).is_some();
        Ok(clean && on_task_branch && !in_rebase && (preserves_before || rebased_onto_target))
    }

    pub(super) fn on_worker_finished(
        &mut self,
        task_id: TaskId,
        run_id: String,
        provider: ProviderId,
        result: Result<RunOutcome, AdapterError>,
    ) -> Result<(), DispatchError> {
        // ADR-0024 D2/D4: プールで選んだアカウント（無ければ `None`）。失敗の cooldown をプロバイダかアカウントか
        // どちらに向けるかを後で決める。
        // ADR-0061（Phase 104）: `since`（dispatch した時刻）も一緒に取り出し、run の wall time を計算する。
        let (account, account_adapter, run_since) = self
            .take_running_by_run_id(&run_id)
            .map(|e| (e.account, e.account_adapter, Some(e.since)))
            .unwrap_or((None, None, None));
        let Some(task) = self.store.get(task_id)? else {
            tracing::warn!(%task_id, %run_id, "worker finished for unknown task");
            self.release_quota_if_tracked(
                &run_id,
                account.as_deref(),
                account_adapter,
                &provider,
                task_id,
            );
            return Ok(());
        };
        let lease_matches = self.run_holds_lease(&task, &run_id)?;
        if !lease_matches {
            // ADR-0002 D9 / ADR-0005 D4: リース回収済み・cancel 済みの古い結果は捨てる。
            tracing::warn!(%task_id, %run_id, status = ?task.status, "stale worker result discarded");
            self.release_quota_if_tracked(
                &run_id,
                account.as_deref(),
                account_adapter,
                &provider,
                task_id,
            );
            return Ok(());
        }
        // ADR-0072 D6（Phase E2）: 計画のある Task で、この run が `running` の WorkUnit のものなら
        // `Some`（`last_run_id` が一致するもの。無ければ暗黙の WorkUnit = 従来どおり `None`）。
        let current_wu = self.store.work_units_for(task_id)?.into_iter().find(|u| {
            u.status == task_core::WorkUnitStatus::Running
                && u.last_run_id.as_deref() == Some(run_id.as_str())
        });
        // ADR-0072 D14（Phase E3）: task-local な planner run はここで分岐する（Reviewing への遷移・
        // checkpoint の合成など、通常のワーカー/WU の判定は経由しない）。
        if current_wu.is_none() {
            let started_as_planner = self.store.events_for(task_id)?.iter().any(|(_, e)| {
                matches!(
                    e,
                    Event::WorkerStarted { run_id: r, role: Some(RunRole::Planner), .. }
                        if r == &run_id
                )
            });
            if started_as_planner {
                return self.on_planner_finished(
                    task_id,
                    &task,
                    run_id,
                    run_since,
                    provider,
                    (account.as_deref(), account_adapter),
                    result,
                );
            }
        }
        // ADR-0072 D14/D6・E4 (g): この WU に決定的な `checks`（`Command`）があり、run が
        // `Terminal::Done` で終わったのなら、WU を `done` にする前にそれらを実行する（`review.rs` の
        // Command 実行を再利用）。checks が無い WU・atomic な run はこれまでどおり即座に
        // `finish_worker_result` へ進む。
        if let Some(wu) = &current_wu
            && !wu.spec.checks.is_empty()
            && matches!(
                &result,
                Ok(RunOutcome {
                    terminal: Terminal::Done { .. },
                    ..
                })
            )
            && !(task_core::is_integration_repair_unit(wu.kind, &wu.spec.title)
                && !self.integration_repair_result_trusted(&task, wu)?)
        {
            // ADR-0040 付記（2026-10-04、WU 検査の引き継ぎ）D1: draining の旧 instance は run の終わりで手を離す。
            if !self.coord.accepting_new_work()
                && self.hand_off_work_unit_checks(
                    &task,
                    wu,
                    &run_id,
                    account.as_deref(),
                    account_adapter,
                    &provider,
                    &result,
                )?
            {
                return Ok(());
            }
            return self.spawn_work_unit_checks(
                task_id,
                run_id,
                wu.clone(),
                account,
                account_adapter,
                run_since,
                provider,
                result,
            );
        }
        self.finish_worker_result(
            task,
            current_wu,
            run_id,
            account,
            account_adapter,
            run_since,
            provider,
            result,
        )
    }

    /// `on_worker_finished`（checks が無い、または atomic な run）と `on_work_unit_checks_finished`
    /// （WU の checks が終わった後）の共通の後段。`task`/`current_wu` は呼び出し側が確定させたもの。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn finish_worker_result(
        &mut self,
        task: Task,
        current_wu: Option<task_core::WorkUnitRow>,
        run_id: String,
        account: Option<String>,
        account_adapter: Option<AccountAdapter>,
        run_since: Option<OffsetDateTime>,
        provider: ProviderId,
        result: Result<RunOutcome, AdapterError>,
    ) -> Result<(), DispatchError> {
        self.finish_worker_result_with(
            task,
            current_wu,
            run_id,
            account,
            account_adapter,
            run_since,
            provider,
            result,
            None,
        )
    }

    /// `finish_worker_result` の本体。`check_failure` は WU の checks が不合格で `Terminal::Done` を
    /// `Terminal::Error` にすり替えた run だけ `Some`（ADR-0079 付記 R7-5: 記録・outcome の要約・usage）。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn finish_worker_result_with(
        &mut self,
        task: Task,
        current_wu: Option<task_core::WorkUnitRow>,
        run_id: String,
        account: Option<String>,
        account_adapter: Option<AccountAdapter>,
        run_since: Option<OffsetDateTime>,
        provider: ProviderId,
        result: Result<RunOutcome, AdapterError>,
        check_failure: Option<WorkUnitCheckFailure>,
    ) -> Result<(), DispatchError> {
        let task_id = task.id;
        // ADR-0013 D9: 供給側失敗なら種別（ProviderThrottled.reason）を、result を消費する前に取っておく。
        let failure_reason = result.as_ref().err().and_then(provider_failure_reason);
        // ADR-0033 D3 / ADR-0034 D2（監査 M-1〜M-3）: 報告の材料も、result を消費する前に取る。
        // `terminal_report` は `Ok(RunOutcome)` の内容（question / worker 自身が返した error）。
        // `adapter_error_text` は `Err(AdapterError)`（アダプタ／供給側の失敗）の表示文字列。
        // どちらも「報告するかどうか」は後で `outcome.next` を見て決める（run の終端ではなくタスクの終端状態）。
        let terminal_report = crate::reports::terminal_report(&result);
        let adapter_error_text = result.as_ref().err().map(|e| e.to_string());
        // ADR-0033 D6（Phase 24）: 結果ファイルの `memory` を、この run の担当の記憶に追記する
        // （run の終わり方に依らず。ファイル I/O だけで、覚える中身を決めるのはワーカー側）。
        self.absorb_memory(&task);
        let current_wu_id = current_wu.as_ref().map(|w| w.id.clone());
        // ADR 2026-11-11 dispatcher-state-ownership D3: run の終わり方は `run_settlement::settle` が決める。ここでは
        // events（部をまたぐ質問・連続 requeue 数）・供給側の失敗の分類・cluster job の wait の検証を読んで渡す。
        let cluster_wait_checked = match &result {
            Ok(RunOutcome {
                terminal:
                    Terminal::Waiting {
                        request,
                        checkpoint,
                        ..
                    },
                ..
            }) => Some(self.cluster_job_wait_for(
                &task,
                &run_id,
                current_wu_id.as_deref(),
                request,
                checkpoint.clone(),
            )),
            _ => None,
        };
        let supply_failure = result.as_ref().err().and_then(provider_failure_outcome);
        let browser_code = result
            .as_ref()
            .err()
            .and_then(|e| browser_prereq::worker_error_code(&e.to_string()));
        let events_now = self.store.events_for(task_id)?;
        let infra_n = consecutive_infra_requeues(&events_now) + 1;
        let proposal = crate::run_settlement::settle(run_settlement::RunFinishInput {
            result,
            events: &events_now,
            run_id: &run_id,
            conversation: task_core::is_conversation(&task),
            work_unit: current_wu.is_some(),
            supply_failure,
            browser_prereq: browser_code,
            cluster_wait: cluster_wait_checked,
            max_requeues: self.config.max_requeues,
            infra_retry: (
                infra_n,
                infra_retry_after(infra_n, self.config.max_infra_retries),
            ),
            check_failure: check_failure.as_ref().map(|f| (f.usage, f.summary())),
        });
        let misplaced_result = proposal.misplaced;
        let cross_department = proposal.cross_department;
        let check_failure_suffix = proposal.check_failure_suffix;
        let mut subject = ReviewSubject {
            summary: proposal.done_summary,
            evidence: proposal.done_evidence,
        };
        // ADR-0070 D3（Phase 116）: `Trigger::InfraRequeue` を選んだときだけ `Some((n, 待ち時間))`。遷移の後に
        // `RunSupervisor` の infra backoff のバックオフ期限を立てるのに使う。
        let mut infra_requeue = proposal.infra_requeue;
        // ADR 2026-10-08-browser-prod-enablement D2: `BrowserPrereqBlock` と同じトランザクションで残す event。
        let mut browser_prereq_event = proposal.browser_prereq;
        // ADR-0072 D7（Phase E1）: この run の構造化した終わり方（`WorkerFinished.end` に写す）。
        let run_end = proposal.run_end;
        // ADR-0072 D9: `Terminal::Yielded` の生の checkpoint JSON（`result.json` の `yield`）。
        let yield_checkpoint_json = proposal.yield_checkpoint;
        // ADR-0090 D1/D2: `Terminal::Waiting` を検証して組んだクラスタ job の wait（`ClusterJobWaitStarted` にする）。
        let mut cluster_wait = proposal.cluster_wait;
        let (mut trigger, mut outcome_str, usage, provider_outcome) = (
            proposal.trigger,
            proposal.outcome,
            proposal.usage,
            proposal.provider_outcome,
        );
        // ADR-0072 D7/D8/D9/D11/D18（Phase E1/E2）: 予算切れ・yield の続き（continuation）。
        // checkpoint は常に合成して残す（(b)）。continuation そのものの可否・上限到達の扱いは
        // `[execution]` で決める。無効化・上限到達のときは trigger/outcome_str を従来の形に戻す。
        let mut checkpoint_event: Option<Event> = None;
        // ADR-0072 D5（Phase E2）: `runs` 索引の `finish` に使う（`(g)`。atomic/WU どちらの run にも
        // 書く）。
        let mut checkpoint_for_index: Option<task_core::Checkpoint> = None;
        // ADR-0072 D6（Phase E2）: この run が WU の run なら、その WU の新しい行と、伝播で
        // 一緒に書く他の WU の新しい行（`newly_blocked`/`newly_ready`）、`plan_complete` かどうか。
        let mut wu_update: Option<(
            task_core::WorkUnitRow,
            &'static str,
            Vec<task_core::WorkUnitRow>,
        )> = None;
        let mut integration_exhausted_event: Option<Event> = None;
        // ADR 2026-10-22-stalled-auto-replan D1: WU の失敗から起こす自動 replan の記録。
        let mut auto_replan_event: Option<Event> = None;
        // ADR-0079 D7（Phase R3a）: worker が `result.json` の `decisions` で出した決定の要求（記録と止める unit）。
        let mut worker_decisions: Option<WorkerDecisions> = None;
        // ADR-0090 D2: この WU の run が unit を `blocked(cluster_jobs)` にした。
        let mut wu_parked_on_cluster_jobs = false;
        if let Some(wu) = &current_wu {
            // ADR-0072 D6（Phase E2）: WU の run。`end` が無い run の扱いは `run_settlement::work_unit_run_end`。
            let effective_end = run_settlement::work_unit_run_end(run_end);
            let tree = self.config.execution.limits.tree;
            let auto_counts = if tree.enabled && tree.auto_leaf {
                auto_leaf::counts(
                    &self.store.events_for(task_id)?,
                    wu,
                    &run_id,
                    Some(effective_end),
                )
            } else {
                None
            };
            let auto_exceeded = auto_counts
                .as_ref()
                .is_some_and(|counts| counts.exceeded(tree));
            let mut checkpoint_opt: Option<task_core::Checkpoint> = None;
            let mut prev_checkpoint_opt: Option<task_core::Checkpoint> = None;
            let mut no_progress_before = 0u32;
            let mut continuations_since_answer = 0u32;
            let mut progress_checkpoints = 0u32;
            if run_settlement::work_unit_saves_checkpoint(
                effective_end,
                self.config.execution.continuation,
                auto_exceeded,
            ) {
                let events_so_far = self.store.events_for(task_id)?;
                // ADR-0074 D1.6（Phase F2b）: v2 の WU は WU の worktree・ブランチ・base で取る。
                let (artifacts_dir, cwd_buf, branch, base) =
                    self.work_unit_checkpoint_site(&task, Some(wu));
                let cwd = cwd_buf.as_deref();
                let activity = tool_activity(&events_so_far, &run_id);
                let mechanical =
                    crate::checkpoint::gather_from(cwd, &branch, base.as_deref(), &activity);
                let worker_checkpoint = worker_checkpoint_input(
                    yield_checkpoint_json.as_ref(),
                    artifacts_dir.as_deref(),
                );
                let checkpoint_end = effective_end
                    .as_checkpoint_end()
                    .unwrap_or(task_core::CheckpointEnd::BudgetExhausted);
                let ctx = task_core::CheckpointContext {
                    task_id: task_id.to_string(),
                    work_unit: Some(wu.key.clone()),
                    run_id: run_id.clone(),
                    run_seq: wu.runs,
                    end: checkpoint_end,
                    created_at: rfc3339(OffsetDateTime::now_utc()),
                };
                let checkpoint = task_core::merge_checkpoint(worker_checkpoint, mechanical, ctx);
                prev_checkpoint_opt =
                    task_ops::derive::latest_progress_checkpoint(&events_so_far, Some(&wu.id));
                no_progress_before = no_progress_streak(&events_so_far, Some(&wu.id));
                continuations_since_answer =
                    task_ops::derive::wu_continuations_since_answer(&events_so_far, &wu.id);
                progress_checkpoints = task_ops::derive::progress_checkpoints_since_answer(
                    &events_so_far,
                    Some(&wu.id),
                );
                checkpoint_event = Some(Event::CheckpointSaved {
                    run_id: run_id.clone(),
                    work_unit_id: Some(wu.id.clone()),
                    checkpoint: Box::new(checkpoint.clone()),
                });
                checkpoint_opt = Some(checkpoint);
            }
            checkpoint_for_index = checkpoint_opt.clone();
            // ADR 2026-11-11 dispatcher-state-ownership D3: WU の遷移と task の trigger・outcome の文は
            // `run_settlement::settle_work_unit` が決める（遷移の正本は `execution_scheduler`）。ここでは WU の行・auto-leaf
            // の決定の要求・integration repair の結果の信頼を読んで渡し、提案の副作用を今と同じ順で行う。
            let units = self.store.work_units_for(task_id)?;
            let auto_leaf_request = match auto_counts.as_ref().filter(|c| c.exceeded(tree)) {
                Some(counts) => Some(self.auto_leaf_decision(
                    &task,
                    wu,
                    &run_id,
                    counts,
                    checkpoint_opt.as_ref(),
                )?),
                None => None,
            };
            let integration_repair_trusted =
                if task_core::is_integration_repair_unit(wu.kind, &wu.spec.title) {
                    Some(self.integration_repair_result_trusted(&task, wu)?)
                } else {
                    None
                };
            let mut settled = run_settlement::settle_work_unit(run_settlement::WorkUnitRunInput {
                end: effective_end,
                run_id: &run_id,
                wu,
                units: &units,
                continuation: run_settlement::ContinuationInputs {
                    checkpoint: checkpoint_opt.as_ref(),
                    prev_checkpoint: prev_checkpoint_opt.as_ref(),
                    no_progress_before,
                    continuations_since_answer,
                    progress_checkpoints,
                },
                continuation_enabled: self.config.execution.continuation,
                continuation_hard_limit: self.config.execution.continuation_hard_limit,
                no_progress_limit: self.config.execution.no_progress_limit,
                max_retries: task.budget.max_retries,
                auto_leaf_measured: auto_counts.is_some(),
                auto_leaf_request,
                integration_repair_trusted,
                trigger,
                outcome: outcome_str,
                check_failure_suffix: &check_failure_suffix,
                check_failure_signature: check_failure.as_ref().map(|f| f.failure_signature()),
                misplaced: misplaced_result,
            });
            if let Some(reason) = settled.integration_repair_exhaust {
                let (fallback, event) = self.integration_repair_exhaustion(&task, wu, reason)?;
                integration_exhausted_event = event;
                (settled.trigger, settled.outcome) =
                    run_settlement::integration_repair_exhausted(&wu.key, fallback);
            }
            if let Some(failure) = settled.failure.as_ref().map(|f| f.failure.clone()) {
                let limits = self.replan_limits(task_id, Some((&wu.key, &failure)), None)?;
                match run_settlement::settle_work_unit_failure(&mut settled, limits) {
                    run_settlement::WorkUnitFailureStep::Settled => {}
                    run_settlement::WorkUnitFailureStep::Stalled { stalled } => {
                        tracing::warn!(%task_id, key = %wu.key, stalled, "the same work unit failure repeated without progress; asking a human instead of replanning (ADR 2026-10-22-stalled-auto-replan)");
                    }
                    run_settlement::WorkUnitFailureStep::AskExhausted { why, used } => {
                        (settled.trigger, settled.outcome) =
                            self.replan_exhausted_ask(&task, &why, used)?;
                    }
                }
            }
            auto_replan_event = settled.auto_replan.take();
            if let Some(decision) = &settled.decision
                && !settled.reset_only
                && decision.plan_complete
            {
                // D15: Task の完了。`ReviewSubject.summary` は WU ごとの最終 checkpoint の `completed` を key ごとに
                // 1 段落ずつ並べた決定的な要約にする（evidence はこの最後の run のものを残す）。checkpoint が無い WU は
                // 「完了」とだけ書く。
                let mut paragraphs = Vec::new();
                for u in &units {
                    if u.id == wu.id {
                        continue;
                    }
                    if !u.status.is_active() || u.status != task_core::WorkUnitStatus::Done {
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
                let this_completed = checkpoint_for_index
                    .as_ref()
                    .map(|cp| cp.completed.join("; "))
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| subject.summary.clone());
                paragraphs.push(format!("{}: {}", wu.spec.title, this_completed));
                subject.summary = paragraphs.join("\n");
            }
            // ADR-0079 D7（Phase R3a）: 木の節点の leaf の run が `result.json` の `decisions` で人への決定の要求を
            // 出した。記録し（path 付き）、`needed_before: self` ならこの leaf を done にせず止める。他の unit を指した
            // 決定はその unit だけを止め、この run の完了は妨げない。
            if settled.reads_worker_decisions() {
                let (site_artifacts, _, _, _) = self.work_unit_checkpoint_site(&task, Some(wu));
                if let Some(found) =
                    self.worker_decisions(&task, Some(wu), site_artifacts.as_deref(), &run_id)?
                {
                    if found.self_hold {
                        run_settlement::hold_for_worker_decisions(
                            &mut settled,
                            &wu.key,
                            found.count,
                        );
                    }
                    worker_decisions = Some(found);
                }
            } else if let Some(request) = settled.auto_leaf_request.take() {
                worker_decisions = Some(WorkerDecisions {
                    events: vec![Event::DecisionRequested {
                        decision: Box::new(request),
                    }],
                    held_rows: vec![],
                    self_hold: true,
                    count: 1,
                });
            }
            trigger = settled.trigger;
            outcome_str = settled.outcome;
            if let Some(decision) = settled.decision {
                let mut all_new_rows = decision.newly_blocked;
                all_new_rows.extend(decision.newly_ready);
                wu_parked_on_cluster_jobs = decision.reason == "cluster_jobs";
                wu_update = Some((decision.updated, decision.reason, all_new_rows));
            }
        } else if let Some(end) = run_end
            && end.saves_checkpoint()
        {
            // atomic task の run。続け方は `run_settlement::settle_task_run` が決める。ここでは checkpoint を組み立て、
            // events と委譲した子（ADR 2026-10-18-parent-waits-for-children）を読んで渡す。
            let events_so_far = self.store.events_for(task_id)?;
            let run_seq = current_run_seq(&events_so_far);
            let workspace_dir = self.task_dir(&task);
            let artifacts_dir = workspace_dir.as_ref().map(|d| self.artifacts_dir(&task, d));
            let workspaces = self.task_workspaces_for(&task);
            let cwd = workspaces.as_ref().and_then(|w| w.cwd());
            let branch = workspaces
                .as_ref()
                .and_then(|w| w.repos.first())
                .and_then(|r| r.branch())
                .unwrap_or_default();
            let mechanical =
                crate::checkpoint::gather(cwd, branch, &tool_activity(&events_so_far, &run_id));
            // D9: 優先順位は `result.json.yield` > `checkpoint.json` > mechanical。
            let worker_checkpoint =
                worker_checkpoint_input(yield_checkpoint_json.as_ref(), artifacts_dir.as_deref());
            let children_pending = end != task_core::RunEnd::Waiting
                && !self.awaited_children(task_id, true)?.is_empty();
            let ctx = task_core::CheckpointContext {
                task_id: task_id.to_string(),
                work_unit: None,
                run_id: run_id.clone(),
                run_seq,
                end: run_settlement::task_checkpoint_end(end, children_pending),
                created_at: rfc3339(OffsetDateTime::now_utc()),
            };
            let checkpoint = task_core::merge_checkpoint(worker_checkpoint, mechanical, ctx);
            let settled = run_settlement::settle_task_run(run_settlement::TaskRunInput {
                end,
                events: &events_so_far,
                run_seq,
                checkpoint: &checkpoint,
                children_pending,
                continuation_enabled: self.config.execution.continuation,
                continuation_hard_limit: self.config.execution.continuation_hard_limit,
                no_progress_limit: self.config.execution.no_progress_limit,
                trigger,
                outcome: outcome_str,
            });
            (trigger, outcome_str) = (settled.trigger, settled.outcome);
            if settled.saves_checkpoint {
                checkpoint_for_index = Some(checkpoint.clone());
                checkpoint_event = Some(Event::CheckpointSaved {
                    run_id: run_id.clone(),
                    work_unit_id: None,
                    checkpoint: Box::new(checkpoint),
                });
            }
        }
        // ADR-0079 D7（Phase R3a）: 木の節点の atomic の run（WU を持たない）が `result.json` の `decisions` を
        // 書いた。記録し、`needed_before: self` なら最終レビューに進めず `ready` に戻して（`advance`）、答えが
        // 出るまで run を起こさない（`decision_self_hold`。答えは次の run の前置きの `answers` に入る）。
        if current_wu.is_none() && run_settlement::task_reads_worker_decisions(&trigger, run_end) {
            let artifacts_dir = self.task_dir(&task).map(|d| self.artifacts_dir(&task, &d));
            if let Some(found) =
                self.worker_decisions(&task, None, artifacts_dir.as_deref(), &run_id)?
            {
                if found.self_hold {
                    (trigger, outcome_str) =
                        run_settlement::hold_task_for_worker_decisions(found.count);
                }
                worker_decisions = Some(found);
            }
        }
        // ADR-0054 D1（Phase 67）: CoS の対話 run（継続セッション）は、この run の usage を
        // `node_sessions.approx_tokens` に積む（rollover 判定の材料。turns も 1 進む）。継続セッションの
        // 対象でない run（`node_sessions` の行が無い）では `node_session_touch` が no-op で返るだけ。
        if task_core::is_conversation(&task) && task.assignee.as_deref() == Some(task_core::COS_ID)
        {
            let tokens = usage
                .as_ref()
                .map(|u| u.input_tokens.unwrap_or(0) + u.output_tokens.unwrap_or(0))
                .unwrap_or(0);
            if let Err(e) = self.store.node_session_touch(
                task_core::COS_ID,
                task_core::SessionKind::Conversation,
                None,
                tokens as i64,
                OffsetDateTime::now_utc(),
            ) {
                tracing::warn!(%task_id, error = %e, "failed to record session usage");
            }
        }
        // ADR-0140 D2: WU（と atomic task。付記 session-container）の継続 session を使った run は、その usage を `approx_tokens` に積む
        // （判断表 #10 の rollover の材料）。継続 session を持たない run（`runs.session_id` が無い）は何もしない。
        if let Ok(Some(row)) = self.store.run_index_get(&run_id)
            && row.session_id.is_some()
            && let Some(adapter) = row.adapter.as_deref()
        {
            let tokens = usage
                .as_ref()
                .map(|u| u.input_tokens.unwrap_or(0) + u.output_tokens.unwrap_or(0))
                .unwrap_or(0);
            if let Err(e) = self.store.work_unit_session_touch(
                task_id,
                row.work_unit_id.as_deref(),
                adapter,
                row.account.as_deref(),
                tokens as i64,
                OffsetDateTime::now_utc(),
            ) {
                tracing::warn!(%task_id, error = %e, "failed to record continuation session usage");
            }
        }
        // ADR-0033 D4: 部をまたぐ委譲の質問は、run の自己申告の終わり方より優先する（子は作られていない）。
        // Phase 27: 人に見せる質問は 1 件の部またぎにつき 1 つ（`approvals` の行の単位）。
        let asked = run_settlement::questions_of_run(&trigger, &outcome_str, &cross_department);
        if let Some(outcome) = asked.replaced_outcome {
            trigger = Trigger::WorkerQuestion;
            outcome_str = outcome;
            subject = ReviewSubject::default();
        }
        let mut questions = asked.questions;

        // ADR-0061（Phase 104）: `retries` はこの run が始まった時点でタスクが既に消費していた試行回数
        // （= 遷移前の `task.attempts`）。
        let metrics = run_since.map(|since| task_core::RunMetrics {
            wall_ms: wall_ms_since(since),
            retries: task.attempts,
            knowledge: crate::kb_use(self.work_unit_checkpoint_site(&task, current_wu.as_ref())),
            ..Default::default()
        });
        // ADR-0072 D5/D6/D15（Phase E2）: WU の行の更新（このWU自身 + 伝播で一緒に決まった他のWU）を、
        // Task の trigger の適用とは別に先に書く（既存の `RoutingDecided` 等の慣習と同じ:
        // 別のトランザクションでも監査上の実害は無い。再起動時の照合は `work_units.status` を正とする）。
        // ADR-0074 D1.2（Phase F2b）: v2 の WU が done になったら、WU の作業ツリーで commit する。
        let mut committed_event: Option<Event> = None;
        // ADR-0130 D2: actual write-set を採る WU の行（自動 commit 後）と、done になったか。
        let mut write_set_wu: Option<(task_core::WorkUnitRow, bool)> = None;
        if let Some((updated_wu, reason, side_effect_rows)) = wu_update.take() {
            let mut updated_wu = updated_wu;
            if updated_wu.phase.is_some() && reason == "completed" {
                committed_event = self.commit_work_unit(&task, &mut updated_wu)?;
            }
            write_set_wu = Some((updated_wu.clone(), reason == "completed"));
            let from = current_wu
                .as_ref()
                .map(|w| w.status)
                .unwrap_or(updated_wu.status);
            let mut transition_events = vec![Event::WorkUnitTransitioned {
                work_unit_id: updated_wu.id.clone(),
                key: updated_wu.key.clone(),
                from,
                to: updated_wu.status,
                reason: reason.to_string(),
                run_id: Some(run_id.clone()),
            }];
            // ADR-0090 D2: `blocked(cluster_jobs)` への遷移と wait 行の作成は同じトランザクションにする
            // （間で止まると、wait 行の無い blocked(cluster_jobs) の WU が永久に再開しない）。
            if current_wu.is_some()
                && wu_parked_on_cluster_jobs
                && let Some(wait) = cluster_wait.take()
            {
                transition_events.push(Event::ClusterJobWaitStarted {
                    wait: Box::new(wait),
                });
            }
            if let Err(e) = self.store.work_unit_transition_with_events(
                task_id,
                updated_wu.clone(),
                transition_events,
            ) {
                tracing::warn!(%task_id, %run_id, error = %e, "failed to record the work unit transition");
            }
            #[cfg(test)]
            if self.test_hooks.halt_after_wu_transition {
                return Ok(());
            }
            for row in side_effect_rows {
                // D15: `newly_ready`/`dependents_to_block` の対象は、必ず未着手（`pending`）の
                // WU だけ（依存が未解決な限り `ready` には上がれないため）。
                let dep_reason = if row.status == task_core::WorkUnitStatus::Blocked {
                    "dependency_failed"
                } else {
                    "dependency_ready"
                };
                if let Err(e) = self.store.work_unit_transition(
                    task_id,
                    row.clone(),
                    Event::WorkUnitTransitioned {
                        work_unit_id: row.id.clone(),
                        key: row.key.clone(),
                        from: task_core::WorkUnitStatus::Pending,
                        to: row.status,
                        reason: dep_reason.to_string(),
                        run_id: None,
                    },
                ) {
                    tracing::warn!(%task_id, %run_id, work_unit = %row.key, error = %e, "failed to record a dependent work unit transition");
                }
            }
        }
        // ADR-0130 D2: WU の自動 commit の後に、この run（と done になった WU）の確定差分を残す。
        // git が読めなくても `unavailable` を残すだけで、run の遷移は変えない。
        {
            let (wu_row, wu_completed) = match &write_set_wu {
                Some((row, completed)) => (Some(row), *completed),
                None => (current_wu.as_ref(), false),
            };
            self.record_run_write_sets(&task, &run_id, wu_row, wu_completed);
        }
        // ADR-0079 D7（Phase R3a）: worker の決定の要求（`DecisionRequested`、path 付き）と、それが指した他の unit の
        // `blocked(decision)` を 1 トランザクションで残す（工程の判定〈下の `settle_phase`〉より前）。
        if let Some(found) = worker_decisions.take() {
            tracing::info!(%task_id, %run_id, decisions = found.count, self_hold = found.self_hold, held = found.held_rows.len(), "the worker asked humans for decisions (ADR-0079 D7)");
            if let Err(e) =
                self.store
                    .work_units_apply(task_id, Vec::new(), found.held_rows, found.events)
            {
                tracing::warn!(%task_id, %run_id, error = %e, "failed to record the worker's decision requests");
            }
        }
        // ADR-0072 D5（Phase E2）: `runs` 索引の finish（(g): 全タスクの run について書く）。
        {
            let index_status = run_end
                .map(task_core::RunIndexStatus::from_run_end)
                .unwrap_or(task_core::RunIndexStatus::HarnessError);
            if let Err(e) = self.store.run_index_finish(
                &run_id,
                index_status,
                checkpoint_for_index.clone(),
                usage,
                metrics,
                OffsetDateTime::now_utc(),
            ) {
                tracing::warn!(%task_id, %run_id, error = %e, "failed to finish the runs index row");
            }
        }
        // ADR-0074 D1.6（Phase F2b）: v2 の WU の run。段の進み方は WU の行を書いた後に
        // `run_settlement::phase_step_after_write` が決める。ここでは WU の行を読み直し、兄弟が走っている間に止まった
        // unit の trigger と質問（`deferred_work_unit_trigger`。events の追記を伴う）を適用する。
        let mut phase_step: Option<run_settlement::PhaseStep> = None;
        if let Some(wu) = &current_wu
            && wu.phase.is_some()
        {
            let units_now = self.store.work_units_for(task_id)?;
            let step = run_settlement::phase_step_after_write(&units_now, &wu.id, &trigger);
            if step.clear_questions {
                questions.clear();
            }
            if let Some(t) = step.trigger.clone() {
                trigger = t;
            }
            if let Some(id) = &step.deferred_unit {
                let (t, _outcome, q) = self.deferred_work_unit_trigger(task_id, &units_now, id)?;
                trigger = t;
                questions = q;
            }
            phase_step = Some(step);
        }
        // ADR 2026-10-20-mlsys-main2-fixes D1-4: 次の run（continuation・retry・requeue・次の unit・replan）や
        // reviewer を起こす終わり方でも、task 木の消費（この run を含む）が `max_tokens` に達していたら起こさず、
        // task を failed にする（人には聞かない）。兄弟の unit が走っている間（task を遷移させない）は最後の 1 本で決める。
        let stays_running = phase_step
            .as_ref()
            .is_some_and(run_settlement::PhaseStep::stays_running);
        if run_settlement::needs_tree_tokens(&trigger, stays_running)
            && let Some((root, max)) = token_budget::budget_scope(self.store.as_ref(), &task)?
        {
            let prior = token_budget::tree_tokens_used(self.store.as_ref(), root, Some(&run_id))?;
            let tokens = run_settlement::TreeTokens {
                prior,
                run: usage,
                max,
            };
            if let Some(stop) = run_settlement::stop_at_token_budget(tokens) {
                let (used, max) = (stop.used, stop.max);
                tracing::info!(%task_id, %run_id, used, max, previous = %outcome_str, "token budget reached; no continuation, retry or review (ADR 2026-10-20-mlsys-main2-fixes D1-4)");
                trigger = stop.trigger;
                outcome_str = stop.outcome;
                questions.clear();
                infra_requeue = None;
            }
        }
        let finished = run_settlement::run_end_record(
            &run_id,
            outcome_str.clone(),
            usage,
            None,
            metrics,
            run_end,
        );
        let mut events = vec![finished];
        if let Some(event) = integration_exhausted_event {
            events.push(event);
        }
        if let Some(event) = auto_replan_event {
            events.push(event);
        }
        // ADR-0079 付記 R7-5 D1: WU の checks の不合格（どの check が・どこで・どう落ちたか）を同じトランザクションで残す。
        if let (Some(f), Some(wu)) = (&check_failure, &current_wu) {
            events.push(f.event(&run_id, wu));
        }
        // ADR-0072 D5/D8（Phase E1）: `CheckpointSaved` は `WorkerFinished` と同じトランザクションで残す。
        if let Some(checkpoint_event) = checkpoint_event {
            events.push(checkpoint_event);
        }
        // ADR-0090 D2: wait を開く（`cluster_job_waits` の行は同じトランザクションで作られる）。atomic の run は task を
        // `ClusterJobWait` で止めるときだけ、WU の run は unit を `blocked(cluster_jobs)` にしたときだけ。
        if let Some(wait) = cluster_wait.take() {
            let parked = match &current_wu {
                None => matches!(trigger, Trigger::ClusterJobWait),
                Some(_) => wu_parked_on_cluster_jobs,
            };
            if parked {
                events.push(Event::ClusterJobWaitStarted {
                    wait: Box::new(wait),
                });
            } else {
                tracing::warn!(%task_id, %run_id, "the run asked for a cluster job wait, but another outcome took precedence; not waiting (ADR-0090)");
            }
        }
        if let Some(ev) = committed_event {
            events.push(ev);
        }
        if let Some(ev) = browser_prereq_event.take() {
            events.push(ev);
        }
        // ADR-0074 D4（Phase F3 quota）: この run の quota 消費を見積もる（`WorkerFinished` と同じ
        // トランザクションで残す。重なった run のグループがこれで閉じれば、他のメンバー分は
        // `RoutingRuntime::settle_quota` の中で別タスクへ直接書く）。
        {
            let model_for_quota = self.started_model_of(task_id, &run_id);
            let quota_event = self.routing.settle_quota(
                &*self.store,
                task_id,
                &run_id,
                current_wu.as_ref().map(|wu| wu.id.clone()),
                account.as_deref().zip(account_adapter),
                provider.to_string(),
                &model_for_quota,
                usage.as_ref(),
            );
            events.push(quota_event);
        }
        if let Some(reason) = failure_reason {
            match (&account, account_adapter) {
                // ADR-0024 D4: アカウントの cooldown として記録する（`ProviderThrottled` イベントは出さない）。
                (Some(acct), Some(adapter)) if reason != "spawn" => {
                    self.record_account_failure(adapter, acct, reason, &provider_outcome)
                }
                // ADR-0013 D9 / ADR 2026-10-08 §(e): プールを使わない、または Spawn 失敗（アカウント非依存）は
                // source の暗黙アカウントの cooldown として、遷移と同じトランザクションで記録する。
                _ => events.push(self.record_source_failure(&provider, reason, &provider_outcome)),
            }
        }
        // ADR-0033 D5（Phase 26 / Phase 27）: `Question` で終わった run（部をまたぐ委譲の質問への置き換えも
        // 含む）は、既存の `answers[]` の経路（`Status::Blocked`）に加えて `approvals` にも 1 件ずつ残す
        // （部またぎは 1 件の委譲につき 1 行。同じ質問がまだ未決なら増やさない）。
        for text in &questions {
            if let Err(e) = crate::approvals::record_question_approval(
                self.store.as_ref(),
                &task,
                text,
                OffsetDateTime::now_utc(),
            ) {
                tracing::warn!(%task_id, %run_id, error = %e, "failed to record the approval for this question");
            }
        }
        // ADR-0074 D1.6（Phase F2b）: 兄弟が走っている／工程の統合を始めるなら、Task は遷移させない
        // （events だけを残す）。
        if let Some(step) = phase_step.as_ref().filter(|step| step.stays_running()) {
            for ev in &events {
                self.store.append_event(task_id, ev)?;
            }
            tracing::info!(%task_id, %run_id, settle = ?step.phase, outcome = %outcome_str, "work unit finished (task stays running)");
            // ADR-0040 付記（2026-10-04、WU 検査の引き継ぎ）D1: draining の旧 instance は統合を始めない。段の WU が
            // 全部済んだ Task は lease の照合（ADR-0074 D1.7）で ready に戻り、新しい active の dispatch が統合を始める。
            if let Some(id) = step.integration()
                && self.coord.accepting_new_work()
            {
                self.start_integration(&task, id)?;
            }
            return Ok(());
        }
        match self
            .store
            .apply_transition_with_events(task_id, trigger, events)
        {
            Ok(outcome) => {
                tracing::info!(%task_id, %run_id, next = ?outcome.next, attempts = outcome.attempts, outcome = %outcome_str, "worker finished");
                // ADR-0070 D3（Phase 116）: `InfraRequeue` は `dispatch_ready` がすぐ拾わないよう、
                // バックオフの期限を立てる（30秒/2分/5分。`infra_backoff_delay`）。
                if let Some((n, delay)) = infra_requeue {
                    let until = OffsetDateTime::now_utc() + delay;
                    tracing::warn!(%task_id, %run_id, attempt = n, until = %until, "infra failure; requeued with backoff (attempts not consumed; ADR-0070 D3)");
                    self.supervisor.start_infra_backoff(task_id, until);
                }
                // ADR-0033 D4（Phase 24 / 監査 M-5）: 対話用タスクの run なら、`summary`（質問なら本文、
                // 失敗なら理由）をそのノードの返事として `messages` に残す。**「返事できませんでした」は
                // タスクが `Failed` に落ちたときだけ**（requeue / まだ試行が残る失敗では書かない）。
                self.record_conversation_reply(&task, &run_id, &outcome_str, outcome.next);
                // ADR-0038 D1 の `milestone_proposal` の取り込みは ADR-0079 D13（Phase R5a）で廃止（途中目標は凍結）。
                // ADR-0034 D2（監査 M-1〜M-3）: `question` は run の終端でそのまま届ける。`done` はレビューを
                // 通って `Status::Done` になってから（`on_review_finished` 側）作るので、ここでは作らない。
                // Phase 28: 対話 run の `Question` は `Done` 扱い（上の match）なので、ここでは報告しない
                // （レビューが通れば `on_review_finished` 側が通常の `Done` 報告を作る）。
                if !task_core::is_conversation(&task)
                    && matches!(
                        terminal_report,
                        Some(crate::reports::TerminalReport::Question { .. })
                    )
                    && let Some(question) = terminal_report.as_ref()
                    && let Err(e) = crate::reports::record_run_report(
                        self.store.as_ref(),
                        &task,
                        &run_id,
                        question,
                        None,
                        OffsetDateTime::now_utc(),
                    )
                {
                    tracing::warn!(%task_id, %run_id, error = %e, "failed to record the report for this run");
                }
                // `bad_news` は `Status::Failed` に遷移したときだけ、原因を問わず作る（ワーカー自身の `error`、
                // 供給側失敗が requeue 上限に達した場合のどちらも含む。監査 M-1）。
                if outcome.next == Status::Failed {
                    let bad_news = match &terminal_report {
                        Some(t @ crate::reports::TerminalReport::Error { .. }) => Some(t.clone()),
                        _ => adapter_error_text.as_ref().map(|message| {
                            crate::reports::TerminalReport::Error {
                                message: message.clone(),
                                retryable: true,
                            }
                        }),
                    };
                    if let Some(terminal) = bad_news.as_ref()
                        && let Err(e) = crate::reports::record_run_report(
                            self.store.as_ref(),
                            &task,
                            &run_id,
                            terminal,
                            None,
                            OffsetDateTime::now_utc(),
                        )
                    {
                        tracing::warn!(%task_id, %run_id, error = %e, "failed to record the report for this run");
                    }
                }
                if outcome.next == Status::Reviewing
                    && !self.spawn_review(task_id, run_id, &subject)?
                {
                    // Reviewer run の枠が無い: 次 tick の recover_reviews で再試行する。
                    self.coord.defer_review(task_id, subject);
                }
            }
            Err(StoreError::InvalidTransition(e)) => {
                tracing::warn!(%task_id, %run_id, error = %e, "worker result could not be applied");
            }
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }

    /// ADR-0033 D6（Phase 24）: 結果ファイル（`<artifacts_dir>/result.json`）の `memory` を担当の記憶に追記する。
    /// `[memory]` を設定していない・担当がいない・`memory` が無いときは何もしない。失敗しても run は壊さない。
    pub(super) fn absorb_memory(&self, task: &Task) {
        let (Some(dir), Some(node_id)) = (&self.config.memory_dir, task.assignee.as_deref()) else {
            return;
        };
        let Some(workspace) = self.task_dir(task) else {
            return;
        };
        // ADR-0036 D2: 結果ファイルはそのタスクの成果物ディレクトリの中。
        let Some(update) = task_worker::read_result_memory(&self.artifacts_dir(task, &workspace))
        else {
            return;
        };
        let today = OffsetDateTime::now_utc().date().to_string();
        let project = task.project_id.map(|p| p.to_string());
        if let Err(e) = MemoryDir::new(dir).append(node_id, project.as_deref(), &update, &today) {
            tracing::warn!(task_id = %task.id, error = %e, "failed to append to the node's memory");
        }
    }

    /// ADR-0033 D4（Phase 24 / 監査 M-5）: 対話用タスクの run の終わりを、そのノードの返事として
    /// `messages` に残す。`outcome_str` は `done: <summary>` / `question: <text>` /
    /// `error(...): <message>` のいずれか。`next` は遷移後の状態で、**失敗の返事は `Failed` のときだけ**
    /// 書く（retryable な途中失敗や requeue では、同じ問いに何度も「返事できませんでした」が並ばない）。
    pub(super) fn record_conversation_reply(
        &self,
        task: &Task,
        run_id: &str,
        outcome_str: &str,
        next: Status,
    ) {
        if !task_core::is_conversation(task) {
            return;
        }
        let mut metadata = None;
        let text = if let Some(summary) = outcome_str.strip_prefix("done: ") {
            let mut text = summary.to_string();
            // ADR-0048 D3（Phase 60b）: CoS が `done` で返ってきたときだけ、結果ファイルの `actions` を
            // 決定的に実行する。実行できなかった action があれば返事に節を足し、実行結果は metadata に残す。
            if let Some(outcome) = self.absorb_console_actions(task, run_id) {
                if let Some(note) = outcome.failure_note() {
                    text.push_str(&note);
                }
                metadata = outcome.to_metadata();
            }
            text
        } else if let Some(question) = outcome_str.strip_prefix("question: ") {
            // 質問は人への問いかけそのものなので、返事としてもそのまま見せる（`approvals` にも 1 行入る）。
            question.to_string()
        } else if next == Status::Failed {
            task_core::failure_reply(outcome_str)
        } else {
            return;
        };
        if let Err(e) = task_ops::conversation::record_reply_with_metadata(
            self.store.as_ref(),
            task,
            run_id,
            &text,
            metadata,
            OffsetDateTime::now_utc(),
        ) {
            tracing::warn!(task_id = %task.id, error = %e, "failed to record the conversation reply");
        }
    }

    /// ADR-0048 D3（Phase 60b）: CoS（根ノード。`OrgKind::Secretary`）の対話 run の結果ファイルの
    /// `actions` を決定的に実行する。CoS 以外の対話・対話でない run・宣言が無い run では何もしない
    /// （`None`）。冪等（`task_ops::actions::execute` が `run_id` を記録し、2 回目は `None`）。
    /// 失敗しても run は壊さない。
    pub(super) fn absorb_console_actions(
        &self,
        task: &Task,
        run_id: &str,
    ) -> Option<task_ops::actions::ActionsOutcome> {
        if !task_core::is_conversation(task) {
            return None;
        }
        let assignee = task.assignee.as_deref()?;
        let org = self.store.org_list().ok()?;
        let node = org.iter().find(|n| n.id == assignee)?;
        if node.kind != OrgKind::Secretary {
            return None;
        }
        let workspace = self.task_dir(task)?;
        let artifacts_dir = self.artifacts_dir(task, &workspace);
        let parsed = task_worker::read_result_actions(&artifacts_dir);
        if parsed.is_empty() {
            return None;
        }
        // Phase 98（ADR-0018）: `create_task.workspace` がクラスタを指すときに `[[clusters]]` へ照らして
        // 検証するため、既知のクラスタ id を渡す。
        let known_clusters: Vec<String> = self.config.clusters.keys().cloned().collect();
        match task_ops::actions::execute(
            self.store.as_ref(),
            &org,
            &self.config.roles,
            &self.config.genres,
            &known_clusters,
            task,
            run_id,
            &parsed.valid,
            &parsed.malformed,
            OffsetDateTime::now_utc(),
        ) {
            Ok(outcome) => outcome,
            Err(e) => {
                tracing::warn!(task_id = %task.id, %run_id, error = %e, "failed to execute console actions");
                None
            }
        }
    }

    /// ADR-0072 D15（Phase E2）: `run_id` の run が「まだ `running` の WU」に属していたら、
    /// checkpoint があれば `needs_continuation`、無ければ `ready` に戻す
    /// （`WorkUnitTransitioned{reason}`）。属していなければ何もしない（`Ok(())`）。
    /// ADR-0074 D1.5（Phase F2）: `run_id` の run がまだこの Task の lease を持っているか。
    /// v1・atomic は従来どおり Task の lease の保持者と比べる。v2（工程の lease）では WU の
    /// `lease_run_id` と比べる（lease を失った WU の run の結果を捨てる判定）。
    /// Phase F5-fix2: run の完了（`on_worker_finished` / `on_work_unit_checks_finished`）の確定が
    /// エラーで終わった。`drain_completions` が `?` で tick ごと抜けると受信済みの完了が消え、run は
    /// `running` のまま lease 切れまで残る。ここでエラーを event に残し、run を「インフラ都合の失敗」
    /// （ADR-0070 D3 の `InfraRequeue`、上限を超えたら `infra failure ×N`）として閉じる。記録にも
    /// 失敗したら ERROR だけ残す（lease 切れの経路が拾い、`runs/<run_id>/result.json` があれば
    /// そこから確定させる）。
    pub(super) fn record_finalisation_failure(
        &mut self,
        task_id: TaskId,
        run_id: &str,
        error: &DispatchError,
    ) {
        tracing::error!(%task_id, %run_id, %error, "failed to finalise a finished run; recording it as an infra failure (Phase F5-fix2)");
        if let Err(e) = self.close_run_after_finalisation_failure(task_id, run_id, error) {
            tracing::error!(%task_id, %run_id, error = %e, "could not record the finalisation failure either; the lease expiry will reclaim the run");
        }
    }

    pub(super) fn close_run_after_finalisation_failure(
        &mut self,
        task_id: TaskId,
        run_id: &str,
        error: &DispatchError,
    ) -> Result<(), DispatchError> {
        let Some(task) = self.store.get(task_id)? else {
            return Ok(());
        };
        // 途中まで書けていて、既に run が lease を手放している（Task の遷移まで済んだ）なら何もしない。
        if !self.run_holds_lease(&task, run_id)? {
            return Ok(());
        }
        let Some(lease) = task.lease.clone() else {
            return Ok(());
        };
        let now = OffsetDateTime::now_utc();
        let events = self.store.events_for(task_id)?;
        let infra_end = task_core::RunEnd::HarnessError {
            class: task_core::HarnessErrorClass::Infra,
        };
        let finished = |outcome: String| {
            run_settlement::run_end_record(run_id, outcome, None, None, None, Some(infra_end))
        };
        if let Err(e) = self.store.run_index_finish(
            run_id,
            task_core::RunIndexStatus::HarnessError,
            None,
            None,
            None,
            now,
        ) {
            tracing::warn!(%task_id, %run_id, error = %e, "failed to finish the runs index row");
        }
        if is_phase_lease_holder(&lease.worker_run_id) {
            // ADR-0074 D1.5/D1.7: 工程の lease（v2）。兄弟の WU の run を巻き込まないよう Task は
            // 遷移させず、この WU だけを戻す（何も走っていなければ `reconcile_parallel_tasks` が
            // Task を ready に戻す）。同じ WU で `max_infra_retries` を超えたら WU を failed にする
            // （ADR-0072 D12/D17: replan の余地があれば replan、無ければ Task の失敗）。
            let wu = self.store.work_units_for(task_id)?.into_iter().find(|u| {
                u.status == task_core::WorkUnitStatus::Running
                    && u.lease_run_id.as_deref() == Some(run_id)
            });
            let Some(wu) = wu else {
                return Ok(());
            };
            let failures_so_far = events
                .iter()
                .filter(|(_, e)| {
                    matches!(
                        e,
                        Event::WorkUnitTransitioned { work_unit_id, reason, .. }
                            if work_unit_id == &wu.id && reason == FINALISE_FAILED_REASON
                    )
                })
                .count() as u32;
            let infra_n = failures_so_far + 1;
            let exhausted = infra_retry_after(infra_n, self.config.max_infra_retries).is_none();
            let outcome = if exhausted {
                format!("{INFRA_FAILURE_MARKER}{infra_n}: finalisation failed: {error}")
            } else {
                format!("infra_requeue: finalisation failed: {error}")
            };
            self.store.append_event(task_id, &finished(outcome))?;
            if exhausted {
                let mut updated = wu.clone();
                updated.status = task_core::WorkUnitStatus::Failed;
                updated.clear_lease();
                updated.updated_at = rfc3339(now);
                self.store.work_unit_transition(
                    task_id,
                    updated,
                    Event::WorkUnitTransitioned {
                        work_unit_id: wu.id.clone(),
                        key: wu.key.clone(),
                        from: task_core::WorkUnitStatus::Running,
                        to: task_core::WorkUnitStatus::Failed,
                        reason: FINALISE_FAILED_REASON.to_string(),
                        run_id: Some(run_id.to_string()),
                    },
                )?;
            } else {
                self.reconcile_work_unit_run(task_id, run_id, FINALISE_FAILED_REASON)?;
            }
            return Ok(());
        }
        // Task の lease をこの run が持つ（atomic・v1 の WU の run）: lease 切れの回収と同じ遷移。
        let infra_n = consecutive_infra_requeues(&events) + 1;
        let retry = infra_retry_after(infra_n, self.config.max_infra_retries);
        let (trigger, outcome) = if retry.is_some() {
            (
                Trigger::InfraRequeue,
                format!("infra_requeue: finalisation failed: {error}"),
            )
        } else {
            (
                Trigger::WorkerError { retryable: false },
                format!("{INFRA_FAILURE_MARKER}{infra_n}: finalisation failed: {error}"),
            )
        };
        self.store.apply_transition_with_events(
            task_id,
            trigger.clone(),
            vec![finished(outcome)],
        )?;
        if let Some(delay) = retry {
            self.supervisor.start_infra_backoff(task_id, now + delay);
        }
        self.reconcile_work_unit_run(task_id, run_id, FINALISE_FAILED_REASON)?;
        Ok(())
    }

    /// Phase F5-fix2（P-F5-3 の result.json の部分）: lease（または WU の lease）が切れた run で、
    /// このインスタンスが抱えていない（`running`/`checking` に無い）もののうち、
    /// `runs/<run_id>/result.json` に終端が残っているものは、requeue せずにその内容で確定させる
    /// （`on_worker_finished` と同じ経路。WU の `checks` があればここから走り直す）。
    /// 本番では draining の旧デーモンが WU の検査の途中で exit し、完了した run が `lease expired`
    /// で捨てられてやり直しになった。確定させた（または確定の失敗を記録した）ら `true`。
    pub(super) fn finalise_from_result_json(&mut self, task: &Task, run_id: &str) -> bool {
        if ownerless_runs::holds_run_or_check(&self.supervisor, &self.coord, run_id) {
            return false;
        }
        let Some(dir) = self.task_dir(task) else {
            return false;
        };
        let Some(terminal) = terminal_from_run_dir(&dir, run_id) else {
            return false;
        };
        let provider = self
            .store
            .events_for(task.id)
            .ok()
            .and_then(|events| {
                events.iter().rev().find_map(|(_, e)| match e {
                    Event::WorkerStarted {
                        run_id: r,
                        provider,
                        ..
                    } if r == run_id => provider.clone(),
                    _ => None,
                })
            })
            .unwrap_or_default();
        tracing::warn!(task_id = %task.id, %run_id, "the run's lease expired (or its daemon is gone) but it left a terminal result.json; finalising from it instead of requeueing (Phase F5-fix2 / F5-fix6)");
        if let Err(e) = self.on_worker_finished(
            task.id,
            run_id.to_string(),
            provider,
            Ok(RunOutcome {
                terminal,
                exit_code: None,
            }),
        ) {
            self.record_finalisation_failure(task.id, run_id, &e);
        }
        true
    }
}

/// ADR-0072 D5（E2b の指摘、Phase E3 で配線）: reviewer run の `runs` 索引の finish。`completed_review_run`
/// が `None`（Reviewer run を起動しなかった判定）なら何もしない。失敗しても run は壊さない（警告のみ）。
///
/// ADR-0074 §6 F1 (k)（Phase F1 で直した逸脱）: 以前はここで `usage` を常に `None` に固定しており、
/// reviewer run の `runs` 索引の行が（`WorkerFinished` に実際の usage があっても）`usage_json` を
/// 持たないまま確定していた（E6 report 問題 3、`celerisctl replay --check` で
/// `rebuild_work_units_and_runs` の usage と食い違う）。`usage` を引数で受け取り、そのまま渡す。
pub(super) fn finish_reviewer_run_index(
    store: &dyn TaskStore,
    completed_review_run: &Option<String>,
    status: task_core::RunIndexStatus,
    usage: Option<task_core::Usage>,
    metrics: Option<task_core::RunMetrics>,
) {
    let Some(run_id) = completed_review_run else {
        return;
    };
    if let Err(e) = store.run_index_finish(
        run_id,
        status,
        None,
        usage,
        metrics,
        OffsetDateTime::now_utc(),
    ) {
        tracing::warn!(%run_id, error = %e, "failed to finish the reviewer run in the runs index");
    }
}

/// (k): `reviewer_finished`（`Some(Event::WorkerFinished{..})` なら）が運ぶ `usage` を取り出す
/// （`finish_reviewer_run_index` に渡すため。move で消費する前に呼ぶ）。
pub(super) fn worker_finished_usage(event: &Option<Event>) -> Option<task_core::Usage> {
    match event {
        Some(Event::WorkerFinished { usage, .. }) => *usage,
        _ => None,
    }
}

/// ADR-0074 §6 F1 (k)（Phase F1 で直した逸脱）: reviewer run の `WorkerFinished.end` は今まで常に
/// `None` だった（`task_ops::replay::rebuild_work_units_and_runs` はこれを `HarnessError` として
/// 復元するので、`celerisctl replay --check` は reviewer run のたびに `status` の食い違いを報告して
/// いた）。この関数が最終的に決めた `RunIndexStatus` と同じ `RunEnd` を event 自身にも書き戻す。
pub(super) fn set_worker_finished_end(event: &mut Option<Event>, end: task_core::RunEnd) {
    if let Some(Event::WorkerFinished { end: slot, .. }) = event.as_mut() {
        *slot = Some(end);
    }
}

/// ADR-0072 D9（Phase E1）: この run が continuation（予算切れ・yield の続き）なら、次の run の
/// `RunContext.continuation` に渡す最小限の文脈を events から純粋に組み立てる。`events` には、
/// これから始まる run 自身の `WorkerStarted`（run_seq の計算に使う）が既に入っている前提
/// （`run_worker` が `dispatch` の遷移と `WorkerStarted` の追記のあとに呼ばれるため）。
/// continuation でなければ `None`（前の run の会話・出力の全文は載せない。D9）。
pub(super) fn build_continuation_context(
    events: &[(u64, Event)],
) -> Option<task_worker::ContinuationContext> {
    let after_wait = cluster_job_wait::last_worker_run_waited(events);
    if consecutive_continuations(events) == 0 && !after_wait {
        return None;
    }
    let checkpoint = latest_checkpoint(events, None)?;
    let run_seq = current_run_seq(events);
    let previous_end = events
        .iter()
        .rev()
        .find_map(|(_, ev)| match ev {
            Event::WorkerFinished {
                role: None,
                end: Some(e),
                ..
            } => Some(run_settlement::describe_run_end(*e)),
            _ => None,
        })
        .unwrap_or_else(|| "budget_exhausted".to_string());
    // これまでの run の 1 行要約（古い順、最大 10 件）。
    let mut seq_by_run: HashMap<&str, u32> = HashMap::new();
    let mut n = 0u32;
    for (_, ev) in events {
        if let Event::WorkerStarted {
            run_id, role: None, ..
        } = ev
        {
            n += 1;
            seq_by_run.insert(run_id.as_str(), n);
        }
    }
    let mut prior_runs: Vec<String> = Vec::new();
    for (_, ev) in events {
        if let Event::WorkerFinished {
            run_id,
            role: None,
            end: Some(e),
            ..
        } = ev
            && let Some(seq) = seq_by_run.get(run_id.as_str())
        {
            prior_runs.push(format!(
                "Run #{seq} {}",
                run_settlement::describe_run_end(*e)
            ));
        }
    }
    if prior_runs.len() > 10 {
        let start = prior_runs.len() - 10;
        prior_runs = prior_runs.split_off(start);
    }
    let checkpoint_json = serde_json::to_value(&checkpoint).ok()?;
    Some(task_worker::ContinuationContext {
        run_seq,
        previous_end,
        checkpoint: checkpoint_json,
        prior_runs,
        cluster_jobs: if after_wait {
            cluster_job_wait::cluster_jobs_from_events(events)
        } else {
            None
        },
    })
}

/// ADR-0072 D8: checkpoint の `tests_run`（最大 10 件）と `recent_activity`（最大 20 行）の材料。この run の
/// `WorkerProgress{kind: tool_use}` と、それに続く `tool_result` の組（WU の run と atomic task の run で同じ）。
fn tool_activity(events: &[(u64, Event)], run_id: &str) -> Vec<crate::checkpoint::ToolActivity> {
    events
        .iter()
        .filter_map(|(_, ev)| match ev {
            Event::WorkerProgress {
                run_id: r,
                kind: Some(task_core::ProgressKind::ToolUse),
                tool,
                summary,
                ..
            } if r == run_id => Some(crate::checkpoint::ToolActivity::Use {
                tool: tool.clone(),
                summary: summary.clone(),
            }),
            Event::WorkerProgress {
                run_id: r,
                kind: Some(task_core::ProgressKind::ToolResult),
                error,
                ..
            } if r == run_id => Some(crate::checkpoint::ToolActivity::Result { error: *error }),
            _ => None,
        })
        .collect()
}

/// ADR-0072 D9: worker が書いた checkpoint。優先順位は `result.json.yield` > `checkpoint.json`。
fn worker_checkpoint_input(
    yield_checkpoint: Option<&serde_json::Value>,
    artifacts_dir: Option<&Path>,
) -> Option<task_core::WorkerCheckpointInput> {
    yield_checkpoint
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .or_else(|| artifacts_dir.and_then(crate::checkpoint::read_worker_checkpoint))
}
