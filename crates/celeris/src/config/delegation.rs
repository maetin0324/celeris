use serde::Deserialize;
use task_core::DelegationLimits;

use super::Config;

/// `[delegation]`（ADR-0016 D2 / M6）: 実行中の委譲の上限。既定は `task_core::DelegationLimits::default()` と同じ。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DelegationConfig {
    /// 1 run あたりに受け付ける提案の件数（複数の `delegate` メッセージをまたいで数える）。
    #[serde(default = "default_max_delegate_per_run")]
    pub max_delegate_per_run: usize,
    /// 木の深さ（根 = 1）。
    #[serde(default = "default_max_tree_depth")]
    pub max_tree_depth: u32,
    /// 木全体のワーカー run 数。
    #[serde(default = "default_max_tree_runs")]
    pub max_tree_runs: u32,
    /// ADR-0021 D4: 委譲した子が `failed` になったときの親の扱い。
    /// `"retry_then_ask"`（既定。やり直し → 駄目なら人に質問して `blocked`）か `"ignore"`（子の失敗を見ない）。
    #[serde(default = "default_on_child_failure")]
    pub on_child_failure: String,
}

impl Default for DelegationConfig {
    fn default() -> Self {
        Self {
            max_delegate_per_run: default_max_delegate_per_run(),
            max_tree_depth: default_max_tree_depth(),
            max_tree_runs: default_max_tree_runs(),
            on_child_failure: default_on_child_failure(),
        }
    }
}

fn default_on_child_failure() -> String {
    "retry_then_ask".to_string()
}

fn default_max_delegate_per_run() -> usize {
    task_core::DelegationLimits::default().max_delegate_per_run
}
fn default_max_tree_depth() -> u32 {
    task_core::DelegationLimits::default().max_tree_depth
}
fn default_max_tree_runs() -> u32 {
    task_core::DelegationLimits::default().max_tree_runs
}

impl Config {
    /// ADR-0016 D2: `[delegation]` を task-core の型に写す。
    pub fn delegation_limits(&self) -> DelegationLimits {
        DelegationLimits {
            max_delegate_per_run: self.delegation.max_delegate_per_run,
            max_tree_depth: self.delegation.max_tree_depth,
            max_tree_runs: self.delegation.max_tree_runs,
            on_child_failure: match self.delegation.on_child_failure.as_str() {
                "ignore" => task_core::OnChildFailure::Ignore,
                _ => task_core::OnChildFailure::RetryThenAsk,
            },
        }
    }
}
