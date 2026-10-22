use super::{evidence::major_version, stage::RoutingStage};
use crate::llm_route::{Candidate, CandidateKey};
use crate::routing_evidence::EvidenceRow;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionSource {
    Evidence,
    RouteDefaults,
    /// frontier lane の規則（ADR 2026-10-22 D3）: lane の `route_defaults` の先頭が根拠表の同等判定に勝つ。
    FrontierLane,
}

/// 同等集合の中で、最小の p90 のこの倍率を超える候補は制約違反として同等から外す（ADR 2026-10-22 D5）。
pub const WALL_P90_SLACK: f64 = 1.5;
/// frontier 以外の lane では ε 探索をしない（探索は frontier lane の中に寄せる。ADR D4）。
pub const FRONTIER_LANE: &str = "frontier";

#[derive(Debug, Clone)]
pub struct PolicyInput<'a> {
    pub eligible: &'a [Candidate],
    pub evidence: &'a [EvidenceRow],
    pub genre: &'a str,
    pub harness_id: &'a str,
    pub harness_version: &'a str,
    /// 決定の lane（空 = lane 不明）。根拠表は lane が一致する行だけを読む。
    pub lane: &'a str,
    /// frontier lane の `route_defaults`（frontier lane の決定のときだけ。他は空）。
    pub pinned: &'a [CandidateKey],
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
                && row.key.lane == input.lane
                && row.key.harness_id == input.harness_id
                && major_version(&row.key.harness_version) == major_version(input.harness_version)
                && row.key.source_id == candidate.source.as_ref()
                && row.key.model_id == candidate.model.as_ref()
        }) {
            rows.insert(identity(&candidate.key()), row);
        }
    }
    if let Some(output) = select_frontier_lane(input) {
        return output;
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
        // 制約: 経過時間 p90 が同等集合の最小の WALL_P90_SLACK 倍以内（値の無い候補は制約の対象外）。
        let min_p90 = equivalent
            .iter()
            .filter_map(|key| rows[key].wall_p90_s)
            .fold(f64::INFINITY, f64::min);
        if min_p90.is_finite() {
            equivalent.retain(|key| {
                rows[key]
                    .wall_p90_s
                    .is_none_or(|p90| p90 <= min_p90 * WALL_P90_SLACK)
            });
        }
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
        // ε 探索は frontier lane の中でだけ行う（ADR D4）。lane が不明な決定は従来どおり。
        let epsilon = if input.lane.is_empty() {
            input.epsilon.clamp(0.0, 1.0)
        } else {
            0.0
        };
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

/// frontier lane の規則（ADR 2026-10-22 D3/D4）。lane の `route_defaults` のうち eligible な先頭を選び、
/// ε 探索は eligible 全体（lane の列 + 候補列）から選ぶ。根拠表の同等判定は見ない。
/// 該当する候補が無い・enforce 以外・lane の列が空のときは `None`（従来の経路）。
fn select_frontier_lane(input: &PolicyInput<'_>) -> Option<Result<PolicyOutput, NoRoute>> {
    if input.pinned.is_empty() {
        return None;
    }
    let pool: Vec<Key> = input.eligible.iter().map(|c| identity(&c.key())).collect();
    let first = input
        .pinned
        .iter()
        .map(identity)
        .find(|key| pool.contains(key))?;
    let mut chosen = first.clone();
    let mut propensity = 1.0;
    let epsilon = input.epsilon.clamp(0.0, 1.0);
    if input.stage == RoutingStage::Enforce && pool.len() > 1 {
        let alternatives: Vec<_> = pool.iter().filter(|key| **key != first).collect();
        if deterministic_unit(input.run_id) < epsilon {
            chosen = alternatives[(deterministic_hash(input.run_id) as usize) % alternatives.len()]
                .clone();
            propensity = epsilon / alternatives.len() as f64;
        } else {
            propensity = 1.0 - epsilon;
        }
    }
    // admission の順: 選んだもの → lane の列 → 残りの候補（宣言順）。
    let mut order = vec![chosen.clone()];
    order.extend(input.pinned.iter().map(identity));
    order.extend(pool.iter().cloned());
    let mut seen = Vec::new();
    order.retain(|key| {
        let fresh = !seen.contains(key);
        seen.push(key.clone());
        fresh && pool.contains(key)
    });
    // 先頭 = 探索しないときの選択（trace が探索かどうかを先頭との比較で判定する）。
    let equivalent = std::iter::once(&first)
        .chain(pool.iter().filter(|key| **key != first))
        .map(candidate_key)
        .collect();
    Some(build_output(
        input,
        &order,
        &chosen,
        equivalent,
        SelectionSource::FrontierLane,
        propensity,
        None,
    ))
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
    // 主指標: 成功 1 件あたりの quota 加重コスト（ADR 2026-10-22 D5）。値の無い候補は後ろ。
    known_first(rows[a].quota_cost_per_pass, rows[b].quota_cost_per_pass)
        .then_with(|| pressure(a).total_cmp(&pressure(b)))
        .then_with(|| known_first(rows[a].usd_per_pass, rows[b].usd_per_pass))
        .then_with(|| known_first(rows[a].wall_p90_s, rows[b].wall_p90_s))
        .then_with(|| a.cmp(b))
}

/// 値の無い（`None`）定価・時間は、値のある候補より後に回す。
fn known_first(a: Option<f64>, b: Option<f64>) -> std::cmp::Ordering {
    match (a, b) {
        (Some(a), Some(b)) => a.total_cmp(&b),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
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
            quota_cost_per_pass: None,
            key: crate::routing_evidence::EvidenceKey {
                lane: String::new(),
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
            pass_boot_lower: lower,
            pass_boot_upper: upper,
            wall_p90_s: Some(10.0),
            usd_per_pass: Some(usd),
            quota_per_run: [("week".to_string(), quota)].into(),
            timeout_rate: 0.0,
            low_confidence: false,
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
            lane: "",
            pinned: &[],
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
            lane: "",
            pinned: &[],
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
