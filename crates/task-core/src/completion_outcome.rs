//! 完了単位の記録（ADR 2026-10-22-harness-balance-and-lane-first-routing D6）。
//! task / WorkUnit が終端（done / failed）になったときの、完了までの総消費と判定の経過。
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompletionOutcome {
    /// `task` | `work_unit`。
    pub unit_kind: String,
    pub unit_id: String,
    pub task_id: String,
    /// 評価に使う葉の単位（WU、または WU を持たない task）。
    pub leaf: bool,
    pub genre: String,
    /// 判定に通った（WU done / task done）。
    pub passed: bool,
    pub harness_id: Option<String>,
    pub harness_version: Option<String>,
    pub source_id: Option<String>,
    pub model_id: Option<String>,
    pub lane: Option<String>,
    pub jev_specificity: Option<String>,
    pub jev_confidence: Option<f64>,
    pub jev_lane: Option<String>,
    pub classifier_status: Option<String>,
    pub propensity: Option<f64>,
    pub tokens_in: Option<u64>,
    pub tokens_out: Option<u64>,
    pub tokens_cache_read: Option<u64>,
    pub tokens_cache_creation: Option<u64>,
    pub usd_list: Option<f64>,
    /// quota 窓 → 完了までの消費率（0..=1）。
    pub quota: BTreeMap<String, f64>,
    pub wall_s: Option<f64>,
    pub attempts: u32,
    pub check_failures: u32,
    pub review_rejections: u32,
    pub replans: u32,
    pub timed_out: bool,
    /// harness の並べ替えが先頭を変えた実行クラス。
    pub switched_classes: Vec<String>,
    pub celeris_version: String,
    pub completed_at: String,
}

impl CompletionOutcome {
    /// 失敗、または検査失敗・review 差し戻しが 1 回以上（guardrail の「悪い」）。
    pub fn bad(&self) -> bool {
        !self.passed || self.check_failures > 0 || self.review_rejections > 0
    }
}

/// quota 窓の重み（ADR D5。7d・1 か月 = 1.0、5h と他の窓 = 0.25）。
pub fn quota_window_weight(window: &str) -> f64 {
    match window {
        "seven_day" | "7d" | "one_month" | "1m" => 1.0,
        _ => 0.25,
    }
}

/// 消費率の窓別の値から、quota 加重コスト（重み付きの和）を出す。窓が無ければ `None`。
pub fn quota_weighted_cost(quota: &BTreeMap<String, f64>) -> Option<f64> {
    (!quota.is_empty()).then(|| {
        quota
            .iter()
            .map(|(window, used)| quota_window_weight(window) * used)
            .sum()
    })
}

// ---- 根拠表の更新（ADR 2026-10-22 D5/D7）と切替の guardrail（D2）----

use crate::route_select::{
    BalanceOutcome, BalanceTrip, evaluate_balance_guardrail, wilson_interval,
};
use crate::routing_evidence::{EvidenceKey, EvidenceRow};
use crate::store::{StoreError, TaskStore};

/// 重み `1 / propensity` の上限（探索の稀な選択が根拠表を支配しないように）。
pub const MAX_WEIGHT: f64 = 20.0;
/// 1 回の更新で読む葉の完了単位の上限（新しい順）。
pub const EVIDENCE_WINDOW: u32 = 5000;
/// 根拠表の版の接頭辞。版は文字列順で新しさを決めるので、時刻（RFC3339）を後ろに付ける。
pub const EVIDENCE_VERSION_PREFIX: &str = "completion-";

fn weight(o: &CompletionOutcome) -> f64 {
    o.propensity
        .filter(|p| *p > 0.0)
        .map_or(1.0, |p| (1.0 / p).clamp(1.0, MAX_WEIGHT))
}

fn p90(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    // nearest-rank
    let rank = ((values.len() as f64) * 0.9).ceil() as usize;
    values.get(rank.clamp(1, values.len()) - 1).copied()
}

/// 葉の完了単位を (genre, harness, harness 版 major, lane, source, model) で畳んで根拠表の行にする。
/// 重みは探索の propensity の逆数（上限 [`MAX_WEIGHT`]）。route・lane の分からない単位は使わない。
pub fn evidence_rows(outcomes: &[CompletionOutcome], version: &str) -> Vec<EvidenceRow> {
    let mut groups: BTreeMap<EvidenceKey, Vec<&CompletionOutcome>> = BTreeMap::new();
    for o in outcomes.iter().filter(|o| o.leaf) {
        let (Some(harness), Some(source), Some(model), Some(lane)) = (
            o.harness_id.as_ref(),
            o.source_id.as_ref(),
            o.model_id.as_ref(),
            o.lane.as_ref().filter(|l| !l.is_empty()),
        ) else {
            continue;
        };
        let version = o.harness_version.as_deref().unwrap_or_default();
        groups
            .entry(EvidenceKey {
                genre: o.genre.clone(),
                harness_id: harness.clone(),
                harness_version: crate::route_select::evidence::major_version(version).to_owned(),
                lane: lane.clone(),
                source_id: source.clone(),
                model_id: model.clone(),
            })
            .or_default()
            .push(o);
    }
    groups
        .into_iter()
        .map(|(key, items)| {
            let w_all: f64 = items.iter().map(|o| weight(o)).sum();
            let w_pass: f64 = items.iter().filter(|o| o.passed).map(|o| weight(o)).sum();
            let n = w_all.round().max(1.0) as u64;
            let pass = (w_pass.round() as u64).min(n);
            let interval = wilson_interval(pass, n, 1.96);
            let per_pass = |value: &dyn Fn(&CompletionOutcome) -> Option<f64>| -> Option<f64> {
                let known: Vec<_> = items
                    .iter()
                    .filter_map(|o| value(o).map(|v| (o, v)))
                    .collect();
                (!known.is_empty() && w_pass > 0.0)
                    .then(|| known.iter().map(|(o, v)| weight(o) * v).sum::<f64>() / w_pass)
            };
            let mut quota_per_run: BTreeMap<String, f64> = BTreeMap::new();
            for o in &items {
                for (window, used) in &o.quota {
                    *quota_per_run.entry(window.clone()).or_default() += weight(o) * used;
                }
            }
            for used in quota_per_run.values_mut() {
                *used /= w_all;
            }
            EvidenceRow {
                key,
                evidence_version: version.to_owned(),
                n,
                pass,
                pass_lower: interval.lower,
                pass_upper: interval.upper,
                pass_boot_lower: interval.lower,
                pass_boot_upper: interval.upper,
                wall_p90_s: p90(items.iter().filter_map(|o| o.wall_s).collect()),
                usd_per_pass: per_pass(&|o| o.usd_list),
                quota_cost_per_pass: per_pass(&|o| quota_weighted_cost(&o.quota)),
                quota_per_run,
                timeout_rate: items
                    .iter()
                    .filter(|o| o.timed_out)
                    .map(|o| weight(o))
                    .sum::<f64>()
                    / w_all,
                low_confidence: items.len() < 5,
            }
        })
        .collect()
}

/// 本番の完了単位から根拠表の新しい版を書く（daemon の定期処理）。書いた行数。葉の単位が無ければ書かない。
pub fn refresh_routing_evidence(store: &dyn TaskStore, now: &str) -> Result<usize, StoreError> {
    let outcomes = store.completion_outcomes_list(true, EVIDENCE_WINDOW)?;
    let version = format!("{EVIDENCE_VERSION_PREFIX}{now}");
    let rows = evidence_rows(&outcomes, &version);
    if rows.is_empty() {
        return Ok(0);
    }
    store.replace_routing_evidence(&version, &rows)?;
    Ok(rows.len())
}

/// 有効な各クラスの guardrail を判定し、止める条件に当たったクラスを `harness_balance_trips` に記録する。
/// 返すのは今回止めたクラスと判定の数値。既に止まっているクラスは判定しない。
pub fn evaluate_harness_balance(
    store: &dyn TaskStore,
    classes: &std::collections::BTreeSet<String>,
    window: u64,
    guard_min: u32,
    margin: f64,
    now: &str,
) -> Result<Vec<(String, BalanceTrip)>, StoreError> {
    let tripped = store.harness_balance_trips()?;
    let outcomes =
        store.completion_outcomes_list(false, (window.max(1) * 4).min(u32::MAX as u64) as u32)?;
    let mut out = Vec::new();
    for class in classes.iter().filter(|c| !tripped.contains_key(*c)) {
        let items: Vec<BalanceOutcome> = outcomes
            .iter()
            .map(|o| BalanceOutcome {
                switched: o.switched_classes.contains(class),
                bad: o.bad(),
            })
            .collect();
        if let Some(trip) = evaluate_balance_guardrail(&items, guard_min, margin) {
            let reason = format!(
                "切替した {} 件の悪い率 {:.2} が、しなかった {} 件の {:.2} を {margin:.2} 超えて悪化",
                trip.switched_n, trip.switched_bad_rate, trip.other_n, trip.other_bad_rate
            );
            store.harness_balance_trip(class, &reason, now)?;
            out.push((class.clone(), trip));
        }
    }
    Ok(out)
}
