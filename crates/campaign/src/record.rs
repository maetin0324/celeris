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

impl TrialRecord {
    pub fn ran(&self) -> bool {
        !matches!(
            self.reason.as_deref(),
            Some(REASON_BUDGET_STOP | REASON_BLOCKED)
        )
    }
}
