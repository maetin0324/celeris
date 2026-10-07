//! ADR 2026-10-06 model-role-assignments 付記「モデルごとの複数役割と優先度」: 役割に同じ source の
//! モデルが複数あるとき、shadow / enforce はその source の**全メンバー**を候補にし、kernel
//! （task-core の `optimize` + 品質推定）で 1 つに決める。推定できない（品質 unknown・同点）ときは
//! membership の priority 順で決定的に選ぶ。legacy はこの module を使わず、priority 順の先頭を使う。
//!
//! - 候補の identity は `<provider>/model:<model_id>`（`LegacyProfile.deployment.id`）、容量・account の
//!   identity は provider（`LegacyProfile.provider_id`）。kernel には同じ source 状態を全メンバーに渡す。
//! - 品質は daemon が差し込む routing catalog の model profile（`RoutingModelProfiles`）から引く。無ければ
//!   `quality_unknown` で priority 順に落ちる。LLM も HTTP も呼ばない。

use super::provider_select::LegacyProfile;
use super::*;
use task_core::model_router::{
    context::standalone_context,
    estimator::HeuristicEstimator,
    optimizer::{Candidate, optimize},
    policy::{RoutingMode, RoutingPolicy},
    profiles::{ModelProfile, SourceState},
    trace::CandidateTrace,
};

/// routing catalog の model profile を dispatcher へ渡す口（daemon が共有 snapshot を包んで差し込む）。
pub trait RoutingModelProfiles: Send + Sync {
    fn model_profiles(&self) -> Vec<ModelProfile>;
}

/// 固定の一覧を返す（試験用）。
#[derive(Debug, Clone, Default)]
pub struct StaticModelProfiles(pub Vec<ModelProfile>);

impl RoutingModelProfiles for StaticModelProfiles {
    fn model_profiles(&self) -> Vec<ModelProfile> {
        self.0.clone()
    }
}

/// `RoutingTraceV1.estimator_version` / `MemberRanking.ranked_by`: kernel が品質で順位を決めた。
pub const MEMBER_RANKING_ESTIMATOR: &str = "heuristic-1";
/// 推定不能・同点で membership の priority 順に決めた。
pub const MEMBER_RANKING_PRIORITY: &str = "priority";

/// 1 つの provider の役割メンバーの順位づけの結果。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MemberRanking {
    /// 実行する wire model（メンバーが無ければ `None` = 実効 bindings のまま）。
    pub chosen: Option<String>,
    pub chosen_deployment: Option<String>,
    /// `MEMBER_RANKING_ESTIMATOR` か `MEMBER_RANKING_PRIORITY`（メンバーが無ければ空）。
    pub ranked_by: &'static str,
    /// 決めた順（deployment id）。kernel が順位を付けたものが先、推定不能のものは priority 順で後ろ。
    pub order: Vec<String>,
    /// kernel の候補ごとの trace（score・品質・除外理由）。
    pub traces: Vec<CandidateTrace>,
}

impl MemberRanking {
    pub fn trace_for(&self, deployment_id: &str) -> Option<&CandidateTrace> {
        self.traces
            .iter()
            .find(|t| t.deployment_id == deployment_id)
    }
}

impl Dispatcher {
    /// routing catalog の model profile（品質・context 上限）を差し込む。無ければ品質は unknown。
    pub fn set_routing_model_profiles(&mut self, reader: Arc<dyn RoutingModelProfiles>) {
        self.routing_model_profiles = Some(reader);
    }

    /// catalog に同じ id（`<wire>`）か `…:<wire>`（llm-proxy 由来の `legacy:<family>:<wire>`）の profile が
    /// あればその品質・context 上限・価格を写す。無ければそのまま。
    fn enrich_model(&self, catalog: &[ModelProfile], model: &ModelProfile) -> ModelProfile {
        let suffix = format!(":{}", model.id);
        let Some(found) = catalog
            .iter()
            .find(|m| m.id == model.id)
            .or_else(|| catalog.iter().find(|m| m.id.ends_with(&suffix)))
        else {
            return model.clone();
        };
        let mut out = model.clone();
        out.quality = found.quality.clone();
        out.context_limits = found.context_limits.clone();
        out.pricing = found.pricing.clone();
        out.capabilities = found.capabilities.clone();
        out.family = found.family.clone();
        out
    }

    /// `provider_id` の役割メンバー（`legacy_provider_profiles` の割り当て由来の行）を kernel で順位づけする。
    /// `state` は選んだ source / account の状態（全メンバーで共有）。
    pub(super) fn rank_role_members(
        &self,
        lane: Tier,
        provider_id: &str,
        profiles: &[LegacyProfile],
        state: Option<&SourceState>,
    ) -> MemberRanking {
        let members: Vec<&LegacyProfile> = profiles
            .iter()
            .filter(|p| {
                p.provider_id == provider_id && p.model.provenance == "model_role_assignments"
            })
            .collect();
        if members.is_empty() {
            return MemberRanking::default();
        }
        let catalog = self
            .routing_model_profiles
            .as_ref()
            .map(|r| r.model_profiles())
            .unwrap_or_default();
        let models: Vec<ModelProfile> = members
            .iter()
            .map(|p| self.enrich_model(&catalog, &p.model))
            .collect();
        let candidates: Vec<Candidate<'_>> = members
            .iter()
            .zip(models.iter())
            .map(|(p, model)| Candidate {
                model,
                deployment: &p.deployment,
                state,
                eligible_provider_ids: vec![provider_id.to_string()],
                cost_usd: None,
                latency_ms: None,
                pressure: None,
            })
            .collect();
        // Shadow mode の policy で回す: dispatcher は input tokens を知らないので Enforce の
        // `context_unknown` 除外は当てない（供給側の除外・品質の floor は当たる）。
        let policy = RoutingPolicy::defaults(lane, RoutingMode::Shadow);
        let context = standalone_context("dispatcher:role-members");
        let result = optimize(&policy, &context, &candidates, &HeuristicEstimator).ok();
        let mut order: Vec<String> = result
            .as_ref()
            .map(|r| r.ranked.iter().map(|c| c.deployment_id.clone()).collect())
            .unwrap_or_default();
        let ranked_by = if order.is_empty() {
            MEMBER_RANKING_PRIORITY
        } else {
            MEMBER_RANKING_ESTIMATOR
        };
        for p in &members {
            if !order.contains(&p.deployment.id) {
                order.push(p.deployment.id.clone());
            }
        }
        let chosen_deployment = order.first().cloned();
        let chosen = chosen_deployment
            .as_ref()
            .and_then(|id| members.iter().find(|p| p.deployment.id == *id))
            .map(|p| p.model.id.clone());
        MemberRanking {
            chosen,
            chosen_deployment,
            ranked_by,
            order,
            traces: result.map(|r| r.traces).unwrap_or_default(),
        }
    }

    /// run 用アダプタの `lane` の束縛を kernel が選んだ wire model に替える（実効 bindings の他 lane は保つ）。
    pub(super) fn rebind_lane_model(
        &self,
        provider_id: &str,
        adapter: Arc<dyn WorkerAdapter>,
        lane: Tier,
        model: &str,
        view: &task_core::model_catalog::assignments::AssignmentView,
    ) -> Arc<dyn WorkerAdapter> {
        if adapter.model_for_tier(lane).ok().flatten().as_deref() == Some(model) {
            return adapter;
        }
        let Some(mut effective) = self.effective_tier_models(provider_id, view) else {
            return adapter;
        };
        let effort = effective
            .get(&lane)
            .and_then(|b| b.reasoning_effort.clone());
        effective.insert(
            lane,
            task_core::model_routing::ModelBinding {
                name: model.to_string(),
                model_id: Some(model.to_string()),
                unavailable_reason: None,
                reasoning_effort: effort,
            },
        );
        adapter.with_tier_models(effective).unwrap_or(adapter)
    }
}
