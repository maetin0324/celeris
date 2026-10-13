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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}
impl Default for RouteSelectionSettings {
    fn default() -> Self {
        Self {
            exploration_epsilon: 0.1,
            guardrail_window: 50,
            guardrail_quota_floor: 0.1,
            evaluation_reservations: BTreeMap::new(),
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    pub source_remaining: BTreeMap<SourceId, f64>,
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
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("no eligible candidate in declared route priority")]
pub struct NoEligibleCandidate;
pub trait RoutePolicy {
    fn choose(&self, eligible: &[Candidate]) -> Result<Candidate, NoEligibleCandidate>;
}
#[derive(Debug, Clone, Default)]
pub struct DefaultRoutePolicy {
    pub priority: Vec<CandidateKey>,
}
impl RoutePolicy for DefaultRoutePolicy {
    fn choose(&self, eligible: &[Candidate]) -> Result<Candidate, NoEligibleCandidate> {
        default_route_policy(eligible, &self.priority)
    }
}
pub fn default_route_policy(
    eligible: &[Candidate],
    priority: &[CandidateKey],
) -> Result<Candidate, NoEligibleCandidate> {
    priority
        .iter()
        .find_map(|key| eligible.iter().find(|c| c.key() == *key).cloned())
        .ok_or(NoEligibleCandidate)
}
pub fn route_priority<'a>(
    registry: &'a Registry,
    harness: &HarnessId,
    cos: bool,
) -> &'a [CandidateKey] {
    if cos {
        &registry.cos_route_candidates
    } else {
        registry
            .route_defaults
            .get(harness)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
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
mod tests {
    use super::*;
    fn harness(id: &str) -> HarnessId {
        id.into()
    }
    fn fixture() -> Registry {
        let mut r = Registry::default();
        r.harnesses.insert(
            harness("h"),
            HarnessSpec {
                kind: "cli".into(),
                api_formats: [ApiFormat::from("openai")].into_iter().collect(),
                source_kinds: [SourceKind::from("openai")].into_iter().collect(),
                capabilities: BTreeSet::new(),
                command: None,
                args: vec![],
            },
        );
        r.sources.insert(
            "s".into(),
            SourceSpec {
                kind: "openai".into(),
                api_format: "openai".into(),
                accounts: vec!["a".into()],
                models: vec!["m".into()],
                max_runs: Some(2),
                account_max_runs: BTreeMap::from([("a".into(), 1)]),
                quota: None,
                endpoint: None,
                credential_ref: None,
            },
        );
        r
    }
    fn candidate() -> Candidate {
        Candidate {
            source: "s".into(),
            model: "m".into(),
            api_format: "openai".into(),
        }
    }
    #[test]
    fn harness_select_is_deterministic_and_rejects_same_precedence_duplicates() {
        let mut r = fixture();
        r.harnesses
            .insert("h2".into(), r.harnesses[&harness("h")].clone());
        r.harness_selection.rules = vec![
            HarnessRule {
                genre: Some("g".into()),
                harness: "h".into(),
                ..Default::default()
            },
            HarnessRule {
                genre: Some("g".into()),
                harness: "h2".into(),
                ..Default::default()
            },
        ];
        let req = HarnessRequest {
            genre: Some("g".into()),
            ..Default::default()
        };
        assert!(matches!(
            select_harness(&req, &r),
            Err(HarnessSelectionError::Ambiguous(_))
        ));
        r.harness_selection.rules.pop();
        assert_eq!(select_harness(&req, &r), Ok(harness("h")));
        assert_eq!(select_harness(&req, &r), Ok(harness("h")));
    }
    #[test]
    fn route_select_filters_explicit_model_and_lane_is_ignored() {
        let r = fixture();
        let a = enumerate_candidates(
            &RouteRequest {
                task_id: "t".into(),
                harness: "h".into(),
                model_hint: Some("missing".into()),
                lane: Some("cheap".into()),
                execution_class: None,
            },
            &r,
        );
        assert!(a.is_empty());
    }
    #[test]
    fn route_select_excludes_incompatible_harness() {
        let mut r = fixture();
        let mut h = r.harnesses[&harness("h")].clone();
        h.source_kinds.clear();
        let (_, rejected) = constrain_for_harness(vec![candidate()], &h, &r, &harness("h"));
        assert_eq!(rejected[0].reason, ExclusionReason::NoCompatibleRoute);
        r.llm_routes.clear();
    }
    #[test]
    fn route_select_excludes_quota_cooldown_and_concurrency() {
        let r = fixture();
        let c = candidate();
        let mut q = QuotaSnapshot::default();
        let ac = AccountSnapshot {
            available: BTreeMap::from([("s".into(), vec!["a".into()])]),
            ..Default::default()
        };
        q.cooling_down.insert(("s".into(), "a".into()));
        assert_eq!(
            admit_source(&c, &ac, &q, &ReservationSnapshot::default(), &r),
            Admission::Rejected(ExclusionReason::Cooldown)
        );
        q.cooling_down.clear();
        let rs = ReservationSnapshot {
            account_in_use: BTreeMap::from([(("s".into(), "a".into()), 1)]),
            ..Default::default()
        };
        assert_eq!(
            admit_source(&c, &ac, &q, &rs, &r),
            Admission::Rejected(ExclusionReason::AccountConcurrency)
        );
        let source_busy = ReservationSnapshot {
            source_in_use: BTreeMap::from([("s".into(), 2)]),
            ..Default::default()
        };
        assert_eq!(
            admit_source(&c, &ac, &QuotaSnapshot::default(), &source_busy, &r),
            Admission::Rejected(ExclusionReason::SourceConcurrency)
        );
        let account_empty = QuotaSnapshot {
            account_remaining: BTreeMap::from([(("s".into(), "a".into()), 0.0)]),
            ..Default::default()
        };
        assert_eq!(
            admit_source(&c, &ac, &account_empty, &ReservationSnapshot::default(), &r),
            Admission::Rejected(ExclusionReason::AccountQuota)
        );
        q.source_remaining.insert("s".into(), 0.0);
        assert_eq!(
            admit_source(&c, &ac, &q, &ReservationSnapshot::default(), &r),
            Admission::Rejected(ExclusionReason::SourceQuota)
        );
    }
    #[test]
    fn route_select_default_fails_closed_and_cos_uses_declared_fallback() {
        let c = candidate();
        assert_eq!(
            default_route_policy(std::slice::from_ref(&c), &[]),
            Err(NoEligibleCandidate)
        );
        let fallback = CandidateKey {
            source: "backup".into(),
            model: "m2".into(),
        };
        let eligible = Candidate {
            source: "backup".into(),
            model: "m2".into(),
            api_format: "openai".into(),
        };
        assert_eq!(
            default_route_policy(&[c, eligible.clone()], &[fallback]),
            Ok(eligible)
        );
    }
}
