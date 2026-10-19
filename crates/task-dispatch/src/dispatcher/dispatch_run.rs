//! run の起動（`dispatch_one`・`spawn_worker`）と、その前段の担当・lane・harness の決定。lease の取得から `running.insert` までは一続きに保つ。ADR-0082 の L4。

use super::*;

impl Dispatcher {
    /// `dispatch_ready` の 1 件分（ADR-0074 D1.3（Phase F2b）で切り出した。並列 WU の 2 本目以降も
    /// ここを通る）。run を起こしたら `Ok(true)`。
    pub(super) fn dispatch_one(
        &mut self,
        task: Task,
        forced_wu: Option<task_core::WorkUnitRow>,
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
            let scope = if cos {
                task_core::CapacityScope::Cos
            } else {
                task_core::CapacityScope::Global
            };
            self.note_dispatch_wait(task.id, task_core::TaskWait::Capacity { scope, id: None });
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
        // ADR 2026-10-07-build-tmp-hygiene 付記 A4: 監視している path が `critical` のあいだは新しい coding run を
        // 起こさない（ready のまま。attempts を消費しない。並列 WU の 2 本目以降も同じ）。
        if self.held_by_disk_critical(&task) {
            let path = self
                .disk_watch
                .as_ref()
                .map(|r| {
                    r.entries
                        .iter()
                        .map(|e| e.path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            self.note_dispatch_wait(task.id, task_core::TaskWait::DiskCritical { path });
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
                self.note_dispatch_wait(
                    task.id,
                    task_core::TaskWait::Backoff {
                        until: rfc3339(task.updated_at + delay),
                        cause: task_core::BackoffCause::Retry,
                    },
                );
                return Ok(false);
            }
        }
        // ADR-0070 D3（Phase 116）: インフラ都合の再試行のバックオフ（`self.infra_backoff`。
        // `task.attempts` に依らない別軸。上のバックオフとは独立にゲートする）。期限を過ぎたら
        // このタスクへのゲートは外す（次に infra 失敗すればまた立て直す）。
        if !second_pass && let Some(until) = self.infra_backoff.get(&task.id).copied() {
            if self.now_utc() < until {
                tracing::debug!(task_id = %task.id, %until, "infra backoff; not dispatching yet");
                self.note_dispatch_wait(
                    task.id,
                    task_core::TaskWait::Backoff {
                        until: rfc3339(until),
                        cause: task_core::BackoffCause::Infra,
                    },
                );
                return Ok(false);
            }
            self.infra_backoff.remove(&task.id);
        }
        // ADR 2026-10-08-browser-prod-enablement D2: browser の前提（適合台帳）が無い・古いなら、lease・worker の
        // 前に理由つきで止める（worker 起動後の infra_requeue を繰り返さない）。
        if !second_pass
            && Self::browser_prereq_applies(&task, is_planner_dispatch)
            && self.browser_prereq_hold(&task)?
        {
            return Ok(false);
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
            self.note_dispatch_wait(
                task.id,
                task_core::TaskWait::WriteSet {
                    blocking_task_ids: vec![blocker.task],
                },
            );
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
                self.note_dispatch_wait(
                    task.id,
                    task_core::TaskWait::Unroutable {
                        detail: "no such cluster in the config".to_string(),
                    },
                );
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
                self.note_dispatch_wait(task.id, cluster_wait(&spec.id, "cooldown"));
                return Ok(false);
            }
            if self.cluster_in_use(&spec.id) >= spec.concurrency {
                self.note_dispatch_wait(task.id, cluster_wait(&spec.id, "concurrency full"));
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
                        self.note_dispatch_wait(
                            task.id,
                            cluster_wait(&spec.id, &format!("auto-connect failed: {detail}")),
                        );
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
                        self.note_dispatch_wait(
                            task.id,
                            cluster_wait(&spec.id, "no ssh ControlMaster connection"),
                        );
                        return Ok(false);
                    }
                }
            }
        }
        let dir_started = Instant::now();
        // ADR-0041 D1 / ADR-0043 D2: ローカルの作業場所（1 つ以上のリポジトリ）を用意する
        // （`dir` はその親 = `runs/` `artifacts/` の置き場）。足回りの root はここで初めて固定する
        // （ADR 2026-10-14-local-machine-settings D5。以後の解決は固定された root を見る）。
        self.pin_task_root(&task)?;
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
        if let Some(wu) = &current_wu {
            // ADR-0074「Phase F5-fix7 実装時の明確化」: 一時的な失敗の後のバックオフ中は試さない。
            if let Some(f) = self.wu_prepare_failures.get(&wu.id)
                && self.now_utc() < f.retry_at
            {
                tracing::debug!(task_id = %task.id, work_unit = %wu.key, failures = f.count, retry_at = %f.retry_at, "work unit worktree backoff; not dispatching yet");
                return Ok(false);
            }
        }
        if let Some(wu) = &v2_wu {
            match self.prepare_work_unit_workspace(&task, wu, task_worktree.as_ref()) {
                Ok(prepared) => {
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
        // Capture before any worker can write; retries load the same WU-id snapshot.
        let mut wu_scope_env = Vec::new();
        if let Some(wu) = &current_wu
            && !matches!(task.workspace, WorkspaceSpec::Remote { .. })
        {
            let prepared = (|| {
                if let Some(ws) = &worktree {
                    for repo in &ws.repos {
                        if let Some(wt) = &repo.worktree {
                            wt.ensure_blocking()
                                .map_err(|e| WuPrepareError::transient(e.to_string()))?;
                        }
                    }
                }
                // A shared repository's workspace symlink is installed by the
                // worker later; its source resolves to the same git directory.
                let cwd = worktree
                    .as_ref()
                    .and_then(|ws| ws.repos.first())
                    .map(|repo| {
                        if repo.worktree.is_some() {
                            repo.dir.as_path()
                        } else {
                            repo.source.as_path()
                        }
                    })
                    .unwrap_or(&dir);
                super::wu_base::scope_env(cwd, &wu.id, true)
                    .map_err(|e| WuPrepareError::transient(e.to_string()))
            })();
            match prepared {
                Ok(env) => {
                    self.wu_prepare_failures.remove(&wu.id);
                    wu_scope_env = env;
                }
                Err(e) => {
                    self.on_work_unit_prepare_failed(&task, wu, &e, second_pass)?;
                    return Ok(false);
                }
            }
        }
        // ADR-0052 D1 / D2（Phase 64）: 知識整理 run は dispatch の直前に `langmem` の接続先へ
        // `GET /models` を当て、届かなければ tier `cheap` の**汎用**ハーネスへ倒す
        // （task harness の固定を外すだけで、通常の route 選択へ戻す）。LLM は呼ばない。
        let fallback_reason = self.knowledge_fallback_reason(&task, now);
        if let Some(reason) = &fallback_reason
            && let Some(tier) = self.config.knowledge.fallback_tier
        {
            task.harness_id = None;
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
        let (mut lane_decision, escalation_audit) = if is_planner_dispatch {
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
                    format!("planner/system-{}", tier_label(tier)),
                    "ADR-0074 D5.3: planner run runs at [execution.planner] tier, fixed by \
                     celeris code"
                        .to_string(),
                )
            };
            (
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
                }),
                None,
            )
        } else if let Some(wu) = &current_wu {
            // WU の lane は task の lane を上限にする（ADR-0074 D5.2）。軌跡 escalation は task の
            // run だけに掛け、WU の retry では上げない（ADR-0072 D21）。
            (self.decide_lane_for_work_unit(&task, wu)?, None)
        } else {
            self.decide_lane(&task)?
                .map_or((None, None), |(decision, audit)| (Some(decision), audit))
        };
        // `routing.mode = decision_assisted`（既定 fixed では何もしない）: 分類段の結果で lane を引き上げ得る。
        let mut lane_assist_record = None;
        if !is_planner_dispatch && let Some(decision) = lane_decision.as_mut() {
            let (record, held) = self.apply_lane_assist(&task, current_wu.as_ref(), decision)?;
            if held {
                return Ok(false);
            }
            lane_assist_record = record;
        }
        if let Some(decision) = &lane_decision {
            task.worker_hint.tier = decision.lane;
        }
        // ADR 2026-10-11 llm-source-harness-selection: harness → 宣言順の source×model →
        // `(source, account)` の admission を `select_route` だけで決め、その route の adapter と
        // worker に渡す `SelectedLlmRoute` を作る。選べなければ起動しない（別の source へ倒さない）。
        let route_input = route_select::RouteInput {
            task_id: task.id.to_string(),
            harness: {
                let mut request = route_select::harness_request_for(&task);
                if is_planner_dispatch {
                    request.execution_class = Some("planner".into());
                }
                request
            },
            model_hint: None,
            lane: Some(tier_label(task.worker_hint.tier).to_string()),
            cos,
        };
        let run_id = ulid::Ulid::new().to_string();
        // Capture the task-features snapshot before selection. The selector, route_decisions row,
        // and RoutingDecided event must all refer to this same content-addressed input.
        let route_features = lane_decision.as_ref().and_then(|decision| {
            match self.store.record_route_features(
                task_core::route_extract::Subject {
                    kind: if current_wu.is_some() { "work_unit" } else { "task" }.to_owned(),
                    id: current_wu
                        .as_ref()
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
        let routing_override = lane_assist::task_routing_override(self.store.as_ref(), task.id);
        let (route_outcome, mut route_trace) = self.select_route_with(
            &route_input,
            &route_select::RouteContext {
                run_id: Some(run_id.clone()),
                features: route_features.clone(),
                work_unit_id: current_wu.as_ref().map(|wu| wu.id.clone()),
                evidence_mode: routing_override
                    .as_ref()
                    .map(|o| o.evidence_mode)
                    .unwrap_or_default(),
                evidence_version: routing_override
                    .as_ref()
                    .and_then(|o| o.evidence_table_id.clone()),
            },
        );
        if let (Some(record), Some(decision)) = (lane_assist_record, route_trace.decision.as_mut())
        {
            decision.lane_assist = Some(record);
        }
        // task 欄で routing を固定した task だけ、解決結果（mode・出自・下段・根拠表・online 更新）を判断 record に残す。
        if routing_override.is_some()
            && let Some(decision) = route_trace.decision.as_mut()
        {
            decision.routing_resolved = Some(task_core::route_select::resolve_routing(
                self.routing_mode,
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
        let (pick, adapter, llm_route) = match launch {
            Ok(launch) => {
                self.warned_unroutable.remove(&task.id);
                launch
            }
            Err(launch_error) => {
                self.note_route_refusal(
                    task.id,
                    &route_input,
                    &route_outcome,
                    &route_trace,
                    launch_error.as_ref(),
                )?;
                return Ok(false);
            }
        };
        // ADR 2026-10-15-mlsys-min D4: 未登録の credential を要する経路は、この unit だけを待機へ移す。
        if self.credential_gate_hold(&task, current_wu.as_ref(), &pick)? {
            return Ok(false);
        }
        let adapter_id = adapter.id().to_string();
        let source_id = pick.candidate.source.to_string();
        let model = pick.candidate.model.to_string();
        let account_adapter = self
            .llm_route_registry
            .sources
            .get(&pick.candidate.source)
            .and_then(|s| route_select::account_adapter_for_source_kind(s.kind.as_ref()));
        // The pool account of an OAuth source (other sources book `(source, account)` on the route only).
        let account = account_adapter.map(|_| pick.account.to_string());

        // variant 付き試行は、lease と run 記録の前にもう一度 pin を確かめる（割当後に org が変わっていたら起動しない）。
        if let Err(reason) = self.variant_start_check(&task) {
            self.block_variant_task(&task, reason)?;
            return Ok(false);
        }
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
            // ADR-0074 F5-fix（不具合 1）: WU ごとの `CARGO_TARGET_DIR`。
            if let Some(wu) = &v2_wu {
                extras.cargo_target_work_unit = Some((wu.id.clone(), wu.key.clone()));
            }
        }
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
        self.reserve_write_set(key.clone(), write_reservation);
        self.running.insert(
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
                cos,
                route: Some((pick.candidate.source, pick.account)),
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
            self.harness_adapters.iter().collect();
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
        self.max_concurrency * 4 + 16 + self.warned_unroutable.len()
    }

    /// ADR-0043 D4: レビュー担当の `Check::Command` の既定になる検査コマンド
    /// （タスクに `acceptance` が明示されていればそれが勝つ。決めるのはここではなく `review.rs` の
    /// 呼び出し側）。**先頭のリポジトリの** `[commands] check` だけを使う。
    /// ADR-0069 D3 / D6（Phase 114）: このタスクの lane を決める（`routing` を持つ execute タスクだけ。
    /// それ以外は `None` で従来どおり `worker_hint.tier`）。担当の実効 profile の天井（`allowed_tiers` /
    /// `budget.max_lane`）で丸め、やり直し（`attempts > 0`）ならイベントの軌跡から
    /// `EscalationPolicy::decide_trajectory` で 1 段まで上げる。上げた/上げなかった決定を
    /// `EscalationAudit` として返す（`RoutingRecord.escalation` に残す）。LLM は使わない（DESIGN 原則 1）。
    pub(super) fn decide_lane(
        &self,
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
                ..self.dispatch_routing.escalation.clone()
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
            self.unroutable.insert(task_id);
        }
        if !self.warned_unroutable.insert(task_id) {
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

/// ADR 2026-10-11-task-wait-reasons M3: クラスタ待ち。
fn cluster_wait(cluster_id: &str, detail: &str) -> task_core::TaskWait {
    task_core::TaskWait::Cluster {
        cluster_id: cluster_id.to_string(),
        detail: detail.to_string(),
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
