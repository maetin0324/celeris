use super::evidence::wilson_interval;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingStage {
    Shadow,
    Enforce,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardrailReason {
    PassRate,
    FailureRate,
    WallClock,
    EvidenceHarnessMajorMismatch,
    QuotaLow,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GuardrailInput {
    pub successes: u64,
    pub runs: u64,
    pub failures: u64,
    pub failure_baseline: u64,
    pub baseline_runs: u64,
    pub wall_median_s: f64,
    pub evidence_wall_median_s: f64,
    pub selected_pass_lower: f64,
    pub quota_remaining_fraction: f64,
    pub live_harness_version: String,
    pub evidence_harness_version: String,
    pub window: u64,
    pub z: f64,
    pub quota_floor: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GuardrailResult {
    pub stage: RoutingStage,
    pub reasons: Vec<GuardrailReason>,
    pub notify: bool,
}

pub fn evaluate_guardrail(stage: RoutingStage, input: &GuardrailInput) -> GuardrailResult {
    if stage == RoutingStage::Shadow {
        return GuardrailResult {
            stage,
            reasons: Vec::new(),
            notify: false,
        };
    }
    let n = input.runs.min(input.window);
    let fail_n = input.failures.min(n);
    let mut reasons = Vec::new();
    let pass_interval = wilson_interval(input.successes.min(n), n, input.z);
    if n > 0 && pass_interval.upper < input.selected_pass_lower {
        reasons.push(GuardrailReason::PassRate);
    }
    let base_n = input.baseline_runs.min(input.window);
    let base_fail = input.failure_baseline.min(base_n);
    let failure_upper_threshold = if base_n == 0 {
        f64::INFINITY
    } else {
        (base_fail as f64 / base_n as f64) * 2.0
    };
    let failure_lower = wilson_interval(fail_n, n, input.z).lower;
    if n > 0 && failure_lower > failure_upper_threshold {
        reasons.push(GuardrailReason::FailureRate);
    }
    if input.evidence_wall_median_s > 0.0
        && input.wall_median_s > 2.0 * input.evidence_wall_median_s
    {
        reasons.push(GuardrailReason::WallClock);
    }
    if super::evidence::major_version(&input.live_harness_version)
        != super::evidence::major_version(&input.evidence_harness_version)
    {
        reasons.push(GuardrailReason::EvidenceHarnessMajorMismatch);
    }
    let notify = input.quota_remaining_fraction < input.quota_floor;
    if notify {
        reasons.push(GuardrailReason::QuotaLow);
    }
    let stop = reasons.iter().any(|r| *r != GuardrailReason::QuotaLow);
    GuardrailResult {
        stage: if stop {
            RoutingStage::Shadow
        } else {
            RoutingStage::Enforce
        },
        reasons,
        notify,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> GuardrailInput {
        GuardrailInput {
            successes: 50,
            runs: 50,
            failures: 0,
            failure_baseline: 0,
            baseline_runs: 50,
            wall_median_s: 10.0,
            evidence_wall_median_s: 10.0,
            selected_pass_lower: 0.8,
            quota_remaining_fraction: 0.5,
            live_harness_version: "3.1".into(),
            evidence_harness_version: "3.0".into(),
            window: 50,
            z: 1.96,
            quota_floor: 0.1,
        }
    }
    #[test]
    fn guardrail_stops_on_each_quality_condition_and_only_notifies_for_quota() {
        let mut input = sample();
        input.successes = 0;
        input.failures = 50;
        assert!(
            evaluate_guardrail(RoutingStage::Enforce, &input)
                .reasons
                .contains(&GuardrailReason::PassRate)
        );
        assert!(
            evaluate_guardrail(RoutingStage::Enforce, &input)
                .reasons
                .contains(&GuardrailReason::FailureRate)
        );
        input = sample();
        input.wall_median_s = 21.0;
        assert!(
            evaluate_guardrail(RoutingStage::Enforce, &input)
                .reasons
                .contains(&GuardrailReason::WallClock)
        );
        input = sample();
        input.evidence_harness_version = "2.9".into();
        assert!(
            evaluate_guardrail(RoutingStage::Enforce, &input)
                .reasons
                .contains(&GuardrailReason::EvidenceHarnessMajorMismatch)
        );
        input = sample();
        input.quota_remaining_fraction = 0.09;
        let result = evaluate_guardrail(RoutingStage::Enforce, &input);
        assert_eq!(result.stage, RoutingStage::Enforce);
        assert!(result.notify);
        input = sample();
        assert_eq!(
            evaluate_guardrail(RoutingStage::Enforce, &input).stage,
            RoutingStage::Enforce
        );
    }
}
