//! Release の台帳検査 CLI。実 browser・ネットワーク・本番 path は使わない。
use std::path::Path;
use std::process::{Command, Output};

use serde_json::{Value, json};
use task_worker::browser::SUPPORTED_VERSION;

const RELEASE: &str = "1a2b3c4d5e6f";

fn ledger() -> Value {
    json!({
        "schema": 1,
        "source": "celeris-browser-conformance",
        "generated_for": {"celeris_release": RELEASE, "agent_browser": SUPPORTED_VERSION},
        "results": [{
            "backend_id": "claude-code", "version": SUPPORTED_VERSION,
            "passed": ["open_allowed_origin", "refuse_denied_origin", "resume_after_crash",
                "snapshot_has_refs", "click_by_ref", "screenshot_artifact", "download_to_artifacts"]
        }]
    })
}

fn write_ledger(dir: &Path, value: &Value) -> std::path::PathBuf {
    let file = dir.join("conformance.json");
    std::fs::write(&file, serde_json::to_vec(value).unwrap()).unwrap();
    file
}

fn check(file: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_celerisctl"))
        .args(["browser", "ledger", "check", "--file"])
        .arg(file)
        .args(extra)
        .output()
        .unwrap()
}

fn assert_status(out: Output, expected: &str) -> Value {
    assert_eq!(
        out.status.code(),
        Some(if expected == "ok" { 0 } else { 3 })
    );
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(stdout.lines().count(), 1);
    let value: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["code"], expected);
    assert_eq!(value["ok"], expected == "ok");
    assert!(value["message"].as_str().is_some_and(|s| !s.is_empty()));
    assert!(value["backends"].is_array());
    assert!(value["credential_backends"].is_array());
    let mut keys: Vec<_> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "agent_browser",
            "backends",
            "code",
            "credential_backends",
            "message",
            "ok",
            "release"
        ]
    );
    value
}

#[test]
fn browser_ledger_release_ok_json_and_text_without_host_probe() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_ledger(dir.path(), &ledger());
    // 存在しない executable を明示しても no-host-probe が優先される。
    let absent = dir.path().join("absent-agent-browser");
    let value = assert_status(
        check(
            &file,
            &[
                "--release",
                RELEASE,
                "--agent-browser",
                absent.to_str().unwrap(),
                "--no-host-probe",
                "--json",
            ],
        ),
        "ok",
    );
    assert_eq!(value["release"], RELEASE);
    assert_eq!(value["agent_browser"], SUPPORTED_VERSION);
    assert_eq!(value["backends"], json!(["claude-code"]));
    assert_eq!(value["credential_backends"], json!([]));
    let text = check(&file, &["--no-host-probe"]);
    assert!(text.status.success());
    assert!(String::from_utf8(text.stdout).unwrap().contains("code=ok"));
}

#[test]
fn browser_ledger_release_missing() {
    let dir = tempfile::tempdir().unwrap();
    let value = assert_status(
        check(
            &dir.path().join("missing.json"),
            &["--no-host-probe", "--json"],
        ),
        "missing",
    );
    assert!(value["release"].is_null());
    assert!(value["agent_browser"].is_null());
}

#[test]
fn browser_ledger_release_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("invalid.json");
    std::fs::write(&file, "{broken").unwrap();
    assert_status(check(&file, &["--no-host-probe", "--json"]), "invalid");
}

#[test]
fn browser_ledger_release_stale_release() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_ledger(dir.path(), &ledger());
    let value = assert_status(
        check(
            &file,
            &["--release", "ffffffffffff", "--no-host-probe", "--json"],
        ),
        "stale_release",
    );
    assert_eq!(value["release"], RELEASE);
    let mut old = ledger();
    old.as_object_mut().unwrap().remove("generated_for");
    write_ledger(dir.path(), &old);
    assert_status(
        check(&file, &["--release", RELEASE, "--no-host-probe", "--json"]),
        "stale_release",
    );
    assert_status(check(&file, &["--no-host-probe", "--json"]), "ok");
}

#[test]
fn browser_ledger_release_stale_agent_browser() {
    let dir = tempfile::tempdir().unwrap();
    for metadata in [false, true] {
        let mut old = ledger();
        if metadata {
            old["generated_for"]["agent_browser"] = json!("0.37.0");
        } else {
            old["results"][0]["version"] = json!("0.37.0");
        }
        let file = write_ledger(dir.path(), &old);
        assert_status(
            check(&file, &["--release", RELEASE, "--no-host-probe", "--json"]),
            "stale_agent_browser",
        );
    }
}

#[test]
fn browser_ledger_release_no_conformant_backend() {
    let dir = tempfile::tempdir().unwrap();
    let mut incomplete = ledger();
    incomplete["results"][0]["passed"] = json!(["open_allowed_origin"]);
    let file = write_ledger(dir.path(), &incomplete);
    assert_status(
        check(&file, &["--no-host-probe", "--json"]),
        "no_conformant_backend",
    );
}

#[test]
fn browser_ledger_release_json_keeps_public_backend_without_credential_proof() {
    let dir = tempfile::tempdir().unwrap();
    let mut value = ledger();
    value["results"][0]["passed"] = json!([
        "open_allowed_origin",
        "refuse_denied_origin",
        "resume_after_crash",
        "snapshot_has_refs",
        "click_by_ref",
        "screenshot_artifact",
        "download_to_artifacts",
        "isolation_suite",
        "egress_negative_suite",
        "injection_attack_suite",
        "auth_section_observation_stop"
    ]);
    // Suite names without per-test evidence must not unlock credentials.
    let file = write_ledger(dir.path(), &value);
    let status = assert_status(
        check(&file, &["--release", RELEASE, "--no-host-probe", "--json"]),
        "ok",
    );
    assert_eq!(status["backends"], json!(["claude-code"]));
    assert_eq!(status["credential_backends"], json!([]));
}

#[test]
fn browser_ledger_release_host_probe_uses_explicit_executable() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let file = write_ledger(dir.path(), &ledger());
    let exe = dir.path().join("fake-agent-browser");
    // fake は固定の版だけを返す。
    std::fs::write(&exe, format!("#!/bin/sh\n[ \"$1\" = --version ] || exit 1\nprintf 'agent-browser {SUPPORTED_VERSION}\\n'\n")).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_status(
        check(&file, &["--agent-browser", exe.to_str().unwrap(), "--json"]),
        "ok",
    );
    std::fs::write(&exe, "#!/bin/sh\nprintf 'agent-browser 0.37.0\\n'\n").unwrap();
    assert_status(
        check(&file, &["--agent-browser", exe.to_str().unwrap(), "--json"]),
        "stale_agent_browser",
    );
    std::fs::remove_file(&exe).unwrap();
    assert_status(
        check(&file, &["--agent-browser", exe.to_str().unwrap(), "--json"]),
        "agent_browser_missing",
    );
}
