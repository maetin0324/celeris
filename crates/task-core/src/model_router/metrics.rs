//! Pure offline metrics for comparable, paired routing outcomes.
//!
//! A curve point is an aggregate over the same tasks evaluated with the weak,
//! strong, and routed policies. Callers must not turn unobserved counterfactuals
//! into points. Costs must use one unit (for example, USD per task) throughout.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UndefinedReason {
    PairedOutcomeMissing,
    ZeroDenominator,
    QualityBelowWeak,
    NonPositiveAdditionalCost,
    InvalidInput,
    TargetNotReached,
}

/// A finite value, or an absent value with a machine-readable reason.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MetricValue {
    Defined(f64),
    Undefined { reason: UndefinedReason },
}

impl MetricValue {
    pub fn value(self) -> Option<f64> {
        match self {
            Self::Defined(value) => Some(value),
            Self::Undefined { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PairedBaseline {
    pub weak_quality: f64,
    pub strong_quality: f64,
    pub weak_cost: f64,
    pub strong_cost: f64,
    /// Number of tasks with observed outcomes for both baseline models.
    pub paired_outcomes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PairedCurvePoint {
    /// Fraction in [0, 1], not a percentage.
    pub strong_call_fraction: f64,
    pub quality: f64,
    pub cost: f64,
    /// Number of the same baseline tasks with an observed routed outcome.
    pub paired_outcomes: usize,
}

fn undefined(reason: UndefinedReason) -> MetricValue {
    MetricValue::Undefined { reason }
}

fn valid_baseline(b: PairedBaseline) -> Result<(), UndefinedReason> {
    if b.paired_outcomes == 0 {
        return Err(UndefinedReason::PairedOutcomeMissing);
    }
    if ![b.weak_quality, b.strong_quality, b.weak_cost, b.strong_cost]
        .into_iter()
        .all(f64::is_finite)
        || b.weak_cost < 0.0
        || b.strong_cost < 0.0
    {
        return Err(UndefinedReason::InvalidInput);
    }
    if b.strong_quality < b.weak_quality {
        return Err(UndefinedReason::QualityBelowWeak);
    }
    Ok(())
}

fn valid_points(b: PairedBaseline, points: &[PairedCurvePoint]) -> Result<(), UndefinedReason> {
    valid_baseline(b)?;
    if points.is_empty()
        || points
            .iter()
            .any(|p| p.paired_outcomes != b.paired_outcomes)
    {
        return Err(UndefinedReason::PairedOutcomeMissing);
    }
    if points.iter().any(|p| {
        !p.strong_call_fraction.is_finite()
            || !(0.0..=1.0).contains(&p.strong_call_fraction)
            || !p.quality.is_finite()
            || !p.cost.is_finite()
            || p.cost < 0.0
    }) {
        return Err(UndefinedReason::InvalidInput);
    }
    if points.iter().any(|p| p.quality < b.weak_quality) {
        return Err(UndefinedReason::QualityBelowWeak);
    }
    Ok(())
}

/// Average Performance Gap Recovered, from *RouteLLM: Learning to Route LLMs
/// from Preference Data* (Ong et al., 2024), `evaluate.py` at `0b64fdafe049`.
/// <https://github.com/lm-sys/RouteLLM/blob/0b64fdafe049/routellm/evals/evaluate.py>
/// Integrates (routed_quality - weak_quality)/(strong_quality - weak_quality)
/// over strong-call fraction, with linear interpolation and explicit endpoints.
pub fn apgr(b: PairedBaseline, points: &[PairedCurvePoint]) -> MetricValue {
    if let Err(reason) = valid_points(b, points) {
        return undefined(reason);
    }
    let gap = b.strong_quality - b.weak_quality;
    if gap == 0.0 {
        return undefined(UndefinedReason::ZeroDenominator);
    }
    let mut curve = vec![(0.0, b.weak_quality), (1.0, b.strong_quality)];
    curve.extend(points.iter().map(|p| (p.strong_call_fraction, p.quality)));
    curve.sort_by(|a, b| a.0.total_cmp(&b.0));
    if curve
        .windows(2)
        .any(|pair| pair[0].0 == pair[1].0 && pair[0].1 != pair[1].1)
    {
        return undefined(UndefinedReason::InvalidInput);
    }
    curve.dedup_by(|a, b| a.0 == b.0);
    let area: f64 = curve
        .windows(2)
        .map(|pair| {
            (pair[1].0 - pair[0].0) * ((pair[0].1 - b.weak_quality) + (pair[1].1 - b.weak_quality))
                / (2.0 * gap)
        })
        .sum();
    if area.is_finite() {
        MetricValue::Defined(area)
    } else {
        undefined(UndefinedReason::InvalidInput)
    }
}

/// Minimum strong-call fraction reaching a target fraction of the quality gap.
/// The threshold metric follows *RouteLLM: Learning to Route LLMs from Preference
/// Data* (Ong et al., 2024), `evaluate.py` at `0b64fdafe049`.
/// <https://github.com/lm-sys/RouteLLM/blob/0b64fdafe049/routellm/evals/evaluate.py>
pub fn strong_call_rate_at_quality(
    b: PairedBaseline,
    points: &[PairedCurvePoint],
    target_gap_fraction: f64,
) -> MetricValue {
    if let Err(reason) = valid_points(b, points) {
        return undefined(reason);
    }
    if !target_gap_fraction.is_finite() || !(0.0..=1.0).contains(&target_gap_fraction) {
        return undefined(UndefinedReason::InvalidInput);
    }
    let gap = b.strong_quality - b.weak_quality;
    if gap == 0.0 {
        return undefined(UndefinedReason::ZeroDenominator);
    }
    let target = b.weak_quality + target_gap_fraction * gap;
    let mut curve = vec![(0.0, b.weak_quality), (1.0, b.strong_quality)];
    curve.extend(points.iter().map(|p| (p.strong_call_fraction, p.quality)));
    curve.sort_by(|a, b| a.0.total_cmp(&b.0));
    if curve[0].1 >= target {
        return MetricValue::Defined(0.0);
    }
    for pair in curve.windows(2) {
        if pair[0].1 < target && pair[1].1 >= target {
            let rate = pair[0].0
                + (pair[1].0 - pair[0].0) * (target - pair[0].1) / (pair[1].1 - pair[0].1);
            return MetricValue::Defined(rate);
        }
    }
    undefined(UndefinedReason::TargetNotReached)
}

/// Area under the non-decreasing upper cost-quality convex hull, divided by
/// the baseline cost span. From *ROUTERBENCH: A Benchmark for Multi-LLM Routing
/// System* (Hu et al., 2024), `AIQ.py` at `cc67d1008bd8`.
/// <https://github.com/withmartian/routerbench/blob/cc67d1008bd8/evaluation/AIQ.py>
pub fn aiq(b: PairedBaseline, points: &[PairedCurvePoint]) -> MetricValue {
    if let Err(reason) = valid_points(b, points) {
        return undefined(reason);
    }
    let span = b.strong_cost - b.weak_cost;
    if span <= 0.0 {
        return undefined(UndefinedReason::NonPositiveAdditionalCost);
    }
    if points
        .iter()
        .any(|p| p.cost < b.weak_cost || p.cost > b.strong_cost)
    {
        return undefined(UndefinedReason::InvalidInput);
    }
    let mut candidates = vec![
        (b.weak_cost, b.weak_quality),
        (b.strong_cost, b.strong_quality),
    ];
    candidates.extend(points.iter().map(|p| (p.cost, p.quality)));
    candidates.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| b.1.total_cmp(&a.1)));
    let mut hull: Vec<(f64, f64)> = Vec::new();
    for (cost, quality) in candidates {
        if hull.last().is_some_and(|last| last.0 == cost) {
            continue;
        }
        if hull.last().is_some_and(|last| quality <= last.1) {
            continue;
        }
        while hull.len() >= 2 {
            let a = hull[hull.len() - 2];
            let z = hull[hull.len() - 1];
            let old_slope = (z.1 - a.1) / (z.0 - a.0);
            let new_slope = (quality - z.1) / (cost - z.0);
            if new_slope <= old_slope {
                break;
            }
            hull.pop();
        }
        hull.push((cost, quality));
    }
    // A cheaper point may dominate the strong endpoint; carry its quality to
    // the right boundary, where spending more cannot reduce available quality.
    if hull.last().is_some_and(|last| last.0 < b.strong_cost) {
        hull.push((b.strong_cost, hull.last().map_or(0.0, |last| last.1)));
    }
    let area: f64 = hull
        .windows(2)
        .map(|pair| (pair[1].0 - pair[0].0) * (pair[0].1 + pair[1].1) / 2.0)
        .sum();
    let value = area / span;
    if value.is_finite() {
        MetricValue::Defined(value)
    } else {
        undefined(UndefinedReason::InvalidInput)
    }
}

/// Incremental Benefit per Cost (quality gained per additional cost over weak).
/// From *AutoMix: Automatically Mixing Language Models* (Aggarwal et al., 2023),
/// Section 3. Returns the raw slope; the paper's relative lift is a distinct metric.
/// <https://openreview.net/pdf?id=FJo2lroF7R>
pub fn ibc(b: PairedBaseline, point: PairedCurvePoint) -> MetricValue {
    if let Err(reason) = valid_points(b, &[point]) {
        return undefined(reason);
    }
    let additional_cost = point.cost - b.weak_cost;
    if additional_cost <= 0.0 {
        return undefined(UndefinedReason::NonPositiveAdditionalCost);
    }
    let value = (point.quality - b.weak_quality) / additional_cost;
    if value.is_finite() {
        MetricValue::Defined(value)
    } else {
        undefined(UndefinedReason::InvalidInput)
    }
}

/// Linear-interpolated percentile of finite, observed values; an empty or
/// invalid sample has no value and carries a reason.
pub fn percentile(values: &[f64], quantile: f64) -> MetricValue {
    if values.is_empty() {
        return undefined(UndefinedReason::ZeroDenominator);
    }
    if !quantile.is_finite()
        || !(0.0..=1.0).contains(&quantile)
        || values.iter().any(|value| !value.is_finite())
    {
        return undefined(UndefinedReason::InvalidInput);
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let index = quantile * (sorted.len() - 1) as f64;
    let low = index.floor() as usize;
    let high = index.ceil() as usize;
    let value = sorted[low] + (sorted[high] - sorted[low]) * (index - low as f64);
    if value.is_finite() {
        MetricValue::Defined(value)
    } else {
        undefined(UndefinedReason::InvalidInput)
    }
}

pub fn p50_p95(values: &[f64]) -> (MetricValue, MetricValue) {
    (percentile(values, 0.5), percentile(values, 0.95))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_metrics_apgr_aiq_ibc_known_curves() {
        let baseline = PairedBaseline {
            weak_quality: 0.4,
            strong_quality: 0.8,
            weak_cost: 1.0,
            strong_cost: 5.0,
            paired_outcomes: 4,
        };
        let points = [
            PairedCurvePoint {
                strong_call_fraction: 0.25,
                quality: 0.6,
                cost: 2.0,
                paired_outcomes: 4,
            },
            PairedCurvePoint {
                strong_call_fraction: 0.5,
                quality: 0.7,
                cost: 3.0,
                paired_outcomes: 4,
            },
            PairedCurvePoint {
                strong_call_fraction: 0.75,
                quality: 0.72,
                cost: 4.0,
                paired_outcomes: 4,
            },
        ];
        // PGR trapezoids: 0.0625 + 0.15625 + 0.19375 + 0.225.
        assert!((apgr(baseline, &points).value().unwrap() - 0.6375).abs() < 1e-12);
        assert!(
            (strong_call_rate_at_quality(baseline, &points, 0.5)
                .value()
                .unwrap()
                - 0.25)
                .abs()
                < 1e-12
        );
        // (2, .6) is above the weak-to-strong line; (3, .7) lies on its
        // upper hull edge. (4, .72) is below and is discarded.
        assert!((aiq(baseline, &points).value().unwrap() - 0.6625).abs() < 1e-12);
        assert!((ibc(baseline, points[0]).value().unwrap() - 0.2).abs() < 1e-12);
        let (p50, p95) = p50_p95(&[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(p50, MetricValue::Defined(2.5));
        assert!((p95.value().unwrap() - 3.85).abs() < 1e-12);

        let missing = PairedBaseline {
            paired_outcomes: 0,
            ..baseline
        };
        assert_eq!(
            apgr(missing, &points),
            undefined(UndefinedReason::PairedOutcomeMissing)
        );
        assert_eq!(apgr(missing, &points).value(), None);
        assert_eq!(
            aiq(
                baseline,
                &[PairedCurvePoint {
                    paired_outcomes: 0,
                    ..points[0]
                }]
            ),
            undefined(UndefinedReason::PairedOutcomeMissing)
        );
        let zero_gap = PairedBaseline {
            strong_quality: 0.4,
            ..baseline
        };
        assert_eq!(
            apgr(zero_gap, &points),
            undefined(UndefinedReason::ZeroDenominator)
        );
        assert_eq!(
            strong_call_rate_at_quality(zero_gap, &points, 0.5),
            undefined(UndefinedReason::ZeroDenominator)
        );
        let lower_strong = PairedBaseline {
            strong_quality: 0.3,
            ..baseline
        };
        assert_eq!(
            apgr(lower_strong, &points),
            undefined(UndefinedReason::QualityBelowWeak)
        );
        assert_eq!(
            ibc(lower_strong, points[0]),
            undefined(UndefinedReason::QualityBelowWeak)
        );
        let lower_routed = PairedCurvePoint {
            quality: 0.3,
            ..points[0]
        };
        assert_eq!(
            ibc(baseline, lower_routed),
            undefined(UndefinedReason::QualityBelowWeak)
        );
        let no_extra_cost = PairedCurvePoint {
            cost: 1.0,
            ..points[0]
        };
        assert_eq!(
            ibc(baseline, no_extra_cost),
            undefined(UndefinedReason::NonPositiveAdditionalCost)
        );
        assert_eq!(
            aiq(
                PairedBaseline {
                    strong_cost: 1.0,
                    ..baseline
                },
                &points
            ),
            undefined(UndefinedReason::NonPositiveAdditionalCost)
        );
        assert_eq!(
            percentile(&[], 0.5),
            undefined(UndefinedReason::ZeroDenominator)
        );
    }
}
