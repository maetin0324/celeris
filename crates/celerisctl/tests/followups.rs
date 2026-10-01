//! ADR-0098 D6（Phase R7-10）: worker の run の中で daemon の DB（`CELERIS_RUN_DB`）に向けた `celerisctl add` は
//! DB を開かず、その run の `followups.json`（`CELERIS_FOLLOWUPS_FILE`）に追記する。別の DB（試験の一時 DB）に
//! 向けた `add` と、run の外（env 無し）の `add` は従来どおり DB に書く。

use std::path::Path;
use std::process::{Command, Output};

use task_core::{SqliteStore, TaskStore};

fn ctl(args: &[&str], env: &[(&str, &Path)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_celerisctl"));
    cmd.args(args)
        .env_remove("CELERIS_DB")
        .env_remove("CELERIS_CONFIG")
        .env_remove("CELERIS_FOLLOWUPS_FILE")
        .env_remove("CELERIS_RUN_DB");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output()
        .unwrap_or_else(|e| panic!("spawn celerisctl: {e}"))
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

const ADD: [&str; 7] = [
    "add",
    "--title",
    "next step",
    "--objective",
    "continue the work",
    "--check-cmd",
    "true",
];

fn queued_titles(file: &Path) -> Vec<String> {
    let parsed: task_ops::followup::FollowupsFile =
        serde_json::from_str(&std::fs::read_to_string(file).unwrap_or_else(|e| panic!("{e}")))
            .unwrap_or_else(|e| panic!("{e}"));
    parsed
        .tasks
        .into_iter()
        .map(|v| v["title"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn add_inside_a_run_queues_a_followup_instead_of_opening_the_db() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    // daemon の DB（run の中では読み取り専用）。中身は要らない: queue モードは DB を開かない。
    let run_db = dir.path().join("celeris.sqlite3");
    let file = dir.path().join("artifacts").join("followups.json");
    let env = [
        ("CELERIS_RUN_DB", run_db.as_path()),
        ("CELERIS_FOLLOWUPS_FILE", file.as_path()),
    ];

    // `--db` 無し → `CELERIS_RUN_DB` に向かう → 宣言になる。
    let out = ctl(&ADD, &env);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("queued follow-up #1"), "{}", text(&out));
    // 事故の形（`--db <本番>` を明示）も同じ。
    let mut explicit = vec!["--db", run_db.to_str().unwrap_or_default()];
    explicit.extend(ADD);
    let out = ctl(&explicit, &env);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("queued follow-up #2"), "{}", text(&out));
    assert_eq!(queued_titles(&file), vec!["next step", "next step"]);
    assert!(!run_db.exists(), "DB は開かない（作らない）");

    // daemon が使わない欄は run の中では断る（ファイルは変えない）。
    let mut with_parent = ADD.to_vec();
    with_parent.extend(["--parent", "01M3SPF94RDWTPWHNDEQD68VB9"]);
    let out = ctl(&with_parent, &env);
    assert!(!out.status.success());
    assert!(text(&out).contains("--parent cannot be used inside a worker run"));
    assert_eq!(queued_titles(&file).len(), 2);
}

#[test]
fn add_to_another_db_or_outside_a_run_writes_the_db_as_before() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let test_db = dir.path().join("test.sqlite3");
    drop(SqliteStore::open(&test_db).unwrap_or_else(|e| panic!("{e}")));
    let run_db = dir.path().join("celeris.sqlite3");
    let file = dir.path().join("followups.json");
    let env = [
        ("CELERIS_RUN_DB", run_db.as_path()),
        ("CELERIS_FOLLOWUPS_FILE", file.as_path()),
    ];

    // run の中でも、試験の一時 DB に向けた `add` は DB に書く（宣言に化けない）。
    let mut args = vec!["--db", test_db.to_str().unwrap_or_default()];
    args.extend(ADD);
    let out = ctl(&args, &env);
    assert!(out.status.success(), "{}", text(&out));
    assert!(!file.exists());
    // run の外（env 無し）も従来どおり。
    let out = ctl(&args, &[]);
    assert!(out.status.success(), "{}", text(&out));
    let store = SqliteStore::open(&test_db).unwrap_or_else(|e| panic!("{e}"));
    let tasks = store.list(None).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(tasks.len(), 2);
    assert!(tasks.iter().all(|t| t.project_id.is_none()));
}
