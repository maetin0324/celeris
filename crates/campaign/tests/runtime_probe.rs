use std::{fs, path::PathBuf, process::Command};

use celeris_campaign::{
    harness_exec::Sandbox,
    runtime_probe::{mounts_outside_allow, probe},
};
use serde_json::Value;

fn bin() -> String {
    std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("binary path")
}

#[test]
fn file_empty_file_missing_path_and_parent_listing() {
    let d = tempfile::tempdir().unwrap();
    let file = d.path().join("secret");
    fs::write(&file, "private").unwrap();
    let r = probe("A", &[file.clone()], &[]).unwrap();
    assert!(r.paths[0].readable && r.paths[0].listable);
    assert!(r.paths[0].errno.is_none());
    assert!(!r.ok);
    fs::write(&file, "").unwrap();
    assert!(probe("A", &[file], &[]).unwrap().paths[0].readable);
    let missing = d.path().join("missing");
    let r = probe("A", &[missing], &[]).unwrap();
    assert!(!r.paths[0].readable);
    assert!(r.paths[0].listable);
    assert!(r.paths[0].errno.is_some());
    let r = probe("A", &[d.path().join("absent/secret")], &[]).unwrap();
    assert!(!r.paths[0].readable && !r.paths[0].listable);
}

#[test]
fn mount_sources_are_decoded_deduplicated_and_matched_on_path_boundaries() {
    let info = "1 0 0:1 / / rw - tmpfs tmpfs rw\n2 1 0:2 / /m rw - ext4 /allowed\\040dir/dev rw\n3 1 0:3 / /n rw - ext4 /allowed-sibling rw\n";
    assert_eq!(
        mounts_outside_allow(
            info,
            &[PathBuf::from("tmpfs"), PathBuf::from("/allowed dir")]
        )
        .unwrap(),
        vec!["/allowed-sibling"]
    );
    assert!(mounts_outside_allow("broken", &[]).is_err());
    assert!(mounts_outside_allow("", &[]).is_err());
}

#[test]
fn cli_json_output_and_exit_codes() {
    let d = tempfile::tempdir().unwrap();
    let secret = d.path().join("secret");
    fs::write(&secret, "x").unwrap();
    let out = d.path().join("probe.json");
    let result = Command::new(bin())
        .args(["probe-read", "--principal", "operator", "--deny"])
        .arg(&secret)
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    let v: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(v["schema"], "celeris.runtime-probe/1");
    assert_eq!(v["principal"], "operator");
    assert!(v["uid"].is_u64());
    assert_eq!(v["paths"][0]["path"], secret.to_str().unwrap());
    assert_eq!(v["paths"][0]["readable"], true);
    assert_eq!(v["paths"][0]["listable"], true);
    assert!(v["paths"][0]["errno"].is_null());
    assert_eq!(v["ok"], false);
    assert!(v["mounts_outside_allow"].is_array());
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(out).unwrap()).unwrap(),
        v
    );

    // 許可表に現在の全 source を明記した上で、存在しない親の path を検査する。
    let info = fs::read_to_string("/proc/self/mountinfo").unwrap();
    let sources = mounts_outside_allow(&info, &[]).unwrap();
    let mut cmd = Command::new(bin());
    cmd.args(["probe-read", "--principal", "A", "--deny"])
        .arg(d.path().join("missing/secret"));
    for source in sources {
        cmd.arg("--allow").arg(source);
    }
    let result = cmd.output().unwrap();
    assert!(result.status.success(), "{:?}", result);
    let v: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(v["ok"], true);
    assert!(
        !Command::new(bin())
            .args(["probe-read", "--principal", "A"])
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn a_and_probe_use_identical_sandbox_arguments() {
    let sb = Sandbox {
        account_id: "test".into(),
        auth_dir: "/account".into(),
        state_root: "/state".into(),
        toolchain: Default::default(),
    };
    let (ws, state, real) = (
        "/workspace".as_ref(),
        "/state/trial".as_ref(),
        "/usr/bin/true".as_ref(),
    );
    assert_eq!(
        sb.args(ws, state, real, &[]),
        sb.runtime_args(ws, state, real, &[])
    );
}

#[test]
fn real_bwrap_probe_cannot_read_outside_a_allowlist() {
    let setting = std::env::var("CELERIS_USERNS_TESTS").unwrap_or_default();
    if setting != "1" && setting != "require" {
        eprintln!("skip: real bwrap probe requires CELERIS_USERNS_TESTS=1 (or require)");
        return;
    }
    let usable = Command::new("bwrap")
        .args([
            "--ro-bind",
            "/",
            "/",
            "--proc",
            "/proc",
            "--unshare-pid",
            "--",
            "true",
        ])
        .output()
        .is_ok_and(|o| o.status.success());
    if !usable {
        assert_ne!(setting, "require", "bwrap user namespace is required");
        eprintln!("skip: bwrap cannot create a namespace here");
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let ws = d.path().join("workspace");
    let state = d.path().join("state");
    let account = d.path().join("account");
    let hidden = d.path().join("private/secret");
    for p in [&ws, &state, &account, hidden.parent().unwrap()] {
        fs::create_dir_all(p).unwrap();
    }
    fs::write(&hidden, "grader-private").unwrap();
    fs::write(account.join(".credentials.json"), "{}").unwrap();
    fs::write(state.join(".credentials.json"), "").unwrap();
    let staged = ws.join("probe");
    fs::copy(bin(), &staged).unwrap();
    let sb = Sandbox {
        account_id: "test".into(),
        auth_dir: account,
        state_root: state.clone(),
        toolchain: Default::default(),
    };
    let mut cmd = Command::new("bwrap");
    cmd.args(sb.runtime_args(&ws, &state, "/usr/bin/true".as_ref(), &[]))
        .arg("--")
        .arg(staged)
        .args(["probe-read", "--principal", "A", "--deny"])
        .arg(hidden);
    let result = cmd.output().unwrap();
    let v: Value =
        serde_json::from_slice(&result.stdout).unwrap_or_else(|e| panic!("{e}: {result:?}"));
    assert_eq!(v["paths"][0]["readable"], false);
    assert_eq!(v["paths"][0]["listable"], false);
    // source の許可表を省略したため、mount 違反で exit 1（path の拒否とは別の判定）。
    assert_eq!(result.status.code(), Some(1));
}
