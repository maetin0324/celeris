//! 木の子 task（tree units）の gate・liveness・plan_invalid と一括作成（ADR-0079）。ADR-0082 の L2。

use super::*;

impl Dispatcher {
    /// ADR-0079 D4 (3) / D3（Phase R2a）: 採用する /3 の計画に unit の gate をかけ（上げる・下げるを spec に
    /// 当てる）、採用の直後に止める unit（子 task にできない compound な leaf、上限を超える unit）を決める。
    /// /3 でない・木が無効なら何もしない（`None`）。上げる・下げるで計画の上限を超えうるので、spec を変えた
    /// ときは `adopt_limits` を計画の上限を外したものにする（超えた分は `holds` が止める）。変えた spec が
    /// 他の理由で検証に落ちれば（通常起きない）、元の spec のまま採用し、上げる・下げるは当てない（警告）。
    pub(super) fn tree_plan_gate(
        &self,
        task: &Task,
        validated: task_core::execution_plan::ValidatedPlan,
        done_work_units: &[(String, task_core::WorkUnitSpec)],
        adopt_limits: &mut task_core::ExecutionLimits,
    ) -> (
        task_core::execution_plan::ValidatedPlan,
        Option<TreePlanOutcome>,
    ) {
        // ADR-0079 R5b-prep: 人の計画（`PUT`）と同じ関数（`task_ops::tree_plan::unit_gate_plan`）。
        task_ops::tree_plan::unit_gate_plan(
            self.store.as_ref(),
            task,
            validated,
            done_work_units,
            self.config.execution.limits,
            adopt_limits,
            task_core::PlanOrigin::Planner,
        )
    }

    /// ADR-0079 D4 (3) / D3（Phase R2a）: 採用した /3 の計画について、unit の gate の不一致
    /// （`UnitGateOverridden`）と、決定の要求（`leaf_too_large` / `limit`。D7 の形、path 付き、`decisions` の行）
    /// と、止める unit の `blocked(decision)` を 1 トランザクションで残す。止めるのは `pending` / `ready` の行
    /// だけ（replan で持ち越した走っている・終わった行は止めない）。同じ段階の他の unit・兄弟は止めない。
    pub(super) fn apply_tree_plan_holds(
        &self,
        task: &Task,
        plan: &task_core::ExecutionPlanRow,
        run_id: &str,
        outcome: TreePlanOutcome,
        now: OffsetDateTime,
    ) -> Result<(), DispatchError> {
        // ADR-0079 R5b-prep: 人の計画（`PUT`）と同じ関数（`task_ops::tree_plan::plan_hold_writes`）。planner の経路は
        // 採用の後の行を store から読み、止めを続きの 1 トランザクションで書く（R2a / R3a のまま）。
        let units = self.store.work_units_for(task.id)?;
        let writes = task_ops::tree_plan::plan_hold_writes(
            self.store.as_ref(),
            task,
            plan,
            &units,
            Some(run_id),
            task_core::DecisionOrigin::Planner,
            outcome,
            now,
        )
        .map_err(ops_to_store)?;
        if !writes.events.is_empty() {
            self.store
                .work_units_apply(task.id, Vec::new(), writes.rows, writes.events)?;
        }
        Ok(())
    }

    /// ADR-0079 D10（Phase R3b）: 木の生存確認。`ready` の木の節点（木の子・/3 の計画を持つ root）を
    /// `task_core::tree::liveness` で分け、「走っている / 走れる / 名指しの待ち」のどれでもない（`Unexplained`）まま
    /// `liveness_timeout_secs` 続いたら、`Event::StallDetected` と障害通知（`TaskFailed`、key
    /// `tree-stall:<task_id>:<StallDetected の seq>`）を 1 回だけ出す。同じ止まり方の間は繰り返さない（節点の最後の
    /// event が `StallDetected` の間は見送る）。`running` / `reviewing`（lease・レビュー）と `blocked`（人の質問・
    /// 途中確認・計画の承認）は常に名指しの状態なので `ready` だけを見る。判定は store の読み取りだけで、LLM なし。
    /// ADR-0074「F5-fix8 実装時の明確化」: 木が無効でも、また木の節点でなくても、有効な計画を持つ `ready` の
    /// Task（/1・/2 の計画）を同じ規則で見る（通知の key は `stall:<task_id>:<seq>`。木の節点は従来どおり
    /// `tree-stall:`）。計画を持たない atomic の Task は対象外。
    pub(super) fn check_tree_liveness(&mut self) -> Result<(), DispatchError> {
        let tree = self.config.execution.limits.tree;
        let now = self.now_utc();
        if let Some(last) = self.liveness_checked_at
            && (now - last).whole_seconds() < LIVENESS_CHECK_INTERVAL_SECS
        {
            return Ok(());
        }
        self.liveness_checked_at = Some(now);
        let mut nodes: Vec<Task> = Vec::new();
        let mut tree_nodes: std::collections::HashSet<TaskId> = std::collections::HashSet::new();
        for t in self.store.list(Some(Status::Ready))? {
            if tree.enabled && self.is_tree_node(&t)? {
                tree_nodes.insert(t.id);
                nodes.push(t);
            } else if self.store.execution_plan_active(t.id)?.is_some() {
                nodes.push(t);
            }
        }
        if nodes.is_empty() {
            self.stall_watch.clear();
            return Ok(());
        }
        let eligible: std::collections::HashSet<TaskId> = self
            .store
            .ready_tasks(100_000)?
            .into_iter()
            .map(|t| t.id)
            .collect();
        let mut facts = Vec::with_capacity(nodes.len());
        for t in &nodes {
            let replans_so_far = self
                .store
                .execution_plan_list(t.id)?
                .len()
                .saturating_sub(1) as u32;
            let replans_left = replans_so_far < self.effective_max_replans(t.id)?;
            facts.push(
                task_ops::tree::node_liveness_facts(
                    self.store.as_ref(),
                    t,
                    eligible.contains(&t.id),
                    replans_left,
                )
                .map_err(ops_to_store)?,
            );
        }
        let verdicts = task_core::tree::liveness(&task_core::TreeSnapshot { nodes: facts });
        let mut watching: std::collections::HashSet<TaskId> = std::collections::HashSet::new();
        for v in verdicts {
            if v.class != task_core::LivenessClass::Unexplained {
                continue;
            }
            let events = self.store.events_for(v.task_id)?;
            let Some((last_seq, last)) = events.last() else {
                continue;
            };
            if matches!(last, Event::StallDetected { .. }) {
                // 既に知らせた止まり方（何か起きるまで繰り返さない）。
                continue;
            }
            watching.insert(v.task_id);
            let since = match self.stall_watch.get(&v.task_id) {
                Some((seq, since)) if seq == last_seq => *since,
                _ => {
                    self.stall_watch.insert(v.task_id, (*last_seq, now));
                    now
                }
            };
            let elapsed = (now - since).whole_seconds();
            if elapsed < i64::try_from(tree.liveness_timeout_secs).unwrap_or(i64::MAX) {
                continue;
            }
            let Some(task) = nodes.iter().find(|t| t.id == v.task_id) else {
                continue;
            };
            let path =
                task_ops::tree::decision_path(self.store.as_ref(), task).map_err(ops_to_store)?;
            let breadcrumb = path
                .iter()
                .map(|p| p.title.as_str())
                .collect::<Vec<_>>()
                .join(" › ");
            let seq = self.store.append_event(
                v.task_id,
                &Event::StallDetected {
                    task_id: v.task_id,
                    detail: v.detail.clone(),
                    reason: v.reason.clone(),
                    since: rfc3339(since),
                    path,
                },
            )?;
            let is_tree_node = tree_nodes.contains(&v.task_id);
            let body = format!(
                "障害（stall）: 『{}』が理由なく止まっています（{} 秒以上。{}）: {}。位置: {}（{}）",
                task.title,
                elapsed,
                v.reason,
                v.detail,
                breadcrumb,
                if is_tree_node {
                    "ADR-0079 D10"
                } else {
                    "ADR-0074 F5-fix8"
                }
            );
            tracing::error!(task_id = %v.task_id, reason = %v.reason, detail = %v.detail, elapsed, tree = is_tree_node, "a task with an execution plan is stalled without a named wait (ADR-0079 D10 / ADR-0074 F5-fix8)");
            let key_prefix = if is_tree_node { "tree-stall" } else { "stall" };
            if let Err(e) = self.store.notification_upsert_pending(
                NotificationKind::TaskFailed,
                &format!("{key_prefix}:{}:{seq}", v.task_id),
                &body,
                task.project_id,
                now,
            ) {
                tracing::error!(task_id = %v.task_id, error = %e, "failed to record the stall notification");
            }
            self.stall_watch.remove(&v.task_id);
            watching.remove(&v.task_id);
        }
        self.stall_watch.retain(|id, _| watching.contains(id));
        Ok(())
    }

    /// ADR-0079 D8（Phase R3b）: 採用した計画が root の /3 の計画なら、承認の要否（`task_core::tree::plan_approval`）。
    /// 木が無効・/3 でない・木の子（子の計画は承認を求めない）なら `None`（従来どおり進める。報告も残さない）。
    pub(super) fn root_plan_approval(
        &self,
        task: &Task,
        plan: &task_core::ExecutionPlanRow,
    ) -> Result<Option<task_core::PlanApproval>, DispatchError> {
        if !self.config.execution.limits.tree.enabled
            || plan.spec.schema != task_core::EXECUTION_PLAN_SCHEMA_V3
            || task_core::tree::is_tree_child(task)
        {
            return Ok(None);
        }
        let facts = task_ops::plan_gate::approval_facts(self.store.as_ref(), task, plan)
            .map_err(ops_to_store)?;
        let limits = self.effective_tree_limits(task_core::tree::root_id_of(task))?;
        Ok(Some(task_core::tree::plan_approval(&facts, &limits)))
    }

    /// ADR-0079 D9（Phase R2b）: この task が出した未回答の `kind: plan_invalid` の決定。
    pub(super) fn open_plan_invalid(
        &self,
        task: &Task,
    ) -> Result<Option<task_core::DecisionRow>, DispatchError> {
        let root_id = task_core::tree::root_id_of(task);
        Ok(self
            .store
            .decisions_list(Some(root_id))?
            .into_iter()
            .find(|d| {
                d.task_id == task.id
                    && d.kind == task_core::DecisionKind::PlanInvalid
                    && d.status == task_core::DecisionStatus::Open
            }))
    }

    /// ADR-0079 D7（Phase R3a）: 木の節点の worker の run の `result.json` の `decisions` を読み、検証して
    /// （`task_core::decision::prepare_worker_decisions`: 形・指す先・key の重複・`max_open_decisions` の残りを超えた分の
    /// 束ね）、記録する event と止める unit を組み立てる。書き込みはしない。木が無効・木の節点でない・ファイルが
    /// 無い・`decisions` が無ければ `None`（従来と 1 バイトも変わらない）。
    pub(super) fn worker_decisions(
        &self,
        task: &Task,
        wu: Option<&task_core::WorkUnitRow>,
        artifacts_dir: Option<&Path>,
        run_id: &str,
    ) -> Result<Option<WorkerDecisions>, DispatchError> {
        let limits = self.config.execution.limits.tree;
        if !limits.enabled || !self.is_tree_node(task)? {
            return Ok(None);
        }
        let Some(dir) = artifacts_dir else {
            return Ok(None);
        };
        let Ok(text) = std::fs::read_to_string(dir.join("result.json")) else {
            return Ok(None);
        };
        let raw = task_core::decision::worker_decisions_from_json(&text);
        if raw.is_empty() {
            return Ok(None);
        }
        let now = rfc3339(OffsetDateTime::now_utc());
        let units = if wu.is_some() {
            self.store.work_units_for(task.id)?
        } else {
            Vec::new()
        };
        let unit_keys: std::collections::BTreeSet<String> = units
            .iter()
            .filter(|u| u.kind != task_core::WorkUnitKind::Integrate)
            .map(|u| u.key.clone())
            .collect();
        let stages: std::collections::BTreeSet<String> =
            units.iter().filter_map(|u| u.phase.clone()).collect();
        let root_id = task_core::tree::root_id_of(task);
        let rows = self.store.decisions_list(Some(root_id))?;
        let taken: std::collections::BTreeSet<String> = rows
            .iter()
            .filter(|r| r.task_id == task.id)
            .map(|r| r.key.clone())
            .collect();
        let open_tree = rows
            .iter()
            .filter(|r| r.status == task_core::DecisionStatus::Open)
            .count();
        let open_node = rows
            .iter()
            .filter(|r| r.status == task_core::DecisionStatus::Open && r.task_id == task.id)
            .count();
        let cap = limits
            .max_open_decisions_per_plan
            .saturating_sub(open_node)
            .min(limits.max_open_decisions_per_tree.saturating_sub(open_tree));
        let self_key = wu.map(|w| w.key.as_str());
        let batch = task_core::decision::prepare_worker_decisions(
            &raw, self_key, &unit_keys, &stages, &taken, cap,
        );
        let mut events: Vec<Event> = Vec::new();
        for reason in &batch.rejected {
            events.push(Event::worker_progress(
                run_id,
                format!("worker の決定の要求を記録できませんでした（ADR-0079 D7）: {reason}"),
            ));
        }
        if !batch.bundled.is_empty() {
            events.push(Event::worker_progress(
                run_id,
                format!(
                    "未回答の決定の上限（残り {cap} 件）を超えたので、worker の決定 {} を 1 件にまとめました（ADR-0079 D7）",
                    batch.bundled.join(", ")
                ),
            ));
        }
        if batch.accepted.is_empty() {
            return Ok(Some(WorkerDecisions {
                events,
                held_rows: Vec::new(),
                self_hold: false,
                count: 0,
            }));
        }
        let mut path =
            task_ops::tree::decision_path(self.store.as_ref(), task).map_err(ops_to_store)?;
        if let Some(w) = wu {
            // 決定を出した leaf を path の最後の段に（パンくず「root › … › <段階> › <leaf>」。`stage` は D7 の
            // path と同じく「次の段が属する段階」なので、節点の段に leaf の段階を書く）。
            if let Some(last) = path.last_mut() {
                last.stage = w.phase.clone();
            }
            path.push(task_core::DecisionPathEntry {
                task_id: task.id,
                title: w.spec.title.clone(),
                stage: None,
                unit: Some(w.key.clone()),
            });
        }
        let raised_by = task_core::DecisionRaisedBy {
            task_id: task.id,
            run_id: Some(run_id.to_string()),
            origin: task_core::DecisionOrigin::Worker,
        };
        let self_target = self_key.unwrap_or(task_core::decision::NEEDED_BEFORE_SELF);
        let self_hold = batch
            .accepted
            .iter()
            .any(|d| d.needed_before.iter().any(|n| n == self_target));
        for spec in &batch.accepted {
            events.push(Event::DecisionRequested {
                decision: Box::new(task_core::decision::request_from_spec(
                    spec,
                    path.clone(),
                    raised_by.clone(),
                )),
            });
        }
        // 他の unit（まだ走っていないもの）を止める。この leaf 自身は呼び出し側が止める。
        let mut held_rows = Vec::new();
        for u in &units {
            if Some(u.key.as_str()) == self_key
                || u.kind == task_core::WorkUnitKind::Integrate
                || !matches!(
                    u.status,
                    task_core::WorkUnitStatus::Pending | task_core::WorkUnitStatus::Ready
                )
            {
                continue;
            }
            let stage_ref = u
                .phase
                .as_deref()
                .map(|p| format!("{}{p}", task_core::decision::NEEDED_BEFORE_STAGE_PREFIX));
            let named = batch.accepted.iter().any(|d| {
                d.needed_before
                    .iter()
                    .any(|n| n == &u.key || stage_ref.as_deref() == Some(n.as_str()))
            });
            if !named {
                continue;
            }
            let mut row = u.clone();
            row.status = task_core::WorkUnitStatus::Blocked;
            row.blocked_reason = Some(task_core::WorkUnitBlockedReason::Decision);
            row.updated_at = now.clone();
            events.push(Event::WorkUnitTransitioned {
                work_unit_id: u.id.clone(),
                key: u.key.clone(),
                from: u.status,
                to: task_core::WorkUnitStatus::Blocked,
                reason: "decision".to_string(),
                run_id: Some(run_id.to_string()),
            });
            held_rows.push(row);
        }
        Ok(Some(WorkerDecisions {
            events,
            held_rows,
            self_hold,
            count: batch.accepted.len(),
        }))
    }

    /// ADR-0079 D9（Phase R2b）/ D7（Phase R3a）: この節点が出した未回答の決定で `needed_before: [self]` のもの
    /// （`plan_invalid`・atomic の run の worker の `self`）がある task は run を起こさない（`ready` のまま。名指しの
    /// 待ち = その決定）。回答（`task_ops::decision::answer`）で決定が閉じれば次の tick から走る。`kind: limit` の
    /// `self`（run 時の木の上限・節点の replan の上限）はここでは止めない: run を起こすときに `tree_run_limit_hold` /
    /// `replan_gate` が回答の余裕込みで見直す（R2a 付記 9.: 走っている run・判定・統合は止めない）。木が無効なら
    /// 常に `false`（従来と 1 バイトも変わらない）。
    pub(super) fn decision_self_hold(&self, task: &Task) -> Result<bool, DispatchError> {
        if !self.config.execution.limits.tree.enabled {
            return Ok(false);
        }
        let root_id = task_core::tree::root_id_of(task);
        Ok(self.store.decisions_list(Some(root_id))?.iter().any(|d| {
            d.task_id == task.id
                && d.status == task_core::DecisionStatus::Open
                && d.kind != task_core::DecisionKind::Limit
                && d.needed_before
                    .iter()
                    .any(|n| n == task_core::decision::NEEDED_BEFORE_SELF)
        }))
    }

    /// ADR-0079 D9 / D7（Phase R3a）: この節点の最後の `plan_invalid` の決定（回答済みでも）。
    pub(super) fn last_plan_invalid(
        &self,
        task: &Task,
    ) -> Result<Option<task_core::DecisionRow>, DispatchError> {
        let root_id = task_core::tree::root_id_of(task);
        Ok(self
            .store
            .decisions_list(Some(root_id))?
            .into_iter()
            .rfind(|d| d.task_id == task.id && d.kind == task_core::DecisionKind::PlanInvalid))
    }

    /// ADR-0079 D9 / D7（Phase R3a）: `plan_invalid` に「atomic で試す」と答えた節点（計画がまだ無い）の gate の
    /// 判定を atomic に書き換える（ADR-0072 D14 の atomic への倒し方と同じ書き方。規則 id `atomic/decision`）。
    /// 決定的（回答済みの行を読むだけ）。当てなければ `task` をそのまま返す。
    pub(super) fn apply_plan_invalid_atomic(&self, task: Task) -> Result<Task, DispatchError> {
        if !self.config.execution.limits.tree.enabled {
            return Ok(task);
        }
        let Some(row) = self.last_plan_invalid(&task)? else {
            return Ok(task);
        };
        let Some(answer) = row.request.answer.as_ref() else {
            return Ok(task);
        };
        if task_core::answer_effect(row.kind, &answer.option) != task_core::DecisionEffect::Atomic
            || self.store.execution_plan_active(task.id)?.is_some()
        {
            return Ok(task);
        }
        let Some(mut routing) = task.routing.clone() else {
            return Ok(task);
        };
        let Some(mut decision) = routing.execution.clone() else {
            return Ok(task);
        };
        if decision.mode == task_core::ExecutionMode::Atomic {
            return Ok(task);
        }
        decision.mode = task_core::ExecutionMode::Atomic;
        decision.rule_id = "atomic/decision".to_string();
        decision.signals.push(task_core::GateSignal {
            name: "plan_invalid_answered_atomic".to_string(),
            weight: 0,
            detail: format!(
                "a human answered the plan_invalid decision {} with atomic ({})",
                row.id, answer.by
            ),
        });
        routing.execution = Some(decision.clone());
        let mut fresh = task.clone();
        fresh.routing = Some(routing);
        fresh.updated_at = OffsetDateTime::now_utc();
        tracing::info!(task_id = %task.id, decision = %row.id, "plan_invalid answered with atomic; running the node as one run (ADR-0079 D9 / R3a)");
        Ok(self.store.update_task(
            &fresh,
            Event::ExecutionGated {
                decision: Box::new(decision),
            },
        )?)
    }

    /// ADR-0079 D7（Phase R3a）: 回答で足した余裕（`raise-once` / `replan`）を当てた木の上限（run 時の上限だけ）。
    pub(super) fn effective_tree_limits(
        &self,
        root_id: TaskId,
    ) -> Result<task_core::TreeLimits, DispatchError> {
        let tree = self.config.execution.limits.tree;
        let allowances = task_ops::decision::limit_allowances(self.store.as_ref(), root_id, None)
            .map_err(ops_to_store)?;
        Ok(task_core::tree::limits_with_allowances(&tree, &allowances))
    }

    /// ADR-0079 D7（Phase R3a）: 節点の replan の上限（`[execution] max_replans`）に、`limit:max_replans` への
    /// `raise-once` / `replan` の回答の数を足したもの（木が無効・木の節点でなければ設定の値のまま）。
    pub(super) fn effective_max_replans(&self, task_id: TaskId) -> Result<u32, DispatchError> {
        let base = self.config.execution.max_replans;
        if !self.config.execution.limits.tree.enabled {
            return Ok(base);
        }
        let Some(task) = self.store.get(task_id)? else {
            return Ok(base);
        };
        let root_id = task_core::tree::root_id_of(&task);
        let allowances =
            task_ops::decision::limit_allowances(self.store.as_ref(), root_id, Some(task_id))
                .map_err(ops_to_store)?;
        Ok(base.saturating_add(
            allowances
                .get(&task_core::TreeLimitKind::NodeReplans)
                .copied()
                .unwrap_or(0),
        ))
    }

    /// ADR-0079 D9（Phase R2b）: 木の節点の replan が節点の上限（`[execution] max_replans`）に達した。黙って
    /// 止まらず（`Skip` のまま何も起きない、を避ける）`kind: limit`（`limit:max_replans`）の決定の要求を 1 件だけ
    /// 出す（同じ節点で未回答のものがあれば出さない）。木が無効・木の節点でなければ何もしない。
    pub(super) fn raise_node_replan_limit(
        &self,
        task_id: TaskId,
        used: u32,
    ) -> Result<(), DispatchError> {
        if !self.config.execution.limits.tree.enabled {
            return Ok(());
        }
        let Some(task) = self.store.get(task_id)? else {
            return Ok(());
        };
        if !self.is_tree_node(&task)? {
            return Ok(());
        }
        let key = task_core::TreeLimitKind::NodeReplans.decision_key(None);
        let root_id = task_core::tree::root_id_of(&task);
        let already = self
            .store
            .decisions_list(Some(root_id))?
            .into_iter()
            .any(|d| {
                d.task_id == task_id && d.key == key && d.status == task_core::DecisionStatus::Open
            });
        if already {
            return Ok(());
        }
        let path =
            task_ops::tree::decision_path(self.store.as_ref(), &task).map_err(ops_to_store)?;
        let request = task_core::tree::limit_decision(
            task_core::TreeLimitKind::NodeReplans,
            None,
            u64::from(used),
            u64::from(self.effective_max_replans(task_id)?),
            vec![task_core::decision::NEEDED_BEFORE_SELF.to_string()],
            path,
            task_core::DecisionRaisedBy {
                task_id,
                run_id: None,
                origin: task_core::DecisionOrigin::Daemon,
            },
        );
        tracing::warn!(%task_id, used, decision = %request.id, "the node's replans are exhausted; asking a human (ADR-0079 D9)");
        self.store.append_event(
            task_id,
            &Event::DecisionRequested {
                decision: Box::new(request),
            },
        )?;
        Ok(())
    }

    /// ADR-0079 D3（Phase R2a）: 木の節点か（木の子 task、または /3 の計画を持つ root）。
    pub(super) fn is_tree_node(&self, task: &Task) -> Result<bool, DispatchError> {
        if task.tree.is_some() {
            return Ok(true);
        }
        Ok(self
            .store
            .execution_plan_active(task.id)?
            .is_some_and(|p| p.spec.schema == task_core::EXECUTION_PLAN_SCHEMA_V3))
    }

    /// ADR-0079 D3（Phase R2a）: この dispatch が起こす run（worker / planner / repair。統合と「何もしない」は
    /// 数えない）が木の上限（`max_tree_runs` / `max_tree_tokens` / replan なら `max_tree_replans`）を超えるなら
    /// `true`（呼び出し側は run を起こさない。Task は今の状態のまま待つ）。超えたときは `kind: limit` の決定の
    /// 要求を**木に 1 件だけ**出す（同じ key の未回答の決定が木にあれば出さない。tick ごとに増やさない）。
    /// 木が無効・木の節点でなければ常に `false`（従来と 1 バイトも変わらない）。
    pub(super) fn tree_run_limit_hold(
        &self,
        task: &Task,
        gate: &WuDispatchGate,
    ) -> Result<bool, DispatchError> {
        let tree = self.config.execution.limits.tree;
        if !tree.enabled {
            return Ok(false);
        }
        let next = match gate {
            WuDispatchGate::Atomic | WuDispatchGate::RunWorkUnit(_) => task_core::NextRun::Worker,
            WuDispatchGate::RunPlanner { replan } => {
                task_core::NextRun::Planner { replan: *replan }
            }
            WuDispatchGate::StartIntegration(_)
            | WuDispatchGate::FinalReview
            | WuDispatchGate::Skip => return Ok(false),
        };
        if !self.is_tree_node(task)? {
            return Ok(false);
        }
        let root_id = task_core::tree::root_id_of(task);
        let counters =
            task_ops::tree::tree_counters(self.store.as_ref(), root_id).map_err(ops_to_store)?;
        // ADR-0079 D7（Phase R3a）: `raise-once` / `replan` の回答で足した余裕を当てる。
        let tree = self.effective_tree_limits(root_id)?;
        let Some(breach) = task_core::tree::run_limit_breach(&tree, &counters, next) else {
            return Ok(false);
        };
        let key = breach.limit.decision_key(None);
        if task_ops::tree::open_decision(self.store.as_ref(), root_id, &key)
            .map_err(ops_to_store)?
            .is_none()
        {
            let path =
                task_ops::tree::decision_path(self.store.as_ref(), task).map_err(ops_to_store)?;
            let request = task_core::tree::limit_decision(
                breach.limit,
                None,
                breach.count,
                breach.max,
                vec![task_core::decision::NEEDED_BEFORE_SELF.to_string()],
                path,
                task_core::DecisionRaisedBy {
                    task_id: task.id,
                    run_id: None,
                    origin: task_core::DecisionOrigin::Daemon,
                },
            );
            tracing::warn!(task_id = %task.id, %root_id, limit = breach.limit.as_str(), count = breach.count, max = breach.max, decision = %request.id, "a tree limit is exceeded; this node starts no new run until a human decides (ADR-0079 D3)");
            self.store.append_event(
                task.id,
                &Event::DecisionRequested {
                    decision: Box::new(request),
                },
            )?;
        }
        Ok(true)
    }

    /// ADR-0079 D2 / D4 (4)（Phase R1b）: /3 の計画の採用前の検査。kind task の unit の `repos` が親の
    /// repos の部分集合か（外れれば `Err` = 不正な試行）、部をまたぐ子が認可済みか（未認可なら
    /// `NeedsAuthorization`、人が認めなかったなら `Err`）。子はまだ作らない（unit が ready になったとき）。
    pub(super) fn tree_plan_checks(
        &self,
        task: &Task,
        spec: &task_core::ExecutionPlanSpec,
        now: OffsetDateTime,
    ) -> Result<task_ops::delegate::ChildrenPlan, String> {
        task_ops::tree::check_task_unit_repos(self.store.as_ref(), task, spec)?;
        let mut tentative = Vec::new();
        for unit in spec.units.iter().filter(|u| u.is_task()) {
            let (child, _) = task_ops::tree::build_child_task(
                self.store.as_ref(),
                task,
                "",
                unit,
                &[],
                &self.config.roles,
                &self.config.genres,
                now,
            )?;
            tentative.push(child);
        }
        let pending =
            task_ops::delegate::cross_department_questions(self.store.as_ref(), task, &tentative)?;
        if pending.is_empty() {
            Ok(task_ops::delegate::ChildrenPlan::Ready(Vec::new()))
        } else {
            Ok(task_ops::delegate::ChildrenPlan::NeedsAuthorization(
                pending,
            ))
        }
    }

    /// ADR-0079 D4 (4)・(5) / D5（Phase R1b）: 木の照合（tick ごと。決定的、LLM なし）。終わっていない
    /// kind task の unit を持つ Task ごとに:
    /// 1. 子の状態を unit に写す（子 done → unit done、子 failed / cancelled → unit failed。非終端は running の
    ///    まま）。unit が done になったら、それを待っていた unit を ready にする。
    /// 2. ready の kind task の unit（依存は `newly_ready` が満たした、段階は現在の段階）で、`needs_decisions`
    ///    がすべて回答済みで、`max_parallel_child_tasks` に空きがあるものから子 task を 1 トランザクションで作る。
    ///
    /// 親の状態は変えない（子だけを待つ親は `Ready` のまま `WuDispatchGate::Skip`、段階が揃えば次の
    /// dispatch で統合）。親が終端なら何もしない（中止の連鎖は store が子へ伝える）。
    pub(super) fn reconcile_tree_units(&mut self) -> Result<(), DispatchError> {
        let now = OffsetDateTime::now_utc();
        for task_id in self.store.tasks_with_open_task_units()? {
            let Some(parent) = self.store.get(task_id)? else {
                continue;
            };
            if parent.status.is_terminal() {
                // 子だけを待っていた（run の無い）親の中止: 未完了の unit を cancelled にする（子は store の
                // 連鎖で既に中止済み）。done / failed の親の残りの kind task の unit も閉じる（照合の対象から外す）。
                if parent.status == Status::Cancelled {
                    self.cancel_open_work_units(&parent)?;
                } else {
                    self.close_open_task_units(&parent)?;
                }
                continue;
            }
            let Some(plan) = self.store.execution_plan_active(task_id)? else {
                continue;
            };
            if plan.spec.schema != task_core::EXECUTION_PLAN_SCHEMA_V3 {
                continue;
            }
            // 1. 子の状態の写し。
            let units = self.store.work_units_for(task_id)?;
            let mut changed = false;
            for u in units.iter().filter(|u| {
                u.kind == task_core::WorkUnitKind::Task
                    && u.status == task_core::WorkUnitStatus::Running
            }) {
                let Some(child_id) = u
                    .child_task_id
                    .as_deref()
                    .and_then(|s| s.parse::<TaskId>().ok())
                else {
                    continue;
                };
                let Some(child) = self.store.get(child_id)? else {
                    continue;
                };
                let Some((to, reason)) = task_ops::tree::unit_mirror(child.status) else {
                    continue;
                };
                // ADR-0079 D9（Phase R2b）: 子が基盤の分類（infra）で failed なら、親の replan にはしない:
                // 同じ unit から 1 回だけ子を作り直し、それでも失敗したら障害通知と unit `blocked(infra)`。
                if child.status == Status::Failed {
                    let failure = task_ops::tree::child_failure(self.store.as_ref(), &child)
                        .map_err(ops_to_store)?;
                    if failure.class == task_ops::derive::FailureClass::Infra {
                        self.handle_child_infra_failure(&parent, &plan, u, &child, &failure, now)?;
                        changed = true;
                        continue;
                    }
                }
                let mut row = u.clone();
                row.status = to;
                row.blocked_reason = None;
                row.clear_lease();
                row.updated_at = rfc3339(now);
                let mut events = vec![Event::WorkUnitTransitioned {
                    work_unit_id: u.id.clone(),
                    key: u.key.clone(),
                    from: u.status,
                    to,
                    reason: reason.to_string(),
                    run_id: None,
                }];
                // ADR-0079 D6（Phase R1c）: 子の done は「親の段階で取り込まれる準備ができた」。子の worktree に
                // 残った変更を決定的に commit し（WU の完了時の commit と同じ規則）、子のブランチの HEAD を
                // unit の `head_commit`、子の基点を `base_commit` に残す（`WorkUnitCommitted`、同じトランザクション）。
                if to == task_core::WorkUnitStatus::Done
                    && let Some((branch, head)) = self.commit_child_branch(&child)
                {
                    row.head_commit = Some(head.clone());
                    row.base_commit =
                        task_core::tree::child_base_commit(&child).map(str::to_string);
                    events.push(Event::WorkUnitCommitted {
                        work_unit_id: u.id.clone(),
                        key: u.key.clone(),
                        branch,
                        base: row.base_commit.clone(),
                        commit: head,
                    });
                }
                self.store
                    .work_units_apply(task_id, Vec::new(), vec![row], events)?;
                tracing::info!(%task_id, work_unit = %u.key, %child_id, child_status = ?child.status, to = ?to, "task unit mirrors its child task (ADR-0079 D4 (5))");
                changed = true;
            }
            let units = if changed {
                let units = self.store.work_units_for(task_id)?;
                // 子の done で依存が満たされた unit を ready に（段階の障壁は `newly_ready` が見る）。
                for id in task_core::newly_ready(&units) {
                    let Some(u) = units.iter().find(|u| u.id == id) else {
                        continue;
                    };
                    let mut row = u.clone();
                    row.status = task_core::WorkUnitStatus::Ready;
                    row.updated_at = rfc3339(now);
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
                self.store.work_units_for(task_id)?
            } else {
                units
            };
            // 2. 子 task の生成。ADR-0079 D8（Phase R3b）: root の計画が承認を待つ間は子を作らない
            // （承認までは unit を 1 つも起こさない）。
            if parent.status == Status::Blocked
                && task_ops::plan_gate::is_awaiting_plan_approval(
                    &parent,
                    &self.store.events_for(task_id)?,
                )
            {
                continue;
            }
            let limit = self
                .config
                .execution
                .limits
                .tree
                .max_parallel_child_tasks
                .max(1);
            let mut open_children = units
                .iter()
                .filter(|u| {
                    u.kind == task_core::WorkUnitKind::Task
                        && u.status == task_core::WorkUnitStatus::Running
                })
                .count();
            // 現在の段階（終端でない行のうち seq 最小の行の段階。`runnable_work_units` と同じ）。
            let current_stage = units
                .iter()
                .filter(|u| !u.status.is_terminal())
                .min_by_key(|u| u.seq)
                .and_then(|u| u.phase.clone());
            let mut ready: Vec<&task_core::WorkUnitRow> = units
                .iter()
                .filter(|u| {
                    u.kind == task_core::WorkUnitKind::Task
                        && u.status == task_core::WorkUnitStatus::Ready
                        && u.child_task_id.is_none()
                        && u.phase == current_stage
                })
                .collect();
            ready.sort_by_key(|u| u.seq);
            for u in ready {
                if open_children >= limit {
                    break;
                }
                let Some(unit_spec) = plan.spec.units.iter().find(|s| s.key == u.key) else {
                    continue;
                };
                // ADR-0079 D15（Phase R5b-prep）: `adopt` の unit は新しい子を作らない（既存の task を採用の入口
                // 〈人の計画の採用・`POST /tasks/{id}/tree/adopt`〉が結ぶまで待つ）。
                if unit_spec.adopt.is_some() {
                    continue;
                }
                let Some(decisions) = task_ops::tree::answered_decisions(
                    self.store.as_ref(),
                    task_id,
                    &u.needs_decisions,
                )
                .map_err(ops_to_store)?
                else {
                    // 答えの無い決定を待つ（この unit だけ。兄弟は止めない。ADR-0079 D7）。
                    continue;
                };
                // ADR-0079 D6（Phase R1c）: 子の worktree の基点（段階の基点か、同じ段階の依存先の HEAD）を
                // 子の `tree.base_commit` に書く（`Created` の Task の JSON に入るので replay で同じ値になる）。
                let built = task_ops::tree::build_child_task(
                    self.store.as_ref(),
                    &parent,
                    &plan.id,
                    unit_spec,
                    &decisions,
                    &self.config.roles,
                    &self.config.genres,
                    now,
                )
                .and_then(|(mut child, downgrades)| {
                    let base = self.child_base_commit(&parent, u, &units)?;
                    // ADR-0079 D9（Phase R2b）: 同じ unit から作る何回目の子か（replan が失敗した子の unit を
                    // 同じ key で残したときは attempt + 1）。
                    let attempt = self
                        .store
                        .events_for(task_id)
                        .map(|events| task_ops::tree::child_attempts(&events, &u.key) + 1)
                        .unwrap_or(1);
                    if let Some(tree) = child.tree.as_mut() {
                        tree.base_commit = base;
                        if let Some(pu) = tree.parent_unit.as_mut() {
                            pu.attempt = attempt;
                        }
                    }
                    Ok((child, downgrades))
                });
                match built {
                    Ok((child, downgrades)) => {
                        for reason in &downgrades {
                            tracing::info!(%task_id, child_id = %child.id, %reason, "workspace downgraded to local (ADR-0062 B2)");
                        }
                        let depth = task_core::tree::depth_of(&child);
                        let mut row = u.clone();
                        row.status = task_core::WorkUnitStatus::Running;
                        row.blocked_reason = None;
                        row.child_task_id = Some(child.id.to_string());
                        row.updated_at = rfc3339(now);
                        let events = vec![
                            Event::WorkUnitTransitioned {
                                work_unit_id: u.id.clone(),
                                key: u.key.clone(),
                                from: task_core::WorkUnitStatus::Ready,
                                to: task_core::WorkUnitStatus::Running,
                                reason: "child_created".to_string(),
                                run_id: None,
                            },
                            Event::ChildTaskCreated {
                                plan_id: plan.id.clone(),
                                unit_key: u.key.clone(),
                                child_task_id: child.id,
                                depth,
                            },
                        ];
                        if self
                            .store
                            .tree_child_create(task_id, &child, row, events, None)?
                        {
                            tracing::info!(%task_id, work_unit = %u.key, child_id = %child.id, depth, "child task created from a task unit (ADR-0079 D4 (4))");
                            open_children += 1;
                        }
                    }
                    Err(reason) => {
                        tracing::warn!(%task_id, work_unit = %u.key, %reason, "could not create the child task; the unit fails (ADR-0079 D4 (4))");
                        let mut row = u.clone();
                        row.status = task_core::WorkUnitStatus::Failed;
                        row.updated_at = rfc3339(now);
                        self.store.work_unit_transition(
                            task_id,
                            row,
                            Event::WorkUnitTransitioned {
                                work_unit_id: u.id.clone(),
                                key: u.key.clone(),
                                from: task_core::WorkUnitStatus::Ready,
                                to: task_core::WorkUnitStatus::Failed,
                                reason: "child_create_failed".to_string(),
                                run_id: None,
                            },
                        )?;
                        self.store.append_event(
                            task_id,
                            &Event::worker_progress(
                                String::new(),
                                format!("子 task「{}」を作れませんでした: {reason}", u.spec.title),
                            ),
                        )?;
                    }
                }
            }
        }
        Ok(())
    }

    /// ADR-0079 D9（Phase R2b）: kind task の unit の子が基盤の分類（`classify_task_failure` の infra: 基盤の再試行
    /// 〈`InfraRequeue`〉を使い切った harness_error・lease の失効・準備の失敗、供給側の requeue の上限）で `failed`
    /// になった。人への質問にはしない:
    /// - この版で作り直した回数が `MAX_CHILD_INFRA_RETRIES`（1）未満なら、同じ unit から新しい子（attempt + 1、同じ
    ///   基点）を作る。unit は `running` のまま（`WorkUnitTransitioned{child_infra_retry}` と `ChildTaskCreated`）。
    /// - 使い切っていれば unit を `blocked(infra)`（`child_infra_failed`）にし、障害通知（`TaskFailed`、基盤の分類）を
    ///   1 件出す。段階は完了しない（親は `ready` のまま待つ）。同じ段階の他の unit・兄弟の子は止めない。
    pub(super) fn handle_child_infra_failure(
        &self,
        parent: &Task,
        plan: &task_core::ExecutionPlanRow,
        u: &task_core::WorkUnitRow,
        child: &Task,
        failure: &task_ops::tree::ChildFailure,
        now: OffsetDateTime,
    ) -> Result<(), DispatchError> {
        let task_id = parent.id;
        let events = self.store.events_for(task_id)?;
        let retries = task_ops::tree::child_infra_retries(&events, &u.id);
        let child_id = child.id.to_string();
        let mut block_reason = failure.reason.clone();
        if retries < task_core::tree::MAX_CHILD_INFRA_RETRIES {
            let decisions = task_ops::tree::answered_decisions(
                self.store.as_ref(),
                task_id,
                &u.needs_decisions,
            )
            .map_err(ops_to_store)?
            .unwrap_or_default();
            let built = match plan.spec.units.iter().find(|s| s.key == u.key) {
                None => Err(format!("unit {} is not in the active plan", u.key)),
                Some(unit_spec) => task_ops::tree::build_child_task(
                    self.store.as_ref(),
                    parent,
                    &plan.id,
                    unit_spec,
                    &decisions,
                    &self.config.roles,
                    &self.config.genres,
                    now,
                ),
            };
            match built {
                Ok((mut next, _downgrades)) => {
                    let attempt = task_ops::tree::child_attempts(&events, &u.key) + 1;
                    if let Some(tree) = next.tree.as_mut() {
                        // 基点は同じ（親のブランチは段階の途中では動かない。ADR-0079 D6）。
                        tree.base_commit =
                            task_core::tree::child_base_commit(child).map(str::to_string);
                        if let Some(pu) = tree.parent_unit.as_mut() {
                            pu.attempt = attempt;
                        }
                    }
                    let depth = task_core::tree::depth_of(&next);
                    let mut row = u.clone();
                    row.child_task_id = Some(next.id.to_string());
                    row.updated_at = rfc3339(now);
                    let parent_events = vec![
                        Event::WorkUnitTransitioned {
                            work_unit_id: u.id.clone(),
                            key: u.key.clone(),
                            from: task_core::WorkUnitStatus::Running,
                            to: task_core::WorkUnitStatus::Running,
                            reason: task_ops::tree::CHILD_INFRA_RETRY_REASON.to_string(),
                            run_id: None,
                        },
                        Event::ChildTaskCreated {
                            plan_id: plan.id.clone(),
                            unit_key: u.key.clone(),
                            child_task_id: next.id,
                            depth,
                        },
                        Event::worker_progress(
                            String::new(),
                            format!(
                                "子 task「{}」（{}）が基盤の失敗で終わりました（{}）。質問にはせず、同じ unit {} から子を作り直します（attempt {attempt}。ADR-0079 D9）。",
                                child.title, child.id, failure.reason, u.key
                            ),
                        ),
                    ];
                    if self.store.tree_child_create(
                        task_id,
                        &next,
                        row,
                        parent_events,
                        Some(&child_id),
                    )? {
                        tracing::warn!(%task_id, work_unit = %u.key, failed_child = %child.id, child_id = %next.id, attempt, "the child task failed for an infrastructure reason; recreated it from the same unit (ADR-0079 D9)");
                    }
                    return Ok(());
                }
                Err(reason) => {
                    block_reason = format!("{block_reason}（作り直せませんでした: {reason}）");
                }
            }
        }
        let mut row = u.clone();
        row.status = task_core::WorkUnitStatus::Blocked;
        row.blocked_reason = Some(task_core::WorkUnitBlockedReason::Infra);
        row.clear_lease();
        row.updated_at = rfc3339(now);
        let attempts = retries + 1;
        let body = format!(
            "障害（infra）: 子 task「{}」が基盤の失敗で終わりました（自動の作り直し {retries} 回の後。計 {attempts} 回）: {block_reason}。unit {} は blocked(infra)、親「{}」の段階 {} は止まり、兄弟は続きます。再試行は人の操作で（ADR-0079 D9）。",
            child.title,
            u.key,
            parent.title,
            u.phase.as_deref().unwrap_or("-")
        );
        self.store.work_units_apply(
            task_id,
            Vec::new(),
            vec![row],
            vec![
                Event::WorkUnitTransitioned {
                    work_unit_id: u.id.clone(),
                    key: u.key.clone(),
                    from: u.status,
                    to: task_core::WorkUnitStatus::Blocked,
                    reason: task_ops::tree::CHILD_INFRA_FAILED_REASON.to_string(),
                    run_id: None,
                },
                Event::worker_progress(String::new(), body.clone()),
            ],
        )?;
        tracing::error!(%task_id, work_unit = %u.key, %child_id, "the child task failed again for an infrastructure reason; the unit is blocked(infra) and a failure notification is raised (ADR-0079 D9)");
        if let Err(e) = self.store.notification_upsert_pending(
            NotificationKind::TaskFailed,
            &format!("tree-infra:{}:{child_id}", u.id),
            &body,
            parent.project_id,
            now,
        ) {
            tracing::error!(%task_id, error = %e, "failed to record the infra failure notification");
        }
        Ok(())
    }

    /// ADR-0079（Phase R1b）: 終端（done / failed）になった親に残った kind task の unit を cancelled にする
    /// （`parent_terminal`。子は store の連鎖で中止済みか終端）。
    pub(super) fn close_open_task_units(&self, task: &Task) -> Result<(), DispatchError> {
        let units = self.store.work_units_for(task.id)?;
        let mut rows = Vec::new();
        let mut events = Vec::new();
        for u in units
            .iter()
            .filter(|u| u.kind == task_core::WorkUnitKind::Task && !u.status.is_terminal())
        {
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
                reason: "parent_terminal".to_string(),
                run_id: None,
            });
            rows.push(row);
        }
        if !rows.is_empty() {
            self.store
                .work_units_apply(task.id, Vec::new(), rows, events)?;
        }
        Ok(())
    }
}
