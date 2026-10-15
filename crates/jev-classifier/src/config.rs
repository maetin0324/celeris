use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::JevError;

pub const DEFAULT_ENDPOINT: &str = "https://openrouter.ai/api/alpha/decisions";
pub const DEFAULT_MODEL_ID: &str = "typesafe/jev-1.14";

/// 版つき設定（ADR D2）。model ID は pin し、`~` で始まる最新追従 alias は拒否する。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct JevConfig {
    pub endpoint: String,
    pub model_id: String,
    pub schema_version: String,
    pub prompt_version: String,
    pub timeout_ms: u64,
    pub max_retries: u32,
    pub backoff_base_ms: u64,
    pub backoff_max_ms: u64,
    pub cache_max_entries: usize,
    pub breaker_failure_threshold: u32,
    pub breaker_cooldown_ms: u64,
    /// 鍵を保管する secret の id（ADR-0030）。値は持たない。
    pub secret_id: String,
}

impl Default for JevConfig {
    fn default() -> Self {
        Self {
            endpoint: DEFAULT_ENDPOINT.to_string(),
            model_id: DEFAULT_MODEL_ID.to_string(),
            schema_version: "jev-questions/1".to_string(),
            prompt_version: "jev-state/1".to_string(),
            timeout_ms: 10_000,
            max_retries: 2,
            backoff_base_ms: 200,
            backoff_max_ms: 5_000,
            cache_max_entries: 256,
            breaker_failure_threshold: 3,
            breaker_cooldown_ms: 60_000,
            secret_id: "jev_api_key".to_string(),
        }
    }
}

impl JevConfig {
    pub fn validate(&self) -> Result<(), JevError> {
        let bad = |m: &str| Err(JevError::Config(m.to_string()));
        if !(self.endpoint.starts_with("http://") || self.endpoint.starts_with("https://")) {
            return bad("endpoint must be http(s) url");
        }
        if self.model_id.trim().is_empty() {
            return bad("model_id is empty");
        }
        if self.model_id.starts_with('~') {
            return bad("model_id must be pinned; '~' latest alias is not allowed");
        }
        if self.schema_version.is_empty() || self.prompt_version.is_empty() {
            return bad("schema_version and prompt_version must be set");
        }
        if self.timeout_ms == 0 {
            return bad("timeout_ms must be > 0");
        }
        if self.breaker_failure_threshold == 0 {
            return bad("breaker_failure_threshold must be > 0");
        }
        if self.secret_id.trim().is_empty() {
            return bad("secret_id is empty");
        }
        Ok(())
    }

    pub fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_ms)
    }

    pub fn breaker_cooldown(&self) -> Duration {
        Duration::from_millis(self.breaker_cooldown_ms)
    }

    /// attempt（0 起点の失敗回数）に対する指数 backoff。上限で飽和する。
    pub fn backoff(&self, attempt: u32) -> Duration {
        let factor = 1u64.checked_shl(attempt.min(32)).unwrap_or(u64::MAX);
        let ms = self.backoff_base_ms.saturating_mul(factor).min(self.backoff_max_ms);
        Duration::from_millis(ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_valid_and_pinned() {
        let c = JevConfig::default();
        assert!(c.validate().is_ok());
        assert_eq!(c.model_id, "typesafe/jev-1.14");
    }

    #[test]
    fn latest_alias_is_rejected() {
        let c = JevConfig { model_id: "~typesafe/jev-latest".into(), ..Default::default() };
        assert!(matches!(c.validate(), Err(JevError::Config(_))));
    }

    #[test]
    fn unknown_field_is_rejected() {
        let r: Result<JevConfig, _> = serde_json::from_str(r#"{"bogus":1}"#);
        assert!(r.is_err());
    }

    #[test]
    fn backoff_is_exponential_and_capped() {
        let c = JevConfig { backoff_base_ms: 100, backoff_max_ms: 500, ..Default::default() };
        assert_eq!(c.backoff(0), Duration::from_millis(100));
        assert_eq!(c.backoff(1), Duration::from_millis(200));
        assert_eq!(c.backoff(2), Duration::from_millis(400));
        assert_eq!(c.backoff(3), Duration::from_millis(500));
        assert_eq!(c.backoff(100), Duration::from_millis(500));
    }
}
