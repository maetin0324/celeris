//! ADR 2026-10-04 multi-objective model routing §7.1・§10 Phase 4（dispatch-shadow）: decision shadow。
//!
//! - `mode = shadow` のとき primary は legacy の経路（`select_provider_for` と `select_tier`）のまま決め、
//!   同じ候補行（`legacy_provider_profiles`）と同じ source 状態（account 帳簿・cooldown・in-use）で
//!   候補 policy（Phase 2 の heuristic kernel = enforce の判断）を**純粋に**計算する。
//! - 計算は読むだけ: upstream・HTTP・health probe・worker 起動・reviewer 追加・run 再実行をしない。
//!   `full`・`unroutable`・警告の集合・アカウントの scan cache も書き換えない。ローカル行の health は
//!   primary の選択がこの tick に見た結果（`ProviderSelection.candidates`）だけを使う。
//! - 結果は `Event::RoutingShadowRecorded`（kind = decision）1 件。primary との差・除外理由・候補比較は
//!   `DecisionShadowComparison`（version 1）を JSON にして `ShadowRecord.detail` に入れる
//!   （本文・prompt・credential・tokens・費用は持たない）。mode = legacy / enforce では何も記録しない。

use super::*;
use serde::{Deserialize, Serialize};
use task_core::model_router::{
    policy::RoutingMode,
    shadow::ShadowPolicy,
};

/// `DecisionShadowComparison` の wire 版。
pub const DECISION_SHADOW_COMPARISON_VERSION: u32 = 1;

/// decision shadow の候補 policy の版（Phase 2 の enforce と同じ kernel）。
pub const DECISION_SHADOW_POLICY_VERSION: &str = "dispatch-enforce-heuristic-v1";

/// 候補 1 行の比較（設定順）。`excluded_reasons` が空なら候補 policy にとって適格。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionShadowCandidate {
    pub deployment_id: String,
    /// 容量・account の identity（設定行の id）。複数役割の候補は `deployment_id` がモデルごとに分かれる。
    #[serde(default)]
    pub provider_id: String,
    pub model_profile_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_reasons: Vec<String>,
    /// 候補 policy がこの行を選んだ。
    #[serde(default)]
    pub selected: bool,
    /// primary（legacy）がこの行を選んだ。
    #[serde(default)]
    pub primary: bool,
}

/// primary と候補 policy の判断の比較（`ShadowRecord.detail` に JSON で入る）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionShadowComparison {
    pub version: u32,
    pub primary_mode: RoutingMode,
    pub candidate_policy: String,
    pub requested_lane: Tier,
    pub primary_source: String,
    pub primary_model: String,
    pub primary_lane: Tier,
    /// 候補 policy が選んだ source（全候補が除外なら `None` = defer）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_model: Option<String>,
    /// 候補 policy は lane を下げない（requested のまま）。defer なら `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_lane: Option<Tier>,
    /// source・lane とも一致。
    pub agrees: bool,
    /// 差の種類（`source` / `model` / `lane` / `deferred`）。一致なら空。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub differences: Vec<String>,
    pub candidates: Vec<DecisionShadowCandidate>,
}

/// `[model_routing.shadow]` の差し替えを受け取る側（daemon が llm-proxy の shadow queue を繋ぐ）。
/// `mode` は同じ時点の `DispatchRoutingSettings.mode`。呼び出し側を待たせない実装にする。
pub trait RoutingShadowListener: Send + Sync {
    fn reload(&self, mode: RoutingMode, policy: &ShadowPolicy);
}

impl Dispatcher {

    /// 検証済みの shadow policy を差し替え、listener へ同じ値を渡す。`set_dispatch_routing` の後に呼ぶ
    /// （listener は新しい mode と policy の組を 1 回で受け取る）。
    pub fn set_routing_shadow(&mut self, policy: ShadowPolicy) {
        self.routing_shadow_policy = policy;
        for listener in &self.routing_shadow_listeners {
            listener.reload(self.dispatch_routing.mode, &self.routing_shadow_policy);
        }
    }

    pub fn routing_shadow_policy(&self) -> &ShadowPolicy {
        &self.routing_shadow_policy
    }

    /// listener を足す。足した時点の mode と policy をすぐ 1 回渡す。
    pub fn add_routing_shadow_listener(&mut self, listener: Arc<dyn RoutingShadowListener>) {
        listener.reload(self.dispatch_routing.mode, &self.routing_shadow_policy);
        self.routing_shadow_listeners.push(listener);
    }

}
