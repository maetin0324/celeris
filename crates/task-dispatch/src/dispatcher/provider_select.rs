//! provider / account の選択と cooldown 判定（ADR-0012、ADR-0013、ADR-0024、ADR-0049）。ADR-0082 の L1。

use super::*;
use task_core::model_catalog::assignments::{
    AssignmentState, AssignmentView, WireRule, apply_to_bindings, configured_wire,
    source_name_for_llm_source,
};
use task_core::model_router::{
    optimizer::{Candidate, OptimizationResult, legacy_rank},
    policy::RoutingMode,
    profiles::{Billing, Capabilities, ContextLimits, DeploymentProfile, ModelProfile, Support},
    trace::{CandidateTrace, RoutingTraceV1},
};
use task_core::model_routing::{
    ProviderCandidate, ProviderCandidateKind, ProviderCandidateOutcome, ProviderSelection,
    ProviderSelectionReason, TierModels,
};

/// ADR 2026-10-06 model-role-assignments D2: 割り当てで除外された provider を候補の記録に残すときの語彙
/// （`ProviderCandidate.detail` の先頭。outcome は `Unsupported`）。
pub(super) const ASSIGNMENT_EXCLUDED: &str = "assignment_excluded";

/// 選んだ (アダプタ, 設定行, プールのアカウント)。プールを使わない行ならアカウントは `None`。
pub(super) type ProviderPick = (AdapterId, ProviderId, Option<(AccountAdapter, String)>);

/// `ProviderThrottled.reason` に書く供給側失敗の種別（ADR-0013 D9）。供給側失敗でなければ `None`。
pub(super) fn provider_failure_reason(e: &AdapterError) -> Option<&'static str> {
    match e {
        AdapterError::Throttled { .. } => Some("throttled"),
        AdapterError::AuthFailed(_) => Some("auth_failed"),
        AdapterError::Exhausted(_) => Some("exhausted"),
        AdapterError::Spawn(_) => Some("spawn"),
        AdapterError::Io(_) | AdapterError::Serde(_) | AdapterError::Other(_) => None,
    }
}

/// Reviewer run の供給側失敗（`ProviderOutcome` しか残っていない）の種別名（ADR-0013 D9）。
pub(super) fn cooldown_reason_name(outcome: &ProviderOutcome) -> &'static str {
    match outcome {
        ProviderOutcome::Throttled { .. } => "throttled",
        ProviderOutcome::AuthFailed => "auth_failed",
        ProviderOutcome::Exhausted => "exhausted",
        ProviderOutcome::Ok => "ok",
    }
}

/// ADR-0024 D3: `AccountView.excluded_reason` の語彙（`docs/api/v1/gui-api.md` §3.29）。
pub(super) fn excluded_reason_name(reason: ExcludedReason) -> &'static str {
    match reason {
        ExcludedReason::NotLoggedIn => "not_logged_in",
        ExcludedReason::AtCapacity => "at_capacity",
        ExcludedReason::Cooldown => "cooldown",
        ExcludedReason::FiveHourExhausted => "five_hour_exhausted",
        ExcludedReason::SevenDayExhausted => "seven_day_exhausted",
        ExcludedReason::OneMonthExhausted => "one_month_exhausted",
        ExcludedReason::Rejected => "rejected",
    }
}

/// ADR-0024 D4/D5: `AccountCooldownView.reason` の語彙。
pub(super) fn account_cooldown_reason_name(reason: AccountCooldownReason) -> &'static str {
    match reason {
        AccountCooldownReason::AuthFailed => "auth_failed",
        AccountCooldownReason::Throttled => "throttled",
        AccountCooldownReason::Exhausted => "exhausted",
    }
}

/// ADR-0024 D4: 供給側失敗の種別名（`provider_failure_reason` と同じ語彙）を `AccountCooldownReason` に写す。
pub(super) fn account_cooldown_reason_from_failure(reason: &str) -> AccountCooldownReason {
    match reason {
        "auth_failed" => AccountCooldownReason::AuthFailed,
        "throttled" => AccountCooldownReason::Throttled,
        // "exhausted" | "spawn"
        _ => AccountCooldownReason::Exhausted,
    }
}

/// 供給側失敗（ADR-0010 D5）なら `ProviderPolicy::report` に渡す結果を返す。起動失敗（`Spawn`）も供給側として扱う。
/// `AdapterError`/`ProviderOutcome` は `task-dispatch`/`task-worker` の型なので、`task-ops` には移さない。
pub fn provider_failure_outcome(e: &AdapterError) -> Option<ProviderOutcome> {
    match e {
        AdapterError::Throttled { retry_after } => Some(ProviderOutcome::Throttled {
            retry_after: *retry_after,
        }),
        AdapterError::AuthFailed(_) => Some(ProviderOutcome::AuthFailed),
        AdapterError::Exhausted(_) | AdapterError::Spawn(_) => Some(ProviderOutcome::Exhausted),
        AdapterError::Io(_) | AdapterError::Serde(_) | AdapterError::Other(_) => None,
    }
}

/// 設定行を正規化した 1 候補（`proxy_routed` は `llm_source` を持つ行）。
pub(super) struct LegacyProfile {
    /// 容量・account・cooldown の identity（設定行の id）。候補の identity は `deployment.id`。
    pub provider_id: ProviderId,
    pub model: ModelProfile,
    pub deployment: DeploymentProfile,
    pub proxy_routed: bool,
}

impl Dispatcher {
    /// ADR 2026-10-06 model-role-assignments D2: 割り当ての実効 view を読む（決定 1 回につき 1 回）。
    /// reader が無ければ空。読み取りに失敗しても dispatch は止めず、空 view（= config のまま）で続ける。
    pub(super) fn current_assignment_view(&self) -> AssignmentView {
        let Some(reader) = &self.role_assignments else {
            return AssignmentView::empty();
        };
        match reader.assignment_view() {
            Ok(view) => view,
            Err(error) => {
                tracing::warn!(%error, "model_role_assignments: cannot read assignments; routing by config only");
                AssignmentView::empty()
            }
        }
    }

    /// provider の catalog source 名と、その provider の実効 lane bindings（config の `tier_models` に割り当てを
    /// 重ねたもの）。snapshot の `ProviderLive` が `llm_source` を持たない（catalog の無い source・
    /// `llm_source` なし）provider は `None` = 割り当ての対象外。
    pub(super) fn effective_tier_models(
        &self,
        provider_id: &str,
        view: &AssignmentView,
    ) -> Option<TierModels> {
        let live = self
            .publisher
            .as_ref()
            .and_then(|p| p.providers.iter().find(|p| p.id == provider_id))?;
        let source = live
            .llm_source
            .as_ref()
            .and_then(|s| source_name_for_llm_source(&s.source))?;
        // 付記 2026-10-07 wire-prefix: 行の config の `<prefix>/` を引き継ぐ（無ければ source × adapter の表）。
        let rule = WireRule::new(&source, &live.adapter, live.model.as_deref());
        // ADR 2026-10-06 D3: model も `tier_models` も持たない **acp 行**（opencode go）は割り当てだけで
        // routing する。割り当てが 1 つも当たらなくても、全 lane を `assignment:none` から始める。
        // claude-code / codex の行は model が無くても CLI の既定モデルで走れるので対象にしない。
        // reader が無い（試験・celerisctl）なら従来どおり（空の bindings = 行の既定で走る）。
        let mut base = live.tier_models.clone();
        if self.role_assignments.is_some()
            && live.adapter == "acp"
            && live.model.is_none()
            && base.is_empty()
        {
            for &lane in &live.tiers {
                base.insert(
                    lane,
                    task_core::model_routing::ModelBinding {
                        name: task_core::model_catalog::assignments::tier_str(lane).to_string(),
                        model_id: None,
                        unavailable_reason: Some("assignment:none".to_string()),
                        reasoning_effort: None,
                    },
                );
            }
        }
        Some(apply_to_bindings(&base, &source, &live.tiers, rule, view))
    }

    /// provider の lane に実際に渡る model（run 起動と同じ実効 bindings。割り当て > config）。
    /// provider が無い・adapter が無い・lane が `Excluded` / 未設定で解決できなければ `Err`、
    /// config に束縛が無ければ `Ok(None)`（従来どおり行の `model` で走る）。
    pub fn effective_lane_model(
        &self,
        provider_id: &str,
        tier: Tier,
    ) -> Result<Option<String>, String> {
        let adapter = self
            .adapters
            .get(provider_id)
            .cloned()
            .ok_or_else(|| format!("no adapter instance for provider {provider_id}"))?;
        self.adapter_with_effective_models(provider_id, adapter, &self.current_assignment_view())
            .model_for_tier(tier)
    }

    /// `hint` に合う設定行のうち、割り当てのせいで `hint.tier` に routing できない provider とその理由
    /// （`Excluded` の割り当て、または割り当てのある source で binding も割り当ても無い lane）。
    /// config 由来の「未設定」「unavailable」は従来どおり `resolve` が扱うので、ここでは外さない。
    pub(super) fn assignment_excluded_providers(
        &self,
        hint: &task_core::WorkerHint,
        view: &AssignmentView,
    ) -> Vec<(ProviderId, String)> {
        if self.role_assignments.is_none() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for spec in self.policy.legacy_specs(hint) {
            let Some(effective) = self.effective_tier_models(&spec.id, view) else {
                continue;
            };
            if task_core::model_routing::resolve(&effective, hint.tier).is_ok() {
                continue;
            }
            if let Some(reason) = effective
                .get(&hint.tier)
                .and_then(|b| b.unavailable_reason.as_ref())
                .filter(|r| r.starts_with("assignment:"))
            {
                out.push((spec.id.clone(), reason.clone()));
            }
        }
        out
    }

    /// run 用のアダプタに割り当て済みの実効 bindings を載せる。割り当てが当たらない（実効 = config）、
    /// provider が割り当て対象でない、アダプタが `with_tier_models` を持たないときは元のアダプタのまま。
    pub(super) fn adapter_with_effective_models(
        &self,
        provider_id: &str,
        adapter: Arc<dyn WorkerAdapter>,
        view: &AssignmentView,
    ) -> Arc<dyn WorkerAdapter> {
        if self.role_assignments.is_none() {
            return adapter;
        }
        let Some(effective) = self.effective_tier_models(provider_id, view) else {
            return adapter;
        };
        let own = self
            .publisher
            .as_ref()
            .and_then(|p| p.providers.iter().find(|p| p.id == provider_id))
            .map(|p| &p.tier_models);
        if own == Some(&effective) {
            return adapter;
        }
        adapter.with_tier_models(effective).unwrap_or(adapter)
    }

    /// Normalize the legacy provider rows into model/deployment identities. The kernel
    /// preserves their config order; the existing selector still owns live capacity,
    /// cooldown and account decisions.
    fn legacy_provider_rank(&self, hint: &task_core::WorkerHint) -> OptimizationResult {
        let profiles = self.legacy_provider_profiles(hint);
        let candidates: Vec<_> = profiles
            .iter()
            .map(|p| Candidate {
                model: &p.model,
                deployment: &p.deployment,
                state: None,
                eligible_provider_ids: vec![p.provider_id.clone()],
                cost_usd: None,
                latency_ms: None,
                pressure: None,
            })
            .collect();
        legacy_rank(&candidates)
    }

    /// 設定行（hint に合うもの・設定順）を model/deployment の profile に写す。
    pub(super) fn legacy_provider_profiles(
        &self,
        hint: &task_core::WorkerHint,
    ) -> Vec<LegacyProfile> {
        let specs = self.policy.legacy_specs(hint);
        let view = self.current_assignment_view();
        let mut profiles = Vec::with_capacity(specs.len());
        for (order, spec) in specs.iter().enumerate() {
            let live = self
                .publisher
                .as_ref()
                .and_then(|p| p.providers.iter().find(|p| p.id == spec.id));
            // ADR 2026-10-06 model-role-assignments D2: 割り当てを重ねた実効 bindings（reader が無い・
            // 割り当てが無ければ config の `tier_models` と同じ）。
            let effective = self.effective_tier_models(&spec.id, &view);
            let assigned = live
                .and_then(|p| p.llm_source.as_ref())
                .and_then(|s| source_name_for_llm_source(&s.source))
                .and_then(|source| view.get(&source, hint.tier).map(|a| a.state))
                == Some(AssignmentState::Assigned);
            let model_id = effective
                .as_ref()
                .or(live.map(|p| &p.tier_models))
                .and_then(|m| m.get(&hint.tier))
                .and_then(|b| b.model_id.as_ref())
                .cloned()
                .or_else(|| {
                    self.adapters
                        .get(&spec.id)
                        .and_then(|a| a.model_for_tier(hint.tier).ok().flatten())
                })
                .or_else(|| live.and_then(|p| p.model.clone()))
                .unwrap_or_else(|| spec.model.clone());
            let model = ModelProfile {
                id: model_id.clone(),
                revision: String::new(),
                // ADR 2026-10-07: routing catalog の family（無ければ空 = 導出は source / pool へ）。
                family: self.catalog_family(&model_id),
                capabilities: Capabilities {
                    tools: Support::Unknown,
                    structured_output: Support::Unknown,
                    vision: Support::Unknown,
                    streaming: Support::Unknown,
                    reasoning_efforts: vec![],
                },
                context_limits: ContextLimits {
                    input: None,
                    output: None,
                    total: None,
                },
                quality: vec![],
                pricing: None,
                provenance: if assigned {
                    "model_role_assignments".into()
                } else {
                    "providers.tier_models".into()
                },
            };
            let proxy_routed = live.and_then(|p| p.llm_source.as_ref()).is_some();
            let source_ref = live
                .and_then(|p| p.llm_source.as_ref())
                .map(|s| match &s.source {
                    task_core::LlmSourceRef::OpenaiCompatible(id) => {
                        format!("openai_compatible:{id}")
                    }
                    other => other.as_str().to_string(),
                })
                .unwrap_or_else(|| spec.id.clone());
            let is_local = self.local_providers.iter().any(|p| p.provider == spec.id);
            let deployment = DeploymentProfile {
                id: spec.id.clone(),
                source_ref,
                model_profile_id: model_id,
                upstream_model: spec.model.clone(),
                adapter_constraints: vec![spec.adapter.clone()],
                billing: if is_local {
                    Billing::SelfHosted
                } else {
                    Billing::Subscription
                },
                host: None,
                region: None,
                trust_zone: None,
                external_network: !is_local,
                retains_data: None,
                allowed_lanes: spec.tiers.clone(),
                resource_group_id: None,
                concurrency_limit: Some(spec.concurrency as u32),
                rpm_limit: None,
                tpm_limit: None,
                price_override: None,
                config_order: order,
            };
            let source = live
                .and_then(|p| p.llm_source.as_ref())
                .and_then(|s| source_name_for_llm_source(&s.source));
            if let Some(source) = source.as_deref().filter(|s| view.manages(s, hint.tier)) {
                for member in view
                    .members(source, hint.tier)
                    .into_iter()
                    .filter(|a| a.state == AssignmentState::Assigned)
                {
                    // 付記 2026-10-07 wire-prefix: `effective_tier_models` と同じ規則（config の `<prefix>/` → 表）。
                    let wire = live.map_or_else(
                        || member.model_id.clone(),
                        |p| {
                            WireRule::new(source, &p.adapter, p.model.as_deref()).model(
                                configured_wire(p.tier_models.get(&hint.tier)),
                                &member.model_id,
                            )
                        },
                    );
                    let mut model = model.clone();
                    model.id = wire.clone();
                    model.provenance = "model_role_assignments".into();
                    let mut deployment = deployment.clone();
                    // 候補の identity はモデルごとに分け、容量の identity（provider）は共有する。
                    deployment.id = format!("{}/model:{}", spec.id, member.model_id);
                    deployment.resource_group_id = Some(spec.id.clone());
                    deployment.model_profile_id = wire.clone();
                    deployment.upstream_model = wire;
                    deployment.config_order = member.priority as usize;
                    deployment.allowed_lanes = vec![hint.tier];
                    profiles.push(LegacyProfile {
                        provider_id: spec.id.clone(),
                        model,
                        deployment,
                        proxy_routed,
                    });
                }
            } else {
                profiles.push(LegacyProfile {
                    provider_id: spec.id.clone(),
                    model,
                    deployment,
                    proxy_routed,
                });
            }
        }
        profiles.sort_by(|a, b| {
            (
                a.deployment.config_order,
                &a.deployment.source_ref,
                &a.model.id,
            )
                .cmp(&(
                    b.deployment.config_order,
                    &b.deployment.source_ref,
                    &b.model.id,
                ))
        });
        profiles
    }

    pub(super) fn legacy_optimizer_trace(
        &self,
        hint: &task_core::WorkerHint,
        run_id: &str,
        selected: &str,
    ) -> Option<RoutingTraceV1> {
        let result = self.legacy_provider_rank(hint);
        if result.allowlist.is_empty() {
            return None;
        }
        let assigned_any = self
            .legacy_provider_profiles(hint)
            .iter()
            .any(|p| p.model.provenance == "model_role_assignments");
        Some(RoutingTraceV1 {
            decision_id: run_id.into(),
            parent_decision_id: None,
            task_id: None,
            work_unit_id: None,
            run_id: Some(run_id.into()),
            request_id: None,
            stage: "dispatcher".into(),
            mode: RoutingMode::Legacy,
            policy_version: "legacy-provider-config-v1".into(),
            catalog_version: if assigned_any {
                "model_role_assignments+providers.tier_models".into()
            } else {
                "providers.tier_models".into()
            },
            feature_version: "legacy".into(),
            estimator_version: "none".into(),
            snapshot_id: "provider-config".into(),
            observed_at: None,
            requested_lane: hint.tier,
            selected_lane: Some(hint.tier),
            candidates: result
                .ranked
                .iter()
                .map(|c| CandidateTrace {
                    model_profile_id: c.model_profile_id.clone(),
                    deployment_id: c.deployment_id.clone(),
                    eligible_provider_ids: c.eligible_provider_ids.clone(),
                    excluded_reasons: vec![],
                    quality: None,
                    cost_usd: None,
                    latency_ms: None,
                    pressure: None,
                    score: None,
                    ..Default::default()
                })
                .collect(),
            selected: Some(selected.into()),
            fallback_order: result.allowlist,
            reasons: vec!["providers.tier_models/config_order".into()],
            source_id: None,
            model: None,
            account_id: None,
        })
    }

    /// ADR-0013 D9: cooldown に入った供給側失敗の `ProviderThrottled`。期限はポリシーの `cooldowns()` から取り、
    /// ポリシーが公開しない場合は `Throttled.retry_after` から計算する（どちらも無ければ記録しない）。
    pub(super) fn provider_throttled_event(
        &self,
        provider: &str,
        outcome: &ProviderOutcome,
        reason: &str,
    ) -> Option<Event> {
        let now = Instant::now();
        let until = self
            .policy
            .cooldowns(now)
            .into_iter()
            .find(|c| c.provider == provider)
            .map(|c| c.until)
            .or(match outcome {
                ProviderOutcome::Throttled { retry_after } => Some(now + *retry_after),
                _ => None,
            })?;
        Some(Event::ProviderThrottled {
            provider: provider.to_string(),
            until: OffsetDateTime::now_utc() + until.saturating_duration_since(now),
            reason: Some(reason.to_string()),
        })
    }

    /// ADR-0012 D2（P-20 / P-33）: 並列度の上限に達したプロバイダを除外しながら選ぶ（設定表の次の行へフォールバック）。
    /// 条件に合うプロバイダが設定に無ければ、タスクごとに 1 回 warn し `unroutable` に入れる。
    /// ADR-0024 D2: 選んだプロバイダが `account_pool = true` なら、続けて D3 でアカウントを選ぶ。選べるアカウントが
    /// 無ければそのプロバイダを満杯として扱い（除外集合に入れて）次の候補へ進む。戻り値の第 3 要素が選んだアカウント
    /// （プールを使わないプロバイダなら `None`）。
    ///
    /// ADR-0054 Phase 67c: `sticky_session` にこのノード・kind の現役セッションが渡されたら、まず
    /// `crate::sessions::decide_sticky` でそのセッションの `(adapter, account_id)` に留まれるかを試す
    /// （`sticky_provider`）。留まれれば ADR-0049 のランキングを走らせない（毎 run アカウントを
    /// 付け替えてセッションを退役させ続ける事故の修正）。留まれなければ、これまでどおり下のランキングへ。
    pub(super) fn select_provider(
        &mut self,
        hint: &task_core::WorkerHint,
        now: Instant,
        task_id: TaskId,
        full: &mut std::collections::HashSet<ProviderId>,
        sticky_session: Option<&NodeSession>,
    ) -> Option<ProviderPick> {
        self.select_provider_for(hint, now, task_id, full, sticky_session, false, false)
            .0
    }

    /// `select_provider` に ADR-0089（Phase R6-5）の `cos` を足したもの。`cos = true`（CoS の対話 run）
    /// なら、プールのプロバイダの `concurrency` を見ず（`crate::capacity::provider_full`）、アカウントは
    /// 上限 +1 で走っている run の最も少ないものを選ぶ。この tick の満杯集合 `full` は非 CoS の判定
    /// なので、CoS は共有せず自分だけの集合で選ぶ（CoS の選択も `full` を汚さない）。
    ///
    /// ADR-0132 付記 L2/L3: `prefer_local = true`（worker run の `dispatch_run` だけ）で lane が cheap、
    /// CoS でなく、ローカルの行（`set_local_providers`）があれば、順位付けの前にローカルの行を設定順に
    /// 見る（hint に合い・cooldown でなく・空きがあり・health が落ちていなければ選ぶ）。見送ったローカルの
    /// 行は順位付けの fallback からも外す。付記 L8: 戻り値の第 2 要素が選択の理由と見た候補の記録。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn select_provider_for(
        &mut self,
        hint: &task_core::WorkerHint,
        now: Instant,
        task_id: TaskId,
        full: &mut std::collections::HashSet<ProviderId>,
        sticky_session: Option<&NodeSession>,
        cos: bool,
        prefer_local: bool,
    ) -> (Option<ProviderPick>, ProviderSelection) {
        self.select_provider_excluding(
            hint,
            now,
            task_id,
            full,
            sticky_session,
            cos,
            prefer_local,
            &std::collections::HashSet::new(),
        )
    }

    /// `select_provider_for` に enforce の除外集合（制約・quota で外した provider）を足したもの。
    /// `excluded` は満杯の集合 `full` に混ぜない（他の task の判定を汚さない）。空なら従来と同じ。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn select_provider_excluding(
        &mut self,
        hint: &task_core::WorkerHint,
        now: Instant,
        task_id: TaskId,
        full: &mut std::collections::HashSet<ProviderId>,
        sticky_session: Option<&NodeSession>,
        cos: bool,
        prefer_local: bool,
        excluded: &std::collections::HashSet<ProviderId>,
    ) -> (Option<ProviderPick>, ProviderSelection) {
        let view = self.current_assignment_view();
        let ranked_role = view.items.iter().any(|a| a.tier == hint.tier)
            || view.managed.iter().any(|(_, tier)| *tier == hint.tier);
        let profiles = self.legacy_provider_profiles(hint);
        let priority = |id: &str| {
            profiles
                .iter()
                .position(|p| p.provider_id == id)
                .unwrap_or(usize::MAX)
        };
        let prefer_local = prefer_local && !ranked_role;
        let allowlist = self.legacy_provider_rank(hint).allowlist;
        // ADR 2026-10-06 model-role-assignments D2: 割り当てで lane に出せない provider は候補から外す。
        let assignment_excluded =
            self.assignment_excluded_providers(hint, &self.current_assignment_view());
        let mut cos_full = std::collections::HashSet::new();
        let full = if cos { &mut cos_full } else { full };
        if let Some(sticky) = self.sticky_provider(sticky_session, hint, now, &*full, cos)
            && !ranked_role
            && !excluded.contains(&sticky.1)
            && !assignment_excluded.iter().any(|(id, _)| *id == sticky.1)
        {
            return (
                Some(sticky),
                ProviderSelection {
                    reason: ProviderSelectionReason::Sticky,
                    candidates: Vec::new(),
                },
            );
        }
        // 候補を列挙するための除外はこの選択だけ。満杯の集合へ候補自体を混ぜない。
        let mut visited = full.clone();
        visited.extend(excluded.iter().cloned());
        let mut candidates: Vec<ProviderCandidate> = Vec::new();
        for (id, reason) in &assignment_excluded {
            visited.insert(id.clone());
            let kind = if self.account_pool_providers.contains(id) {
                ProviderCandidateKind::Pool
            } else if self.local_providers.iter().any(|l| l.provider == *id) {
                ProviderCandidateKind::Local
            } else {
                ProviderCandidateKind::Other
            };
            candidates.push(ProviderCandidate {
                provider: id.clone(),
                kind,
                outcome: ProviderCandidateOutcome::Unsupported,
                detail: Some(format!("{ASSIGNMENT_EXCLUDED}: {reason}")),
            });
        }
        let mut local_full = false;
        let mut local_down = false;
        if prefer_local && !cos && hint.tier == Tier::Cheap && !self.local_providers.is_empty() {
            let cooling: std::collections::HashSet<ProviderId> = self
                .policy
                .cooldowns(now)
                .into_iter()
                .map(|c| c.provider)
                .collect();
            let locals = self.local_providers.clone();
            for spec in &locals {
                let id = &spec.provider;
                let candidate = |outcome, detail| ProviderCandidate {
                    provider: id.clone(),
                    kind: ProviderCandidateKind::Local,
                    outcome,
                    detail,
                };
                let adapter = self.policy.adapter_of(id);
                if assignment_excluded.iter().any(|(x, _)| x == id) {
                    // 上で除外の記録を残してある。
                    continue;
                }
                if excluded.contains(id)
                    || !self.policy.offers(id, hint)
                    || self.account_pool_providers.contains(id)
                    || adapter.is_none()
                {
                    candidates.push(candidate(ProviderCandidateOutcome::Unsupported, None));
                    continue;
                }
                if cooling.contains(id) {
                    local_down = true;
                    visited.insert(id.clone());
                    candidates.push(candidate(ProviderCandidateOutcome::Cooldown, None));
                    continue;
                }
                if full.contains(id) || self.provider_full(id, cos) {
                    local_full = true;
                    full.insert(id.clone());
                    visited.insert(id.clone());
                    candidates.push(candidate(ProviderCandidateOutcome::Full, None));
                    continue;
                }
                if let Err(reason) = self.local_provider_health(&spec.health, now) {
                    local_down = true;
                    visited.insert(id.clone());
                    candidates.push(candidate(ProviderCandidateOutcome::Down, Some(reason)));
                    continue;
                }
                let Some(adapter) = adapter else {
                    continue;
                };
                candidates.push(candidate(ProviderCandidateOutcome::Selected, None));
                self.warned_unroutable.remove(&task_id);
                return (
                    Some((adapter, id.clone(), None)),
                    ProviderSelection {
                        reason: ProviderSelectionReason::LocalPreferred,
                        candidates,
                    },
                );
            }
        }
        let mut best_pool = None;
        let mut best_pool_index = None;
        let mut best_score = f64::NEG_INFINITY;
        let mut fallback = None;
        let mut fallback_index = None;
        for _ in 0..64 {
            match self.policy.select(hint, now, &visited) {
                Selection::Picked { adapter, provider } => {
                    if !allowlist.is_empty() && !allowlist.contains(&provider) {
                        visited.insert(provider);
                        continue;
                    }
                    if !visited.insert(provider.clone()) {
                        break;
                    }
                    let is_pool = self.account_pool_providers.contains(&provider);
                    let kind = if is_pool {
                        ProviderCandidateKind::Pool
                    } else if self.local_providers.iter().any(|l| l.provider == provider) {
                        ProviderCandidateKind::Local
                    } else {
                        ProviderCandidateKind::Other
                    };
                    let mut record = |outcome| {
                        candidates.push(ProviderCandidate {
                            provider: provider.clone(),
                            kind,
                            outcome,
                            detail: None,
                        });
                        candidates.len() - 1
                    };
                    if self.provider_full(&provider, cos) {
                        record(ProviderCandidateOutcome::Full);
                        full.insert(provider);
                        continue;
                    }
                    if is_pool {
                        let Some(account_adapter) = self.pool_adapter_of(&provider, &adapter)
                        else {
                            record(ProviderCandidateOutcome::NoAccount);
                            full.insert(provider);
                            continue;
                        };
                        let requested_account = self
                            .adapters
                            .get(&provider)
                            .and_then(|a| a.account_id())
                            .map(str::to_owned);
                        let Some(account_id) =
                            self.pick_account(account_adapter, requested_account.as_deref(), cos)
                        else {
                            record(ProviderCandidateOutcome::NoAccount);
                            full.insert(provider);
                            continue;
                        };
                        let index = record(ProviderCandidateOutcome::Available);
                        let score = if ranked_role {
                            -(priority(&provider) as f64)
                        } else {
                            self.account_score(account_adapter, &account_id)
                        };
                        if score > best_score {
                            best_score = score;
                            best_pool_index = Some(index);
                            best_pool =
                                Some((adapter, provider, Some((account_adapter, account_id))));
                        }
                    } else {
                        let index = record(ProviderCandidateOutcome::Available);
                        if fallback.as_ref().is_none_or(|(_, id, _): &ProviderPick| {
                            ranked_role && priority(&provider) < priority(id)
                        }) {
                            fallback_index = Some(index);
                            fallback = Some((adapter, provider, None));
                        }
                    }
                }
                Selection::Busy => break,
                Selection::NoMatchingProvider => {
                    if best_pool.is_none() && fallback.is_none() {
                        self.unroutable.insert(task_id);
                        if self.warned_unroutable.insert(task_id) {
                            tracing::warn!(%task_id, ?hint, "no provider in the config matches this worker_hint");
                        }
                    }
                    break;
                }
            }
        }
        // 割り当てが除外した行しか lane に合う行が無ければ、設定に合う行が無いときと同じく経路なし。
        if best_pool.is_none()
            && fallback.is_none()
            && !assignment_excluded.is_empty()
            && self
                .policy
                .legacy_specs(hint)
                .iter()
                .all(|s| assignment_excluded.iter().any(|(id, _)| *id == s.id))
        {
            self.unroutable.insert(task_id);
            if self.warned_unroutable.insert(task_id) {
                tracing::warn!(%task_id, ?hint, "every provider for this worker_hint is excluded by model_role_assignments");
            }
        }
        if ranked_role
            && let (Some(pool), Some(other)) = (&best_pool, &fallback)
            && priority(&other.1) < priority(&pool.1)
        {
            best_pool = None;
        }
        let (selected, selected_index, pool_selected) = match best_pool {
            Some(pool) => (Some(pool), best_pool_index, true),
            None => (fallback, fallback_index, false),
        };
        if let Some(c) = selected_index.and_then(|i| candidates.get_mut(i)) {
            c.outcome = ProviderCandidateOutcome::Selected;
        }
        if selected.is_some() {
            self.warned_unroutable.remove(&task_id);
        }
        // ADR-0132 付記 L3: 前段で見送ったローカルの行が理由の先に立つ（満杯 > 不通・cooldown）。
        let reason = if local_full {
            ProviderSelectionReason::LocalFull
        } else if local_down {
            ProviderSelectionReason::LocalDown
        } else if pool_selected {
            ProviderSelectionReason::Pool
        } else {
            ProviderSelectionReason::Fallback
        };
        (selected, ProviderSelection { reason, candidates })
    }

    /// ADR-0054 Phase 67c: `sticky_session` があれば、`crate::sessions::decide_sticky` にかけて
    /// 「留まれるか」を判断する。使う材料（アカウントの状態・設定表に今もその tier を提供する行が
    /// あるか）はここで集める（I/O）。留まれるなら `select_provider` と同じ形の戻り値、留まれなければ
    /// `None`（呼び出し側が通常のランキングへフォールバックする）。
    #[allow(clippy::type_complexity)]
    pub(super) fn sticky_provider(
        &mut self,
        sticky_session: Option<&NodeSession>,
        hint: &task_core::WorkerHint,
        now: Instant,
        full: &std::collections::HashSet<ProviderId>,
        cos: bool,
    ) -> Option<(AdapterId, ProviderId, Option<(AccountAdapter, String)>)> {
        let active = sticky_session?;
        let account_usable = match &active.account_id {
            None => true,
            Some(account_id) => AccountAdapter::parse(&active.adapter)
                .is_some_and(|adapter| self.account_usable(adapter, account_id, cos)),
        };
        let provider =
            self.matching_provider_for_adapter(&active.adapter, hint.tier, now, full, cos);
        // 設定行が見つかっても、プールの有無がセッション作成時と食い違っていたら（config を書き換えた
        // 等）留まらない。`decide` の `AccountChanged`（プールを使う ⇔ 使わないの切り替えも該当）と
        // 矛盾しないように。
        let provider_offers_tier = provider.as_ref().is_some_and(|p| {
            self.account_pool_providers.contains(p) == active.account_id.is_some()
        });
        let decision = crate::sessions::decide_sticky(
            Some(active),
            self.config.session_rollover_tokens,
            account_usable,
            provider_offers_tier,
        );
        if decision != crate::sessions::StickyDecision::Stick {
            return None;
        }
        let provider_id = provider?;
        let selected_account = active
            .account_id
            .clone()
            .and_then(|id| AccountAdapter::parse(&active.adapter).map(|a| (a, id)));
        Some((active.adapter.clone(), provider_id, selected_account))
    }

    /// ADR-0054 Phase 67c: `adapter_id` の設定行のうち、`requested_tier` と同じかそれ以上の tier を
    /// 提供し（`crate::sessions::tier_rank`。要求そのものから順に試す）、cooldown 中でも並列度上限でも
    /// ないものを 1 つ返す。`hint.adapter` をこの 1 アダプタに固定して `ProviderPolicy::select` に
    /// 任せるので、cooldown・除外集合の扱いは通常のランキングと同じ規則になる。
    pub(super) fn matching_provider_for_adapter(
        &self,
        adapter_id: &str,
        requested_tier: Tier,
        now: Instant,
        excluded: &std::collections::HashSet<ProviderId>,
        cos: bool,
    ) -> Option<ProviderId> {
        let mut tiers: Vec<Tier> = [Tier::Cheap, Tier::Standard, Tier::Frontier]
            .into_iter()
            .filter(|t| {
                crate::sessions::tier_rank(*t) >= crate::sessions::tier_rank(requested_tier)
            })
            .collect();
        tiers.sort_by_key(|t| crate::sessions::tier_rank(*t));
        for tier in tiers {
            let pinned = task_core::WorkerHint {
                tier,
                adapter: Some(adapter_id.to_string()),
            };
            if let Selection::Picked { provider, .. } = self.policy.select(&pinned, now, excluded)
                && !self.provider_full(&provider, cos)
            {
                return Some(provider);
            }
        }
        None
    }

    /// ADR-0054 Phase 67c: 指定した 1 アカウントが今すぐ使えるか（ログイン済み・cooldown 外・上限未満・
    /// 枯渇していない。`crate::accounts::evaluate` の除外判定をそのまま使う）。`pick_account` と同じ
    /// 読み取りだが、ベストスコアを探すのではなく特定の 1 件が使えるかだけを見る。
    pub(super) fn account_usable(
        &mut self,
        adapter: AccountAdapter,
        account_id: &str,
        cos: bool,
    ) -> bool {
        let Some(cfg) = self.config.accounts.clone() else {
            return false;
        };
        let Some(root) = cfg.root_for(adapter) else {
            return false;
        };
        let dirs = self
            .accounts_scan_cache
            .entry(adapter)
            .or_insert_with(|| scan_accounts(root, adapter))
            .clone();
        let Some(dir) = dirs.iter().find(|d| d.id == account_id) else {
            return false;
        };
        let Some(book) = self.account_book(adapter) else {
            return false;
        };
        let now = (self.now_unix_fn)();
        let book = book.lock().unwrap_or_else(|e| e.into_inner());
        let candidate = AccountCandidate {
            id: account_id,
            logged_in: dir.logged_in,
            in_use: self.account_in_use(adapter, account_id),
        };
        evaluate(
            &candidate,
            book.state(account_id),
            crate::capacity::account_run_limit(cfg.max_runs_per_account, cos),
            now,
        )
        .excluded
        .is_none()
    }

    pub(super) fn account_score(&self, adapter: AccountAdapter, id: &str) -> f64 {
        let Some(book) = self.account_book(adapter) else {
            return f64::NEG_INFINITY;
        };
        let book = book.lock().unwrap_or_else(|e| e.into_inner());
        let candidate = AccountCandidate {
            id,
            logged_in: true,
            in_use: self.account_in_use(adapter, id),
        };
        crate::accounts::evaluate(
            &candidate,
            book.state(id),
            self.config
                .accounts
                .as_ref()
                .map_or(1, |c| c.max_runs_per_account),
            (self.now_unix_fn)(),
        )
        .score
        .unwrap_or(f64::NEG_INFINITY)
    }

    /// ADR-0024 D3 / ADR-0025 D2: `[accounts]` の指定アダプタのプールから 1 アカウントを選ぶ（残量に基づく決定的な
    /// 選択）。そのアダプタの根ディレクトリが無い、または選べるアカウントが無ければ `None`。
    /// ディレクトリのスキャンは tick につき高々 1 回（アダプタごと）。
    pub(super) fn pick_account(
        &mut self,
        adapter: AccountAdapter,
        requested: Option<&str>,
        cos: bool,
    ) -> Option<String> {
        let cfg = self.config.accounts.clone()?;
        let root = cfg.root_for(adapter)?;
        let dirs = self
            .accounts_scan_cache
            .entry(adapter)
            .or_insert_with(|| scan_accounts(root, adapter))
            .clone();
        let now = (self.now_unix_fn)();
        let book = self.account_book(adapter)?;
        let book = book.lock().unwrap_or_else(|e| e.into_inner());
        let candidates: Vec<AccountCandidate<'_>> = dirs
            .iter()
            .filter(|d| requested.is_none_or(|id| d.id == id))
            .map(|d| AccountCandidate {
                id: d.id.as_str(),
                logged_in: d.logged_in,
                in_use: self.account_in_use(adapter, &d.id),
            })
            .collect();
        if cos {
            let limit = crate::capacity::account_run_limit(cfg.max_runs_per_account, true);
            return crate::accounts::select_account_least_loaded(&candidates, &book, limit, now);
        }
        select_account(&candidates, &book, cfg.max_runs_per_account, now)
    }

    /// ADR-0024 D4: プール run の供給側失敗をアカウントの cooldown として記録する（プロバイダは cooldown にしない）。
    /// `reason` は `provider_failure_reason` と同じ語彙（`throttled` / `auth_failed` / `exhausted` / `spawn`）。
    pub(super) fn record_account_failure(
        &self,
        adapter: AccountAdapter,
        account_id: &str,
        reason: &str,
        outcome: &ProviderOutcome,
    ) {
        let Some(cfg) = &self.config.accounts else {
            return;
        };
        let now = (self.now_unix_fn)();
        let fallback_secs = match outcome {
            ProviderOutcome::Throttled { retry_after } if retry_after.as_secs() > 0 => {
                retry_after.as_secs()
            }
            _ => cfg.fallback_cooldown_secs,
        };
        let cooldown_reason = account_cooldown_reason_from_failure(reason);
        let Some(book) = self.account_book(adapter) else {
            return;
        };
        let Ok(mut book) = book.lock() else { return };
        let cooldown = {
            let state = book.state(account_id);
            cooldown_for_failure(state, cooldown_reason, now, fallback_secs)
        };
        book.set_cooldown(account_id, cooldown, now);
        if let Err(e) = book.save() {
            tracing::warn!(%account_id, %adapter, error = %e, "failed to save account book after cooldown");
        }
    }

    /// ADR-0024 D2 / ADR-0025 D2: プールから選んだアカウントの環境変数（claude-code は
    /// `CLAUDE_SECURESTORAGE_CONFIG_DIR`、codex は `CODEX_HOME`）を末尾に重ねたアダプタを返す。`with_env` が
    /// `None`（アダプタがこの経路を実装していない）なら `None`（呼び出し側は満杯として扱う）。
    pub(super) fn adapter_for_account(
        &self,
        base: &Arc<dyn WorkerAdapter>,
        account_adapter: AccountAdapter,
        account_id: &str,
    ) -> Option<Arc<dyn WorkerAdapter>> {
        let cfg = self.config.accounts.as_ref()?;
        let root = cfg.root_for(account_adapter)?;
        let dir = root.join(account_id);
        base.with_env(&[(
            account_adapter.env_var().to_string(),
            dir.display().to_string(),
        )])
    }
}
