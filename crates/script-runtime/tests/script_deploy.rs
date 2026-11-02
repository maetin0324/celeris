//! `script-deploy` command の試験。tempdir の state dir に `tests/fixtures/selftest-bundle/` を配備する（本番の state dir には書かない）。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use celeris_script::manifest::{
    BUNDLE_FILE, BUNDLES_DIR, MANIFEST_FILE, deployed_dir, load_deployed, sha256_hex,
};
use serde_json::{Value, json};
use tempfile::TempDir;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/selftest-bundle")
}

fn dist() -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for name in [BUNDLE_FILE, MANIFEST_FILE] {
        fs::copy(fixture().join(name), dir.path().join(name)).expect("copy fixture");
    }
    dir
}

fn edit_manifest(dir: &Path, f: impl FnOnce(&mut Value)) {
    let path = dir.join(MANIFEST_FILE);
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(&path).expect("read manifest")).expect("parse manifest");
    f(&mut manifest);
    fs::write(&path, serde_json::to_vec_pretty(&manifest).expect("json")).expect("write manifest");
}

fn fixture_sha() -> String {
    sha256_hex(&fs::read(fixture().join(BUNDLE_FILE)).expect("read bundle"))
}

fn bin() -> String {
    std::env::var("CARGO_BIN_EXE_script-deploy").expect("script-deploy bin")
}

fn deploy(dist: &Path, state: &Path) -> Output {
    Command::new(bin())
        .arg("--dist")
        .arg(dist)
        .arg("--state-dir")
        .arg(state)
        .output()
        .expect("run script-deploy")
}

/// 配備先に残っている entry の名前（一時 dir の残りも見える）。
fn entries(state: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(state.join(BUNDLES_DIR))
        .expect("read bundles dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn script_deploy_new_prints_sha_and_writes_bundle() {
    let dist = dist();
    let state = tempfile::tempdir().expect("state");
    let out = deploy(dist.path(), state.path());
    assert!(out.status.success(), "{out:?}");
    let sha = fixture_sha();
    assert_eq!(
        String::from_utf8(out.stdout).expect("utf8"),
        format!("{sha}\n")
    );
    assert_eq!(entries(state.path()), vec![sha.clone()]);
    for name in [BUNDLE_FILE, MANIFEST_FILE] {
        assert_eq!(
            fs::read(deployed_dir(state.path(), &sha).join(name)).expect("deployed file"),
            fs::read(dist.path().join(name)).expect("dist file"),
        );
    }
    load_deployed(state.path(), &sha).expect("deployed bundle verifies");
}

#[test]
fn script_deploy_same_hash_reuses_existing() {
    let dist = dist();
    let state = tempfile::tempdir().expect("state");
    assert!(deploy(dist.path(), state.path()).status.success());
    let sha = fixture_sha();
    let manifest_path = deployed_dir(state.path(), &sha).join(MANIFEST_FILE);
    let first_manifest = fs::read(&manifest_path).expect("manifest");

    // 来歴（source_commit）だけが違う再配備は既存を残し、同じ hash を出す。
    edit_manifest(dist.path(), |m| m["source_commit"] = json!("later"));
    let again = deploy(dist.path(), state.path());
    assert!(again.status.success(), "{again:?}");
    assert_eq!(
        String::from_utf8(again.stdout).expect("utf8"),
        format!("{sha}\n")
    );
    assert_eq!(fs::read(&manifest_path).expect("manifest"), first_manifest);
    assert_eq!(entries(state.path()), vec![sha]);
}

#[test]
fn script_deploy_rejects_manifest_conflict_and_invalid_input() {
    let dist = dist();
    let state = tempfile::tempdir().expect("state");
    assert!(deploy(dist.path(), state.path()).status.success());
    let sha = fixture_sha();
    let manifest_path = deployed_dir(state.path(), &sha).join(MANIFEST_FILE);
    let first_manifest = fs::read(&manifest_path).expect("manifest");

    // 同じ bundle で契約（capabilities）が違う manifest は上書きせず非 0。
    edit_manifest(dist.path(), |m| m["capabilities"] = json!([]));
    let conflict = deploy(dist.path(), state.path());
    assert!(!conflict.status.success(), "{conflict:?}");
    assert!(conflict.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&conflict.stderr).contains("different contract"),
        "{conflict:?}"
    );
    assert_eq!(fs::read(&manifest_path).expect("manifest"), first_manifest);
    assert_eq!(entries(state.path()), vec![sha]);

    // bundle と manifest の hash が食い違う dist は配備先を作らずに非 0。
    let tampered = self::dist();
    fs::write(tampered.path().join(BUNDLE_FILE), "globalThis.x = 1;\n").expect("tamper");
    let fresh = tempfile::tempdir().expect("state");
    let bad = deploy(tampered.path(), fresh.path());
    assert!(!bad.status.success(), "{bad:?}");
    assert!(bad.stdout.is_empty());
    assert!(!fresh.path().join(BUNDLES_DIR).exists());

    // 引数が足りなければ非 0。
    let usage = Command::new(bin())
        .arg("--dist")
        .arg(dist.path())
        .output()
        .expect("run");
    assert!(!usage.status.success());
    assert!(usage.stdout.is_empty());
}
