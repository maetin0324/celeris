//! deny list を file で渡す probe・preflight（ADR 2026-10-31 付記: 引数に path を並べない）と、
//! 測定用 claude の固定 copy の sha256 照合（`--claude` / `--claude-sha256`）。
//! 外部ネットワーク・LLM は使わない。bwrap を使う試験は CELERIS_USERNS_TESTS=1 のときだけ走る。
use std::{fs, path::PathBuf};

use celeris_campaign::preflight_isolation::{
    IsolationInputs, ProbeRequest, Wrap, claude_findings, load_deny_list, run,
};
use sha2::{Digest, Sha256};

fn synthetic(dir: &std::path::Path, n: usize) -> Vec<PathBuf> {
    let long = "y".repeat(200);
    (0..n)
        .map(|i| dir.join(format!("absent-{i}-{long}")).join("secret"))
        .collect()
}

fn write_list(dir: &std::path::Path, paths: &[PathBuf]) -> PathBuf {
    let f = dir.join("deny.list");
    let text: String = paths.iter().map(|p| format!("{}\n", p.display())).collect();
    fs::write(&f, text).unwrap();
    f
}

#[test]
fn deny_list_of_15000_paths_loads_in_full() {
    let d = tempfile::tempdir().unwrap();
    let paths = synthetic(d.path(), 15_000);
    let f = write_list(d.path(), &paths);
    assert_eq!(load_deny_list(&f).unwrap(), paths);
}

#[test]
fn claude_copy_sha256_must_match_the_approved_one() {
    let d = tempfile::tempdir().unwrap();
    let copy = d.path().join("claude");
    fs::write(&copy, b"claude-binary").unwrap();
    let good = format!("{:x}", Sha256::digest(b"claude-binary"));
    let iso = |c: Option<&PathBuf>, h: Option<&str>| IsolationInputs {
        claude: c.cloned(),
        claude_sha256: h.map(str::to_string),
        ..Default::default()
    };
    assert!(claude_findings(&iso(Some(&copy), Some(&good))).is_empty());
    assert!(claude_findings(&iso(Some(&copy), Some(&good.to_uppercase()))).is_empty());
    assert!(claude_findings(&iso(None, None)).is_empty());
    let bad = claude_findings(&iso(Some(&copy), Some(&"0".repeat(64))));
    assert!(bad[0].starts_with("deny_readable:claude:") && bad[0].contains("differs"));
    assert_eq!(claude_findings(&iso(Some(&copy), None)).len(), 1);
    assert_eq!(claude_findings(&iso(None, Some(&good))).len(), 1);
    let gone = d.path().join("gone");
    assert_eq!(claude_findings(&iso(Some(&gone), Some(&good))).len(), 1);
}

/// 15,000 件の deny file を bwrap の中の probe-read へ渡しても引数上限に当たらず、全件が拒否と判定される。
#[test]
fn bwrap_probe_runs_with_15000_deny_paths() {
    let setting = std::env::var("CELERIS_USERNS_TESTS").unwrap_or_default();
    if setting != "1" && setting != "require" {
        eprintln!("skip: bwrap deny-file probe requires CELERIS_USERNS_TESTS=1 (or require)");
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let stage = d.path().join("ws");
    fs::create_dir_all(&stage).unwrap();
    let deny = synthetic(d.path(), 15_000);
    let args: Vec<String> = [
        "--unshare-user",
        "--ro-bind",
        "/",
        "/",
        "--dev",
        "/dev",
        "--proc",
        "/proc",
        "--bind",
        stage.to_str().unwrap(),
        stage.to_str().unwrap(),
    ]
    .map(String::from)
    .to_vec();
    let req = ProbeRequest {
        principal: "A",
        program: PathBuf::from(std::env::var("CARGO_BIN_EXE_celeris-campaign").unwrap()),
        wrap: Wrap::Bwrap { args, stage },
        deny,
    };
    let p = run(&req).expect("probe ran without hitting the argv limit");
    assert_eq!(p.paths.len(), 15_000);
    assert!(p.paths.iter().all(|x| !x.readable));
}
