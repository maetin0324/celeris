//! WorkUnit の gate・準備・並列・checks（ADR-0072、ADR-0074）。`ExecutionGated` と routing は同じ transaction のまま。ADR-0082 の L2。

use super::*;

impl Dispatcher {
    /// ADR-0072 D14/D6・E4 (g): `wu.spec.checks` を `review::run_work_unit_checks`（review.rs の
    /// `Check::Command` 実行を再利用）で実行し、終わったら `Completion::WorkUnitChecks` を送る。
    /// `self.running` からは既に取り除かれている（`on_worker_finished` の冒頭）ので、ここでは
    /// lease の維持や snapshot への影響は無い（review run の Command 実行と同じ扱い）。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn spawn_work_unit_checks(
        &mut self,
        task_id: TaskId,
        run_id: String,
        wu: task_core::WorkUnitRow,
        account: Option<String>,
        account_adapter: Option<AccountAdapter>,
        run_since: Option<OffsetDateTime>,
        provider: ProviderId,
        result: Result<RunOutcome, AdapterError>,
    ) -> Result<(), DispatchError> {
        let Some(task) = self.store.get(task_id)? else {
            return Ok(());
        };
        let Some(dir) = self.task_dir(&task) else {
            // リモートの workspace は E4 の範囲外（review.rs の Command 実行も同様、remote_review
            // 経由の別経路を持つ。ここでは checks を飛ばして通常どおり `Done` として扱う）。
            return self.finish_worker_result(
                task,
                Some(wu),
                run_id,
                account,
                account_adapter,
                run_since,
                provider,
                result,
            );
        };
        // ADR-0074 D1.2（Phase F2b）: v2 の WU の checks は WU の作業ツリーで走らせ、検査の間も WU の
        // lease（と Task の lease）を延ばしておく（検査中に照合で WU を戻さないため）。
        let work_dir = match self.work_unit_trees(&task, &wu)?.into_iter().next() {
            Some((tree, _)) => Some(tree),
            None => self.work_dir_for(&task),
        };
        if wu.phase.is_some() {
            let ttl = self
                .config
                .review_timeout
                .saturating_mul(wu.spec.checks.len() as u32 * 2 + 1)
                + self.config.lease_grace;
            if let Err(e) = self.store.renew_lease(task_id, &run_id, ttl) {
                tracing::warn!(%task_id, %run_id, error = %e, "could not extend the work unit lease for its checks");
            }
        }
        // ADR-0074 F5-fix（不具合 1）: 検査も run と同じ `CARGO_TARGET_DIR` で走らせる（自分の
        // worktree の WU は `<repo-key>/wu-<id>`、それ以外は `<repo-key>`）。
        let own_tree = !self.work_unit_trees(&task, &wu)?.is_empty();
        let check_env = self.check_cargo_target_env(&task, own_tree.then_some(wu.id.as_str()));
        let ws: task_worker::LocalWorkspace = match work_dir {
            Some(w) if w.is_dir() => task_worker::LocalWorkspace::new(&dir).with_work_dir(w),
            _ => task_worker::LocalWorkspace::new(&dir),
        }
        .with_cargo_env(check_env);
        let checks = wu.spec.checks.clone();
        let timeout = self.config.review_timeout;
        let tx = self.tx.clone();
        let checking_run_id = run_id.clone();
        // ADR-0079 付記 R7-5 D1: 不合格の記録に残す、check を実際に走らせる所。
        let check_cwd = ws.work_dir().to_path_buf();
        let handle = tokio::spawn(async move {
            let check_results = crate::review::run_work_unit_checks(&ws, &checks, timeout).await;
            let _ = tx.send(Completion::WorkUnitChecks {
                task_id,
                run_id,
                account,
                account_adapter,
                run_since,
                provider,
                result: Box::new(result),
                check_results,
                checks,
                check_cwd,
            });
        });
        // Phase F5-fix2: 検査の間も「手元の仕事」として数える（`in_flight`・lease の照合）。
        self.checking
            .insert(checking_run_id, CheckingEntry { task_id, handle });
        Ok(())
    }

    /// ADR-0072 D14/D6・E4 (g): `spawn_work_unit_checks` の結果を受けて、`finish_worker_result` に
    /// 引き継ぐ。1 つでも `pass = false` があれば、この run を `Terminal::Error{retryable: true}`
    /// （WU の retry。`execution_scheduler::decide` が `RunEnd::Failed{retryable:true}` として扱う）に
    /// すり替える。全部 pass なら元の `result`（`Terminal::Done`）をそのまま使う。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn on_work_unit_checks_finished(
        &mut self,
        task_id: TaskId,
        run_id: String,
        account: Option<String>,
        account_adapter: Option<AccountAdapter>,
        run_since: Option<OffsetDateTime>,
        provider: ProviderId,
        result: Result<RunOutcome, AdapterError>,
        check_run: WorkUnitCheckRun,
    ) -> Result<(), DispatchError> {
        let Some(task) = self.store.get(task_id)? else {
            tracing::warn!(%task_id, %run_id, "work unit checks finished for unknown task");
            return Ok(());
        };
        // ADR-0002 D9 / ADR-0005 D4: checks の実行中にリースが失効・タスクが cancel されていたら、
        // stale worker result と同じ扱いで捨てる。
        let lease_matches = self.run_holds_lease(&task, &run_id)?;
        if !lease_matches {
            tracing::warn!(%task_id, %run_id, status = ?task.status, "stale work unit check result discarded");
            return Ok(());
        }
        let current_wu = self.store.work_units_for(task_id)?.into_iter().find(|u| {
            u.status == task_core::WorkUnitStatus::Running
                && u.last_run_id.as_deref() == Some(run_id.as_str())
        });
        // ADR-0079 付記 R7-5: 不合格の検査（cmd・期待する exit・判定文）と走らせた所を残し、run の usage を落とさない。
        let failure = check_run.failure(&result);
        let result = match &failure {
            None => result,
            Some(f) => Ok(RunOutcome {
                terminal: Terminal::Error {
                    message: format!("work unit {}", f.summary()),
                    retryable: true,
                },
                exit_code: None,
            }),
        };
        self.finish_worker_result_with(
            task,
            current_wu,
            run_id,
            account,
            account_adapter,
            run_since,
            provider,
            result,
            failure,
        )
    }

    /// ADR-0072 D15（Phase E2）: `dispatch_ready` がこの Task について何をすべきか。
    pub(super) fn wu_dispatch_gate(
        &self,
        task_id: TaskId,
    ) -> Result<WuDispatchGate, DispatchError> {
        let Some(active_plan) = self.store.execution_plan_active(task_id)? else {
            return Ok(WuDispatchGate::Atomic);
        };
        // ADR-0072「Phase F6 実装時の決定」: 計画を持つ Task に人が後から compound を依頼した
        // （`POST /tasks/{id}/execution/decompose`、`ExecutionHintSet{replan: true}`）なら、次の run は
        // replan の planner run（D17 5.。`max_replans` に数える）。依頼はこの dispatch の
        // `Transitioned{to: running}` で消費される（`pending_replan_request`）。
        let events = self.store.events_for(task_id)?;
        // ADR-0079 D10 / R3a 付記 15.（Phase R3b）: planner の試行が拒否されて「もう一度だけ試します」が約束された
        // （人の replan の依頼・途中確認 / 承認の replan は 1 回目の `Transitioned{to: running}` で消費済み）。起点を
        // 問わず 2 回目の試行を起こす（この replan は 1 回目で `replan_gate` を通っている）。
        if task_ops::tree::planner_retry_pending(&events) {
            return Ok(WuDispatchGate::RunPlanner { replan: true });
        }
        // ADR-0079 付記「R6-1」D3: 人が起こした replan（decompose・計画の承認の replan・決定への replan の回答・
        // 途中確認の replan・replan の上限を使い切った後の質問への回答）は `max_replans` に数えず、常に受ける
        // （以前は上限を使い切っていると黙って捨て、今の版の unit を進めていた。21:22Z の P-R5b-4 の再現では
        // 承認されていない版の unit がそのまま子を作った）。木の上限（`max_tree_replans`）は dispatch の
        // `tree_run_limit_hold` が今どおり見る。
        if task_ops::regate::pending_replan_request(&events).is_some()
            || task_ops::phase_gate::last_transition_reason(&events)
                == Some(task_core::PhaseResumeMode::Replan.name())
            || task_ops::plan_gate::answered_replan_exhausted(&events)
        {
            return Ok(WuDispatchGate::RunPlanner { replan: true });
        }
        // ADR-0079 付記「R6-1」D1: PlanGate を通っていない版の unit は起こさない（承認待ちのまま人が replan を
        // 求めた後、planner の run が済んで次の版が採用・承認されるまで）。
        if let Some(task) = self.store.get(task_id)?
            && let Some(why) = self.human_gate_hold(&task, &active_plan, &events)
        {
            tracing::debug!(%task_id, hold = why, "a human gate holds the plan's units; not dispatching (ADR-0079 R6-1)");
            return Ok(WuDispatchGate::Skip);
        }
        // ADR-0074 D1.3（Phase F2b）: v2 の計画は工程ごとの scheduler（`settle_phase` /
        // `runnable_work_units`）で決める。v1 は従来どおり（`next_work_unit`）。
        // ADR-0079（Phase R1b）: /3 も段階ごとの scheduler（`internal_view` で /2 の工程と同じ行）。
        let v2 = task_core::is_phased_schema(&active_plan.spec.schema);
        let mut units = self.store.work_units_for(task_id)?;
        // ADR-0074 D3.7（Phase F4b (f)）: `child:<key>` の依存を子 Task の状態で決定的に解く。
        let (changed, waiting_on_children) = self.resolve_child_dependencies(task_id, &units)?;
        if changed {
            units = self.store.work_units_for(task_id)?;
        }
        // ADR-0079 付記「R6-1」D2: 人の gate の間に上げなかった unit（段階の `review: human` の後の次の段階、
        // 止めている間に子が終わって依存が満たされた unit）を、gate が解けた今 `ready` に上げる。
        if v2 && self.promote_newly_ready(task_id, &units)? {
            units = self.store.work_units_for(task_id)?;
        }
        // ADR-0079 付記「R7-9」D3: 統合済みの段階に統合されていない unit（修正前の replan が足した unit など）が
        // 残っていれば、その段階の統合 WU を `pending` に戻す（計画の完了を見る前。下の段階の scheduler が統合を返す）。
        if v2 && self.reopen_stale_stage_integrations(task_id, &units)? {
            units = self.store.work_units_for(task_id)?;
        }
        if waiting_on_children
            && !units.iter().any(|u| {
                matches!(
                    u.status,
                    task_core::WorkUnitStatus::Ready
                        | task_core::WorkUnitStatus::NeedsContinuation
                        | task_core::WorkUnitStatus::Running
                )
            })
        {
            // 進められる WU は子の完了待ちのものだけ（Task は ready のまま待つ）。
            return Ok(WuDispatchGate::Skip);
        }
        let stuck = if v2 {
            matches!(
                crate::execution_scheduler::settle_phase(&units),
                crate::execution_scheduler::PhaseSettle::Question(_)
                    | crate::execution_scheduler::PhaseSettle::Failure(_)
            )
        } else {
            matches!(
                task_core::next_work_unit(&units),
                task_core::NextStep::Stuck(_)
            )
        };
        // ADR-0072 D18（Phase E2）/ D17（Phase E4）: 人の回答（`Trigger::Answer` で Task が
        // `Blocked` から `Ready` に戻った）で、`blocked(question|limit)` の WU を再開する（窓は 0 に
        // 戻る。`Ready`/`NeedsContinuation` が無い = `next_work_unit` が `Stuck` を返すときだけ試す）。
        // E4: `Continue{why: Replan}`（WU の failed/limit から replan する。下）でも Task は
        // `running → ready` に戻るので、**直前の `Transitioned.reason` が実際に `"answer"` のとき
        // だけ**再開する（`replan` を誤って人の回答扱いにしない）。
        if stuck {
            let just_answered = self
                .store
                .events_for(task_id)?
                .iter()
                .rev()
                .find_map(|(_, e)| match e {
                    Event::Transitioned { reason, .. } => Some(reason.clone()),
                    _ => None,
                })
                == Some("answer".to_string());
            if just_answered {
                let resumable: Vec<task_core::WorkUnitRow> = units
                    .iter()
                    .filter(|u| {
                        u.status == task_core::WorkUnitStatus::Blocked
                            && matches!(
                                u.blocked_reason,
                                Some(task_core::WorkUnitBlockedReason::Question)
                                    | Some(task_core::WorkUnitBlockedReason::Limit)
                                    // ADR-0072 D17 3.（Phase E4b 項目2）: replan の上限を使い切った
                                    // plan_issue も、人の回答で（Question と同じく `Ready` から
                                    // やり直す形で）再開できる。
                                    | Some(task_core::WorkUnitBlockedReason::PlanIssue)
                            )
                    })
                    .cloned()
                    .collect();
                for wu in resumable {
                    let resumed = crate::execution_scheduler::resume_after_answer(&wu);
                    self.store.work_unit_transition(
                        task_id,
                        resumed.clone(),
                        Event::WorkUnitTransitioned {
                            work_unit_id: wu.id.clone(),
                            key: wu.key.clone(),
                            from: task_core::WorkUnitStatus::Blocked,
                            to: resumed.status,
                            reason: "answer".to_string(),
                            run_id: None,
                        },
                    )?;
                }
                units = self.store.work_units_for(task_id)?;
            }
        }
        // ADR-0074「F5-fix8 実装時の明確化」: 仕事の残っていない計画（WU がすべて done、または有効な WU が
        // 1 つも無い）。v1 / v2 / v3 共通。採用の後にまだ最終レビューを受けていない版なら最終レビューへ、
        // 不合格の後なら従来どおり replan（D17 4.）。以前は採用の直後でも replan に回り、`max_replans` を
        // 使い切っていると `Skip` のまま黙って止まっていた（F5-fix8 の事故）。
        if task_core::plan_work_finished(&units) {
            return self.finished_plan_gate(task_id, &active_plan.id, &events);
        }
        if v2 {
            return self.wu_dispatch_gate_v2(task_id, units);
        }
        match task_core::next_work_unit(&units) {
            task_core::NextStep::RunWorkUnit(id) => {
                match units.into_iter().find(|u| u.id == id) {
                    Some(wu) => Ok(WuDispatchGate::RunWorkUnit(Box::new(wu))),
                    // 理論上到達しない（`next_work_unit` は `units` の中の id しか返さない）。
                    None => Ok(WuDispatchGate::Skip),
                }
            }
            // ADR-0072 D17（Phase E4）: 正常完了（`plan_complete`）は `on_worker_finished` が即座に
            // `WorkerDone` へ遷移させるので、`Ready` の Task をこの状態（全 WU done）で見るのは、
            // repair の上限を使い切った後の `ReviewFail`、または実質的な review 不合格の後の再
            // dispatch（D17 4.）だけ（理論上の一瞬の不整合を除く）。replan の余地があれば試す。
            task_core::NextStep::AllDone => self.replan_gate(task_id),
            task_core::NextStep::RunPlanner { replan } => Ok(WuDispatchGate::RunPlanner { replan }),
            task_core::NextStep::Stuck(reason) => {
                // ADR-0072 D17（Phase E4）/ D17 3.（Phase E4b 項目2）: WU が failed、または
                // blocked(dependency_failed/limit/plan_issue) のままで進められる WU が無いなら
                // replan の対象（D17 1./2./3.）。plan_issue は通常この分岐に来る前に即
                // `Continue{why: Replan}` で Ready に戻るので、ここに残るのは replan の上限を
                // 使い切った直後の一瞬（`replan_gate` が `Skip` を返す）だけ。
                let has_unresolved_failure = units.iter().any(|u| {
                    u.status == task_core::WorkUnitStatus::Failed
                        || (u.status == task_core::WorkUnitStatus::Blocked
                            && matches!(
                                u.blocked_reason,
                                Some(task_core::WorkUnitBlockedReason::DependencyFailed)
                                    | Some(task_core::WorkUnitBlockedReason::Limit)
                                    | Some(task_core::WorkUnitBlockedReason::PlanIssue)
                            ))
                });
                if has_unresolved_failure {
                    self.replan_gate(task_id)
                } else {
                    tracing::warn!(task_id = %task_id, %reason, "execution plan stuck; not dispatching this tick");
                    Ok(WuDispatchGate::Skip)
                }
            }
        }
    }

    /// ADR-0079 付記「R6-1」D2: 依存と工程の障壁が満たされた `pending` の unit を `ready` に上げる
    /// （`WorkUnitTransitioned{dependency_ready}`。`finish_phase_integration` と同じ）。人の gate の間は上げずに
    /// おくので、gate が解けた後の最初の dispatch で上げる。上げたら `true`。
    pub(super) fn promote_newly_ready(
        &self,
        task_id: TaskId,
        units: &[task_core::WorkUnitRow],
    ) -> Result<bool, DispatchError> {
        let ids = task_core::newly_ready(units);
        for id in &ids {
            let Some(u) = units.iter().find(|u| &u.id == id) else {
                continue;
            };
            let mut row = u.clone();
            row.status = task_core::WorkUnitStatus::Ready;
            row.updated_at = rfc3339(OffsetDateTime::now_utc());
            self.store.work_unit_transition(
                task_id,
                row,
                Event::WorkUnitTransitioned {
                    work_unit_id: u.id.clone(),
                    key: u.key.clone(),
                    from: task_core::WorkUnitStatus::Pending,
                    to: task_core::WorkUnitStatus::Ready,
                    reason: "dependency_ready".to_string(),
                    run_id: None,
                },
            )?;
        }
        Ok(!ids.is_empty())
    }

    /// ADR-0079 付記「R7-9」D3: [`task_core::stale_stage_integrations`] に当たる done の統合 WU を
    /// [`task_core::reopened_integration`]（依存に足りない key を足して `pending`）に戻し、
    /// `WorkUnitTransitioned{from: done, to: pending, reason: "stage_reopened"}` を積む。戻したら `true`。
    pub(super) fn reopen_stale_stage_integrations(
        &self,
        task_id: TaskId,
        units: &[task_core::WorkUnitRow],
    ) -> Result<bool, DispatchError> {
        let stale = task_core::stale_stage_integrations(units);
        for (id, missing) in &stale {
            let Some(integ) = units.iter().find(|u| &u.id == id) else {
                continue;
            };
            let mut row = task_core::reopened_integration(integ, missing);
            row.updated_at = rfc3339(OffsetDateTime::now_utc());
            tracing::info!(
                %task_id,
                work_unit = %integ.key,
                missing = %missing.join(","),
                "an integrated stage has units that were never merged; reopening its integration (ADR-0079 R7-9)"
            );
            self.store.work_unit_transition(
                task_id,
                row,
                Event::WorkUnitTransitioned {
                    work_unit_id: integ.id.clone(),
                    key: integ.key.clone(),
                    from: task_core::WorkUnitStatus::Done,
                    to: task_core::WorkUnitStatus::Pending,
                    reason: task_core::STAGE_REOPENED_REASON.to_string(),
                    run_id: None,
                },
            )?;
        }
        Ok(!stale.is_empty())
    }

    /// ADR-0074 D3.7（Phase F4b (f)）: `depends_on: ["child:<key>"]` の WU を、子 Task（`child-<key>` の
    /// 印を持つ `parent_id = task_id` の Task）の状態で進める。子が `done` なら（他の依存も満たされて
    /// いれば）`pending → ready`、子が `failed` / `cancelled` なら `pending → blocked(dependency_failed)`
    /// （D17 の replan の対象）。戻り値は（書き換えたか、まだ終わっていない子を待っている WU があるか）。
    pub(super) fn resolve_child_dependencies(
        &self,
        task_id: TaskId,
        units: &[task_core::WorkUnitRow],
    ) -> Result<(bool, bool), DispatchError> {
        let prefix = task_core::CHILD_DEP_PREFIX;
        let has_child_deps =
            |u: &task_core::WorkUnitRow| u.depends_on.iter().any(|d| d.starts_with(prefix));
        if !units
            .iter()
            .any(|u| u.status == task_core::WorkUnitStatus::Pending && has_child_deps(u))
        {
            return Ok((false, false));
        }
        let mut done: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut failed: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for child in self.store.children(task_id)? {
            for label in &child.labels {
                let Some(key) = label.strip_prefix("child-") else {
                    continue;
                };
                let dep = format!("{prefix}{key}");
                match child.status {
                    Status::Done => {
                        done.insert(dep);
                    }
                    Status::Failed | Status::Cancelled => {
                        failed.insert(dep);
                    }
                    _ => {}
                }
            }
        }
        let mut changed = false;
        for u in units.iter().filter(|u| {
            u.status == task_core::WorkUnitStatus::Pending
                && u.depends_on.iter().any(|d| failed.contains(d))
        }) {
            let mut row = u.clone();
            row.status = task_core::WorkUnitStatus::Blocked;
            row.blocked_reason = Some(task_core::WorkUnitBlockedReason::DependencyFailed);
            self.store.work_unit_transition(
                task_id,
                row,
                Event::WorkUnitTransitioned {
                    work_unit_id: u.id.clone(),
                    key: u.key.clone(),
                    from: task_core::WorkUnitStatus::Pending,
                    to: task_core::WorkUnitStatus::Blocked,
                    reason: "dependency_failed".to_string(),
                    run_id: None,
                },
            )?;
            changed = true;
        }
        if !changed {
            for id in task_core::newly_ready_with(units, &done) {
                let Some(u) = units.iter().find(|u| u.id == id) else {
                    continue;
                };
                if !has_child_deps(u) {
                    continue;
                }
                let mut row = u.clone();
                row.status = task_core::WorkUnitStatus::Ready;
                self.store.work_unit_transition(
                    task_id,
                    row,
                    Event::WorkUnitTransitioned {
                        work_unit_id: u.id.clone(),
                        key: u.key.clone(),
                        from: task_core::WorkUnitStatus::Pending,
                        to: task_core::WorkUnitStatus::Ready,
                        reason: "child_done".to_string(),
                        run_id: None,
                    },
                )?;
                changed = true;
            }
        }
        let waiting = units.iter().any(|u| {
            u.status == task_core::WorkUnitStatus::Pending
                && u.depends_on
                    .iter()
                    .any(|d| d.starts_with(prefix) && !done.contains(d) && !failed.contains(d))
        });
        Ok((changed, waiting))
    }

    /// ADR-0074 D1.3/D1.6（Phase F2b）: v2 の計画の Ready な Task が次に何をするか。
    pub(super) fn wu_dispatch_gate_v2(
        &self,
        task_id: TaskId,
        units: Vec<task_core::WorkUnitRow>,
    ) -> Result<WuDispatchGate, DispatchError> {
        use crate::execution_scheduler::PhaseSettle;
        match crate::execution_scheduler::settle_phase(&units) {
            PhaseSettle::Integrate(id) => Ok(units
                .into_iter()
                .find(|u| u.id == id)
                .map(|u| WuDispatchGate::StartIntegration(Box::new(u)))
                .unwrap_or(WuDispatchGate::Skip)),
            // 全部 done（最終レビューの不合格の後など。v1 の `AllDone` と同じ）・工程の失敗は replan。
            PhaseSettle::AllDone | PhaseSettle::Failure(_) => self.replan_gate(task_id),
            PhaseSettle::Question(_) | PhaseSettle::Wait => Ok(WuDispatchGate::Skip),
            PhaseSettle::Advance => {
                let Some(task) = self.store.get(task_id)? else {
                    return Ok(WuDispatchGate::Skip);
                };
                let mode = self.parallel_mode(&task)?;
                let ids = crate::execution_scheduler::runnable_in_phase(&units, 0, mode.limit);
                Ok(ids
                    .first()
                    .and_then(|id| units.into_iter().find(|u| &u.id == id))
                    .map(|u| WuDispatchGate::RunWorkUnit(Box::new(u)))
                    .unwrap_or(WuDispatchGate::Skip))
            }
        }
    }

    /// ADR-0074 D1.2（Phase F2b）: v2 の Task を並列でどう走らせるか（決定的）。v1・atomic・計画の
    /// 無い Task は並列 1（WU の worktree なし）。remote / `Shared` / 書き込み可能な `dir` の repo /
    /// git の worktree が無い Task は並列 1 に倒し、理由を返す。
    pub(super) fn parallel_mode(&self, task: &Task) -> Result<ParallelMode, DispatchError> {
        let serial = |reason: Option<String>| ParallelMode {
            limit: 1,
            worktrees: false,
            fallback: reason,
        };
        let Some(plan) = self.store.execution_plan_active(task.id)? else {
            return Ok(serial(None));
        };
        if !task_core::is_phased_schema(&plan.spec.schema) {
            return Ok(serial(None));
        }
        match &task.workspace {
            WorkspaceSpec::Remote { .. } => {
                return Ok(serial(Some(
                    "remote workspace: no work unit worktrees on the cluster (ADR-0074 D1.2)"
                        .to_string(),
                )));
            }
            WorkspaceSpec::Local {
                mode: Some(WorkspaceMode::Shared),
                ..
            } => {
                return Ok(serial(Some(
                    "workspace_mode = shared: work units share the task's directory (ADR-0074 D1.2)"
                        .to_string(),
                )));
            }
            WorkspaceSpec::Local { .. } => {}
        }
        let Some(ws) = self.task_workspaces_for(task) else {
            return Ok(serial(Some(
                "no git worktree for this task (the local path is not a git repository)"
                    .to_string(),
            )));
        };
        if ws.repos.is_empty() {
            return Ok(serial(Some(
                "no repository in this task's workspace".to_string(),
            )));
        }
        if let Some(repo) = ws.repos.iter().find(|r| !r.is_git()) {
            return Ok(serial(Some(format!(
                "repository {} is a writable dir (not git): work units would share it (ADR-0074 D1.2)",
                repo.name
            ))));
        }
        Ok(ParallelMode {
            limit: self
                .config
                .execution
                .max_parallel_work_units
                .clamp(1, MAX_PARALLEL_WORK_UNITS_CAP),
            worktrees: true,
            fallback: None,
        })
    }

    /// ADR-0074 D1.2（Phase F2b）: 並列 1 に倒した理由を計画ごとに 1 回だけ残す。
    pub(super) fn record_serialized(&self, task_id: TaskId, plan_id: &str, reason: &str) {
        let already = self.store.events_for(task_id).is_ok_and(|events| {
            events.iter().any(
                |(_, e)| matches!(e, Event::WorkUnitsSerialized { plan_id: p, .. } if p == plan_id),
            )
        });
        if already {
            return;
        }
        if let Err(e) = self.store.append_event(
            task_id,
            &Event::WorkUnitsSerialized {
                plan_id: plan_id.to_string(),
                reason: reason.to_string(),
            },
        ) {
            tracing::warn!(%task_id, error = %e, "failed to record why the work units run serially");
        }
    }

    /// ADR-0074 D1.2（Phase F2b）: WU の run の worktree を用意する（冪等）。
    /// - 並列 1 に倒した Task（理由を記録）・統合の repair WU（Task の worktree で走る）は `Ok(None)`。
    /// - 基点は、既にブランチがあればそれを使い回し、無ければ同じ工程の依存先の WU ブランチの HEAD
    ///   （積み上げ）か Task ブランチの HEAD。依存先の WU ブランチが無ければ
    ///   `integration::dependency_base`（F5-fix7: 依存先の記録した commit か Task ブランチの HEAD）。
    pub(super) fn prepare_work_unit_workspace(
        &self,
        task: &Task,
        wu: &task_core::WorkUnitRow,
        task_ws: Option<&task_worker::TaskWorkspaces>,
    ) -> Result<Option<WorkUnitWorkspace>, WuPrepareError> {
        let mode = self
            .parallel_mode(task)
            .map_err(|e| WuPrepareError::transient(e.to_string()))?;
        if let Some(reason) = &mode.fallback {
            self.record_serialized(task.id, &wu.plan_id, reason);
            return Ok(None);
        }
        if !mode.worktrees || wu.kind == task_core::WorkUnitKind::Repair {
            return Ok(None);
        }
        let Some(ws) = task_ws else {
            return Ok(None);
        };
        // Task の worktree（基点のブランチ）を先に用意する。
        for repo in &ws.repos {
            if let Some(wt) = &repo.worktree {
                wt.ensure_blocking().map_err(|e| {
                    WuPrepareError::transient(format!("cannot prepare the task worktree: {e}"))
                })?;
            }
        }
        let units = self
            .store
            .work_units_for(task.id)
            .map_err(|e| WuPrepareError::transient(e.to_string()))?;
        let intra_dep = wu.depends_on.iter().find_map(|d| {
            units
                .iter()
                .find(|u| &u.key == d && u.phase.is_some() && u.phase == wu.phase)
        });
        let task_id = task.id.to_string();
        let branch = crate::integration::wu_branch(&task_id, &wu.key);
        let wu_dir = crate::integration::wu_dir(&ws.task_dir, &wu.key);
        let mut repos = Vec::new();
        let mut first_base: Option<String> = None;
        for repo in &ws.repos {
            let Some(task_wt) = &repo.worktree else {
                continue;
            };
            let existing =
                crate::integration::rev_parse(&repo.source, &format!("refs/heads/{branch}"));
            let base = match (&existing, intra_dep) {
                (Some(_), _) => wu
                    .base_commit
                    .clone()
                    .or_else(|| existing.clone())
                    .unwrap_or_default(),
                // ADR-0074「Phase F5-fix7 実装時の明確化」: 依存先の WU ブランチが無い（Task の worktree で
                // 走った repair / 統合 WU、ref が消えた）ときは、依存先の記録した commit か Task ブランチの
                // HEAD に倒す（`integration::dependency_base`）。解決できなければ時間では直らない。
                (None, Some(dep_row)) => crate::integration::dependency_base(
                    &repo.source,
                    &task_id,
                    dep_row,
                    &task_wt.branch,
                    &self.config.worktree_branch_prefix,
                )
                .map_err(WuPrepareError::permanent)?,
                (None, None) => crate::integration::rev_parse(
                    &repo.source,
                    &format!("refs/heads/{}", task_wt.branch),
                )
                .ok_or_else(|| {
                    WuPrepareError::permanent(format!(
                        "task branch {} does not exist",
                        task_wt.branch
                    ))
                })?,
            };
            let lwt = crate::integration::wu_worktree(
                &ws.task_dir,
                &task_id,
                &wu.key,
                &repo.name,
                &repo.source,
                &base,
            );
            crate::integration::ensure_wu_worktree(&lwt).map_err(WuPrepareError::transient)?;
            first_base.get_or_insert(base);
            repos.push(task_worker::TaskRepo::git(repo.name.clone(), lwt));
        }
        if repos.is_empty() {
            return Ok(None);
        }
        Ok(Some(WorkUnitWorkspace {
            workspaces: task_worker::TaskWorkspaces {
                task_dir: wu_dir.clone(),
                repos,
            },
            branch,
            base: wu.base_commit.clone().or(first_base).unwrap_or_default(),
            artifacts_dir: wu_dir.join(task_core::artifacts::ARTIFACTS_DIR_NAME),
        }))
    }

    /// ADR-0074「Phase F5-fix7 実装時の明確化」: WU の worktree を用意できなかった。黙って tick ごとに
    /// やり直し続けない（本番 2026-09-28 の 20 分の停止）:
    /// - 一時的な失敗は [`MAX_WU_PREPARE_ATTEMPTS`] 回まで [`wu_prepare_backoff`] で待ってやり直す。
    /// - 時間で直らない失敗（依存先の成果が解決できない等）と、上限を使い切った一時的な失敗は、WU を
    ///   `blocked(question)`（`WorkUnitTransitioned{reason: "prepare_failed"}`）にし、理由を
    ///   `worker_progress` に残す。Task が Ready（1 本目）なら `approvals` に 1 件作って `Trigger::Unroutable`
    ///   で `ready → blocked`（`QuestionRaised`。ADR-0062 B1 の `block_task_missing_cluster_tool` と同じ出口）。
    ///   人が直して回答すると、D18 の `answer` の経路でこの WU が ready に戻り、もう一度用意を試す。
    ///   並列の 2 本目以降（Task は Running）は WU だけ blocked にし、兄弟の run が終わったときの
    ///   `settle_phase` → `Question` が Task を blocked にする。
    pub(super) fn on_work_unit_prepare_failed(
        &mut self,
        task: &Task,
        wu: &task_core::WorkUnitRow,
        error: &WuPrepareError,
        second_pass: bool,
    ) -> Result<(), DispatchError> {
        let now = self.now_utc();
        if !error.permanent {
            let count = self
                .wu_prepare_failures
                .get(&wu.id)
                .map_or(0, |f| f.count)
                .saturating_add(1);
            if count < MAX_WU_PREPARE_ATTEMPTS {
                let retry_at = now + wu_prepare_backoff(count);
                self.wu_prepare_failures
                    .insert(wu.id.clone(), WuPrepareFailures { count, retry_at });
                tracing::warn!(task_id = %task.id, work_unit = %wu.key, error = %error, attempt = count, max_attempts = MAX_WU_PREPARE_ATTEMPTS, %retry_at, "cannot prepare the work unit worktree (transient); retrying after a backoff");
                return Ok(());
            }
        }
        self.wu_prepare_failures.remove(&wu.id);
        tracing::warn!(task_id = %task.id, work_unit = %wu.key, error = %error, permanent = error.permanent, "cannot prepare the work unit worktree; blocking the work unit and asking a human (reason=prepare_failed)");
        let mut row = wu.clone();
        row.status = task_core::WorkUnitStatus::Blocked;
        row.blocked_reason = Some(task_core::WorkUnitBlockedReason::Question);
        row.clear_lease();
        row.updated_at = rfc3339(now);
        self.store.work_unit_transition(
            task.id,
            row,
            Event::WorkUnitTransitioned {
                work_unit_id: wu.id.clone(),
                key: wu.key.clone(),
                from: wu.status,
                to: task_core::WorkUnitStatus::Blocked,
                reason: "prepare_failed".to_string(),
                run_id: None,
            },
        )?;
        let question = format!(
            "WorkUnit `{}` の作業場所（git worktree）を用意できないため、この WU を止めました: {}。\
             依存先のブランチ・Task ブランチ・git の状態を直してから回答すると、この WU の用意をやり直します。",
            wu.key, error.message
        );
        let progress = Event::worker_progress(
            "prepare",
            format!("prepare_failed: work unit {}: {}", wu.key, error.message),
        );
        if second_pass {
            self.store.append_event(task.id, &progress)?;
            return Ok(());
        }
        if let Err(e) = crate::approvals::record_question_approval(
            self.store.as_ref(),
            task,
            &question,
            OffsetDateTime::now_utc(),
        ) {
            tracing::warn!(task_id = %task.id, error = %e, "failed to record the approval for the work unit prepare failure");
        }
        let events = vec![
            progress,
            Event::QuestionRaised {
                run_id: format!("wu-prepare-{}", wu.id),
                text: question,
            },
        ];
        match self
            .store
            .apply_transition_with_events(task.id, Trigger::Unroutable, events)
        {
            Ok(_) => Ok(()),
            Err(StoreError::InvalidTransition(e)) => {
                tracing::warn!(task_id = %task.id, error = %e, "prepare-failed transition could not be applied");
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    /// ADR-0074 D1.2（Phase F2b）: v2 の WU が作業したツリー（`(dir, branch)`。repo ごと）。WU の
    /// worktree を持つ WU はそのツリー、統合の repair WU のように Task の worktree で走った WU は Task の
    /// worktree。並列 1 に倒した Task・v1 は空（daemon は commit しない）。
    pub(super) fn work_unit_trees(
        &self,
        task: &Task,
        wu: &task_core::WorkUnitRow,
    ) -> Result<Vec<(PathBuf, String)>, DispatchError> {
        if wu.phase.is_none() {
            return Ok(Vec::new());
        }
        let Some(ws) = self.task_workspaces_for(task) else {
            return Ok(Vec::new());
        };
        if let Some(branch) = &wu.branch {
            let wu_dir = crate::integration::wu_dir(&ws.task_dir, &wu.key);
            return Ok(ws
                .repos
                .iter()
                .filter(|r| r.is_git())
                .map(|r| {
                    (
                        wu_dir
                            .join(task_worker::task_repos::REPOS_DIR_NAME)
                            .join(&r.name),
                        branch.clone(),
                    )
                })
                .collect());
        }
        if !self.parallel_mode(task)?.worktrees {
            return Ok(Vec::new());
        }
        Ok(ws
            .repos
            .iter()
            .filter_map(|r| r.branch().map(|b| (r.dir.clone(), b.to_string())))
            .collect())
    }

    /// ADR-0074 D1.2（Phase F2b）: WU の run が done になったら、その作業ツリーで決定的に commit する
    /// （作者は celeris の固定値。変更が無ければ commit しない）。`WorkUnitCommitted` を返し、
    /// `wu.head_commit` を書き換える。ツリーが無ければ（並列 1・v1）`None`。
    pub(super) fn commit_work_unit(
        &self,
        task: &Task,
        wu: &mut task_core::WorkUnitRow,
    ) -> Result<Option<Event>, DispatchError> {
        let trees = self.work_unit_trees(task, wu)?;
        let mut first: Option<(String, String)> = None;
        for (dir, branch) in trees {
            if !dir.is_dir() {
                continue;
            }
            match crate::integration::commit_all(
                &dir,
                &crate::integration::commit_message(&wu.key, &wu.spec.title),
            ) {
                Ok((head, _)) => {
                    first.get_or_insert((head, branch));
                }
                Err(e) => {
                    tracing::warn!(task_id = %task.id, work_unit = %wu.key, error = %e, "could not commit the work unit's changes");
                }
            }
        }
        let Some((head, branch)) = first else {
            return Ok(None);
        };
        wu.head_commit = Some(head.clone());
        Ok(Some(Event::WorkUnitCommitted {
            work_unit_id: wu.id.clone(),
            key: wu.key.clone(),
            branch,
            base: wu.base_commit.clone(),
            commit: head,
        }))
    }

    /// ADR-0074 D1.6（Phase F2b）: checkpoint の mechanical な欄を取る場所（`(artifacts_dir, cwd,
    /// branch, base)`）。WU の worktree を持つ v2 の WU は WU の worktree・WU のブランチ・`base_commit`、
    /// それ以外は従来どおり Task の作業場所。
    pub(super) fn work_unit_checkpoint_site(
        &self,
        task: &Task,
        wu: &task_core::WorkUnitRow,
    ) -> (Option<PathBuf>, Option<PathBuf>, String, Option<String>) {
        let workspaces = self.task_workspaces_for(task);
        if let Some(branch) = &wu.branch
            && let Some(ws) = &workspaces
        {
            let wu_dir = crate::integration::wu_dir(&ws.task_dir, &wu.key);
            let cwd = ws.repos.iter().find(|r| r.is_git()).map(|r| {
                wu_dir
                    .join(task_worker::task_repos::REPOS_DIR_NAME)
                    .join(&r.name)
            });
            return (
                Some(wu_dir.join(task_core::artifacts::ARTIFACTS_DIR_NAME)),
                cwd,
                branch.clone(),
                wu.base_commit.clone(),
            );
        }
        let workspace_dir = self.task_dir(task);
        let artifacts_dir = workspace_dir.as_ref().map(|d| self.artifacts_dir(task, d));
        let cwd = workspaces
            .as_ref()
            .and_then(|w| w.cwd())
            .map(Path::to_path_buf);
        let branch = workspaces
            .as_ref()
            .and_then(|w| w.repos.first())
            .and_then(|r| r.branch())
            .unwrap_or_default()
            .to_string();
        // ADR-0079 D6（Phase R1c）: 木の子の差分の基点は `tree.base_commit`（main との merge-base ではない）。
        let base = task_core::tree::child_base_commit(task).map(str::to_string);
        (artifacts_dir, cwd, branch, base)
    }

    /// ADR-0074 D1.6（Phase F2b）: 兄弟が走っている間に question / 失敗で止まった WU（`id`）について、
    /// in-flight が 0 になった今 Task をどう遷移させるか（trigger と人への質問）。
    pub(super) fn deferred_work_unit_trigger(
        &self,
        task_id: TaskId,
        units: &[task_core::WorkUnitRow],
        id: &str,
    ) -> Result<(Trigger, String, Vec<String>), DispatchError> {
        let Some(wu) = units.iter().find(|u| u.id == id) else {
            return Ok((
                Trigger::Continue {
                    why: task_core::ContinueWhy::Advance,
                },
                String::new(),
                Vec::new(),
            ));
        };
        let events = self.store.events_for(task_id)?;
        let last_outcome = wu
            .last_run_id
            .as_deref()
            .and_then(|rid| {
                events.iter().rev().find_map(|(_, e)| match e {
                    Event::WorkerFinished {
                        run_id, outcome, ..
                    } if run_id == rid => Some(outcome.clone()),
                    _ => None,
                })
            })
            .unwrap_or_default();
        let question_text = last_outcome
            .strip_prefix("question: ")
            .unwrap_or(last_outcome.as_str())
            .to_string();
        if wu.status == task_core::WorkUnitStatus::Blocked
            && wu.blocked_reason == Some(task_core::WorkUnitBlockedReason::Question)
        {
            let text = if question_text.is_empty() {
                format!("WorkUnit {} が質問しています", wu.key)
            } else {
                question_text
            };
            return Ok((
                Trigger::WorkerQuestion,
                format!("question: {text}"),
                vec![text],
            ));
        }
        let replans_so_far = self.counted_replans(task_id)?;
        if replans_so_far < self.effective_max_replans(task_id)? {
            let why = match (wu.status, wu.blocked_reason) {
                (task_core::WorkUnitStatus::Failed, _) => format!("work unit {} failed", wu.key),
                (_, Some(task_core::WorkUnitBlockedReason::Limit)) => {
                    format!("work unit {} made no progress", wu.key)
                }
                (_, Some(task_core::WorkUnitBlockedReason::PlanIssue)) => {
                    format!("work unit {} reported a plan issue", wu.key)
                }
                _ => format!("work unit {} is blocked by a failed dependency", wu.key),
            };
            return Ok((
                Trigger::Continue {
                    why: task_core::ContinueWhy::Replan,
                },
                format!("replan: {why}"),
                Vec::new(),
            ));
        }
        if matches!(wu.status, task_core::WorkUnitStatus::Failed)
            || wu.blocked_reason == Some(task_core::WorkUnitBlockedReason::DependencyFailed)
        {
            // ADR-0079 付記「R6-1」D3: replan を使い切っても `failed` にせず人に聞く（木の節点は決定の要求）。
            let Some(task) = self.store.get(task_id)? else {
                return Ok((
                    Trigger::WorkerError { retryable: false },
                    format!("error(retryable=false): work unit {} failed", wu.key),
                    Vec::new(),
                ));
            };
            let (trigger, outcome) = self.replan_exhausted_ask(
                &task,
                &format!("work unit {} failed", wu.key),
                replans_so_far,
            )?;
            let questions = match outcome.strip_prefix("question: ") {
                Some(q) => {
                    // この run の `WorkerFinished` は兄弟の run の終わり方なので、質問の本文は別に残す
                    // （受信箱の質問文と、回答を人の replan と読む `answered_replan_exhausted` のため）。
                    self.store.append_event(
                        task_id,
                        &Event::QuestionRaised {
                            run_id: last_run_id(&events).unwrap_or_default(),
                            text: q.to_string(),
                        },
                    )?;
                    vec![q.to_string()]
                }
                None => Vec::new(),
            };
            return Ok((trigger, outcome, questions));
        }
        let text = if question_text.is_empty() {
            format!(
                "WorkUnit {} が進みません。続け方を指示してください。",
                wu.key
            )
        } else {
            question_text
        };
        Ok((
            Trigger::WorkerQuestion,
            format!("question: {text}"),
            vec![text],
        ))
    }

    /// ADR-0074 D1.7（Phase F2b）: tick の最初に、v2（工程の lease）の Task を照合する。
    /// - WU が running で、WU の lease が切れていて、このインスタンスの run でもない → 戻す。
    /// - Task が Running で、WU も統合も走っていない → `Continue{advance}`（Ready に戻して通常の経路へ）。
    ///
    /// Task の lease ごと切れた（全部が死んだ）Task は `reclaim_expired_leases` → `InfraRequeue` が扱う。
    ///
    /// Phase F5-fix6: WU の lease がまだ切れていなくても、その run の持ち主のデーモンが居なければ
    /// （孤児。`crate::orphan`）同じく戻す（result.json があればその内容で確定、無ければ reason
    /// `orphan_takeover` で ready / needs_continuation）。統合 WU も同じ（spawn が手元に無ければ pending）。
    pub(super) fn reconcile_parallel_tasks(&mut self) -> Result<(), DispatchError> {
        let now = OffsetDateTime::now_utc();
        let mut holders_gone: Option<bool> = None;
        for task in self.store.list(Some(Status::Running))? {
            if !self.is_eligible(&task) {
                continue;
            }
            let Some(lease) = &task.lease else { continue };
            if !is_phase_lease_holder(&lease.worker_run_id) || lease.expires_at <= now {
                continue;
            }
            let units = self.store.work_units_for(task.id)?;
            for u in units.iter().filter(|u| {
                u.status == task_core::WorkUnitStatus::Running
                    && u.kind != task_core::WorkUnitKind::Integrate
            }) {
                let Some(run_id) = u.lease_run_id.clone().or_else(|| u.last_run_id.clone()) else {
                    continue;
                };
                // Phase F5-fix2: 検査中の run（`checking`）も手元の run。
                let ours = self.running.values().any(|e| e.run_id == run_id)
                    || self.checking.contains_key(&run_id);
                let expired = u
                    .lease_expires_at
                    .as_deref()
                    .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
                    .is_none_or(|t| t <= now);
                if ours {
                    continue;
                }
                if expired {
                    if self.finalise_from_result_json(&task, &run_id) {
                        continue;
                    }
                    tracing::warn!(task_id = %task.id, work_unit = %u.key, %run_id, "work unit lease expired without a live run; reconciling (ADR-0074 D1.7)");
                    self.reconcile_work_unit_run(task.id, &run_id, "restart_reconcile")?;
                } else if self.lease_holders_gone(&mut holders_gone, now) {
                    // Phase F5-fix6: 持ち主のデーモンが居ない WU の run。lease の失効を待たない。
                    let until = u
                        .lease_expires_at
                        .as_deref()
                        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
                        .unwrap_or(now);
                    self.note_orphan_takeover(&task, &run_id, until);
                    if self.finalise_from_result_json(&task, &run_id) {
                        continue;
                    }
                    self.requeue_orphaned_work_unit_run(&task, &run_id)?;
                }
            }
            // Phase F5-fix6: 持ち主の居ない工程の統合（統合 WU が running で、spawn が手元に無い）。
            if !self.integrating.contains_key(&task.id)
                && self.running_for_task(task.id) == 0
                && units.iter().any(|u| {
                    u.kind == task_core::WorkUnitKind::Integrate
                        && u.status == task_core::WorkUnitStatus::Running
                })
                && self.lease_holders_gone(&mut holders_gone, now)
            {
                tracing::warn!(task_id = %task.id, "the phase integration's daemon is gone; returning the integration to pending without waiting for the lease (Phase F5-fix6 orphan_takeover)");
                self.reconcile_integration(task.id, crate::orphan::ORPHAN_TAKEOVER_REASON)?;
            }
            if self.running_for_task(task.id) > 0 || self.integrating.contains_key(&task.id) {
                continue;
            }
            let units = self.store.work_units_for(task.id)?;
            // ADR-0079 D5（Phase R1b）: kind task の unit の `running` は子 task の写し（この Task の run ではない）。
            if units.iter().any(|u| {
                u.status == task_core::WorkUnitStatus::Running
                    && u.kind != task_core::WorkUnitKind::Task
            }) {
                continue;
            }
            tracing::warn!(task_id = %task.id, "running v2 task has no work unit or integration in flight; returning it to ready (ADR-0074 D1.7)");
            match self.store.apply_transition_with_events(
                task.id,
                Trigger::Continue {
                    why: task_core::ContinueWhy::Advance,
                },
                vec![],
            ) {
                Ok(_) | Err(StoreError::InvalidTransition(_)) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    /// ADR-0074 D1.6（Phase F2b）/ ADR-0072 D6: Cancel（取り下げ）で、未完了の WU を cancelled にし、
    /// WU の worktree とブランチを消す（ADR-0043 D2 の中止の規則）。
    pub(super) fn cancel_open_work_units(&self, task: &Task) -> Result<(), DispatchError> {
        let units = self.store.work_units_for(task.id)?;
        let mut rows = Vec::new();
        let mut events = Vec::new();
        for u in units.iter().filter(|u| !u.status.is_terminal()) {
            let mut row = u.clone();
            row.status = task_core::WorkUnitStatus::Cancelled;
            row.blocked_reason = None;
            row.clear_lease();
            row.updated_at = rfc3339(OffsetDateTime::now_utc());
            events.push(Event::WorkUnitTransitioned {
                work_unit_id: u.id.clone(),
                key: u.key.clone(),
                from: u.status,
                to: task_core::WorkUnitStatus::Cancelled,
                reason: "cancel".to_string(),
                run_id: None,
            });
            rows.push(row);
        }
        if !rows.is_empty() {
            self.store
                .work_units_apply(task.id, Vec::new(), rows, events)?;
        }
        if let Some(ws) = self.task_workspaces_for(task) {
            for u in units.iter().filter(|u| u.branch.is_some()) {
                for repo in ws.repos.iter().filter(|r| r.is_git()) {
                    let lwt = crate::integration::wu_worktree(
                        &ws.task_dir,
                        &task.id.to_string(),
                        &u.key,
                        &repo.name,
                        &repo.source,
                        u.base_commit.as_deref().unwrap_or_default(),
                    );
                    let _ = lwt.remove_with_branch();
                    if let Some(parent) = lwt.dir.parent() {
                        let _ = std::fs::remove_dir(parent);
                    }
                }
            }
        }
        Ok(())
    }

    /// ADR-0074「F5-fix8 実装時の明確化」: 仕事の残っていない計画の次の一手。採用（`ExecutionPlanned`）の後に
    /// 最終レビューの判定がまだ無ければ `FinalReview`、判定の後（不合格で `ready` に戻った）なら `replan_gate`。
    pub(super) fn finished_plan_gate(
        &self,
        task_id: TaskId,
        plan_id: &str,
        events: &[(u64, Event)],
    ) -> Result<WuDispatchGate, DispatchError> {
        if task_core::plan_awaits_final_review(events, plan_id) {
            return Ok(WuDispatchGate::FinalReview);
        }
        self.replan_gate(task_id)
    }

    /// ADR-0072 D17/D18（Phase E4）: replan の余地（`max_replans`）があれば `RunPlanner{replan:
    /// true}`、無ければ `Skip`（進められる WU が無いまま何もしない。呼び出し元が既に上限を見て
    /// `Continue{why: Replan}` を避けていれば通常ここには来ない防御的フォールバック）。
    pub(super) fn replan_gate(&self, task_id: TaskId) -> Result<WuDispatchGate, DispatchError> {
        let replans_so_far = self.counted_replans(task_id)?;
        if replans_so_far < self.effective_max_replans(task_id)? {
            Ok(WuDispatchGate::RunPlanner { replan: true })
        } else {
            // ADR-0079 D9（Phase R2b）: 木の節点では上限の超過を人への決定の要求にする（黙って止まらない）。
            self.raise_node_replan_limit(task_id, replans_so_far)?;
            Ok(WuDispatchGate::Skip)
        }
    }

    /// ADR-0072 D5/D6/D9（Phase E2）: WU の run を始める（行の遷移・`runs` 索引・prompt 文脈）。
    /// `extras.work_unit`/`extras.continuation_override` を書き換える。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_work_unit_run(
        &self,
        task_id: TaskId,
        wu: &task_core::WorkUnitRow,
        run_id: &str,
        adapter_id: &str,
        model: &str,
        account: Option<&str>,
        extras: &mut RunExtras,
        lease_taken: bool,
    ) -> Result<(), DispatchError> {
        let is_continuation = wu.status == task_core::WorkUnitStatus::NeedsContinuation;
        if is_continuation {
            extras.continuation_override = self.work_unit_continuation_context(wu);
        }
        // ADR-0074 D1.5（Phase F2b）: v2 の WU は `acquire_work_unit_lease` が既に running にしている。
        if !lease_taken {
            let mut updated = wu.clone();
            updated.status = task_core::WorkUnitStatus::Running;
            updated.blocked_reason = None;
            updated.runs += 1;
            updated.last_run_id = Some(run_id.to_string());
            updated.updated_at = rfc3339(OffsetDateTime::now_utc());
            self.store.work_unit_transition(
                task_id,
                updated,
                Event::WorkUnitTransitioned {
                    work_unit_id: wu.id.clone(),
                    key: wu.key.clone(),
                    from: wu.status,
                    to: task_core::WorkUnitStatus::Running,
                    reason: "dispatch".to_string(),
                    run_id: Some(run_id.to_string()),
                },
            )?;
        }
        self.store.run_index_start(task_core::RunRow {
            run_id: run_id.to_string(),
            task_id: task_id.to_string(),
            work_unit_id: Some(wu.id.clone()),
            role: task_core::RunIndexRole::Worker,
            seq: wu.runs + 1,
            status: task_core::RunIndexStatus::Running,
            adapter: Some(adapter_id.to_string()),
            model: Some(model.to_string()),
            account: account.map(str::to_string),
            session_id: None,
            checkpoint: None,
            usage: None,
            metrics: None,
            started_at: rfc3339(OffsetDateTime::now_utc()),
            finished_at: None,
        })?;
        let units = self.store.work_units_for(task_id)?;
        extras.work_unit = Some(self.work_unit_prompt_context(task_id, &units, wu, run_id)?);
        Ok(())
    }

    /// ADR-0072 D9（Phase E2）: WU の spec から prompt に渡す文脈を組み立てる（`## Objective` の
    /// 差し替え・計画の一覧・依存する WU の完了要約）。
    pub(super) fn work_unit_prompt_context(
        &self,
        task_id: TaskId,
        units: &[task_core::WorkUnitRow],
        wu: &task_core::WorkUnitRow,
        run_id: &str,
    ) -> Result<task_worker::protocol::WorkUnitPromptContext, DispatchError> {
        let task_objective_excerpt = self
            .store
            .get(task_id)?
            .map(|t| t.objective.chars().take(1500).collect::<String>())
            .unwrap_or_default();
        let plan_overview: Vec<String> = units
            .iter()
            .map(|u| format!("{}: {} [{}]", u.key, u.spec.title, u.status.as_str()))
            .collect();
        let mut dependency_summaries = Vec::new();
        for dep_key in &wu.spec.depends_on {
            let Some(dep) = units.iter().find(|u| &u.key == dep_key) else {
                continue;
            };
            let completed = dep
                .last_run_id
                .as_deref()
                .and_then(|run_id| self.store.run_index_get(run_id).ok().flatten())
                .and_then(|r| r.checkpoint)
                .and_then(|cp| cp.completed.last().cloned())
                .unwrap_or_else(|| "完了".to_string());
            dependency_summaries.push(format!("{}: {}", dep.spec.title, completed));
        }
        // ADR-0074 D1.2（Phase F2b）: WU ごとの worktree で走る run には、作業ブランチと並行しうる兄弟を渡す。
        let branch = self
            .store
            .work_unit_get(&wu.id)?
            .and_then(|row| row.branch)
            .or_else(|| wu.branch.clone());
        let parallel_siblings = if branch.is_some() {
            units
                .iter()
                .filter(|u| {
                    u.id != wu.id
                        && u.phase.is_some()
                        && u.phase == wu.phase
                        && u.kind != task_core::WorkUnitKind::Integrate
                        && u.status.is_active()
                })
                .map(|u| format!("{}: {}", u.key, u.spec.title))
                .collect()
        } else {
            Vec::new()
        };
        // ADR-0079 D7（Phase R3a）: この leaf が待っていた決定の人の回答（前置きの「人の決定」節。固定の書式）。
        let human_decisions = match self.store.get(task_id)? {
            Some(task) if self.config.execution.limits.tree.enabled => {
                task_ops::decision::leaf_decision_lines(
                    self.store.as_ref(),
                    task_id,
                    task_core::tree::root_id_of(&task),
                    wu,
                )
                .map_err(ops_to_store)?
            }
            _ => Vec::new(),
        };
        // ADR-0079 付記 R7-5 D3: 直前の run が done を返したのに checks が落ちていれば、その記録を次の run に渡す。
        let previous_check_failures =
            previous_check_failure_lines(&self.store.events_for(task_id)?, &wu.id, run_id);
        Ok(task_worker::protocol::WorkUnitPromptContext {
            key: wu.key.clone(),
            title: wu.spec.title.clone(),
            objective: wu.spec.objective.clone(),
            done_when: wu.spec.done_when.clone(),
            task_objective_excerpt,
            dependency_summaries,
            plan_overview,
            branch,
            parallel_siblings,
            human_decisions,
            previous_check_failures,
        })
    }

    /// ADR-0072 D9（Phase E2）: この WU の continuation の文脈を `runs` 索引から組み立てる
    /// （events は WU をまたぐ run の区別を持たないため、`events` ではなく `runs` を使う。
    /// E1 の `build_continuation_context` と同じ役割の WU 版）。
    pub(super) fn work_unit_continuation_context(
        &self,
        wu: &task_core::WorkUnitRow,
    ) -> Option<task_worker::ContinuationContext> {
        let runs = self.store.runs_for_work_unit(&wu.id).ok()?;
        let last = runs.last()?;
        let after_wait = last.status == task_core::RunIndexStatus::Waiting;
        if wu.continuations == 0 && !after_wait {
            return None;
        }
        let checkpoint = last.checkpoint.clone()?;
        let checkpoint_json = serde_json::to_value(&checkpoint).ok()?;
        let previous_end = match last.status {
            task_core::RunIndexStatus::Yielded => "yielded".to_string(),
            task_core::RunIndexStatus::Waiting => "waiting(cluster_jobs)".to_string(),
            other => other.as_str().to_string(),
        };
        let cluster_jobs = if after_wait {
            wu.task_id
                .parse::<TaskId>()
                .ok()
                .and_then(|task_id| self.work_unit_cluster_jobs(task_id, &wu.id, &last.run_id))
        } else {
            None
        };
        let prior_runs: Vec<String> = runs
            .iter()
            .map(|r| format!("Run #{} {}", r.seq, r.status.as_str()))
            .collect();
        Some(task_worker::ContinuationContext {
            run_seq: wu.runs + 1,
            previous_end,
            checkpoint: checkpoint_json,
            prior_runs,
            cluster_jobs,
        })
    }

    /// ADR-0072 D13（Phase E3）: Complexity Gate。Task の最初の dispatch で 1 回だけ判定し、
    /// `Event::ExecutionGated` と `Task.routing.execution` を同じトランザクションで書く。
    /// `gate = "off"` なら何もしない。対象外の大半（対話・support-task・kind != Execute・`routing`
    /// 無し）は呼び出し側で既に除いてある（D13「いつ」節）ので、ここでは残りの対象外
    /// （固定パイプラインの harness・`workspace_mode = Shared`）を `execution_gate::decide` の中で
    /// 判定する。すでに判定済みの Task（`routing.execution` が `Some`）には触らない。
    ///
    /// ADR-0079 D4 (1)（Phase R2a）: 木の子 task（`tree.parent_unit` を持つ。深さ ≥ 2）は、閾値を深さで
    /// 上げ（`5 + gate_depth_step × (depth − 1)`）、`[execution] gate` が `shadow` / `off` でも判定し採用する
    /// （`shadow = false`、`depth` を記録）。root・木でない task は従来どおり（1 バイトも変えない。U-R5）。
    pub(super) fn execution_gate_if_needed(&self, task: Task) -> Result<Task, DispatchError> {
        let tree_child = task_core::tree::is_tree_child(&task);
        if self.config.execution.gate == task_core::GateMode::Off && !tree_child {
            return Ok(task);
        }
        if task.kind != task_core::TaskKind::Execute || task.routing.is_none() {
            return Ok(task);
        }
        if task_core::support_kind(&task).is_some() {
            return Ok(task);
        }
        let routing = task.routing.clone().unwrap_or_default();
        if routing.execution.is_some() {
            return Ok(task);
        }
        let (features, _overridden) =
            task_core::TaskFeatures::infer_with_hints(&task, routing.features.as_ref());
        let human_execution = routing
            .execution_hint
            .filter(|h| h.explicit)
            .map(|h| h.mode);
        let cos_hint_compound = routing
            .execution_hint
            .is_some_and(|h| !h.explicit && h.mode == task_core::ExecutionMode::Compound);
        // ADR-0079 D4 (1)（Phase R2a）: 木の子は shadow でも採用する（記録は `shadow = false`）。
        let shadow = self.config.execution.gate == task_core::GateMode::Shadow && !tree_child;
        let at = if tree_child {
            task_core::GateThreshold::at_depth(
                task_core::tree::depth_of(&task),
                self.config.execution.limits.tree.gate_depth_step,
            )
        } else {
            task_core::GateThreshold::ROOT
        };
        // S4/S6: E3 では決定的な既定値（`false`/`None`）で運用する（U10 と同じく、閾値・重みは
        // shadow の記録を見て後で調整する。ADR-0072「Phase E3 実装時の逸脱・明確化」参照）。
        let decision = task_core::decide_execution_gate_at(
            &task,
            &features,
            human_execution,
            cos_hint_compound,
            task_core::ExecutionGateInputs::default(),
            shadow,
            at,
        );
        let mut fresh = task.clone();
        let mut new_routing = routing;
        new_routing.execution = Some(decision.clone());
        fresh.routing = Some(new_routing);
        fresh.updated_at = OffsetDateTime::now_utc();
        match self.store.update_task(
            &fresh,
            Event::ExecutionGated {
                decision: Box::new(decision),
            },
        ) {
            Ok(updated) => Ok(updated),
            Err(e) => {
                tracing::warn!(task_id = %task.id, error = %e, "failed to record the execution gate decision; continuing without it");
                Ok(task)
            }
        }
    }

    /// ADR-0074 D1.3 3.（Phase F2b）: 並列 WU の 2 本目以降。このインスタンスが既に run を持っている
    /// （＝工程の lease の持ち主の）Task だけを対象にする（引き継ぎ中の別インスタンスの Task には
    /// 手を出さない。持ち主のいない Task は再起動の照合〈D1.7〉が Ready に戻す）。
    pub(super) fn dispatch_parallel_work_units(
        &mut self,
        full: &mut std::collections::HashSet<ProviderId>,
        now: Instant,
    ) -> Result<usize, DispatchError> {
        if self.workers_in_flight() >= self.config.max_concurrency {
            return Ok(0);
        }
        let mut dispatched = 0;
        let window = self.ready_window();
        for task in self.store.running_tasks_with_runnable_work_units(window)? {
            if !self.is_eligible(&task)
                || self.running_for_task(task.id) == 0
                || self.integrating.contains_key(&task.id)
                || self.just_aborted.contains(&task.id)
            {
                continue;
            }
            // ADR-0079 D13（Phase R5a）: subtree の一時停止・案件の停止の後は、走っている run は終わるまで走らせるが、
            // 並列 WU の 2 本目以降は新しく起こさない（`ready_tasks` と同じ判定）。
            if self.store.halted_by_pause(&task)? {
                continue;
            }
            let mode = self.parallel_mode(&task)?;
            loop {
                if self.workers_in_flight() >= self.config.max_concurrency {
                    return Ok(dispatched);
                }
                let units = self.store.work_units_for(task.id)?;
                let in_flight = units
                    .iter()
                    .filter(|u| {
                        u.status == task_core::WorkUnitStatus::Running
                            && u.kind != task_core::WorkUnitKind::Integrate
                    })
                    .count();
                let ids =
                    crate::execution_scheduler::runnable_in_phase(&units, in_flight, mode.limit);
                let Some(wu) = ids
                    .first()
                    .and_then(|id| units.into_iter().find(|u| &u.id == id))
                else {
                    break;
                };
                if !self.dispatch_one(task.clone(), Some(wu), full, now)? {
                    break;
                }
                dispatched += 1;
            }
        }
        Ok(dispatched)
    }

    /// ADR-0072 D21（Phase E3）: WU の run の lane。`decide_lane` と同じ天井（担当ノードの実効
    /// profile）を使うが、`TaskFeatures` は WU の view（`decide_for_work_unit`）で計算する。
    /// エスカレーション（リトライでの lane の引き上げ）は WU の retries を数えないので、E3 では
    /// 行わない（`task.attempts` は計画のある Task では WU の失敗で増えない。D11）。
    /// ADR-0074 D5.2（Phase F1）: `[execution] work_unit_lane_cap = "task"`（既定）のときは、Task
    /// 自身の（policy が決めた、エスカレーション前の）lane を上限の材料として渡す。`"none"` なら
    /// 上限を掛けない。
    pub(super) fn decide_lane_for_work_unit(
        &self,
        task: &Task,
        wu: &task_core::WorkUnitRow,
    ) -> Result<Option<task_core::LaneDecision>, DispatchError> {
        if task.routing.is_none() || task.kind != TaskKind::Execute {
            return Ok(None);
        }
        let org = self.store.org_list()?;
        let profile = task
            .assignee
            .as_deref()
            .filter(|_| !org.is_empty())
            .map(|a| task_core::profile::resolve(&org, a));
        let ceiling = profile
            .as_ref()
            .map(|p| p.lane_ceiling())
            .unwrap_or_default();
        let task_lane = match self.config.execution.work_unit_lane_cap {
            task_core::WorkUnitLaneCap::Task => {
                task_core::model_policy::decide_for_task(task, &ceiling).map(|d| d.lane)
            }
            task_core::WorkUnitLaneCap::None => None,
        };
        Ok(task_core::model_policy::decide_for_work_unit(
            task, wu, &ceiling, task_lane,
        ))
    }
}

/// ADR-0079 付記 R7-5 D3: WU `wu_id` の直前の run（`current_run_id` を除く最後の run）の checks の不合格を、
/// プロンプトの行（先頭が `cwd: <所>`、続いて不合格の検査ごとの判定文）にする。events を新しい方から見て、
/// その WU の `WorkUnitChecksFailed` が、別の run の `running` への遷移より先に見つかったときだけ返す
/// （直前の run が checks で落ちていない・continuation の続き・初回の run は空）。
pub(super) fn previous_check_failure_lines(
    events: &[(u64, Event)],
    wu_id: &str,
    current_run_id: &str,
) -> Vec<String> {
    for (_, ev) in events.iter().rev() {
        match ev {
            Event::WorkUnitChecksFailed {
                work_unit_id,
                run_id,
                cwd,
                failed,
                ..
            } if work_unit_id == wu_id && run_id != current_run_id => {
                let mut lines = vec![format!("cwd: {cwd}")];
                lines.extend(failed.iter().map(|f| {
                    if f.detail.contains(&f.cmd) {
                        f.detail.clone()
                    } else {
                        format!("cmd={:?} expected={}: {}", f.cmd, f.expect_exit, f.detail)
                    }
                }));
                return lines;
            }
            Event::WorkUnitTransitioned {
                work_unit_id,
                to: task_core::WorkUnitStatus::Running,
                run_id: Some(r),
                ..
            } if work_unit_id == wu_id && r != current_run_id => return Vec::new(),
            _ => {}
        }
    }
    Vec::new()
}

/// ADR-0079 付記 R7-5 D2: outcome に足す checks の不合格の要約の上限（文字数）。
const CHECK_FAILURE_SUMMARY_MAX_CHARS: usize = 1500;

/// ADR-0079 付記 R7-5 D1: WU の checks を走らせた結果（`Completion::WorkUnitChecks` の中身）。
pub(super) struct WorkUnitCheckRun {
    /// 走らせた checks（`results` と同じ順）。
    pub(super) checks: Vec<task_core::WorkUnitCheck>,
    /// `(pass, reason)` の 1 件ずつ（`review::run_work_unit_checks` の結果そのまま）。
    pub(super) results: Vec<(bool, String)>,
    /// check を実際に走らせた所。
    pub(super) cwd: PathBuf,
}

impl WorkUnitCheckRun {
    /// 1 つでも不合格があれば、その記録（worker が `Done` で返した usage も持つ）。全部合格なら `None`。
    pub(super) fn failure(
        &self,
        result: &Result<RunOutcome, AdapterError>,
    ) -> Option<WorkUnitCheckFailure> {
        let failed: Vec<task_core::FailedWorkUnitCheck> = self
            .results
            .iter()
            .enumerate()
            .filter(|(_, (pass, _))| !pass)
            .map(|(i, (_, detail))| {
                let (cmd, expect_exit) = self
                    .checks
                    .get(i)
                    .map(|c| (c.cmd.clone(), c.expect_exit))
                    .unwrap_or_default();
                task_core::FailedWorkUnitCheck {
                    cmd,
                    expect_exit,
                    detail: detail.clone(),
                }
            })
            .collect();
        if failed.is_empty() {
            return None;
        }
        let usage = match result {
            Ok(RunOutcome {
                terminal: Terminal::Done { usage, .. },
                ..
            }) => *usage,
            _ => None,
        };
        Some(WorkUnitCheckFailure {
            cwd: self.cwd.clone(),
            failed,
            usage,
        })
    }
}

/// ADR-0079 付記 R7-5: WU の checks が不合格で、run を retry / failed にすり替えたときの記録。
pub(super) struct WorkUnitCheckFailure {
    cwd: PathBuf,
    failed: Vec<task_core::FailedWorkUnitCheck>,
    /// worker が `Terminal::Done` で返した usage（D4: すり替えても落とさない）。
    pub(super) usage: Option<task_core::Usage>,
}

impl WorkUnitCheckFailure {
    /// D2: `checks failed in <cwd>: <判定文>; <判定文>…`（全体を [`CHECK_FAILURE_SUMMARY_MAX_CHARS`] で切る）。
    pub(super) fn summary(&self) -> String {
        let joined = self
            .failed
            .iter()
            .map(|f| f.detail.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        task_core::report::truncate_chars(
            &format!("checks failed in {}: {joined}", self.cwd.display()),
            CHECK_FAILURE_SUMMARY_MAX_CHARS,
        )
    }

    /// D1: `Event::WorkUnitChecksFailed`。
    pub(super) fn event(&self, run_id: &str, wu: &task_core::WorkUnitRow) -> Event {
        Event::WorkUnitChecksFailed {
            run_id: run_id.to_string(),
            work_unit_id: wu.id.clone(),
            key: wu.key.clone(),
            cwd: self.cwd.to_string_lossy().into_owned(),
            failed: self.failed.clone(),
        }
    }
}
