//! run の起動（`dispatch_one`・`spawn_worker`）と、その前段の担当・lane・harness の決定。lease の取得から `running.insert` までは一続きに保つ。ADR-0082 の L4。

use super::*;

impl Dispatcher {
    /// `dispatch_ready` の 1 件分（ADR-0074 D1.3（Phase F2b）で切り出した。並列 WU の 2 本目以降も
    /// ここを通る）。run を起こしたら `Ok(true)`。
    pub(super) fn dispatch_one(
        &mut self,
        task: Task,
        forced_wu: Option<task_core::WorkUnitRow>,
        full: &mut std::collections::HashSet<ProviderId>,
        now: Instant,
    ) -> Result<bool, DispatchError> {
        // ADR-0074 D1.3（Phase F2b）: 並列 WU の 2 本目以降（Task は既に Running。担当・gate・
        // バックオフは 1 本目で済んでいる）。
        let second_pass = forced_wu.is_some();
        if !second_pass && self.running_for_task(task.id) > 0 {
            return Ok(false);
        }
        // ADR-0089（Phase R6-5）: CoS の対話 run は `max_concurrency` ではなく `max_cos_runs` で数える。
        let cos = !second_pass && self.is_cos_task(&task)?;
        if !self.run_load().admits(cos) {
            return Ok(false);
        }
        // ADR-0044 D2: この tick で打ち切ったばかりの run と同じ worktree に、すぐ次の run を
        // 入れない（孫プロセスが片付く猶予を 1 tick 置く）。
        if self.just_aborted.contains(&task.id) {
            return Ok(false);
        }
        // ADR-0041 D5: verify モードは `genre = "smoke"` の煙試験だけを起こす（他は ready のまま）。
        if !self.is_eligible(&task) {
            return Ok(false);
        }
        // ADR-0046 D5（Phase 59）: 担当が決まっていないタスクは dispatch の前に matching で決める
        // （計画 run の子、人が作ったタスク、Console から作られたタスクが全部ここを通る）。
        let mut task = if second_pass {
            task
        } else {
            match self.assign_if_needed(task)? {
                Some(task) => task,
                // 候補が無くて `blocked` にした（人に聞いた）。この tick では dispatch しない。
                None => return Ok(false),
            }
        };
        // ADR-0072 D13（Phase E3）: Complexity Gate（assign_if_needed の後、decide_lane の前。
        // 最初の dispatch で 1 回だけ判定する）。
        if !second_pass {
            task = self.execution_gate_if_needed(task)?;
            // ADR-0079 D9 / D7（Phase R3a）: `plan_invalid` に atomic と答えた節点は 1 run で走らせる。
            task = self.apply_plan_invalid_atomic(task)?;
        }
        // ADR-0072 D6/D15（Phase E2）: 計画のある Task は、次に走らせる WorkUnit を
        // 決定的な scheduler（`task_core::next_work_unit`）で選ぶ。計画が無ければ従来どおり
        // （`current_wu = None`。プロンプト・遷移は E1 までと 1 バイトも変わらない。(i)）。
        let mut replan_dispatch = false;
        let gate = match forced_wu {
            Some(wu) => WuDispatchGate::RunWorkUnit(Box::new(wu)),
            None => self.wu_dispatch_gate(task.id)?,
        };
        // ADR-0079 D9（Phase R2b）/ D7（Phase R3a）: `needed_before: [self]` の未回答の決定（plan_invalid・run 時の
        // 木の上限・worker の self）を待つ task は run を起こさない。
        if !second_pass && self.decision_self_hold(&task)? {
            return Ok(false);
        }
        // ADR-0079 D3（Phase R2a）: 木の上限（run・トークン・replan）。超えるなら新しい run を起こさず、
        // `kind: limit` の決定の要求を出して（木に 1 件）この節点だけを止める（兄弟の走っている run は続く）。
        if self.tree_run_limit_hold(&task, &gate)? {
            return Ok(false);
        }
        let current_wu = match gate {
            WuDispatchGate::Atomic => None,
            WuDispatchGate::RunWorkUnit(wu) => Some(*wu),
            WuDispatchGate::StartIntegration(wu) => {
                self.start_integration_from_ready(&task, &wu)?;
                return Ok(false);
            }
            // ADR-0074「F5-fix8 実装時の明確化」: 仕事の残っていない計画の最終レビュー（run は起こさない）。
            WuDispatchGate::FinalReview => {
                self.start_final_review_from_ready(&task)?;
                return Ok(false);
            }
            // ADR-0072 D17（Phase E4）: replan の planner run。既存の `is_planner_dispatch` の
            // 配線（budget/lane/role の上書き）をそのまま使うが、`ExecutionPlannerContext.replan`
            // を `true` にする（下）。
            WuDispatchGate::RunPlanner { replan } => {
                replan_dispatch = replan;
                None
            }
            WuDispatchGate::Skip => return Ok(false),
        };
        // ADR-0072 D14（Phase E3）/ D17（Phase E4）: gate が compound と判定し、`gate = "on"` で、
        // まだ計画が無い（`current_wu` が None = atomic 経路）なら、この run は task-local な
        // planner run にする。`replan_dispatch` は既に計画がある Task の replan（`wu_dispatch_gate`
        // が上限まで確認済み）。
        // ADR-0074「Phase F3（途中確認）実装時の逸脱・明確化」: `gate = "shadow"` でも、人が
        // `execution: compound` を明示した Task（`ExecutionGateDecision.source = Human`、
        // `rule_id = human/explicit`）は採用して planner run に進む（F5-1 dogfood で見つかった
        // 不具合の修正）。CoS のヒント（source = Hint）と規則表の判定（source = Policy）は
        // shadow では従来どおり記録だけ（採用しない）。
        let gate_decision = task.routing.as_ref().and_then(|r| r.execution.as_ref());
        let decision_is_compound =
            gate_decision.map(|d| d.mode) == Some(task_core::ExecutionMode::Compound);
        let shadow_human_explicit_compound = self.config.execution.gate
            == task_core::GateMode::Shadow
            && decision_is_compound
            && gate_decision.map(|d| d.source) == Some(task_core::GateSource::Human);
        // ADR-0079 D4 (1)（Phase R2a）: 木の子 task の compound は `[execution] gate` に関わらず採用する
        // （root が compound で分けると決めた木を途中で 1 run に潰さない）。root は従来どおり（U-R5）。
        let tree_child_compound = decision_is_compound && task_core::tree::is_tree_child(&task);
        // ADR-0124 D3: 直行経路（`route = direct`・shadow でない）と記録した Task は planner を挟まず、
        // 計画の無い 1 本の implementation run で走る（その後は従来の review 前同期 → command checks →
        // reviewer）。`replan_dispatch` は計画を持つ Task だけなので触らない。
        let direct_route = task
            .routing
            .as_ref()
            .and_then(|r| r.route.as_ref())
            .filter(|r| r.route == task_core::Route::Direct && !r.shadow);
        let is_planner_dispatch = replan_dispatch
            || (current_wu.is_none()
                && direct_route.is_none()
                && decision_is_compound
                && (self.config.execution.gate == task_core::GateMode::On
                    || shadow_human_explicit_compound
                    || tree_child_compound));
        // D18/D14: 上書きする前の Task の予算（WU/planner の既定の計算に使う。ADR-0072 D14）。
        let original_task_budget = task.budget;
        // ADR-0074 D5.3（Phase F1）: planner run は `[execution.planner] tier`（既定 standard）で
        // 走る。人が Task に `tier:frontier` を明示していれば、それが優先される
        // （`TierSource::Human` かつ `worker_hint.tier == Frontier`）。
        let planner_human_frontier = task
            .routing
            .as_ref()
            .is_some_and(|r| r.tier_source == task_core::TierSource::Human)
            && task.worker_hint.tier == task_core::Tier::Frontier;
        if is_planner_dispatch {
            // D14: harness/adapter は `[execution.planner]`、lane は固定（下の `lane_decision` で
            // `TierSource::System`／人の明示なら `TierSource::Human` にする）。
            task.worker_hint.tier = if planner_human_frontier {
                task_core::Tier::Frontier
            } else {
                self.config.execution.planner.tier
            };
            task.worker_hint.adapter = Some(self.config.execution.planner.adapter.clone());
            task.budget.max_turns = self.config.execution.planner.max_turns;
            task.budget.max_wall_secs = self.config.execution.planner.max_wall_secs;
        } else if let Some(wu) = &current_wu {
            // ADR-0072 D18/E2 申し送り（Phase E3）: WU の予算（`WorkUnitSpec.budget`）を実際の
            // run の wall-clock/turn 上限に反映する。書かなければ D18 の既定
            // （`max(task.budget.*, 既定)`）。
            let default_max_turns = task.budget.max_turns.max(30);
            let default_max_wall = task.budget.max_wall_secs.max(1800);
            task.budget.max_turns = wu
                .spec
                .budget
                .and_then(|b| b.max_turns)
                .unwrap_or(default_max_turns);
            task.budget.max_wall_secs = wu
                .spec
                .budget
                .and_then(|b| b.max_wall_secs)
                .unwrap_or(default_max_wall);
        }
        // ADR-0010 D6（P-3）: ready に入った時刻（DB の updated_at）からのバックオフ。
        // 並列 WU の 2 本目以降（Task は Running）は 1 本目で済んでいるので見ない。
        if !second_pass && task.attempts > 0 {
            let delay = retry_backoff(
                self.config.retry_backoff_base,
                self.config.retry_backoff_max,
                task.attempts,
            );
            if self.now_utc() < task.updated_at + delay {
                tracing::debug!(task_id = %task.id, attempts = task.attempts, delay_ms = delay.as_millis() as u64, "retry backoff; not dispatching yet");
                return Ok(false);
            }
        }
        // ADR-0070 D3（Phase 116）: インフラ都合の再試行のバックオフ（`self.infra_backoff`。
        // `task.attempts` に依らない別軸。上のバックオフとは独立にゲートする）。期限を過ぎたら
        // このタスクへのゲートは外す（次に infra 失敗すればまた立て直す）。
        if !second_pass && let Some(until) = self.infra_backoff.get(&task.id).copied() {
            if self.now_utc() < until {
                tracing::debug!(task_id = %task.id, %until, "infra backoff; not dispatching yet");
                return Ok(false);
            }
            self.infra_backoff.remove(&task.id);
        }
        // ADR-0130 D3: 同じ repo で走っている run と expected write-set が強く重なれば、起動を次 tick に回す
        // （lease・遷移・attempts の前。見送りは `Ok(false)` だけ）。planner run は書き込みを予約しない。
        let run_key = RunKey {
            task: task.id,
            work_unit: current_wu
                .as_ref()
                .filter(|w| w.phase.is_some())
                .map(|w| w.id.clone()),
        };
        let write_reservation = if is_planner_dispatch || self.write_set_gate_disabled() {
            None
        } else {
            self.write_reservation_for(&task, current_wu.as_ref())?
        };
        if let Some(reservation) = &write_reservation
            && let Some(blocker) = self.write_set_blocker(&run_key, reservation)
        {
            self.note_write_set_hold(&run_key, &blocker, reservation);
            return Ok(false);
        }
        // ADR-0018: リモート実行のタスクは、クラスタの設定・cooldown・並列度・多重接続を先に確かめる。
        // ADR-0062 B1（Phase 107）: `cluster_of` が `None` の理由を分ける。(a) 設定に無いクラスタ
        // → 従来どおり `unroutable`（人が設定を直すまで進まない）。(b) 担当が `cluster:<id>` を
        // 持たない → `blocked` にして人に質問を 1 件作る（設定の問題ではなく担当の問題なので、
        // 「no such cluster in the config」という誤解を招く文言は出さない）。
        let cluster = match self.resolve_cluster(&task) {
            ClusterResolution::Local => None,
            ClusterResolution::Resolved(spec, path, mode) => Some((spec, path, mode)),
            ClusterResolution::NotConfigured => {
                if self.warned_unroutable.insert(task.id) {
                    let cluster_id = match &task.workspace {
                        WorkspaceSpec::Remote { cluster, .. } => cluster.clone(),
                        WorkspaceSpec::Local { .. } => String::new(),
                    };
                    tracing::warn!(task_id = %task.id, cluster = %cluster_id, "no such cluster in the config; task left ready");
                }
                self.unroutable.insert(task.id);
                return Ok(false);
            }
            ClusterResolution::AssigneeLacksTool { cluster } => {
                self.block_task_missing_cluster_tool(&task, &cluster)?;
                return Ok(false);
            }
        };
        if let Some((spec, _, _)) = &cluster {
            if self
                .cluster_cooldown
                .get(&spec.id)
                .is_some_and(|until| *until > now)
            {
                // ADR-0018 D2: 人がログインするまで進まないので、待ち対象には数えない（`--until-idle` を止めない）。
                self.cluster_waiting.insert(task.id);
                return Ok(false);
            }
            if self.cluster_in_use(&spec.id) >= spec.concurrency {
                return Ok(false);
            }
            // この tick の `refresh_cluster_liveness` の結果を使う（1 tick に 1 回だけ `ssh -O check` を呼ぶ）。
            let alive = self
                .cluster_connected
                .get(&spec.id)
                .copied()
                .unwrap_or(false);
            if !alive {
                let spec = spec.clone();
                // ADR-0032 D3: `auth = "publickey"` かつ接続フックがあれば、cooldown にする前に
                // 1 回だけ接続を試みる（cooldown 中はここに来ないので、tick ごとに ssh は湧かない。
                // 同じ tick の別タスクが同じクラスタを指していても、成功時は `cluster_connected` の
                // キャッシュが true になり、失敗時は下で cooldown が立つので、2 本目は走らない）。
                let attempt = self.try_auto_connect_cluster(&spec);
                if let Some(result) = &attempt {
                    self.note_key_auth_attempt(&spec, result.is_ok());
                }
                match attempt {
                    Some(Ok(())) => {
                        self.set_cluster_connected(&spec.id, true, ClusterConnChange::KeyAuth);
                    }
                    Some(Err(detail)) => {
                        self.mark_cluster_unavailable(
                            task.id,
                            &spec,
                            format!("auto-connect failed: {detail}"),
                        )?;
                        self.cluster_waiting.insert(task.id);
                        return Ok(false);
                    }
                    None => {
                        self.mark_cluster_unavailable(
                            task.id,
                            &spec,
                            format!(
                                "no ssh ControlMaster connection to {} (host {})",
                                spec.id, spec.host
                            ),
                        )?;
                        self.cluster_waiting.insert(task.id);
                        return Ok(false);
                    }
                }
            }
        }
        let dir_started = Instant::now();
        // ADR-0041 D1 / ADR-0043 D2: ローカルの作業場所（1 つ以上のリポジトリ）を用意する
        // （`dir` はその親 = `runs/` `artifacts/` の置き場）。
        let worktree = self.task_workspaces_for(&task);
        let dir = match &worktree {
            Some(ws) => ws.task_dir.clone(),
            None => match self.task_dir(&task) {
                Some(d) => d,
                None => {
                    tracing::warn!(task_id = %task.id, "cannot resolve the workspace directory; task left ready");
                    return Ok(false);
                }
            },
        };
        log_slow_step("task_dir", dir_started);
        // ADR-0074 D1.2（Phase F2b）: v2 の WU の run は、WU ごとの worktree
        // （`<task_dir>/wu/<key>/repos/<name>`、ブランチ `celeris-wu/<task_id>/<key>`）で走る。並列 1 に
        // 倒した Task・統合の repair WU は Task の worktree を共有する（`None`）。
        let v2_wu = current_wu.as_ref().filter(|w| w.phase.is_some()).cloned();
        let task_worktree = worktree.clone();
        let mut wu_workspace: Option<WorkUnitWorkspace> = None;
        if let Some(wu) = &v2_wu {
            // ADR-0074「Phase F5-fix7 実装時の明確化」: 一時的な失敗の後のバックオフ中は試さない。
            if let Some(f) = self.wu_prepare_failures.get(&wu.id)
                && self.now_utc() < f.retry_at
            {
                tracing::debug!(task_id = %task.id, work_unit = %wu.key, failures = f.count, retry_at = %f.retry_at, "work unit worktree backoff; not dispatching yet");
                return Ok(false);
            }
            match self.prepare_work_unit_workspace(&task, wu, task_worktree.as_ref()) {
                Ok(prepared) => {
                    self.wu_prepare_failures.remove(&wu.id);
                    wu_workspace = prepared;
                }
                Err(e) => {
                    self.on_work_unit_prepare_failed(&task, wu, &e, second_pass)?;
                    return Ok(false);
                }
            }
        }
        let worktree = match &wu_workspace {
            Some(w) => Some(w.workspaces.clone()),
            None => worktree,
        };
        // ADR-0052 D1 / D2（Phase 64）: 知識整理 run は dispatch の直前に `langmem` の接続先へ
        // `GET /models` を当て、届かなければ tier `cheap` の**汎用**ハーネスへ倒す
        // （`worker_hint.adapter` を外すだけ ＝ ADR-0049 の選び方にそのまま乗る）。LLM は呼ばない。
        let fallback_reason = self.knowledge_fallback_reason(&task, now);
        if let Some(reason) = &fallback_reason
            && let Some(tier) = self.config.knowledge.fallback_tier
        {
            task.worker_hint.adapter = None;
            task.worker_hint.tier = tier;
            task.budget.max_turns = KNOWLEDGE_FALLBACK_MAX_TURNS;
            task.budget.max_wall_secs = KNOWLEDGE_FALLBACK_MAX_WALL_SECS;
            tracing::info!(task_id = %task.id, %reason, ?tier, "knowledge: falling back to a generic harness");
        }
        // ADR-0069 D3 / D6（Phase 114）: `routing` を持つ execute タスクは、lane を決定的な policy
        // （TaskFeatures → 規則表 → 組織の天井）とリトライのエスカレーションで決める。人の明示・
        // System の tier はそのまま（記録だけ）。残量による調整はこの後の `select_tier`（別の層）。
        // ADR-0074 D5.3（Phase F1）: planner run は lane を丸めない固定の `[execution.planner]
        // tier`（既定 standard。E3〜E6 は frontier 固定だった）。人が Task に `tier:frontier` を
        // 明示していれば `TierSource::Human` として記録する。D21: WU の run は WU の view
        // （objective/acceptance/budget/genre/features を差し替えたもの）で lane を決める。
        let lane_decision = if is_planner_dispatch {
            let tier = task.worker_hint.tier;
            let (source, rule_id, reason) = if planner_human_frontier {
                (
                    task_core::TierSource::Human,
                    "planner/human-frontier".to_string(),
                    "human explicitly set tier:frontier on this task; the planner run \
                     inherits it (ADR-0074 D5.3)"
                        .to_string(),
                )
            } else {
                (
                    task_core::TierSource::System,
                    format!(
                        "planner/system-{}",
                        match tier {
                            task_core::Tier::Frontier => "frontier",
                            task_core::Tier::Standard => "standard",
                            task_core::Tier::Cheap => "cheap",
                        }
                    ),
                    "ADR-0074 D5.3: planner run runs at [execution.planner] tier, fixed by \
                     celeris code"
                        .to_string(),
                )
            };
            Some(task_core::LaneDecision {
                lane: tier,
                proposed: tier,
                source,
                rule_id,
                policy_version: task_core::LANE_POLICY_VERSION.to_string(),
                features: task_core::TaskFeatures::infer(&task),
                reasons: vec![reason],
                clamped_by: None,
                hint: None,
                escalation: None,
                shadow: None,
            })
        } else if let Some(wu) = &current_wu {
            self.decide_lane_for_work_unit(&task, wu)?
        } else {
            self.decide_lane(&task)?
        };
        if let Some(decision) = &lane_decision {
            task.worker_hint.tier = decision.lane;
        }
        // ADR-0054 Phase 67c: CoS の対話 run だけ、継続セッションの (adapter, account) に留まれるかを
        // 先に試す（`run_extras` の `is_cos_conversation` と同じ判定を select_provider より前に
        // 軽く行う。継続セッションを見つけてから選ぶのでないと、ADR-0049 ランキングが先に別の
        // アダプタ・アカウントへ倒れてしまう）。
        let sticky_session = self.cos_conversation_session(&task)?;
        // multi-objective routing Phase 2: mode=enforce（opt-in）だけ候補 allowlist と source 状態で選ぶ。
        // quota の枯渇・圧迫は lane を下げず、同じ lane の別 source を先に試し、無ければ defer する。
        let mut enforce_round = self
            .enforce_active()
            .then(|| self.enforce_round(&task.worker_hint));
        let mut enforce_full = full.clone();
        let (
            adapter_id,
            provider_id,
            selected_account,
            provider_selection,
            adapter,
            tier,
            routing_reason,
        ) = loop {
            // ADR-0132 付記 L2/L8: worker run だけ cheap lane のローカル優先を効かせ、選択の記録を残す。
            let (picked, provider_selection) = match &enforce_round {
                Some(round) => {
                    // enforce の除外は満杯の集合へ混ぜない（他の task の判定を汚さない）。
                    let excluded = round.excluded.clone();
                    let result = self.select_provider_excluding(
                        &task.worker_hint,
                        now,
                        task.id,
                        &mut enforce_full,
                        sticky_session.as_ref(),
                        cos,
                        true,
                        &excluded,
                    );
                    full.extend(enforce_full.iter().cloned());
                    result
                }
                None => self.select_provider_for(
                    &task.worker_hint,
                    now,
                    task.id,
                    full,
                    sticky_session.as_ref(),
                    cos,
                    true,
                ),
            };
            let Some((adapter_id, provider_id, selected_account)) = picked else {
                if let Some(round) = &enforce_round {
                    tracing::info!(task_id = %task.id, tier = ?task.worker_hint.tier, excluded = %round.summary(),
                        "routing enforce: no source in the requested lane; deferring (lane is not lowered)");
                }
                return Ok(false);
            };
            let Some(base_adapter) = self.adapters.get(&provider_id).cloned() else {
                tracing::warn!(task_id = %task.id, provider = %provider_id, adapter = %adapter_id, "no adapter instance for provider");
                return Ok(false);
            };
            // ADR-0024 D2 / ADR-0025 D2: プールで選んだアカウントの env を重ねる。`with_env` が `None` を返すのは
            // アダプタの実装漏れ（設定検証で account_pool は claude-code/codex 限定にしているため通常は起きない）
            // なので、このタスクは今回見送る。
            let adapter = match &selected_account {
                Some((account_adapter, account_id)) => {
                    match self.adapter_for_account(&base_adapter, *account_adapter, account_id) {
                        Some(a) => a,
                        None => {
                            tracing::warn!(task_id = %task.id, provider = %provider_id, account_id, "adapter does not support account pools (with_env returned None); skipping this tick");
                            return Ok(false);
                        }
                    }
                }
                None => base_adapter,
            };
            if let Some(round) = enforce_round.as_mut() {
                let state = self.source_state_of(&provider_id, selected_account.as_ref(), now);
                match self.enforce_check_source(task.worker_hint.tier, &state) {
                    Ok(reason) => {
                        round.observed_at = state.observed_at.clone();
                        break (
                            adapter_id,
                            provider_id,
                            selected_account,
                            provider_selection,
                            adapter,
                            task.worker_hint.tier,
                            reason,
                        );
                    }
                    Err((codes, detail)) => {
                        Self::enforce_exclude(round, &provider_id, &codes, detail);
                        continue;
                    }
                }
            }
            let remaining = selected_account.as_ref().and_then(|(kind, id)| {
                let book = self.account_book(*kind)?;
                let book = book.lock().ok()?;
                let observation = book.state(id)?.usage.as_ref()?;
                crate::accounts::measured_remaining(observation, (self.now_unix_fn)())
            });
            let (tier, routing_reason) =
                match task_core::model_routing::select_tier(task.worker_hint.tier, remaining) {
                    Ok(decision) => decision,
                    Err(_) => return Ok(false), // quota refresh will make this task eligible again
                };
            break (
                adapter_id,
                provider_id,
                selected_account,
                provider_selection,
                adapter,
                tier,
                routing_reason,
            );
        };
        // A legacy provider has no tier mapping: keep its historical behavior.
        if adapter
            .model_for_tier(task.worker_hint.tier)
            .ok()
            .flatten()
            .is_some()
        {
            task.worker_hint.tier = tier;
        }
        let resolved_model = match adapter.model_for_tier(task.worker_hint.tier) {
            Ok(model) => model,
            Err(reason) => {
                self.store.apply_transition_with_events(
                    task.id,
                    Trigger::Unroutable,
                    vec![Event::worker_progress(
                        "routing",
                        format!("model routing blocked: {reason}"),
                    )],
                )?;
                return Ok(false);
            }
        };
        let account = selected_account.as_ref().map(|(_, id)| id.clone());
        let account_adapter = selected_account.as_ref().map(|(a, _)| *a);

        let run_id = ulid::Ulid::new().to_string();
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
        let model = resolved_model
            .or_else(|| self.models.get(&provider_id).cloned())
            .unwrap_or_default();
        let event_started = Instant::now();
        self.store.append_event(
            task.id,
            &Event::WorkerStarted {
                run_id: run_id.clone(),
                adapter: adapter_id.clone(),
                model: model.clone(),
                provider: Some(provider_id.clone()),
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
        // ADR-0069 D5: この run の routing の監査記録（担当・harness・lane・model・features・規則）。
        if let Some(mut decision) = lane_decision {
            if task_core::model_policy::lane_rank(task.worker_hint.tier)
                < task_core::model_policy::lane_rank(decision.lane)
            {
                decision.reasons.push(format!(
                    "quota layer lowered lane {:?} -> {:?} (budget guard)",
                    decision.lane, task.worker_hint.tier
                ));
            }
            let record = task_core::RoutingRecord {
                org_node: task.assignee.clone(),
                harness: task.genre.clone(),
                resolution: task_core::model_routing::LaneResolution {
                    lane: Some(task.worker_hint.tier),
                    adapter: adapter_id.clone(),
                    provider: Some(provider_id.clone()),
                    account: account.clone(),
                    model_id: model.clone(),
                    // ADR-0069 Phase 118 D1: 監査記録は「設定した」値ではなく「実際に CLI へ
                    // 渡った」値を残す（対応しないアダプタでは `None` になる）。
                    reasoning_effort: adapter
                        .reasoning_effort_for_tier(task.worker_hint.tier)
                        .filter(|_| adapter.supports_reasoning_effort()),
                    // ADR-0132 付記 L8: provider 選択の理由と見た候補。
                    selection: Some(provider_selection),
                },
                quota_reason: Some(routing_reason.clone()),
                decision,
                // ADR-0072 D21（Phase E3）: WU の run だけ `work_unit_id` を持つ。
                work_unit_id: current_wu.as_ref().map(|wu| wu.id.clone()),
                optimizer: match &enforce_round {
                    Some(round) => Some(self.enforce_optimizer_trace(
                        &task.worker_hint,
                        &run_id,
                        round,
                        &provider_id,
                        &model,
                        account.as_deref(),
                    )),
                    None => self.legacy_optimizer_trace(&task.worker_hint, &run_id, &provider_id),
                },
            };
            self.store.append_event(
                task.id,
                &Event::RoutingDecided {
                    run_id: run_id.clone(),
                    record: Box::new(record),
                },
            )?;
        }
        if matches!(adapter_id.as_str(), "claude-code" | "codex") {
            self.store.append_event(task.id, &Event::worker_progress(&run_id,
                format!("model routing: {routing_reason}; execution tier={:?}; provider={provider_id}", task.worker_hint.tier)))?;
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
        tracing::info!(task_id = %task.id, %run_id, adapter = %adapter_id, provider = %provider_id, account = account.as_deref(), "dispatching");
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
                provider: Some(provider_id.as_str()),
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
        if let Some(w) = &wu_workspace {
            extras.artifacts_dir_override = Some(w.artifacts_dir.clone());
            // ADR-0074 F5-fix（不具合 1）: WU ごとの `CARGO_TARGET_DIR`。
            if let Some(wu) = &v2_wu {
                extras.cargo_target_work_unit = Some((wu.id.clone(), wu.key.clone()));
            }
        }
        if current_wu.is_some() {
            // ADR-0072 D22（Phase E3）: 計画のある Task の WU の run からは delegate.json を
            // 使えない（部をまたぐ委譲は Task 単位。D21）。
            extras.available_genres = Vec::new();
        }
        // ADR-0079 D7（Phase R3a）: 木の節点の worker の run は `result.json` の `decisions` で決定の要求を出せる。
        // ADR-0124 D4: 直行経路の implementation run にだけ worker への節を渡す。
        if current_wu.is_none() && !is_planner_dispatch {
            extras.direct_route = direct_route.map(direct_route_context);
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
                replan_dispatch,
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
            self.task_workspaces.insert(task.id, ws.clone());
        }
        // ADR-0074 D4（Phase F3 quota）: 観測の `before` を記録する。ADR-0076: planner run も同じく
        // 登録する（`on_planner_finished` が `resolve_quota_estimate` で閉じる）。
        self.quota_begin(account.as_deref(), account_adapter, &run_id);
        // ADR-0074「R7-11 実装時の明確化」: 上で書いた予算（planner / WU / 知識整理のフォールバック）は手元の写しにしか
        // 無い。`run_worker` は DB から task を読み直すので、実効の予算を必ず渡す（`wall` だけでなく `max_turns` も効かせる）。
        extras.budget = Some(task.budget);
        // ADR-0130 D2: 実装 run の開始 HEAD を worker を起こす前に固定する（planner run は書かない）。
        if !is_planner_dispatch {
            self.capture_run_write_bases(&task, &run_id, worktree.as_ref());
        }
        let handle = self.spawn_worker(
            task.id,
            task.worker_hint.tier,
            run_id.clone(),
            provider_id.clone(),
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
        self.reserve_write_set(key.clone(), write_reservation);
        self.running.insert(
            key,
            RunEntry {
                run_id,
                provider: provider_id,
                handle,
                since: OffsetDateTime::now_utc(),
                cluster: cluster.map(|(spec, ..)| spec.id),
                account,
                account_adapter,
                container: container_stop,
                cos,
            },
        );
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

    /// ADR-0107 D2: browser fallback candidates for one run. Only providers that are enabled
    /// (configured in the policy table with a non-zero concurrency, not an account pool), healthy
    /// (not in cooldown) and whose adapter id is conformant in the runner-recorded ledger are
    /// offered, ordered specialist first and one per adapter id. A `CredentialUse` policy never
    /// falls back (credential waits and auth sections must not be replayed). When a browser task
    /// ends up without any candidate the refusal is recorded as a progress event.
    pub(super) fn browser_fallback_candidates(
        &self,
        task_id: TaskId,
        run_id: &str,
        primary_provider: &ProviderId,
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
        let cooling: std::collections::HashSet<ProviderId> = self
            .policy
            .cooldowns(Instant::now())
            .into_iter()
            .map(|c| c.provider)
            .collect();
        let mut excluded: Vec<String> = Vec::new();
        let mut candidates: Vec<(String, ProviderId, Arc<dyn WorkerAdapter>)> = Vec::new();
        let mut providers: Vec<(&ProviderId, &Arc<dyn WorkerAdapter>)> =
            self.adapters.iter().collect();
        providers.sort_by(|a, b| a.0.cmp(b.0));
        for (provider, adapter) in providers {
            let id = adapter.id();
            if provider == primary_provider
                || id == primary_adapter
                || !task_worker::browser::BROWSER_BACKEND_IDS.contains(&id)
            {
                continue;
            }
            let reason = if self.account_pool_providers.contains(provider)
                || self.policy.concurrency_limit(provider.clone()) == 0
            {
                Some("disabled")
            } else if cooling.contains(provider) {
                Some("unhealthy")
            } else if !conformant.as_ref().is_some_and(|ids| ids.contains(id)) {
                Some("not_conformant")
            } else {
                None
            };
            match reason {
                Some(reason) => excluded.push(format!("{provider}={reason}")),
                None => candidates.push((id.to_string(), provider.clone(), Arc::clone(adapter))),
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
                    "browser fallback refused: no enabled, healthy, conformant alternate backend ({ledger}; excluded: [{}])",
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
        extras: RunExtras,
        container: ContainerDecision,
    ) -> JoinHandle<()> {
        let store = self.store.clone();
        // ADR-0107 D2: the dispatcher builds the browser fallback list from provider state and
        // the runner-recorded ledger; the worker wraps each candidate like the primary (ADR-0107).
        let browser_candidates = self.browser_fallback_candidates(
            task_id,
            &run_id,
            &provider,
            adapter.id(),
            task_worker::browser::conformance_record_path().as_deref(),
        );
        let tx = self.tx.clone();
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
                candidates: self.scratch.candidates.clone(),
            }
        } else {
            CargoTargetPlan::Legacy(self.config.build_cache_dir.clone())
        };
        tokio::spawn(async move {
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
        self.config.max_concurrency * 4 + 16 + self.warned_unroutable.len()
    }

    /// ADR-0043 D4: レビュー担当の `Check::Command` の既定になる検査コマンド
    /// （タスクに `acceptance` が明示されていればそれが勝つ。決めるのはここではなく `review.rs` の
    /// 呼び出し側）。**先頭のリポジトリの** `[commands] check` だけを使う。
    /// ADR-0069 D3 / D6（Phase 114）: このタスクの lane を決める（`routing` を持つ execute タスクだけ。
    /// それ以外は `None` で従来どおり `worker_hint.tier`）。担当の実効 profile の天井（`allowed_tiers` /
    /// `budget.max_lane`）で丸め、やり直し（`attempts > 0`）ならイベントの履歴から
    /// `EscalationPolicy` で 1 段まで上げる。LLM は使わない（DESIGN 原則 1）。
    pub(super) fn decide_lane(
        &self,
        task: &Task,
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
        let Some(mut decision) = task_core::model_policy::decide_for_task(task, &ceiling) else {
            return Ok(None);
        };
        if task.attempts > 0 && decision.source.policy_decides() {
            let events: Vec<Event> = self
                .store
                .events_for(task.id)?
                .into_iter()
                .map(|(_, e)| e)
                .collect();
            let history = task_core::retry_policy::attempt_history(task, &events);
            let policy = task_core::EscalationPolicy::for_task(task, profile.as_ref());
            let next = policy.decide(&history, decision.lane, task_core::BudgetState::Ok);
            if next.lane() != decision.lane {
                decision.reasons.push(format!(
                    "retry lane {:?} -> {:?}",
                    decision.lane,
                    next.lane()
                ));
            }
            decision.lane = next.lane();
            decision.escalation = Some(next.describe());
            tracing::info!(task_id = %task.id, attempts = task.attempts, decision = %next.describe(), "retry lane decided (ADR-0069 D6)");
        }
        Ok(Some(decision))
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
        match task_ops::matching::decide(&org, &task) {
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
