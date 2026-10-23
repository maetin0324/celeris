//! Pure harness selection and LLM source/model routing boundary (ADR 2026-10-11).
//! This module does not start processes, consult mutable state, or infer from task prose.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(
            Debug,
            Clone,
            Default,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            Serialize,
            Deserialize,
            schemars::JsonSchema,
        )]
        #[serde(transparent)]
        pub struct $name(pub String);
        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }
        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }
        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}
id_type!(HarnessId);
id_type!(SourceId);
id_type!(ModelId);
id_type!(ApiFormat);
id_type!(SourceKind);
id_type!(ExecutionClass);
id_type!(AccountId);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceSpec {
    pub kind: SourceKind,
    pub api_format: ApiFormat,
    pub accounts: Vec<AccountId>,
    pub models: Vec<ModelId>,
    pub max_runs: Option<u32>,
    pub account_max_runs: BTreeMap<AccountId, u32>,
    pub quota: Option<f64>,
    /// Base URL of the source's API, when it has one (OAuth CLI sources do not).
    #[serde(default)]
    pub endpoint: Option<String>,
    /// Secret reference id for the source's credential; never the secret value.
    #[serde(default)]
    pub credential_ref: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessSpec {
    pub kind: String,
    pub api_formats: BTreeSet<ApiFormat>,
    pub source_kinds: BTreeSet<SourceKind>,
    pub capabilities: BTreeSet<String>,
    pub command: Option<String>,
    pub args: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CandidateKey {
    pub source: SourceId,
    pub model: ModelId,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmRoute {
    pub harness: HarnessId,
    pub key: CandidateKey,
    pub via: Option<String>,
    pub from_api: Option<ApiFormat>,
    pub to_api: Option<ApiFormat>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessRule {
    pub genre: Option<String>,
    pub role: Option<String>,
    pub required_capabilities: BTreeSet<String>,
    pub requires_repo: Option<bool>,
    pub execution_class: Option<ExecutionClass>,
    pub harness: HarnessId,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessSelection {
    pub rules: Vec<HarnessRule>,
    pub execution_class_defaults: BTreeMap<ExecutionClass, Vec<HarnessId>>,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Registry {
    pub sources: BTreeMap<SourceId, SourceSpec>,
    pub harnesses: BTreeMap<HarnessId, HarnessSpec>,
    pub llm_routes: Vec<LlmRoute>,
    pub route_defaults: BTreeMap<HarnessId, Vec<CandidateKey>>,
    /// Lane-specific human priorities override the harness-wide priority when present.
    pub lane_route_defaults: BTreeMap<(HarnessId, String), Vec<CandidateKey>>,
    pub route_selection: RouteSelectionSettings,
    /// Per-harness, per-execution-class candidate lists. A class list replaces the harness
    /// default for that class; it never falls back to `route_defaults`.
    #[serde(default)]
    pub route_class_defaults: BTreeMap<HarnessId, BTreeMap<ExecutionClass, Vec<CandidateKey>>>,
    pub cos_route_candidates: Vec<CandidateKey>,
    pub harness_selection: HarnessSelection,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RouteSelectionSettings {
    pub exploration_epsilon: f64,
    pub guardrail_window: u64,
    pub guardrail_quota_floor: f64,
    pub evaluation_reservations: BTreeMap<SourceId, u32>,
    /// harness の試行順を残量で並べ替える実行クラス（ADR 2026-10-22 D1）。空 = 全クラス宣言順。
    pub harness_balance_classes: BTreeSet<String>,
    /// 最大スコアからこの幅以内の harness は同等（宣言順を優先）。
    pub harness_balance_tolerance: f64,
    /// guardrail: 切替した完了単位がこの件数以上で判定する（ADR D2）。
    pub harness_balance_guard_min: u32,
    /// guardrail: 切替した単位の悪い率が、しなかった単位より、この幅を超えて高いと止める。
    pub harness_balance_guard_margin: f64,
}
impl Default for RouteSelectionSettings {
    fn default() -> Self {
        Self {
            exploration_epsilon: 0.1,
            guardrail_window: 50,
            guardrail_quota_floor: 0.1,
            evaluation_reservations: BTreeMap::new(),
            harness_balance_classes: BTreeSet::new(),
            harness_balance_tolerance: 0.15,
            harness_balance_guard_min: 10,
            harness_balance_guard_margin: 0.15,
        }
    }
}
impl Registry {
    pub fn route_defaults_for(&self, harness: &HarnessId, lane: &str) -> &[CandidateKey] {
        self.lane_route_defaults
            .get(&(harness.clone(), lane.to_owned()))
            .or_else(|| self.route_defaults.get(harness))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

/// Model name sent over the harness wire. Only explicitly proxied routes need a
/// source namespace; direct connections keep the canonical model id.
pub fn route_via<'a>(
    registry: &'a Registry,
    harness: &HarnessId,
    key: &CandidateKey,
) -> Option<&'a str> {
    registry
        .llm_routes
        .iter()
        .find(|route| route.harness == *harness && route.key == *key)
        .and_then(|route| route.via.as_deref())
}

pub fn wire_model_id(registry: &Registry, harness: &HarnessId, key: &CandidateKey) -> ModelId {
    if route_via(registry, harness, key) == Some("llm_proxy") {
        format!("{}/{}", key.source, key.model).into()
    } else {
        key.model.clone()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HarnessRequest {
    pub harness: Option<HarnessId>,
    pub genre: Option<String>,
    pub role: Option<String>,
    pub required_capabilities: BTreeSet<String>,
    pub has_repo: bool,
    pub execution_class: Option<ExecutionClass>,
}
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HarnessSelectionError {
    #[error("unknown explicitly selected harness: {0}")]
    Unknown(HarnessId),
    #[error("multiple harnesses match the same selection precedence: {0:?}")]
    Ambiguous(Vec<HarnessId>),
    #[error("no eligible harness is declared")]
    NoneEligible,
}
/// Select by explicit task field, then matching genre/role rules, then execution-class defaults.
pub fn select_harness(
    req: &HarnessRequest,
    registry: &Registry,
) -> Result<HarnessId, HarnessSelectionError> {
    if let Some(id) = &req.harness {
        return registry
            .harnesses
            .contains_key(id)
            .then(|| id.clone())
            .ok_or_else(|| HarnessSelectionError::Unknown(id.clone()));
    }
    let matches: Vec<_> = registry
        .harness_selection
        .rules
        .iter()
        .filter(|r| {
            r.genre
                .as_ref()
                .is_none_or(|v| req.genre.as_ref() == Some(v))
                && r.role.as_ref().is_none_or(|v| req.role.as_ref() == Some(v))
                && r.execution_class
                    .as_ref()
                    .is_none_or(|v| req.execution_class.as_ref() == Some(v))
                && r.requires_repo.is_none_or(|v| req.has_repo == v)
                && r.required_capabilities
                    .is_subset(&req.required_capabilities)
                && registry
                    .harnesses
                    .get(&r.harness)
                    .is_some_and(|h| req.required_capabilities.is_subset(&h.capabilities))
        })
        .collect();
    if let Some(best) = matches.iter().map(|r| specificity(r)).max() {
        let ids: BTreeSet<_> = matches
            .into_iter()
            .filter(|r| specificity(r) == best)
            .map(|r| r.harness.clone())
            .collect();
        return unique_harness(ids);
    }
    if let Some(class) = &req.execution_class
        && let Some(ids) = registry
            .harness_selection
            .execution_class_defaults
            .get(class)
    {
        return unique_harness(
            ids.iter()
                .filter(|id| registry.harnesses.contains_key(*id))
                .cloned()
                .collect(),
        );
    }
    Err(HarnessSelectionError::NoneEligible)
}

/// Return harnesses in admission priority order. A declared execution-class list takes
/// precedence over explicit task harnesses and genre/role rules; unknown registry entries
/// are ignored while preserving the configured order.
pub fn harness_candidates(
    req: &HarnessRequest,
    registry: &Registry,
) -> Result<Vec<HarnessId>, HarnessSelectionError> {
    if let Some(class) = &req.execution_class
        && let Some(ids) = registry
            .harness_selection
            .execution_class_defaults
            .get(class)
    {
        return Ok(ids
            .iter()
            .filter(|id| registry.harnesses.contains_key(*id))
            .cloned()
            .collect());
    }
    select_harness(req, registry).map(|id| vec![id])
}
fn specificity(r: &HarnessRule) -> usize {
    usize::from(r.genre.is_some())
        + usize::from(r.role.is_some())
        + r.required_capabilities.len()
        + usize::from(r.requires_repo.is_some())
        + usize::from(r.execution_class.is_some())
}
fn unique_harness(ids: BTreeSet<HarnessId>) -> Result<HarnessId, HarnessSelectionError> {
    if ids.len() > 1 {
        return Err(HarnessSelectionError::Ambiguous(ids.into_iter().collect()));
    }
    ids.into_iter()
        .next()
        .ok_or(HarnessSelectionError::NoneEligible)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteRequest {
    pub task_id: String,
    pub harness: HarnessId,
    pub model_hint: Option<ModelId>,
    pub lane: Option<String>,
    pub execution_class: Option<ExecutionClass>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Candidate {
    pub source: SourceId,
    pub model: ModelId,
    pub api_format: ApiFormat,
}
impl Candidate {
    pub fn key(&self) -> CandidateKey {
        CandidateKey {
            source: self.source.clone(),
            model: self.model.clone(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum ExclusionReason {
    UnknownSource,
    ModelNotDeclared,
    ModelHintMismatch,
    NoCompatibleRoute,
    AccountUnavailable,
    SourceQuota,
    AccountQuota,
    Cooldown,
    SourceConcurrency,
    AccountConcurrency,
    NoDeclaredDefault,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateRejection {
    pub candidate: Candidate,
    pub reason: ExclusionReason,
}
pub fn enumerate_candidates(req: &RouteRequest, registry: &Registry) -> Vec<Candidate> {
    registry
        .sources
        .iter()
        .flat_map(|(sid, s)| {
            s.models
                .iter()
                .filter(|m| req.model_hint.as_ref().is_none_or(|hint| hint == *m))
                .map(|m| Candidate {
                    source: sid.clone(),
                    model: m.clone(),
                    api_format: s.api_format.clone(),
                })
        })
        .collect()
}
pub fn constrain_for_harness(
    candidates: Vec<Candidate>,
    harness: &HarnessSpec,
    registry: &Registry,
    harness_id: &HarnessId,
) -> (Vec<Candidate>, Vec<CandidateRejection>) {
    let mut eligible = Vec::new();
    let mut rejected = Vec::new();
    for c in candidates {
        let source = registry.sources.get(&c.source);
        let compatible = source.is_some_and(|s| {
            harness.source_kinds.contains(&s.kind)
                && (harness.api_formats.contains(&s.api_format)
                    || registry.llm_routes.iter().any(|route| {
                        route.harness == *harness_id
                            && route.key == c.key()
                            && route.from_api.as_ref() == Some(&s.api_format)
                            && route
                                .to_api
                                .as_ref()
                                .is_some_and(|format| harness.api_formats.contains(format))
                    }))
        });
        if compatible {
            eligible.push(c);
        } else {
            rejected.push(CandidateRejection {
                candidate: c,
                reason: ExclusionReason::NoCompatibleRoute,
            });
        }
    }
    (eligible, rejected)
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountSnapshot {
    pub available: BTreeMap<SourceId, Vec<AccountId>>,
    pub account_in_use: BTreeMap<(SourceId, AccountId), u32>,
    pub source_in_use: BTreeMap<SourceId, u32>,
}
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QuotaSnapshot {
    /// source の残量（最も余裕のある account の、最も逼迫した窓の残り割合 0..=1。ADR 2026-10-22 D1）。
    pub source_remaining: BTreeMap<SourceId, f64>,
    /// source の残量スコア（最良 account の ADR-0024 D3 スコア `min(h_5h, h_7d, h_1m)`。除外は 0）。
    /// account プールを持たない source は欠ける（不明）。
    pub source_headroom: BTreeMap<SourceId, f64>,
    pub account_remaining: BTreeMap<(SourceId, AccountId), f64>,
    pub cooling_down: BTreeSet<(SourceId, AccountId)>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReservationSnapshot {
    pub source_in_use: BTreeMap<SourceId, u32>,
    pub account_in_use: BTreeMap<(SourceId, AccountId), u32>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationOwner {
    pub source_id: SourceId,
    pub account_id: AccountId,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    Admitted(ReservationOwner),
    Rejected(ExclusionReason),
}
pub fn admit_source(
    candidate: &Candidate,
    accounts: &AccountSnapshot,
    quota: &QuotaSnapshot,
    reservations: &ReservationSnapshot,
    registry: &Registry,
) -> Admission {
    let Some(src) = registry.sources.get(&candidate.source) else {
        return Admission::Rejected(ExclusionReason::UnknownSource);
    };
    let Some(available) = accounts.available.get(&candidate.source) else {
        return Admission::Rejected(ExclusionReason::AccountUnavailable);
    };
    let mut reasons = Vec::new();
    for account in available {
        let key = (candidate.source.clone(), account.clone());
        if quota.cooling_down.contains(&key) {
            reasons.push(ExclusionReason::Cooldown);
            continue;
        }
        if quota
            .source_remaining
            .get(&candidate.source)
            .is_some_and(|v| *v <= 0.0)
        {
            reasons.push(ExclusionReason::SourceQuota);
            continue;
        }
        if quota.account_remaining.get(&key).is_some_and(|v| *v <= 0.0) {
            reasons.push(ExclusionReason::AccountQuota);
            continue;
        }
        if src.max_runs.is_some_and(|max| {
            reservations
                .source_in_use
                .get(&candidate.source)
                .copied()
                .unwrap_or(0)
                >= max
        }) {
            reasons.push(ExclusionReason::SourceConcurrency);
            continue;
        }
        if src
            .account_max_runs
            .get(account)
            .is_some_and(|max| reservations.account_in_use.get(&key).copied().unwrap_or(0) >= *max)
        {
            reasons.push(ExclusionReason::AccountConcurrency);
            continue;
        }
        return Admission::Admitted(ReservationOwner {
            source_id: candidate.source.clone(),
            account_id: account.clone(),
        });
    }
    Admission::Rejected(
        reasons
            .into_iter()
            .next()
            .unwrap_or(ExclusionReason::AccountUnavailable),
    )
}
/// The one default priority path (ADR 2026-10-11-llm-source-model-routing-evaluation D2):
/// CoS chat uses `cos.route_candidates`. Otherwise a declared `route_class_defaults` list for the
/// execution class replaces every other column (ADR 2026-10-11-llm-source-harness-selection 付記
/// 2026-10-13 §5: no fallback from a class list); without one, the harness×lane `route_defaults`
/// column, falling back to the harness-wide column. There is no lexicographic fallback.
pub fn route_priority<'a>(
    registry: &'a Registry,
    harness: &HarnessId,
    lane: Option<&str>,
    cos: bool,
    execution_class: Option<&ExecutionClass>,
) -> &'a [CandidateKey] {
    if cos {
        return &registry.cos_route_candidates;
    }
    if let Some(class) = execution_class
        && let Some(list) = registry
            .route_class_defaults
            .get(harness)
            .and_then(|classes| classes.get(class))
    {
        return list.as_slice();
    }
    registry.route_defaults_for(harness, lane.unwrap_or_default())
}

/// What the dispatcher persists on the run row once a route is admitted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSelection {
    pub harness: HarnessId,
    pub source: SourceId,
    pub model: ModelId,
    pub account: AccountId,
    /// Versioned selection evidence (`{"version": 1, ...}`).
    pub metadata: serde_json::Value,
}

#[cfg(test)]
#[path = "llm_route/tests.rs"]
mod tests;
