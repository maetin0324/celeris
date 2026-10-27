//! 生 trial 記録（JSONL 1 行 = 1 試行）とファイル名。runner が書き、checker/export が読む。
use serde::{Deserialize, Serialize};

use crate::{
    aggregate::Outcome,
    manifest::{ConfigId, Split},
};

pub const TRIAL_SCHEMA: &str = "celeris.campaign-trial/1";
pub const TRIALS_FILE: &str = "trials.jsonl";
pub const SUMMARY_FILE: &str = "summary.json";
pub const META_FILE: &str = "run-meta.json";
pub const CAMPAIGN_FILE: &str = "campaign.json";
pub const INPUT_FILE: &str = "input-manifest.json";
pub const SNAPSHOT_FILE: &str = "snapshot/snapshot.json";
/// 予算停止で実行しなかった試行の reason。分母には残す（outcome = cancelled）。
pub const REASON_BUDGET_STOP: &str = "budget_stop";
/// 1 試行の上限（per_case）を超えたため止めた（実行中の cancel、または完了後の判定）。outcome = cancelled。
pub const REASON_CASE_BUDGET: &str = "case_budget";
/// arm の累積上限（per_arm）が尽きたため止めた／実行しなかった。他 arm は止めない。outcome = cancelled。
pub const REASON_ARM_BUDGET: &str = "arm_budget";
/// executor が `Err` を返したため、その試行だけ error として記録した（run は続行）。使用量は観測できないので課金しない。
pub const REASON_EXECUTOR_ERROR: &str = "executor_error";
/// A（harness）が exit 0 でも試行の workspace が初期 snapshot から変わらなかった（成果物なし）。outcome = failed。
pub const REASON_NO_ARTIFACT: &str = "no_artifact";
/// 試行記録の `error` 欄に残す executor の失敗本文の上限（byte）。
pub const ERROR_MAX_BYTES: usize = 2048;
/// 前提 problem が passed にならなかったため実行しなかった試行の reason。分母には残す（outcome = blocked）。
pub const REASON_BLOCKED: &str = "dependency_blocked";

/// 実際に走った run 1 件の実測（宣言ではなく Celeris の run 記録・harness の起動引数から読んだ値）。
/// 欠測の欄は `None`（宣言名で埋めない）。
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ObservedRun {
    pub run_id: String,
    /// `worker` / `reviewer`（A は常に `worker`）。
    pub role: String,
    pub harness: Option<String>,
    pub source: Option<String>,
    pub model: Option<String>,
    /// run の判断 record にあった実効 routing（mode・出自・下段・根拠表・online 更新）。A・固定の無い run は None。
    pub routing_mode: Option<String>,
    pub routing_source: Option<String>,
    pub lower_policy: Option<String>,
    /// 実効の evidence_mode（none / pinned / latest）。checker が manifest と照合する。
    #[serde(default)]
    pub evidence_mode: Option<String>,
    pub evidence_table_id: Option<String>,
    pub online_update: Option<bool>,
}

/// 試行の workspace の `inputs/` に配った外部入力（case.json の宣言と同じ name・sha256）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeliveredInput {
    pub name: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TrialRecord {
    pub schema: String,
    pub campaign_sha256: String,
    pub input_sha256: String,
    pub split: Split,
    pub config: ConfigId,
    pub problem_id: String,
    pub trial_no: u32,
    /// 独立反復の番号（1 始まり）。trial_no（内部再試行）とは別 ID。
    #[serde(default = "one")]
    pub rep: u32,
    /// problem 内の arm 実行位置（1 始まり）。順序交絡の監査用。
    #[serde(default)]
    pub arm_rank: u32,
    pub outcome: Outcome,
    /// outcome == passed の冗長コピー。改ざん検出に使う。
    pub passed: bool,
    /// manifest の `budget.token_basis` で数えた tokens（予算判定に使う量）。欠測は null（0 にしない）。
    pub tokens: Option<u64>,
    /// tokens の内訳（input / output / cache_read / cache_creation）。欠測は null。
    pub token_usage: Option<crate::manifest::TokenUsage>,
    pub cost_micro_usd: Option<u64>,
    pub wall_secs: u64,
    pub reason: Option<String>,
    /// executor の失敗本文（HTTP status と本文など。秘密値は含まない）。`ERROR_MAX_BYTES` で切る。
    pub error: Option<String>,
    /// 試行の task に固定した実効 routing（mode・lower_policy・evidence_table_id・online_update）。
    /// Celeris 経路（B/C）の試行だけが持つ。A・投入前に拒否された試行は無い。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<serde_json::Value>,
    /// 試行で実際に走った全 run の実測（子・孫・再試行を含む）。executor が観測できた分だけ。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub observed: Vec<ObservedRun>,
    pub harness: String,
    pub harness_version: String,
    pub model: String,
    pub model_version: String,
    pub policy_version: String,
    /// 試行の開始時に観測した実際に動く版（daemon の release・Claude Code の版と sha256・単価表と CoS 方針の版）。
    /// manifest が契約 revision（`contract`）を宣言した試行だけが持つ。上の 4 欄は宣言の写し。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_versions: Option<crate::versions::ObservedVersions>,
    /// out からの相対 path。実行しなかった試行は空。
    pub workspace: String,
    pub kb: String,
    /// 配った外部入力（`workspace/inputs/`）。宣言の無い問題は空。同じ問題なら A/B/C で一致する。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<DeliveredInput>,
    pub workspace_sha256_initial: String,
    pub kb_sha256_initial: String,
    pub workspace_sha256_final: String,
    pub kb_sha256_final: String,
}

fn one() -> u32 {
    1
}

impl TrialRecord {
    /// 予算に課金する tokens。欠測は per_case 上限、executor_error は観測が無いので 0。
    pub fn charged_tokens(&self, b: &crate::manifest::Budget) -> u64 {
        if self.reason.as_deref() == Some(REASON_EXECUTOR_ERROR) {
            0
        } else {
            b.charged_tokens(self.tokens)
        }
    }

    pub fn ran(&self) -> bool {
        match self.reason.as_deref() {
            Some(REASON_BUDGET_STOP | REASON_BLOCKED) => false,
            // arm の残りが無く実行しなかった試行は workspace が空（実行中に尽きた試行は workspace がある）。
            Some(REASON_ARM_BUDGET) => !self.workspace.is_empty(),
            _ => true,
        }
    }
}

/// 失敗本文を `ERROR_MAX_BYTES` 以内に切る（char 境界を守る）。
pub fn clip_error(e: &str) -> String {
    if e.len() <= ERROR_MAX_BYTES {
        return e.to_string();
    }
    let mut i = ERROR_MAX_BYTES;
    while !e.is_char_boundary(i) {
        i -= 1;
    }
    format!("{}…", &e[..i])
}

/// 出力に残す証拠の種別。mock 実行器の数値は実性能として扱えない。
pub fn evidence_kind(simulated: bool) -> &'static str {
    if simulated { "mock" } else { "real" }
}
