use std::{fs, path::PathBuf, process::Command};

use celeris_campaign::{
    harness_exec::Sandbox,
    runtime_probe::{mounts_outside_allow, probe},
};
use serde_json::Value;

fn bin() -> String {
    std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("binary path")
}

/// deny list file を `dir/deny.list` に書いて path を返す。
fn deny_file(dir: &std::path::Path, paths: &[PathBuf]) -> PathBuf {
    let f = dir.join("deny.list");
    let text: String = paths.iter().map(|p| format!("{}\n", p.display())).collect();
    fs::write(&f, text).unwrap();
    f
}

#[test]
fn file_empty_file_missing_path_and_parent_listing() {
    let d = tempfile::tempdir().unwrap();
    let file = d.path().join("secret");
    fs::write(&file, "private").unwrap();
    let r = probe("A", std::slice::from_ref(&file), &[]).unwrap();
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
        .args([
            "probe-read",
            "--principal",
            "operator",
            "--deny-readable-file",
        ])
        .arg(deny_file(d.path(), std::slice::from_ref(&secret)))
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
    cmd.args(["probe-read", "--principal", "A", "--deny-readable-file"])
        .arg(deny_file(d.path(), &[d.path().join("missing/secret")]));
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
        .args(["probe-read", "--principal", "A", "--deny-readable-file"])
        .arg(deny_file(&state, &[hidden]));
    let result = cmd.output().unwrap();
    let v: Value =
        serde_json::from_slice(&result.stdout).unwrap_or_else(|e| panic!("{e}: {result:?}"));
    assert_eq!(v["paths"][0]["readable"], false);
    assert_eq!(v["paths"][0]["listable"], false);
    // source の許可表を省略したため、mount 違反で exit 1（path の拒否とは別の判定）。
    assert_eq!(result.status.code(), Some(1));
}

/// preflight と同じ経路（`preflight_isolation::run`）で、A は `Sandbox::runtime_args`、B・operator は実
/// `ops-r2-sandbox.sh`、L1 は人の uid の process 自身から probe する。許可表の外の path は読めず、
/// 許可表の中に置いた path は読めることを確かめる（probe が拒否を偽って出していない）。
#[test]
fn preflight_principals_probe_with_their_real_wrapping() {
    use celeris_campaign::preflight_isolation::{ProbeRequest, ScriptSandbox, Wrap, findings, run};
    let setting = std::env::var("CELERIS_USERNS_TESTS").unwrap_or_default();
    if setting != "1" && setting != "require" {
        eprintln!("skip: preflight principal probes require CELERIS_USERNS_TESTS=1 (or require)");
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
    // ops-r2-sandbox.sh は workspaces・state の下の path を拒むので、cargo の target の一時 dir を使う。
    let d = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let root = d.path();
    let hidden = root.join("private/secret");
    let (ws, cfg) = (root.join("ws"), root.join("cfg"));
    let sb = ScriptSandbox {
        state: Some(root.join("state")),
        release: Some(root.join("release")),
        account: Some(root.join("account")),
        ro: Vec::new(),
    };
    for p in [hidden.parent().unwrap(), &ws, &cfg].into_iter().chain(
        [&sb.state, &sb.release, &sb.account]
            .into_iter()
            .flatten()
            .map(|p| p.as_path()),
    ) {
        fs::create_dir_all(p).unwrap();
    }
    fs::write(&hidden, "grader-private").unwrap();
    fs::write(cfg.join(".credentials.json"), "").unwrap();
    let inside = sb.state.as_ref().unwrap().join("visible");
    fs::write(&inside, "trial").unwrap();
    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/campaign/ops-r2-sandbox.sh");
    let a = Sandbox {
        account_id: "probe".into(),
        auth_dir: cfg.clone(),
        state_root: cfg.clone(),
        toolchain: Default::default(),
    };
    let reqs = [
        ProbeRequest {
            principal: "A",
            program: bin().into(),
            wrap: Wrap::Bwrap {
                args: a.runtime_args(&ws, &cfg, "/usr/bin/true".as_ref(), &[]),
                stage: ws.clone(),
            },
            deny: vec![hidden.clone()],
        },
        ProbeRequest {
            principal: "B",
            program: bin().into(),
            wrap: Wrap::Script {
                script: script.clone(),
                sandbox: sb.clone(),
            },
            deny: vec![hidden.clone(), inside.clone()],
        },
        ProbeRequest {
            principal: "operator",
            program: bin().into(),
            wrap: Wrap::Script {
                script,
                sandbox: sb.clone(),
            },
            deny: vec![hidden.clone()],
        },
    ];
    for r in &reqs {
        let p = run(r).unwrap_or_else(|e| panic!("{}: {e}", r.principal));
        assert!(
            !p.paths[0].readable && !p.paths[0].listable,
            "{}: {p:?}",
            r.principal
        );
        assert!(p.mounts_outside_allow.is_empty(), "{}: {p:?}", r.principal);
        if r.principal == "B" {
            // 許可表の中（--state）に置いた path は読める = probe は実際に開いている。
            assert!(p.paths[1].readable, "{p:?}");
            assert_eq!(
                findings(&p),
                [format!("deny_readable:B:{}", inside.display())]
            );
        } else {
            assert!(p.ok && findings(&p).is_empty(), "{p:?}");
        }
    }
    // probe binary の一時 stage は残さない。
    assert!(!sb.state.as_ref().unwrap().join(".preflight-probe").exists());
    // L1: 人の uid の process は同じ uid の file を開ける（chmod では防げない = 別 uid が要る理由）。
    let l1 = run(&ProbeRequest {
        principal: "L1",
        program: bin().into(),
        wrap: Wrap::Host,
        deny: vec![hidden.clone()],
    })
    .unwrap();
    assert_eq!(
        findings(&l1),
        [format!("deny_readable:L1:{}", hidden.display())]
    );
}

/// l1 は持ち主の uid の分離だけを確かめる。mount の外への到達は報告に残すが判定に入れない
/// （ADR 2026-10-30-ops-r2-isolation-principal 付記 G6、運用文書 §4.7）。他の principal は従来どおり mount で落ちる。
#[test]
fn l1_principal_reports_mounts_but_does_not_fail_on_them() {
    let d = tempfile::tempdir().unwrap();
    let deny = [d.path().join("absent/secret")];
    let other = probe("runtime", &deny, &[]).unwrap();
    assert!(!other.mounts_outside_allow.is_empty());
    assert!(!other.ok);
    let l1 = probe("l1", &deny, &[]).unwrap();
    assert!(!l1.mounts_outside_allow.is_empty());
    assert!(l1.ok);
    let file = d.path().join("secret");
    fs::write(&file, "x").unwrap();
    assert!(!probe("l1", &[file], &[]).unwrap().ok);
}

/// 引数に path を並べない: 15,000 件（Linux の 1 引数 128KiB・全体 2MiB 前後の上限を超える長さ）の
/// deny file を 1 つ渡して probe-read が走り、全件を検査する。旧 `--deny` は受け付けない。
#[test]
fn probe_read_checks_15000_paths_from_one_deny_file() {
    let d = tempfile::tempdir().unwrap();
    let long = "x".repeat(200);
    let paths: Vec<PathBuf> = (0..15_000)
        .map(|i| d.path().join(format!("absent-{i}-{long}")).join("secret"))
        .collect();
    let total: usize = paths.iter().map(|p| p.as_os_str().len() + 1).sum();
    assert!(
        total > 2 * 1024 * 1024,
        "must exceed the argv limit: {total}"
    );
    let f = deny_file(d.path(), &paths);
    let r = Command::new(bin())
        .args(["probe-read", "--principal", "l1", "--deny-readable-file"])
        .arg(&f)
        .output()
        .unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let v: Value = serde_json::from_slice(&r.stdout).unwrap();
    assert_eq!(v["paths"].as_array().unwrap().len(), 15_000);
    assert_eq!(v["ok"], true);
    // 読めた 1 件は 15,000 件の最後でも見逃さない。
    let secret = d.path().join("last-secret");
    fs::write(&secret, "x").unwrap();
    let mut with = paths.clone();
    with.push(secret.clone());
    let f = deny_file(d.path(), &with);
    let r = Command::new(bin())
        .args(["probe-read", "--principal", "l1", "--deny-readable-file"])
        .arg(&f)
        .output()
        .unwrap();
    assert_eq!(r.status.code(), Some(1));
    let v: Value = serde_json::from_slice(&r.stdout).unwrap();
    assert_eq!(v["paths"].as_array().unwrap().len(), 15_001);
    assert_eq!(v["paths"][15_000]["readable"], true);
    // 旧引数は削除済み。
    assert!(
        !Command::new(bin())
            .args(["probe-read", "--principal", "A", "--deny"])
            .arg(&secret)
            .status()
            .unwrap()
            .success()
    );
}
