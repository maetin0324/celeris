//! ADR-0126 A2: worker run の中（guard が読み取り専用にした本番 DB の dir = 印 `CELERIS_WORKER_DB_GUARD`）で、
//! 本番側の DB を使う実 `celeris` は userns の probe に進まず起動を止める（非 0 終了・拒否の文言）。
//!
//! 1. 本物の印がある（worker sandbox の中）→ db を印の path 配下にした config でそのまま確かめる。
//! 2. 印が無い → 1 行理由を出して skip。ただし `CELERIS_USERNS_TESTS=1` なら `unshare -U -r -m` で
//!    読み取り専用の印を作って同じことを確かめる（作れなければ fail）。
//!
//! ADR-0126 付記2 の 1: `[db] worker_read_only = false`（opt-out）の config でも同じく止まる
//! （`worker_db_guard_refuse_with_opt_out`。skip の規則は同じ）。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use task_worker::db_guard::{
    PRODUCTION_IN_WORKER_RUN, WORKER_DB_GUARD_ENV, WorkerRunMarker, verify_worker_run_marker,
};

/// 試験用 config（偽のアダプタだけ。外部に出ない）。`db` は絶対 path で渡す。
/// `worker_read_only = false` は `[db]` テーブルの opt-out（ADR-0095 D5/D-a）。
fn write_config_with(dir: &Path, db: &Path, worker_read_only: bool) -> PathBuf {
    std::fs::create_dir_all(dir.join("ws")).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(
        &path,
        format!(
            r#"workspace_root = "ws"
tick_ms = 100

[adapters.fake]
command = ["true"]

[[providers]]
id = "p1"
adapter = "fake"

[db]
path = "{}"
worker_read_only = {worker_read_only}
"#,
            db.display()
        ),
    )
    .unwrap();
    path
}

#[track_caller]
fn assert_refused(out: &Output, what: &str) {
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !out.status.success(),
        "{what}: celeris started (status {:?})\nstderr: {stderr}\nstdout: {stdout}",
        out.status
    );
    assert!(
        stderr.contains(PRODUCTION_IN_WORKER_RUN) || stdout.contains(PRODUCTION_IN_WORKER_RUN),
        "{what}: no refusal message (status {:?})\nstderr: {stderr}\nstdout: {stdout}",
        out.status
    );
    assert!(
        !stderr.contains("probing user namespaces") && !stderr.contains("cannot make"),
        "{what}: reached the userns probe\nstderr: {stderr}"
    );
}

fn celeris_args(config: &Path) -> Vec<String> {
    [
        "--config",
        config.to_str().unwrap(),
        "--max-ticks",
        "1",
        "--log-format",
        "text",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

#[test]
fn worker_db_guard_refuse_real_binary_inside_worker_run() {
    run_refuse_case(true);
}

/// ADR-0126 付記2 の 1: `worker_read_only = false` の config・印 `ReadOnly`・印の配下の DB でも、
/// celeris は非 0 で終わり DB を作らない（opt-out は worker run の外でだけ効く）。
#[test]
fn worker_db_guard_refuse_with_opt_out() {
    run_refuse_case(false);
}

fn run_refuse_case(worker_read_only: bool) {
    match verify_worker_run_marker(std::env::var_os(WORKER_DB_GUARD_ENV)) {
        WorkerRunMarker::ReadOnly(guarded) => inside_real_sandbox(&guarded, worker_read_only),
        other => {
            if std::env::var("CELERIS_USERNS_TESTS").as_deref() == Ok("1") {
                inside_unshare(worker_read_only);
            } else {
                eprintln!(
                    "SKIPPED (worker_db_guard_refuse, worker_read_only = {worker_read_only}): no read-only \
                     {WORKER_DB_GUARD_ENV} marker ({other:?}); run inside a worker sandbox or set \
                     CELERIS_USERNS_TESTS=1 (ADR-0126)"
                );
            }
        }
    }
}

/// worker sandbox の中: 印（本番 DB の dir、読み取り専用）の配下を db にした daemon を起こす。
fn inside_real_sandbox(guarded: &Path, worker_read_only: bool) {
    let tmp = tempfile::tempdir().unwrap();
    // まだ無い DB（読み取り専用の dir には作れない）。
    let db = guarded.join(format!(
        "worker-db-guard-refuse-{}.sqlite3",
        std::process::id()
    ));
    let config = write_config_with(tmp.path(), &db, worker_read_only);
    let out = Command::new(env!("CARGO_BIN_EXE_celeris"))
        .args(celeris_args(&config))
        .output()
        .unwrap();
    assert_refused(&out, "new db under the marker");
    assert!(!db.exists());
    // 既にある本番 DB（DB を開く前に止まる。印の dir は読み取り専用なので書かれもしない）。
    let prod = guarded.join("celeris.sqlite3");
    if prod.exists() {
        let config = write_config_with(tmp.path(), &prod, worker_read_only);
        let out = Command::new(env!("CARGO_BIN_EXE_celeris"))
            .args(celeris_args(&config))
            .output()
            .unwrap();
        assert_refused(&out, "production db");
    }
}

/// `CELERIS_USERNS_TESTS=1`: userns + mount ns で一時 dir を読み取り専用にした印を作り、その配下の DB で起こす。
/// userns の中では元の mount の nosuid・nodev・noexec を外せない（locked）ので、付けて remount し直す。
fn inside_unshare(worker_read_only: bool) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let guarded = root.join("guarded");
    std::fs::create_dir_all(&guarded).unwrap();
    let existing = guarded.join("celeris.sqlite3");
    std::fs::write(&existing, b"").unwrap();
    let missing = guarded.join("new.sqlite3");
    let celeris = env!("CARGO_BIN_EXE_celeris");
    for (what, db, dir) in [
        ("existing db under the marker", &existing, root.join("a")),
        ("new db under the marker", &missing, root.join("b")),
    ] {
        let config = write_config_with(&dir, db, worker_read_only);
        let args = celeris_args(&config).join(" ");
        let script = format!(
            "set -e; mount --bind '{g}' '{g}'; mount -o remount,ro,bind '{g}' '{g}' 2>/dev/null \
             || mount -o remount,ro,bind,nosuid,nodev '{g}' '{g}' 2>/dev/null \
             || mount -o remount,ro,bind,nosuid,nodev,noexec '{g}' '{g}'; \
             if touch '{g}/.w' 2>/dev/null; then echo 'marker is writable' >&2; exit 97; fi; \
             export {WORKER_DB_GUARD_ENV}='{g}'; exec '{celeris}' {args}",
            g = guarded.display()
        );
        let out = Command::new("unshare")
            .args(["-U", "-r", "-m", "sh", "-c", &script])
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stderr.contains("unshare:")
                && !stderr.contains("mount:")
                && out.status.code() != Some(97),
            "CELERIS_USERNS_TESTS=1 but a read-only marker cannot be made: {stderr}"
        );
        assert_refused(&out, what);
    }
    assert!(!missing.exists());
    assert_eq!(std::fs::read(&existing).unwrap(), b"");
}
