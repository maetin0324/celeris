//! worker run の終了処理（`finish_worker_result` と finalise の失敗の後始末）。同じ transaction の組と `release_quota_if_tracked` はそのまま。ADR-0082 の L3。

use super::*;

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
        let mut subject = ReviewSubject::default();
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
        // ADR-0033 D4 / SPEC §3.1: 部をまたぐ委譲を試みた run は、子を作らずに秘書へ聞く終わり方にする
        // （`StoreSink::delegate_impl` が `QuestionRaised` を残している）。
        let cross_department =
            cross_department_questions_of(&self.store.events_for(task_id)?, &run_id);
        // ADR-0070 D3（Phase 116）: `Trigger::InfraRequeue` を選んだときだけ `Some(n)`（n 回目の
        // インフラ再試行）。`Ok(outcome) =>` の中で `self.infra_backoff` のバックオフ期限を立てるのに使う。
        let mut infra_requeue_n: Option<u32> = None;
        // ADR-0072 D7（Phase E1）: この run の構造化した終わり方（`WorkerFinished.end` に写す）。
        let mut run_end: Option<task_core::RunEnd> = None;
        // ADR-0072 D9: `Terminal::Yielded` の生の checkpoint JSON（`result.json` の `yield`）。
        let mut yield_checkpoint_json: Option<serde_json::Value> = None;
        // ADR-0090 D1/D2: `Terminal::Waiting` を検証して組んだクラスタ job の wait（`ClusterJobWaitStarted` にする）。
        let mut cluster_wait: Option<task_core::cluster_job::ClusterJobWait> = None;
        let current_wu_id = current_wu.as_ref().map(|w| w.id.clone());
        let (mut trigger, mut outcome_str, usage, provider_outcome) = match result {
            Ok(RunOutcome {
                terminal:
                    Terminal::Done {
                        summary,
                        usage,
                        evidence,
                    },
                ..
            }) => {
                subject = ReviewSubject {
                    summary: summary.clone(),
                    evidence,
                };
                run_end = Some(task_core::RunEnd::Completed);
                (
                    Trigger::WorkerDone,
                    format!("done: {summary}"),
                    usage,
                    ProviderOutcome::Ok,
                )
            }
            // ADR-0033 D4 / Phase 28: 対話 run は `Question` を出さない。人に聞きたいことは返事に書けば
            // よいので、そのまま `Done` 扱いにする（`approvals` の行は作らない。実機で秘書が「最終試行なので
            // 自分の一般知識で答えた」まま `Question` の代わりに走った事故の反省）。
            Ok(RunOutcome {
                terminal: Terminal::Question { text },
                ..
            }) if task_core::is_conversation(&task) => {
                subject = ReviewSubject {
                    summary: text.clone(),
                    evidence: Vec::new(),
                };
                run_end = Some(task_core::RunEnd::Completed);
                (
                    Trigger::WorkerDone,
                    format!("done: {text}"),
                    None,
                    ProviderOutcome::Ok,
                )
            }
            Ok(RunOutcome {
                terminal: Terminal::Question { text },
                ..
            }) => {
                run_end = Some(task_core::RunEnd::Question);
                (
                    Trigger::WorkerQuestion,
                    format!("question: {text}"),
                    None,
                    ProviderOutcome::Ok,
                )
            }
            Ok(RunOutcome {
                terminal: Terminal::Error { message, retryable },
                ..
            }) => {
                // ADR-0072 D7: 二重の安全網。構造化されていない `Error` でも、予算切れの語彙なら
                // `BudgetExhausted` として分類する（usage はこの経路では運べない）。
                run_end = if retryable {
                    classify_budget_kind_from_text(&message)
                        .map(|kind| task_core::RunEnd::BudgetExhausted { kind })
                } else {
                    None
                };
                if run_end.is_none() {
                    run_end = Some(task_core::RunEnd::Failed { retryable });
                }
                (
                    Trigger::WorkerError { retryable },
                    format!("error(retryable={retryable}): {message}"),
                    None,
                    ProviderOutcome::Ok,
                )
            }
            // ADR-0072 D9/D10（Phase E1）: graceful yield（result.json の `{"yield": {...}}`）。
            // `trigger`/`outcome_str` はここでは仮の値で、continuation の判定（下）で確定させる。
            Ok(RunOutcome {
                terminal: Terminal::Yielded { checkpoint, usage },
                ..
            }) => {
                run_end = Some(task_core::RunEnd::Yielded);
                yield_checkpoint_json = Some(checkpoint);
                (
                    Trigger::WorkerError { retryable: true },
                    "error(retryable=true): yielded".to_string(),
                    usage,
                    ProviderOutcome::Ok,
                )
            }
            // ADR-0090 D1: クラスタ job の終了待ち。検証に通れば run を `waiting` で閉じ、task / unit を止める
            // （continuation の回数・進捗なし・attempts に数えない）。通らなければ retryable な失敗。
            Ok(RunOutcome {
                terminal:
                    Terminal::Waiting {
                        request,
                        checkpoint,
                        usage,
                    },
                ..
            }) => match self.cluster_job_wait_for(
                &task,
                &run_id,
                current_wu_id.as_deref(),
                &request,
                checkpoint.clone(),
            ) {
                Ok(wait) => {
                    run_end = Some(task_core::RunEnd::Waiting);
                    yield_checkpoint_json = checkpoint;
                    let line = format!(
                        "waiting: クラスタ {} の {} job の終了を待ちます: {}",
                        wait.cluster,
                        wait.scheduler.as_str(),
                        wait.jobs.join(" ")
                    );
                    cluster_wait = Some(wait);
                    (Trigger::ClusterJobWait, line, usage, ProviderOutcome::Ok)
                }
                Err(reason) => {
                    run_end = Some(task_core::RunEnd::Failed { retryable: true });
                    (
                        Trigger::WorkerError { retryable: true },
                        format!("error(retryable=true): invalid cluster job wait: {reason}"),
                        usage,
                        ProviderOutcome::Ok,
                    )
                }
            },
            // ADR-0072 D7（Phase E1）: turn / wall-clock / context の上限に当たった。usage を運ぶ。
            Ok(RunOutcome {
                terminal:
                    Terminal::BudgetExhausted {
                        kind,
                        message,
                        usage,
                    },
                ..
            }) => {
                run_end = Some(task_core::RunEnd::BudgetExhausted { kind });
                (
                    Trigger::WorkerError { retryable: true },
                    format!("error(retryable=true): budget exhausted ({kind:?}): {message}"),
                    usage,
                    ProviderOutcome::Ok,
                )
            }
            Err(e) => match provider_failure_outcome(&e) {
                // ADR-0010 D5（P-21）: 供給側失敗は attempts を消費せず requeue し、プロバイダを cooldown にする。
                Some(po)
                    if consecutive_requeues(&self.store.events_for(task_id)?)
                        < self.config.max_requeues =>
                {
                    (Trigger::Requeue, format!("requeue: adapter: {e}"), None, po)
                }
                // ADR-0011（P-38）: 同じ試行での連続 requeue が上限に達したら、通常の失敗として attempts を消費する。
                Some(po) => (
                    Trigger::WorkerError { retryable: true },
                    format!(
                        "error(retryable=true): requeue limit ({}) reached: adapter: {e}",
                        self.config.max_requeues
                    ),
                    None,
                    po,
                ),
                // ADR-0070 D3（Phase 116）: プロバイダが分類できない失敗（resume 拒否・プロセス
                // I/O・result.json 不在など）は「インフラ都合」として attempts を消費せず、
                // `max_infra_retries` までバックオフして再試行する。上限に達したときだけ
                // `WorkerError{retryable:false}`（無条件に `Failed`）で打ち切り、`"infra failure ×N"`
                // を付ける（D1 の失敗分類がこの接頭辞を見る）。
                None => {
                    let infra_n = consecutive_infra_requeues(&self.store.events_for(task_id)?) + 1;
                    if infra_n <= self.config.max_infra_retries {
                        infra_requeue_n = Some(infra_n);
                        (
                            Trigger::InfraRequeue,
                            format!("infra_requeue: adapter: {e}"),
                            None,
                            ProviderOutcome::Ok,
                        )
                    } else {
                        (
                            Trigger::WorkerError { retryable: false },
                            format!("{INFRA_FAILURE_MARKER}{infra_n}: adapter: {e}"),
                            None,
                            ProviderOutcome::Ok,
                        )
                    }
                }
            },
        };
        // ADR-0079 付記 R7-5 D4: checks の不合格で `Terminal::Error` にすり替えた run も、worker が返した usage を残す。
        let usage = usage.or_else(|| check_failure.as_ref().and_then(|f| f.usage));
        // ADR-0079 付記 R7-5 D2: outcome の文に足す不合格の要約（無ければ空）。
        let check_failure_suffix = check_failure
            .as_ref()
            .map(|f| format!(": {}", f.summary()))
            .unwrap_or_default();
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
        // ADR-0079 D7（Phase R3a）: worker が `result.json` の `decisions` で出した決定の要求（記録と止める unit）。
        let mut worker_decisions: Option<WorkerDecisions> = None;
        // ADR-0090 D2: この WU の run が unit を `blocked(cluster_jobs)` にした。
        let mut wu_parked_on_cluster_jobs = false;
        if let Some(wu) = &current_wu {
            // ADR-0072 D6（Phase E2）: WU の run。`end` が無ければ（分類できない供給側・インフラの
            // 失敗）、harness_error 相当として WU を ready に戻すだけで、Task レベルの trigger は
            // 触らない（既存の Requeue/InfraRequeue/WorkerError の経路のまま。D6 の表どおり）。
            let effective_end = run_end.unwrap_or(task_core::RunEnd::HarnessError {
                class: task_core::HarnessErrorClass::Infra,
            });
            let reset_only = matches!(
                effective_end,
                task_core::RunEnd::HarnessError { .. } | task_core::RunEnd::Cancelled
            );
            let mut checkpoint_opt: Option<task_core::Checkpoint> = None;
            let mut prev_checkpoint_opt: Option<task_core::Checkpoint> = None;
            let mut no_progress_before = 0u32;
            // ADR-0090 D1: クラスタ job の wait も checkpoint を残す（`[execution] continuation` に依らない）。
            if (effective_end.is_continuable() && self.config.execution.continuation)
                || effective_end == task_core::RunEnd::Waiting
            {
                let events_so_far = self.store.events_for(task_id)?;
                // ADR-0074 D1.6（Phase F2b）: v2 の WU は WU の worktree・ブランチ・base で取る。
                let (artifacts_dir, cwd_buf, branch, base) =
                    self.work_unit_checkpoint_site(&task, wu);
                let cwd = cwd_buf.as_deref();
                let activity: Vec<crate::checkpoint::ToolActivity> = events_so_far
                    .iter()
                    .filter_map(|(_, ev)| match ev {
                        Event::WorkerProgress {
                            run_id: r,
                            kind: Some(task_core::ProgressKind::ToolUse),
                            tool,
                            summary,
                            ..
                        } if r == &run_id => Some(crate::checkpoint::ToolActivity::Use {
                            tool: tool.clone(),
                            summary: summary.clone(),
                        }),
                        Event::WorkerProgress {
                            run_id: r,
                            kind: Some(task_core::ProgressKind::ToolResult),
                            error,
                            ..
                        } if r == &run_id => {
                            Some(crate::checkpoint::ToolActivity::Result { error: *error })
                        }
                        _ => None,
                    })
                    .collect();
                let mechanical =
                    crate::checkpoint::gather_from(cwd, &branch, base.as_deref(), &activity);
                let worker_checkpoint = yield_checkpoint_json
                    .as_ref()
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .or_else(|| {
                        artifacts_dir
                            .as_deref()
                            .and_then(crate::checkpoint::read_worker_checkpoint)
                    });
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
                checkpoint_event = Some(Event::CheckpointSaved {
                    run_id: run_id.clone(),
                    work_unit_id: Some(wu.id.clone()),
                    checkpoint: Box::new(checkpoint.clone()),
                });
                checkpoint_opt = Some(checkpoint);
            }
            checkpoint_for_index = checkpoint_opt.clone();

            if effective_end.is_continuable() && !self.config.execution.continuation {
                // ADR-0072 §6 (f): `[execution] continuation = false` なら従来どおり
                // `WorkerError{retryable:true}` に戻す（WU の状態は変えない）。
                trigger = Trigger::WorkerError { retryable: true };
                outcome_str = format!(
                    "error(retryable=true): {} (continuation disabled)",
                    describe_run_end(effective_end)
                );
            } else {
                let units = self.store.work_units_for(task_id)?;
                let limits = crate::execution_scheduler::WuLimits {
                    max_continuations: self.config.execution.max_continuations_per_work_unit,
                    no_progress_limit: self.config.execution.no_progress_limit,
                    max_retries: task.budget.max_retries,
                };
                let decision = crate::execution_scheduler::decide(
                    effective_end,
                    &run_id,
                    wu,
                    &units,
                    crate::execution_scheduler::ContinuationInputs {
                        checkpoint: checkpoint_opt.as_ref(),
                        prev_checkpoint: prev_checkpoint_opt.as_ref(),
                        no_progress_before,
                    },
                    limits,
                );
                if !reset_only {
                    trigger = decision.trigger;
                    match decision.reason {
                        "failed" => {
                            let msg = outcome_str
                                .strip_prefix("error(retryable=false): ")
                                .or_else(|| outcome_str.strip_prefix("error(retryable=true): "))
                                .unwrap_or(outcome_str.as_str());
                            outcome_str = format!(
                                "error(retryable=false): work unit {} failed: {msg}",
                                wu.key
                            );
                        }
                        "limit" => {
                            if let Some(question) = &decision.outcome_override {
                                outcome_str = question.clone();
                            }
                        }
                        "continue" => {
                            outcome_str = format!(
                                "continue: {} の続き（WorkUnit {}, Run #{}）",
                                describe_run_end(effective_end),
                                wu.key,
                                wu.runs + 1
                            );
                        }
                        "retry" => {
                            outcome_str = format!(
                                "work_unit_retry: WorkUnit {} を最初からやり直します（{}/{}）{check_failure_suffix}",
                                wu.key, decision.updated.retries, limits.max_retries
                            );
                        }
                        // ADR-0090 D2: unit はクラスタ job を待つ（`blocked(cluster_jobs)`）。v2 の task は兄弟を止めず
                        // `advance` で進み、段階の unit を順に並べる v1 の task は task ごと待つ。
                        "cluster_jobs" => {
                            outcome_str = format!("{outcome_str}（WorkUnit {}）", wu.key);
                            if wu.phase.is_none() {
                                trigger = Trigger::ClusterJobWait;
                            }
                        }
                        // ADR-0072 D17 3.（Phase E4b 項目2）: worker の checkpoint/result.json が
                        // `plan_issue` を書いた。
                        "plan_issue" => {
                            let text = checkpoint_opt
                                .as_ref()
                                .and_then(|cp| cp.plan_issue.clone())
                                .unwrap_or_default();
                            outcome_str = format!(
                                "question: WorkUnit {} が計画の問題を申告しました: {text}",
                                wu.key
                            );
                        }
                        _ => {}
                    }
                    // ADR-0072 D17（Phase E4）/ D17 3.（Phase E4b 項目2）: WU が failed、または
                    // 進捗なし/continuation の上限（"limit"）に達した、または `plan_issue` を
                    // 申告したら、replan の余地（`max_replans`）があれば Task を failed/blocked に
                    // する代わりに replan の planner run を起こす（`ContinueWhy::Replan`）。
                    // D12「失敗にしないもの」: 進捗なし・継続の上限到達は元々失敗にしない。
                    // D12 3.: WU failed は「replan できない」ときだけ failed にする。
                    if task_core::is_integration_repair_unit(wu.kind, &wu.spec.title)
                        && (matches!(decision.reason, "failed" | "limit" | "plan_issue")
                            || (decision.reason == "completed"
                                && !self.integration_repair_result_trusted(&task, wu)?))
                    {
                        let reason = match decision.reason {
                            "plan_issue" => task_core::IntegrationRepairExhaustReason::PlanIssue,
                            "failed" => task_core::IntegrationRepairExhaustReason::WorkUnitFailed,
                            "completed" => {
                                task_core::IntegrationRepairExhaustReason::ResultUntrusted
                            }
                            _ if matches!(
                                effective_end,
                                task_core::RunEnd::BudgetExhausted { .. }
                            ) =>
                            {
                                task_core::IntegrationRepairExhaustReason::BudgetExhausted
                            }
                            _ => task_core::IntegrationRepairExhaustReason::WorkUnitFailed,
                        };
                        let (fallback, event) =
                            self.integration_repair_exhaustion(&task, wu, reason)?;
                        integration_exhausted_event = event;
                        trigger = if fallback {
                            Trigger::WorkerDone
                        } else {
                            Trigger::WorkerQuestion
                        };
                        outcome_str = if fallback {
                            format!(
                                "integration repair {} exhausted; review unsynced HEAD",
                                wu.key
                            )
                        } else {
                            format!(
                                "question: integration repair {} needs manual inspection",
                                wu.key
                            )
                        };
                    } else if matches!(decision.reason, "failed" | "limit" | "plan_issue") {
                        let replans_so_far = self.counted_replans(task_id)?;
                        if replans_so_far < self.effective_max_replans(task_id)? {
                            let why = match decision.reason {
                                "failed" => {
                                    format!("work unit {} failed{check_failure_suffix}", wu.key)
                                }
                                "limit" => format!("work unit {} made no progress", wu.key),
                                _ => {
                                    let text = checkpoint_opt
                                        .as_ref()
                                        .and_then(|cp| cp.plan_issue.clone())
                                        .unwrap_or_default();
                                    format!("work unit {} reported a plan issue: {text}", wu.key)
                                }
                            };
                            trigger = Trigger::Continue {
                                why: task_core::ContinueWhy::Replan,
                            };
                            outcome_str = format!("replan: {why}");
                        } else if decision.reason == "failed" {
                            // ADR-0079 付記「R6-1」D3: replan を使い切った後の WU の失敗で task を黙って `failed`
                            // にしない（web Phase 0 の 17:56Z）。木の節点は `limit:max_replans` の決定、木で
                            // ない task は人への質問（回答 = 人の replan、上限に数えない）。
                            let msg = outcome_str
                                .strip_prefix("error(retryable=false): ")
                                .unwrap_or(outcome_str.as_str())
                                .to_string();
                            let (t, o) = self.replan_exhausted_ask(&task, &msg, replans_so_far)?;
                            trigger = t;
                            outcome_str = o;
                        }
                    }
                    if decision.plan_complete {
                        // D15: Task の完了。`ReviewSubject.summary` は WU ごとの最終 checkpoint の
                        // `completed` を key ごとに 1 段落ずつ並べた決定的な要約にする（evidence は
                        // この最後の run のものを残す）。checkpoint が無い WU は「完了」とだけ書く。
                        let mut paragraphs = Vec::new();
                        for u in &units {
                            if u.id == wu.id {
                                continue;
                            }
                            if !u.status.is_active() || u.status != task_core::WorkUnitStatus::Done
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
                        let this_completed = checkpoint_for_index
                            .as_ref()
                            .map(|cp| cp.completed.join("; "))
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| subject.summary.clone());
                        paragraphs.push(format!("{}: {}", wu.spec.title, this_completed));
                        subject.summary = paragraphs.join("\n");
                    }
                }
                let mut all_new_rows = decision.newly_blocked.clone();
                all_new_rows.extend(decision.newly_ready.clone());
                let mut updated_row = decision.updated;
                let mut wu_reason = decision.reason;
                // ADR-0079 D7（Phase R3a）: 木の節点の leaf の run が `result.json` の `decisions` で人への決定の
                // 要求を出した。記録し（path 付き）、`needed_before: self` ならこの leaf を done にせず
                // `blocked(decision)` にする（答えは次の run の前置きの「人の決定」節に入る）。他の unit を指した決定は
                // その unit だけを止め、この run の完了は妨げない。
                if wu_reason == "completed" && !reset_only {
                    let (site_artifacts, _, _, _) = self.work_unit_checkpoint_site(&task, wu);
                    if let Some(found) =
                        self.worker_decisions(&task, Some(wu), site_artifacts.as_deref(), &run_id)?
                    {
                        if found.self_hold {
                            updated_row.status = task_core::WorkUnitStatus::Blocked;
                            updated_row.blocked_reason =
                                Some(task_core::WorkUnitBlockedReason::Decision);
                            wu_reason = "decision";
                            // この leaf はまだ done ではないので、それを待つ unit は ready にしない。
                            all_new_rows.clear();
                            if matches!(trigger, Trigger::WorkerDone) {
                                trigger = Trigger::Continue {
                                    why: task_core::ContinueWhy::Advance,
                                };
                            }
                            outcome_str = format!(
                                "decision: WorkUnit {} は人の決定を待ちます（決定の要求 {} 件。ADR-0079 D7）",
                                wu.key, found.count
                            );
                        }
                        worker_decisions = Some(found);
                    }
                }
                wu_parked_on_cluster_jobs = wu_reason == "cluster_jobs";
                wu_update = Some((updated_row, wu_reason, all_new_rows));
            }
        } else if let Some(end) = run_end
            && end.saves_checkpoint()
        {
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
            // ADR-0072 D8: `tests_run`（最大 10 件）と `recent_activity`（最大 20 行）は、この run の
            // `WorkerProgress{kind: tool_use}` と、それに続く `tool_result` の組から作る。
            let activity: Vec<crate::checkpoint::ToolActivity> = events_so_far
                .iter()
                .filter_map(|(_, ev)| match ev {
                    Event::WorkerProgress {
                        run_id: r,
                        kind: Some(task_core::ProgressKind::ToolUse),
                        tool,
                        summary,
                        ..
                    } if r == &run_id => Some(crate::checkpoint::ToolActivity::Use {
                        tool: tool.clone(),
                        summary: summary.clone(),
                    }),
                    Event::WorkerProgress {
                        run_id: r,
                        kind: Some(task_core::ProgressKind::ToolResult),
                        error,
                        ..
                    } if r == &run_id => {
                        Some(crate::checkpoint::ToolActivity::Result { error: *error })
                    }
                    _ => None,
                })
                .collect();
            let mechanical = crate::checkpoint::gather(cwd, branch, &activity);
            // D9: 優先順位は `result.json.yield` > `checkpoint.json` > mechanical。
            let worker_checkpoint = yield_checkpoint_json
                .as_ref()
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .or_else(|| {
                    artifacts_dir
                        .as_deref()
                        .and_then(crate::checkpoint::read_worker_checkpoint)
                });
            let checkpoint_end = end
                .as_checkpoint_end()
                .unwrap_or(task_core::CheckpointEnd::BudgetExhausted);
            let ctx = task_core::CheckpointContext {
                task_id: task_id.to_string(),
                work_unit: None,
                run_id: run_id.clone(),
                run_seq,
                end: checkpoint_end,
                created_at: rfc3339(OffsetDateTime::now_utc()),
            };
            let checkpoint = task_core::merge_checkpoint(worker_checkpoint, mechanical, ctx);

            if end == task_core::RunEnd::Waiting {
                // ADR-0090 D1: クラスタ job の wait。checkpoint は残すが continuation の上限・進捗なしには数えない
                // （trigger は `ClusterJobWait` のまま）。
                checkpoint_for_index = Some(checkpoint.clone());
                checkpoint_event = Some(Event::CheckpointSaved {
                    run_id: run_id.clone(),
                    work_unit_id: None,
                    checkpoint: Box::new(checkpoint),
                });
            } else if self.config.execution.continuation {
                let continuations_so_far = consecutive_continuations(&events_so_far);
                let prev_checkpoint =
                    task_ops::derive::latest_progress_checkpoint(&events_so_far, None);
                let progressed =
                    task_core::checkpoint_shows_progress(prev_checkpoint.as_ref(), &checkpoint);
                let no_progress = if progressed {
                    0
                } else {
                    no_progress_streak(&events_so_far, None) + 1
                };
                checkpoint_for_index = Some(checkpoint.clone());
                checkpoint_event = Some(Event::CheckpointSaved {
                    run_id: run_id.clone(),
                    work_unit_id: None,
                    checkpoint: Box::new(checkpoint),
                });
                if continuations_so_far >= self.config.execution.max_continuations_per_work_unit
                    || no_progress >= self.config.execution.no_progress_limit
                {
                    // ADR-0072 D18: 上限到達・進捗なしは失敗にせず、人に聞く（blocked）。
                    trigger = Trigger::WorkerQuestion;
                    outcome_str = format!(
                        "question: 実行が進みません（continuation {continuations_so_far} 回 / 進捗なし {no_progress} 回）。予算を増やして続ける／分割し直す（replan）／中止のいずれかを選んでください。"
                    );
                } else {
                    trigger = Trigger::Continue {
                        why: task_core::ContinueWhy::Continue,
                    };
                    outcome_str = format!(
                        "continue: {} の続き（Run #{}）",
                        describe_run_end(end),
                        run_seq + 1
                    );
                }
            } else {
                // ADR-0072 §6 (f): `[execution] continuation = false` なら従来どおり
                // `WorkerError{retryable:true}` に戻す（checkpoint も保存しない）。
                trigger = Trigger::WorkerError { retryable: true };
                outcome_str = format!(
                    "error(retryable=true): {} (continuation disabled)",
                    describe_run_end(end)
                );
            }
        }
        // ADR-0079 D7（Phase R3a）: 木の節点の atomic の run（WU を持たない）が `result.json` の `decisions` を
        // 書いた。記録し、`needed_before: self` なら最終レビューに進めず `ready` に戻して（`advance`）、答えが
        // 出るまで run を起こさない（`decision_self_hold`。答えは次の run の前置きの `answers` に入る）。
        if current_wu.is_none()
            && matches!(trigger, Trigger::WorkerDone)
            && matches!(run_end, Some(task_core::RunEnd::Completed))
        {
            let artifacts_dir = self.task_dir(&task).map(|d| self.artifacts_dir(&task, &d));
            if let Some(found) =
                self.worker_decisions(&task, None, artifacts_dir.as_deref(), &run_id)?
            {
                if found.self_hold {
                    trigger = Trigger::Continue {
                        why: task_core::ContinueWhy::Advance,
                    };
                    outcome_str = format!(
                        "decision: 人の決定を待ちます（決定の要求 {} 件。ADR-0079 D7）",
                        found.count
                    );
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
        // ADR-0140 D2: WU の継続 session を使った run は、その usage を `approx_tokens` に積む
        // （判断表 #10 の rollover の材料）。継続 session を持たない run（`runs.session_id` が無い）は何もしない。
        if let Ok(Some(row)) = self.store.run_index_get(&run_id)
            && row.session_id.is_some()
            && let (Some(wu_id), Some(adapter)) =
                (row.work_unit_id.as_deref(), row.adapter.as_deref())
        {
            let tokens = usage
                .as_ref()
                .map(|u| u.input_tokens.unwrap_or(0) + u.output_tokens.unwrap_or(0))
                .unwrap_or(0);
            if let Err(e) = self.store.work_unit_session_touch(
                task_id,
                Some(wu_id),
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
        let mut questions: Vec<String> = Vec::new();
        if matches!(trigger, Trigger::WorkerQuestion) {
            questions.push(
                outcome_str
                    .strip_prefix("question: ")
                    .unwrap_or(outcome_str.as_str())
                    .to_string(),
            );
        }
        if !cross_department.is_empty() {
            if !matches!(trigger, Trigger::WorkerQuestion) {
                trigger = Trigger::WorkerQuestion;
                outcome_str = format!("question: {}", cross_department.join("\n"));
                subject = ReviewSubject::default();
            }
            questions.extend(cross_department.iter().cloned());
        }
        // ADR-0024 D4 / S10: プール経由の run の失敗は、原因がアカウント側（throttled/auth_failed/exhausted）なら
        // アカウントを cooldown にしプロバイダは cooldown にしない。`Spawn` 失敗（起動できない）はアカウントの
        // 責任ではないので、通常どおりプロバイダを cooldown にする（`failure_reason == Some("spawn")`）。
        let account_at_fault = account.is_some() && failure_reason != Some("spawn");
        let policy_outcome = if account_at_fault {
            ProviderOutcome::Ok
        } else {
            provider_outcome.clone()
        };
        self.policy.report(provider.clone(), &policy_outcome);

        // ADR-0061（Phase 104）: `retries` はこの run が始まった時点でタスクが既に消費していた試行回数
        // （= 遷移前の `task.attempts`）。
        let metrics = run_since.map(|since| task_core::RunMetrics {
            wall_ms: wall_ms_since(since),
            retries: task.attempts,
            peak_context_tokens: None,
            turns: None,
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
            if let Err(e) = self.store.work_unit_transition(
                task_id,
                updated_wu.clone(),
                Event::WorkUnitTransitioned {
                    work_unit_id: updated_wu.id.clone(),
                    key: updated_wu.key.clone(),
                    from,
                    to: updated_wu.status,
                    reason: reason.to_string(),
                    run_id: Some(run_id.clone()),
                },
            ) {
                tracing::warn!(%task_id, %run_id, error = %e, "failed to record the work unit transition");
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
        // ADR-0074 D1.6（Phase F2b）: v2 の WU の run。兄弟の WU（または統合）が走っていれば Task は
        // 遷移させない。in-flight が 0 になったら工程の状態（question → 失敗 → 起こせる WU → 統合）で決める。
        let mut v2_settle: Option<crate::execution_scheduler::PhaseSettle> = None;
        if let Some(wu) = &current_wu
            && wu.phase.is_some()
        {
            use crate::execution_scheduler::PhaseSettle;
            let units_now = self.store.work_units_for(task_id)?;
            let settle = crate::execution_scheduler::settle_phase(&units_now);
            match &settle {
                PhaseSettle::Wait | PhaseSettle::Integrate(_) => {
                    // 質問は in-flight が 0 になってから（承認の行もそのときに作る）。
                    questions.clear();
                }
                PhaseSettle::Advance => {
                    if matches!(trigger, Trigger::WorkerDone) {
                        trigger = Trigger::Continue {
                            why: task_core::ContinueWhy::Advance,
                        };
                    }
                }
                PhaseSettle::Question(id) | PhaseSettle::Failure(id) if id != &wu.id => {
                    let (t, _outcome, q) =
                        self.deferred_work_unit_trigger(task_id, &units_now, id)?;
                    trigger = t;
                    questions = q;
                }
                PhaseSettle::AllDone => {
                    trigger = Trigger::WorkerDone;
                }
                _ => {}
            }
            v2_settle = Some(settle);
        }
        let finished = Event::WorkerFinished {
            run_id: run_id.clone(),
            outcome: outcome_str.clone(),
            usage,
            role: None,
            metrics,
            end: run_end,
        };
        let mut events = vec![finished];
        if let Some(event) = integration_exhausted_event {
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
        // ADR-0074 D4（Phase F3 quota）: この run の quota 消費を見積もる（`WorkerFinished` と同じ
        // トランザクションで残す。重なった run のグループがこれで閉じれば、他のメンバー分は
        // `resolve_quota_estimate` の中で別タスクへ直接書く）。
        {
            let model_for_quota = self.started_model_of(task_id, &run_id);
            let quota_event = self.resolve_quota_estimate(
                task_id,
                &run_id,
                current_wu.as_ref().map(|wu| wu.id.clone()),
                account.as_deref(),
                account_adapter,
                &provider,
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
                // ADR-0013 D9 / S10: プールを使わない、または Spawn 失敗（アカウント非依存）はプロバイダの
                // cooldown として、遷移と同じトランザクションで記録する。
                _ => {
                    if let Some(ev) =
                        self.provider_throttled_event(&provider, &provider_outcome, reason)
                    {
                        events.push(ev);
                    }
                }
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
        if let Some(settle) = &v2_settle
            && matches!(
                settle,
                crate::execution_scheduler::PhaseSettle::Wait
                    | crate::execution_scheduler::PhaseSettle::Integrate(_)
            )
        {
            for ev in &events {
                self.store.append_event(task_id, ev)?;
            }
            tracing::info!(%task_id, %run_id, settle = ?settle, outcome = %outcome_str, "work unit finished (task stays running)");
            if let crate::execution_scheduler::PhaseSettle::Integrate(id) = settle {
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
                if let Some(n) = infra_requeue_n {
                    let until = OffsetDateTime::now_utc() + infra_backoff_delay(n);
                    tracing::warn!(%task_id, %run_id, attempt = n, until = %until, "infra failure; requeued with backoff (attempts not consumed; ADR-0070 D3)");
                    self.infra_backoff.insert(task_id, until);
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
                    self.pending_subjects.insert(task_id, subject);
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
        let finished = |outcome: String| Event::WorkerFinished {
            run_id: run_id.to_string(),
            outcome,
            usage: None,
            role: None,
            metrics: None,
            end: Some(task_core::RunEnd::HarnessError {
                class: task_core::HarnessErrorClass::Infra,
            }),
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
            let exhausted = infra_n > self.config.max_infra_retries;
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
        let (trigger, outcome) = if infra_n <= self.config.max_infra_retries {
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
        if matches!(trigger, Trigger::InfraRequeue) {
            self.infra_backoff
                .insert(task_id, now + infra_backoff_delay(infra_n));
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
        if self.running.values().any(|e| e.run_id == run_id) || self.checking.contains_key(run_id) {
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

/// ADR-0033 D4（Phase 24 / Phase 27）: この run の途中で「部をまたぐ委譲」を止めたときに `StoreSink` が
/// 残した質問（1 件の部またぎにつき 1 件。同じ文面は 1 回だけ）。
pub(super) fn cross_department_questions_of(events: &[(u64, Event)], run_id: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (_, e) in events {
        if let Event::QuestionRaised { run_id: r, text } = e
            && r == run_id
            && !out.contains(text)
        {
            out.push(text.clone());
        }
    }
    out
}

/// ADR-0072 D7（Phase E1）: 「二重の安全網」。構造化した `Terminal::BudgetExhausted` を返さない
/// 古い経路・アダプタのために、`Terminal::Error{retryable:true}` のメッセージを字句判定する
/// （`task_core::is_budget_outcome` と同じ語彙。分類できなければ `None`）。
pub(super) fn classify_budget_kind_from_text(message: &str) -> Option<task_core::BudgetKind> {
    let m = message.to_lowercase();
    if task_core::looks_like_context_exceeded(&m) {
        Some(task_core::BudgetKind::Context)
    } else if m.contains("max_turns") || m.contains("max turns") || m.contains("turn limit") {
        Some(task_core::BudgetKind::Turns)
    } else if m.contains("wall-clock") || m.contains("wall clock") || m.contains("budget") {
        Some(task_core::BudgetKind::WallClock)
    } else {
        None
    }
}

/// ADR-0072 D9（Phase E1）: `WorkerFinished.outcome` に載せる、人が読む 1 行の終わり方の説明。
pub(super) fn describe_run_end(end: task_core::RunEnd) -> String {
    match end {
        task_core::RunEnd::Completed => "completed".to_string(),
        task_core::RunEnd::Yielded => "yielded".to_string(),
        task_core::RunEnd::BudgetExhausted { kind } => {
            let k = match kind {
                task_core::BudgetKind::Turns => "turns",
                task_core::BudgetKind::WallClock => "wall_clock",
                task_core::BudgetKind::Context => "context",
            };
            format!("budget_exhausted({k})")
        }
        task_core::RunEnd::Question => "question".to_string(),
        task_core::RunEnd::Failed { retryable } => format!("failed(retryable={retryable})"),
        task_core::RunEnd::HarnessError { class } => format!("harness_error({class:?})"),
        task_core::RunEnd::Cancelled => "cancelled".to_string(),
        task_core::RunEnd::Waiting => "waiting(cluster_jobs)".to_string(),
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
            } => Some(describe_run_end(*e)),
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
            prior_runs.push(format!("Run #{seq} {}", describe_run_end(*e)));
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
