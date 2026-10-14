//! 評価 run の試行（ADR 2026-10-11-llm-source-model-routing-evaluation 付記 2026-10-12 eval-dispatch）。
//!
//! 1 試行 = harness 固定・source×model 固定・新 session・新 worktree の評価 run 1 本。
//! `check_cmd` は採点専用で、worker に渡す本文（`objective`）には含めない。
use serde::{Deserialize, Serialize};

/// 試行の状態。`queued → running → scoring → passed|failed`、起動・採点の異常は `error`、
/// `queued`/`running` からは `cancelled`。
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum EvalTrialStatus {
    Queued,
    Running,
    Scoring,
    Passed,
    Failed,
    Error,
    Cancelled,
}

impl EvalTrialStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Scoring => "scoring",
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "queued" => Self::Queued,
            "running" => Self::Running,
            "scoring" => Self::Scoring,
            "passed" => Self::Passed,
            "failed" => Self::Failed,
            "error" => Self::Error,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }

    /// これ以上動かない状態。
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Passed | Self::Failed | Self::Error | Self::Cancelled
        )
    }
}

/// 投入の入力（`POST /api/v1/routing-eval/trials` の 1 試行）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvalTrialSpec {
    pub manifest_sha: String,
    pub problem_id: String,
    pub trial_no: u32,
    pub source_id: String,
    pub model_id: String,
    pub harness_id: String,
    pub repo: String,
    pub base_commit: String,
    pub objective: String,
    pub check_cmd: String,
    pub check_timeout_secs: u32,
}

/// `eval_trials` の 1 行。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvalTrial {
    pub trial_id: String,
    pub batch_id: String,
    pub manifest_sha: String,
    pub problem_id: String,
    pub trial_no: u32,
    pub source_id: String,
    pub model_id: String,
    pub harness_id: String,
    pub harness_version: Option<String>,
    pub repo: String,
    pub base_commit: String,
    pub objective: String,
    pub check_cmd: String,
    pub check_timeout_secs: u32,
    pub status: EvalTrialStatus,
    pub run_id: Option<String>,
    pub decision_id: Option<String>,
    pub worktree_path: Option<String>,
    pub check_exit: Option<i32>,
    pub timed_out: bool,
    pub error: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    /// 試行を claim（`running` に）した daemon instance（`daemon_instances.instance_id`）。終端・`queued`
    /// に戻しても消さない。migration 0078 より前に起動した行は `None`（ADR 2026-10-14-eval-verify-isolation D3.1）。
    pub instance_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_round_trips_and_terminal_set() {
        for s in [
            EvalTrialStatus::Queued,
            EvalTrialStatus::Running,
            EvalTrialStatus::Scoring,
            EvalTrialStatus::Passed,
            EvalTrialStatus::Failed,
            EvalTrialStatus::Error,
            EvalTrialStatus::Cancelled,
        ] {
            assert_eq!(EvalTrialStatus::parse(s.as_str()), Some(s));
            assert_eq!(
                serde_json::to_value(s).expect("serialize"),
                serde_json::Value::String(s.as_str().into())
            );
        }
        assert_eq!(EvalTrialStatus::parse("bogus"), None);
        assert!(!EvalTrialStatus::Running.is_terminal());
        assert!(EvalTrialStatus::Cancelled.is_terminal());
    }
}
