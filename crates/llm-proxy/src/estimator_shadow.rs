//! ADR 2026-10-04 §7.3・§10 Phase 5: estimator shadow（`kind = estimator` の decision shadow）。
//!
//! primary は heuristic / legacy で選んだまま変えない。primary が成功した後に [`EstimatorShadow::submit`]
//! が同期で返り、sidecar への `POST /estimate` は `tokio::spawn` の中で走る（primary は完了を待たない）。
//! 返った検証済み snapshot を [`QualityEstimator`] として shadow kernel（task-core の `optimize`）に渡し、
//! 選ばれた候補を primary と比べて `routing_shadow_recorded`（ADR §6 の `routing_shadow_evaluated`、
//! `kind = estimator`）に残す。
//!
//! - 決定権なし: `shadow_only = false` は [`EstimatorShadow::new`] で拒否する。kernel は hard constraint・
//!   privacy の除外を estimate より前に当てるので、除外された候補を sidecar が高評価しても選ばれない。
//!   sidecar には除外後の候補 id だけを送る（除外候補の id も外に出さない）。
//! - 上限: 呼び出しは `ShadowPolicy`（allowlist・標本化・日次上限）の対象判定と [`ShadowBudget`] の
//!   日次予約を通ったものだけ。sidecar の推論費用は計測できないので、予約した最悪値
//!   （`worst_call_effective_usd`）で確定する（ゼロにしない）。最悪値が分からなければ `unknown_cost` で
//!   dropped（送信 0）。`resource_group` があれば primary と共有の [`ReservationTable`] から枠を待たずに
//!   取り、埋まっていれば `concurrency_limit` で dropped（resource pressure に数える）。
//! - 記録: `policy_version = estimator:<id>/<version>`、status は completed / failed / dropped、reason と
//!   `detail`（sidecar 側の理由語・primary との比較）、`latency_ms` は推論 overhead（sidecar 往復）。

use std::sync::Arc;

use serde_json::{Map, Value, json};
use task_core::Tier;
use task_core::model_router::context::RoutingContext;
use task_core::model_router::estimator::QualityEstimator;
use task_core::model_router::estimator::sidecar::{EstimateCandidateV1, EstimateRequestV1};
use task_core::model_router::optimizer::{Candidate, optimize};
use task_core::model_router::policy::{RoutingMode, RoutingPolicy};
use task_core::model_router::shadow::{
    ShadowAllowlist, ShadowKind, ShadowPolicy, ShadowReason, ShadowRecord, ShadowReservation,
    ShadowReservationRequest, ShadowSettlement, ShadowStatus, ShadowTarget,
};
use tokio::time::Instant;

use crate::estimator_sidecar::{SidecarEstimatorClient, SidecarUnavailable};
use crate::legacy_catalog::LegacyCatalog;
use crate::reservation::{Clock, ReservationTable, SlotKey};
use crate::shadow::{ShadowBudget, ShadowCandidate, ShadowEvent, ShadowSink};

/// 品質の floor・候補順位の理由のうち「hard constraint ではない」もの（sidecar に送ってよい候補）。
const QUALITY_REASONS: [&str; 3] = ["quality_unknown", "quality_below_min", "quality_invalid"];

/// estimator shadow の設定（daemon の `[model_routing.estimator.sidecar]` から組み立てる）。
#[derive(Debug, Clone)]
pub struct EstimatorShadowConfig {
    /// Phase 5 では常に true。false は [`EstimatorShadow::new`] が拒否する。
    pub shadow_only: bool,
    /// 対象判定と日次上限（`execute = true` で検証を通るものだけ受け付ける）。
    pub policy: ShadowPolicy,
    /// sidecar 1 回の最悪 effective 費用。`None`・非有限・負は未知（`unknown_cost` で dropped）。
    pub worst_call_effective_usd: Option<f64>,
    /// 日次予約の token に数える 1 回分（sidecar は LLM token を使わないが、0 にせず数えられる）。
    pub worst_call_tokens: u64,
    /// sidecar が使う計算資源の group（primary と共有の予約表で数える）。
    pub resource_group: Option<String>,
    /// prompt を sidecar へ渡してよい対象（既定は空＝渡さない。client の `send_prompt` も要る）。
    pub prompt_allowlist: ShadowAllowlist,
}

/// [`EstimatorShadow::new`] の拒否理由。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EstimatorShadowError {
    #[error("estimator sidecar must be shadow_only (heuristic remains primary)")]
    ShadowOnlyRequired,
    #[error("estimator shadow requires an executable shadow policy with daily caps")]
    PolicyNotExecutable,
}

/// primary 決定 1 件の比較の入力（要求の本文は持たない。prompt は allowlist 内のときだけ）。
#[derive(Debug, Clone)]
pub struct EstimatorShadowInput {
    pub primary_decision_id: String,
    pub task_id: Option<String>,
    pub run_id: Option<String>,
    pub request_id: Option<String>,
    pub lane: Tier,
    /// 対象判定の属性（`source` は primary の source）。
    pub target: ShadowTarget,
    pub context: RoutingContext,
    /// primary の候補列（要求の制約を通ったもの、試した順）。
    pub candidates: Vec<ShadowCandidate>,
    pub primary: ShadowCandidate,
    pub prompt: Option<String>,
}

/// `submit` の結果（primary はこれを見るだけで sidecar を待たない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstimatorSubmit {
    /// 入口の判定（off・allowlist 外・標本外）で落ちた。記録しない（送信 0）。
    NotAdmitted(ShadowReason),
    /// dropped として記録した（送信 0）。
    Dropped(ShadowReason),
    /// 評価を始めた（結果は sink に後から届く）。
    Started,
}

/// shadow kernel の結果。
#[derive(Debug, Clone, PartialEq)]
pub struct KernelChoice {
    /// estimator が選んだ候補（hard constraint を通った中から）。
    pub chosen: Option<ShadowCandidate>,
    /// hard constraint を通った候補の model profile id（sidecar に送る id）。
    pub eligible_model_ids: Vec<String>,
}

/// shadow kernel: `candidates` を catalog の deployment に写し、`optimize` に `estimator` を渡して
/// 先頭を返す。policy は `Shadow` mode で回す（legacy の素通しにしない）。catalog に無い候補は
/// 比較の対象外。hard constraint・privacy の除外は estimate より前に当たる。
pub fn kernel_choice(
    catalog: &LegacyCatalog,
    policy: &RoutingPolicy,
    context: &RoutingContext,
    candidates: &[ShadowCandidate],
    estimator: &dyn QualityEstimator,
) -> KernelChoice {
    let mut policy = policy.clone();
    policy.mode = RoutingMode::Shadow;
    let mut mapped = Vec::new();
    for c in candidates {
        let Some(deployment) = catalog.deployments.iter().find(|d| {
            d.source_ref == c.source
                && d.upstream_model == c.model
                && d.allowed_lanes.contains(&policy.lane)
        }) else {
            continue;
        };
        if let Some(model) = catalog
            .models
            .iter()
            .find(|m| m.id == deployment.model_profile_id)
        {
            mapped.push((c, model, deployment));
        }
    }
    let kernel: Vec<Candidate<'_>> = mapped
        .iter()
        .map(|(_, model, deployment)| Candidate {
            model,
            deployment,
            state: None,
            eligible_provider_ids: vec![deployment.source_ref.clone()],
            cost_usd: None,
            latency_ms: None,
            pressure: None,
        })
        .collect();
    let Ok(result) = optimize(&policy, context, &kernel, estimator) else {
        return KernelChoice {
            chosen: None,
            eligible_model_ids: Vec::new(),
        };
    };
    let mut eligible_model_ids: Vec<String> = Vec::new();
    for trace in &result.traces {
        let hard = trace
            .excluded_reasons
            .iter()
            .any(|r| !QUALITY_REASONS.contains(&r.as_str()));
        if !hard && !eligible_model_ids.contains(&trace.model_profile_id) {
            eligible_model_ids.push(trace.model_profile_id.clone());
        }
    }
    let chosen = result.ranked.first().and_then(|top| {
        mapped
            .iter()
            .find(|(_, _, d)| d.id == top.deployment_id)
            .map(|(c, _, _)| (*c).clone())
    });
    KernelChoice {
        chosen,
        eligible_model_ids,
    }
}

fn lane_policy(catalog: &LegacyCatalog, lane: Tier) -> RoutingPolicy {
    catalog
        .policies
        .iter()
        .find(|p| p.lane == lane)
        .cloned()
        .unwrap_or_else(|| RoutingPolicy::defaults(lane, RoutingMode::Shadow))
}

/// sidecar の評価不能を記録の (status, reason) に写す。送る前の gate は dropped、送った後は failed。
pub fn classify_unavailable(error: &SidecarUnavailable) -> (ShadowStatus, ShadowReason) {
    match error {
        SidecarUnavailable::Timeout => (ShadowStatus::Failed, ShadowReason::Timeout),
        SidecarUnavailable::CircuitOpen | SidecarUnavailable::Inflight => {
            (ShadowStatus::Dropped, ShadowReason::ConcurrencyLimit)
        }
        SidecarUnavailable::PromptRequired | SidecarUnavailable::DependenciesNotAllowed => {
            (ShadowStatus::Dropped, ShadowReason::Privacy)
        }
        SidecarUnavailable::PayloadTooLarge(_) => {
            (ShadowStatus::Dropped, ShadowReason::CapExceeded)
        }
        _ => (ShadowStatus::Failed, ShadowReason::UpstreamError),
    }
}

/// estimator shadow 一式。状態は持たない（client の circuit を除く）。
pub struct EstimatorShadow {
    config: EstimatorShadowConfig,
    client: Arc<SidecarEstimatorClient>,
    catalog: Arc<LegacyCatalog>,
    budget: Arc<dyn ShadowBudget>,
    sink: Arc<dyn ShadowSink>,
    clock: Arc<dyn Clock>,
    owner: String,
    capacity: Option<Arc<ReservationTable>>,
}

impl EstimatorShadow {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: EstimatorShadowConfig,
        client: Arc<SidecarEstimatorClient>,
        catalog: Arc<LegacyCatalog>,
        owner: impl Into<String>,
        budget: Arc<dyn ShadowBudget>,
        sink: Arc<dyn ShadowSink>,
        clock: Arc<dyn Clock>,
    ) -> Result<Arc<Self>, EstimatorShadowError> {
        if !config.shadow_only {
            return Err(EstimatorShadowError::ShadowOnlyRequired);
        }
        if config.policy.daily_caps().is_none() {
            return Err(EstimatorShadowError::PolicyNotExecutable);
        }
        Ok(Arc::new(Self {
            config,
            client,
            catalog,
            budget,
            sink,
            clock,
            owner: owner.into(),
            capacity: None,
        }))
    }

    /// primary と共有する実行枠の表を差し込む（作った直後、まだ共有していない `Arc` にだけ効く）。
    pub fn with_capacity(mut self: Arc<Self>, table: Arc<ReservationTable>) -> Arc<Self> {
        match Arc::get_mut(&mut self) {
            Some(shadow) => shadow.capacity = Some(table),
            None => tracing::warn!("llm-proxy: estimator shadow capacity ignored (already shared)"),
        }
        self
    }

    /// `target` に prompt を渡してよいか（既定の空 allowlist は常に false）。
    pub fn prompt_allowed(&self, target: &ShadowTarget) -> bool {
        self.config.prompt_allowlist.matches(target)
    }

    /// `kind = estimator` の版（`estimator:<id>/<version>`）。
    pub fn policy_version(&self) -> String {
        let d = self.client.descriptor();
        format!("estimator:{}/{}", d.estimator_id, d.version)
    }

    /// 同期で返る。sidecar 呼び出しは spawn する（tokio の runtime の中から呼ぶ）。
    pub fn submit(self: &Arc<Self>, input: EstimatorShadowInput) -> EstimatorSubmit {
        if let Err(reason) = self
            .config
            .policy
            .admit(&input.target, &input.primary_decision_id)
        {
            return EstimatorSubmit::NotAdmitted(reason);
        }
        let shadow_id = format!("she_{}", ulid::Ulid::new());
        let policy = lane_policy(&self.catalog, input.lane);
        let heuristic = kernel_choice(
            &self.catalog,
            &policy,
            &input.context,
            &input.candidates,
            &task_core::model_router::estimator::HeuristicEstimator,
        );
        if heuristic.eligible_model_ids.is_empty() {
            let mut record = self.record(&shadow_id, &input, ShadowStatus::Dropped);
            record.reason = Some(ShadowReason::NotAllowlisted);
            record.detail = Some("no_eligible_candidate".into());
            self.emit(&input, record);
            return EstimatorSubmit::Dropped(ShadowReason::NotAllowlisted);
        }
        let this = Arc::clone(self);
        tokio::spawn(async move {
            this.run(shadow_id, input, policy, heuristic).await;
        });
        EstimatorSubmit::Started
    }

    async fn run(
        &self,
        shadow_id: String,
        input: EstimatorShadowInput,
        policy: RoutingPolicy,
        heuristic: KernelChoice,
    ) {
        // primary が先に取った残りの枠だけを使う（待たない）。枠は sidecar の往復が終わるまで持つ。
        let _slot = match (&self.capacity, &self.config.resource_group) {
            (Some(table), Some(group)) => {
                match table.try_reserve(&[SlotKey::group(group.clone())]) {
                    Ok(slot) => Some(slot),
                    Err(_) => {
                        self.dropped(
                            &shadow_id,
                            &input,
                            ShadowReason::ConcurrencyLimit,
                            Some(format!("resource_group_full:{group}")),
                        );
                        return;
                    }
                }
            }
            _ => None,
        };
        let worst_usd = self
            .config
            .worst_call_effective_usd
            .filter(|usd| usd.is_finite() && *usd >= 0.0);
        let request = ShadowReservationRequest {
            shadow_id: shadow_id.clone(),
            owner: self.owner.clone(),
            worst_tokens: self.config.worst_call_tokens,
            worst_effective_usd: worst_usd,
        };
        let reservation_id = match self.budget.reserve(&request, self.clock.now()) {
            ShadowReservation::Reserved { reservation_id, .. } => reservation_id,
            ShadowReservation::Denied(reason) => {
                self.dropped(&shadow_id, &input, reason, None);
                return;
            }
        };
        let estimate_request = EstimateRequestV1 {
            request_id: shadow_id.replace('_', "-"),
            context_features: context_features(&input),
            candidates: heuristic
                .eligible_model_ids
                .iter()
                .map(|id| EstimateCandidateV1 {
                    model_profile_id: id.clone(),
                })
                .collect(),
            optional_prompt: input.prompt.clone(),
        };
        let started = Instant::now();
        let result = self.client.estimate(estimate_request).await;
        let overhead_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let mut record = match result {
            Ok(snapshot) => {
                // 推論費用は計測できない: 予約した最悪値で確定する（ゼロにしない）。
                self.budget.settle(
                    &reservation_id,
                    ShadowSettlement::Completed {
                        tokens: self.config.worst_call_tokens,
                        effective_usd: worst_usd.unwrap_or(0.0),
                    },
                );
                let choice = kernel_choice(
                    &self.catalog,
                    &policy,
                    &input.context,
                    &input.candidates,
                    snapshot.as_ref(),
                );
                let mut r = self.record(&shadow_id, &input, ShadowStatus::Completed);
                r.detail = Some(format!(
                    "{};heuristic:{}",
                    compare(choice.chosen.as_ref(), &input.primary),
                    compare(
                        choice.chosen.as_ref(),
                        heuristic.chosen.as_ref().unwrap_or(&input.primary)
                    ),
                ));
                r.candidate_model = choice.chosen.as_ref().map(|c| c.model.clone());
                r.candidate_source = choice.chosen.map(|c| c.source);
                r
            }
            Err(error) => {
                let (status, reason) = classify_unavailable(&error);
                let settlement = match error {
                    SidecarUnavailable::Timeout => ShadowSettlement::TimedOut,
                    _ => ShadowSettlement::Failed {
                        tokens: None,
                        effective_usd: None,
                    },
                };
                // 失敗・送る前の gate も予約の最悪値で数える（store が予約と実測の大きい方を取る）。
                self.budget.settle(&reservation_id, settlement);
                let mut r = self.record(&shadow_id, &input, status);
                r.reason = Some(reason);
                r.detail = Some(error.reason().to_string());
                r
            }
        };
        record.latency_ms = Some(overhead_ms);
        record.reservation_id = Some(reservation_id);
        self.emit(&input, record);
    }

    fn dropped(
        &self,
        shadow_id: &str,
        input: &EstimatorShadowInput,
        reason: ShadowReason,
        detail: Option<String>,
    ) {
        let mut record = self.record(shadow_id, input, ShadowStatus::Dropped);
        record.reason = Some(reason);
        record.detail = detail;
        self.emit(input, record);
    }

    fn record(
        &self,
        shadow_id: &str,
        input: &EstimatorShadowInput,
        status: ShadowStatus,
    ) -> ShadowRecord {
        ShadowRecord {
            shadow_id: shadow_id.to_string(),
            primary_decision_id: input.primary_decision_id.clone(),
            kind: ShadowKind::Estimator,
            status,
            reason: None,
            detail: None,
            policy_version: self.policy_version(),
            run_id: input.run_id.clone(),
            request_id: input.request_id.clone(),
            candidate_model: None,
            candidate_source: None,
            input_tokens: None,
            output_tokens: None,
            output_sha256: None,
            cash_usd: None,
            effective_usd: None,
            latency_ms: None,
            reservation_id: None,
        }
    }

    fn emit(&self, input: &EstimatorShadowInput, record: ShadowRecord) {
        if let Err(e) = record.validate() {
            tracing::warn!(error = %e, "llm-proxy: invalid estimator shadow record; not recorded");
            return;
        }
        self.sink.record(ShadowEvent {
            task_id: input.task_id.clone(),
            record,
        });
    }
}

fn compare(chosen: Option<&ShadowCandidate>, other: &ShadowCandidate) -> &'static str {
    match chosen {
        None => "no_candidate",
        Some(c) if c == other => "same_as_primary",
        Some(_) => "differs_from_primary",
    }
}

/// sidecar に渡す特徴（本文・credential・path は入れない）。
fn context_features(input: &EstimatorShadowInput) -> Map<String, Value> {
    let ctx = &input.context;
    let mut features = Map::new();
    features.insert("lane".into(), json!(input.target.lane));
    if let Some(kind) = &ctx.task_kind {
        features.insert("task_kind".into(), json!(kind));
    }
    if let Some(role) = &ctx.role {
        features.insert("role".into(), json!(role));
    }
    if let Some(tokens) = ctx.input_tokens {
        features.insert("input_tokens".into(), json!(tokens));
    }
    if let Some(tokens) = ctx.output_reserve {
        features.insert("output_reserve".into(), json!(tokens));
    }
    features.insert("required_tools".into(), json!(ctx.required_tools));
    features.insert("required_streaming".into(), json!(ctx.required_streaming));
    features
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_failures_map_to_dropped_before_send_and_failed_after() {
        assert_eq!(
            classify_unavailable(&SidecarUnavailable::PromptRequired),
            (ShadowStatus::Dropped, ShadowReason::Privacy)
        );
        assert_eq!(
            classify_unavailable(&SidecarUnavailable::Timeout),
            (ShadowStatus::Failed, ShadowReason::Timeout)
        );
        assert_eq!(
            classify_unavailable(&SidecarUnavailable::Status(500)),
            (ShadowStatus::Failed, ShadowReason::UpstreamError)
        );
    }
}
