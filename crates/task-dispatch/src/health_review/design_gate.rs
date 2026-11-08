//! ADR 2026-11-07-health-review-integration D3.3・D3.5・D3.6: 計画の採用前の risk 判定と、高 risk の計画だけの設計
//! review の起動（Dispatcher の状態を使う配線）。
//!
//! 実行中の run（`running`）と完了の channel（`Completion`）は `dispatcher` の private な状態なので、この file は
//! `dispatcher` の子 module として `#[path]` で読む（`dispatcher.rs` の `mod design_gate`）。file を
//! `health_review/` に置くのは、`dispatcher/` が module 全体で repo health の `size.responsibility-growth` の閾値を
//! 超えているため。純粋な判定は [`crate::health_review::design`] にある。LLM は呼ばない（I4）。
//!
//! 流れ（planner の計画 1 つにつき）:
//! 1. [`Dispatcher::design_brief_gate`]: 検証を通った planner の計画が高 risk の class の path を書くのに brief が
//!    無ければ、`DesignBriefMissing` を既存の検証失敗と同じ planner の再試行に回す（D3.3）。
//! 2. [`Dispatcher::design_gate`]（`tree_plan_gate` の後、`adopt_plan_with_children`・`replan` の前）: repo ごとの
//!    判定の最も高い level が `none`・`low` なら今までどおり採用する（追加の run なし。D3.6）。`high`・`unknown` なら
//!    `Event::DesignReviewRequested` を追記し、計画を採用せずに task を `ready` に戻す（D3.5 1.）。
//! 3. [`Dispatcher::design_review_hold`]（`dispatch_one`）: 待っている設計 review がある task には planner も WU も
//!    起こさない。まだ reviewer run を起こしていなければ `pick_reviewer` で選んで起こす（D3.5 2.）。待ちは events
//!    だけで決まるので、デーモンの再起動の後も同じ判定になる（I9）。
//! 4. [`Dispatcher::on_design_review_finished`]: 設計 review の run の終わりは `Completion::DesignReview` で届き、
//!    `WorkerFinished`（role reviewer）・quota・runs 索引だけを記録する。task の final review の判定
//!    （`on_review_finished`・`review_verdict`）には流さない。`review.json` の `design` の読み込みと計画の採用・
//!    replan は後続の design-verdict。

use super::*;

use crate::health_review::design::{
    brief_required_classes, design_review_started, merge_risk, pending_design_review, plan_summary,
    write_paths_by_repo,
};
use crate::health_review::policy::{REVIEW_POLICY_TIMEOUT, load_review_policy};
use task_core::ExecutionPlanSpec;
use task_core::design_review::ReviewPolicyState;
use task_worker::protocol::DesignReviewRequest;

/// 設計 review の reviewer run の終わり（`Completion::DesignReview`）。
pub(super) struct DesignReviewEnd {
    pub(super) task_id: TaskId,
    pub(super) run_id: String,
    pub(super) provider: ProviderId,
    pub(super) result: Result<RunOutcome, AdapterError>,
}

/// [`Dispatcher::design_brief_gate`] が読んだ repo ごとの review 方針（採用前の判定で使い回す）と planner run の id。
pub(super) struct DesignPolicies {
    run_id: String,
    states: Vec<(String, ReviewPolicyState)>,
}

impl Dispatcher {
    /// task の repo ごとの review 方針（`names` の repo だけ。名前 `""` は案件の repo の無い作業場所）。target は
    /// review 前の同期と同じ `review_target_ref`、案件の repo の無い task は worktree の base。remote は
    /// `Unavailable`、git の repo が無ければ `Absent`（`policy::task_review_policy` と同じ規則を repo ごとに当てる）。
    fn repo_review_policies(
        &self,
        task: &Task,
        names: &[String],
    ) -> Vec<(String, ReviewPolicyState)> {
        let state_of = |name: &String| -> ReviewPolicyState {
            if matches!(task.workspace, WorkspaceSpec::Remote { .. }) {
                return ReviewPolicyState::Unavailable("remote workspace".into());
            }
            let Some(workspaces) = self.task_workspaces_for(task) else {
                return if task.repos.is_empty() {
                    ReviewPolicyState::Absent
                } else {
                    ReviewPolicyState::Unavailable("作業場所の repo を解決できない".into())
                };
            };
            let repo = workspaces
                .repos
                .into_iter()
                .enumerate()
                .find(|(i, r)| &r.name == name || (name.is_empty() && *i == 0))
                .map(|(_, r)| r);
            let Some(worktree) = repo.and_then(|r| r.worktree) else {
                return ReviewPolicyState::Absent;
            };
            let target = match task.repos.iter().find(|r| &r.name == name) {
                Some(reference) => {
                    match self.review_target_ref(task, reference.repo_id, &worktree.repo) {
                        Ok(Some(target)) => target,
                        Ok(None) => {
                            return ReviewPolicyState::Unavailable(format!(
                                "repo {name} の target branch を解決できない"
                            ));
                        }
                        Err(e) => {
                            return ReviewPolicyState::Unavailable(format!(
                                "repo {name} の target branch を読めない: {e}"
                            ));
                        }
                    }
                }
                None => worktree.base.sha.clone(),
            };
            load_review_policy(&worktree.repo, &target, REVIEW_POLICY_TIMEOUT)
        };
        names.iter().map(|n| (n.clone(), state_of(n))).collect()
    }

    /// 計画が書く repo（と task の先頭の repo）の review 方針。
    fn plan_review_policies(
        &self,
        task: &Task,
        spec: &ExecutionPlanSpec,
    ) -> Vec<(String, ReviewPolicyState)> {
        let names: Vec<String> = write_paths_by_repo(spec, &primary_repo(task))
            .into_keys()
            .collect();
        self.repo_review_policies(task, &names)
    }

    /// D3.3: 検証を通った planner の計画が高 risk の class の path を書き、brief が無ければ `DesignBriefMissing` を
    /// 検証失敗の理由にする（既存の planner の再試行・諦めの経路）。通った計画には読んだ方針を添えて返す。
    pub(super) fn design_brief_gate(
        &self,
        task: &Task,
        run_id: &str,
        validation: Result<task_core::execution_plan::ValidatedPlan, String>,
    ) -> Result<(task_core::execution_plan::ValidatedPlan, DesignPolicies), String> {
        let validated = validation?;
        let states = self.plan_review_policies(task, &validated.spec);
        let by_repo = write_paths_by_repo(&validated.spec, &primary_repo(task));
        let classes = brief_required_classes(&states, &by_repo);
        task_core::execution_plan::require_design_brief(
            &validated.spec,
            task_core::PlanOrigin::Planner,
            &classes,
        )
        .map_err(|e| task_ops::execution::describe_validation_errors(&[e]))?;
        let run_id = run_id.to_string();
        Ok((validated, DesignPolicies { run_id, states }))
    }

    /// D3.5・D3.6: 採用の直前の risk 判定。`none`・`low` は `false` を返し、呼び出し側が今までどおり採用する（追加の
    /// run なし。level と理由は log に残す）。`high`・`unknown` は `events`（`WorkerFinished` と quota）と
    /// `DesignReviewRequested` を追記して task を `ready` に戻し（計画は採用しない）、`true` を返す。設計 review の
    /// run は次の dispatch（[`Self::design_review_hold`]）が起こす。
    pub(super) fn design_gate(
        &self,
        task: &Task,
        gate: DesignPolicies,
        spec: &ExecutionPlanSpec,
        events: [&Event; 2],
    ) -> Result<bool, DispatchError> {
        let DesignPolicies { run_id, states } = gate;
        let by_repo = write_paths_by_repo(spec, &primary_repo(task));
        let (risk, policy_sha256) = merge_risk(&states, &by_repo, spec);
        if !risk.level.needs_design_review() {
            tracing::info!(task_id = %task.id, %run_id, level = ?risk.level, reasons = ?risk.reasons, "plan risk does not need a design review; adopting (ADR 2026-11-07-health-review-integration D3.6)");
            return Ok(false);
        }
        tracing::info!(task_id = %task.id, %run_id, level = ?risk.level, reasons = ?risk.reasons, "plan risk needs a design review before adoption (ADR 2026-11-07-health-review-integration D3.5)");
        let progress = Event::worker_progress(
            run_id.as_str(),
            format!(
                "計画の risk は {:?}（{}）。採用の前に設計 review を起こします。",
                risk.level,
                risk.reasons.join(", ")
            ),
        );
        let requested = Event::DesignReviewRequested {
            plan_sha256: task_core::execution_plan::plan_sha256(spec),
            plan: Box::new(spec.clone()),
            risk,
            policy_sha256,
        };
        let [finished, quota] = events;
        self.store.apply_transition_with_events(
            task.id,
            Trigger::Continue {
                why: task_core::ContinueWhy::Planned,
            },
            vec![finished.clone(), quota.clone(), progress, requested],
        )?;
        Ok(true)
    }

    /// I9・D3.5 2.: 待っている設計 review がある task には run を起こさない（`true`）。まだ reviewer run を起こして
    /// いなければ起こす（選べなければ次の tick にもう一度試す）。待っていなければ `false`。
    pub(super) fn design_review_hold(&mut self, task: &Task) -> Result<bool, DispatchError> {
        let events = self.store.events_for(task.id)?;
        let Some((seq, request)) = pending_design_review(&events) else {
            return Ok(false);
        };
        if design_review_started(&events, seq) {
            return Ok(true);
        }
        let Event::DesignReviewRequested {
            plan,
            risk,
            policy_sha256,
            ..
        } = request
        else {
            return Ok(true);
        };
        let policies = self.plan_review_policies(task, plan);
        let review_policy = policies.into_iter().find_map(|(_, state)| match state {
            ReviewPolicyState::Present(p)
                if Some(&p.definition_sha256) == policy_sha256.as_ref() =>
            {
                Some(p)
            }
            _ => None,
        });
        let request = DesignReviewRequest {
            brief: plan.design_brief.clone(),
            plan_summary: plan_summary(plan),
            risk: risk.clone(),
            review_policy,
        };
        self.start_design_review(task, request)?;
        Ok(true)
    }

    /// D3.5 2.: `pick_reviewer` で reviewer を選び、`run_reviewer_inner` と同じ形の reviewer run を起こす（新しい run
    /// 種別は作らない）。文脈は `ReviewRequest.design` だけで、task の review の文脈（summary・evidence・criteria）は
    /// 渡さない。run は `running` に task の run として入る（並列度・cancel・同じ task の二重 dispatch の抑止）。
    fn start_design_review(
        &mut self,
        task: &Task,
        request: DesignReviewRequest,
    ) -> Result<(), DispatchError> {
        let Some(dir) = self.task_dir(task) else {
            tracing::warn!(task_id = %task.id, "design review deferred: the task directory is unknown");
            return Ok(());
        };
        let subject = self
            .store
            .events_for(task.id)?
            .into_iter()
            .rev()
            .find_map(|(_, e)| match e {
                Event::WorkerStarted {
                    run_id,
                    role: Some(RunRole::Planner),
                    ..
                } => Some(run_id),
                _ => None,
            })
            .unwrap_or_default();
        let Some(reviewer) = self.pick_reviewer(task, &subject) else {
            tracing::debug!(task_id = %task.id, "design review deferred (no admissible reviewer route)");
            return Ok(());
        };
        let run = reviewer.run;
        let run_id = run.run_id.clone();
        let source = reviewer.pick.candidate.source.clone();
        let account = reviewer.pick.account.to_string();
        let model = reviewer.pick.candidate.model.to_string();
        self.store.append_event(
            task.id,
            &Event::WorkerStarted {
                run_id: run_id.clone(),
                adapter: run.adapter.id().to_string(),
                model: model.clone(),
                provider: Some(source.to_string()),
                account: Some(account.clone()),
                role: Some(RunRole::Reviewer),
                task_role: None,
            },
        )?;
        let seq = self
            .store
            .runs_for_task(task.id)
            .map(|rs| {
                rs.iter()
                    .filter(|r| r.role == task_core::RunIndexRole::Reviewer)
                    .count() as u32
                    + 1
            })
            .unwrap_or(1);
        let indexed = self.store.run_index_start(task_core::RunRow {
            run_id: run_id.clone(),
            task_id: task.id.to_string(),
            work_unit_id: None,
            role: task_core::RunIndexRole::Reviewer,
            seq,
            status: task_core::RunIndexStatus::Running,
            adapter: Some(run.adapter.id().to_string()),
            model: Some(model),
            account: Some(account.clone()),
            session_id: None,
            checkpoint: None,
            usage: None,
            metrics: None,
            started_at: rfc3339(OffsetDateTime::now_utc()),
            finished_at: None,
        });
        match indexed {
            Ok(()) => {
                if let Err(e) = self.record_route_selection(
                    task.id,
                    Some(&run_id),
                    &reviewer.route_input,
                    &Ok(reviewer.pick.clone()),
                    &reviewer.trace,
                ) {
                    tracing::warn!(task_id = %task.id, %run_id, error = %e, "failed to record the design reviewer route selection");
                }
            }
            Err(e) => {
                tracing::warn!(task_id = %task.id, %run_id, error = %e, "failed to record the design review run start in the runs index");
            }
        }
        self.quota_begin(Some(&account), reviewer.account_adapter, &run_id);
        let artifacts_dir = self.artifacts_dir(task, &dir);
        let subject_task = task.clone();
        let (task_id, end_run_id, provider) = (task.id, run_id.clone(), source.to_string());
        let tx = self.tx.clone();
        let handle = tokio::spawn(async move {
            let result =
                run_design_reviewer(&subject_task, &dir, &artifacts_dir, request, run).await;
            let _ = tx.send(Completion::DesignReview(Box::new(DesignReviewEnd {
                task_id,
                run_id: end_run_id,
                provider,
                result,
            })));
        });
        self.running.insert(
            RunKey {
                task: task.id,
                work_unit: None,
            },
            RunEntry {
                run_id,
                provider: source.to_string(),
                handle,
                since: OffsetDateTime::now_utc(),
                cluster: None,
                account: Some(account),
                account_adapter: reviewer.account_adapter,
                container: None,
                cos: false,
                route: Some((source, reviewer.pick.account)),
            },
        );
        Ok(())
    }

    /// 設計 review の run の終わり。`WorkerFinished`（role reviewer）・quota・runs 索引だけを記録し、task の状態は
    /// 変えない。task の final review の判定（`on_review_finished`・`review_verdict`）には流さない。
    pub(super) fn on_design_review_finished(
        &mut self,
        end: DesignReviewEnd,
    ) -> Result<(), DispatchError> {
        let DesignReviewEnd {
            task_id,
            run_id,
            provider,
            result,
        } = end;
        let (account, account_adapter, since) = self
            .take_running_by_run_id(&run_id)
            .map(|e| (e.account, e.account_adapter, Some(e.since)))
            .unwrap_or((None, None, None));
        let (run_end, outcome, usage) = run_end_of(&result, "design review");
        let model = self.started_model_of(task_id, &run_id);
        let quota = self.resolve_quota_estimate(
            task_id,
            &run_id,
            None,
            account.as_deref(),
            account_adapter,
            &provider,
            &model,
            usage.as_ref(),
        );
        let metrics = since.map(|since| task_core::RunMetrics {
            wall_ms: wall_ms_since(since),
            retries: 0,
            peak_context_tokens: None,
            turns: None,
        });
        tracing::info!(%task_id, %run_id, %outcome, "design review run finished (the verdict is read by the design-verdict step, not by the final review)");
        self.store.append_event(
            task_id,
            &Event::WorkerFinished {
                run_id: run_id.clone(),
                outcome,
                usage,
                role: Some(RunRole::Reviewer),
                metrics,
                end: run_end,
            },
        )?;
        self.store.append_event(task_id, &quota)?;
        let status = run_end
            .map(task_core::RunIndexStatus::from_run_end)
            .unwrap_or(task_core::RunIndexStatus::HarnessError);
        if let Err(e) = self.store.run_index_finish(
            &run_id,
            status,
            None,
            usage,
            metrics,
            OffsetDateTime::now_utc(),
        ) {
            tracing::warn!(%task_id, %run_id, error = %e, "failed to finish the runs index row for the design review run");
        }
        Ok(())
    }
}

/// `run_reviewer_inner` と同じ形で reviewer の adapter を起こす（合成の `Review` task・対象 task の dir・成果物 dir・
/// run 専用の `TMPDIR`）。文脈は設計 review の依頼（`ReviewRequest.design`）だけ。前の `review.json` は消してから起こす。
async fn run_design_reviewer(
    task: &Task,
    workspace_dir: &Path,
    artifacts_dir: &Path,
    request: DesignReviewRequest,
    run: crate::review::ReviewerRun,
) -> Result<RunOutcome, AdapterError> {
    let _ = std::fs::remove_file(artifacts_dir.join(crate::review::REVIEW_FILE_NAME));
    let mut review_task = crate::review::synthetic_review_task(task, &run.run_id, &run.hint);
    if let Some(node) = &run.node {
        review_task.assignee = Some(node.id.clone());
    }
    let req = RunRequest {
        llm_route: Some(run.llm_route.clone()),
        cargo_target_dir: run.cargo_target_dir.clone(),
        protocol: task_worker::PROTOCOL_VERSION,
        task: review_task,
        workspace: workspace_dir.to_path_buf(),
        work_dir: None,
        artifacts_dir: artifacts_dir.to_path_buf(),
        context: RunContext {
            node: run.node.clone(),
            profile: run.profile.clone(),
            skills: run.skills.clone(),
            review: Some(task_worker::ReviewRequest {
                design: Some(Box::new(request)),
                ..Default::default()
            }),
            subject_genre: run.subject_genre.clone(),
            session: run.session.clone(),
            session_diff: run.session_diff.clone(),
            ..RunContext::default()
        },
    };
    let (adapter, _run_tmp) =
        crate::review::with_run_tmpdir(&run.adapter, workspace_dir, &run.run_id);
    run.sink
        .progress(&format!("design review started (adapter={})", adapter.id()));
    adapter
        .run(req, &run.run_id, run.limits, run.sink.as_ref())
        .await
}

/// task の先頭の repo の名前（worker の cwd。案件の repo の無い task は `""`）。
fn primary_repo(task: &Task) -> String {
    task.repos
        .first()
        .map(|r| r.name.clone())
        .unwrap_or_default()
}

/// planner run と設計 review の run の終わり方（`RunEnd`・`WorkerFinished.outcome` の文・usage）。どちらの run も
/// 続き（continuation）にも job の待ちにもならないので、yield と job の待ち（ADR-0090）は不正な試行として扱う。
/// `role` は文に入る run の名前（`planner` など）。
pub(super) fn run_end_of(
    result: &Result<RunOutcome, AdapterError>,
    role: &str,
) -> (Option<task_core::RunEnd>, String, Option<task_core::Usage>) {
    use task_core::RunEnd;
    let Ok(RunOutcome { terminal, .. }) = result else {
        let e = result
            .as_ref()
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        return (None, format!("infra error: {e}"), None);
    };
    match terminal {
        Terminal::Done { summary, usage, .. } => {
            (Some(RunEnd::Completed), format!("done: {summary}"), *usage)
        }
        Terminal::Question { text, .. } => {
            (Some(RunEnd::Question), format!("question: {text}"), None)
        }
        Terminal::Error {
            message, retryable, ..
        } => (
            Some(RunEnd::Failed {
                retryable: *retryable,
            }),
            format!("error(retryable={retryable}): {message}"),
            None,
        ),
        Terminal::Yielded { usage, .. } => (
            Some(RunEnd::Yielded),
            format!("yielded ({role} runs are not continued; treated as an invalid attempt)"),
            *usage,
        ),
        Terminal::Waiting { usage, .. } => (
            Some(RunEnd::Failed { retryable: true }),
            format!(
                "error(retryable=true): {role} runs cannot wait for cluster jobs; treated as an invalid attempt"
            ),
            *usage,
        ),
        Terminal::BudgetExhausted {
            kind,
            message,
            usage,
        } => (
            Some(RunEnd::BudgetExhausted { kind: *kind }),
            format!("budget_exhausted({kind:?}): {message}"),
            *usage,
        ),
    }
}
