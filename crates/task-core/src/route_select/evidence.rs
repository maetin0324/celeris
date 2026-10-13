use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EvidenceKey {
    pub genre: String,
    pub harness_id: String,
    pub harness_version: String,
    pub source_id: String,
    pub model_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceRow {
    pub key: EvidenceKey,
    pub evidence_version: String,
    pub n: u64,
    pub pass: u64,
    pub pass_lower: f64,
    pub pass_upper: f64,
    pub wall_median_s: f64,
    pub usd_list_median: f64,
    pub quota_per_run: Vec<(String, f64)>,
    pub timeout_rate: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WilsonInterval {
    pub lower: f64,
    pub upper: f64,
}

/// Wilson score interval for a binomial proportion (default confidence: 95%).
pub fn wilson_interval(successes: u64, trials: u64, z: f64) -> WilsonInterval {
    if trials == 0 {
        return WilsonInterval {
            lower: 0.0,
            upper: 1.0,
        };
    }
    let n = trials as f64;
    let p = successes.min(trials) as f64 / n;
    let z2 = z * z;
    let d = 1.0 + z2 / n;
    let center = (p + z2 / (2.0 * n)) / d;
    let radius = z * ((p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt()) / d;
    WilsonInterval {
        lower: (center - radius).max(0.0),
        upper: (center + radius).min(1.0),
    }
}

/// Evidence is usable only when all dimensions match and harness major versions agree.
pub fn major_version(version: &str) -> &str {
    version.split('.').next().unwrap_or(version)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wilson_interval_bounds_and_zero_trials() {
        let interval = wilson_interval(8, 10, 1.96);
        assert!(interval.lower < 0.8 && interval.upper > 0.8);
        assert_eq!(
            wilson_interval(0, 0, 1.96),
            WilsonInterval {
                lower: 0.0,
                upper: 1.0
            }
        );
    }
}
