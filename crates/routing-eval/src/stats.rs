//! Deterministic, dependency-free statistics used by routing evaluation.
//!
//! All rates use one Bernoulli outcome per problem (`pass^1`). Repeated trials
//! are first reduced to whether the first trial passed; callers must not treat
//! repeated runs of one problem as independent observations.

use serde::{Deserialize, Serialize};
pub use task_core::routing_evidence::Interval;

const Z_95: f64 = 1.959_963_984_540_054;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Estimate {
    pub n: usize,
    pub pass: usize,
    pub wilson: Interval,
    pub bootstrap: Interval,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CandidateMetrics {
    pub id: String,
    pub pass: f64,
    pub usd: f64,
    pub wall_seconds: f64,
    pub quota: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CandidateEvidence {
    pub id: String,
    pub interval: Interval,
    pub n: usize,
    pub planned_n: usize,
    pub k: usize,
}

/// Wilson score interval for a binomial proportion. Empty input is `[0, 1]`.
pub fn wilson_interval(pass: usize, n: usize, confidence_z: f64) -> Interval {
    if n == 0 {
        return Interval {
            lower: 0.0,
            upper: 1.0,
        };
    }
    let n_f = n as f64;
    let p = (pass.min(n)) as f64 / n_f;
    let z2 = confidence_z * confidence_z;
    let denom = 1.0 + z2 / n_f;
    let center = (p + z2 / (2.0 * n_f)) / denom;
    let half = confidence_z * ((p * (1.0 - p) / n_f + z2 / (4.0 * n_f * n_f)).sqrt()) / denom;
    Interval {
        lower: (center - half).max(0.0),
        upper: (center + half).min(1.0),
    }
}

pub fn wilson_95(pass: usize, n: usize) -> Interval {
    wilson_interval(pass, n, Z_95)
}

/// Bootstrap the mean of problem-level binary outcomes with a reproducible
/// SplitMix64 stream. Percentile bounds use linear interpolation.
pub fn bootstrap_interval(
    outcomes: &[bool],
    seed: u64,
    resamples: usize,
    confidence: f64,
) -> Interval {
    if outcomes.is_empty() || resamples == 0 {
        return Interval {
            lower: 0.0,
            upper: 1.0,
        };
    }
    let alpha = ((1.0 - confidence) / 2.0).clamp(0.0, 0.5);
    let mut rng = SplitMix64(seed);
    let mut samples = Vec::with_capacity(resamples);
    for _ in 0..resamples {
        let mut successes = 0usize;
        for _ in outcomes {
            let index = (rng.next() % outcomes.len() as u64) as usize;
            successes += usize::from(outcomes[index]);
        }
        samples.push(successes as f64 / outcomes.len() as f64);
    }
    samples.sort_by(f64::total_cmp);
    Interval {
        lower: quantile(&samples, alpha),
        upper: quantile(&samples, 1.0 - alpha),
    }
}

pub fn estimate(outcomes: &[bool], seed: u64, resamples: usize) -> Estimate {
    let pass = outcomes.iter().filter(|&&x| x).count();
    Estimate {
        n: outcomes.len(),
        pass,
        wilson: wilson_95(pass, outcomes.len()),
        bootstrap: bootstrap_interval(outcomes, seed, resamples, 0.95),
    }
}

/// Exact two-sided McNemar p-value (binomial test on discordant pairs).
pub fn mcnemar_exact(left: &[bool], right: &[bool]) -> Result<f64, String> {
    if left.len() != right.len() {
        return Err("paired outcome vectors must have equal length".into());
    }
    let (mut left_only, mut right_only) = (0u64, 0u64);
    for (&a, &b) in left.iter().zip(right) {
        match (a, b) {
            (true, false) => left_only += 1,
            (false, true) => right_only += 1,
            _ => {}
        }
    }
    let n = left_only + right_only;
    if n == 0 {
        return Ok(1.0);
    }
    let tail = (0..=left_only.min(right_only))
        .map(|k| binomial_probability(n, k))
        .sum::<f64>();
    Ok((2.0 * tail).min(1.0))
}

/// Paired bootstrap confidence interval for mean(left - right), one pair per problem.
pub fn paired_difference_interval(
    left: &[bool],
    right: &[bool],
    seed: u64,
    resamples: usize,
    confidence: f64,
) -> Result<Interval, String> {
    if left.len() != right.len() {
        return Err("paired outcome vectors must have equal length".into());
    }
    if left.is_empty() || resamples == 0 {
        return Ok(Interval {
            lower: -1.0,
            upper: 1.0,
        });
    }
    let diffs: Vec<i8> = left
        .iter()
        .zip(right)
        .map(|(&a, &b)| i8::from(a) - i8::from(b))
        .collect();
    let alpha = ((1.0 - confidence) / 2.0).clamp(0.0, 0.5);
    let mut rng = SplitMix64(seed);
    let mut samples = Vec::with_capacity(resamples);
    for _ in 0..resamples {
        let sum: i64 = (0..diffs.len())
            .map(|_| diffs[(rng.next() % diffs.len() as u64) as usize] as i64)
            .sum();
        samples.push(sum as f64 / diffs.len() as f64);
    }
    samples.sort_by(f64::total_cmp);
    Ok(Interval {
        lower: quantile(&samples, alpha),
        upper: quantile(&samples, 1.0 - alpha),
    })
}

/// Return non-dominated candidates, maximizing pass and minimizing all costs.
pub fn pareto_frontier(candidates: &[CandidateMetrics]) -> Vec<String> {
    candidates
        .iter()
        .filter(|candidate| {
            !candidates.iter().any(|other| {
                other.id != candidate.id
                    && other.pass >= candidate.pass
                    && other.usd <= candidate.usd
                    && other.wall_seconds <= candidate.wall_seconds
                    && other.quota <= candidate.quota
                    && (other.pass > candidate.pass
                        || other.usd < candidate.usd
                        || other.wall_seconds < candidate.wall_seconds
                        || other.quota < candidate.quota)
            })
        })
        .map(|c| c.id.clone())
        .collect()
}

/// D1 evidence threshold: 24 of 30 for k=2 cells; every planned problem for k=1 cells.
pub fn evidence_is_established(n: usize, planned_n: usize, k: usize) -> bool {
    if planned_n == 0 {
        return false;
    }
    if k == 1 {
        n == planned_n
    } else if planned_n == 30 && k == 2 {
        n >= 24
    } else {
        n >= planned_n
    }
}

/// Candidates whose intervals overlap the highest lower-bound candidate form
/// the non-dominated-by-evidence equivalence set. k=1 is never a superiority claim.
pub fn equivalent_set(candidates: &[CandidateEvidence]) -> Vec<String> {
    let Some(best_lower) = candidates
        .iter()
        .map(|c| c.interval.lower)
        .max_by(f64::total_cmp)
    else {
        return Vec::new();
    };
    candidates
        .iter()
        .filter(|c| c.interval.upper >= best_lower)
        .map(|c| c.id.clone())
        .collect()
}

pub fn may_claim_superiority(k: usize, paired_p: f64, difference: Interval) -> bool {
    k > 1 && paired_p < 0.05 && (difference.lower > 0.0 || difference.upper < 0.0)
}

/// A selection is invalidated when the selected-vs-alternative pass ordering
/// reverses on held-out problems within any stratum.
pub fn held_out_reverses(dev: &[f64], held_out: &[f64]) -> bool {
    if dev.len() != held_out.len() {
        return true;
    }
    dev.iter()
        .zip(held_out)
        .any(|(&d, &h)| (d > 0.0 && h < 0.0) || (d < 0.0 && h > 0.0))
}

pub fn run() -> Result<(), String> {
    Err("stats is a library operation; aggregate consumes evaluation outcomes".into())
}

fn quantile(sorted: &[f64], p: f64) -> f64 {
    if sorted.len() == 1 {
        return sorted[0];
    }
    let position = p * (sorted.len() - 1) as f64;
    let lo = position.floor() as usize;
    let hi = position.ceil() as usize;
    sorted[lo] + (sorted[hi] - sorted[lo]) * (position - lo as f64)
}

fn binomial_probability(n: u64, k: u64) -> f64 {
    // Recurrence avoids factorial overflow; n is bounded by the problem count.
    let mut p = 0.5f64.powi(n as i32);
    for i in 0..k {
        p *= (n - i) as f64 / (i + 1) as f64;
    }
    p
}

struct SplitMix64(u64);
impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-7, "{a} != {b}");
    }

    #[test]
    fn wilson_known_values() {
        let interval = wilson_95(8, 10);
        close(interval.lower, 0.49016247);
        close(interval.upper, 0.94331785);
        assert_eq!(
            wilson_95(0, 0),
            Interval {
                lower: 0.0,
                upper: 1.0
            }
        );
    }

    #[test]
    fn bootstrap_is_seeded_and_problem_level() {
        let outcomes = [true, false, true, true, false];
        let a = bootstrap_interval(&outcomes, 42, 2_000, 0.95);
        let b = bootstrap_interval(&outcomes, 42, 2_000, 0.95);
        assert_eq!(a, b);
        assert!(a.lower <= 0.6 && a.upper >= 0.6);
    }

    #[test]
    fn exact_mcnemar_matches_known_table() {
        // 6 wins in one direction, none in the other: two-sided exact p=0.03125.
        close(mcnemar_exact(&[true; 6], &[false; 6]).unwrap(), 0.03125);
        close(
            mcnemar_exact(&[true, false, true], &[true, false, true]).unwrap(),
            1.0,
        );
    }

    #[test]
    fn paired_interval_and_pareto_are_deterministic() {
        let diff = paired_difference_interval(
            &[true, true, false, true],
            &[false, true, false, false],
            9,
            2_000,
            0.95,
        )
        .unwrap();
        assert!(diff.lower <= 0.5 && diff.upper >= 0.5);
        let candidates = vec![
            CandidateMetrics {
                id: "a".into(),
                pass: 0.8,
                usd: 1.0,
                wall_seconds: 20.0,
                quota: 0.1,
            },
            CandidateMetrics {
                id: "b".into(),
                pass: 0.7,
                usd: 2.0,
                wall_seconds: 30.0,
                quota: 0.2,
            },
            CandidateMetrics {
                id: "c".into(),
                pass: 0.9,
                usd: 2.0,
                wall_seconds: 10.0,
                quota: 0.05,
            },
        ];
        assert_eq!(pareto_frontier(&candidates), vec!["a", "c"]);
    }

    #[test]
    fn d1_evidence_equivalence_k1_and_heldout_rules() {
        assert!(evidence_is_established(24, 30, 2));
        assert!(!evidence_is_established(23, 30, 2));
        assert!(evidence_is_established(12, 12, 1));
        assert!(!evidence_is_established(11, 12, 1));
        let set = equivalent_set(&[
            CandidateEvidence {
                id: "a".into(),
                interval: Interval {
                    lower: 0.55,
                    upper: 0.80,
                },
                n: 30,
                planned_n: 30,
                k: 2,
            },
            CandidateEvidence {
                id: "b".into(),
                interval: Interval {
                    lower: 0.45,
                    upper: 0.60,
                },
                n: 12,
                planned_n: 12,
                k: 1,
            },
            CandidateEvidence {
                id: "c".into(),
                interval: Interval {
                    lower: 0.1,
                    upper: 0.4,
                },
                n: 30,
                planned_n: 30,
                k: 2,
            },
        ]);
        assert_eq!(set, vec!["a", "b"]);
        assert!(!may_claim_superiority(
            1,
            0.01,
            Interval {
                lower: 0.1,
                upper: 0.3
            }
        ));
        assert!(may_claim_superiority(
            2,
            0.01,
            Interval {
                lower: 0.1,
                upper: 0.3
            }
        ));
        assert!(held_out_reverses(&[0.2, -0.1], &[-0.1, 0.2]));
        assert!(!held_out_reverses(&[0.2], &[0.1]));
    }
}
