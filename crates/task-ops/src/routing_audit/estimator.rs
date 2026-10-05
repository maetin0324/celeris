//! ADR 2026-10-04-multi-objective-model-routing Phase 5: estimator shadow（`kind = estimator`）の監査欄。
//!
//! llm-proxy の estimator shadow は Phase 4 の `routing_shadow_recorded` を再利用し、
//! `policy_version = estimator:<id>/<version>`、completed の `detail = <primary との比較>;heuristic:<heuristic との比較>`、
//! 失敗の `detail` に sidecar の理由語（`timeout`・`prompt_required` など）を入れる。ここではそれを
//! 読み分けて別欄に出す。primary の outcome・attempts・review には触れない。記録に無い値（descriptor の
//! network・外部 embeddings の依存）は推定せず `None` にする。自由文の `detail` は allowlist の語だけ写す。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::model_router::shadow::{ShadowKind, ShadowReason, ShadowRecord, ShadowStatus};

/// estimator shadow の `policy_version` の接頭辞（llm-proxy `EstimatorShadow::policy_version`）。
pub const ESTIMATOR_POLICY_PREFIX: &str = "estimator:";

/// `detail` から写してよい理由語（llm-proxy `SidecarUnavailable::reason` と kernel の除外）。
const UNAVAILABLE_CODES: &[&str] = &[
    "circuit_open",
    "max_inflight",
    "timeout",
    "transport",
    "http_status",
    "payload_too_large",
    "decode",
    "version_mismatch",
    "prompt_required",
    "dependencies_not_allowed",
    "dependency_mismatch",
    "invalid_estimate",
    "no_eligible_candidate",
];

/// estimator shadow 1 件の結果。primary の成否とは別。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EstimatorShadowOutcome {
    Completed,
    Failed,
    Timeout,
    Dropped,
    /// sidecar が prompt を要したが `send_prompt=false`（または prompt allowlist 外）で評価不能。
    PromptRequired,
}

/// estimator の選択と比較相手の関係。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EstimatorComparison {
    Same,
    Differs,
    /// estimator 側で選べる候補が無かった。
    NoCandidate,
}

/// descriptor の依存。記録から分かる値だけ `Some`（未記録は推定しない）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EstimatorDependencyAudit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_network: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_prompt: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_embeddings: Option<bool>,
    /// 宣言された依存が daemon の許可（network allowlist・privacy）に合わず送らなかった。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub not_allowed: bool,
}

/// `RoutingShadowAudit.estimator`（`kind = estimator` のときだけ）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EstimatorShadowAudit {
    /// `policy_version` が `estimator:<id>/<version>` のときの id と version。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimator_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimator_version: Option<String>,
    pub dependencies: EstimatorDependencyAudit,
    pub outcome: EstimatorShadowOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<ShadowReason>,
    /// 評価不能の理由語（allowlist のみ。自由文は出さない）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
    /// estimator の選択と heuristic primary（実際の primary 決定）の差。completed のみ。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vs_primary: Option<EstimatorComparison>,
    /// estimator の選択と、同じ候補集合での heuristic 首位の差。completed のみ。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vs_heuristic: Option<EstimatorComparison>,
    /// 推論 overhead（sidecar 往復 ms）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overhead_ms: Option<u64>,
}

/// estimator shadow の欄を記録から読む。`kind` が estimator 以外は `None`。
pub fn estimator_shadow_audit(record: &ShadowRecord) -> Option<EstimatorShadowAudit> {
    if record.kind != ShadowKind::Estimator {
        return None;
    }
    let (estimator_id, estimator_version) = parse_policy_version(&record.policy_version);
    let detail = record.detail.as_deref().unwrap_or("");
    let (vs_primary, vs_heuristic, unavailable_reason) = match record.status {
        ShadowStatus::Completed => {
            let mut parts = detail.split(';');
            let primary = parts.next().and_then(comparison);
            let heuristic = parts
                .find_map(|p| p.strip_prefix("heuristic:"))
                .and_then(comparison);
            (primary, heuristic, None)
        }
        ShadowStatus::Failed | ShadowStatus::Dropped => {
            let code = detail.split(|c: char| c == ':' || c.is_whitespace()).next();
            let code = code
                .filter(|c| UNAVAILABLE_CODES.contains(c))
                .map(str::to_string);
            (None, None, code)
        }
    };
    let code = unavailable_reason.as_deref();
    let outcome = match record.status {
        ShadowStatus::Completed => EstimatorShadowOutcome::Completed,
        _ if code == Some("prompt_required") => EstimatorShadowOutcome::PromptRequired,
        _ if record.reason == Some(ShadowReason::Timeout) || code == Some("timeout") => {
            EstimatorShadowOutcome::Timeout
        }
        ShadowStatus::Failed => EstimatorShadowOutcome::Failed,
        ShadowStatus::Dropped => EstimatorShadowOutcome::Dropped,
    };
    let dependencies = EstimatorDependencyAudit {
        needs_prompt: (code == Some("prompt_required")).then_some(true),
        not_allowed: matches!(
            code,
            Some("dependencies_not_allowed" | "dependency_mismatch")
        ),
        ..EstimatorDependencyAudit::default()
    };
    Some(EstimatorShadowAudit {
        estimator_id,
        estimator_version,
        dependencies,
        outcome,
        reason: record.reason,
        unavailable_reason,
        vs_primary,
        vs_heuristic,
        overhead_ms: record.latency_ms,
    })
}

fn parse_policy_version(policy_version: &str) -> (Option<String>, Option<String>) {
    let Some(rest) = policy_version.strip_prefix(ESTIMATOR_POLICY_PREFIX) else {
        return (None, None);
    };
    match rest.rsplit_once('/') {
        Some((id, version)) if !id.is_empty() && !version.is_empty() => {
            (Some(id.to_string()), Some(version.to_string()))
        }
        _ => (None, None),
    }
}

fn comparison(code: &str) -> Option<EstimatorComparison> {
    match code {
        "same_as_primary" => Some(EstimatorComparison::Same),
        "differs_from_primary" => Some(EstimatorComparison::Differs),
        "no_candidate" => Some(EstimatorComparison::NoCandidate),
        _ => None,
    }
}

/// task の estimator shadow の要約（run を跨いで数える）。対象が 0 件なら作らない。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EstimatorShadowSummary {
    /// estimator shadow の記録数（評価の対象）。
    pub targets: u64,
    pub completed: u64,
    pub failed: u64,
    pub timeout: u64,
    pub dropped: u64,
    pub prompt_required: u64,
    /// completed / targets。
    pub coverage: f64,
    /// completed のうち heuristic primary と選択が違った件数。
    pub differs_from_primary: u64,
    /// completed のうち heuristic 首位と選択が違った件数。
    pub differs_from_heuristic: u64,
    /// overhead の記録がある件の平均（ms）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean_overhead_ms: Option<f64>,
    /// 記録に現れた estimator（`<id>/<version>`、重複なし・昇順）。
    pub estimators: Vec<String>,
}

/// estimator shadow の監査から要約を作る。
pub fn estimator_shadow_summary<'a>(
    shadows: impl IntoIterator<Item = &'a EstimatorShadowAudit>,
) -> Option<EstimatorShadowSummary> {
    let mut s = EstimatorShadowSummary::default();
    let mut overhead = Vec::new();
    for e in shadows {
        s.targets += 1;
        match e.outcome {
            EstimatorShadowOutcome::Completed => s.completed += 1,
            EstimatorShadowOutcome::Failed => s.failed += 1,
            EstimatorShadowOutcome::Timeout => s.timeout += 1,
            EstimatorShadowOutcome::Dropped => s.dropped += 1,
            EstimatorShadowOutcome::PromptRequired => s.prompt_required += 1,
        }
        if e.vs_primary == Some(EstimatorComparison::Differs) {
            s.differs_from_primary += 1;
        }
        if e.vs_heuristic == Some(EstimatorComparison::Differs) {
            s.differs_from_heuristic += 1;
        }
        overhead.extend(e.overhead_ms);
        if let (Some(id), Some(v)) = (&e.estimator_id, &e.estimator_version) {
            s.estimators.push(format!("{id}/{v}"));
        }
    }
    if s.targets == 0 {
        return None;
    }
    s.coverage = s.completed as f64 / s.targets as f64;
    if !overhead.is_empty() {
        s.mean_overhead_ms = Some(overhead.iter().sum::<u64>() as f64 / overhead.len() as f64);
    }
    s.estimators.sort();
    s.estimators.dedup();
    Some(s)
}
