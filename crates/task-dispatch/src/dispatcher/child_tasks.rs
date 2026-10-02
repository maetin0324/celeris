//! 委譲した子（集約 run・失敗の escalate・待ちの解消）と人の承認の子の作成（ADR-0010）。ADR-0082 の L2。

use super::*;

impl Dispatcher {
    /// ADR-0016 M4: `aggregate = true` で、Approval 以外の子が 1 件以上あり、まだ集約遷移をしていないか。
    pub(super) fn needs_aggregate_run(&self, task: &Task) -> Result<bool, DispatchError> {
        if !task.aggregate {
            return Ok(false);
        }
        let has_children = self
            .store
            .children(task.id)?
            .iter()
            .any(|c| c.kind != TaskKind::Approval);
        if !has_children {
            return Ok(false);
        }
        let events = self.store.events_for(task.id)?;
        Ok(!has_aggregate_transition(&events))
    }

    /// ADR-0021 D1/D3: 委譲した子（`Event::Delegated`）のうち、**前回この親が子の失敗を扱ってから後に** `failed` に
    /// なったもの。一度扱った失敗は数え直さない（親が別の子に割り当て直して成功したのに、古い失敗で止まらないため）。
    /// 返り値は id 順の `(子, 直近のワーカー run の outcome)`。
    ///
    /// Phase 45（実機バグ）: ここは親と子で別々のタスクのイベント id を比較するので、`events_for`（タスクごとの
    /// ローカルな `seq`）ではなく `events_for_with_global_ids`（`events` テーブルの全タスク共通の `id`）を
    /// 使わなければならない。`seq` で比較すると、子の方がイベント数が多い（＝ `seq` が大きい）場合に
    /// 「一度扱った失敗」でも毎回「新規」と判定され、同じ質問が繰り返し出る。
    pub(super) fn newly_failed_delegated_children(
        &self,
        task_id: TaskId,
    ) -> Result<Vec<(Task, Option<String>)>, DispatchError> {
        let events = self.store.events_for_with_global_ids(task_id)?;
        // 直近に子の失敗を扱った時点（グローバル id。イベント id は単調増加）。
        let handled_at = events
            .iter()
            .rev()
            .find_map(|(id, e)| match e {
                Event::Transitioned { reason, .. } if reason == Trigger::ChildFailed.name() => {
                    Some(*id)
                }
                _ => None,
            })
            .unwrap_or(0);
        let mut delegated: Vec<TaskId> = events
            .iter()
            .filter_map(|(_, e)| match e {
                Event::Delegated { task_ids, .. } => Some(task_ids.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        delegated.sort();
        delegated.dedup();

        let mut out = Vec::new();
        for id in delegated {
            let Some(child) = self.store.get(id)? else {
                continue;
            };
            if child.status != Status::Failed {
                continue;
            }
            let child_events = self.store.events_for_with_global_ids(id)?;
            let failed_at = child_events.iter().rev().find_map(|(eid, e)| match e {
                Event::Transitioned {
                    to: Status::Failed, ..
                } => Some(*eid),
                _ => None,
            });
            // 既に扱った失敗（id が前回の child_failed より前）は数えない。
            if failed_at.is_some_and(|at| at <= handled_at) {
                continue;
            }
            let outcome = child_events.iter().rev().find_map(|(_, e)| match e {
                Event::WorkerFinished {
                    outcome,
                    role: None,
                    ..
                } => Some(outcome.clone()),
                _ => None,
            });
            out.push((child, outcome));
        }
        Ok(out)
    }

    /// ADR-0021 D1/D2: 委譲した子が失敗していたら、親をやり直す（attempts に余裕があるとき）か、
    /// 人間に質問して待つ（`blocked`）。**親を `failed` にはしない。** 扱ったら `true`（`events` も記録済み）。
    /// `events`（判定結果など、まだ記録していないもの）は、**扱ったときだけ**同じトランザクションで一緒に記録する。
    /// 扱わなかったときは触らない（呼び出し側がそのまま使う）。
    pub(super) fn escalate_failed_children(
        &mut self,
        task: &Task,
        run_id: &str,
        events: &mut Vec<Event>,
    ) -> Result<bool, DispatchError> {
        if self.config.delegation.on_child_failure == OnChildFailure::Ignore {
            return Ok(false);
        }
        // ADR-0033 D4 / Phase 28: 対話タスクは委譲できない（子を持たない）ので、そもそも子の失敗は
        // 起きないはずだが、念のため `retry_then_ask` の対象から外す（対話タスクが `blocked` に落ちる
        // 経路を完全に断つ）。
        if task_core::is_conversation(task) {
            return Ok(false);
        }
        let failed = self.newly_failed_delegated_children(task.id)?;
        if failed.is_empty() {
            return Ok(false);
        }
        let mut events = std::mem::take(events);
        let listed = failed
            .iter()
            .map(|(child, outcome)| {
                format!(
                    "{} ({}): {}",
                    child.title,
                    child.id,
                    outcome.as_deref().unwrap_or("(no outcome recorded)")
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        // 状態機械と同じ判定（ADR-0021 D1）。ここで分かるのは「やり直せるか」だけ。
        let will_retry = task.attempts < task.budget.max_retries;
        if will_retry {
            events.push(Event::worker_progress(
                run_id,
                format!(
                    "{} delegated child task(s) failed; retrying this task (attempt {}/{}): {listed}",
                    failed.len(),
                    task.attempts + 1,
                    task.budget.max_retries,
                ),
            ));
        } else {
            let text = format!(
                "委譲した子タスクが失敗し、やり直し（max_retries = {}）でも解決しませんでした。どうしますか。\n\
                 失敗した子: {listed}\n\
                 回答するとこのタスクは指示を持って再開します: celerisctl answer {} \"…\"",
                task.budget.max_retries, task.id,
            );
            // Phase 44（実機 2026-09-18）: この質問はディスパッチャ由来（ワーカーの `Question` ではない）だが、
            // Phase 26 と同じく `approvals` にも残す。そうしないと認可画面に出ず、`approval_pending` の
            // Discord 通知も飛ばない（受信箱にだけ出て気づかれない）。
            if let Err(e) = crate::approvals::record_question_approval(
                self.store.as_ref(),
                task,
                &text,
                OffsetDateTime::now_utc(),
            ) {
                tracing::warn!(task_id = %task.id, %run_id, error = %e, "failed to record the approval for the child-failure question");
            }
            events.push(Event::QuestionRaised {
                run_id: run_id.to_string(),
                text,
            });
        }
        match self
            .store
            .apply_transition_with_events(task.id, Trigger::ChildFailed, events)
        {
            Ok(outcome) => {
                tracing::info!(
                    task_id = %task.id, %run_id, next = ?outcome.next, failed = failed.len(),
                    "delegated child task(s) failed; parent retries or asks a human (ADR-0021)"
                );
            }
            Err(StoreError::InvalidTransition(e)) => {
                tracing::warn!(task_id = %task.id, error = %e, "child_failed transition could not be applied");
                return Ok(false);
            }
            Err(e) => return Err(e.into()),
        }
        Ok(true)
    }

    /// ADR-0016 M1 / M4: `Aggregate`（reviewing → ready、attempts 据え置き）を適用し、次の dispatch を集約 run にする。
    pub(super) fn schedule_aggregate_run(
        &mut self,
        task_id: TaskId,
        run_id: &str,
        mut events: Vec<Event>,
    ) -> Result<(), DispatchError> {
        let children = self.store.children(task_id)?.len();
        events.push(Event::worker_progress(
            run_id,
            format!(
                "all {children} delegated child task(s) finished; scheduling the aggregate run"
            ),
        ));
        match self
            .store
            .apply_transition_with_events(task_id, Trigger::Aggregate, events)
        {
            Ok(outcome) => {
                tracing::info!(%task_id, %run_id, next = ?outcome.next, "aggregate run scheduled");
            }
            Err(StoreError::InvalidTransition(e)) => {
                tracing::warn!(%task_id, error = %e, "aggregate transition could not be applied");
            }
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }

    /// ADR-0016 M5: 子待ちの親を毎 tick 数え直し、全て終端になったら集約 run（M4）か `ReviewPass`（Plan なら `complete_plan`）。
    pub(super) fn settle_awaiting_children(&mut self) -> Result<(), DispatchError> {
        let ids: Vec<TaskId> = self.awaiting_children.keys().copied().collect();
        for task_id in ids {
            let Some(task) = self.store.get(task_id)? else {
                self.awaiting_children.remove(&task_id);
                continue;
            };
            if task.status != Status::Reviewing {
                self.awaiting_children.remove(&task_id);
                continue;
            }
            let pending = pending_children(self.store.as_ref(), task_id).map_err(ops_to_store)?;
            if pending > 0 {
                continue;
            }
            let Some(waiting) = self.awaiting_children.remove(&task_id) else {
                continue;
            };
            // ADR-0021 D1: 子が失敗していたら、集約・完了より先に「やり直す or 人に聞く」。
            if self.escalate_failed_children(&task, &waiting.run_id, &mut Vec::new())? {
                continue;
            }
            if self.needs_aggregate_run(&task)? {
                self.schedule_aggregate_run(task_id, &waiting.run_id, Vec::new())?;
                continue;
            }
            let result = match (task.kind, waiting.plan) {
                (TaskKind::Plan, Some(mut plan)) => {
                    let org = self.store.org_list()?;
                    self.fix_plan_for_harness(&task, &mut plan, &org);
                    // ADR-0039 D2: 子の作業場所は 明示 > 案件 > 親。
                    let project_workspace =
                        task_ops::delegate::project_workspace(self.store.as_ref(), &task)
                            .map_err(ops_to_store)?;
                    // ADR-0043 D2: 子のリポジトリは 明示（計画の `repos`）> 親 > 案件の primary。
                    let project_repos =
                        task_ops::delegate::project_repos(self.store.as_ref(), &task)
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
                    self.store.complete_plan(
                        task_id,
                        Vec::new(),
                        children,
                        self.config.plan_auto_accept,
                    )
                }
                _ => self.store.apply_transition_with_events(
                    task_id,
                    Trigger::ReviewPass,
                    Vec::new(),
                ),
            };
            match result {
                Ok(outcome) => {
                    tracing::info!(%task_id, run_id = %waiting.run_id, next = ?outcome.next, "delegated children finished; parent completed");
                }
                Err(StoreError::InvalidTransition(e)) => {
                    tracing::warn!(%task_id, error = %e, "parent completion could not be applied");
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    /// `task.acceptance` の各 `Check::Human` について `Approval` 子タスクを解決する（ADR-0008 D2）。
    /// 子が無ければ作る。いずれかがまだ未決（`Ready`/`Draft`）なら `Ok(None)`（レビュー全体を延期）。
    /// 全て終端に達していれば `idx -> (pass, reason)` を返す（`Human` criterion が無ければ空の map）。
    pub(super) fn resolve_human_approvals(
        &self,
        task: &Task,
    ) -> Result<Option<HumanVerdicts>, DispatchError> {
        let human_indices: Vec<usize> = task
            .acceptance
            .iter()
            .enumerate()
            .filter(|(_, c)| matches!(c.check, Check::Human))
            .map(|(idx, _)| idx)
            .collect();
        if human_indices.is_empty() {
            return Ok(Some(HashMap::new()));
        }

        let existing_children = self.store.list(None)?;
        let mut resolved = HashMap::with_capacity(human_indices.len());
        for idx in human_indices {
            let title = human_approval_title(task, idx);
            let child = match existing_children.iter().find(|c| {
                c.parent_id == Some(task.id) && c.kind == TaskKind::Approval && c.title == title
            }) {
                Some(c) => c.clone(),
                None => self.create_human_approval_child(task, idx, &title)?,
            };
            match child.status {
                Status::Done => {
                    resolved.insert(
                        idx,
                        (true, format!("approved (approval task {})", child.id)),
                    );
                }
                Status::Failed => {
                    let note = approval_decision_note(&self.store.events_for(child.id)?);
                    resolved.insert(
                        idx,
                        (
                            false,
                            format!("rejected (approval task {}){note}", child.id),
                        ),
                    );
                }
                Status::Cancelled => {
                    resolved.insert(
                        idx,
                        (false, format!("approval task {} was cancelled", child.id)),
                    );
                }
                _ => return Ok(None),
            }
        }
        Ok(Some(resolved))
    }

    /// `Human` criterion のための `Approval` 子タスクを新規作成する（ADR-0008 D2）。
    pub(super) fn create_human_approval_child(
        &self,
        task: &Task,
        idx: usize,
        title: &str,
    ) -> Result<Task, DispatchError> {
        let now = OffsetDateTime::now_utc();
        let approval = Task {
            tree: None,
            paused_at: None,
            routing: None,
            repos: Vec::new(),
            id: TaskId::new(),
            parent_id: Some(task.id),
            kind: TaskKind::Approval,
            title: title.to_string(),
            objective: task.acceptance[idx].text.clone(),
            acceptance: vec![],
            inputs: vec![],
            depends_on: vec![],
            status: Status::Ready,
            priority: task.priority,
            worker_hint: task.worker_hint.clone(),
            workspace: task.workspace.clone(),
            budget: task.budget,
            attempts: 0,
            lease: None,
            created_at: now,
            updated_at: now,
            role: None,
            genre: None,
            aggregate: false,
            // ADR-0033 D2（監査 D-3）: 派生タスクは親の案件・途中目標・担当を継ぐ（仕事の木から子が消えないように）。
            project_id: task.project_id,
            milestone_id: task.milestone_id,
            assignee: task.assignee.clone(),
            conversation: None,
            labels: Vec::new(),
            category: Default::default(),
            // ADR-0046 D4（Phase 59）: 派生タスクは親の進め方を継ぐ。
            skills: Vec::new(),
            mode: task.mode,
        };
        // ADR-0010 D2: 挿入・Created・ApprovalRequested を 1 トランザクションで。
        self.store
            .create_task(&approval, vec![Event::ApprovalRequested])?;
        tracing::info!(task_id = %task.id, approval_id = %approval.id, criterion_idx = idx, "created approval child for human check");
        Ok(approval)
    }
}
