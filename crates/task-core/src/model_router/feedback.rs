//! ADR §6（Phase 3）: routing の feature・要求・結果（outcome）の event schema と、task の events から
//! run ごとの outcome を決定的に投影する純粋関数。
//!
//! - `routing_features_recorded`: 決定 1 件の特徴 snapshot（本文・secret を持たない）。
//! - `routing_request_decided`: proxy の要求 1 件の trace（run 側 decision を親に持つ）。
//! - `routing_outcome_recorded`: run 1 件の結果 vector と offline 比較用 scalar の reward。
//!
//! 合否は既存の reviewer・決定的検査の event だけから読む（worker の自己申告は使わない）。未レビュー・
//! 中断は `None`（false や 0 にしない）。同じ入力の再投影は同じ `outcome_id`、後の訂正は新しい
//! `outcome_id` + `supersedes`。run の合否を個々の要求（request）に複写しない。

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::trace::RoutingTraceV1;
use crate::execution::{HarnessErrorClass, RunEnd};
use crate::model::{Event, RunRole};

/// `routing_features_recorded.context_version` の現行値（§3.4 の RoutingContext の版。lane-policy とは独立）。
pub const ROUTING_CONTEXT_VERSION: &str = "routing-context/1";
/// `routing_outcome_recorded.evaluation_version` の現行値（下の投影規則と reward 式の版）。
pub const ROUTING_OUTCOME_EVALUATION_VERSION: &str = "routing-outcome/1";

/// 特徴を記録した時点。run の時点と proxy の更新時点を区別し、後の review 結果を当時の特徴に混ぜない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FeatureStage {
    Dispatch,
    Proxy,
}

/// `Event::RoutingFeaturesRecorded` の中身（wire では event の欄に平たく並ぶ）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RoutingFeaturesRecord {
    pub decision_id: String,
    pub context_version: String,
    /// §3.4 の特徴（本文・生の command・credential を除いた snapshot）。形は `context_version` が決める。
    #[serde(default)]
    pub features: serde_json::Value,
    /// 欄名 → 出自（例 `task.acceptance`、`work_unit.spec`、`estimator:chars/4`）。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub provenance: BTreeMap<String, String>,
    /// 取れなかった欄（未知を低リスクと同一視しないため、欠測を明示する）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing_fields: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<FeatureStage>,
}

/// proxy が要求 1 件で実際に試した source の参照（secret を含まない ID だけ）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RequestSourceAttempt {
    pub source_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    /// 次の候補へ送った理由（§6 の reason code。例 `rate_limit`・`health_down`）。最後に使った source は `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
}

/// `Event::RoutingRequestDecided` の中身（wire では event の欄に平たく並ぶ）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RoutingRequestRecord {
    /// `llm_proxy_requests` と結合する要求 ID。
    pub request_id: String,
    /// この要求の proxy 側 decision。
    pub decision_id: String,
    /// run 側 decision（dispatch の `RoutingTraceV1.decision_id`）。standalone は `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_decision_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// proxy の trace（stage = proxy）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace: Option<Box<RoutingTraceV1>>,
    /// 試した順の source。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attempts: Vec<RequestSourceAttempt>,
    /// 最終の source へ落ちた原因（fallback が無ければ `None`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
}

/// `Event::RoutingOutcomeRecorded` の中身（wire では event の欄に平たく並ぶ）。生の outcome vector が正本で、
/// `reward` は offline 比較用の派生値。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RoutingOutcome {
    pub outcome_id: String,
    pub decision_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub evaluation_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    #[serde(default)]
    pub acceptance_passed: Option<bool>,
    #[serde(default)]
    pub review_passed: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed_criterion_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cash_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retries: Option<u32>,
    #[serde(default)]
    pub reward: Option<f64>,
}

/// reward の C・L を正規化する参照値（同じ評価版の中では固定する）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RewardNormalization {
    /// C_actual = min(cash_usd / cash_ref_usd, 1)。
    pub cash_ref_usd: f64,
    /// L_actual = min(wall_ms / wall_ref_ms, 1)。
    pub wall_ref_ms: u64,
}

impl Default for RewardNormalization {
    fn default() -> Self {
        Self {
            cash_ref_usd: 1.0,
            wall_ref_ms: 3_600_000,
        }
    }
}

/// §6 の初期 scalar: `R = pass - 0.2*C - 0.1*L - 0.1*min(retries/4, 1)`。pass・cash・wall・retries の
/// どれかが欠ければ `None`（未判定・欠測を 0 とみなさない）。
pub fn routing_reward(
    pass: Option<bool>,
    cash_usd: Option<f64>,
    wall_ms: Option<u64>,
    retries: Option<u32>,
    norm: &RewardNormalization,
) -> Option<f64> {
    let pass = if pass? { 1.0 } else { 0.0 };
    let ratio = |v: f64, r: f64| {
        if r > 0.0 {
            (v / r).clamp(0.0, 1.0)
        } else {
            1.0
        }
    };
    let c = ratio(cash_usd?, norm.cash_ref_usd);
    let l = ratio(wall_ms? as f64, norm.wall_ref_ms as f64);
    let r = (f64::from(retries?) / 4.0).min(1.0);
    Some(pass - 0.2 * c - 0.1 * l - 0.1 * r)
}

#[derive(Default)]
struct RunFacts {
    decision_id: Option<String>,
    finished: bool,
    end: Option<RunEnd>,
    acceptance_passed: Option<bool>,
    review_passed: Option<bool>,
    failed_criteria: BTreeSet<String>,
    cash_usd: Option<f64>,
    tokens: Option<u64>,
    wall_ms: Option<u64>,
    retries: Option<u32>,
}

/// 欠測（`None`）は足し算の単位元として扱うが、両方欠測なら `None` のまま（0 とみなさない）。
fn add_opt<T: std::ops::Add<Output = T>>(a: Option<T>, b: Option<T>) -> Option<T> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a + b),
        (a, b) => a.or(b),
    }
}

/// 中断・供給側の終わり方の安定 code（低品質の証拠ではない）。品質の失敗は `acceptance_failed` /
/// `review_failed` が優先する。
fn end_class(end: RunEnd) -> Option<&'static str> {
    match end {
        RunEnd::Completed => None,
        RunEnd::Yielded => Some("yielded"),
        RunEnd::BudgetExhausted { .. } => Some("budget_exhausted"),
        RunEnd::Question => Some("question"),
        RunEnd::Failed { .. } => Some("failed"),
        RunEnd::HarnessError { class } => Some(match class {
            HarnessErrorClass::Supply => "supply_side",
            HarnessErrorClass::Infra => "infra",
            HarnessErrorClass::LeaseExpired => "lease_expired",
            HarnessErrorClass::IdleTimeout => "idle_timeout",
        }),
        RunEnd::Cancelled => Some("cancelled"),
        RunEnd::Waiting => Some("waiting"),
    }
}

/// outcome の中身（`outcome_id` を除く。`supersedes` は含む）から ID を作る。同じ入力なら同じ ID。
fn outcome_id_for(o: &RoutingOutcome) -> String {
    let mut body = o.clone();
    body.outcome_id = String::new();
    let canonical = serde_json::to_string(&body).unwrap_or_default();
    let digest = Sha256::digest(canonical.as_bytes());
    let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!("outcome-{hex}")
}

/// 同じ結果か（ID・supersedes を除いて比べる）。
fn same_result(a: &RoutingOutcome, b: &RoutingOutcome) -> bool {
    let strip = |o: &RoutingOutcome| RoutingOutcome {
        outcome_id: String::new(),
        supersedes: None,
        ..o.clone()
    };
    strip(a) == strip(b)
}

/// task の events（古い順）から、worker run ごとの現在の outcome を投影する（古い run が先）。
///
/// - 対象は `RoutingDecided` を持ち、`WorkerFinished` まで届いた worker run（reviewer run は除く）。
/// - review の判定（`Transitioned` の `review_pass` / `review_fail`）は直前の worker run に帰属させる。
///   届いていなければ `review_passed = None`。
/// - WU の受け入れ検査（`WorkUnitCheckFinished` / `WorkUnitChecksFailed`）は `run_id` で帰属させる。
/// - pass は review があれば review、無ければ受け入れ検査の不合格だけを確定とみなす（合格だけでは未判定）。
/// - events に既にある `RoutingOutcomeRecorded`（同じ run・decision の最新）と結果が同じなら、それをそのまま
///   返す（同じ `outcome_id`）。違えば新しい `outcome_id` と `supersedes` を付ける。
/// - `request_id` は常に `None`（run の合否を個々の要求に複写しない）。
pub fn project_run_outcomes(events: &[Event], norm: &RewardNormalization) -> Vec<RoutingOutcome> {
    // worker run（reviewer を除く）の初出順。
    let mut order: Vec<String> = Vec::new();
    let mut runs: BTreeMap<String, RunFacts> = BTreeMap::new();
    let mut last_worker: Option<String> = None;
    let mut failed_review: BTreeSet<String> = BTreeSet::new();
    let mut recorded: BTreeMap<(String, String), RoutingOutcome> = BTreeMap::new();
    for event in events {
        match event {
            Event::WorkerStarted {
                run_id, role: None, ..
            } => {
                if !order.contains(run_id) {
                    order.push(run_id.clone());
                }
                runs.entry(run_id.clone()).or_default();
                last_worker = Some(run_id.clone());
                failed_review.clear();
            }
            Event::RoutingDecided { run_id, record } => {
                let f = runs.entry(run_id.clone()).or_default();
                f.decision_id = Some(
                    record
                        .optimizer
                        .as_ref()
                        .map(|t| t.decision_id.clone())
                        .unwrap_or_else(|| format!("run:{run_id}")),
                );
            }
            Event::WorkerFinished {
                run_id,
                usage,
                role,
                metrics,
                end,
                ..
            } if *role != Some(RunRole::Reviewer) => {
                let f = runs.entry(run_id.clone()).or_default();
                f.finished = true;
                f.end = *end;
                // run に合算する（同じ run の複数の完了記録を足す）。欠測同士は欠測のままにする。
                if let Some(u) = usage {
                    f.cash_usd = add_opt(f.cash_usd, u.cost_usd);
                    let tokens = match (u.input_tokens, u.output_tokens) {
                        (None, None) => None,
                        (i, o) => Some(i.unwrap_or(0) + o.unwrap_or(0)),
                    };
                    f.tokens = add_opt(f.tokens, tokens);
                }
                if let Some(m) = metrics {
                    f.wall_ms = add_opt(f.wall_ms, Some(m.wall_ms));
                    f.retries = add_opt(f.retries, Some(m.retries));
                }
            }
            Event::WorkUnitCheckFinished {
                run_id,
                pass,
                index,
                ..
            } => {
                let f = runs.entry(run_id.clone()).or_default();
                if *pass {
                    f.acceptance_passed.get_or_insert(true);
                } else {
                    f.acceptance_passed = Some(false);
                    f.failed_criteria.insert(format!("check:{index}"));
                }
            }
            Event::WorkUnitChecksFailed { run_id, .. } => {
                runs.entry(run_id.clone()).or_default().acceptance_passed = Some(false);
            }
            Event::ReviewVerdict {
                criterion_idx,
                pass: false,
                ..
            } => {
                failed_review.insert(format!("acceptance:{criterion_idx}"));
            }
            Event::Transitioned { reason, .. }
                if reason == "review_pass" || reason == "review_fail" =>
            {
                if let Some(run_id) = &last_worker {
                    let f = runs.entry(run_id.clone()).or_default();
                    f.review_passed = Some(reason == "review_pass");
                    f.failed_criteria.extend(std::mem::take(&mut failed_review));
                }
            }
            Event::RoutingOutcomeRecorded { outcome } => {
                if let Some(run_id) = &outcome.run_id
                    && outcome.request_id.is_none()
                {
                    recorded.insert(
                        (run_id.clone(), outcome.decision_id.clone()),
                        (**outcome).clone(),
                    );
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for run_id in order {
        let Some(f) = runs.get(&run_id) else { continue };
        let Some(decision_id) = f.decision_id.clone() else {
            continue;
        };
        if !f.finished {
            continue;
        }
        let pass = match (f.review_passed, f.acceptance_passed) {
            (_, Some(false)) => Some(false),
            (Some(r), _) => Some(r),
            _ => None,
        };
        let failure_class = if f.acceptance_passed == Some(false) {
            Some("acceptance_failed".to_string())
        } else if f.review_passed == Some(false) {
            Some("review_failed".to_string())
        } else {
            f.end.and_then(end_class).map(str::to_string)
        };
        let mut outcome = RoutingOutcome {
            outcome_id: String::new(),
            decision_id: decision_id.clone(),
            run_id: Some(run_id.clone()),
            request_id: None,
            evaluation_version: ROUTING_OUTCOME_EVALUATION_VERSION.to_string(),
            supersedes: None,
            acceptance_passed: f.acceptance_passed,
            review_passed: f.review_passed,
            failed_criterion_ids: f.failed_criteria.iter().cloned().collect(),
            failure_class,
            cash_usd: f.cash_usd,
            tokens: f.tokens,
            wall_ms: f.wall_ms,
            retries: f.retries,
            reward: routing_reward(pass, f.cash_usd, f.wall_ms, f.retries, norm),
        };
        match recorded.get(&(run_id, decision_id)) {
            Some(prev) if same_result(prev, &outcome) => outcome = prev.clone(),
            Some(prev) => {
                outcome.supersedes = Some(prev.outcome_id.clone());
                outcome.outcome_id = outcome_id_for(&outcome);
            }
            None => outcome.outcome_id = outcome_id_for(&outcome),
        }
        out.push(outcome);
    }
    out
}

/// まだ追記されていない outcome（投影のうち、events に同じ `outcome_id` が無いもの）。冪等に追記する入口。
pub fn pending_run_outcomes(events: &[Event], norm: &RewardNormalization) -> Vec<RoutingOutcome> {
    let known: BTreeSet<&str> = events
        .iter()
        .filter_map(|e| match e {
            Event::RoutingOutcomeRecorded { outcome } => Some(outcome.outcome_id.as_str()),
            _ => None,
        })
        .collect();
    project_run_outcomes(events, norm)
        .into_iter()
        .filter(|o| !known.contains(o.outcome_id.as_str()))
        .collect()
}

#[cfg(test)]
mod tests;
