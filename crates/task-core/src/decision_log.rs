//! 判断・実行・成果・消費の最小一貫ログの型（ADR 2026-10-15-decision-log-export D1〜D7）。
//!
//! 既存の `routing_decided`（[`RoutingRecord`]）・`RunEnd`・`Usage` を読み元にし、足りない欄だけを持つ。
//! 提案（`proposed_*`）と実行（`exec_*`）は別の型・別の欄で、結果と消費は実行の側にだけ付く。
//! 未判定・欠測は `None`（`false` / `0` にしない）。
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::execution::{BudgetKind, RunEnd};
use crate::model_policy::RoutingRecord;

/// 候補 1 つ（source × model × harness）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CandidateRef {
    pub source_id: String,
    pub model_id: String,
    pub harness_id: String,
}

/// 除外された候補と理由。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExclusionRef {
    pub candidate: CandidateRef,
    pub reason: String,
}

/// 要求分類器の出力（D4）。`confidence` は分類器自身の確信であり、成功確率でも propensity でもない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ClassifierRecord {
    /// `rule` | `jev` | `human_hint`。
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    /// `{option: p}`（各選択肢の確率。これも分類の確信であって方策の確率ではない）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probs: Option<serde_json::Value>,
    /// Jev 失敗で fixed に戻った理由（`timeout` など）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
}

/// 提案の群（判断時点）。`run_id` が主キー。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DecisionLogProposal {
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub campaign_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_unit_id: Option<String>,
    /// retry・continuation・repair の元 run。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<String>,
    /// `RunMetrics.retries`（開始時点の `Task.attempts`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_no: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_class: Option<String>,
    /// `fixed` | `decision_assisted`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classifier: Option<ClassifierRecord>,
    /// 実行方策がその候補を選んだ確率（`RouteDecision.propensity`）。分類の確信では埋めない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub propensity: Option<f64>,
    /// 規則表が出した lane（clamp 前）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed_lane: Option<String>,
    /// clamp 後の lane。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_lane: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proposed_candidates: Vec<CandidateRef>,
    /// 最初の推奨候補。結果・消費はここに付かない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed_first: Option<CandidateRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclusions: Vec<ExclusionRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question_schema_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_model_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options_hash: Option<String>,
}

impl DecisionLogProposal {
    /// 旧い `routing_decided` event（新欄なし）からも作れる。無い欄は `None` のまま。
    pub fn from_routing_record(run_id: &str, task_id: &str, record: &RoutingRecord) -> Self {
        let lane = |t: crate::model::Tier| match t {
            crate::model::Tier::Cheap => "cheap",
            crate::model::Tier::Standard => "standard",
            crate::model::Tier::Frontier => "frontier",
        };
        let decision = record.features.as_ref().and_then(|f| f.decision.as_deref());
        Self {
            run_id: run_id.to_string(),
            decision_id: decision.map(|d| d.decision_id.clone()),
            task_id: Some(task_id.to_string()),
            work_unit_id: record.work_unit_id.clone(),
            proposed_lane: Some(lane(record.decision.proposed).into()),
            decided_lane: Some(lane(record.decision.lane).into()),
            policy_version: Some(record.decision.policy_version.clone()),
            features_hash: record.features.as_ref().map(|f| f.hash.clone()),
            options_hash: decision.and_then(|d| d.options_hash.clone()),
            propensity: decision.map(|d| d.propensity),
            proposed_candidates: decision
                .map(|d| {
                    d.candidates
                        .iter()
                        .map(|c| CandidateRef {
                            source_id: c.source_id.clone(),
                            model_id: c.model_id.clone(),
                            harness_id: c.harness_id.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            ..Self::default()
        }
    }
}

/// 実行の群（実際に走ったもの）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DecisionLogExecution {
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_source_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_adapter: Option<String>,
    /// 提案先頭と違う候補で走ったときの理由（`rate_limit` など）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    /// 実際に試した列（`RequestSourceAttempt[]`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempts: Option<serde_json::Value>,
}

/// run 終了の別ラベル（D3）。`timeout` は壁時計切れの別名（元の値は `run_end_raw`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunEndLabel {
    Completed,
    Yielded,
    BudgetExhausted,
    Question,
    Failed,
    HarnessError,
    Cancelled,
    Waiting,
    Timeout,
}

impl RunEndLabel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Yielded => "yielded",
            Self::BudgetExhausted => "budget_exhausted",
            Self::Question => "question",
            Self::Failed => "failed",
            Self::HarnessError => "harness_error",
            Self::Cancelled => "cancelled",
            Self::Waiting => "waiting",
            Self::Timeout => "timeout",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "completed" => Self::Completed,
            "yielded" => Self::Yielded,
            "budget_exhausted" => Self::BudgetExhausted,
            "question" => Self::Question,
            "failed" => Self::Failed,
            "harness_error" => Self::HarnessError,
            "cancelled" => Self::Cancelled,
            "waiting" => Self::Waiting,
            "timeout" => Self::Timeout,
            _ => return None,
        })
    }

    /// `RunEnd` からのラベルと、元の値（`run_end_raw`）。
    pub fn from_run_end(end: RunEnd) -> (Self, String) {
        let raw = match serde_json::to_value(end) {
            Ok(v) => v.to_string(),
            Err(_) => String::new(),
        };
        let label = match end {
            RunEnd::Completed => Self::Completed,
            RunEnd::Yielded => Self::Yielded,
            RunEnd::BudgetExhausted {
                kind: BudgetKind::WallClock,
            } => Self::Timeout,
            RunEnd::BudgetExhausted { .. } => Self::BudgetExhausted,
            RunEnd::Question => Self::Question,
            RunEnd::Failed { .. } => Self::Failed,
            RunEnd::HarnessError { .. } => Self::HarnessError,
            RunEnd::Cancelled => Self::Cancelled,
            RunEnd::Waiting => Self::Waiting,
        };
        (label, raw)
    }
}

/// WorkUnit 検証の結果（決定的 check / reviewer が決める。worker の自己申告のみは `Unverified`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WuVerdict {
    Passed,
    Failed,
    Unverified,
}

impl WuVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Unverified => "unverified",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "passed" => Self::Passed,
            "failed" => Self::Failed,
            "unverified" => Self::Unverified,
            _ => return None,
        })
    }

    /// `RoutingOutcome.acceptance_passed` / `review_passed` からの導出。どちらも未判定なら `Unverified`。
    pub fn derive(acceptance_passed: Option<bool>, review_passed: Option<bool>) -> Self {
        match (acceptance_passed, review_passed) {
            (Some(false), _) | (_, Some(false)) => Self::Failed,
            (Some(true), Some(true)) | (Some(true), None) | (None, Some(true)) => Self::Passed,
            (None, None) => Self::Unverified,
        }
    }
}

/// campaign 合否（独立 checker が問題単位で付ける）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CampaignVerdict {
    Pass,
    Fail,
    Unscored,
}

impl CampaignVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Unscored => "unscored",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "pass" => Self::Pass,
            "fail" => Self::Fail,
            "unscored" => Self::Unscored,
            _ => return None,
        })
    }
}

/// 成果の群。3 階層の合否は別欄で、互いの代用にしない。`None` = 未判定。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DecisionLogOutcome {
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_end: Option<RunEndLabel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_end_raw: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_retryable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wu_verdict: Option<WuVerdict>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wu_failure_class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub campaign_verdict: Option<CampaignVerdict>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_ms: Option<u64>,
}

impl DecisionLogOutcome {
    /// run 終了だけを記録する（WU 検証・campaign 合否は `None` のまま）。
    pub fn from_run_end(run_id: &str, end: RunEnd, wall_ms: Option<u64>) -> Self {
        let (label, raw) = RunEndLabel::from_run_end(end);
        Self {
            run_id: run_id.to_string(),
            run_end: Some(label),
            run_end_raw: Some(raw),
            run_retryable: match end {
                RunEnd::Failed { retryable } => Some(retryable),
                _ => None,
            },
            wall_ms,
            ..Self::default()
        }
    }
}

/// 保存された 1 run の全体（提案・実行・成果）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DecisionLogRun {
    pub proposal: DecisionLogProposal,
    pub execution: DecisionLogExecution,
    pub outcome: DecisionLogOutcome,
}

impl DecisionLogRun {
    /// 実行が提案先頭と同じか。どちらかが未記録なら `None`。
    pub fn exec_equals_proposed_first(&self) -> Option<bool> {
        let first = self.proposal.proposed_first.as_ref()?;
        let (s, m) = (
            self.execution.exec_source_id.as_ref()?,
            self.execution.exec_model.as_ref()?,
        );
        Some(&first.source_id == s && &first.model_id == m)
    }

    pub fn is_retry(&self) -> bool {
        self.proposal.parent_run_id.is_some()
    }
}

/// 消費行の役割（D6）。`retry` は役割ではなく `is_retry` で表す。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UsageRole {
    Cos,
    Planner,
    Worker,
    Reviewer,
    Jev,
}

impl UsageRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cos => "cos",
            Self::Planner => "planner",
            Self::Worker => "worker",
            Self::Reviewer => "reviewer",
            Self::Jev => "jev",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "cos" => Self::Cos,
            "planner" => Self::Planner,
            "worker" => Self::Worker,
            "reviewer" => Self::Reviewer,
            "jev" => Self::Jev,
            _ => return None,
        })
    }
}

/// 消費行の冪等キー（D3）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UsageKey {
    /// run 由来: `(run_id, usage_source)`。
    Run {
        run_id: String,
        usage_source: String,
    },
    /// proxy 外の呼び出し（Jev など）: `(decision_id, call_attempt)`。
    Call {
        decision_id: String,
        call_attempt: u32,
    },
}

impl UsageKey {
    pub fn dedup_key(&self) -> String {
        match self {
            Self::Run {
                run_id,
                usage_source,
            } => format!("run:{run_id}:{usage_source}"),
            Self::Call {
                decision_id,
                call_attempt,
            } => format!("decision:{decision_id}:{call_attempt}"),
        }
    }
}

/// 消費 1 行。数値欄の `None` は欠測、`Some(0)` は 0 と観測した。区分（token・定価換算・実支出・
/// 計算資源・壁時計）は別欄で、互いに加算・代用しない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DecisionLogUsage {
    pub key: UsageKey,
    pub role: UsageRole,
    #[serde(default)]
    pub is_retry: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub campaign_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<u64>,
    /// 単価表からの定価換算（推定。実支出ではない）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_cost_usd: Option<f64>,
    /// 実請求（取れなければ `None`。`list_cost_usd` で代用しない）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cash_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_wait_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_run_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_node_hours: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_queue_ms: Option<u64>,
}

impl DecisionLogUsage {
    /// 空の消費行（全欄が欠測）。
    pub fn new(key: UsageKey, role: UsageRole) -> Self {
        Self {
            key,
            role,
            is_retry: false,
            decision_id: None,
            run_id: None,
            campaign_id: None,
            task_id: None,
            input_tokens: None,
            output_tokens: None,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            list_cost_usd: None,
            cash_cost_usd: None,
            wall_ms: None,
            cluster_wait_ms: None,
            cluster_run_ms: None,
            cluster_node_hours: None,
            cluster_queue_ms: None,
        }
    }
}

/// 人の介入の区分（D7）。分類できないものは `Unclassified`（推測で 3 区分に入れない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InterventionKind {
    /// 研究の方針を人が決めた。
    ResearchJudgment,
    /// 人が作業を引き継いで実行した。
    ManualHandoff,
    /// 障害・不具合から戻すための人手。
    Recovery,
    Unclassified,
}

impl InterventionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ResearchJudgment => "research_judgment",
            Self::ManualHandoff => "manual_handoff",
            Self::Recovery => "recovery",
            Self::Unclassified => "unclassified",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "research_judgment" => Self::ResearchJudgment,
            "manual_handoff" => Self::ManualHandoff,
            "recovery" => Self::Recovery,
            "unclassified" => Self::Unclassified,
            _ => return None,
        })
    }
}

/// 介入 1 件。`intervention_id` は元 event の `seq` 由来で冪等。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DecisionLogIntervention {
    pub intervention_id: String,
    pub kind: InterventionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub campaign_id: Option<String>,
    pub at: String,
    /// 人の所要時間（取れなければ `None`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_event: Option<String>,
}
