//! ADR 2026-10-07-build-tmp-hygiene D1.1・D1.2: `[maintenance.target_sweep]` の既定値・上書き・検証。

use std::path::{Path, PathBuf};

use task_worker::target_sweep::SweepParams;

use crate::config::{Config, DEFAULT_DEV_BUILD_CACHE, TargetSweepConfig};

const BASE: &str = r#"
db = "t.sqlite3"

[[providers]]
id = "fake"
adapter = "fake"

[workspace]
build_cache_dir = "/srv/build-cache"
"#;

fn load(extra: &str) -> Result<Config, crate::config::ConfigError> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, format!("{BASE}{extra}")).unwrap();
    Config::load(&path)
}

/// 省略時は `SweepParams::default()` の上限と、既定の 2 つの roots。
#[test]
fn target_sweep_config_defaults_match_sweep_params_default() {
    let cfg = load("").unwrap();
    assert_eq!(cfg.maintenance.target_sweep, TargetSweepConfig::default());
    let params = cfg.target_sweep_params();
    let d = SweepParams::default();
    assert_eq!(params.max_age_days, d.max_age_days);
    assert_eq!(params.max_bytes_per_root, d.max_bytes_per_root);
    assert_eq!(params.target_ratio, d.target_ratio);
    assert_eq!(params.stale_target_days, d.stale_target_days);
    assert_eq!(
        params.roots,
        vec![
            PathBuf::from("/srv/build-cache/cargo"),
            PathBuf::from(DEFAULT_DEV_BUILD_CACHE)
        ]
    );
    // `[maintenance]` だけ書いても同じ。
    let cfg = load("\n[maintenance.target_sweep]\n").unwrap();
    assert_eq!(cfg.maintenance.target_sweep, TargetSweepConfig::default());
}

/// 書いた欄だけ上書きし、相対 roots は設定ファイル基準に直す。
#[test]
fn target_sweep_config_overrides_and_resolves_relative_roots() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        format!(
            "{BASE}\n[maintenance.target_sweep]\nroots = [\"/abs/t\", \"rel/t\"]\nmax_age_days = 3\ntarget_ratio = 0.5\n"
        ),
    )
    .unwrap();
    let cfg = Config::load(&path).unwrap();
    let params = cfg.target_sweep_params();
    assert_eq!(
        params.roots,
        vec![PathBuf::from("/abs/t"), dir.path().join("rel/t")]
    );
    assert_eq!(params.max_age_days, 3);
    assert_eq!(params.target_ratio, 0.5);
    assert_eq!(
        params.max_bytes_per_root,
        SweepParams::default().max_bytes_per_root
    );
}

#[test]
fn target_sweep_config_rejects_invalid_values() {
    for (body, needle) in [
        ("max_age_days = 0", "max_age_days"),
        ("max_bytes_per_root = 0", "max_bytes_per_root"),
        ("target_ratio = 1.5", "target_ratio"),
        ("target_ratio = 0.0", "target_ratio"),
        ("roots = [\"/\"]", "roots"),
        ("bogus = 1", "bogus"),
    ] {
        let err = load(&format!("\n[maintenance.target_sweep]\n{body}\n"))
            .expect_err(body)
            .to_string();
        assert!(err.contains(needle), "{needle:?} not in {err:?}");
    }
}

/// 例の設定の `[maintenance.target_sweep]` は既定値どおり（roots は省略）。
#[test]
fn target_sweep_config_example_toml_matches_defaults() {
    let example = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config/celeris.example.toml"
    ));
    let cfg = Config::load(example).unwrap();
    assert_eq!(cfg.maintenance.target_sweep, TargetSweepConfig::default());
}
