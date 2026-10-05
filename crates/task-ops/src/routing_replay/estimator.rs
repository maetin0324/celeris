//! Phase 5: estimator shadow comparison and the RouteLLM strong/weak pair adapter.
//!
//! Inputs are the frozen dataset rows and a pinned descriptor/pair supplied by the caller. Nothing
//! here opens the database or calls a sidecar. A RouteLLM score is a relative win-rate of one
//! pinned strong/weak pair: it is written only to those two models, never transferred to other
//! models, and stays `index = None` (unknown) until a calibration version is pinned.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use task_core::model_router::estimator::sidecar::{EstimateResponseV1, EstimatorDescriptor};

use super::{
    DatasetRowV1, ReplayError, ShadowV1, finite, heuristic_choice, mean, percentile, ratio,
};

/// Reason codes that may be copied from an estimator shadow's `detail`. Anything else is dropped.
const DETAIL_CODES: &[&str] = &[
    "prompt_required",
    "uncalibrated",
    "outside_pinned_pair",
    "protocol_error",
    "unreachable",
    "circuit_open",
    "privacy",
    "external_dependency",
];

/// Reason prefix by which a pair wrapper reports the raw strong-win score as metadata.
const RAW_PAIR_SCORE_PREFIXES: &[&str] = &["raw_pair_score=", "raw_pair_score:"];

pub(super) fn detail_code(detail: &str) -> Option<String> {
    let token = detail
        .split(|c: char| c == ':' || c == '=' || c.is_whitespace())
        .next()?;
    DETAIL_CODES.contains(&token).then(|| token.to_owned())
}

/// The pinned RouteLLM pair. `calibration_version` is `None` until the pair score has been
/// calibrated to the Celeris quality scale; until then every pair score is unknown quality.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteLlmPairV1 {
    pub router: String,
    pub strong_model: String,
    pub weak_model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration_version: Option<String>,
}

impl RouteLlmPairV1 {
    fn validate(&self) -> Result<(), ReplayError> {
        if self.router.is_empty() || self.strong_model.is_empty() || self.weak_model.is_empty() {
            return Err(ReplayError::Invalid(
                "RouteLLM pair fields must not be empty",
            ));
        }
        if self.strong_model == self.weak_model {
            return Err(ReplayError::Invalid(
                "RouteLLM strong and weak model must differ",
            ));
        }
        Ok(())
    }

    fn contains(&self, model: &str) -> bool {
        model == self.strong_model || model == self.weak_model
    }

    fn calibrated(&self) -> bool {
        self.calibration_version.is_some()
    }
}

/// Pinned estimator identity for the comparison. Saved verbatim in the report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EstimatorComparisonInputV1 {
    pub descriptor: EstimatorDescriptor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pair: Option<RouteLlmPairV1>,
}

/// One candidate's estimate after the pair adapter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairEstimateV1 {
    pub model_profile_id: String,
    /// Raw strong-win score of the pinned pair (evaluation metadata only).
    pub raw_pair_score: Option<f64>,
    /// Celeris quality index. `None` unless the pair is calibrated and the model is in the pair.
    pub index: Option<f64>,
    /// `calibrated`, `uncalibrated`, `outside_pinned_pair` or `pair_score_missing`.
    pub status: String,
}

fn raw_pair_score(response: &EstimateResponseV1, model: &str) -> Option<f64> {
    response
        .estimates
        .iter()
        .filter(|e| e.model_profile_id == model)
        .flat_map(|e| &e.reasons)
        .find_map(|r| {
            RAW_PAIR_SCORE_PREFIXES
                .iter()
                .find_map(|p| r.strip_prefix(p))
                .and_then(|v| v.trim().parse::<f64>().ok())
        })
        .filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
}

/// Maps a RouteLLM `/estimate` response onto the candidates. Only the pinned strong/weak models
/// receive the pair score; other candidates stay unknown even if the sidecar returned an index.
pub fn routellm_pair_estimates(
    descriptor: &EstimatorDescriptor,
    pair: &RouteLlmPairV1,
    response: &EstimateResponseV1,
    candidates: &[String],
) -> Result<Vec<PairEstimateV1>, ReplayError> {
    descriptor
        .validate()
        .map_err(|_| ReplayError::Invalid("estimator descriptor invalid"))?;
    pair.validate()?;
    if response.estimator_id != descriptor.estimator_id || response.version != descriptor.version {
        return Err(ReplayError::Invalid(
            "estimate response does not match the pinned descriptor",
        ));
    }
    // The score is the strong model's win-rate; the weak side may echo it.
    let raw = raw_pair_score(response, &pair.strong_model)
        .or_else(|| raw_pair_score(response, &pair.weak_model));
    let mut models: BTreeSet<&str> = candidates.iter().map(String::as_str).collect();
    models.insert(&pair.strong_model);
    models.insert(&pair.weak_model);
    Ok(models
        .into_iter()
        .map(|model| {
            if !pair.contains(model) {
                return PairEstimateV1 {
                    model_profile_id: model.to_owned(),
                    raw_pair_score: None,
                    index: None,
                    status: "outside_pinned_pair".into(),
                };
            }
            let Some(raw) = raw else {
                return PairEstimateV1 {
                    model_profile_id: model.to_owned(),
                    raw_pair_score: None,
                    index: None,
                    status: "pair_score_missing".into(),
                };
            };
            let relative = if model == pair.strong_model {
                raw
            } else {
                1.0 - raw
            };
            PairEstimateV1 {
                model_profile_id: model.to_owned(),
                raw_pair_score: finite(Some(raw)),
                index: pair.calibrated().then(|| finite(Some(relative))).flatten(),
                status: if pair.calibrated() {
                    "calibrated"
                } else {
                    "uncalibrated"
                }
                .into(),
            }
        })
        .collect())
}

/// Estimator shadow vs the heuristic primary. Counts are over decisions, not shadow records.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EstimatorComparisonV1 {
    /// The pinned descriptor and pair, as supplied to the evaluation.
    pub descriptor: Option<EstimatorDescriptor>,
    pub pair: Option<RouteLlmPairV1>,
    pub calibrated: bool,
    pub observed_estimator_versions: Vec<String>,
    /// Decisions the estimator should have scored (with a pinned pair: both pair models offered).
    pub target_decisions: usize,
    /// Target decisions with a completed shadow of the pinned estimator.
    pub evaluated: usize,
    pub coverage: f64,
    pub completed: usize,
    pub failed: usize,
    pub timeout: usize,
    pub dropped: usize,
    pub prompt_required: usize,
    pub overhead_ms_mean: Option<f64>,
    pub overhead_ms_p50: Option<u64>,
    pub overhead_ms_p95: Option<u64>,
    pub same_as_heuristic: usize,
    pub differs_from_heuristic: usize,
    pub heuristic_unavailable: usize,
    pub same_as_primary: usize,
    /// Estimator choices whose acceptance outcome is observed and countable.
    pub quality_observed: usize,
    pub quality_unknown: usize,
    pub acceptance_success_rate: Option<f64>,
    pub incomparable_reasons: BTreeMap<String, usize>,
    pub unknown_reason: String,
}

fn version_matches(observed: &str, d: &EstimatorDescriptor) -> bool {
    let id = &d.estimator_id;
    let v = &d.version;
    [
        v.clone(),
        format!("{id}@{v}"),
        format!("{id}:{v}"),
        format!("{id}/{v}"),
    ]
    .iter()
    .any(|s| s == observed)
}

fn is_prompt_required(s: &ShadowV1) -> bool {
    s.reason.as_deref() == Some("prompt_required")
        || s.detail_code.as_deref() == Some("prompt_required")
}

pub(super) fn compare(
    rows: &[DatasetRowV1],
    input: Option<&EstimatorComparisonInputV1>,
) -> Result<Option<EstimatorComparisonV1>, ReplayError> {
    let has_estimator_shadow = rows
        .iter()
        .any(|r| r.shadows.iter().any(|s| s.kind == "estimator"));
    if input.is_none() && !has_estimator_shadow {
        return Ok(None);
    }
    if let Some(input) = input {
        input
            .descriptor
            .validate()
            .map_err(|_| ReplayError::Invalid("estimator descriptor invalid"))?;
        if let Some(pair) = &input.pair {
            pair.validate()?;
        }
    }
    let descriptor = input.map(|i| &i.descriptor);
    let pair = input.and_then(|i| i.pair.as_ref());
    // Without a pair the sidecar returns indices on the Celeris scale (protocol v1).
    let calibrated = pair.is_none_or(RouteLlmPairV1::calibrated);
    let mut c = EstimatorComparisonV1 {
        descriptor: descriptor.cloned(),
        pair: pair.cloned(),
        calibrated,
        observed_estimator_versions: Vec::new(),
        target_decisions: 0,
        evaluated: 0,
        coverage: 0.0,
        completed: 0,
        failed: 0,
        timeout: 0,
        dropped: 0,
        prompt_required: 0,
        overhead_ms_mean: None,
        overhead_ms_p50: None,
        overhead_ms_p95: None,
        same_as_heuristic: 0,
        differs_from_heuristic: 0,
        heuristic_unavailable: 0,
        same_as_primary: 0,
        quality_observed: 0,
        quality_unknown: 0,
        acceptance_success_rate: None,
        incomparable_reasons: BTreeMap::new(),
        unknown_reason: "The estimator never selects; an estimator choice other than the observed \
                         primary has no outcome, and an uncalibrated pair score is unknown quality"
            .into(),
    };
    let mut versions = BTreeSet::new();
    let mut overhead = Vec::new();
    let mut passed = 0;
    let mut incomparable = |reason: &str| {
        *c.incomparable_reasons.entry(reason.to_owned()).or_default() += 1;
    };
    for row in rows {
        let shadows: Vec<&ShadowV1> = row
            .shadows
            .iter()
            .filter(|s| s.kind == "estimator")
            .collect();
        for s in &shadows {
            versions.insert(s.policy_version.clone());
        }
        let offers_pair = pair.is_none_or(|p| {
            [&p.strong_model, &p.weak_model]
                .iter()
                .all(|m| row.candidates.iter().any(|c| &&c.model == m))
        });
        if !offers_pair {
            if !shadows.is_empty() {
                incomparable("pair_not_in_candidates");
            }
            continue;
        }
        c.target_decisions += 1;
        let pinned: Vec<&ShadowV1> = shadows
            .iter()
            .copied()
            .filter(|s| descriptor.is_none_or(|d| version_matches(&s.policy_version, d)))
            .collect();
        if pinned.is_empty() {
            incomparable(if shadows.is_empty() {
                "not_evaluated"
            } else {
                "estimator_version_mismatch"
            });
            continue;
        }
        for s in &pinned {
            if let Some(ms) = s.latency_ms {
                overhead.push(ms);
            }
        }
        if pinned.iter().any(|s| is_prompt_required(s)) {
            c.prompt_required += 1;
        }
        if pinned
            .iter()
            .any(|s| s.reason.as_deref() == Some("timeout"))
        {
            c.timeout += 1;
        }
        let Some(done) = pinned.iter().find(|s| s.status == "completed") else {
            if pinned.iter().any(|s| s.status == "dropped") {
                c.dropped += 1;
            } else {
                c.failed += 1;
            }
            let reason = pinned
                .iter()
                .find_map(|s| {
                    is_prompt_required(s)
                        .then_some("prompt_required".to_owned())
                        .or_else(|| s.reason.clone())
                })
                .unwrap_or_else(|| "estimator_failed".into());
            incomparable(&reason);
            continue;
        };
        c.completed += 1;
        c.evaluated += 1;
        let Some(model) = done.model.as_deref() else {
            incomparable("no_estimator_choice");
            continue;
        };
        if pair.is_some_and(|p| !p.contains(model)) {
            incomparable("outside_pinned_pair");
            continue;
        }
        match heuristic_choice(row) {
            None => c.heuristic_unavailable += 1,
            Some(h) if h.model == model => c.same_as_heuristic += 1,
            Some(_) => c.differs_from_heuristic += 1,
        }
        if row.primary_model.as_deref() != Some(model) {
            c.quality_unknown += 1;
            incomparable("unselected_model_outcome_unknown");
            continue;
        }
        c.same_as_primary += 1;
        if !calibrated {
            c.quality_unknown += 1;
            incomparable("uncalibrated_raw_score");
            continue;
        }
        match row.acceptance_passed.or(row.review_passed) {
            Some(ok) => {
                c.quality_observed += 1;
                if ok {
                    passed += 1;
                }
            }
            None => {
                c.quality_unknown += 1;
                incomparable("outcome_missing");
            }
        }
    }
    c.coverage = ratio(c.evaluated, c.target_decisions);
    c.acceptance_success_rate = (c.quality_observed > 0).then(|| ratio(passed, c.quality_observed));
    c.observed_estimator_versions = versions.into_iter().collect();
    let as_f64: Vec<f64> = overhead.iter().map(|&ms| ms as f64).collect();
    c.overhead_ms_mean = mean(&as_f64);
    c.overhead_ms_p50 = percentile(&overhead, 50);
    c.overhead_ms_p95 = percentile(&overhead, 95);
    Ok(Some(c))
}

#[cfg(test)]
mod tests;
