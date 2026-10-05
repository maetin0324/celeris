//! state 選択（ADR 2026-10-04-multi-objective-model-routing Phase 2・p2-proxy-select）。
//!
//! 入力は候補ごとの `SourceState` の snapshot・要求の最小 context（[`request_context`]）・policy の制約
//! （min_quality・tools・privacy・context floor）・注入した時刻 `now`。手順:
//! 1. 供給の除外（score より前）: cooldown / rate limit の終了前、新鮮な観測での枯渇
//!    （`cost::exclusion_reasons`）と、予約表で枠が埋まった候補（`concurrency`）。
//! 2. 残りの候補に effective cost（`cost::estimate_cost`）を付け、kernel の `optimize` に渡す。
//!    `optimize` は lane・privacy・capability・context・min_quality などの制約を score より前に当て、
//!    合格した候補だけを score で並べる。
//! 3. 落ちた候補は全部 trace（`CandidateTrace.excluded_reasons` / `excluded_reason`）に残す。
//!
//! mode=legacy の既存経路（`select_across_pools` 等）はここを通らない。同じ snapshot・時刻・予約表から
//! 同じ結果になる（I/O も時計の読み出しもしない）。
//!
//! OAuth pool の account は deployment を account ごとに分けた候補にする（trace の deployment id は
//! `<deployment>@<account>`）。kernel は deployment id で候補を区別するため。

use sha2::{Digest, Sha256};
use task_core::Tier;
use task_core::model_router::context::RoutingContext;
use task_core::model_router::cost::{
    CostEstimate, CostInputs, RequestUsage, SelfHostRates, estimate_cost, exclusion_reasons,
};
use task_core::model_router::estimator::QualityEstimator;
use task_core::model_router::optimizer::{Candidate, optimize};
use task_core::model_router::policy::{FreshnessPolicy, RoutingMode, RoutingPolicy};
use task_core::model_router::profiles::{DeploymentProfile, ModelProfile, SourceState};
use task_core::model_router::trace::{CandidateTrace, ExcludedReason, RoutingTraceV1};
use task_core::store::RoutingCorrelation;
use time::OffsetDateTime;

use crate::openai::ChatCompletionRequest;
use crate::reservation::{Reservable, ReservationTable, SlotKey};

/// 候補 1 件（deployment と、その観測 snapshot・OAuth account）。
#[derive(Debug, Clone, Copy)]
pub struct StateCandidate<'a> {
    pub model: &'a ModelProfile,
    pub deployment: &'a DeploymentProfile,
    pub state: &'a SourceState,
    /// subscription pool の account。self-host・API は None。
    pub account_id: Option<&'a str>,
}

/// 選択の設定（policy・要求・見積もりの係数）。
pub struct StateSelectInput<'a> {
    pub policy: &'a RoutingPolicy,
    pub context: &'a RoutingContext,
    pub estimator: &'a dyn QualityEstimator,
    pub usage: RequestUsage,
    pub self_host: SelfHostRates,
    pub freshness: FreshnessPolicy,
}

/// 選ばれうる候補（順位順）。予約の枠を持つ。
#[derive(Debug, Clone, PartialEq)]
pub struct StatePick {
    /// trace 上の候補 id（account 付きは `<deployment>@<account>`）。
    pub candidate_id: String,
    pub deployment_id: String,
    pub model_profile_id: String,
    pub source_ref: String,
    pub upstream_model: String,
    pub account_id: Option<String>,
    pub score: Option<f64>,
    pub slots: Vec<SlotKey>,
}

impl Reservable for StatePick {
    fn slots(&self) -> Vec<SlotKey> {
        self.slots.clone()
    }
}

/// 選択の結果。`traces` は全候補（落ちた候補も）を持つ。
#[derive(Debug, Clone, PartialEq)]
pub struct StateSelection {
    pub ranked: Vec<StatePick>,
    pub traces: Vec<CandidateTrace>,
    /// `ranked` / `defer`（供給の都合で全部落ちた）/ `unroutable`（制約で全部落ちた）。
    pub outcome: &'static str,
}

/// 要求の最小 context（既存の要求の欄から。詳細の搬送は Phase 3）。
/// input は本文の文字数 / 4 の切り上げ（下限の目安）、output は `max_tokens`。
pub fn request_context(req: &ChatCompletionRequest) -> RoutingContext {
    let chars: usize = req
        .messages
        .iter()
        .filter_map(|m| m.content.as_ref())
        .map(|c| c.as_text().chars().count())
        .sum();
    RoutingContext {
        version: "proxy-request-v1".into(),
        origin: "llm-proxy".into(),
        task_id: None,
        work_unit_id: None,
        run_id: None,
        role: None,
        harness: None,
        task_kind: None,
        required_tools: req.tools.as_ref().is_some_and(|t| !t.is_empty()),
        required_structured_output: false,
        required_vision: false,
        required_streaming: req.stream,
        input_tokens: Some(chars.div_ceil(4) as u64),
        output_reserve: req.max_tokens.map(u64::from),
        safety_margin: 0,
        provenance: "llm-proxy:request-fields".into(),
        ..RoutingContext::default()
    }
}

/// 候補の予約の枠: account（あれば）と共有 GPU の resource group（あれば）。別の枠として数える。
pub fn slots_for(deployment: &DeploymentProfile, account_id: Option<&str>) -> Vec<SlotKey> {
    let mut slots = Vec::new();
    if let Some(account) = account_id {
        slots.push(SlotKey::account(&deployment.source_ref, account));
    }
    if let Some(group) = &deployment.resource_group_id {
        slots.push(SlotKey::group(group));
    }
    slots
}

/// snapshot の id（候補の状態の JSON の sha256 先頭 16 桁）。同じ snapshot は同じ id。
pub fn snapshot_id(candidates: &[StateCandidate<'_>]) -> String {
    let mut hasher = Sha256::new();
    for c in candidates {
        hasher.update(c.deployment.id.as_bytes());
        hasher.update([0]);
        hasher.update(c.account_id.unwrap_or("").as_bytes());
        hasher.update([0]);
        hasher.update(serde_json::to_vec(c.state).unwrap_or_default());
        hasher.update([0xff]);
    }
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("snap-{}", &hex[..16])
}

fn candidate_id(deployment: &DeploymentProfile, account_id: Option<&str>) -> String {
    match account_id {
        Some(a) => format!("{}@{a}", deployment.id),
        None => deployment.id.clone(),
    }
}

fn excluded_trace(
    id: String,
    c: &StateCandidate<'_>,
    reasons: Vec<String>,
    estimate: Option<&CostEstimate>,
) -> CandidateTrace {
    let mut trace = CandidateTrace {
        model_profile_id: c.model.id.clone(),
        deployment_id: id,
        eligible_provider_ids: vec![c.deployment.source_ref.clone()],
        excluded_reason: ExcludedReason::primary(&reasons),
        excluded_reasons: reasons,
        config_order: Some(c.deployment.config_order),
        latency_ms: c.state.latency_ms,
        ..CandidateTrace::default()
    };
    if let Some(e) = estimate {
        trace = trace.with_cost(e);
        trace.cost_usd = e.effective_usd;
        trace.pressure = e.pressure;
    }
    trace.normalize();
    trace
}

/// state 選択。`occupancy` を渡すと、枠が埋まった候補を score より前に落とす（選び直しで使う）。
pub fn select_state(
    input: &StateSelectInput<'_>,
    candidates: &[StateCandidate<'_>],
    now: OffsetDateTime,
    occupancy: Option<&ReservationTable>,
) -> Result<StateSelection, &'static str> {
    if input.policy.mode == RoutingMode::Legacy {
        return Err("state selection requires mode shadow or enforce");
    }
    input.freshness.validate()?;
    let mut traces = Vec::new();
    let mut supply_excluded = false;
    // kernel に渡す候補（account ごとに deployment を分ける）と、その見積もり。
    let mut scored: Vec<(DeploymentProfile, StateCandidate<'_>, CostEstimate)> = Vec::new();
    for c in candidates {
        let id = candidate_id(c.deployment, c.account_id);
        let mut reasons: Vec<String> = Vec::new();
        match exclusion_reasons(c.state, now, input.freshness) {
            Ok(codes) => reasons.extend(codes.into_iter().map(String::from)),
            Err(_) => reasons.push("invalid_state".into()),
        }
        let slots = slots_for(c.deployment, c.account_id);
        if occupancy.is_some_and(|t| !t.full_slots(&slots).is_empty()) {
            reasons.push("concurrency".into());
        }
        let estimate = estimate_cost(&CostInputs {
            billing: c.deployment.billing,
            pricing: c
                .deployment
                .price_override
                .as_ref()
                .or(c.model.pricing.as_ref()),
            usage: input.usage,
            self_host: input.self_host,
            state: c.state,
            concurrency_limit: c.deployment.concurrency_limit,
            now,
            freshness: input.freshness,
        });
        if !reasons.is_empty() {
            supply_excluded = true;
            traces.push(excluded_trace(id, c, reasons, estimate.as_ref().ok()));
            continue;
        }
        match estimate {
            Ok(e) => {
                let mut d = c.deployment.clone();
                d.id = id;
                scored.push((d, *c, e));
            }
            Err(_) => traces.push(excluded_trace(id, c, vec!["invalid_estimate".into()], None)),
        }
    }
    let kernel: Vec<Candidate<'_>> = scored
        .iter()
        .map(|(d, c, e)| Candidate {
            model: c.model,
            deployment: d,
            state: Some(c.state),
            eligible_provider_ids: vec![d.source_ref.clone()],
            cost_usd: e.effective_usd,
            latency_ms: c.state.latency_ms,
            pressure: e.pressure,
        })
        .collect();
    let result = optimize(input.policy, input.context, &kernel, input.estimator)?;
    for mut t in result.traces {
        if let Some((_, _, e)) = scored.iter().find(|(d, _, _)| d.id == t.deployment_id) {
            t = t.with_cost(e);
        }
        t.normalize();
        traces.push(t);
    }
    let ranked: Vec<StatePick> = result
        .ranked
        .iter()
        .filter_map(|r| {
            scored
                .iter()
                .find(|(d, _, _)| d.id == r.deployment_id)
                .map(|(d, c, _)| StatePick {
                    candidate_id: d.id.clone(),
                    deployment_id: c.deployment.id.clone(),
                    model_profile_id: c.model.id.clone(),
                    source_ref: c.deployment.source_ref.clone(),
                    upstream_model: c.deployment.upstream_model.clone(),
                    account_id: c.account_id.map(String::from),
                    score: r.score,
                    slots: slots_for(c.deployment, c.account_id),
                })
        })
        .collect();
    let outcome = if !ranked.is_empty() {
        "ranked"
    } else if supply_excluded || result.outcome == "defer" {
        "defer"
    } else {
        "unroutable"
    };
    Ok(StateSelection {
        ranked,
        traces,
        outcome,
    })
}

/// trace の id 群（呼び出し側が採番する。log の相関にも使う）。
#[derive(Debug, Clone, Default)]
pub struct TraceIds {
    pub decision_id: String,
    pub snapshot_id: String,
    pub request_id: Option<String>,
    pub task_id: Option<String>,
    pub run_id: Option<String>,
}

/// 選択の結果を `RoutingTraceV1` にする（stage `proxy`）。`chosen` は予約できた候補。
pub fn routing_trace(
    input: &StateSelectInput<'_>,
    selection: &StateSelection,
    chosen: Option<&StatePick>,
    ids: &TraceIds,
    catalog_version: &str,
    now: OffsetDateTime,
) -> RoutingTraceV1 {
    let estimator = input.estimator.descriptor();
    let mut trace = RoutingTraceV1 {
        decision_id: ids.decision_id.clone(),
        parent_decision_id: None,
        task_id: ids.task_id.clone(),
        work_unit_id: None,
        run_id: ids.run_id.clone(),
        request_id: ids.request_id.clone(),
        stage: "proxy".into(),
        mode: input.policy.mode,
        policy_version: input.policy.version.clone(),
        catalog_version: catalog_version.into(),
        feature_version: input.context.version.clone(),
        estimator_version: format!("{}-{}", estimator.id, estimator.version),
        snapshot_id: ids.snapshot_id.clone(),
        observed_at: now
            .format(&time::format_description::well_known::Rfc3339)
            .ok(),
        requested_lane: input.policy.lane,
        selected_lane: chosen.map(|_| input.policy.lane),
        candidates: selection.traces.clone(),
        selected: chosen.map(|p| p.candidate_id.clone()),
        fallback_order: selection
            .ranked
            .iter()
            .map(|p| p.candidate_id.clone())
            .collect(),
        reasons: vec![selection.outcome.into()],
        source_id: chosen.map(|p| p.source_ref.clone()),
        model: chosen.map(|p| p.upstream_model.clone()),
        account_id: chosen.and_then(|p| p.account_id.clone()),
    };
    trace.normalize();
    trace
}

/// log の相関欄（`llm_proxy_requests` 0048）。task events とは log の id（request id）で結ぶ。
pub fn correlation(trace: &RoutingTraceV1) -> RoutingCorrelation {
    RoutingCorrelation {
        decision_id: Some(trace.decision_id.clone()),
        snapshot_id: Some(trace.snapshot_id.clone()),
        run_id: trace.run_id.clone(),
        task_id: trace.task_id.clone(),
        source_id: trace.source_id.clone(),
        model: trace.model.clone(),
        account: trace.account_id.clone(),
    }
}

/// lane の既定 policy（shadow）。設定配線は config unit が行う。
pub fn shadow_policy(lane: Tier) -> RoutingPolicy {
    RoutingPolicy::defaults(lane, RoutingMode::Shadow)
}

#[cfg(test)]
#[path = "selection_state_tests.rs"]
mod tests;
