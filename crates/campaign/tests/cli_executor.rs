//! `--executor` の選択（mock|harness|celeris）。実 API・LLM は呼ばない。
use std::{path::Path, process::Command};

use serde_json::Value;

fn fx() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn run(extra: &[&str], out: &Path, envs: &[(&str, &str)]) -> std::process::Output {
    cmd("run", extra, out, envs)
}

fn cmd(sub: &str, extra: &[&str], out: &Path, envs: &[(&str, &str)]) -> std::process::Output {
    let mut c = Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("bin"));
    c.args([sub, "--manifest"])
        .arg(fx().join("pilot.json"))
        .arg("--out")
        .arg(out)
        .arg("--repo-src")
        .arg(fx().join("repo"))
        .arg("--materials-src")
        .arg(fx().join("materials"))
        .arg("--kb-src")
        .arg(fx().join("kb"))
        .args(extra);
    for (k, v) in envs {
        c.env(k, v);
    }
    c.output().expect("run")
}

#[test]
fn mock_run_prints_ledger_summary() {
    let d = tempfile::tempdir().expect("dir");
    let o = run(&["--executor", "mock"], &d.path().join("o"), &[]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v: Value = serde_json::from_slice(&o.stdout).expect("json");
    assert_eq!(v["executed"], 12);
    assert!(v["ledger"]["rows"].is_u64());
}

#[test]
fn unknown_and_missing_executor_are_errors() {
    let d = tempfile::tempdir().expect("dir");
    let o = run(&["--executor", "bogus"], &d.path().join("o"), &[]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("expected mock|harness|celeris"));
    let o = run(&[], &d.path().join("o2"), &[]);
    assert!(!o.status.success());
}

#[test]
fn celeris_requires_url_and_token_env_and_never_prints_the_secret() {
    let d = tempfile::tempdir().expect("dir");
    let secret = "s3cret-token-value-xyz";
    let o = run(
        &["--executor", "celeris", "--token-env", "CELERIS_TEST_TOKEN"],
        &d.path().join("a"),
        &[("CELERIS_TEST_TOKEN", secret)],
    );
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("--api-url"));
    // 環境変数が無ければ名前だけを含む Err。
    let o = run(
        &[
            "--executor",
            "celeris",
            "--api-url",
            "http://127.0.0.1:1",
            "--token-env",
            "CELERIS_TEST_TOKEN_UNSET",
        ],
        &d.path().join("b"),
        &[],
    );
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("CELERIS_TEST_TOKEN_UNSET"));
    for out in [&o.stdout, &o.stderr] {
        assert!(!String::from_utf8_lossy(out).contains(secret));
    }
}

#[test]
fn watch_with_mock_settles_and_reports_iterations() {
    let d = tempfile::tempdir().expect("dir");
    let o = cmd(
        "watch",
        &[
            "--executor",
            "mock",
            "--interval-secs",
            "1",
            "--max-iterations",
            "3",
        ],
        &d.path().join("o"),
        &[],
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v: Value = serde_json::from_slice(&o.stdout).expect("json");
    assert_eq!(v["executed"], 12);
    assert_eq!(v["watch"]["end"], "settled");
    assert_eq!(v["watch"]["iterations"], 1);
    // 再起動（同じ out）: 台帳から再開し、再実行しない。
    let o = cmd(
        "watch",
        &[
            "--executor",
            "mock",
            "--interval-secs",
            "1",
            "--max-iterations",
            "3",
        ],
        &d.path().join("o"),
        &[],
    );
    let v: Value = serde_json::from_slice(&o.stdout).expect("json");
    assert_eq!(
        (v["executed"].as_u64(), v["resumed_skipped"].as_u64()),
        (Some(0), Some(12))
    );
    let o = cmd(
        "watch",
        &["--executor", "mock", "--interval-secs", "0"],
        &d.path().join("o"),
        &[],
    );
    assert!(!o.status.success());
}
