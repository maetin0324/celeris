//! `[routing_eval]`（ADR 2026-10-11-llm-source-model-routing-evaluation 付記「評価 run の受け口」）:
//! 評価 run（routing-eval の試行）専用の予約枠。
//!
//! 評価 run は本番の全体並列度（`max_concurrency`）に数えず、`reserved_runs` の中だけで走る。
//! 既定 `0` = 評価 run を起こさない（受け口は動くが枠が無いので何も dispatch しない）。
//! account の同時実行と quota 上限は本番と同じ規則で効く（`task_dispatch::capacity::eval_admission`）。

use serde::Deserialize;

/// ```toml
/// [routing_eval]
/// reserved_runs = 2
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingEvalConfig {
    /// 評価 run 専用の予約枠（同時に走らせる評価 run の上限）。既定 0（評価 run を起こさない）。
    #[serde(default = "default_reserved_runs")]
    pub reserved_runs: usize,
}

impl Default for RoutingEvalConfig {
    fn default() -> Self {
        Self {
            reserved_runs: default_reserved_runs(),
        }
    }
}

/// 既定は 0: 評価の予約枠を作らなければ評価 run は起こらない（本番の並列度を食わない）。
fn default_reserved_runs() -> usize {
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    const BASE: &str = r#"
db = "t.sqlite3"
"#;

    fn load(extra: &str) -> Result<Config, crate::config::ConfigError> {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, format!("{BASE}{extra}")).expect("write config");
        Config::load(&path)
    }

    /// 省略時の既定は 0（評価 run を起こさない）。`[routing_eval]` だけ書いても同じ。
    #[test]
    fn routing_eval_defaults_to_zero_reserved_runs() {
        assert_eq!(load("").expect("config").routing_eval.reserved_runs, 0);
        assert_eq!(
            load("\n[routing_eval]\n").expect("config").routing_eval,
            RoutingEvalConfig::default()
        );
        assert_eq!(RoutingEvalConfig::default().reserved_runs, 0);
    }

    /// 書いた値が読め、dispatcher の設定（`DispatchConfig.eval`）まで届く。
    #[test]
    fn routing_eval_reserved_runs_reaches_dispatch_config() {
        let cfg = load("\n[routing_eval]\nreserved_runs = 3\n").expect("config");
        assert_eq!(cfg.routing_eval.reserved_runs, 3);
        assert_eq!(cfg.dispatch_config().eval.reserved_runs, 3);
        // 既定（0）でも届く。
        assert_eq!(
            load("")
                .expect("config")
                .dispatch_config()
                .eval
                .reserved_runs,
            0
        );
    }

    /// 綴り間違い・未知の欄は設定エラー（`deny_unknown_fields`）。
    #[test]
    fn routing_eval_rejects_unknown_fields() {
        let err = load("\n[routing_eval]\nreserved_run = 3\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("routing_eval"), "{err}");
        assert!(load("\n[routing_eval]\nreserved_runs = 3\nquota = 1\n").is_err());
    }
}
