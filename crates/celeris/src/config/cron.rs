//! ADR-0131 D6 / 付記 D10: `[[cron.seed]]`（定期実行 job の**種**）。
//!
//! org.toml と同じく「DB が正、設定は種」: 起動時に `cron_jobs` が空のときだけ一度投入する
//! （`daemon::bootstrap::seed_cron_if_empty`）。以後の編集は API / celerisctl で行い、設定は再読込しない。
//! ここでは形と、DB に依らない検証（名前・cron 式・タイムゾーン・`mode`・harness の存在）だけを行う。
//! 雛形が task に組み立てられるか（acceptance・project 等）は投入時に既存の cron job 作成と同じ
//! `task_ops::cron_jobs::validate_job` / `create_job` を通す。

use serde::Deserialize;
use task_core::{CronCatchUp, CronOverlap, CronSchedule, CronTaskTemplate, CronTz};

use super::ConfigError;

/// 雛形の `extra["mode"]` の値（ADR-0131 付記 D10 3）。省略時は `dry_run` とみなす。
pub const CRON_TEMPLATE_MODES: &[&str] = &["dry_run", "apply"];

/// `[cron]`。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CronConfig {
    /// `[[cron.seed]]`。空の `cron_jobs` に一度だけ入れる job の並び（書いた順に投入する）。
    #[serde(default)]
    pub seed: Vec<CronSeedConfig>,
}

/// `[[cron.seed]]` の 1 件。欄は `POST /cron-jobs` の本文と同じ（`enabled` の既定だけ違う）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CronSeedConfig {
    pub name: String,
    /// 省略時は `false`（種は一時停止の状態で入り、有効にするのは人の `resume`）。
    #[serde(default)]
    pub enabled: bool,
    /// 5 欄の cron 式（`@daily` 等の別名も可）。
    pub schedule: String,
    /// IANA タイムゾーン名（例 `Asia/Tokyo`）。
    pub timezone: String,
    #[serde(default)]
    pub overlap: CronOverlap,
    #[serde(default)]
    pub catch_up: CronCatchUp,
    /// `[cron.seed.template]`。`mode` など雛形の型に無い欄は `extra` に入る。
    pub template: CronTaskTemplate,
}

impl CronConfig {
    /// DB に依らない検証。`known_harnesses` が空（`[[harnesses]]` を使わない最小構成）なら harness の
    /// 存在は見ない。
    pub fn validate(&self, known_harnesses: &[String]) -> Result<(), ConfigError> {
        let mut names: Vec<&str> = Vec::new();
        for seed in &self.seed {
            let invalid =
                |msg: String| ConfigError::Invalid(format!("[[cron.seed]] {:?}: {msg}", seed.name));
            if seed.name.trim().is_empty() {
                return Err(ConfigError::Invalid(
                    "[[cron.seed]]: name must not be blank".into(),
                ));
            }
            if names.contains(&seed.name.as_str()) {
                return Err(invalid("duplicate name".into()));
            }
            names.push(&seed.name);
            seed.schedule
                .parse::<CronSchedule>()
                .map_err(|e| invalid(e.to_string()))?;
            CronTz::iana(&seed.timezone).map_err(|e| invalid(e.to_string()))?;
            if let Some(mode) = seed.template.extra.get("mode") {
                let ok = mode
                    .as_str()
                    .is_some_and(|m| CRON_TEMPLATE_MODES.contains(&m));
                if !ok {
                    return Err(invalid(format!(
                        "template.mode must be one of {CRON_TEMPLATE_MODES:?}, got {mode}"
                    )));
                }
            }
            if let Some(harness) = &seed.template.harness
                && !known_harnesses.is_empty()
                && !known_harnesses.iter().any(|h| h == harness)
            {
                return Err(invalid(format!(
                    "template.harness {harness:?} is not defined in [[harnesses]]"
                )));
            }
        }
        Ok(())
    }
}

impl CronSeedConfig {
    /// 既存の cron job 作成の入力に写す。
    pub fn to_new_job(&self) -> task_ops::cron_jobs::NewCronJob {
        task_ops::cron_jobs::NewCronJob {
            name: self.name.clone(),
            schedule: self.schedule.clone(),
            timezone: self.timezone.clone(),
            overlap: self.overlap,
            catch_up: self.catch_up,
            enabled: self.enabled,
            template: self.template.clone(),
        }
    }
}

#[cfg(test)]
#[path = "cron_tests.rs"]
mod tests;
