//! ADR 2026-10-06 model-role-assignments 付記「モデルごとの複数役割と優先度」: 役割に同じ source の
//! モデルが複数あるとき、shadow / enforce はその source の**全メンバー**を候補にし、kernel
//! （task-core の `optimize` + 品質推定）で 1 つに決める。推定できない（品質 unknown・同点）ときは
//! membership の priority 順で決定的に選ぶ。route_select はこの module を使わず、priority 順の先頭を使う。
//!
//! - 候補の identity は `<provider>/model:<model_id>`、容量・account の identity は provider。kernel には
//!   同じ source 状態を全メンバーに渡す。
//! - 品質は daemon が差し込む routing catalog の model profile（`RoutingModelProfiles`）から引く。無ければ
//!   `quality_unknown` で priority 順に落ちる。LLM も HTTP も呼ばない。

use super::*;
use task_core::model_router::{profiles::ModelProfile, trace::CandidateTrace};

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
}
