//! `[delivery]`（ADR-0137 D1c・D1d）: 配送の main 追従と定型衝突の自動解消。

use serde::Deserialize;
use task_dispatch::auto_resolve::generated::DEFAULT_GLOBS;

use super::ConfigError;

/// `[delivery]`。今は `[delivery.auto_resolve]` だけを持つ。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryConfig {
    #[serde(default)]
    pub auto_resolve: AutoResolveConfig,
}

/// `[delivery.auto_resolve]`（ADR-0137 D1d）: `merge_base` 系の配送失敗で局所修復を作る前に、scratch の
/// worktree で既定ブランチを取り込み `task_dispatch::auto_resolve::resolve` を試す。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutoResolveConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// 1 つの配送で自動解消を試す回数の上限（既定 3）。達したら従来の経路（局所修復・人）へ回す。
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    #[serde(default)]
    pub generated: GeneratedConfig,
}

/// `[delivery.auto_resolve.generated]`（ADR-0137 D1c）: 生成物の衝突の再生成規則。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedConfig {
    /// 生成物として扱う path。分類は resolver（`classify::kind`）と同じ範囲でなければならないので、
    /// 今は `task_dispatch::auto_resolve::generated::DEFAULT_GLOBS` と同じ集合だけを受け付ける。
    #[serde(default = "default_generated_globs")]
    pub globs: Vec<String>,
    /// 再生成コマンドの argv（shell を介さない）。空なら生成物の衝突は人に回す。
    /// 既定は KB schema-regeneration の `UPDATE_SCHEMA=1` の 3 crate。
    #[serde(default = "default_generated_cmd")]
    pub cmd: Vec<String>,
}

impl Default for AutoResolveConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            max_attempts: default_max_attempts(),
            generated: GeneratedConfig::default(),
        }
    }
}

impl Default for GeneratedConfig {
    fn default() -> Self {
        Self {
            globs: default_generated_globs(),
            cmd: default_generated_cmd(),
        }
    }
}

fn default_enabled() -> bool {
    true
}

fn default_max_attempts() -> u32 {
    3
}

fn default_generated_globs() -> Vec<String> {
    DEFAULT_GLOBS.iter().map(|g| g.to_string()).collect()
}

fn default_generated_cmd() -> Vec<String> {
    [
        "env",
        "UPDATE_SCHEMA=1",
        "cargo",
        "test",
        "-p",
        "task-core",
        "-p",
        "task-worker",
        "-p",
        "task-api",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

impl GeneratedConfig {
    /// resolver に渡す argv。空なら `None`（生成物の衝突は人に回る）。
    pub fn command(&self) -> Option<Vec<String>> {
        (!self.cmd.is_empty()).then(|| self.cmd.clone())
    }
}

impl DeliveryConfig {
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        let auto = &self.auto_resolve;
        if auto.enabled && auto.max_attempts == 0 {
            return Err(ConfigError::Invalid(
                "delivery.auto_resolve.max_attempts must be >= 1 when enabled".into(),
            ));
        }
        let mut globs = auto.generated.globs.clone();
        globs.sort();
        let mut defaults = default_generated_globs();
        defaults.sort();
        if globs != defaults {
            return Err(ConfigError::Invalid(format!(
                "delivery.auto_resolve.generated.globs must be {defaults:?} (the resolver's classification)"
            )));
        }
        Ok(())
    }
}
