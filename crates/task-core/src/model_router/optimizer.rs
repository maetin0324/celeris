use super::{
    context::RoutingContext,
    estimator::QualityEstimator,
    policy::{Objective, RoutingMode, RoutingPolicy},
    profiles::{DeploymentProfile, ModelProfile, Reachability, SourceState, Support},
    trace::CandidateTrace,
};
use std::collections::{HashMap, HashSet};

pub struct Candidate<'a> {
    pub model: &'a ModelProfile,
    pub deployment: &'a DeploymentProfile,
    pub state: Option<&'a SourceState>,
    pub eligible_provider_ids: Vec<String>,
    pub cost_usd: Option<f64>,
    pub latency_ms: Option<f64>,
    pub pressure: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RankedCandidate {
    pub model_profile_id: String,
    pub deployment_id: String,
    pub eligible_provider_ids: Vec<String>,
    pub score: Option<f64>,
    pub config_order: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OptimizationResult {
    pub ranked: Vec<RankedCandidate>,
    pub allowlist: Vec<String>,
    pub traces: Vec<CandidateTrace>,
    pub outcome: &'static str,
}

fn finite_nonnegative(v: Option<f64>) -> bool {
    v.is_none_or(|x| x.is_finite() && x >= 0.0)
}
fn quantize(v: f64) -> i64 {
    (v * 1_000_000.0).round() as i64
}

/// Caller supplied order is the existing selector's order. No estimator or score runs in legacy.
pub fn legacy_rank(candidates: &[Candidate<'_>]) -> OptimizationResult {
    let ranked: Vec<_> = candidates
        .iter()
        .map(|c| RankedCandidate {
            model_profile_id: c.model.id.clone(),
            deployment_id: c.deployment.id.clone(),
            eligible_provider_ids: c.eligible_provider_ids.clone(),
            score: None,
            config_order: c.deployment.config_order,
        })
        .collect();
    let allowlist = ranked
        .iter()
        .flat_map(|c| c.eligible_provider_ids.iter().cloned())
        .collect();
    OptimizationResult {
        ranked,
        allowlist,
        traces: vec![],
        outcome: "ranked",
    }
}

pub fn optimize(
    policy: &RoutingPolicy,
    context: &RoutingContext,
    candidates: &[Candidate<'_>],
    estimator: &dyn QualityEstimator,
) -> Result<OptimizationResult, &'static str> {
    policy.validate()?;
    if candidates.len() > 128 {
        return Err("too many candidates");
    }
    if policy.mode == RoutingMode::Legacy {
        return Ok(legacy_rank(candidates));
    }
    let mut ranked = Vec::new();
    let mut traces = Vec::new();
    let mut keys = HashSet::new();
    let mut supply_excluded = false;
    let mut qualities = HashMap::new();
    for c in candidates {
        if !keys.insert(c.deployment.id.as_str()) {
            continue;
        }
        let mut reasons = Vec::new();
        let d = c.deployment;
        let m = c.model;
        if d.model_profile_id != m.id {
            reasons.push("model_identity".into());
        }
        if !d.allowed_lanes.contains(&policy.lane) {
            reasons.push("lane".into());
        }
        if let Some(allowed) = &policy.constraints.allowed_deployments
            && !allowed.contains(&d.id)
        {
            reasons.push("deployment".into());
        }
        if let Some(allowed) = &policy.constraints.allowed_sources
            && !allowed.contains(&d.source_ref)
        {
            reasons.push("source".into());
        }
        if policy.constraints.external_network_allowed == Some(false) && d.external_network {
            reasons.push("privacy".into());
        }
        if policy.constraints.data_retention_allowed == Some(false) && d.retains_data != Some(false)
        {
            reasons.push("privacy".into());
        }
        if let Some(region) = &policy.constraints.required_region
            && d.region.as_ref() != Some(region)
        {
            reasons.push("locality".into());
        }
        if let Some(harness) = &context.harness
            && !d.adapter_constraints.is_empty()
            && !d.adapter_constraints.contains(harness)
        {
            reasons.push("adapter".into());
        }
        if context.required_tools && m.capabilities.tools != Support::Supported {
            reasons.push("capability".into());
        }
        if context.required_structured_output
            && m.capabilities.structured_output != Support::Supported
        {
            reasons.push("capability".into());
        }
        if context.required_vision && m.capabilities.vision != Support::Supported {
            reasons.push("capability".into());
        }
        if context.required_streaming && m.capabilities.streaming != Support::Supported {
            reasons.push("capability".into());
        }
        if policy.mode == RoutingMode::Enforce
            && (context.input_tokens.is_none() || context.output_reserve.is_none())
        {
            reasons.push("context_unknown".into());
        }
        if let (Some(input), Some(output)) = (context.input_tokens, context.output_reserve) {
            let total = input
                .saturating_add(output)
                .saturating_add(context.safety_margin);
            if m.context_limits.input.is_none_or(|max| input > max)
                || m.context_limits.output.is_none_or(|max| output > max)
                || m.context_limits.total.is_none_or(|max| total > max)
            {
                reasons.push("context".into());
            }
        }
        if c.eligible_provider_ids.is_empty() {
            reasons.push("provider".into());
        }
        if let Some(s) = c.state {
            if !s.enabled {
                reasons.push("disabled".into());
            }
            if s.reachability == Reachability::Down {
                reasons.push("health_down".into());
                supply_excluded = true;
            }
            if s.cooldown {
                reasons.push("cooldown".into());
                supply_excluded = true;
            }
            if s.circuit_open {
                reasons.push("circuit_open".into());
                supply_excluded = true;
            }
            if s.in_use
                .zip(d.concurrency_limit)
                .is_some_and(|(used, max)| used >= max)
            {
                reasons.push("concurrency".into());
                supply_excluded = true;
            }
            if s.rpm_remaining == Some(0) || s.tpm_remaining == Some(0) {
                reasons.push("rate_limit".into());
                supply_excluded = true;
            }
            if s.quota_remaining == Some(0.0) {
                reasons.push("quota_exhausted".into());
                supply_excluded = true;
            }
        }
        if !finite_nonnegative(c.cost_usd)
            || !finite_nonnegative(c.latency_ms)
            || !c
                .pressure
                .is_none_or(|p| p.is_finite() && (0.0..=1.0).contains(&p))
        {
            reasons.push("invalid_estimate".into());
        }
        if policy
            .constraints
            .max_cost_usd
            .is_some_and(|max| c.cost_usd.is_none_or(|cost| cost > max))
        {
            reasons.push("cost".into());
        }
        if policy
            .constraints
            .max_latency_ms
            .is_some_and(|max| c.latency_ms.is_none_or(|latency| latency > max))
        {
            reasons.push("latency".into());
        }
        let mut quality = None;
        let mut score = None;
        if reasons.is_empty() {
            let estimate = estimator.estimate(m, context);
            match estimate.index {
                Some(q) if q.is_finite() && (0.0..=1.0).contains(&q) && q >= policy.min_quality => {
                    let w = &policy.weights;
                    let cost = c.cost_usd.map_or(1.0, |v| {
                        (v / policy.normalization.cost_reference_usd).min(1.0)
                    });
                    let latency = c.latency_ms.map_or(1.0, |v| {
                        (v / policy.normalization.latency_reference_ms).min(1.0)
                    });
                    let pressure = c.pressure.unwrap_or(1.0);
                    let value =
                        w.quality * q - w.cost * cost - w.latency * latency - w.pressure * pressure;
                    score = Some(value);
                    qualities.insert(d.id.clone(), q);
                    ranked.push(RankedCandidate {
                        model_profile_id: m.id.clone(),
                        deployment_id: d.id.clone(),
                        eligible_provider_ids: c.eligible_provider_ids.clone(),
                        score,
                        config_order: d.config_order,
                    });
                }
                Some(q) if !q.is_finite() || !(0.0..=1.0).contains(&q) => {
                    reasons.push("quality_invalid".into())
                }
                Some(_) => reasons.push("quality_below_min".into()),
                None => reasons.push("quality_unknown".into()),
            }
            quality = Some(estimate);
        }
        traces.push(CandidateTrace {
            model_profile_id: m.id.clone(),
            deployment_id: d.id.clone(),
            eligible_provider_ids: c.eligible_provider_ids.clone(),
            excluded_reasons: reasons,
            quality,
            cost_usd: c.cost_usd,
            latency_ms: c.latency_ms,
            pressure: c.pressure,
            score,
        });
    }
    ranked.sort_by(|a, b| {
        let da = candidates
            .iter()
            .find(|c| c.deployment.id == a.deployment_id)
            .expect("ranked candidate");
        let db = candidates
            .iter()
            .find(|c| c.deployment.id == b.deployment_id)
            .expect("ranked candidate");
        let group = |c: &Candidate<'_>| {
            if policy.lane == crate::Tier::Cheap
                && policy.local_preference
                && !c.deployment.external_network
            {
                0
            } else if policy.prefer_free && c.cost_usd == Some(0.0) {
                1
            } else {
                2
            }
        };
        group(da)
            .cmp(&group(db))
            .then_with(|| {
                if policy.objective == Objective::QualityFirst {
                    quantize(*qualities.get(&b.deployment_id).unwrap())
                        .cmp(&quantize(*qualities.get(&a.deployment_id).unwrap()))
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .then_with(|| quantize(b.score.unwrap()).cmp(&quantize(a.score.unwrap())))
            .then_with(|| {
                quantize(da.cost_usd.unwrap_or(f64::MAX))
                    .cmp(&quantize(db.cost_usd.unwrap_or(f64::MAX)))
            })
            .then_with(|| a.config_order.cmp(&b.config_order))
            .then_with(|| a.model_profile_id.cmp(&b.model_profile_id))
            .then_with(|| a.deployment_id.cmp(&b.deployment_id))
    });
    let allowlist = ranked
        .iter()
        .flat_map(|c| c.eligible_provider_ids.iter().cloned())
        .collect();
    let outcome = if ranked.is_empty() {
        if supply_excluded {
            "defer"
        } else {
            "unroutable"
        }
    } else {
        "ranked"
    };
    Ok(OptimizationResult {
        ranked,
        allowlist,
        traces,
        outcome,
    })
}
