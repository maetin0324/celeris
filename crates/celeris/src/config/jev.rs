//! `[jev]`（ADR 2026-10-15-jev-wire D1）: Jev 分類段の設定。既定は無効（欄が無ければ従来と同じ挙動）。
//!
//! 未知の欄・不正な値（http(s) でない endpoint、`~` の latest alias、0 の timeout など）は読み込みエラー。
//! 鍵の値は持たない（`secret_id` は secrets dir の名前）。値は `jev_classifier::JevConfig` へ写して使う。

use jev_classifier::JevConfig;
use serde::Deserialize;

use super::ConfigError;

/// ```toml
/// [jev]
/// enabled = true   # 既定 false
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct JevSectionConfig {
    pub enabled: bool,
    pub endpoint: String,
    pub model_id: String,
    pub timeout_ms: u64,
    pub max_retries: u32,
    pub backoff_base_ms: u64,
    pub backoff_max_ms: u64,
    pub cache_max_entries: usize,
    pub breaker_failure_threshold: u32,
    pub breaker_cooldown_ms: u64,
    /// 鍵を保管する secret の id（ADR-0030）。値ではない。
    pub secret_id: String,
}

impl Default for JevSectionConfig {
    fn default() -> Self {
        let d = JevConfig::default();
        Self {
            enabled: false,
            endpoint: d.endpoint,
            model_id: d.model_id,
            timeout_ms: d.timeout_ms,
            max_retries: d.max_retries,
            backoff_base_ms: d.backoff_base_ms,
            backoff_max_ms: d.backoff_max_ms,
            cache_max_entries: d.cache_max_entries,
            breaker_failure_threshold: d.breaker_failure_threshold,
            breaker_cooldown_ms: d.breaker_cooldown_ms,
            secret_id: d.secret_id,
        }
    }
}

impl JevSectionConfig {
    /// schema / prompt の版は classifier crate の既定（コードに固定）を使う。
    pub fn to_jev_config(&self) -> JevConfig {
        JevConfig {
            endpoint: self.endpoint.clone(),
            model_id: self.model_id.clone(),
            timeout_ms: self.timeout_ms,
            max_retries: self.max_retries,
            backoff_base_ms: self.backoff_base_ms,
            backoff_max_ms: self.backoff_max_ms,
            cache_max_entries: self.cache_max_entries,
            breaker_failure_threshold: self.breaker_failure_threshold,
            breaker_cooldown_ms: self.breaker_cooldown_ms,
            secret_id: self.secret_id.clone(),
            ..JevConfig::default()
        }
    }

    /// 無効のときも欄の誤りは弾く（有効化した瞬間に起動できない設定を残さない）。
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        self.to_jev_config()
            .validate()
            .map_err(|e| ConfigError::Invalid(format!("[jev] {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn load(extra: &str) -> Result<Config, ConfigError> {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, format!("db = \"t.sqlite3\"\n{extra}")).expect("write config");
        Config::load(&path)
    }

    #[test]
    fn disabled_by_default_with_or_without_the_table() {
        let cfg = load("").expect("config");
        assert!(!cfg.jev.enabled);
        assert_eq!(cfg.jev, JevSectionConfig::default());
        assert_eq!(load("\n[jev]\n").expect("config").jev, cfg.jev);
        assert_eq!(cfg.jev.to_jev_config(), JevConfig::default());
    }

    #[test]
    fn every_field_round_trips() {
        let cfg = load(
            r#"
[jev]
enabled = true
endpoint = "http://127.0.0.1:9/decisions"
model_id = "typesafe/jev-1.14"
timeout_ms = 1500
max_retries = 4
backoff_base_ms = 10
backoff_max_ms = 90
cache_max_entries = 8
breaker_failure_threshold = 5
breaker_cooldown_ms = 777
secret_id = "other_key"
"#,
        )
        .expect("config");
        let j = &cfg.jev;
        assert!(j.enabled);
        assert_eq!(j.endpoint, "http://127.0.0.1:9/decisions");
        assert_eq!(j.timeout_ms, 1500);
        assert_eq!(j.max_retries, 4);
        assert_eq!(j.backoff_base_ms, 10);
        assert_eq!(j.backoff_max_ms, 90);
        assert_eq!(j.cache_max_entries, 8);
        assert_eq!(j.breaker_failure_threshold, 5);
        assert_eq!(j.breaker_cooldown_ms, 777);
        assert_eq!(j.secret_id, "other_key");
        let jc = j.to_jev_config();
        assert_eq!(jc.timeout_ms, 1500);
        assert_eq!(jc.secret_id, "other_key");
    }

    #[test]
    fn unknown_field_is_a_load_error() {
        assert!(load("\n[jev]\napi_key = \"x\"\n").is_err());
        assert!(load("\n[jev]\nenabled = true\nmodel = \"x\"\n").is_err());
    }

    #[test]
    fn invalid_values_are_load_errors_even_when_disabled() {
        for bad in [
            "endpoint = \"ftp://x\"",
            "model_id = \"~typesafe/jev-latest\"",
            "model_id = \"\"",
            "timeout_ms = 0",
            "breaker_failure_threshold = 0",
            "secret_id = \"\"",
            "enabled = \"yes\"",
            "timeout_ms = -1",
        ] {
            assert!(load(&format!("\n[jev]\n{bad}\n")).is_err(), "{bad}");
        }
    }
}
