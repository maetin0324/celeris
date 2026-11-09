use super::*;

use std::fs;
use std::path::PathBuf;

use task_core::required_checks::health::{
    CHECKER_DIR, CHECKER_SCRIPT, CheckerId, CheckerSource, SOURCE_SIZE_REPORT, base_checker,
    checker_sha256,
};
use task_dispatch::health_review::gate::CHECKER_TREE_FILE;

const RELEASE: &str = "0123456789ab";
const TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const BLOB: &str = "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391";

/// `<root>/releases/<sha12>/` に release.sh の `sd_bundle_repo_health` と同じ配置の同梱
/// （`repo-health/scripts/dev/repo-health/repo_health.py`・`repo-health/scripts/dev/source-size-report.py`、
/// `checker_tree` があれば `repo-health/CHECKER_TREE`）を置き、`<release>/bin/celeris` の path を返す。
fn release_with_bundle(root: &Path, checker_tree: Option<&str>) -> PathBuf {
    let release = root.join("releases").join(RELEASE);
    let bundle = release.join(BUNDLE_DIR);
    let script = bundle.join(CHECKER_SCRIPT);
    fs::create_dir_all(script.parent().unwrap()).unwrap();
    fs::write(&script, "# stub\n").unwrap();
    fs::write(bundle.join(SOURCE_SIZE_REPORT), "# stub\n").unwrap();
    if let Some(text) = checker_tree {
        fs::write(bundle.join(CHECKER_TREE_FILE), text).unwrap();
    }
    fs::create_dir_all(release.join("bin")).unwrap();
    release.join("bin").join("celeris")
}

/// 不変条件: daemon は release.sh が書く `<release>/repo-health/` を実行中の binary の release dir から見つけ、
/// `CHECKER_TREE` の 2 行から作る版（source は Bundled）は、同じ tree id・blob id の base の検査器の版
/// （task_core の `base_checker`）と同じ値になる。
#[test]
fn health_evidence_bundled_checker_tree_gives_base_formula() {
    let root = tempfile::tempdir().unwrap();
    let exe = release_with_bundle(
        root.path(),
        Some(&format!("repo-health {TREE}\nsource-size-report {BLOB}\n")),
    );
    let found = bundled_checker_for(Some(&exe), Some(RELEASE)).expect("bundled checker");
    assert_eq!(
        found.dir,
        root.path()
            .join("releases")
            .join(RELEASE)
            .join("repo-health")
    );
    assert_eq!(
        found.id,
        CheckerId {
            source: CheckerSource::Bundled,
            sha256: checker_sha256(TREE, Some(BLOB)),
        }
    );

    // 同じ commit の検査器を base から読んだときの版（git は rev-parse の答えだけを返す偽物）。
    let base = "b".repeat(40);
    let from_base = base_checker(&base, |args| match args {
        ["cat-file", "-e", _] => Ok(String::new()),
        ["rev-parse", "--verify", object] if *object == format!("{base}:{CHECKER_DIR}") => {
            Ok(format!("{TREE}\n"))
        }
        ["rev-parse", "--verify", object] if *object == format!("{base}:{SOURCE_SIZE_REPORT}") => {
            Ok(format!("{BLOB}\n"))
        }
        other => Err(format!("unexpected git {other:?}")),
    })
    .expect("base checker")
    .expect("present");
    assert_eq!(from_base.source, CheckerSource::Base);
    assert_eq!(from_base.sha256, found.id.sha256);
}

/// 不変条件: release dir はあるが `CHECKER_TREE` が無い・壊れている（hex でない id、未知の行、repo-health の行が
/// 無い）か `repo-health/` 自体が無い release では、daemon は同梱の検査器を持たない（版を推測しない）。
#[test]
fn health_evidence_bundled_missing_or_broken_checker_tree_is_none() {
    let cases: [(&str, Option<String>); 5] = [
        ("missing", None),
        ("not hex", Some("repo-health not-a-hex-id\n".to_string())),
        (
            "unknown line",
            Some(format!("repo-health {TREE}\nsomething-else {BLOB}\n")),
        ),
        (
            "no repo-health line",
            Some(format!("source-size-report {BLOB}\n")),
        ),
        ("empty", Some(String::new())),
    ];
    for (name, text) in cases {
        let root = tempfile::tempdir().unwrap();
        let exe = release_with_bundle(root.path(), text.as_deref());
        match bundled_checker_for(Some(&exe), Some(RELEASE)) {
            Err(NoBundle::Unreadable(why)) => {
                assert!(why.contains(CHECKER_TREE_FILE), "{name}: {why}")
            }
            other => panic!("{name}: {other:?}"),
        }
    }

    // release dir に repo-health/ が無い（同梱の前の release）。
    let root = tempfile::tempdir().unwrap();
    let release = root.path().join("releases").join(RELEASE);
    fs::create_dir_all(release.join("bin")).unwrap();
    let exe = release.join("bin").join("celeris");
    assert!(matches!(
        bundled_checker_for(Some(&exe), Some(RELEASE)),
        Err(NoBundle::Unreadable(_))
    ));
}

/// 不変条件: `--release` で起動していない（開発の起動）・`--release` が空・実行中の binary の path が分からない
/// daemon は、同じ場所に正しい同梱があっても同梱の検査器を持たない（release dir を推測しない）。
#[test]
fn health_evidence_bundled_without_release_is_none() {
    let root = tempfile::tempdir().unwrap();
    let exe = release_with_bundle(
        root.path(),
        Some(&format!("repo-health {TREE}\nsource-size-report {BLOB}\n")),
    );
    assert_eq!(
        bundled_checker_for(Some(&exe), None),
        Err(NoBundle::NoReleaseDir)
    );
    assert_eq!(
        bundled_checker_for(Some(&exe), Some("  ")),
        Err(NoBundle::NoReleaseDir)
    );
    assert_eq!(
        bundled_checker_for(None, Some(RELEASE)),
        Err(NoBundle::NoReleaseDir)
    );
    // 同じ同梱でも release の指定があれば読める（上の None が同梱の不備でないことの対照）。
    assert!(bundled_checker_for(Some(&exe), Some(RELEASE)).is_ok());
}
