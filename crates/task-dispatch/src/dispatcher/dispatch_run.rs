//! run の起動（`dispatch_one`・`spawn_worker`）と、その前段の担当・lane・harness の決定。lease の取得から `running.insert` までは一続きに保つ。ADR-0082 の L4。

use super::*;
use crate::dispatch_admission::{
    self, AdmissionCluster, AdmissionFact, AdmissionInput, AdmissionProposal, AdmissionStop,
    AdmissionTaskValues, AdmissionUnit, BackoffClock, ClusterCooldown, ClusterFact, WorkUnitFact,
    planner_lane,
};
use crate::execution_coordination::WriteSetAdmission;

/// 付記 S2: `lane_rules` の compare の結線（`decide_lane` から呼ぶ）。`dispatcher.rs` を大きくしないよう、ここで宣言する。
#[path = "script_compare.rs"]
mod script_compare;
pub(in crate::dispatcher) use script_compare::ScriptCompareState;
pub(in crate::dispatcher) use script_compare::script_canary::unit_selected as script_canary_unit_selected;
#[cfg(test)]
pub(in crate::dispatcher) use script_compare::{
    SCRIPT_TICK_WAIT_BUDGET, ScriptTickWait, script_canary_target_for_test,
};

impl Dispatcher {
    // Verify must skip snapshot tasks before dependency/child lookups and maintenance.
    // Filtering only in dispatch_one still scans the snapshot on every smoke tick.
    pub(super) fn eligible_ready_tasks(&self, window: usize) -> Result<Vec<Task>, DispatchError> {
        Ok(self
            .store
            .ready_tasks(window)?
            .into_iter()
            .filter(|task| self.is_eligible(task))
            .collect())
    }

    /// `dispatch_ready` の 1 件分（ADR-0074 D1.3（Phase F2b）で切り出した。並列 WU の 2 本目以降も
    /// ここを通る）。run を起こしたら `Ok(true)`。
    pub(super) fn dispatch_one(
        &mut self,
        task: Task,
        forced_wu: Option<task_core::WorkUnitRow>,
        now: Instant,
    ) -> Result<bool, DispatchError> {
        // ADR 2026-11-14 dispatch-admission-order: 起こすかの順序と組み合わせ（どの gate をどの順で見るか、並列 WU の
        // 2 本目以降が飛ばす gate、planner run か、予算の上書き）は `dispatch_admission::admit` が決める。ここは admit が
        // 求めた fact を既存の関数で集め、止めるなら止め方を適用して見送り、進むなら lease 以降を今のとおり行う。
        // ADR-0074 D1.3（Phase F2b）: 並列 WU の 2 本目以降は `forced_wu` で WU を受ける（Task は既に Running）。
        let second_pass = forced_wu.is_some();
        let mut gate = forced_wu.map(|wu| WuDispatchGate::RunWorkUnit(Box::new(wu)));
        let mut input = AdmissionInput {
            forced_unit: gate.as_ref().map(admission_unit),
            ..AdmissionInput::default()
        };
        let mut task = task;
        // admit が求めた置き場所の fact を集めたときに決まる値（起動で使う）。
        let mut original_task_budget = task.budget;
        let mut dir = None;
        let mut cluster: Option<(Box<ClusterSpec>, PathBuf, WorkspaceMode)> = None;
        let mut worktree = None;
        let mut wu_workspace: Option<WorkUnitWorkspace> = None;
        let (mut wu_scope_env, mut prepare_error) = (Vec::new(), None);
        // admit が求めた lane・route・variant の pin の fact を集めたときに決まる値（止め方と起動で使う）。
        let (mut fallback_reason, mut lane_decision, mut escalation_audit) = (None, None, None);
        let (mut lane_assist_record, mut route, mut launched) = (None, None, None);
        let (mut refused, mut campaign_pin) = (None::<Option<RouteLaunchError>>, None);
        let admitted = loop {
            let fact = match dispatch_admission::admit(&input) {
                AdmissionProposal::Need(fact) => fact,
                AdmissionProposal::Launch(admitted) => break admitted,
                AdmissionProposal::Stop(how) => {
                    match how {
                        AdmissionStop::Quiet => {}
                        AdmissionStop::Wait(wait) => self.note_dispatch_wait(task.id, wait),
                        AdmissionStop::Backoff { until, cause } => {
                            tracing::debug!(task_id = %task.id, %until, ?cause, "backoff; not dispatching yet");
                            let until = rfc3339(until);
                            let wait = task_core::TaskWait::Backoff { until, cause };
                            self.note_dispatch_wait(task.id, wait);
                        }
                        AdmissionStop::StartIntegration => {
                            if let Some(WuDispatchGate::StartIntegration(wu)) = &gate {
                                self.start_integration_from_ready(&task, wu)?;
                            }
                        }
                        AdmissionStop::StartFinalReview => {
                            self.start_final_review_from_ready(&task)?
                        }
                        AdmissionStop::ClusterNotConfigured { wait } => {
                            // `resolve_cluster` は remote の作業場所にだけ設定に無いクラスタを返す。
                            if self.routing.warn_unroutable_once(task.id)
                                && let WorkspaceSpec::Remote { cluster, .. } = &task.workspace
                            {
                                tracing::warn!(task_id = %task.id, %cluster, "no such cluster in the config; task left ready");
                            }
                            self.routing.mark_unroutable(task.id);
                            self.note_dispatch_wait(task.id, wait);
                        }
                        AdmissionStop::ClusterToolMissing { cluster } => {
                            self.block_task_missing_cluster_tool(&task, &cluster)?
                        }
                        AdmissionStop::Cluster {
                            unavailable,
                            waiting,
                            wait,
                        } => {
                            if let (Some(reason), Some((spec, ..))) = (unavailable, &cluster) {
                                self.mark_cluster_unavailable(task.id, spec, reason)?;
                            }
                            if waiting {
                                self.coord.note_cluster_waiting(task.id);
                            }
                            self.note_dispatch_wait(task.id, wait);
                        }
                        AdmissionStop::NoTaskDir => {
                            tracing::warn!(task_id = %task.id, "cannot resolve the workspace directory; task left ready");
                        }
                        AdmissionStop::WuPrepareBackoff { failures, retry_at } => {
                            if let Some(wu) = gate_wu(&gate) {
                                tracing::debug!(task_id = %task.id, work_unit = %wu.key, failures, %retry_at, "work unit worktree backoff; not dispatching yet");
                            }
                        }
                        AdmissionStop::WuPrepareFailed { forced } => {
                            if let (Some(wu), Some(e)) = (gate_wu(&gate), &prepare_error) {
                                self.on_work_unit_prepare_failed(&task, wu, e, forced)?;
                            }
                        }
                        AdmissionStop::RouteRefused => {
                            if let (Some((_, req, _, out, trace)), Some(e)) = (&route, &refused) {
                                self.note_route_refusal(task.id, req, out, trace, e.as_ref())?;
                            }
                        }
                        AdmissionStop::VariantPinFailed => {
                            if let Some(Err(reason)) = campaign_pin.take() {
                                self.block_variant_task(&task, reason)?;
                            }
                        }
                    }
                    return Ok(false);
                }
            };
            match fact {
                AdmissionFact::KnowledgeInputs => {
                    let ready =
                        task_ops::knowledge_curation::inputs_ready(self.store.as_ref(), &task)?;
                    input.knowledge_inputs_ready = Some(ready);
                }
                AdmissionFact::TaskRuns => {
                    input.task_runs = Some(self.supervisor.runs_for_task(task.id))
                }
                AdmissionFact::CosRun => input.cos_run = Some(self.is_cos_task(&task)?),
                AdmissionFact::Capacity { cos } => {
                    input.capacity_admits = Some(self.run_load().admits(cos))
                }
                AdmissionFact::JustAborted => {
                    input.just_aborted = Some(self.supervisor.was_just_aborted(task.id))
                }
                AdmissionFact::Eligible => input.eligible = Some(self.is_eligible(&task)),
                AdmissionFact::DiskCritical => {
                    input.disk_critical = Some(self.held_by_disk_critical(&task))
                }
                AdmissionFact::DiskWatchPaths => {
                    input.disk_watch_paths = Some(self.maint.disk_watch_paths())
                }
                // ADR-0046 D5（Phase 59）: 候補が無ければ blocked にして人に聞き `None` を返す（その後は見送るだけ）。
                AdmissionFact::Assignee => {
                    let assigned = self.assign_if_needed(task.clone())?;
                    input.assigned = Some(assigned.is_some());
                    task = assigned.unwrap_or(task);
                }
                AdmissionFact::ExecutionGate => {
                    task = self.execution_gate_if_needed(task)?;
                    task = self.apply_plan_invalid_atomic(task)?;
                    input.execution_gate_applied = true;
                }
                AdmissionFact::Unit => {
                    let unit = self.wu_dispatch_gate(task.id)?;
                    input.unit = Some(admission_unit(&unit));
                    gate = Some(unit);
                }
                AdmissionFact::DecisionSelfHold => {
                    input.decision_self_hold = Some(self.decision_self_hold(&task)?)
                }
                AdmissionFact::DesignReviewHold => {
                    input.design_review_hold = Some(self.design_review_hold(&task)?)
                }
                AdmissionFact::TreeLimit => {
                    let hold = match gate.as_ref().and_then(tree_units::next_run) {
                        Some(next) => self.tree_limit_hold_for(&task, next)?,
                        None => false,
                    };
                    input.tree_limit_hold = Some(hold);
                }
                AdmissionFact::TaskValues => {
                    let ex = &self.config.execution;
                    let budget = (ex.planner.max_turns, ex.planner.max_wall_secs);
                    let values = AdmissionTaskValues::read(&task, ex.gate, ex.planner.tier, budget);
                    input.task_values = Some(values);
                }
                AdmissionFact::RetryBackoff => {
                    let c = &self.config;
                    let delay =
                        retry_backoff(c.retry_backoff_base, c.retry_backoff_max, task.attempts);
                    let (until, now) = (task.updated_at + delay, self.now_utc());
                    input.retry_backoff = Some(BackoffClock { until, now });
                }
                AdmissionFact::InfraBackoff => {
                    let until = self.supervisor.infra_backoff_until(task.id);
                    input.infra_backoff = Some(until.map(|until| BackoffClock {
                        until,
                        now: self.now_utc(),
                    }));
                }
                AdmissionFact::ClearInfraBackoff => {
                    self.supervisor.clear_infra_backoff(task.id);
                    input.infra_backoff_cleared = true;
                }
                AdmissionFact::BrowserPrereq { planner_run } => {
                    input.browser_prereq_needed =
                        Some(Self::browser_prereq_applies(&task, planner_run));
                }
                AdmissionFact::BrowserPrereqHold => {
                    input.browser_prereq_hold = Some(self.browser_prereq_hold(&task)?)
                }
                AdmissionFact::RunOverride { tier, budget } => {
                    // D18/D14: 上書きする前の Task の予算（WU/planner の既定の計算に使う。ADR-0072 D14）。
                    original_task_budget = task.budget;
                    task.worker_hint.tier = tier.unwrap_or(task.worker_hint.tier);
                    if let Some((max_turns, max_wall_secs)) = budget {
                        (task.budget.max_turns, task.budget.max_wall_secs) =
                            (max_turns, max_wall_secs);
                    }
                    input.place.run_override_applied = true;
                }
                // ADR-0130 D3: lease・遷移・attempts の前。planner run は書き込みを予約しない。
                AdmissionFact::WriteSet { planner_run } => {
                    let wu = gate_wu(&gate);
                    let work_unit = wu.filter(|w| w.phase.is_some()).map(|w| w.id.clone());
                    let key = RunKey {
                        task: task.id,
                        work_unit,
                    };
                    let admission = self.admit_write_set(&task, &key, wu, planner_run)?;
                    input.place.write_set = Some(admission);
                }
                AdmissionFact::Cluster => {
                    let resolved = self.resolve_cluster(&task);
                    input.place.cluster = Some(AdmissionCluster::of(&resolved));
                    if let ClusterResolution::Resolved(spec, path, mode) = resolved {
                        cluster = Some((spec, path, mode));
                    }
                }
                // admit は `resolve_cluster` が remote のクラスタを返した後でだけ求める。
                AdmissionFact::ClusterState(step) => {
                    let Some((spec, ..)) = &cluster else {
                        return Ok(false);
                    };
                    let (id, place) = (spec.id.as_str(), &mut input.place);
                    match step {
                        ClusterFact::Cooldown => {
                            let until = self.cluster.cooldown_until(id);
                            place.cluster_cooldown = Some(ClusterCooldown { until, now });
                        }
                        ClusterFact::Load => place.cluster_in_use = Some(self.cluster_in_use(id)),
                        // この tick の `refresh_cluster_liveness` の結果を使う（1 tick に 1 回だけ `ssh -O check` を呼ぶ）。
                        ClusterFact::Connected => {
                            place.cluster_connected = Some(self.cluster.connected(id))
                        }
                        // ADR-0032 D3: `auth = "publickey"` かつ接続フックがあれば、cooldown にする前に 1 回だけ接続を
                        // 試みる（cooldown 中は求められないので、tick ごとに ssh は湧かない。同じ tick の別タスクが同じ
                        // クラスタを指していても、成功時は `cluster_connected` のキャッシュが true になり、失敗時は
                        // cooldown が立つので、2 本目は走らない）。
                        ClusterFact::AutoConnect => {
                            let attempt = self.try_auto_connect_cluster(spec);
                            if let Some(result) = &attempt {
                                self.note_key_auth_attempt(spec, result.is_ok());
                            }
                            place.cluster_auto_connect = Some(attempt);
                        }
                        ClusterFact::MarkConnected => {
                            self.set_cluster_connected(id, true, ClusterConnChange::KeyAuth);
                            place.cluster_marked_connected = true;
                        }
                    }
                }
                // ADR-0041 D1 / ADR-0043 D2: ローカルの作業場所（1 つ以上のリポジトリ）を用意する
                // （`dir` はその親 = `runs/` `artifacts/` の置き場）。足回りの root はここで初めて固定する
                // （ADR 2026-10-14-local-machine-settings D5。以後の解決は固定された root を見る）。
                AdmissionFact::TaskDir => {
                    let dir_started = Instant::now();
                    self.pin_task_root(&task)?;
                    worktree = self.task_workspaces_for(&task);
                    dir = worktree.as_ref().map(|ws| ws.task_dir.clone());
                    dir = dir.or_else(|| self.task_dir(&task));
                    if dir.is_some() {
                        log_slow_step("task_dir", dir_started);
                    }
                    input.place.task_dir_resolved = Some(dir.is_some());
                }
                // admit は WU の run で、作業場所の dir を解決した後でだけ求める。
                AdmissionFact::WorkUnit(step) => {
                    let (Some(wu), Some(dir)) = (gate_wu(&gate), dir.as_deref()) else {
                        return Ok(false);
                    };
                    let place = &mut input.place;
                    match step {
                        WorkUnitFact::PrepareBackoff => {
                            let backoff = self.coord.wu_prepare_backoff(&wu.id, self.now_utc());
                            place.wu_prepare_backoff = Some(backoff.map(|f| (f.count, f.retry_at)));
                        }
                        WorkUnitFact::Workspace => {
                            let prepared =
                                self.prepare_work_unit_workspace(&task, wu, worktree.as_ref());
                            place.wu_workspace_ready = Some(prepared.is_ok());
                            match prepared {
                                Ok(prepared) => wu_workspace = prepared,
                                Err(e) => prepare_error = Some(e),
                            }
                        }
                        // Capture before any worker can write; retries load the same WU-id snapshot.
                        WorkUnitFact::ScopeEnv => {
                            // WU の worktree を用意したら、その作業場所で確かめる。
                            let ws = wu_workspace.as_ref().map(|w| &w.workspaces);
                            let ws = ws.or(worktree.as_ref());
                            let prepared = (|| {
                                if let Some(ws) = ws {
                                    for repo in &ws.repos {
                                        if let Some(wt) = &repo.worktree {
                                            wt.ensure_blocking().map_err(|e| {
                                                WuPrepareError::transient(e.to_string())
                                            })?;
                                        }
                                    }
                                }
                                // A shared repository's workspace symlink is installed by the
                                // worker later; its source resolves to the same git directory.
                                let cwd = ws
                                    .and_then(|ws| ws.repos.first())
                                    .map(|repo| {
                                        if repo.worktree.is_some() {
                                            repo.dir.as_path()
                                        } else {
                                            repo.source.as_path()
                                        }
                                    })
                                    .unwrap_or(dir);
                                super::wu_base::scope_env(cwd, &wu.id, true)
                                    .map_err(|e| WuPrepareError::transient(e.to_string()))
                            })();
                            place.wu_scope_env_ready = Some(prepared.is_ok());
                            match prepared {
                                Ok(env) => {
                                    self.coord.clear_wu_prepare_failures(&wu.id);
                                    wu_scope_env = env;
                                }
                                Err(e) => prepare_error = Some(e),
                            }
                        }
                    }
                }
                // ADR 2026-11-11-disk-aware-dispatch D1: 予備を割るなら待たせ（M12）、通した見込みは起動した後で数える。
                AdmissionFact::Disk => {
                    let v2 = gate_wu(&gate).filter(|w| w.phase.is_some());
                    let target_wu = wu_workspace.as_ref().and(v2);
                    let (view, gate) = (self.maint.scratch_view(), self.maint.admission());
                    let repo_of = || self.cargo_target_repo(&task).map(|r| r.source);
                    let wu = v2.map(|w| w.id.as_str());
                    input.place.disk = Some(gate.judge_run(view, &task, wu, target_wu, repo_of));
                }
                // ADR-0052 D1 / D2（Phase 64）: 知識整理 run は lane を決める前に `langmem` の接続先へ
                // `GET /models` を当て、届かなければ tier `cheap` の**汎用**ハーネスへ倒す
                // （task harness の固定を外すだけで、通常の route 選択へ戻す）。LLM は呼ばない。
                // ADR-0069 D3 / D6（Phase 114）: `routing` を持つ execute タスクは、lane を決定的な policy
                // （TaskFeatures → 規則表 → 組織の天井）とリトライのエスカレーションで決める。人の明示・
                // System の tier はそのまま（記録だけ）。残量による調整はこの後の `select_tier`（別の層）。
                // planner run は `planner_lane`（admit が決めた tier の記録）。D21: WU の run は WU の view
                // （objective/acceptance/budget/genre/features を差し替えたもの）で lane を決める。
                AdmissionFact::Lane(shape) => {
                    if !task_ops::knowledge_curation::is_daily_extraction(&task) {
                        fallback_reason = self.knowledge_fallback_reason(&task, now);
                    }
                    if let Some(reason) = &fallback_reason
                        && let Some(tier) = self.config.knowledge.fallback_tier
                    {
                        task.harness_id = None;
                        task.worker_hint.tier = tier;
                        task.budget.max_turns = KNOWLEDGE_FALLBACK_MAX_TURNS;
                        task.budget.max_wall_secs = KNOWLEDGE_FALLBACK_MAX_WALL_SECS;
                        tracing::info!(task_id = %task.id, %reason, ?tier, "knowledge: falling back to a generic harness");
                    }
                    (lane_decision, escalation_audit) = if shape.planner_run {
                        (
                            Some(planner_lane(&task, shape.planner_human_frontier)),
                            None,
                        )
                    } else if let Some(wu) = gate_wu(&gate) {
                        // WU の lane は task の lane を上限にする（ADR-0074 D5.2）。軌跡 escalation は task の
                        // run だけに掛け、WU の retry では上げない（ADR-0072 D21）。
                        (self.decide_lane_for_work_unit(&task, wu)?, None)
                    } else {
                        self.decide_lane(&task)?
                            .map_or((None, None), |(decision, audit)| (Some(decision), audit))
                    };
                    input.route.lane_decided = Some(lane_decision.is_some());
                }
                // `routing.mode = decision_assisted`（既定 fixed では何もしない）: 分類段の結果で lane を引き上げ得る。
                AdmissionFact::ClassificationDefer => {
                    let deferred = self.defer_for_classification(&task, gate_wu(&gate));
                    input.route.classification_deferred = Some(deferred);
                }
                // admit は lane を決めた run でだけ求める。
                AdmissionFact::LaneAssist => {
                    let Some(decision) = lane_decision.as_mut() else {
                        return Ok(false);
                    };
                    let (record, held) = self.apply_lane_assist(&task, gate_wu(&gate), decision)?;
                    (lane_assist_record, input.route.lane_assist_held) = (record, Some(held));
                }
                // ADR 2026-10-11 llm-source-harness-selection: harness → 宣言順の source×model →
                // `(source, account)` の admission を `select_route` だけで決め、その route の adapter と
                // worker に渡す `SelectedLlmRoute` を作る。選べなければ起動しない（別の source へ倒さない）。
                AdmissionFact::Route(shape) => {
                    if let Some(decision) = &lane_decision {
                        task.worker_hint.tier = decision.lane;
                    }
                    let current_wu = gate_wu(&gate);
                    let route_input = route_select::RouteInput {
                        task_id: task.id.to_string(),
                        harness: {
                            let mut request = route_select::harness_request_for(&task);
                            if shape.planner_run {
                                request.execution_class = Some("planner".into());
                            }
                            request
                        },
                        model_hint: None,
                        lane: Some(tier_label(task.worker_hint.tier).to_string()),
                        cos: shape.cos_run,
                    };
                    let run_id = ulid::Ulid::new().to_string();
                    // Capture the task-features snapshot before selection. The selector, route_decisions row,
                    // and RoutingDecided event must all refer to this same content-addressed input.
                    let route_features = lane_decision.as_ref().and_then(|decision| {
                        match self.store.record_route_features(
                            task_core::route_extract::Subject {
                                kind: if current_wu.is_some() { "work_unit" } else { "task" }.to_owned(),
                                id: current_wu
                                    .map(|wu| wu.id.clone())
                                    .unwrap_or_else(|| task.id.to_string()),
                            },
                            Some(task_core::route_extract::RouteFeatureContext {
                                lane: Some(
                                    task_core::model_catalog::assignments::tier_str(task.worker_hint.tier)
                                        .to_owned(),
                                ),
                                harness: task.harness_id.as_ref().map(ToString::to_string),
                                task_features_v69: Some(decision.features),
                            }),
                        ) {
                            Ok(features) => Some(task_core::RouteFeaturesRef {
                                schema: features.schema,
                                template: features.template,
                                hash: features.hash,
                                decision: None,
                            }),
                            Err(error) => {
                                tracing::warn!(task_id = %task.id, %error, "routing feature extraction failed; dispatch continues without snapshot");
                                None
                            }
                        }
                    });
                    // task 欄の routing 固定（ADR 2026-10-16-mlsys-measure-task-routing）。無ければ従来（config）。
                    let routing_override =
                        lane_assist::task_routing_override(self.store.as_ref(), task.id);
                    let route_context = route_select::RouteContext {
                        run_id: Some(run_id.clone()),
                        features: route_features.clone(),
                        work_unit_id: current_wu.map(|wu| wu.id.clone()),
                        evidence_mode: routing_override
                            .as_ref()
                            .map(|o| o.evidence_mode)
                            .unwrap_or_default(),
                        evidence_version: routing_override
                            .as_ref()
                            .and_then(|o| o.evidence_table_id.clone()),
                        priority_override: None,
                    };
                    let (mut route_outcome, mut route_trace) =
                        self.select_route_with(&route_input, &route_context);
                    // 付記 S2-11・S5-3: `route_priority` の compare と canary。compare と canary の Rust の側は記録だけ。
                    // TS の側で検証を通った TS の候補列が Rust と違えば、その列で同じ harness を選び直す。
                    if let Some(column) =
                        self.compare_route_priority(task.id, &run_id, &route_input, &route_trace)
                    {
                        let context = route_select::RouteContext {
                            priority_override: Some(column),
                            ..route_context
                        };
                        (route_outcome, route_trace) =
                            self.select_route_with(&route_input, &context);
                    }
                    if let (Some(record), Some(decision)) =
                        (lane_assist_record.take(), route_trace.decision.as_mut())
                    {
                        decision.lane_assist = Some(record);
                    }
                    // task 欄で routing を固定した task だけ、解決結果（mode・出自・下段・根拠表・online 更新）を判断 record に残す。
                    if routing_override.is_some()
                        && let Some(decision) = route_trace.decision.as_mut()
                    {
                        decision.routing_resolved = Some(task_core::route_select::resolve_routing(
                            self.routing.mode(),
                            routing_override.as_ref(),
                        ));
                    }
                    let launch = match &route_outcome {
                        Ok(pick) => self
                            .route_adapter(pick)
                            .and_then(|adapter| Ok((adapter, self.route_launch_target(pick)?)))
                            .map(|(adapter, selected)| (pick.clone(), adapter, selected))
                            .map_err(Some),
                        Err(_) => Err(None),
                    };
                    // 受けた route は、拒否を 1 回だけ記録する印を消す（`note_route_refusal` の対）。
                    match launch {
                        Ok(launch) => {
                            self.routing.clear_warned_unroutable(&task.id);
                            launched = Some(launch);
                        }
                        Err(error) => refused = Some(error),
                    }
                    input.route.route_launched = Some(launched.is_some());
                    route = Some((
                        run_id,
                        route_input,
                        route_features,
                        route_outcome,
                        route_trace,
                    ));
                }
                // ADR 2026-10-15-mlsys-min D4: 未登録の credential を要する経路は、この unit だけを待機へ移す。
                // admit は route を受けた後でだけ求める。
                AdmissionFact::CredentialGate => {
                    let Some((pick, ..)) = &launched else {
                        return Ok(false);
                    };
                    let hold = self.credential_gate_hold(&task, gate_wu(&gate), pick)?;
                    input.route.credential_hold = Some(hold);
                }
                // variant 付き試行は、lease と run 記録の前にもう一度 pin を確かめる（割当後に org が変わっていたら起動しない）。
                AdmissionFact::VariantPin => {
                    let Some((.., llm_route)) = &launched else {
                        return Ok(false);
                    };
                    let pin = self.variant_run_pin(&task, Some(llm_route));
                    input.route.variant_pinned = Some(pin.is_ok());
                    campaign_pin = Some(pin);
                }
            }
        };
        // admit は作業場所の dir を起動の前に求め、解決できなければ止める。
        let Some(dir) = dir else { return Ok(false) };
        let current_wu = match gate {
            Some(WuDispatchGate::RunWorkUnit(wu)) => Some(*wu),
            _ => None,
        };
        // ADR-0074 D1.2（Phase F2b）: v2 の WU の run は、WU ごとの worktree
        // （`<task_dir>/wu/<key>/repos/<name>`、ブランチ `celeris-wu/<task_id>/<key>`）で走る。並列 1 に
        // 倒した Task・統合の repair WU は Task の worktree を共有する（`None`）。
        let v2_wu = current_wu.as_ref().filter(|w| w.phase.is_some()).cloned();
        let task_worktree = worktree.clone();
        let worktree = match &wu_workspace {
            Some(w) => Some(w.workspaces.clone()),
            None => worktree,
        };
        let target_wu = wu_workspace.as_ref().and(v2_wu.as_ref());
        // 通した write-set の予約と disk の見込みは、起動した後で数える（ADR-0130 D3・ADR 2026-11-11-disk-aware-dispatch D1）。
        let write_reservation = match input.place.write_set.take() {
            Some(WriteSetAdmission::Admit(reservation)) => reservation,
            _ => None,
        };
        let disk = input.place.disk.take().flatten();
        let is_planner_dispatch = admitted.planner_run;
        // admit は route を受け、variant の pin を確かめた後でだけ起動へ進む。
        let (Some(route), Some((pick, adapter, llm_route)), Some(Ok(campaign_pin))) =
            (route, launched, campaign_pin)
        else {
            return Ok(false);
        };
        let (run_id, route_input, route_features, route_outcome, mut route_trace) = route;
        let adapter_id = adapter.id().to_string();
        let source_id = pick.candidate.source.to_string();
        let model = pick.candidate.model.to_string();
        let account_adapter = self.routing.source_account_adapter(&pick.candidate.source);
        // The pool account of an OAuth source (other sources book `(source, account)` on the route only).
        let account = account_adapter.map(|_| pick.account.to_string());
        let wall = Duration::from_secs(task.budget.max_wall_secs);
        let ttl = wall + self.config.lease_grace;
        let lease_started = Instant::now();
        // ADR-0074 D1.5（Phase F2b）: v2 の WU は、Task の lease を工程の保持者で取り（1 本目だけ）、
        // 続けて WU の lease を取る（Task の lease の期限は WU の lease の最大値まで延びる）。
        let acquired = match &v2_wu {
            Some(wu) => {
                let task_lease = second_pass || {
                    let holder = format!(
                        "{PHASE_LEASE_PREFIX}{}:{}:{}",
                        wu.plan_id,
                        wu.phase.as_deref().unwrap_or_default(),
                        ulid::Ulid::new()
                    );
                    self.store.acquire_lease(task.id, &holder, ttl)?
                };
                task_lease
                    && self.store.acquire_work_unit_lease(
                        task.id,
                        &wu.id,
                        &run_id,
                        ttl,
                        wu_workspace.as_ref().map(|w| w.branch.clone()),
                        wu_workspace.as_ref().map(|w| w.base.clone()),
                    )?
            }
            None => self.store.acquire_lease(task.id, &run_id, ttl)?,
        };
        log_slow_step("acquire_lease", lease_started);
        if !acquired {
            return Ok(false);
        }
        let event_started = Instant::now();
        self.store.append_event(
            task.id,
            &Event::WorkerStarted {
                run_id: run_id.clone(),
                adapter: adapter_id.clone(),
                model: model.clone(),
                provider: Some(source_id.clone()),
                // ADR-0024 D4: `account_pool` のプロバイダで選んだアカウント（プールを使わなければ `None`）。
                account: account.clone(),
                // ADR-0072 D14（Phase E3）: planner run だけ `Some(Planner)`（ワーカー run は
                // 従来どおり `None`）。
                role: if is_planner_dispatch {
                    Some(RunRole::Planner)
                } else {
                    None
                },
                task_role: task.role.clone(),
            },
        )?;
        // ADR-0077 D1 の dispatch での途中目標の `in_progress` は ADR-0079 D13（Phase R5a）で廃止（途中目標は凍結）。
        let mut routing_decision_id: Option<String> = None;
        if let Some(decision) = lane_decision {
            let feature_ref = route_features;
            if let (Some(route_decision), Some(feature_ref)) =
                (route_trace.decision.as_mut(), feature_ref.as_ref())
            {
                route_decision.features_schema = Some(feature_ref.schema.clone());
                route_decision.features_template = Some(feature_ref.template.clone());
                route_decision.features_hash = Some(feature_ref.hash.clone());
                route_decision.run_id = Some(run_id.clone());
                self.store.record_route_decision(route_decision)?;
            }
            let feature_ref = feature_ref.map(|mut feature_ref| {
                feature_ref.decision = route_trace.decision.clone().map(Box::new);
                feature_ref
            });
            let record = task_core::RoutingRecord {
                features: feature_ref,
                org_node: task.assignee.clone(),
                harness: task.genre.clone(),
                // The selected source/model is already recorded on WorkerStarted. This field is
                // retained for old routing_decided event deserialization until the legacy audit
                // record is removed from task-core.
                quota_reason: None,
                decision,
                // ADR-0072 D21（Phase E3）: WU の run だけ `work_unit_id` を持つ。
                work_unit_id: current_wu.as_ref().map(|wu| wu.id.clone()),
                // Phase 3（ADR 2026-10-04 §5）: 上げた/上げなかった決定の構造化監査。task の run だけ。
                escalation: escalation_audit,
            };
            routing_decision_id = Some(format!("run:{run_id}"));
            self.store.append_event(
                task.id,
                &Event::RoutingDecided {
                    run_id: run_id.clone(),
                    record: Box::new(record),
                },
            )?;
        }
        // ADR-0052 D1: 検査の結果を進行（`status`）として残す（run が始まってから 1 行だけ）。
        let knowledge_fallback = match &fallback_reason {
            Some(reason) => {
                self.store.append_event(
                    task.id,
                    &Event::worker_progress_with(
                        &run_id,
                        format!(
                            "langmem の接続先に届かない（{reason}）。cheap のハーネスに倒す（{adapter_id}）"
                        ),
                        task_core::ProgressFields::of(task_core::ProgressKind::Status),
                    ),
                )?;
                Some(KnowledgeFallbackRun {
                    adapter: adapter_id.clone(),
                    instructions: task_worker::knowledge_fallback_instructions(
                        KNOWLEDGE_CANDIDATES_REL,
                    ),
                    budget: task.budget,
                })
            }
            None => None,
        };
        log_slow_step("append_worker_started", event_started);
        let limits = RunLimits {
            wall_clock: wall,
            idle_timeout: self.config.idle_timeout,
            kill_grace: self.config.kill_grace,
        };
        tracing::info!(task_id = %task.id, %run_id, adapter = %adapter_id, source = %source_id, model = %model, account = account.as_deref(), "dispatching");
        let remote = cluster
            .as_ref()
            .map(|(spec, path, mode)| self.remote_ssh_settings(spec, path, task.id, *mode));
        // ADR-0043 D3（Phase 56）: ホストか、コンテナか、runtime が無くて `blocked` か。
        let container =
            self.container_decision(&task, worktree.as_ref(), &adapter_id, remote.is_some());
        // Phase 55/56 の合流: コンテナで走らせるなら、止めるための口（runtime の実行ファイルと
        // `--label celeris.task=<task_id>`）を覚えておく（ADR-0044 P55-4 / ADR-0043 P56-7）。
        let container_stop: Option<Arc<dyn task_worker::ContainerStopper>> = match &container {
            ContainerDecision::Container(run) => {
                Some(Arc::new(task_worker::ContainerStop::of(&run.plan)))
            }
            ContainerDecision::Host | ContainerDecision::Unavailable { .. } => None,
        };
        let mut extras =
            self.run_extras(&task, worktree.as_ref(), account.as_deref(), &adapter_id)?;
        // ADR 2026-10-20-mlsys-main2-fixes D1-3: task 木に token 予算があれば、この run 以外の消費を前置きに
        // 途中の累積 usage を監視する（既に上限なら run は adapter を起こさずに `BudgetExhausted{Tokens}`）。
        extras.token_budget = token_budget::run_watch(self.store.as_ref(), &task, Some(&run_id))?;
        // ADR-0056 D3（Phase 79）: mount 名にあったが KB に見つからなかった skill を `status` の
        // 進行イベントで 1 行ずつ報告する（run は落とさない）。
        for name in &extras.missing_skills {
            self.store.append_event(
                task.id,
                &Event::worker_progress_with(
                    &run_id,
                    format!("skill {name} not found"),
                    task_core::ProgressFields::of(task_core::ProgressKind::Status),
                ),
            )?;
        }
        // ADR-0072 D6/D9/D15（Phase E2）: 計画のある Task の WU の run。WU の行を `running` にし
        // （`runs`/`last_run_id` を更新）、`runs` 索引に 1 行作り、prompt に載せる文脈を組み立てる。
        // ADR-0140 D1: WU の worker run は、continuation なら同じ Claude Code session を resume するか、
        // checkpoint 前置きの新しい session に倒すかをここで決める（planner run は判断表 #1 で常に fresh）。
        // 付記 session-container: 計画の無い atomic task（直行経路を含む）の worker run も同じ判断表で、
        // task 単位の 1 本（`work_unit_id IS NULL`）を resume するか checkpoint 前置きの fresh に倒す。
        // CoS の対話 run（`extras.session` が `run_extras` で埋まる）と planner run は対象外。
        let atomic_worker =
            current_wu.is_none() && !is_planner_dispatch && extras.session.is_none();
        if current_wu.is_some() || atomic_worker {
            let cwd = worktree
                .as_ref()
                .and_then(|w| w.cwd())
                .unwrap_or(dir.as_path())
                .to_string_lossy()
                .into_owned();
            let surface = continuation_session::ContinuationSurface {
                provider: Some(source_id.as_str()),
                cwd: Some(cwd.as_str()),
                container: matches!(container, ContainerDecision::Container(_)),
            };
            let role = if is_planner_dispatch {
                crate::sessions::ContinuationRole::Planner
            } else {
                crate::sessions::ContinuationRole::Worker
            };
            if let Err(e) = self.resolve_continuation_session(
                &task,
                current_wu.as_ref(),
                &run_id,
                role,
                &adapter_id,
                account.as_deref(),
                &surface,
                &mut extras,
            ) {
                let work_unit = current_wu.as_ref().map(|wu| wu.key.as_str());
                tracing::warn!(task_id = %task.id, work_unit, error = %e, "failed to resolve the continuation session; running with a fresh context");
                extras.session = None;
                extras.continuation_session = None;
            }
        }
        // ADR-0072 D5（E2b の指摘）: 計画の無い Task（暗黙の WorkUnit）の worker run も `runs`
        // 索引に書く（(g)「全タスクの run について書く」。WU の run は `start_work_unit_run`、
        // planner run はこの少し下で、それぞれ自分で `run_index_start` を呼ぶ）。付記 session-container: 続きの
        // session を決めた後に書き、この run が使う session の id を載せる（usage の積み上げの key）。
        if current_wu.is_none() && !is_planner_dispatch {
            let seq = current_run_seq(&self.store.events_for(task.id)?) + 1;
            if let Err(e) = self.store.run_index_start(task_core::RunRow {
                run_id: run_id.clone(),
                task_id: task.id.to_string(),
                work_unit_id: None,
                role: task_core::RunIndexRole::Worker,
                seq,
                status: task_core::RunIndexStatus::Running,
                adapter: Some(adapter_id.clone()),
                model: Some(model.clone()),
                account: account.clone(),
                session_id: extras.session.as_ref().map(|s| s.session_id.clone()),
                checkpoint: None,
                usage: None,
                metrics: None,
                started_at: rfc3339(OffsetDateTime::now_utc()),
                finished_at: None,
            }) {
                tracing::warn!(task_id = %task.id, %run_id, error = %e, "failed to record the (implicit work unit) worker run start in the runs index");
            }
        }
        if let Some(wu) = &current_wu
            && let Err(e) = self.start_work_unit_run(
                task.id,
                wu,
                &run_id,
                &adapter_id,
                &model,
                account.as_deref(),
                &mut extras,
                v2_wu.is_some(),
            )
        {
            tracing::warn!(task_id = %task.id, work_unit = %wu.key, error = %e, "failed to record the work unit run start; continuing without work-unit context");
        }
        // ADR-0074 付記 2026-10-05 D3: WU の run にも checks と同じ `CELERIS_WU_BASE` / `CELERIS_WU_TARGET`。
        if let Some(wu) = &current_wu {
            let base = wu_workspace
                .as_ref()
                .map(|w| w.base.as_str())
                .or(wu.base_commit.as_deref());
            let mut env = self.work_unit_scope_env(&task, base);
            // A scope snapshot overrides the integration ancestry base.
            if !wu_scope_env.is_empty() {
                env.retain(|(k, _)| k != task_core::execution_plan::WU_BASE_ENV);
                env.extend(wu_scope_env);
            }
            extras.work_unit_env = (!env.is_empty()).then_some(env);
        }
        // ADR 2026-10-17-mlsys-trial-context-isolation D4: 試行 task の run（WU・planner・子 task を含む）は
        // worker の `celerisctl knowledge` の根を試行の `kb/` にする（通常の設定の根に落ちない）。
        if let Some(env) = extras.trial_env.take() {
            extras
                .work_unit_env
                .get_or_insert_with(Vec::new)
                .extend(env);
        }
        if let Some(w) = &wu_workspace {
            extras.artifacts_dir_override = Some(w.artifacts_dir.clone());
        }
        // ADR-0074 F5-fix（不具合 1）: 自分の worktree を持つ WU だけ WU ごとの `CARGO_TARGET_DIR`。
        extras.cargo_target_work_unit = target_wu.map(|w| (w.id.clone(), w.key.clone()));
        // ADR 2026-10-10-local-disk-growth-paths D1: 作業場所の先頭に git の repo が無い（`repos=[]` の子 task・
        // shared の Rust checkout）なら、代わりの checkout を決めて `run_worker` に渡す（repo 直下 target を防ぐ）。
        if remote.is_none()
            && !worktree
                .as_ref()
                .and_then(|ws| ws.repos.first())
                .is_some_and(|r| r.is_git())
        {
            extras.cargo_target_fallback = Some(self.cargo_target_fallback(&task));
        }
        if current_wu.is_some() {
            // ADR-0072 D22（Phase E3）: 計画のある Task の WU の run からは delegate.json を
            // 使えない（部をまたぐ委譲は Task 単位。D21）。
            extras.available_genres = Vec::new();
        }
        // ADR-0079 D7（Phase R3a）: 木の節点の worker の run は `result.json` の `decisions` で決定の要求を出せる。
        // ADR-0124 D4: 直行経路の implementation run にだけ worker への節を渡す。
        if current_wu.is_none() && !is_planner_dispatch {
            let route = task.routing.as_ref().and_then(|r| r.route.as_ref());
            extras.direct_route = route
                .filter(|_| admitted.direct_route)
                .map(direct_route_context);
        }
        extras.decision_requests = !is_planner_dispatch
            && self.config.execution.limits.tree.enabled
            && self.is_tree_node(&task).unwrap_or(false);
        // ADR-0072 D14/D9（Phase E3）: planner run は、Task の担当が属する部署の**lead ノード**
        // （`department_of` が返す department ノードそのもの。ADR-0033 D1 の組織の木では
        // department ノード自身が「その部署の実効 profile」を持つ）の実効 profile で走る。
        // `node_sessions` は resume しない（対話タスクではないので、そもそも継続セッションの
        // 判定に掛からない。D9/D14）。
        if is_planner_dispatch {
            extras.execution_planner = Some(self.execution_planner_context(
                &task,
                original_task_budget,
                admitted.replan,
            )?);
            // ADR-0072 D14（Phase E4b 項目3）: `[execution.planner].permission_mode`
            // （既定 `"plan"`）を、この run の実際の CLI 引数として `run_worker` に反映させる
            // （`RunContext` には乗せない。プロンプトではなく実行そのものの配線）。
            extras.planner_permission_mode =
                Some(self.config.execution.planner.permission_mode.clone());
            // ADR-0072 D5（Phase E3）: `runs` 索引に planner run の行を作る（WU の
            // `start_work_unit_run` と同じ役目。`role = planner`、`work_unit_id = None`）。
            let planner_seq = self
                .store
                .runs_for_task(task.id)
                .map(|rs| {
                    rs.iter()
                        .filter(|r| r.role == task_core::RunIndexRole::Planner)
                        .count() as u32
                        + 1
                })
                .unwrap_or(1);
            if let Err(e) = self.store.run_index_start(task_core::RunRow {
                run_id: run_id.clone(),
                task_id: task.id.to_string(),
                work_unit_id: None,
                role: task_core::RunIndexRole::Planner,
                seq: planner_seq,
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
                tracing::warn!(task_id = %task.id, %run_id, error = %e, "failed to record the planner run start in the runs index");
            }
            if let Ok(org) = self.store.org_list()
                && let Some(dept_id) = task
                    .assignee
                    .as_deref()
                    .and_then(|a| task_core::department_of(&org, a))
                && let Some(dept_node) = org.iter().find(|n| n.id == dept_id)
            {
                let effective = task_core::resolve_profile(&org, &dept_node.id);
                extras.profile = if effective.is_trivial() {
                    None
                } else {
                    Some(effective.with_task(&task))
                };
                extras.node = Some(NodeContext {
                    id: dept_node.id.clone(),
                    name: dept_node.name.clone(),
                    brief: dept_node.brief.clone(),
                });
            }
        }
        // ADR 2026-10-11 llm-source-harness-selection: `runs` 索引の行（WU・planner・暗黙の WU）を書いた後に、
        // 選んだ harness・source・model・account を events と run の行へ記録する。
        if let Err(e) = self.record_route_selection(
            task.id,
            Some(&run_id),
            &route_input,
            &route_outcome,
            &route_trace,
        ) {
            tracing::warn!(task_id = %task.id, %run_id, error = %e, "failed to record the llm route selection");
        }
        extras.llm_route = Some(llm_route);
        extras.campaign = campaign_pin;
        // ADR-0052 D2: フォールバックの前置き（LangMem に渡しているのと同じ抽出の指示 + 出力契約）を
        // 役割の指示文として載せる。依頼文（`maintenance_objective`）は `task.objective` のまま。
        if let Some(fallback) = &knowledge_fallback {
            extras.role = Some(RoleContext {
                id: task
                    .role
                    .clone()
                    .unwrap_or_else(|| task_core::BUILTIN_KNOWLEDGE.to_string()),
                instructions: fallback.instructions.clone(),
            });
            extras.knowledge_fallback = knowledge_fallback.clone();
        }
        // ADR-0043 D2: 中止されたときに片付けられるよう、この run で使う作業場所を覚えておく。
        if let Some(ws) = &task_worktree {
            self.maint.remember_task_workspaces(task.id, ws.clone());
        }
        // ADR-0074 D4（Phase F3 quota）: 観測の `before` を記録する。ADR-0076: planner run も同じく
        // 登録する（`on_planner_finished` が `RoutingRuntime::settle_quota` で閉じる）。
        self.quota_begin(account.as_deref(), account_adapter, &run_id);
        // ADR-0074「R7-11 実装時の明確化」: 上で書いた予算（planner / WU / 知識整理のフォールバック）は手元の写しにしか
        // 無い。`run_worker` は DB から task を読み直すので、実効の予算を必ず渡す（`wall` だけでなく `max_turns` も効かせる）。
        extras.budget = Some(task.budget);
        // ADR-0130 D2: 実装 run の開始 HEAD を worker を起こす前に固定する（planner run は書かない）。
        if !is_planner_dispatch {
            self.capture_run_write_bases(&task, &run_id, worktree.as_ref());
        }
        // 多目的 routing Phase 3（ADR 2026-10-04 §3.4・§6）: task / WU / 実効 profile / 履歴から RoutingContext を
        // 組み、features を記録し、registry があれば run に結んで ref を worker の文脈に載せる。
        let routing_events: Vec<Event> = match self.store.events_for(task.id) {
            Ok(events) => events.into_iter().map(|(_, e)| e).collect(),
            Err(e) => {
                tracing::warn!(task_id = %task.id, %run_id, error = %e, "failed to read events for the routing context");
                Vec::new()
            }
        };
        extras.routing_context_ref = self.record_routing_context(
            &routing_context::RoutingContextInput {
                task: &task,
                work_unit: current_wu.as_ref(),
                run_id: &run_id,
                role: if is_planner_dispatch {
                    routing_context::RoutingRunRole::Planner
                } else {
                    routing_context::RoutingRunRole::Worker
                },
                harness: &adapter_id,
                profile: extras.profile.as_ref(),
                cluster: cluster.as_ref().map(|(spec, ..)| spec.id.as_str()),
                events: &routing_events,
            },
            routing_decision_id.as_deref(),
            ttl,
        );
        let handle = self.spawn_worker(
            task.id,
            task.worker_hint.tier,
            run_id.clone(),
            source_id.clone(),
            account.clone(),
            account_adapter,
            adapter,
            dir,
            limits,
            remote,
            worktree,
            extras,
            container,
        );
        let key = RunKey {
            task: task.id,
            work_unit: v2_wu.as_ref().map(|w| w.id.clone()),
        };
        self.coord.reserve_write_set(key.clone(), write_reservation);
        self.supervisor.register_run(
            key,
            RunEntry {
                run_id,
                provider: source_id,
                handle,
                since: OffsetDateTime::now_utc(),
                cluster: cluster.map(|(spec, ..)| spec.id),
                account,
                account_adapter,
                container: container_stop,
                cos: admitted.cos_run,
                route: Some((pick.candidate.source, pick.account)),
            },
        );
        self.maint.admission().started(disk);
        Ok(true)
    }

    /// Phase 38（ADR-0028 追記。実機のレビュー不合格から）: 計画が**ハーネスで動く分野**の担当に
    /// 「自分で決めた名前のファイルを書け」と要求していたら、その `artifact_exists` の条件を落として
    /// `objective` に本当の成果物の名前を注記する（`task_core::plan::fix_harness_artifacts`）。
    /// 壊さず直す（Plan run は失敗させず、`Question` にもしない）。判定は決定的で LLM は呼ばない
    /// （DESIGN 原則 1）。直した事実は `warn` に残す。
    pub(super) fn fix_plan_for_harness(
        &self,
        task: &Task,
        plan: &mut PlanOutput,
        org: &[task_core::OrgNode],
    ) {
        for note in task_core::fix_harness_artifacts(
            plan,
            task,
            org,
            &self.config.roles,
            &self.config.genres,
        ) {
            tracing::warn!(task_id = %task.id, "{note}");
        }
        // ADR-0063 D3（Phase 109）: 調査系（literature/web-research）の受け入れ条件に部分達成の
        // 逃げ道が無ければ**警告**（拒否はしない。決定的、LLM は使わない）。
        for note in task_core::warn_missing_partial_ok(
            plan,
            task,
            org,
            &self.config.roles,
            &self.config.genres,
        ) {
            tracing::warn!(task_id = %task.id, "{note}");
        }
        // ADR-0069 D1（Phase 114）: 計画（LLM）が書いた担当は使わない（子の `routing.dropped_assignee`
        // に残り、担当は matching が決める）。捨てた事実をここで 1 行ずつ残す。
        for (index, t) in plan.tasks.iter().enumerate() {
            if let Some(a) = t.assignee.as_deref().filter(|a| !a.trim().is_empty()) {
                tracing::info!(task_id = %task.id, index, dropped_assignee = %a, "plan-supplied assignee ignored; matching decides (ADR-0069 D1)");
            }
        }
    }

    /// ADR-0107 D2: browser fallback candidates for one run. Only declared harnesses whose
    /// adapter id is a browser backend conformant in the runner-recorded ledger are offered,
    /// ordered specialist first and one per adapter id. A `CredentialUse` policy never
    /// falls back (credential waits and auth sections must not be replayed). When a browser task
    /// ends up without any candidate the refusal is recorded as a progress event.
    pub(super) fn browser_fallback_candidates(
        &self,
        task_id: TaskId,
        run_id: &str,
        primary_adapter: &str,
        record: Option<&std::path::Path>,
    ) -> Vec<Arc<dyn WorkerAdapter>> {
        let task = match self.store.get(task_id) {
            Ok(Some(task)) => task,
            _ => return Vec::new(),
        };
        if !task_core::browser::requests_browser(&task.skills) {
            return Vec::new();
        }
        let credential_use = self
            .store
            .browser_task_policy_get(task_id)
            .ok()
            .flatten()
            .is_some_and(|p| {
                p.allowed_actions
                    .contains(&task_core::BrowserAction::CredentialUse)
            });
        if credential_use {
            self.record_browser_fallback_refusal(
                task_id,
                run_id,
                "browser fallback refused: CredentialUse policy is never replayed on another backend",
            );
            return Vec::new();
        }
        let conformant = match record.map(task_worker::browser::conformant_backend_ids) {
            Some(Ok(ids)) => Some(ids),
            Some(Err(_)) | None => None,
        };
        let mut excluded: Vec<String> = Vec::new();
        let mut candidates: Vec<(String, String, Arc<dyn WorkerAdapter>)> = Vec::new();
        let mut harnesses: Vec<(&task_core::llm_route::HarnessId, &Arc<dyn WorkerAdapter>)> =
            self.routing.harness_adapters().collect();
        harnesses.sort_by(|a, b| a.0.cmp(b.0));
        for (harness, adapter) in harnesses {
            let id = adapter.id();
            if id == primary_adapter || !task_worker::browser::BROWSER_BACKEND_IDS.contains(&id) {
                continue;
            }
            if conformant.as_ref().is_some_and(|ids| ids.contains(id)) {
                candidates.push((id.to_string(), harness.to_string(), Arc::clone(adapter)));
            } else {
                excluded.push(format!("{harness}=not_conformant"));
            }
        }
        candidates.sort_by(|a, b| {
            (a.0 != "browser-specialist", &a.0, &a.1).cmp(&(
                b.0 != "browser-specialist",
                &b.0,
                &b.1,
            ))
        });
        candidates.dedup_by(|a, b| a.0 == b.0);
        if candidates.is_empty() {
            let ledger = if conformant.is_some() {
                "ledger loaded"
            } else {
                "ledger unavailable"
            };
            self.record_browser_fallback_refusal(
                task_id,
                run_id,
                &format!(
                    "browser fallback refused: no conformant alternate backend ({ledger}; excluded: [{}])",
                    excluded.join(", ")
                ),
            );
        }
        candidates
            .into_iter()
            .map(|(_, _, adapter)| adapter)
            .collect()
    }

    fn record_browser_fallback_refusal(&self, task_id: TaskId, run_id: &str, msg: &str) {
        let ev = Event::WorkerProgress {
            run_id: run_id.to_string(),
            msg: msg.to_string(),
            kind: None,
            tool: None,
            summary: None,
            detail: None,
            truncated: false,
            error: false,
        };
        if let Err(e) = self.store.append_event(task_id, &ev) {
            tracing::warn!(task_id = %task_id, error = %e, "failed to record the browser fallback refusal (ADR-0107 D2)");
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn spawn_worker(
        &self,
        task_id: TaskId,
        execution_tier: task_core::Tier,
        run_id: String,
        provider: ProviderId,
        account: Option<String>,
        account_adapter: Option<AccountAdapter>,
        adapter: Arc<dyn WorkerAdapter>,
        dir: PathBuf,
        limits: RunLimits,
        remote: Option<SshSettings>,
        worktree: Option<task_worker::TaskWorkspaces>,
        mut extras: RunExtras,
        container: ContainerDecision,
    ) -> JoinHandle<()> {
        // ADR 2026-10-05 cos-chat-home D4: pin された添付は作業する run にだけ stage する
        // （planner は計画するだけ。reviewer は別経路で、ここを通らない）。
        if extras.planner_permission_mode.is_none() && extras.conversation_addressee.is_none() {
            extras.input_attachment_source = self.input_attachment_source();
        }
        let store = self.store.clone();
        // ADR-0107 D2: the dispatcher builds the browser fallback list from the declared harnesses
        // and the runner-recorded ledger; the worker wraps each candidate like the primary (ADR-0107).
        let browser_candidates = self.browser_fallback_candidates(
            task_id,
            &run_id,
            adapter.id(),
            task_worker::browser::conformance_record_path().as_deref(),
        );
        let tx = self.coord.completion_sender();
        let lease = LeaseRenewal {
            ttl: self.config.idle_timeout + self.config.lease_grace,
            every: self.config.lease_grace / 2,
        };
        let roles = self.config.roles.clone();
        let genres = self.config.genres.clone();
        let delegation = self.config.delegation;
        let account_book = account_adapter.and_then(|a| self.account_book(a));
        // ADR-0066 D1（Phase 110b）: `[workspace] shared_build_cache`（既定 true）。
        // ADR-0075 D3（Phase G1）: `[scratch]` が有効なら scratch pool、無効なら `build_cache_dir`。
        let cargo_target = if !self.config.shared_build_cache {
            CargoTargetPlan::None
        } else if self.scratch_active() {
            CargoTargetPlan::Scratch {
                settings: Box::new(self.config.scratch.clone()),
                candidates: self.maint.scratch_candidates(),
            }
        } else {
            CargoTargetPlan::Legacy(self.config.build_cache_dir.clone())
        };
        #[cfg(test)]
        let test_worker_start_delay = self.test_hooks.worker_start_delay;
        tokio::spawn(async move {
            #[cfg(test)]
            if let Some(delay) = test_worker_start_delay {
                tokio::time::sleep(delay).await;
            }
            let result = run_worker(
                store,
                adapter,
                browser_candidates,
                task_id,
                execution_tier,
                dir,
                &run_id,
                limits,
                lease,
                remote,
                worktree,
                extras,
                roles,
                genres,
                delegation,
                account,
                account_book,
                container,
                cargo_target,
            )
            .await;
            let _ = tx.send(Completion::Worker {
                task_id,
                run_id,
                provider,
                result,
            });
        })
    }

    /// `ready_tasks` の取得件数。経路なしと分かっているタスク（`warned_unroutable`）の分だけ広げ、それらが窓を埋めて
    /// 後ろの実行可能なタスクが dispatch されない・`is_idle` が誤って真になることを防ぐ（ADR-0012 監査）。
    pub(super) fn ready_window(&self) -> usize {
        self.coord.max_concurrency() * 4 + 16 + self.routing.warned_unroutable_count()
    }

    /// ADR-0043 D4: レビュー担当の必須の検査コマンド（ADR 2026-10-23-required-checks-bound-to-candidate:
    /// タスクの `acceptance` で置き換えない。実行は `review.rs`）。**先頭のリポジトリの** `[commands] check` だけを使う。
    /// ADR-0069 D3 / D6（Phase 114）: このタスクの lane を決める（`routing` を持つ execute タスクだけ。
    /// それ以外は `None` で従来どおり `worker_hint.tier`）。担当の実効 profile の天井（`allowed_tiers` /
    /// `budget.max_lane`）で丸め、やり直し（`attempts > 0`）ならイベントの軌跡から
    /// `EscalationPolicy::decide_trajectory` で 1 段まで上げる。上げた/上げなかった決定を
    /// `EscalationAudit` として返す（`RoutingRecord.escalation` に残す）。LLM は使わない（DESIGN 原則 1）。
    pub(super) fn decide_lane(
        &mut self,
        task: &Task,
    ) -> Result<Option<(task_core::LaneDecision, Option<task_core::EscalationAudit>)>, DispatchError>
    {
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
        let Some(mut decision) = task_core::model_policy::decide_for_task(task, &ceiling) else {
            return Ok(None);
        };
        // 付記 S2-1・S2-6: `lane_rules` の mode が compare のときだけ、同じ features を TS に渡して比べ、記録だけを残す。
        // policy が決める task（`Hint`・`Default`）だけが対象で、人の明示・System の task では TS を呼ばない。
        // ここで使う lane は Rust の結果のまま（retry の escalation の前の、天井で丸めた lane を比べる）。
        // 付記 S5-3: canary の TS の側と `ts` は、Rust の検証（出力の形・天井）を通った TS の lane を使う。
        if decision.source.policy_decides()
            && let Some(lane) =
                self.compare_lane_rules(task.id, &decision.features, decision.lane, &ceiling)
        {
            decision.lane = lane;
        }
        let mut audit = None;
        // 人の明示・System の lane も評価する（上げず、理由を audit に残す。`decide_trajectory` が判定）。
        if task.attempts > 0 {
            let events: Vec<Event> = self
                .store
                .events_for(task.id)?
                .into_iter()
                .map(|(_, e)| e)
                .collect();
            let (history, interval_id) =
                task_core::retry_policy::attempt_history_with_interval(task, &events);
            let thresholds = task_core::EscalationThresholds {
                org_ceiling: ceiling.clone(),
                ..self.routing.escalation().clone()
            };
            let policy = task_core::EscalationPolicy::for_task_with_thresholds(
                task,
                profile.as_ref(),
                &thresholds,
            );
            let next = policy.decide_trajectory(
                &history,
                decision.lane,
                decision.source,
                task_core::BudgetState::Ok,
                &thresholds,
                &interval_id,
            );
            let described = describe_escalation(&next);
            if next.selected_lane != decision.lane {
                decision.reasons.push(format!(
                    "retry lane {:?} -> {:?}",
                    decision.lane, next.selected_lane
                ));
            }
            decision.lane = next.selected_lane;
            decision.escalation = Some(described.clone());
            tracing::info!(task_id = %task.id, attempts = task.attempts, decision = %described, "retry lane decided (ADR-0069 D6 / ADR 2026-10-04 §5)");
            audit = Some(next);
        }
        Ok(Some((decision, audit)))
    }

    /// ADR-0046 D5（Phase 59）: `assignee` が無い `ready` のタスクの担当を**決定的に**決める。
    ///
    /// - 決まったら `Event::Assigned { node, score, reason }` を残して担当を書き戻し、そのタスクを返す。
    /// - 候補が 1 つも無ければ `blocked` にして人に聞き（ADR-0021 の質問経路）、`None` を返す。
    /// - matching の対象でない（担当が居る・ハーネスが無い）タスクはそのまま返す。
    ///
    /// LLM は使わない（DESIGN 原則 1）。
    pub(super) fn assign_if_needed(&mut self, task: Task) -> Result<Option<Task>, DispatchError> {
        use task_ops::matching::Assignment;
        let org = self.store.org_list()?;
        // variant 付き試行（campaign 識別に variant_org を持つ task とその子孫）は、matching の前に
        // 共有 org が pin と一致するかを確かめる。解決・照合の失敗は blocked＋理由 event（適用済みと扱わない）。
        // org は読むだけで書き換えない。
        let pin = match self.variant_org_pin(&task) {
            Ok(pin) => pin,
            Err(reason) => return self.block_variant_task(&task, reason),
        };
        if let Some(pin) = &pin
            && let Err(reason) = pin.check_org(&org)
        {
            return self.block_variant_task(&task, reason);
        }
        let decision = task_ops::matching::decide(&org, &task);
        if let Some(pin) = &pin {
            let assignee = match &decision {
                Assignment::Assigned { node, .. } => Some(node.as_str()),
                Assignment::NotApplicable => task.assignee.as_deref(),
                Assignment::Unroutable { .. } => None,
            };
            if let Some(a) = assignee
                && let Err(reason) = variant_pin::check_assignee(pin, a)
            {
                return self.block_variant_task(&task, reason);
            }
        }
        match decision {
            Assignment::NotApplicable => Ok(Some(task)),
            Assignment::Assigned {
                node,
                score,
                reason,
            } => {
                let mut updated = task.clone();
                updated.assignee = Some(node.clone());
                updated.updated_at = OffsetDateTime::now_utc();
                let event = Event::Assigned {
                    node: node.clone(),
                    score,
                    reason: reason.clone(),
                };
                match self.store.update_task(&updated, event) {
                    Ok(stored) => {
                        tracing::info!(
                            task_id = %task.id, assignee = %node, score, reason = %reason,
                            "matching decided the assignee (ADR-0046 D5)"
                        );
                        Ok(Some(stored))
                    }
                    Err(e) => {
                        tracing::warn!(task_id = %task.id, error = %e, "could not write the matched assignee");
                        Ok(Some(task))
                    }
                }
            }
            Assignment::Unroutable { question } => {
                // Phase 44 と同じ規律: ディスパッチャ由来の質問も `approvals` に残す（そうしないと
                // 認可画面に出ず、Discord にも飛ばない）。
                let now = OffsetDateTime::now_utc();
                if let Err(e) = crate::approvals::record_question_approval(
                    self.store.as_ref(),
                    &task,
                    &question,
                    now,
                ) {
                    tracing::warn!(task_id = %task.id, error = %e, "failed to record the approval for the unroutable question");
                }
                let run_id = format!("matching-{}", task.id);
                let events = vec![Event::QuestionRaised {
                    run_id,
                    text: question,
                }];
                match self
                    .store
                    .apply_transition_with_events(task.id, Trigger::Unroutable, events)
                {
                    Ok(_) => {
                        tracing::info!(task_id = %task.id, "no org node can take this task; asking a human (ADR-0046 D5)")
                    }
                    Err(StoreError::InvalidTransition(e)) => {
                        tracing::warn!(task_id = %task.id, error = %e, "unroutable transition could not be applied");
                    }
                    Err(e) => return Err(e.into()),
                }
                Ok(None)
            }
        }
    }

    /// ADR-0062 B1（Phase 107）: 担当が `cluster:<id>` を持たない remote タスクを `blocked` にし、
    /// `assign_if_needed` の `Assignment::Unroutable` と同じ流儀（`approvals` に 1 件、
    /// `Trigger::Unroutable` で `ready → blocked`）で人に質問する。人が答える（または担当・tools を
    /// 変える）と次の tick で `ready` に戻り、そのとき改めて `cluster_of` が評価し直す。
    pub(super) fn block_task_missing_cluster_tool(
        &mut self,
        task: &Task,
        cluster: &str,
    ) -> Result<(), DispatchError> {
        let assignee = task.assignee.as_deref().unwrap_or("(unknown)");
        let wanted = format!("{}{cluster}", task_core::CLUSTER_TOOL_PREFIX);
        let org = self.store.org_list().unwrap_or_default();
        let holders: Vec<&str> = org
            .iter()
            .filter(|n| task_core::resolve_profile(&org, &n.id).has_tool(&wanted))
            .map(|n| n.id.as_str())
            .collect();
        let question = format!(
            "担当 `{assignee}` には道具 `{wanted}` が無いため、このタスクは {cluster} で実行できません。\
             組織画面で担当の tools に `{wanted}` を足す{}、または作業場所をローカルに変えてください。",
            if holders.is_empty() {
                "か、担当を変える".to_string()
            } else {
                format!("、担当を `{}` などに変える", holders.join("` / `"))
            }
        );
        let now = OffsetDateTime::now_utc();
        if let Err(e) =
            crate::approvals::record_question_approval(self.store.as_ref(), task, &question, now)
        {
            tracing::warn!(task_id = %task.id, error = %e, "failed to record the approval for the missing-cluster-tool question");
        }
        let run_id = format!("cluster-routing-{}", task.id);
        let events = vec![Event::QuestionRaised {
            run_id,
            text: question,
        }];
        match self
            .store
            .apply_transition_with_events(task.id, Trigger::Unroutable, events)
        {
            Ok(_) => {
                tracing::info!(task_id = %task.id, %assignee, %cluster, "assignee lacks the cluster tool; blocked and asked a human (ADR-0062 B1)");
            }
            Err(StoreError::InvalidTransition(e)) => {
                tracing::warn!(task_id = %task.id, error = %e, "missing-cluster-tool transition could not be applied");
            }
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }

    /// A refused route (no harness, no eligible declared candidate, or a pick that cannot be
    /// launched) leaves the task ready and starts nothing. The refusal is recorded once per task
    /// until a route is admitted again (`warned_unroutable`, which also widens the ready window),
    /// so a deferred task does not append an event every tick. A refusal that only a config
    /// change can lift (anything but "every declared candidate is busy or ineligible now") counts
    /// the task as unroutable.
    fn note_route_refusal(
        &mut self,
        task_id: TaskId,
        input: &route_select::RouteInput,
        outcome: &Result<route_select::RoutePick, route_select::RouteFailure>,
        trace: &route_select::RouteTrace,
        launch_error: Option<&RouteLaunchError>,
    ) -> Result<(), DispatchError> {
        if !matches!(outcome, Err(route_select::RouteFailure::NoEligible(_))) {
            self.routing.mark_unroutable(task_id);
        }
        if !self.routing.warn_unroutable_once(task_id) {
            return Ok(());
        }
        match (outcome, launch_error) {
            (Ok(pick), Some(error)) => {
                tracing::info!(task_id = %task_id, harness = %pick.harness, source = %pick.candidate.source, %error, "llm route cannot be launched; deferring");
                self.store.append_event(
                    task_id,
                    &Event::worker_progress(
                        "routing",
                        format!(
                            "route refused: {error}; selection: {}",
                            trace.metadata(input, Some(pick), None)
                        ),
                    ),
                )?;
                Ok(())
            }
            (outcome, _) => {
                if let Err(failure) = outcome {
                    tracing::info!(task_id = %task_id, reason = %failure.reason(), "no llm route; deferring");
                }
                self.record_route_selection(task_id, None, input, outcome, trace)
            }
        }
    }

    pub(super) fn default_checks(&self, task: &Task) -> Vec<String> {
        let Some(ws) = self.task_workspaces_for(task) else {
            return Vec::new();
        };
        let Some(repo) = ws.repos.first() else {
            return Vec::new();
        };
        let from = if repo.dir.is_dir() {
            &repo.dir
        } else {
            &repo.source
        };
        task_core::workspace_config::load_or_default(from)
            .0
            .commands
            .check
    }
}

/// ADR-0124 D4: `RouteDecision` を worker に渡す要約（満たした条件の行）にする。
fn direct_route_context(
    decision: &task_core::RouteDecision,
) -> task_worker::protocol::DirectRouteContext {
    task_worker::protocol::DirectRouteContext {
        policy_version: decision.policy_version.clone(),
        overrode_gate: decision.overrode_gate,
        reasons: decision
            .reasons
            .iter()
            .filter(|r| r.ok)
            .map(|r| format!("{}: {}", r.rule_id, r.detail))
            .collect(),
    }
}

/// 軌跡 escalation の監査を 1 行にする。上げた決定は `escalate <from> -> <to>: <理由>` で始まる
/// （旧 `RetryDecision::describe` と同じ書式）。上げなかった決定は `retry at <lane>: <理由>`。
fn describe_escalation(audit: &task_core::EscalationAudit) -> String {
    let current = audit
        .previous_lane
        .filter(|l| {
            task_core::model_policy::lane_rank(*l)
                >= task_core::model_policy::lane_rank(audit.requested_lane)
        })
        .unwrap_or(audit.requested_lane);
    if task_core::model_policy::lane_rank(audit.selected_lane)
        > task_core::model_policy::lane_rank(current)
    {
        format!(
            "escalate {current:?} -> {:?}: {}",
            audit.selected_lane, audit.reason
        )
    } else {
        format!("retry at {:?}: {}", audit.selected_lane, audit.reason)
    }
}

/// The lane label kept on the routing request (the default route policy does not read it).
pub(super) fn tier_label(tier: task_core::Tier) -> &'static str {
    match tier {
        task_core::Tier::Frontier => "frontier",
        task_core::Tier::Standard => "standard",
        task_core::Tier::Cheap => "cheap",
    }
}

/// ADR 2026-11-14 dispatch-admission-order D2: WU の gate（dispatcher の非公開の型）を admit の要る値に落とす。
fn admission_unit(gate: &WuDispatchGate) -> AdmissionUnit {
    match gate {
        WuDispatchGate::Atomic => AdmissionUnit::Atomic,
        WuDispatchGate::RunWorkUnit(wu) => AdmissionUnit::WorkUnit {
            budget: wu.spec.budget,
            phased: wu.phase.is_some(),
        },
        WuDispatchGate::RunPlanner { replan } => AdmissionUnit::Planner { replan: *replan },
        WuDispatchGate::StartIntegration(_) => AdmissionUnit::StartIntegration,
        WuDispatchGate::FinalReview => AdmissionUnit::FinalReview,
        WuDispatchGate::Skip => AdmissionUnit::Skip,
    }
}

/// WU の gate が走らせる WU（`RunWorkUnit` のときだけ）。
fn gate_wu(gate: &Option<WuDispatchGate>) -> Option<&task_core::WorkUnitRow> {
    match gate {
        Some(WuDispatchGate::RunWorkUnit(wu)) => Some(wu),
        _ => None,
    }
}
