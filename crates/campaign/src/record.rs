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
/// 前提 problem が passed にならなかったため実行しなかった試行の reason。分母には残す（outcome = blocked）。
pub const REASON_BLOCKED: &str = "dependency_blocked";

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
    /// 欠測は null（0 にしない）。
    pub tokens: Option<u64>,
    pub cost_micro_usd: Option<u64>,
    pub wall_secs: u64,
    pub reason: Option<String>,
    pub harness: String,
    pub harness_version: String,
    pub model: String,
    pub model_version: String,
    pub policy_version: String,
    /// out からの相対 path。実行しなかった試行は空。
    pub workspace: String,
    pub kb: String,
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
