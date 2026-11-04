//! `[project_controller]`（ADR 2026-11-04-project-controller D11・A1）: 案件の durable wake と controller。
//!
//! 節が無い・`enabled = false`（既定）なら tick の段は何もしない。`allowed_project_ids` に無い案件は
//! `shadow` 以上へ上げられない（API が 403 `project_controller_not_allowed`）。値の誤りは設定エラー。

use serde::Deserialize;

use super::ConfigError;

/// ```toml
/// [project_controller]
/// enabled = true
/// allowed_project_ids = ["01…"]
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectControllerConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub allowed_project_ids: Vec<String>,
    #[serde(default = "d_coalesce")]
    pub coalesce_secs: u64,
    #[serde(default = "d_lease")]
    pub lease_secs: u64,
    #[serde(default = "d_max_eval")]
    pub max_evaluations_per_tick: usize,
    #[serde(default = "d_retry_base")]
    pub retry_base_secs: u64,
    #[serde(default = "d_retry_max")]
    pub retry_max_secs: u64,
    #[serde(default = "d_max_retries")]
    pub max_retries: u32,
    #[serde(default = "d_max_inputs")]
    pub max_cos_inputs_open_per_project: u32,
    /// A6: `[project_controller.budget]`。
    #[serde(default)]
    pub budget: ProjectBudgetConfig,
}

/// ```toml
/// [project_controller.budget]
/// max_cos_inputs_per_day = 10
/// max_roots_per_day = 5
/// max_no_progress_decisions = 5
/// max_same_failure_repeats = 3
/// max_invalid_decisions = 3
/// max_tokens_per_day = 20000000
/// ttl_secs = 259200
/// ```
/// 0 は設定の誤り（上限なしにはしない。I12）。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectBudgetConfig {
    #[serde(default = "b_cos")]
    pub max_cos_inputs_per_day: u32,
    #[serde(default = "b_roots")]
    pub max_roots_per_day: u32,
    #[serde(default = "b_no_progress")]
    pub max_no_progress_decisions: u32,
    #[serde(default = "b_same")]
    pub max_same_failure_repeats: u32,
    #[serde(default = "b_invalid")]
    pub max_invalid_decisions: u32,
    #[serde(default = "b_tokens")]
    pub max_tokens_per_day: u64,
    #[serde(default = "b_ttl")]
    pub ttl_secs: u64,
}

fn b() -> task_ops::project_budget::BudgetLimits {
    task_ops::project_budget::BudgetLimits::default()
}
fn b_cos() -> u32 {
    b().max_cos_inputs_per_day
}
fn b_roots() -> u32 {
    b().max_roots_per_day
}
fn b_no_progress() -> u32 {
    b().max_no_progress_decisions
}
fn b_same() -> u32 {
    b().max_same_failure_repeats
}
fn b_invalid() -> u32 {
    b().max_invalid_decisions
}
fn b_tokens() -> u64 {
    b().max_tokens_per_day
}
fn b_ttl() -> u64 {
    b().ttl_secs
}

impl Default for ProjectBudgetConfig {
    fn default() -> Self {
        let l = b();
        Self {
            max_cos_inputs_per_day: l.max_cos_inputs_per_day,
            max_roots_per_day: l.max_roots_per_day,
            max_no_progress_decisions: l.max_no_progress_decisions,
            max_same_failure_repeats: l.max_same_failure_repeats,
            max_invalid_decisions: l.max_invalid_decisions,
            max_tokens_per_day: l.max_tokens_per_day,
            ttl_secs: l.ttl_secs,
        }
    }
}

impl ProjectBudgetConfig {
    pub fn limits(&self) -> task_ops::project_budget::BudgetLimits {
        task_ops::project_budget::BudgetLimits {
            max_cos_inputs_per_day: self.max_cos_inputs_per_day,
            max_roots_per_day: self.max_roots_per_day,
            max_no_progress_decisions: self.max_no_progress_decisions,
            max_same_failure_repeats: self.max_same_failure_repeats,
            max_invalid_decisions: self.max_invalid_decisions,
            max_tokens_per_day: self.max_tokens_per_day,
            ttl_secs: self.ttl_secs,
        }
    }

    /// TS の判定に渡す上限（snapshot の今日の残り・進捗なしの上限）。
    pub fn decider_limits(&self) -> task_dispatch::project_decider_ts::DeciderLimits {
        let snapshot = task_core::project_proposal::SnapshotLimits {
            max_no_progress_decisions: self.max_no_progress_decisions,
            ..Default::default()
        };
        task_dispatch::project_decider_ts::DeciderLimits {
            snapshot,
            max_cos_inputs_per_day: self.max_cos_inputs_per_day,
            max_roots_per_day: self.max_roots_per_day,
        }
    }
}

fn defaults() -> task_dispatch::project_controller::ProjectControllerSettings {
    task_dispatch::project_controller::ProjectControllerSettings::default()
}
fn d_coalesce() -> u64 {
    defaults().coalesce_secs
}
fn d_lease() -> u64 {
    defaults().lease_secs
}
fn d_max_eval() -> usize {
    defaults().max_evaluations_per_tick
}
fn d_retry_base() -> u64 {
    defaults().retry_base_secs
}
fn d_retry_max() -> u64 {
    defaults().retry_max_secs
}
fn d_max_retries() -> u32 {
    defaults().max_retries
}
fn d_max_inputs() -> u32 {
    defaults().max_cos_inputs_open_per_project
}

impl ProjectControllerConfig {
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        let positive = [
            ("lease_secs", self.lease_secs),
            (
                "max_evaluations_per_tick",
                self.max_evaluations_per_tick as u64,
            ),
            ("retry_base_secs", self.retry_base_secs),
            (
                "max_cos_inputs_open_per_project",
                u64::from(self.max_cos_inputs_open_per_project),
            ),
        ];
        let b = &self.budget;
        let budget = [
            (
                "max_cos_inputs_per_day",
                u64::from(b.max_cos_inputs_per_day),
            ),
            ("max_roots_per_day", u64::from(b.max_roots_per_day)),
            (
                "max_no_progress_decisions",
                u64::from(b.max_no_progress_decisions),
            ),
            (
                "max_same_failure_repeats",
                u64::from(b.max_same_failure_repeats),
            ),
            ("max_invalid_decisions", u64::from(b.max_invalid_decisions)),
            ("max_tokens_per_day", b.max_tokens_per_day),
            ("ttl_secs", b.ttl_secs),
        ];
        if let Some((name, _)) = budget.iter().find(|(_, v)| *v == 0) {
            return Err(ConfigError::Invalid(format!(
                "[project_controller.budget] {name} must be greater than 0"
            )));
        }
        if let Some((name, _)) = positive.iter().find(|(_, v)| *v == 0) {
            return Err(ConfigError::Invalid(format!(
                "[project_controller] {name} must be greater than 0"
            )));
        }
        if self.retry_max_secs < self.retry_base_secs {
            return Err(ConfigError::Invalid(
                "[project_controller] retry_max_secs must be >= retry_base_secs".into(),
            ));
        }
        if self.allowed_project_ids.iter().any(|p| p.trim().is_empty()) {
            return Err(ConfigError::Invalid(
                "[project_controller] allowed_project_ids must not contain blank ids".into(),
            ));
        }
        Ok(())
    }

    /// dispatcher に渡す実効値。
    pub fn settings(&self) -> task_dispatch::project_controller::ProjectControllerSettings {
        task_dispatch::project_controller::ProjectControllerSettings {
            enabled: self.enabled,
            coalesce_secs: self.coalesce_secs,
            lease_secs: self.lease_secs,
            max_evaluations_per_tick: self.max_evaluations_per_tick,
            retry_base_secs: self.retry_base_secs,
            retry_max_secs: self.retry_max_secs,
            max_retries: self.max_retries,
            max_cos_inputs_open_per_project: self.max_cos_inputs_open_per_project,
            budget: self.budget.limits(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_controller_config_defaults_and_unknown_field() {
        let c: ProjectControllerConfig = toml::from_str("").unwrap();
        assert!(!c.enabled);
        assert_eq!(c.settings(), defaults());
        assert!(toml::from_str::<ProjectControllerConfig>("goal = 1").is_err());
        let bad: ProjectControllerConfig = toml::from_str("lease_secs = 0").unwrap();
        assert!(bad.validate().is_err());
        let b: ProjectControllerConfig =
            toml::from_str("[budget]\nmax_roots_per_day = 1\nttl_secs = 60").unwrap();
        assert!(b.validate().is_ok());
        assert_eq!(b.settings().budget.max_roots_per_day, 1);
        assert_eq!(b.settings().budget.ttl_secs, 60);
        let zero: ProjectControllerConfig = toml::from_str("[budget]\nttl_secs = 0").unwrap();
        assert!(zero.validate().is_err());
        assert!(toml::from_str::<ProjectControllerConfig>("[budget]\nunknown = 1").is_err());
    }
}
