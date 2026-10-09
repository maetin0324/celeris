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

/// D4.1: `[[maintenance.disk_watch]]` を省略すると `/`・`/local`・`/tmp` を 80% / 95% で監視する。
/// 書けば置き換え（しきい値の省略は既定）、`disk_watch = []` で監視しない。
#[test]
fn disk_watch_config_defaults_and_overrides() {
    let cfg = load("").unwrap();
    let entries = cfg.disk_watch_entries();
    assert_eq!(
        entries.iter().map(|e| e.path.clone()).collect::<Vec<_>>(),
        vec![
            PathBuf::from("/"),
            PathBuf::from("/local"),
            PathBuf::from("/tmp")
        ]
    );
    assert!(
        entries
            .iter()
            .all(|e| e.warn_pct == 80.0 && e.critical_pct == 95.0)
    );

    let cfg = load(
        "\n[[maintenance.disk_watch]]\npath = \"/srv\"\nwarn_pct = 70\n\n[[maintenance.disk_watch]]\npath = \"/data\"\ncritical_pct = 90\n",
    )
    .unwrap();
    let entries = cfg.disk_watch_entries();
    assert_eq!(entries.len(), 2);
    assert_eq!((entries[0].warn_pct, entries[0].critical_pct), (70.0, 95.0));
    assert_eq!((entries[1].warn_pct, entries[1].critical_pct), (80.0, 90.0));

    let cfg = load("\n[maintenance]\ndisk_watch = []\n").unwrap();
    assert!(cfg.disk_watch_entries().is_empty());
}

/// D4.1: 相対 path・しきい値の逆転・100 超え・重複は設定の読み込みで落とす。例の設定は既定どおり。
#[test]
fn disk_watch_config_rejects_invalid_entries() {
    for (body, needle) in [
        ("path = \"rel\"", "absolute"),
        ("path = \"/a\"\nwarn_pct = 96", "thresholds"),
        ("path = \"/a\"\ncritical_pct = 101", "thresholds"),
        ("path = \"/a\"\nwarn_pct = 0", "thresholds"),
        ("path = \"/a\"\nbogus = 1", "bogus"),
    ] {
        let err = load(&format!("\n[[maintenance.disk_watch]]\n{body}\n"))
            .expect_err(body)
            .to_string();
        assert!(err.contains(needle), "{needle:?} not in {err:?}");
    }
    let err = load("\n[[maintenance.disk_watch]]\npath = \"/a\"\n\n[[maintenance.disk_watch]]\npath = \"/a\"\n")
        .expect_err("duplicate")
        .to_string();
    assert!(err.contains("duplicate"), "{err}");

    let example = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config/celeris.example.toml"
    ));
    let cfg = Config::load(example).unwrap();
    assert_eq!(
        cfg.maintenance.disk_watch,
        crate::config::MaintenanceConfig::default().disk_watch
    );
}

#[test]
fn target_sweep_scope_defaults_and_disable_are_resolved() {
    let default = load("").unwrap().target_sweep_scope();
    assert!(default.scratch_targets);
    assert_eq!(default.workspace_target_after_secs, 6 * 3600);
    let disabled = load(
        "\n[maintenance.target_sweep]\nscratch_targets = false\nworkspace_target_after_hours = 0\n",
    )
    .unwrap()
    .target_sweep_scope();
    assert!(!disabled.scratch_targets);
    assert_eq!(disabled.workspace_target_after_secs, 0);
}
