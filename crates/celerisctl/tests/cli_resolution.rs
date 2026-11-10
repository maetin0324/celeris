//! 実 CLI の解決（`common::resolve_cli`）の回帰試験。環境変数は書き換えず、純関数に値を渡して決定的に確かめる。
mod common;

use common::{celerisctl_path, resolve_cli};
use std::ffi::OsString;
use std::process::Command;

#[test]
fn runtime_mapping_wins_over_stale_embedded_path() {
    let current = celerisctl_path();
    let got = resolve_cli(
        Some(current.clone().into_os_string()),
        "/nonexistent/stale-target/debug/celerisctl",
    )
    .unwrap();
    assert_eq!(got, current);
}

#[test]
fn embedded_path_is_the_fallback_without_runtime_mapping() {
    let got = resolve_cli(None, env!("CARGO_BIN_EXE_celerisctl")).unwrap();
    assert!(got.is_file());
}

#[test]
fn missing_runtime_mapping_target_fails_clearly_without_embedded_fallback() {
    let err = resolve_cli(
        Some(OsString::from("/nonexistent/stale-target/debug/celerisctl")),
        env!("CARGO_BIN_EXE_celerisctl"),
    )
    .unwrap_err();
    assert!(err.contains("runtime mapping"), "{err}");
    assert!(err.contains("does not point to an existing file"), "{err}");
}

#[test]
fn stale_embedded_path_without_mapping_fails_clearly() {
    let err = resolve_cli(None, "/nonexistent/stale-target/debug/celerisctl").unwrap_err();
    assert!(err.contains("embedded at compile time"), "{err}");
}

#[test]
fn invalid_mapping_values_fail_clearly() {
    let embedded = env!("CARGO_BIN_EXE_celerisctl");
    let empty = resolve_cli(Some(OsString::new()), embedded).unwrap_err();
    assert!(empty.contains("empty"), "{empty}");
    let relative = resolve_cli(Some(OsString::from("celerisctl")), embedded).unwrap_err();
    assert!(relative.contains("not an absolute path"), "{relative}");
    let dir = resolve_cli(Some(OsString::from("/")), embedded).unwrap_err();
    assert!(dir.contains("existing file"), "{dir}");
}

#[test]
fn resolved_cli_is_the_real_binary() {
    let out = Command::new(celerisctl_path())
        .arg("--help")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("replay"));
}
