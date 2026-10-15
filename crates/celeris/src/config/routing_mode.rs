//! `[routing]`（ADR 2026-10-15-mlsys-min-routing-policy D2）: 固定方策か decision-assisted かの切替。
//!
//! 欄は任意で既定は `fixed`（従来と同じ選択結果）。未知の値・未知の欄は読み込みエラー。
//! 既存の実測根拠表ルータ（`route_selection` など）とは別の欄で、この欄はそれらに依存も変更もしない。

use serde::Deserialize;
use task_core::route_select::RoutingMode;

/// ```toml
/// [routing]
/// mode = "decision_assisted"   # 既定 "fixed"
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingConfig {
    #[serde(default)]
    pub mode: RoutingMode,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn load(extra: &str) -> Result<Config, crate::config::ConfigError> {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, format!("db = \"t.sqlite3\"\n{extra}")).expect("write config");
        Config::load(&path)
    }

    #[test]
    fn mode_defaults_to_fixed_without_the_table_or_the_field() {
        assert_eq!(load("").expect("config").routing.mode, RoutingMode::Fixed);
        assert_eq!(
            load("\n[routing]\n").expect("config").routing,
            RoutingConfig::default()
        );
    }

    #[test]
    fn both_modes_parse() {
        for (text, mode) in [
            ("fixed", RoutingMode::Fixed),
            ("decision_assisted", RoutingMode::DecisionAssisted),
        ] {
            let cfg = load(&format!("\n[routing]\nmode = \"{text}\"\n")).expect("config");
            assert_eq!(cfg.routing.mode, mode);
        }
    }

    #[test]
    fn unknown_mode_or_field_is_a_load_error() {
        assert!(load("\n[routing]\nmode = \"auto\"\n").is_err());
        assert!(load("\n[routing]\nmode = \"Fixed\"\n").is_err());
        assert!(load("\n[routing]\nmodel = \"x\"\n").is_err());
    }
}
