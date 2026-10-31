//! ops-r2-trial-a-command.sh（ADR 2026-10-31-ops-r2-preflight-fixes 付記 D-trial-a）。起動 command を出すだけで、
//! claude も sandbox も起動しない。承認票の条件（版・model・3600 秒）と A の sandbox の形が command に入ることを確かめる。
use std::{os::unix::fs::PermissionsExt, path::PathBuf, process::Command};

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/campaign/ops-r2-trial-a-command.sh")
}

#[test]
fn prints_sandboxed_claude_command_with_approved_limits() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let state = root.join("a-state");
    let cdir = root.join("bin");
    let account = root.join("acct");
    for d in [
        state.join("seed"),
        state.join("bin"),
        cdir.clone(),
        account.clone(),
    ] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(state.join("seed/benchfs.tar"), b"x").unwrap();
    std::fs::write(state.join("bin/ops-r2-seed.sh"), b"").unwrap();
    std::fs::write(state.join("prompt.md"), b"p").unwrap();
    let claude = cdir.join("claude-2.1.303");
    std::fs::write(&claude, b"#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cfg = root.join("ssh-config");
    std::fs::write(&cfg, b"").unwrap();
    let sock = root.join("master.sock");
    let sha = "a".repeat(64);
    let args = |sha: &str| {
        let mut c = Command::new("sh");
        c.arg(script());
        for (k, v) in [
            ("--state", &state),
            ("--claude-dir", &cdir),
            ("--account", &account),
            ("--ssh-config", &cfg),
            ("--socket", &sock),
        ] {
            c.arg(k).arg(v);
        }
        c.args(["--seed-sha256", sha]);
        c.output().unwrap()
    };
    let out = args(&sha);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let line = String::from_utf8(out.stdout).unwrap();
    for want in [
        "ops-r2-sandbox.sh --state ",
        &format!("--release {} ", cdir.display()),
        &format!(
            "--ssh-config {} --socket {} --",
            cfg.display(),
            sock.display()
        ),
        &format!(
            "unpack --seed {}/seed/benchfs.tar --sha256 {sha}",
            state.display()
        ),
        &format!(
            "CLAUDE_CONFIG_DIR={} exec timeout 3600 {}",
            account.display(),
            claude.display()
        ),
        "--model claude-sonnet-5-5",
    ] {
        assert!(line.contains(want), "missing {want:?} in {line}");
    }
    assert!(!line.contains(" -F "), "trial must not be told -F: {line}");
    assert_eq!(args("XYZ").status.code(), Some(2));
    std::fs::remove_file(state.join("prompt.md")).unwrap();
    assert_eq!(args(&sha).status.code(), Some(1));
}
