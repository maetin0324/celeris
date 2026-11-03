//! `[script]`（境界 ADR 2026-11-02 (b)(d)、付記 S5-1）: TS 制御層の bundle の置き場所と上限。
//!
//! 節が無ければ TS 層を起こさない（thread も isolate も作らない）。値の形の誤りは設定エラーにする。
//! どの bundle を load するかは config ではなく DB（`script_domain_modes` が指す generation）が決める。
//! 消した欄 `active_bundle_sha256` は未知の欄として読み込みエラーになる（付記 S5-1）。
//! bundle の検証は登録と起動時の復元で `daemon::script_layer` が行い、失敗しても daemon は止めない（fail-closed）。

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::ConfigError;

/// ```toml
/// [script]
/// state_dir = "/local/celeris/state"
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptConfig {
    /// `<state_dir>/script-bundles/<bundle_sha256>/` を読む。相対パスは設定ファイル基準。
    pub state_dir: PathBuf,
    /// 1 回の入力の上限（byte）。
    #[serde(default = "default_input_limit_bytes")]
    pub input_limit_bytes: usize,
    /// V8 heap の上限（byte）。
    #[serde(default = "default_heap_limit_bytes")]
    pub heap_limit_bytes: usize,
}

fn default_input_limit_bytes() -> usize {
    celeris_script::DEFAULT_INPUT_LIMIT_BYTES
}

fn default_heap_limit_bytes() -> usize {
    celeris_script::DEFAULT_HEAP_LIMIT_BYTES
}

impl ScriptConfig {
    pub(super) fn resolve_paths(&mut self, base: &Path) {
        self.state_dir = task_core::expand_home(&self.state_dir, task_core::home_dir().as_deref());
        if self.state_dir.is_relative() {
            self.state_dir = base.join(&self.state_dir);
        }
    }

    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        if self.input_limit_bytes == 0 {
            return Err(ConfigError::Invalid(
                "[script] input_limit_bytes must be greater than 0".into(),
            ));
        }
        if self.heap_limit_bytes == 0 {
            return Err(ConfigError::Invalid(
                "[script] heap_limit_bytes must be greater than 0".into(),
            ));
        }
        Ok(())
    }

    /// 検証済みの値から actor の上限を作る。
    pub fn limits(&self) -> celeris_script::RuntimeLimits {
        celeris_script::RuntimeLimits {
            input_limit_bytes: self.input_limit_bytes,
            heap_limit_bytes: self.heap_limit_bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::config::Config;

    const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    /// 付記 S5-1: 消した欄は読み込みエラー（どの bundle を load するかは DB が決める）。
    #[test]
    fn config_active_bundle_sha256_is_rejected() {
        let err = load(&format!(
            "[script]\nstate_dir = \"state\"\nactive_bundle_sha256 = \"{SHA}\"\n"
        ))
        .expect_err("removed field")
        .to_string();
        assert!(err.contains("active_bundle_sha256"), "{err}");
    }

    fn load(extra: &str) -> Result<Config, crate::config::ConfigError> {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, format!("db = \"t.sqlite3\"\n{extra}")).expect("write config");
        Config::load(&path)
    }

    #[test]
    fn script_runtime_config_is_absent_by_default() {
        assert_eq!(load("").expect("config").script, None);
    }

    #[test]
    fn script_runtime_config_reads_values_and_defaults() {
        let cfg = load("[script]\nstate_dir = \"state\"\n").expect("config");
        let script = cfg.script.expect("script");
        assert!(script.state_dir.is_absolute(), "{:?}", script.state_dir);
        assert!(script.state_dir.ends_with("state"));
        assert_eq!(script.limits(), celeris_script::RuntimeLimits::default());
        let cfg = load(
            "[script]
state_dir = \"/s\"
input_limit_bytes = 7
heap_limit_bytes = 9
",
        )
        .expect("config");
        let limits = cfg.script.expect("script").limits();
        assert_eq!(limits.input_limit_bytes, 7);
        assert_eq!(limits.heap_limit_bytes, 9);
    }

    #[test]
    fn script_runtime_config_rejects_invalid_values() {
        for (extra, needle) in [
            ("input_limit_bytes = 0\n", "input_limit_bytes"),
            ("heap_limit_bytes = 0\n", "heap_limit_bytes"),
            ("mode = \"ts\"\n", "mode"),
        ] {
            let err = load(&format!("[script]\nstate_dir = \"/s\"\n{extra}"))
                .expect_err(extra)
                .to_string();
            assert!(err.contains(needle), "{err}");
        }
    }
}
