use super::{
    evidence::{EvidenceRow, major_version},
    stage::RoutingStage,
};
use crate::llm_route::{Candidate, CandidateKey};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionSource {
    Evidence,
    RouteDefaults,
}

#[derive(Debug, Clone)]
pub struct PolicyInput<'a> {
    pub eligible: &'a [Candidate],
    pub evidence: &'a [EvidenceRow],
    pub genre: &'a str,
    pub harness_id: &'a str,
    pub harness_version: &'a str,
    pub route_defaults: &'a [CandidateKey],
    /// Remaining quota by (candidate key, quota window); values are absolute remaining units.
    pub quota_remaining: &'a BTreeMap<(String, String, String), f64>,
    pub stage: RoutingStage,
    pub epsilon: f64,
    pub run_id: &'a str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PolicyOutput {
    pub selected: Candidate,
    pub ranked_fallbacks: Vec<Candidate>,
    pub equivalent: Vec<CandidateKey>,
    pub source: SelectionSource,
    pub propensity: f64,
    pub evidence_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoRoute;

pub fn select(input: &PolicyInput<'_>) -> Result<PolicyOutput, NoRoute> {
    let mut rows = BTreeMap::new();
    for candidate in input.eligible {
        if let Some(row) = input.evidence.iter().find(|row| {
            row.key.genre == input.genre
                && row.key.harness_id == input.harness_id
                && major_version(&row.key.harness_version) == major_version(input.harness_version)
                && row.key.source_id == candidate.source.as_ref()
                && row.key.model_id == candidate.model.as_ref()
        }) {
            rows.insert(identity(&candidate.key()), row);
        }
    }
    if input.stage == RoutingStage::Enforce && !rows.is_empty() {
        let best = rows
            .values()
            .map(|row| row.pass_lower)
            .fold(f64::NEG_INFINITY, f64::max);
        let mut equivalent: Vec<_> = rows
            .iter()
            .filter(|(_, row)| row.pass_upper >= best)
            .map(|(key, _)| key.clone())
            .collect();
        equivalent.sort_by(|a, b| compare_candidates(a, b, &rows, input.quota_remaining));
        let mut all_evidence: Vec<_> = rows.keys().cloned().collect();
        all_evidence.sort_by(|a, b| {
            rows[b]
                .pass_lower
                .total_cmp(&rows[a].pass_lower)
                .then_with(|| compare_candidates(a, b, &rows, input.quota_remaining))
        });
        let mut ordered = equivalent.clone();
        for key in all_evidence
            .into_iter()
            .chain(default_keys(input).into_iter().map(|k| identity(&k)))
        {
            if !ordered.contains(&key) && input.eligible.iter().any(|c| identity(&c.key()) == key) {
                ordered.push(key);
            }
        }
        let selected_key = equivalent.first().ok_or(NoRoute)?.clone();
        let mut chosen = selected_key.clone();
        let epsilon = input.epsilon.clamp(0.0, 1.0);
        if equivalent.len() > 1 && deterministic_unit(input.run_id) < epsilon {
            let alternatives: Vec<_> = equivalent
                .iter()
                .filter(|key| **key != selected_key)
                .collect();
            if !alternatives.is_empty() {
                chosen = alternatives
                    [(deterministic_hash(input.run_id) as usize) % alternatives.len()]
                .clone()
                .clone();
            }
        }
        let propensity = if equivalent.len() > 1 {
            if chosen == selected_key {
                1.0 - epsilon
            } else {
                epsilon / (equivalent.len() - 1) as f64
            }
        } else {
            1.0
        };
        let public_equivalent = equivalent.iter().map(candidate_key).collect();
        return build_output(
            input,
            &ordered,
            &chosen,
            public_equivalent,
            SelectionSource::Evidence,
            propensity,
            rows.get(&selected_key).map(|r| r.evidence_version.clone()),
        );
    }
    let defaults = default_keys(input);
    let ordered: Vec<_> = defaults
        .into_iter()
        .map(|k| identity(&k))
        .filter(|key| input.eligible.iter().any(|c| identity(&c.key()) == *key))
        .collect();
    let chosen = ordered.first().ok_or(NoRoute)?;
    build_output(
        input,
        &ordered,
        chosen,
        Vec::new(),
        SelectionSource::RouteDefaults,
        1.0,
        None,
    )
}

fn default_keys(input: &PolicyInput<'_>) -> Vec<CandidateKey> {
    input.route_defaults.to_vec()
}

type Key = (String, String);
fn identity(key: &CandidateKey) -> Key {
    (key.source.to_string(), key.model.to_string())
}
fn candidate_key(key: &Key) -> CandidateKey {
    CandidateKey {
        source: key.0.clone().into(),
        model: key.1.clone().into(),
    }
}
fn build_output(
    input: &PolicyInput<'_>,
    order: &[Key],
    chosen: &Key,
    equivalent: Vec<CandidateKey>,
    source: SelectionSource,
    propensity: f64,
    version: Option<String>,
) -> Result<PolicyOutput, NoRoute> {
    let find = |key: &Key| {
        input
            .eligible
            .iter()
            .find(|c| identity(&c.key()) == *key)
            .cloned()
    };
    let selected = find(chosen).ok_or(NoRoute)?;
    let ranked_fallbacks = order
        .iter()
        .filter(|key| **key != *chosen)
        .filter_map(find)
        .collect();
    Ok(PolicyOutput {
        selected,
        ranked_fallbacks,
        equivalent,
        source,
        propensity,
        evidence_version: version,
    })
}

fn compare_candidates(
    a: &Key,
    b: &Key,
    rows: &BTreeMap<Key, &EvidenceRow>,
    remaining: &BTreeMap<(String, String, String), f64>,
) -> std::cmp::Ordering {
    let pressure = |key: &Key| -> f64 {
        rows.get(key)
            .map(|row| {
                row.quota_per_run
                    .iter()
                    .map(|(window, amount)| {
                        amount
                            / remaining
                                .get(&(key.0.clone(), key.1.clone(), window.clone()))
                                .copied()
                                .unwrap_or(f64::INFINITY)
                                .max(f64::MIN_POSITIVE)
                    })
                    .fold(0.0, f64::max)
            })
            .unwrap_or(f64::INFINITY)
    };
    pressure(a)
        .total_cmp(&pressure(b))
        .then_with(|| rows[a].usd_list_median.total_cmp(&rows[b].usd_list_median))
        .then_with(|| rows[a].wall_median_s.total_cmp(&rows[b].wall_median_s))
        .then_with(|| a.cmp(b))
}

fn deterministic_hash(value: &str) -> u64 {
    value.bytes().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}
fn deterministic_unit(value: &str) -> f64 {
    (deterministic_hash(value) as f64) / (u64::MAX as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_route::{ApiFormat, ModelId, SourceId};
    fn c(source: &str, model: &str) -> Candidate {
        Candidate {
            source: SourceId::from(source),
            model: ModelId::from(model),
            api_format: ApiFormat::from("openai"),
        }
    }
    fn row(c: &Candidate, lower: f64, upper: f64, quota: f64, usd: f64) -> EvidenceRow {
        EvidenceRow {
            key: super::super::EvidenceKey {
                genre: "coding".into(),
                harness_id: "h".into(),
                harness_version: "2.4.0".into(),
                source_id: c.source.to_string(),
                model_id: c.model.to_string(),
            },
            evidence_version: "v1".into(),
            n: 100,
            pass: 90,
            pass_lower: lower,
            pass_upper: upper,
            wall_median_s: 10.0,
            usd_list_median: usd,
            quota_per_run: vec![("week".into(), quota)],
            timeout_rate: 0.0,
        }
    }
    #[test]
    fn equivalent_quota_order_and_deterministic_exploration() {
        let a = c("a", "m");
        let b = c("b", "m");
        let candidates = vec![a.clone(), b.clone()];
        let evidence = vec![row(&a, 0.8, 0.95, 1.0, 1.0), row(&b, 0.75, 0.9, 1.0, 2.0)];
        let mut remaining = BTreeMap::new();
        remaining.insert(("a".into(), "m".into(), "week".into()), 10.0);
        remaining.insert(("b".into(), "m".into(), "week".into()), 100.0);
        let input = PolicyInput {
            eligible: &candidates,
            evidence: &evidence,
            genre: "coding",
            harness_id: "h",
            harness_version: "2.1.0",
            route_defaults: &[],
            quota_remaining: &remaining,
            stage: RoutingStage::Enforce,
            epsilon: 1.0,
            run_id: "run-1",
        };
        let x = select(&input).unwrap();
        let y = select(&input).unwrap();
        assert_eq!(x, y);
        assert_eq!(x.equivalent.len(), 2);
        assert_eq!(x.selected, a);
        assert!((x.propensity - 1.0).abs() < f64::EPSILON);
    }
    #[test]
    fn shadow_obeys_defaults_and_unknown_evidence_falls_back() {
        let a = c("a", "m");
        let b = c("b", "m");
        let candidates = vec![a.clone(), b.clone()];
        let input = PolicyInput {
            eligible: &candidates,
            evidence: &[],
            genre: "writing",
            harness_id: "h",
            harness_version: "1.0",
            route_defaults: &[b.key(), a.key()],
            quota_remaining: &BTreeMap::new(),
            stage: RoutingStage::Shadow,
            epsilon: 0.0,
            run_id: "x",
        };
        assert_eq!(select(&input).unwrap().selected, b);
    }
}
