//! ADR 2026-11-12-knowledge-usage-per-run D1: `celerisctl knowledge search|get|record` は、env
//! `CELERIS_KNOWLEDGE_USAGE_FILE` があればその台帳に 1 操作 1 行を書く。env が無ければ書かない。
//!
//! 守る不変条件: KB の読み取りの回数は KB の操作の入口で数え、harness の log を読まない。既存の
//! knowledge の試験は関数を直に呼ぶので、binary の入口（`run` の分岐）での計数を確かめられない。

mod common;

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use task_core::knowledge::usage::{self, ENV_USAGE_FILE, KnowledgeUse};

fn ctl(root: &Path, ledger: Option<&Path>, args: &[&str], stdin: &str) -> Output {
    let mut cmd = Command::new(common::celerisctl_path());
    cmd.arg("knowledge")
        .args(args)
        .args(["--root"])
        .arg(root)
        .args(["--config", "/nonexistent/celeris.toml"])
        .env_remove("CELERIS_COS_RUN_CREDENTIAL")
        .env_remove(ENV_USAGE_FILE)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(ledger) = ledger {
        cmd.env(ENV_USAGE_FILE, ledger);
    }
    let mut child = cmd.spawn().expect("spawn celerisctl");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("wait celerisctl")
}

#[test]
fn knowledge_ops_are_counted_at_the_cli_entry_only_when_the_ledger_env_is_set() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("knowledge");
    let ledger = dir.path().join(usage::USAGE_FILE_NAME);

    // init は数えない（KB の読み書きの操作ではない）。
    let out = ctl(&root, Some(&ledger), &["init"], "");
    assert!(out.status.success(), "{out:?}");
    assert!(!ledger.exists());

    let out = ctl(&root, Some(&ledger), &["search", "profile"], "");
    assert!(out.status.success(), "{out:?}");
    let out = ctl(&root, Some(&ledger), &["get", "user/profile.md"], "");
    assert!(out.status.success(), "{out:?}");
    // 無いページの get は失敗として数える（読もうとした回数に入る）。
    let out = ctl(&root, Some(&ledger), &["get", "user/nope.md"], "");
    assert!(!out.status.success());
    let out = ctl(
        &root,
        Some(&ledger),
        &[
            "record",
            "--title",
            "pegasus の作業 dir",
            "--scope",
            "environment",
            "--path",
            "environment/clusters/pegasus-work-dir.md",
            "--source",
            "task:01ABCDEFGHJKMNPQRSTVWXYZ00",
        ],
        "作業は /work の下で行う。\n",
    );
    assert!(out.status.success(), "{out:?}");

    assert_eq!(
        usage::read(&ledger),
        KnowledgeUse {
            search: 1,
            get: 2,
            record: 1,
            failed: 1
        }
    );

    // env が無い呼び出し（人の手元）は台帳に書かない。
    let before = std::fs::read_to_string(&ledger).expect("ledger");
    let out = ctl(&root, None, &["search", "profile"], "");
    assert!(out.status.success(), "{out:?}");
    assert_eq!(std::fs::read_to_string(&ledger).expect("ledger"), before);
}
