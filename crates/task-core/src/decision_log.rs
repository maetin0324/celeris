//! 判断・実行・成果・消費の最小一貫ログの型（ADR 2026-10-15-decision-log-export D1〜D7）。
//!
//! 既存の `routing_decided`（[`RoutingRecord`]）・`RunEnd`・`Usage` を読み元にし、足りない欄だけを持つ。
//! 提案（`proposed_*`）と実行（`exec_*`）は別の型・別の欄で、結果と消費は実行の側にだけ付く。
//! 未判定・欠測は `None`（`false` / `0` にしない）。
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::execution::{BudgetKind, RunEnd};
use crate::model_policy::RoutingRecord;
use crate::route_select::{ClassifierStatus, LaneAssistRecord};
pub use crate::variant_wire::{CampaignOrgPin, CampaignVariantRef};

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
    /// 分類の応答が名乗った model 名（`ClassificationEntry::model_snapshot` の値）。設定の model 名ではない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 応答 model 名とは独立に観測した版。観測していなければ None（`model` から複製しない）。
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

/// campaign 識別（ADR 2026-10-16-campaign-identity-propagation）。campaign の root task に 1 つ付き、
/// 子孫 task・全 run（再試行・continuation・repair・reviewer・planner）が祖先の鎖から継ぐ。
/// `routing_setting` は試行単位で固定した routing 設定（全体 config の `[routing] mode` とは別）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CampaignIdentity {
    pub campaign_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repetition: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_hash: Option<String>,
    /// `fixed` | `decision_assisted`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing_setting: Option<String>,
    /// 比較 variant の参照（campaign `VariantRef` と同じ JSON 形）。`variant_org` と対で付く。
    /// 無い識別（従来の行）は variant 無しの試行のまま。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<CampaignVariantRef>,
    /// variant が前提とする検証済みの構成 pin（campaign `OrgPin` と同じ JSON 形）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant_org: Option<CampaignOrgPin>,
}

impl CampaignIdentity {
    pub fn validate(&self) -> Result<(), String> {
        if self.campaign_id.trim().is_empty() {
            return Err("campaign_id must not be empty".into());
        }
        if let Some(m) = &self.routing_setting
            && !matches!(m.as_str(), "fixed" | "decision_assisted")
        {
            return Err("campaign.routing_setting must be `fixed` or `decision_assisted`".into());
        }
        match (&self.variant, &self.variant_org) {
            (None, None) => {}
            (Some(v), Some(org)) => {
                v.validate().map_err(|e| format!("campaign.{e}"))?;
                org.validate().map_err(|e| format!("campaign.{e}"))?;
            }
            _ => {
                return Err(
                    "campaign.variant and campaign.variant_org must be given together".into(),
                );
            }
        }
        Ok(())
    }
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
    /// campaign 識別の残り（`campaign_id` / `arm` は上の欄）。campaign 無しは全て `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repetition: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing_setting: Option<String>,
    /// campaign 識別の variant（arm の分割用。id と内容 hash だけ写す）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant_content_sha256: Option<String>,
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
    /// 分類段の状態（`jev_ok` | `fell_back_to_fixed` | `not_attempted`）。`classifier`（kind 等）の有無とは別で、
    /// 分類を試みなかった run も `not_attempted` として残る。lane 層の記録が無い run は None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classifier_status: Option<String>,
    /// 分類の呼び出し ID（`jev_call_id`。消費行の鍵と同じ）。同じ分類を複数 run が使えば同じ値になる。
    /// `attempt_no`（run の試行番号）や call_no とは別。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classifier_call_id: Option<String>,
    /// fixed 規則だけの lane（`replan` を含む）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_lane: Option<String>,
    /// Jev 単独の提案 lane（fixed と合成する前）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jev_lane: Option<String>,
    /// lane 層が許可・天井・利用可否で制約した後の lane（`decided_lane` は既存方策の lane で別欄）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constrained_lane: Option<String>,
    /// lane の帰属（`fixed_rule` | `classification_raise` | `fixed_fallback`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane_basis: Option<String>,
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
    /// campaign 識別を欄へ写す（識別が `None` なら何も変えない）。
    pub fn apply_campaign(&mut self, id: &CampaignIdentity) {
        self.campaign_id = Some(id.campaign_id.clone());
        self.case_id = id.case_id.clone();
        self.arm = id.arm.clone();
        self.repetition = id.repetition;
        self.manifest_hash = id.manifest_hash.clone();
        self.routing_setting = id.routing_setting.clone();
        self.variant_id = id.variant.as_ref().map(|v| v.id.clone());
        self.variant_content_sha256 = id.variant.as_ref().map(|v| v.content_sha256.clone());
    }

    /// 旧い `routing_decided` event（新欄なし）からも作れる。無い欄は `None` のまま。
    pub fn from_routing_record(run_id: &str, task_id: &str, record: &RoutingRecord) -> Self {
        let lane = |t: crate::model::Tier| match t {
            crate::model::Tier::Cheap => "cheap",
            crate::model::Tier::Standard => "standard",
            crate::model::Tier::Frontier => "frontier",
        };
        let decision = record.features.as_ref().and_then(|f| f.decision.as_deref());
        let mut p = Self {
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
        };
        if let Some(a) = decision.and_then(|d| d.lane_assist.as_ref()) {
            p.apply_lane_assist(a);
        }
        p
    }

    /// lane 層の記録（`routing_decided` の `lane_assist`）を欄へ写す純粋な投影。記録に無い値は None のまま
    /// （設定の model 名・0・空文字で補わない）。`NotAttempted` は分類器の記録（kind）を作らない。
    pub fn apply_lane_assist(&mut self, a: &LaneAssistRecord) {
        let (status, fallback) = match &a.classifier_status {
            ClassifierStatus::JevOk => ("jev_ok", None),
            ClassifierStatus::FellBackToFixed { reason } => {
                ("fell_back_to_fixed", Some(reason.clone()))
            }
            ClassifierStatus::NotAttempted => ("not_attempted", None),
        };
        self.classifier_status = Some(status.into());
        self.classifier = (status != "not_attempted").then(|| ClassifierRecord {
            kind: "jev".into(),
            model: a.response_model.clone(),
            snapshot: a.observed_snapshot.clone(),
            confidence: None,
            probs: None,
            fallback,
        });
        self.classifier_call_id = a.call_id.clone();
        self.fixed_lane = a.fixed_lane.clone();
        self.jev_lane = a.jev_lane.clone();
        self.constrained_lane = Some(a.final_lane.clone());
        self.lane_basis = Some(a.basis.clone());
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
    /// CoS context, joined from the recorded chat run and its session at read time.
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub thread_kind: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// Harness session id from the run's node_sessions row (not the active session).
    #[serde(default)]
    pub session: Option<String>,
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
    /// 帰属規則の名前。`None` は campaign に直接帰属する行（規則を使っていない）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution_rule: Option<String>,
    /// 特定の run・case に割れない共有処理。run 単位の合計に混ぜず別に数える。
    #[serde(default)]
    pub shared_overhead: bool,
}

/// CoS の対話 1 turn の消費行（task の run ではないので proxy 外の呼び出し鍵）。
/// 常に共有オーバーヘッド。数値は全て欠測から始める（観測した欄だけ呼び出し側が埋める）。
pub const ATTRIBUTION_COS_SHARED: &str = "cos_turn_shared_overhead";
/// Jev の分類 1 回の消費行。分類した work unit（無ければ task）の campaign に直接帰属する。
pub const ATTRIBUTION_JEV_SUBJECT: &str = "jev_classification_subject";

impl DecisionLogUsage {
    /// CoS chat run の消費。task の試行に割れないので共有オーバーヘッド。
    pub fn cos_turn(run_id: &str, turn: u32, campaign_id: Option<&str>) -> Self {
        let mut u = Self::new(
            UsageKey::Call {
                decision_id: format!("cos:{run_id}"),
                call_attempt: turn,
            },
            UsageRole::Cos,
        );
        u.campaign_id = campaign_id.map(str::to_string);
        u.attribution_rule = Some(ATTRIBUTION_COS_SHARED.to_string());
        u.shared_overhead = true;
        u
    }

    /// Jev の分類 1 回の呼び出し ID。消費行の鍵（`UsageKey::dedup_key`）と同一で、分類を使う run 側の
    /// `classifier_call_id` と結合できる。`call_no` は同じ subject の呼び出し通し番号（run の `attempt_no` ではない）。
    pub fn jev_call_id(subject: &str, call_no: u32) -> String {
        UsageKey::Call {
            decision_id: format!("jev:{subject}"),
            call_attempt: call_no,
        }
        .dedup_key()
    }

    /// Jev の分類 1 回の消費（`subject` = work unit id、無ければ task id）。費用が返らなければ欠測のまま。
    pub fn jev_call(
        subject: &str,
        call_attempt: u32,
        task_id: &str,
        campaign_id: Option<&str>,
        list_cost_usd: Option<f64>,
    ) -> Self {
        let mut u = Self::new(
            UsageKey::Call {
                decision_id: format!("jev:{subject}"),
                call_attempt,
            },
            UsageRole::Jev,
        );
        u.task_id = Some(task_id.to_string());
        u.campaign_id = campaign_id.map(str::to_string);
        u.list_cost_usd = list_cost_usd;
        u.attribution_rule = Some(ATTRIBUTION_JEV_SUBJECT.to_string());
        u
    }

    /// 空の消費行（全欄が欠測）。
    pub fn new(key: UsageKey, role: UsageRole) -> Self {
        Self {
            key,
            role,
            thread_id: None,
            thread_kind: None,
            project_id: None,
            model: None,
            session: None,
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
            attribution_rule: None,
            shared_overhead: false,
        }
    }
}

/// campaign の計測の別欄まとめ。区分（定価換算・実支出・run 時間・計算資源・壁時計）は混ぜない。
/// `sum` が `None` は 1 件も観測できなかったこと（0 ではない）、`n_missing` は欠測の行数。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MeteredSum {
    pub sum: Option<f64>,
    pub n_observed: u64,
    pub n_missing: u64,
}

impl MeteredSum {
    pub fn add(&mut self, v: Option<f64>) {
        match v {
            Some(x) => {
                self.sum = Some(self.sum.unwrap_or(0.0) + x);
                self.n_observed += 1;
            }
            None => self.n_missing += 1,
        }
    }
}

/// 直接帰属の行と共有オーバーヘッドの行を分けた消費。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MeteredUsage {
    pub n_rows: u64,
    pub input_tokens: MeteredSum,
    pub output_tokens: MeteredSum,
    /// 定価換算（推定）。
    pub list_cost_usd: MeteredSum,
    /// 実支出（取れなければ欠測。定価換算で代用しない）。
    pub cash_cost_usd: MeteredSum,
    /// 計算資源（クラスタ）の node-hours。token・費用とは別。
    pub cluster_node_hours: MeteredSum,
}

impl MeteredUsage {
    fn add(&mut self, u: &DecisionLogUsage) {
        self.n_rows += 1;
        self.input_tokens.add(u.input_tokens.map(|x| x as f64));
        self.output_tokens.add(u.output_tokens.map(|x| x as f64));
        self.list_cost_usd.add(u.list_cost_usd);
        self.cash_cost_usd.add(u.cash_cost_usd);
        self.cluster_node_hours.add(u.cluster_node_hours);
    }
}

/// campaign の計測（`decision_log_campaign_metering`）。elapsed（壁時計）と run 時間の総和は別欄。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CampaignMetering {
    pub campaign_id: String,
    pub direct: MeteredUsage,
    pub shared_overhead: MeteredUsage,
    /// 帰属規則の名前ごとの行数（`direct` に規則なしの行は `none`）。
    pub by_attribution_rule: std::collections::BTreeMap<String, u64>,
    /// campaign 開始（root の識別を付けた時刻）から最後の活動までの壁時計。開始・終了が無ければ `None`。
    pub elapsed_ms: Option<u64>,
    /// run 由来の消費行の `wall_ms` の総和（並列 run は重なるので elapsed 以上になり得る）。
    pub run_wall_sum_ms: MeteredSum,
}

impl CampaignMetering {
    pub fn from_usage(campaign_id: &str, usage: &[DecisionLogUsage]) -> Self {
        let mut m = Self {
            campaign_id: campaign_id.to_string(),
            ..Default::default()
        };
        for u in usage {
            if u.shared_overhead {
                m.shared_overhead.add(u);
            } else {
                m.direct.add(u);
            }
            *m.by_attribution_rule
                .entry(u.attribution_rule.clone().unwrap_or_else(|| "none".into()))
                .or_insert(0) += 1;
            if matches!(u.key, UsageKey::Run { .. }) {
                m.run_wall_sum_ms.add(u.wall_ms.map(|x| x as f64));
            }
        }
        m
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
