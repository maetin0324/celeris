//! manifest の検証と配備の試験。bundle は `tests/fixtures/selftest-bundle/` の手書きの JS で、業務 TS ではない。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use celeris_script::ScriptRuntimeHandle;
use celeris_script::dto::Capability;
use celeris_script::manifest::{
    BUNDLE_FILE, MANIFEST_FILE, ManifestError, deploy_bundle, deployed_dir, load_bundle_dir,
    load_deployed, sha256_hex,
};
use celeris_script::selftest::{check_output, selftest_call};
use serde_json::{Value, json};
use tempfile::TempDir;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/selftest-bundle")
}

/// fixture を一時 dir に写す。
fn dist() -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for name in [BUNDLE_FILE, MANIFEST_FILE] {
        fs::copy(fixture().join(name), dir.path().join(name)).expect("copy fixture");
    }
    dir
}

/// manifest の JSON を書き換える。
fn edit_manifest(dir: &Path, f: impl FnOnce(&mut Value)) {
    let path = dir.join(MANIFEST_FILE);
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(&path).expect("read manifest")).expect("parse manifest");
    f(&mut manifest);
    fs::write(&path, serde_json::to_vec_pretty(&manifest).expect("json")).expect("write manifest");
}

fn sha_of_fixture_bundle() -> String {
    sha256_hex(&fs::read(fixture().join(BUNDLE_FILE)).expect("read bundle"))
}

#[test]
fn manifest_hash_mismatch() {
    let dir = dist();
    fs::write(dir.path().join(BUNDLE_FILE), "globalThis.x = 1;\n").expect("tamper");
    assert!(matches!(
        load_bundle_dir(dir.path()),
        Err(ManifestError::HashMismatch { .. })
    ));
}

#[test]
fn manifest_host_api_major_mismatch() {
    let dir = dist();
    edit_manifest(dir.path(), |m| m["host_api"] = json!("^2"));
    assert!(matches!(
        load_bundle_dir(dir.path()),
        Err(ManifestError::HostApiMajorMismatch { .. })
    ));
    // `^<major>` 以外の形は major を読まずに拒否する。
    for form in ["1", "^1.2", ">=1", "^", "^x"] {
        let dir = dist();
        edit_manifest(dir.path(), |m| m["host_api"] = json!(form));
        assert!(
            matches!(
                load_bundle_dir(dir.path()),
                Err(ManifestError::InvalidHostApi { .. })
            ),
            "{form}"
        );
    }
}

#[test]
fn manifest_schema_mismatch() {
    let dir = dist();
    edit_manifest(dir.path(), |m| m["schema_sha256"] = json!("0".repeat(64)));
    assert!(matches!(
        load_bundle_dir(dir.path()),
        Err(ManifestError::SchemaMismatch { .. })
    ));
    let dir = dist();
    edit_manifest(dir.path(), |m| {
        m["schema"] = json!("celeris.script-manifest/2")
    });
    assert!(matches!(
        load_bundle_dir(dir.path()),
        Err(ManifestError::UnsupportedSchema { .. })
    ));
}

#[test]
fn manifest_unknown_field() {
    let edits: [fn(&mut Value); 3] = [
        |m| m["extra"] = json!(1),
        |m| m["toolchain"]["bun"] = json!("1"),
        // 未知の capability も欄と同じく拒否する。
        |m| m["capabilities"] = json!(["fs:read"]),
    ];
    for edit in edits {
        let dir = dist();
        edit_manifest(dir.path(), edit);
        assert!(matches!(
            load_bundle_dir(dir.path()),
            Err(ManifestError::Invalid { .. })
        ));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manifest_load_fixture() {
    // schema を変えたら tests/fixtures/selftest-bundle/manifest.json の schema_sha256 を直す。
    let bundle = load_bundle_dir(&fixture()).expect("fixture verifies");
    assert_eq!(
        bundle.manifest.capabilities,
        [Capability::SelftestEchoTask].into()
    );
    let runtime = ScriptRuntimeHandle::start(bundle.source()).expect("start");
    let reply = runtime
        .call(selftest_call(Duration::from_secs(30)))
        .await
        .expect("call");
    check_output(&reply.output).expect("selftest output");
}

#[test]
fn deploy_copies_and_reload_verifies() {
    let dir = dist();
    let state = tempfile::tempdir().expect("state");
    let sha = sha_of_fixture_bundle();
    let deployed = deploy_bundle(dir.path(), state.path()).expect("deploy");
    assert_eq!(deployed.manifest.bundle_sha256, sha);
    assert_eq!(load_deployed(state.path(), &sha).expect("load"), deployed);
    // 一時 dir を残さない。
    let entries: Vec<_> = fs::read_dir(state.path().join("script-bundles"))
        .expect("read_dir")
        .map(|e| e.expect("entry").file_name())
        .collect();
    assert_eq!(entries, [std::ffi::OsString::from(&sha)]);

    // 配備後に書き換わった bundle は読まない。
    fs::write(
        deployed_dir(state.path(), &sha).join(BUNDLE_FILE),
        "globalThis.x = 1;\n",
    )
    .expect("tamper");
    assert!(matches!(
        load_deployed(state.path(), &sha),
        Err(ManifestError::HashMismatch { .. })
    ));
    // 壊れた配備先を黙って再利用しない。
    assert!(matches!(
        deploy_bundle(dir.path(), state.path()),
        Err(ManifestError::HashMismatch { .. })
    ));
}

#[test]
fn deploy_rejects_invalid_dist_without_writing() {
    let dir = dist();
    edit_manifest(dir.path(), |m| m["host_api"] = json!("^2"));
    let state = tempfile::tempdir().expect("state");
    assert!(deploy_bundle(dir.path(), state.path()).is_err());
    assert!(!state.path().join("script-bundles").exists());
}

#[test]
fn deploy_again_keeps_provenance_and_rejects_contract_change() {
    let dir = dist();
    let state = tempfile::tempdir().expect("state");
    deploy_bundle(dir.path(), state.path()).expect("deploy");

    // 来歴だけが違う再配備は既存を残す。
    edit_manifest(dir.path(), |m| m["source_commit"] = json!("later"));
    let again = deploy_bundle(dir.path(), state.path()).expect("redeploy");
    assert_eq!(again.manifest.source_commit, "fixture");

    // 同じ bundle で capability が違う manifest は既存を上書きしない。
    edit_manifest(dir.path(), |m| m["capabilities"] = json!([]));
    assert!(matches!(
        deploy_bundle(dir.path(), state.path()),
        Err(ManifestError::DeployConflict { .. })
    ));
}

#[test]
fn load_deployed_rejects_path_like_hash_and_moved_dir() {
    let state = tempfile::tempdir().expect("state");
    assert!(matches!(
        load_deployed(state.path(), "../../etc"),
        Err(ManifestError::InvalidSha256 { .. })
    ));

    // 別の hash の名前の dir に置かれた bundle は読まない。
    let dir = dist();
    deploy_bundle(dir.path(), state.path()).expect("deploy");
    let other = "a".repeat(64);
    fs::rename(
        deployed_dir(state.path(), &sha_of_fixture_bundle()),
        deployed_dir(state.path(), &other),
    )
    .expect("rename");
    assert!(matches!(
        load_deployed(state.path(), &other),
        Err(ManifestError::HashMismatch { .. })
    ));
}

#[test]
fn load_check_bin_reports_json_and_fails_nonzero() {
    let bin = std::env::var("CARGO_BIN_EXE_script-load-check").expect("bin");
    let ok = Command::new(&bin)
        .arg("--bundle-dir")
        .arg(fixture())
        .output()
        .expect("run");
    assert!(ok.status.success(), "{ok:?}");
    let report: Value = serde_json::from_slice(&ok.stdout).expect("stdout is JSON");
    assert_eq!(report["bundle_sha256"], json!(sha_of_fixture_bundle()));
    check_output(&report["output"]).expect("selftest output");

    let dir = dist();
    fs::write(dir.path().join(BUNDLE_FILE), "globalThis.x = 1;\n").expect("tamper");
    let bad = Command::new(&bin)
        .arg("--bundle-dir")
        .arg(dir.path())
        .output()
        .expect("run");
    assert!(!bad.status.success());
    assert!(bad.stdout.is_empty());

    // 試験用の領域を宣言しない manifest も失敗にする。
    let dir = dist();
    edit_manifest(dir.path(), |m| m["domains"] = json!([]));
    let undeclared = Command::new(&bin)
        .arg("--bundle-dir")
        .arg(dir.path())
        .output()
        .expect("run");
    assert!(!undeclared.status.success());
}

#[test]
fn bench_bin_writes_latency_and_rss() {
    let out = tempfile::tempdir().expect("out");
    let path = out.path().join("bench.json");
    let bin = std::env::var("CARGO_BIN_EXE_script-bench").expect("bin");
    let run = Command::new(&bin)
        .arg("--bundle-dir")
        .arg(fixture())
        .args(["--iters", "5", "--out"])
        .arg(&path)
        .output()
        .expect("run");
    assert!(run.status.success(), "{run:?}");
    let report: Value = serde_json::from_slice(&fs::read(&path).expect("read")).expect("JSON");
    assert_eq!(report["iters"], json!(5));
    for value in [
        &report["latency_ms"]["p50"],
        &report["latency_ms"]["p99"],
        &report["rss_mib"]["one"],
        &report["rss_mib"]["two"],
    ] {
        assert!(value.as_f64().expect("number") > 0.0, "{report}");
    }

    let zero = Command::new(&bin)
        .arg("--bundle-dir")
        .arg(fixture())
        .args(["--iters", "0", "--out"])
        .arg(&path)
        .output()
        .expect("run");
    assert!(!zero.status.success());
}
