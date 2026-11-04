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
    }
}
