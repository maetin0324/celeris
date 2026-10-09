//! review run の起動（`spawn_review`・`pick_reviewer`・最終レビュー）。per-task file lock は verdict の保存まで持ち続ける。ADR-0082 の L2。

use super::*;

/// ADR-0118 D4 付記: stale の自動再同期が上限に達し、reviewing のまま止めたことを示す `WorkerProgress` の接頭辞。
pub(crate) const TARGET_RESYNC_HALTED_PREFIX: &str = "review target resync halted: ";

/// ADR-0118 D4 付記: 直近の遷移（自動の再レビューの遷移を除く）以後の stale（`ReviewTargetAdvanced`）の数、
/// その最後の記録、上限で止めた印の有無。自動の再レビュー（root delivery の `request_rereview`）は
/// `Transitioned{rereview}` の直後に同じ transaction で `ReviewTargetAdvanced` を残すので数え直さない。
fn pre_review_stale_state(events: &[Event]) -> (u32, Option<&Event>, bool) {
    let mut count = 0;
    let mut last = None;
    let mut halted = false;
    for (i, event) in events.iter().enumerate() {
        match event {
            Event::ReviewTargetAdvanced { .. } => {
                count += 1;
                last = Some(event);
            }
            Event::Transitioned { .. }
                if !matches!(events.get(i + 1), Some(Event::ReviewTargetAdvanced { .. })) =>
            {
                count = 0;
                last = None;
                halted = false;
            }
            Event::WorkerProgress { msg, .. } if msg.starts_with(TARGET_RESYNC_HALTED_PREFIX) => {
                halted = true;
            }
            _ => {}
        }
    }
    (count, last, halted)
}

/// ADR-0120 付記（fallback の解除）: fallback を解いて同期へ戻ったことを示す `WorkerProgress` の接頭辞。
pub(crate) const FALLBACK_RELEASED_PREFIX: &str = "integration repair fallback released for ";

/// ADR-0120 付記（fallback の解除）: `repo_id` の最新の IntegrationRepair 記録が
/// `IntegrationRepairExhausted{fallback:true}` なら、そのとき review した未同期 HEAD
/// （rollback したならその SHA、しなければ `before_sha`）。新しい `IntegrationRepairScheduled` が
/// 後にあれば `None`。
fn fallback_head(events: &[(u64, Event)], repo_id: task_core::RepoId) -> Option<&str> {
    events.iter().rev().find_map(|(_, e)| match e {
        Event::IntegrationRepairExhausted {
            repo_id: exhausted_repo,
            fallback,
            before_sha,
            rollback_to_sha,
            ..
        } if *exhausted_repo == repo_id => {
            Some(fallback.then(|| rollback_to_sha.as_deref().unwrap_or(before_sha.as_str())))
        }
        Event::IntegrationRepairScheduled {
            repo_id: scheduled_repo,
            ..
        } if *scheduled_repo == repo_id => Some(None),
        _ => None,
    })?
}

struct ReviewSync {
    repo_id: task_core::RepoId,
    target_ref: String,
    target_sha: String,
    before_sha: String,
    reviewed_sha: String,
}

/// An integration repair is resolved only after its WU is done and a new target
/// snapshot has been recorded. Looking at terminal events also makes a retried
/// review entry idempotent after a daemon restart.
fn pending_integration_repair(
    events: &[(u64, Event)],
    units: &[task_core::WorkUnitRow],
    repo_id: task_core::RepoId,
) -> Option<(String, u32)> {
    let mut closed = std::collections::HashSet::new();
    for (_, event) in events.iter().rev() {
        match event {
            Event::IntegrationRepairResolved { work_unit_id, .. } => {
                closed.insert(work_unit_id.clone());
            }
            Event::IntegrationRepairExhausted {
                work_unit_id: Some(work_unit_id),
                ..
            } => {
                closed.insert(work_unit_id.clone());
            }
            Event::IntegrationRepairExhausted {
                work_unit_id: None,
                repo_id: exhausted_repo,
                ..
            } if *exhausted_repo == repo_id => return None,
            Event::IntegrationRepairScheduled {
                work_unit_id,
                repo_id: scheduled_repo,
                attempt,
                ..
            } if *scheduled_repo == repo_id && !closed.contains(work_unit_id) => {
                return units
                    .iter()
                    .find(|unit| {
                        unit.id == *work_unit_id && unit.status == task_core::WorkUnitStatus::Done
                    })
                    .map(|_| (work_unit_id.clone(), *attempt));
            }
            _ => {}
        }
    }
    None
}

impl Dispatcher {
    /// review 前の target 同期（ADR-0118）を行うか。本番では常に行う。試験だけが切れる
    /// （`test_skip_pre_review_sync`、phase_effect_ab::review_sync の off）。
    fn pre_review_sync_enabled(&self) -> bool {
        #[cfg(test)]
        if self.test_skip_pre_review_sync {
            return false;
        }
        true
    }

    /// ADR-0074「F5-fix8 実装時の明確化」: `ready` の Task の、仕事の残っていない計画を最終レビューに出す
    /// （`Trigger::PlanComplete`。run は起こさない）。レビューの主題は完了した WU の要約（`finish_phase_integration`
    /// と同じ）の前に、今の版の計画の `rationale`（replan で何も足さなかった理由など）を置く。
    pub(super) fn start_final_review_from_ready(
        &mut self,
        task: &Task,
    ) -> Result<(), DispatchError> {
        let task_id = task.id;
        let units = self.store.work_units_for(task_id)?;
        let mut summary = self.plan_summary(&units);
        if let Some(active) = self.store.execution_plan_active(task_id)?
            && !active.spec.rationale.trim().is_empty()
        {
            let head = format!(
                "計画 v{}（仕事の残っていない版）: {}",
                active.version,
                active.spec.rationale.trim()
            );
            summary = if summary.is_empty() {
                head
            } else {
                format!("{head}\n{summary}")
            };
        }
        let subject = ReviewSubject {
            summary,
            evidence: Vec::new(),
        };
        let run_id = units
            .iter()
            .filter(|u| u.kind != task_core::WorkUnitKind::Integrate)
            .filter_map(|u| u.last_run_id.clone())
            .max()
            .unwrap_or_default();
        match self
            .store
            .apply_transition_with_events(task_id, Trigger::PlanComplete, Vec::new())
        {
            Ok(outcome) => {
                tracing::info!(%task_id, next = ?outcome.next, "the active plan has no work left and has not been reviewed yet; sending it to the final review (ADR-0074 F5-fix8)");
                if outcome.next == Status::Reviewing
                    && !self.spawn_review(task_id, run_id, &subject)?
                {
                    self.pending_subjects.insert(task_id, subject);
                }
                Ok(())
            }
            Err(StoreError::InvalidTransition(e)) => {
                tracing::warn!(%task_id, error = %e, "could not send a finished plan to the final review");
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    /// ADR-0117 D1: 対象 task とその祖先（木の親、無ければ `parent_id`）で回答済みの決定と、
    /// それらの task の `question` への回答を集める。決定は `answered_at` の昇順、回答は root 側から順に並べる。
    /// 兄弟の決定は混ぜない。`withdrawn` と未回答は含めない。
    pub(super) fn review_human_inputs(
        &self,
        task: &Task,
    ) -> Result<(Vec<task_worker::protocol::ReviewDecision>, Vec<Answer>), DispatchError> {
        // 対象 task から root へ（循環・深すぎる木に備えて上限を置く）。
        let mut chain = vec![task.id];
        let mut cursor = task_core::tree::tree_parent(task).or(task.parent_id);
        while let Some(id) = cursor {
            if chain.contains(&id) || chain.len() >= 32 {
                break;
            }
            chain.push(id);
            cursor = match self.store.get(id)? {
                Some(t) => task_core::tree::tree_parent(&t).or(t.parent_id),
                None => None,
            };
        }
        let mut rows: Vec<task_core::decision::DecisionRow> = self
            .store
            .decisions_list(Some(
                task.tree
                    .as_ref()
                    .map(|t| t.root_id)
                    .unwrap_or(chain[chain.len() - 1]),
            ))?
            .into_iter()
            .filter(|r| {
                r.status == task_core::decision::DecisionStatus::Answered
                    && chain.contains(&r.task_id)
            })
            .collect();
        rows.sort_by(|a, b| a.answered_at.cmp(&b.answered_at));
        let decisions = rows
            .iter()
            .filter_map(|r| {
                let answer = r.request.answer.as_ref()?;
                let option_label = r
                    .request
                    .options
                    .iter()
                    .find(|o| o.key == answer.option)
                    .map(|o| o.label.clone())
                    .unwrap_or_else(|| answer.option.clone());
                Some(task_worker::protocol::ReviewDecision {
                    task_id: r.task_id,
                    key: r.key.clone(),
                    question: r.request.question.clone(),
                    option: answer.option.clone(),
                    option_label,
                    note: answer.note.clone(),
                })
            })
            .collect();
        let mut answers = Vec::new();
        for id in chain.iter().rev() {
            answers.extend(to_answers(answers_from_events(
                &self.store.events_for(*id)?,
            )));
        }
        Ok((decisions, answers))
    }

    /// ADR 2026-10-07-worker-no-subagents-no-llm-cli D6: 対象 run（`subject_run_id`）で検出された subagent 道具・
    /// 別 LLM CLI / API の起動を events から集める（`WorkerPolicyViolation` のうち run_id が一致するものだけ。
    /// reviewer run 自身の検出や他の run のものは混ぜない）。順序は events の追記順。LLM は呼ばない。
    pub(super) fn review_policy_violations(
        &self,
        task_id: TaskId,
        subject_run_id: &str,
    ) -> Result<Vec<task_worker::protocol::ReviewPolicyViolation>, DispatchError> {
        Ok(self
            .store
            .events_for(task_id)?
            .into_iter()
            .filter_map(|(_, ev)| match ev {
                Event::WorkerPolicyViolation {
                    run_id,
                    kind,
                    tool,
                    matched,
                    command,
                } if run_id == subject_run_id => {
                    Some(task_worker::protocol::ReviewPolicyViolation {
                        kind: kind.as_str().to_string(),
                        tool,
                        matched,
                        command,
                    })
                }
                _ => None,
            })
            .collect())
    }

    /// レビューを開始する。`Reviewer` 条件があるのにプロバイダ／並列度の枠が無いときは `Ok(false)`
    /// （タスクは `reviewing` のまま。次 tick の `recover_reviews` が再試行する。ADR-0007 D5 1.）。
    pub(super) fn spawn_review(
        &mut self,
        task_id: TaskId,
        run_id: String,
        subject: &ReviewSubject,
    ) -> Result<bool, DispatchError> {
        if !self.accepting_new_work || !self.disk_ready {
            return Ok(false);
        }
        let Some(task) = self.store.get(task_id)? else {
            return Ok(true);
        };
        if task.status != Status::Reviewing {
            return Ok(true);
        }
        if self.held_by_disk_critical(&task) {
            return Ok(false);
        }
        let Some(dir) = self.task_dir(&task) else {
            tracing::warn!(%task_id, "cannot review task with remote workspace");
            return Ok(true);
        };
        // Old and new daemons can overlap during live handoff. Both command checks
        // and model review hold the same per-task lock through verdict persistence.
        let lock_dir = dir.join("runs");
        std::fs::create_dir_all(&lock_dir)
            .map_err(|e| StoreError::Invalid(format!("review lock directory: {e}")))?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_dir.join(format!(".review-{task_id}.lock")))
            .map_err(|e| StoreError::Invalid(format!("review lock: {e}")))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Ok(false),
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(StoreError::Invalid(format!("review lock: {e}")).into());
            }
        }
        let review_lock = Arc::new(lock);
        // The previous owner may have committed a verdict after our first read.
        let Some(mut task) = self.store.get(task_id)? else {
            return Ok(true);
        };
        if task.status != Status::Reviewing {
            return Ok(true);
        }
        if self.target_resync_exhausted(task_id, &run_id)? {
            return Ok(false);
        }

        let human = match self.resolve_human_approvals(&task)? {
            Some(h) => {
                self.awaiting_human.remove(&task_id);
                h
            }
            None => {
                self.awaiting_human.insert(task_id);
                tracing::debug!(%task_id, "review deferred (waiting for human approval)");
                return Ok(false);
            }
        };

        let reviewer = if needs_reviewer_run(&task) {
            match self.pick_reviewer(&task, &run_id) {
                Some(r) => Some(r),
                None => {
                    tracing::debug!(%task_id, "reviewer run deferred (no provider capacity)");
                    return Ok(false);
                }
            }
        } else {
            None
        };
        let provider = reviewer.as_ref().map(|(p, _, _)| p.clone());
        // ADR-0024 D2: `account_pool` で選んだアカウント（プールを使わない、または Reviewer run を起動しない場合は `None`）。
        let selected_account = reviewer.as_ref().and_then(|(_, a, _)| a.clone());
        let account = selected_account.as_ref().map(|(_, id)| id.clone());
        let account_adapter = selected_account.as_ref().map(|(a, _)| *a);
        // ADR-0014 D1: (provider, Reviewer run の id, adapter) — WorkerStarted の記録と in_flight に使う。
        let review_run = reviewer
            .as_ref()
            .map(|(p, _, r)| (p.clone(), r.run_id.clone(), r.adapter.id().to_string()));
        let reviewer_run = reviewer.map(|(_, _, r)| r);
        // The review lock covers the rebase and the checks that consume its result.
        // A deferred review reaches this point again and reads the then-current target.
        // Tree children historically commit their remaining run output on completion.
        // Move that same commit before review so the inspected commit is the one merged
        // into the parent stage; a failed commit remains dirty and stops in the sync.
        if task_core::tree::is_tree_child(&task) {
            let _ = self.commit_child_branch(&task);
        }
        let mut synced = Vec::new();
        let mut sync_guards = Vec::new();
        let remote_workspace = matches!(&task.workspace, task_core::WorkspaceSpec::Remote { .. })
            || self.cluster_of(&task).is_some();
        if remote_workspace {
            self.store.append_event(
                task_id,
                &Event::worker_progress(
                    run_id.clone(),
                    "review target sync skipped: remote workspace",
                ),
            )?;
        }
        // Legacy projectless worktrees have no RepoId for the durable candidate event;
        // keep their existing review/integration semantics until they are registered.
        if !remote_workspace
            && self.pre_review_sync_enabled()
            && !task.repos.is_empty()
            && let Some(workspaces) = self.task_workspaces_for(&task)
        {
            for repo in &workspaces.repos {
                let Some(worktree) = &repo.worktree else {
                    continue;
                };
                if !worktree.dir.is_dir() {
                    continue;
                }
                let Some(reference) = task.repos.iter().find(|r| r.name == repo.name) else {
                    continue;
                };
                let target_ref = self.review_target_ref(&task, reference.repo_id, &repo.source)?;
                // A failed repair deliberately reviews the restored, unsynced HEAD.
                // ADR-0120 付記（fallback の解除）: その HEAD のままで target を含まない間だけ同期を省く。
                // branch が変われば（HEAD が動いた・target が祖先になった）通常の同期へ戻り、
                // merge candidate を記録する。判定は events と git だけから毎回作る（daemon 再起動でも同じ）。
                let events = self.store.events_for(task_id)?;
                if let Some(fallback_head) = fallback_head(&events, reference.repo_id) {
                    let head = crate::integration::rev_parse(&worktree.dir, "HEAD");
                    let target_contained =
                        crate::integration::rev_parse(&worktree.dir, &target_ref).is_some_and(
                            |t| crate::integration::is_ancestor(&worktree.dir, &t, "HEAD"),
                        );
                    if head.as_deref() == Some(fallback_head) && !target_contained {
                        self.store.append_event(task_id, &Event::worker_progress(
                            run_id.clone(), format!("review target sync skipped for {}: integration repair exhausted; reviewing the unsynced HEAD without a merge candidate", repo.name)
                        ))?;
                        continue;
                    }
                    let why = if target_contained {
                        format!("{target_ref} is an ancestor of HEAD")
                    } else {
                        format!(
                            "HEAD moved from {fallback_head} to {}",
                            head.as_deref().unwrap_or("?")
                        )
                    };
                    self.store.append_event(
                        task_id,
                        &Event::worker_progress(
                            run_id.clone(),
                            format!(
                                "{FALLBACK_RELEASED_PREFIX}{}: {why}; syncing before review",
                                repo.name
                            ),
                        ),
                    )?;
                }
                // ADR-0130 D4: sync の前に behind を測る（stale 優先の材料）。
                self.observe_behind_target(task_id, reference.repo_id, &worktree.dir, &target_ref);
                let pre_sync_head = crate::integration::rev_parse(&worktree.dir, "HEAD");
                let outcome = task_ops::changes::sync_onto_target(&worktree.dir, &target_ref);
                // ADR-0130 D4: sync の後にも測る（取り込めていれば 0 で since が消える）。
                self.observe_behind_target(task_id, reference.repo_id, &worktree.dir, &target_ref);
                let (target_sha, before_sha, reviewed_sha) = match outcome {
                    task_ops::changes::SyncOutcome::UpToDate {
                        target_sha,
                        head_sha,
                    } => (target_sha, head_sha.clone(), head_sha),
                    task_ops::changes::SyncOutcome::Rebased {
                        target_sha,
                        before_sha,
                        head_sha,
                        ..
                    } => (target_sha, before_sha, head_sha),
                    task_ops::changes::SyncOutcome::Conflict { target_sha, files } => {
                        let before_sha = crate::integration::rev_parse(&worktree.dir, "HEAD")
                            .ok_or_else(|| {
                                StoreError::Invalid(
                                    "integration repair: cannot read HEAD after rebase abort"
                                        .into(),
                                )
                            })?;
                        #[cfg(test)]
                        if self.test_sync_conflict_as_review_fail {
                            // Phase 2 以前の旧経路: 衝突を review の不合格として worker に戻す（attempts を使う）。
                            self.store.apply_transition(
                                task_id,
                                Trigger::ReviewFail,
                                Some(Event::worker_progress(
                                    run_id.clone(),
                                    format!("review target sync conflict: {}", files.join(", ")),
                                )),
                            )?;
                            return Ok(true);
                        }
                        if self.try_integration_repair(
                            &task,
                            reference.repo_id,
                            &target_ref,
                            &target_sha,
                            &before_sha,
                            &files,
                        )? {
                            // No checks or reviewer run may observe the conflicted SHA.
                            return Ok(true);
                        }
                        self.store.append_event(
                            task_id,
                            &Event::IntegrationRepairExhausted {
                                work_unit_id: None,
                                repo_id: reference.repo_id,
                                target_sha: target_sha.clone(),
                                before_sha,
                                attempt: task_ops::delivery::MAX_INTEGRATION_REPAIRS,
                                reason: task_core::IntegrationRepairExhaustReason::LimitReached,
                                rollback_to_sha: None,
                                fallback: true,
                            },
                        )?;
                        let message = format!(
                            "review target sync skipped for {}: rebase onto {target_ref} {target_sha} conflicted in [{}] and was aborted; integration repair limit reached, reviewing the unsynced HEAD without a merge candidate",
                            repo.name,
                            files.join(", ")
                        );
                        tracing::warn!(%task_id, %message);
                        self.store.append_event(
                            task_id,
                            &Event::worker_progress(run_id.clone(), message),
                        )?;
                        continue;
                    }
                    task_ops::changes::SyncOutcome::Failed { ref detail }
                        if detail.contains("rebase --abort") || detail.contains("元の HEAD") =>
                    {
                        if let (Some(before_sha), Some(target_sha)) = (
                            pre_sync_head.as_deref(),
                            crate::integration::rev_parse(&worktree.dir, &target_ref),
                        ) {
                            let already_recorded = self.store.events_for(task_id)?.iter().any(|(_, e)| {
                                matches!(e, Event::IntegrationRepairExhausted {
                                    target_sha: recorded_target,
                                    before_sha: recorded_before,
                                    reason: task_core::IntegrationRepairExhaustReason::AbortFailed,
                                    ..
                                } if recorded_target == &target_sha && recorded_before == before_sha)
                            });
                            if !already_recorded {
                                self.store.append_event(
                                    task_id,
                                    &Event::IntegrationRepairExhausted {
                                        work_unit_id: None,
                                        repo_id: reference.repo_id,
                                        target_sha,
                                        before_sha: before_sha.to_string(),
                                        attempt: 0,
                                        reason:
                                            task_core::IntegrationRepairExhaustReason::AbortFailed,
                                        rollback_to_sha: None,
                                        fallback: false,
                                    },
                                )?;
                            }
                        }
                        self.store.append_event(
                            task_id,
                            &Event::worker_progress(
                                run_id.clone(),
                                format!("integration repair halted for {}: {detail}", repo.name),
                            ),
                        )?;
                        return Ok(false);
                    }
                    other => {
                        // Dirty・一般の Failed は未同期 HEAD で従来どおり checks/reviewer へ進む。
                        // 衝突は上の IntegrationRepair 経路が扱う。
                        let why = match other {
                            task_ops::changes::SyncOutcome::Dirty => {
                                "the worktree has uncommitted changes".to_string()
                            }
                            task_ops::changes::SyncOutcome::Failed { detail } => detail,
                            _ => String::new(),
                        };
                        let message = format!(
                            "review target sync skipped for {}: {why}; reviewing the unsynced HEAD without a merge candidate",
                            repo.name
                        );
                        tracing::warn!(%task_id, %message);
                        self.store.append_event(
                            task_id,
                            &Event::worker_progress(run_id.clone(), message),
                        )?;
                        continue;
                    }
                };
                let current = crate::integration::rev_parse(&worktree.dir, &target_ref);
                if current.as_deref() != Some(target_sha.as_str()) {
                    self.defer_stale_review(
                        task_id,
                        &run_id,
                        reviewer_run.as_ref().map_or(&run_id, |r| &r.run_id),
                        reference.repo_id,
                        &reviewed_sha,
                        current.as_deref().unwrap_or(""),
                        &format!("target {target_ref} advanced during sync from {target_sha}"),
                    )?;
                    return Ok(false);
                }
                sync_guards.push((
                    reference.repo_id,
                    worktree.dir.clone(),
                    format!("refs/heads/{}", worktree.branch),
                    target_ref.clone(),
                    target_sha.clone(),
                    reviewed_sha.clone(),
                ));
                synced.push(ReviewSync {
                    repo_id: reference.repo_id,
                    target_ref,
                    target_sha,
                    before_sha,
                    reviewed_sha,
                });
            }
        }
        // ADR-0117 D1: reviewer run には対象 task と祖先の人の決定・回答を渡す（store から集める。LLM は呼ばない）。
        let (decisions, answers) = if reviewer_run.is_some() {
            self.review_human_inputs(&task)?
        } else {
            (Vec::new(), Vec::new())
        };
        // ADR 2026-10-07-worker-no-subagents-no-llm-cli D6: 対象 run の検出（subagent・別 LLM の起動）を reviewer に渡す。
        let policy_violations = if reviewer_run.is_some() {
            self.review_policy_violations(task_id, &run_id)?
        } else {
            Vec::new()
        };
        if let Some(run) = &reviewer_run {
            task_ops::delivery::begin(
                self.store.as_ref(),
                &mut task,
                &self.config.workspace_root,
                &self.config.delivery,
                &run_id,
                &run.run_id,
            )
            .map_err(DispatchError::from)?;
        }
        for snapshot in &synced {
            if let Some(old) = self.store.delivery_get(task_id)?
                && old.repo_id == snapshot.repo_id
                && old.review_run == reviewer_run.as_ref().map_or("", |r| r.run_id.as_str())
            {
                if old.base != snapshot.target_sha || old.head != snapshot.reviewed_sha {
                    self.defer_stale_review(
                        task_id,
                        &run_id,
                        &old.review_run,
                        snapshot.repo_id,
                        &snapshot.reviewed_sha,
                        &old.base,
                        &format!(
                            "delivery base/head {}..{} differ from the reviewed snapshot {}..{}",
                            old.base, old.head, snapshot.target_sha, snapshot.reviewed_sha
                        ),
                    )?;
                    return Ok(false);
                }
                let mut next = old.clone();
                next.target_sha = Some(snapshot.target_sha.clone());
                next.reviewed_sha = Some(snapshot.reviewed_sha.clone());
                next.merge_candidate_sha = Some(snapshot.reviewed_sha.clone());
                if !self.store.delivery_save(Some(&old), &next)? {
                    return Err(
                        StoreError::Invalid("delivery review changed concurrently".into()).into(),
                    );
                }
            }
            let attempt = self.store.events_for(task_id)?.iter().filter(|(_, event)| {
                matches!(event, Event::ReviewTargetSynced { repo_id, .. } if *repo_id == snapshot.repo_id)
            }).count() as u32 + 1;
            self.store.append_event(
                task_id,
                &Event::ReviewTargetSynced {
                    review_run: reviewer_run.as_ref().map_or(&run_id, |r| &r.run_id).clone(),
                    repo_id: snapshot.repo_id,
                    target_ref: snapshot.target_ref.clone(),
                    target_sha: snapshot.target_sha.clone(),
                    before_sha: snapshot.before_sha.clone(),
                    reviewed_sha: snapshot.reviewed_sha.clone(),
                    merge_candidate_sha: snapshot.reviewed_sha.clone(),
                    attempt,
                },
            )?;
            let events = self.store.events_for(task_id)?;
            let units = self.store.work_units_for(task_id)?;
            if let Some((work_unit_id, attempt)) =
                pending_integration_repair(&events, &units, snapshot.repo_id)
            {
                self.store.append_event(
                    task_id,
                    &Event::IntegrationRepairResolved {
                        work_unit_id,
                        repo_id: snapshot.repo_id,
                        target_sha: snapshot.target_sha.clone(),
                        reviewed_sha: snapshot.reviewed_sha.clone(),
                        attempt,
                    },
                )?;
            }
        }

        // ADR-0074 D3.3（Phase F4a (b)）: 案件計画（マイルストーン DAG）の run は `plan.json` ではなく
        // `project-plan.json` を書くので、`plan.json` の解析・検証（`PlanCheck`）はしない
        // （`finish_project_plan_proposal` が別に読む）。
        let plan = if task.kind == TaskKind::Plan && !task_core::is_milestones_plan_task(&task) {
            Some(PlanCheck {
                depth: self.plan_depth(&task)?,
                limits: PlanLimits::default(),
                genres: self.config.genres.clone(),
                // ADR-0043 D2: 計画が子に書ける `repos` の名前（案件に登録されているものだけ）。
                repos: task_ops::delegate::project_repos(self.store.as_ref(), &task)
                    .map_err(ops_to_store)?
                    .into_iter()
                    .map(|r| r.name)
                    .collect(),
            })
        } else {
            None
        };
        // ADR-0016 M4: 集約 run のレビューには暗黙の条件「artifacts/summary.md がある」が加わる。
        let aggregate =
            task.aggregate && has_aggregate_transition(&self.store.events_for(task_id)?);

        // ADR-0018: 判定コマンドもクラスタで実行する。
        let cluster = self.cluster_of(&task);
        let remote_settings = cluster
            .as_ref()
            .map(|(spec, path, mode)| self.remote_ssh_settings(spec, path, task.id, *mode));
        let cluster_id = cluster.as_ref().map(|(spec, ..)| spec.id.clone());
        let events = self.store.events_for(task_id)?;
        let produced = artifacts_for_run(&events, &run_id);
        // ADR-0014 D1: Reviewer run も対象タスクに WorkerStarted（role: reviewer）を残す（アカウント別の集計に含めるため）。
        if let Some((provider_id, review_run_id, adapter_id)) = &review_run {
            let view = self.current_assignment_view();
            let model = self
                .adapters
                .get(provider_id)
                .map(|a| self.adapter_with_effective_models(provider_id, a.clone(), &view))
                .and_then(|a| {
                    a.model_for_tier(self.config.reviewer_hint.tier)
                        .ok()
                        .flatten()
                })
                .or_else(|| self.models.get(provider_id).cloned())
                .unwrap_or_default();
            self.store.append_event(
                task_id,
                &Event::WorkerStarted {
                    run_id: review_run_id.clone(),
                    adapter: adapter_id.clone(),
                    model: model.clone(),
                    provider: Some(provider_id.clone()),
                    account: account.clone(),
                    role: Some(RunRole::Reviewer),
                    task_role: None,
                },
            )?;
            // ADR-0072 D5（E2b の指摘）: reviewer run も `runs` 索引に書く
            // （(g)「全タスクの run について書く」）。
            let seq = self
                .store
                .runs_for_task(task_id)
                .map(|rs| {
                    rs.iter()
                        .filter(|r| r.role == task_core::RunIndexRole::Reviewer)
                        .count() as u32
                        + 1
                })
                .unwrap_or(1);
            if let Err(e) = self.store.run_index_start(task_core::RunRow {
                run_id: review_run_id.clone(),
                task_id: task_id.to_string(),
                work_unit_id: None,
                role: task_core::RunIndexRole::Reviewer,
                seq,
                status: task_core::RunIndexStatus::Running,
                adapter: Some(adapter_id.clone()),
                model: Some(model.clone()),
                account: account.clone(),
                session_id: None,
                checkpoint: None,
                usage: None,
                metrics: None,
                started_at: rfc3339(OffsetDateTime::now_utc()),
                finished_at: None,
            }) {
                tracing::warn!(%task_id, run_id = %review_run_id, error = %e, "failed to record the reviewer run start in the runs index");
            }
            // ADR-0076: reviewer run も worker と同じく quota の `before` を登録する
            // （`on_review_finished` が `resolve_quota_estimate` で閉じる）。
            self.quota_begin(account.as_deref(), account_adapter, review_run_id);
        }
        let timeout = self.config.review_timeout;
        // ADR-0036 D1/D2: 判定（`plan.json` / `review.json` / `summary.md` / `ArtifactExists` の既定パス）は
        // 対象タスクの成果物ディレクトリを基準にする。
        let artifacts_dir = self.artifacts_dir(&task, &dir);
        let entry_subject = subject.clone();
        let entry_run_id = run_id.clone();
        let subject = subject.clone();
        let tx = self.tx.clone();
        let remote_review = remote_settings.clone();
        // ADR-0019 D1 6. / ADR-0041 D1: 判定コマンドは worktree の中で実行する（元のリポジトリでは実行しない）。
        let review_work_dir = self.work_dir_for(&task);
        // ADR-0074 F5-fix: reviewer の checks は Task の worktree で走るので `<repo-key>`
        // （Task 単位の run と同じ target）。
        let review_check_env = self.check_cargo_target_env(&task, None);
        // ADR-0043 D4: リポジトリが宣言した検査コマンド（`workspace.toml` の `[commands] check`）。
        // ADR-0046 D4（Phase 59）: `mode = prototype` は「明示の受け入れ条件だけ」なので使わない。
        let repo_checks = if task.mode == task_core::TaskMode::Prototype {
            Vec::new()
        } else {
            self.default_checks(&task)
        };
        // ADR-0046 D4（Phase 59）: `mode = research` は「結果に出典か計測の記録」を暗黙の条件に足す。
        let research = task.mode == task_core::TaskMode::Research;
        let running_review_lock = review_lock.clone();
        // ADR-0079 D6（Phase R1c）: 木の子の最終レビューは親のブランチと比べる（検査の
        // `merge-base --is-ancestor main` を親のブランチに置き換え、reviewer の前置きに取り込み先を書く）。
        // ADR-0079 R5b-fix2: remote workspace の reviewer には worker と同じ `.celeris/remote-exec` の
        // 指示を足し、木の子なら「ブランチ統合なし」の注記にする（親のブランチの行は出さない）。
        let task = crate::review::review_view(
            task,
            &self.config.worktree_branch_prefix,
            remote_review.as_ref(),
        );
        let handle = tokio::spawn(async move {
            let _review_lock = running_review_lock;
            let ws: Box<dyn Workspace> = match remote_review {
                Some(settings) => {
                    let ssh = SshWorkspace::new(&dir, settings);
                    // R5b-fix2: reviewer が使うラッパを置く（worker の run が置いたものを最新の設定で
                    // 書き直すだけ。置けなくても判定そのものは続ける）。
                    if let Err(e) = ssh.write_remote_exec_helper().await {
                        tracing::warn!(%task_id, error = %e, "could not write the remote-exec helper for the reviewer (R5b-fix2)");
                    }
                    Box::new(ssh)
                }
                None => Box::new(
                    match review_work_dir {
                        Some(work) if work.is_dir() => {
                            LocalWorkspace::new(&dir).with_work_dir(work)
                        }
                        _ => LocalWorkspace::new(&dir),
                    }
                    .with_cargo_env(review_check_env),
                ),
            };
            let extras = ReviewExtras {
                subject,
                plan,
                reviewer: reviewer_run,
                human,
                aggregate,
                // ADR-0043 D4: リポジトリの `[commands] check`（タスクに検査コマンドが無いときだけ効く）。
                repo_checks,
                // ADR-0046 D4: `mode = research` の暗黙の条件。
                research,
                // ADR-0117 D1: 人の決定・回答。
                decisions,
                answers,
                // ADR 2026-10-07-worker-no-subagents-no-llm-cli D6。
                policy_violations,
            };
            let mut outcome = review_task(
                &task,
                ws.as_ref(),
                &dir,
                &artifacts_dir,
                &produced,
                timeout,
                extras,
            )
            .await;
            // A command check or reviewer may have changed the branch while the lock was
            // held. Never persist a passing verdict for a different commit or target.
            // ADR-0118 D4 付記: snapshot の変化は stale（不合格ではない）として返し、attempts を消費しない。
            for (repo_id, worktree, branch, target, target_sha, reviewed_sha) in &sync_guards {
                let head = crate::integration::rev_parse(worktree, "HEAD");
                let branch_head = crate::integration::rev_parse(worktree, branch);
                let target_head = crate::integration::rev_parse(worktree, target);
                if head.as_deref() != Some(reviewed_sha.as_str())
                    || branch_head != head
                    || target_head.as_deref() != Some(target_sha.as_str())
                {
                    outcome.target_stale = Some(crate::review::TargetStale {
                        repo_id: *repo_id,
                        reviewed_sha: reviewed_sha.clone(),
                        target_sha: target_head.unwrap_or_default(),
                        reason: format!(
                            "review snapshot changed: target={target} expected={target_sha} HEAD={} expected={reviewed_sha}",
                            head.unwrap_or_default()
                        ),
                    });
                    break;
                }
            }
            // The entry owns the lock through verdict persistence. Release this
            // task's copy before sending completion so it cannot outlive that entry.
            drop(_review_lock);
            let _ = tx.send(Completion::Review {
                task_id,
                run_id,
                outcome,
            });
        });
        self.reviewing.insert(
            task_id,
            ReviewEntry {
                _review_lock: review_lock,
                handle,
                provider,
                subject: entry_subject,
                run_id: entry_run_id,
                review_run_id: review_run.map(|(_, id, _)| id),
                since: OffsetDateTime::now_utc(),
                cluster: cluster_id,
                account,
                account_adapter,
            },
        );
        Ok(true)
    }

    /// ADR-0118 D4 付記: stale（target 再進行・delivery の base/head 不一致・検査後の snapshot 変化）。
    /// `ReviewTargetAdvanced`（検査した SHA と今の target の両方）と理由を残し、遷移はしない（attempts を
    /// 消費しない）。task は reviewing のまま、次の tick の review 入口が再 sync → 全 checks → reviewer を
    /// やり直す。上限は [`Self::target_resync_exhausted`] が見る。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn defer_stale_review(
        &self,
        task_id: TaskId,
        run_id: &str,
        review_run: &str,
        repo_id: task_core::RepoId,
        reviewed_sha: &str,
        target_sha: &str,
        reason: &str,
    ) -> Result<(), DispatchError> {
        let events: Vec<Event> = self
            .store
            .events_for(task_id)?
            .into_iter()
            .map(|(_, e)| e)
            .collect();
        let (count, _, _) = pre_review_stale_state(&events);
        tracing::warn!(%task_id, %reason, "review snapshot is stale; re-syncing before review (ADR-0118 D4)");
        self.store.append_event(
            task_id,
            &Event::ReviewTargetAdvanced {
                review_run: review_run.to_owned(),
                repo_id,
                reviewed_sha: reviewed_sha.to_owned(),
                target_sha: target_sha.to_owned(),
                attempt: count + 1,
            },
        )?;
        self.store.append_event(
            task_id,
            &Event::worker_progress(
                run_id.to_owned(),
                format!("review target stale: {reason}; re-syncing, re-checking and re-reviewing"),
            ),
        )?;
        Ok(())
    }

    /// ADR-0118 D4 付記: 自動の再同期が上限（[`task_ops::delivery::MAX_TARGET_RESYNCS`]）に達したか。
    /// 達していれば理由と両 SHA を一度だけ残し、reviewing のまま止める（attempts 不変・`failed` にしない）。
    /// 人のコメント（`interrupt`）などの遷移で数え直して再開する。
    fn target_resync_exhausted(
        &self,
        task_id: TaskId,
        run_id: &str,
    ) -> Result<bool, DispatchError> {
        let events: Vec<Event> = self
            .store
            .events_for(task_id)?
            .into_iter()
            .map(|(_, e)| e)
            .collect();
        let (count, last, halted) = pre_review_stale_state(&events);
        if count <= task_ops::delivery::MAX_TARGET_RESYNCS {
            return Ok(false);
        }
        if !halted {
            let (reviewed, target) = match last {
                Some(Event::ReviewTargetAdvanced {
                    reviewed_sha,
                    target_sha,
                    ..
                }) => (reviewed_sha.as_str(), target_sha.as_str()),
                _ => ("", ""),
            };
            let message = format!(
                "{TARGET_RESYNC_HALTED_PREFIX}the target kept moving after {} automatic re-syncs (reviewed {reviewed}, target {target}); the task stays reviewing with attempts unchanged until a human resumes it",
                task_ops::delivery::MAX_TARGET_RESYNCS
            );
            tracing::warn!(%task_id, %message);
            self.store
                .append_event(task_id, &Event::worker_progress(run_id.to_owned(), message))?;
        }
        Ok(true)
    }

    /// `Reviewer` run のアダプタ／プロバイダを選ぶ（ADR-0007 D5 1.）。並列度の枠は実行中 run と共有する。
    /// ADR-0024 D2: 選んだプロバイダが `account_pool` ならアカウントも選ぶ（戻り値の第 2 要素）。
    #[allow(clippy::type_complexity)]
    pub(super) fn pick_reviewer(
        &mut self,
        task: &Task,
        subject_run_id: &str,
    ) -> Option<(ProviderId, Option<(AccountAdapter, String)>, ReviewerRun)> {
        if self.workers_in_flight() >= self.config.max_concurrency {
            return None;
        }
        // ADR-0012 D2: ワーカー run と同じ手順（上限のプロバイダを飛ばして次へ、候補なしは warn）で選ぶ。
        let org = self.store.org_list().ok()?;
        let department = task
            .assignee
            .as_deref()
            .and_then(|id| task_core::department_of(&org, id));
        let node = department
            .as_deref()
            .and_then(|id| org.iter().find(|n| n.id == id))
            .map(|n| NodeContext {
                id: n.id.clone(),
                name: n.name.clone(),
                brief: n.brief.clone(),
            });
        let profile = department
            .as_deref()
            .map(|id| task_core::profile::resolve(&org, id));
        // Review follows the subject's department rules, not the lead reviewer's profile.
        let review_mounts = task
            .assignee
            .as_deref()
            .map(|id| task_core::profile::resolve(&org, id).skills_mounts)
            .unwrap_or_default();
        let (skills, _) =
            self.skills_context(&review_mounts, task_ops::knowledge::SkillUse::Review);
        // ADR-0069 Phase 118 D4: reviewer の lane は、上ほど強い優先順位で決める。
        //   1. 部署の `profile.review_tier`（ADR-0069 D2。最も具体的な指定）。
        //   2. `[reviewer] tier` の明示（`reviewer_tier_override`）。
        //   3. 既定: worker run の lane に一致させ、組織の天井（`lane_ceiling`）で丸める。
        // 1./2. は丸めない（人・運用の明示は組織の既定より強い。D2 の「人の明示 tier は天井で
        // 丸めない」と同じ考え方を運用の明示にも適用する）。
        let ceiling = profile
            .as_ref()
            .map(|p| p.lane_ceiling())
            .unwrap_or_default();
        let worker_lane = task.worker_hint.tier;
        let (default_tier, default_clamp) = ceiling.clamp(worker_lane);
        let (reviewer_lane, review_rule_id, review_reasons) = match (
            profile.as_ref().and_then(|p| p.review_tier),
            self.config.reviewer_tier_override,
        ) {
            (Some(dept_tier), _) => (
                dept_tier,
                "reviewer/department-review-tier",
                vec![format!(
                    "org profile review.tier = {dept_tier:?} (most specific; ADR-0069 D2)"
                )],
            ),
            (None, Some(explicit)) => (
                explicit,
                "reviewer/explicit-config",
                vec![format!(
                    "[reviewer] tier = {explicit:?} (explicit config; Phase 118 D4)"
                )],
            ),
            (None, None) => {
                let mut reasons = vec![format!("default: matches the worker lane {worker_lane:?}")];
                if let Some(clamp) = &default_clamp {
                    reasons.push(clamp.clone());
                }
                (default_tier, "reviewer/matches-worker-lane", reasons)
            }
        };
        let mut hint = self.config.reviewer_hint.clone();
        hint.tier = reviewer_lane;

        // ADR-0054 Phase 67c: 部署のレビュー・切り分け run（`kind = lead`）も、継続セッションの
        // (adapter, account) に留まれるかを先に試す（`resolve_node_session` と同じキー）。
        let sticky_session = match &department {
            Some(dept_id) => self
                .store
                .node_session_active(dept_id, task_core::SessionKind::Lead, None)
                .ok()
                .flatten(),
            None => None,
        };
        let mut full = std::collections::HashSet::new();
        let (adapter_id, provider_id, selected_account) = self.select_provider(
            &hint,
            Instant::now(),
            task.id,
            &mut full,
            sticky_session.as_ref(),
        )?;
        let base_adapter = match self.adapters.get(&provider_id) {
            Some(a) => a.clone(),
            None => {
                tracing::warn!(task_id = %task.id, provider = %provider_id, adapter = %adapter_id, "no adapter instance for reviewer provider");
                return None;
            }
        };
        // ADR 2026-10-06 model-role-assignments D2: reviewer run も割り当てを重ねた実効 bindings で起こす。
        let base_adapter = self.adapter_with_effective_models(
            &provider_id,
            base_adapter,
            &self.current_assignment_view(),
        );
        if let Err(reason) = base_adapter.model_for_tier(hint.tier) {
            if self.warned_unroutable.insert(task.id) {
                let _ = self.store.append_event(
                    task.id,
                    &Event::worker_progress(
                        subject_run_id,
                        format!("review model routing blocked: {reason}"),
                    ),
                );
            }
            return None;
        }
        let adapter = match &selected_account {
            Some((account_adapter, account_id)) => {
                match self.adapter_for_account(&base_adapter, *account_adapter, account_id) {
                    Some(a) => a,
                    None => {
                        tracing::warn!(task_id = %task.id, provider = %provider_id, account_id, "adapter does not support account pools for reviewer run; deferring");
                        return None;
                    }
                }
            }
            None => base_adapter,
        };
        let account = selected_account.as_ref().map(|(_, id)| id.clone());
        let account_adapter = selected_account.as_ref().map(|(a, _)| *a);
        let review_run_id = ulid::Ulid::new().to_string();
        // ADR-0054 D1 / Phase 67b 追記: 部署があれば、この run の `session_established`/
        // `session_resume_failed` を Lead セッション（`kind = lead`）に配線する（Phase 67 で抜けていた
        // 配線。下の `resolve_node_session` と同じ `(department, Lead, None)` のキー）。
        let session_key = department
            .clone()
            .map(|dept_id| (dept_id, task_core::SessionKind::Lead, None));
        let sink = ReviewerSink {
            store: self.store.clone(),
            task_id: task.id,
            subject_run_id: subject_run_id.to_string(),
            review_run_id: review_run_id.clone(),
            account: account.clone(),
            account_book: account_adapter.and_then(|a| self.account_book(a)),
            session_key,
        };
        // ADR-0054 D1（Phase 67）: 部署の根ノード（engineering/research/operations）は
        // レビュー・切り分け run を継続セッション（`kind = lead`）で走らせる（ADR-0051）。部署が無い
        // 仕事（従来の独立レビュアー）では継続しない。store のエラーはレビューそのものを止めない
        // （継続無し＝Phase 66 までと同じ挙動にフォールバックする）。
        let (session, session_diff) = match &department {
            Some(dept_id) => match self.resolve_node_session(
                dept_id,
                task_core::SessionKind::Lead,
                None,
                &adapter_id,
                account.as_deref(),
                OffsetDateTime::now_utc(),
            ) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(task_id = %task.id, department = %dept_id, error = %e, "failed to resolve the department lead session; reviewing without one");
                    (None, Vec::new())
                }
            },
            None => (None, Vec::new()),
        };
        tracing::info!(task_id = %task.id, %review_run_id, adapter = %adapter_id, provider = %provider_id, account = account.as_deref(), "starting reviewer run");
        // ADR-0069 Phase 118 D4: reviewer run にも `Event::RoutingDecided` を残す（Phase 114 は worker
        // run にしか出していなかった）。reviewer は `TaskFeatures` 規則表を通らないので `rule_id` は
        // 上で決めた 3 種のいずれか、`source = System`（policy ではなく config/組織の指定で決まる）。
        // ストア書き込み失敗はレビューそのものを止めない（`warned_unroutable` と同じベストエフォート）。
        let review_model_id = adapter
            .model_for_tier(hint.tier)
            .ok()
            .flatten()
            .unwrap_or_default();
        let review_reasoning_effort = adapter
            .reasoning_effort_for_tier(hint.tier)
            .filter(|_| adapter.supports_reasoning_effort());
        let review_record = task_core::RoutingRecord {
            org_node: department.clone(),
            harness: Some("reviewer".to_string()),
            decision: task_core::LaneDecision {
                lane: reviewer_lane,
                proposed: worker_lane,
                source: task_core::TierSource::System,
                rule_id: review_rule_id.to_string(),
                policy_version: task_core::LANE_POLICY_VERSION.to_string(),
                features: task_core::TaskFeatures::infer(task),
                reasons: review_reasons,
                clamped_by: if review_rule_id == "reviewer/matches-worker-lane" {
                    default_clamp.clone()
                } else {
                    None
                },
                hint: None,
                escalation: None,
                shadow: None,
            },
            resolution: task_core::model_routing::LaneResolution {
                lane: Some(reviewer_lane),
                adapter: adapter_id.clone(),
                provider: Some(provider_id.clone()),
                account: account.clone(),
                model_id: review_model_id,
                reasoning_effort: review_reasoning_effort,
                // ADR-0132 付記 L8: reviewer run の記録には選択の理由を付けない。
                selection: None,
                coding_default: None,
            },
            quota_reason: None,
            work_unit_id: None,
            escalation: None,
            optimizer: None,
        };
        let _ = self.store.append_event(
            task.id,
            &Event::RoutingDecided {
                run_id: review_run_id.clone(),
                record: Box::new(review_record),
            },
        );
        // ADR 2026-10-07-build-tmp-hygiene 付記 A1（2026-10-09）: reviewer は反証の self-execution で cargo を
        // 走らせうる。run・検査と同じ `CARGO_TARGET_DIR`（対象 task の owner）を重ね、作業場所に target を作らせない。
        let (adapter, cargo_target_dir) = self.with_review_cargo_env(task, adapter);
        Some((
            provider_id,
            selected_account,
            ReviewerRun {
                cargo_target_dir,
                node,
                profile,
                skills,
                adapter,
                run_id: review_run_id,
                limits: RunLimits {
                    wall_clock: Duration::from_secs(task.budget.max_wall_secs),
                    idle_timeout: self.config.idle_timeout,
                    kill_grace: self.config.kill_grace,
                },
                sink: Box::new(sink),
                hint,
                // Phase 38（ADR-0028 追記）: レビュー対象の分野の manifest（決定的。設定を引くだけ）。
                subject_genre: task
                    .genre
                    .as_deref()
                    .and_then(|id| GenreSpec::find(&self.config.genres, id))
                    .map(|g| GenreContext::from_spec(g, &self.config.roles)),
                session,
                session_diff,
            },
        ))
    }
}
