//! ADR-0131 D6 / 付記 D10: `[[cron.seed]]` の解析・検証と、空の `cron_jobs` への一度だけの投入。

use std::path::Path;

use task_core::{CronCatchUp, CronJobStore, CronOverlap, CronTaskTemplate, SqliteStore, Tier};
use time::OffsetDateTime;

use crate::config::{Config, ConfigError};
use crate::{DaemonError, seed_cron_if_empty};

fn example_path() -> &'static Path {
    Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config/celeris.example.toml"
    ))
}

/// 2026-10-03T00:00:00Z（注入した時計）。
fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_790_985_600).unwrap()
}

const BASE: &str = r#"
db = "t.sqlite3"

[[providers]]
id = "fake"
adapter = "fake"

[[harnesses]]
id = "knowledge-curation"
tier = "cheap"
"#;

const SEED: &str = r#"
[[cron.seed]]
name = "daily-curation"
schedule = "30 4 * * *"
timezone = "Asia/Tokyo"

[cron.seed.template]
title = "日次整理: {date}"
objective = "_inbox を空にする"
harness = "knowledge-curation"
lane = "cheap"
mode = "dry_run"
acceptance = [{ type = "artifact_exists", name = "daily-summary.md" }]
"#;

fn load(extra: &str) -> (tempfile::TempDir, Result<Config, ConfigError>) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, format!("{BASE}{extra}")).unwrap();
    let cfg = Config::load(&path);
    (dir, cfg)
}

fn open_store(dir: &tempfile::TempDir) -> SqliteStore {
    SqliteStore::open(&dir.path().join("t.sqlite3")).unwrap()
}

#[test]
fn cron_seed_parses_fields_and_defaults() {
    let (_dir, cfg) = load(SEED);
    let cfg = cfg.unwrap();
    let [seed] = cfg.cron.seed.as_slice() else {
        panic!("{:?}", cfg.cron.seed)
    };
    assert_eq!(seed.name, "daily-curation");
    assert!(!seed.enabled, "種は既定で無効");
    assert_eq!(seed.overlap, CronOverlap::Skip);
    assert_eq!(seed.catch_up, CronCatchUp::Latest);
    assert_eq!(seed.template.lane, Some(Tier::Cheap));
    assert_eq!(seed.template.harness.as_deref(), Some("knowledge-curation"));
    assert_eq!(seed.template.extra.get("mode"), Some(&"dry_run".into()));
    assert_eq!(seed.template.acceptance.len(), 1);
    // `[cron]` を書かなければ種は無い。
    let (_dir, cfg) = load("");
    assert!(cfg.unwrap().cron.seed.is_empty());
}

#[test]
fn cron_seed_rejects_invalid_seeds_at_load() {
    let cases = [
        (SEED.replace("mode = \"dry_run\"", "mode = \"wet\""), "mode"),
        (SEED.replace("30 4 * * *", "61 4 * * *"), "daily-curation"),
        (SEED.replace("Asia/Tokyo", "Mars/Olympus"), "daily-curation"),
        (
            SEED.replace("harness = \"knowledge-curation\"", "harness = \"nope\""),
            "nope",
        ),
        (format!("{SEED}{SEED}"), "duplicate"),
        (
            SEED.replace("name = \"daily-curation\"", "name = \" \""),
            "blank",
        ),
        (
            SEED.replace(
                "timezone = \"Asia/Tokyo\"",
                "timezone = \"Asia/Tokyo\"\nbogus = 1",
            ),
            "bogus",
        ),
    ];
    for (toml, needle) in cases {
        let (_dir, cfg) = load(&toml);
        let err = cfg.expect_err(&toml).to_string();
        assert!(err.contains(needle), "{needle:?} not in {err:?}");
    }
}

#[test]
fn cron_seed_inserts_once_into_empty_cron_jobs() {
    let (dir, cfg) = load(SEED);
    let cfg = cfg.unwrap();
    let store = open_store(&dir);

    assert_eq!(seed_cron_if_empty(&store, &cfg, now()).unwrap(), 1);
    let jobs = store.cron_job_list().unwrap();
    let [job] = jobs.as_slice() else {
        panic!("{jobs:?}")
    };
    assert_eq!(job.name, "daily-curation");
    assert!(!job.enabled);
    assert_eq!(job.next_fire_at, None, "無効の job は次回時刻を持たない");
    assert_eq!(job.template.extra.get("mode"), Some(&"dry_run".into()));
    assert_eq!(job.created_at, now());

    // 2 回目の起動: 重複も上書きもしない（人が変えた値が残る）。
    let mut edited = job.clone();
    edited.schedule = "0 6 * * *".into();
    edited.template.extra.insert("mode".into(), "apply".into());
    store.cron_job_update(&edited).unwrap();
    let later = now() + time::Duration::days(1);
    assert_eq!(seed_cron_if_empty(&store, &cfg, later).unwrap(), 0);
    let jobs = store.cron_job_list().unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].schedule, "0 6 * * *");
    assert_eq!(jobs[0].template.extra.get("mode"), Some(&"apply".into()));
    assert_eq!(jobs[0].created_at, now());
}

#[test]
fn cron_seed_is_not_applied_when_cron_jobs_already_has_a_job() {
    let (dir, cfg) = load(SEED);
    let cfg = cfg.unwrap();
    let store = open_store(&dir);
    let new = task_ops::cron_jobs::NewCronJob {
        name: "other".into(),
        schedule: "@daily".into(),
        timezone: "UTC".into(),
        overlap: CronOverlap::Skip,
        catch_up: CronCatchUp::Latest,
        enabled: true,
        template: CronTaskTemplate {
            title: "other {date}".into(),
            objective: "other".into(),
            acceptance: vec![serde_json::json!({"type": "artifact_exists", "name": "out.md"})],
            ..CronTaskTemplate::default()
        },
    };
    task_ops::cron_jobs::create_job(&store, &Default::default(), new, now()).unwrap();
    assert_eq!(seed_cron_if_empty(&store, &cfg, now()).unwrap(), 0);
    let names: Vec<String> = store
        .cron_job_list()
        .unwrap()
        .into_iter()
        .map(|j| j.name)
        .collect();
    assert_eq!(names, vec!["other".to_string()]);
}

/// 雛形の誤り（DB を見ないと分からない: 存在しない案件・受け入れ条件なし）は投入時の設定エラーで、
/// 前の正しい種も含めて 1 件も書かない。
#[test]
fn cron_seed_with_an_invalid_template_is_a_config_error_and_writes_nothing() {
    let second = SEED
        .replace("daily-curation", "weekly")
        .replace("mode = \"dry_run\"", "project = \"no-such-project\"");
    for bad in [
        second,
        SEED.replace("daily-curation", "weekly").replace(
            "acceptance = [{ type = \"artifact_exists\", name = \"daily-summary.md\" }]",
            "",
        ),
    ] {
        let (dir, cfg) = load(&format!("{SEED}{bad}"));
        let cfg = cfg.unwrap();
        let store = open_store(&dir);
        let err = seed_cron_if_empty(&store, &cfg, now()).unwrap_err();
        assert!(
            matches!(&err, DaemonError::Config(ConfigError::Invalid(m)) if m.contains("weekly")),
            "{err:?}"
        );
        assert!(store.cron_job_list().unwrap().is_empty());
    }
}

/// 例の設定: daily-curation の種（無効・dry_run・cheap・overlap skip・catch_up latest）と
/// knowledge-curation harness（tier cheap、fallback あり）があり、空の DB に 1 件入る。
#[test]
fn cron_seed_example_toml_has_daily_curation_and_knowledge_curation_harness() {
    let cfg = Config::load(example_path()).unwrap();
    let [seed] = cfg.cron.seed.as_slice() else {
        panic!("{:?}", cfg.cron.seed)
    };
    assert_eq!(seed.name, "daily-curation");
    assert!(!seed.enabled);
    assert_eq!(seed.overlap, CronOverlap::Skip);
    assert_eq!(seed.catch_up, CronCatchUp::Latest);
    assert_eq!(seed.template.lane, Some(Tier::Cheap));
    assert_eq!(seed.template.harness.as_deref(), Some("knowledge-curation"));
    assert_eq!(seed.template.extra.get("mode"), Some(&"dry_run".into()));
    for word in [
        "_inbox",
        "human_decisions",
        "curation-plan.json",
        "curation.diff",
        "daily-summary.md",
    ] {
        assert!(
            seed.template.objective.contains(word),
            "objective に {word} が無い"
        );
    }

    let harness = cfg
        .harnesses
        .iter()
        .find(|h| h.id == "knowledge-curation")
        .expect("knowledge-curation harness");
    assert_eq!(harness.tier, Some(Tier::Cheap));
    assert_eq!(harness.adapter, None, "coding 系の汎用 adapter");
    assert!(harness.fallback.is_some());

    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(&dir.path().join("t.sqlite3")).unwrap();
    assert_eq!(seed_cron_if_empty(&store, &cfg, now()).unwrap(), 1);
    assert_eq!(seed_cron_if_empty(&store, &cfg, now()).unwrap(), 0);
    assert_eq!(store.cron_job_list().unwrap().len(), 1);
}

#[test]
fn seed_objective_describes_curation_content_hashes_and_diff_scope() {
    let cfg = Config::load(example_path()).unwrap();
    let [seed] = cfg.cron.seed.as_slice() else {
        panic!("{:?}", cfg.cron.seed)
    };
    let objective = &seed.template.objective;
    for word in [
        "content",
        "expected_hash",
        "target_hash",
        "sha256sum inputs/kb/<path>",
        "new/fix/merge",
        "delete/keep",
        "curation.diff",
        "変更する path だけ",
        "1 件の decision",
    ] {
        assert!(objective.contains(word), "objective に {word} が無い");
    }
}
