//! ADR-0095 D6: `celerisctl` は migration をしない（本番 2026-09-30 22:37:52Z、ブランチの celerisctl が本番 DB を
//! schema 35 にした事故の再発防止）。実バイナリで、古い DB・新しい DB・無い DB・同じ版の DB を開く。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use task_core::{SCHEMA_VERSION, SqliteStore, TaskStore};

fn ctl(db: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_celerisctl"))
        .arg("--db")
        .arg(db)
        .args(args)
        .env_remove("CELERIS_DB")
        .env_remove("CELERIS_CONFIG")
        // ADR-0098 D6: 試験を Celeris の worker の run の中で回しても `add` が後続の宣言に化けないように。
        .env_remove("CELERIS_FOLLOWUPS_FILE")
        .env_remove("CELERIS_RUN_DB")
        .output()
        .unwrap_or_else(|e| panic!("spawn celerisctl: {e}"))
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn schema_version(db: &Path) -> i64 {
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap_or_else(|e| panic!("open: {e}"));
    conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |r| r.get(0),
    )
    .unwrap_or_else(|e| panic!("version: {e}"))
}

/// daemon と同じ `SqliteStore::open`（migration あり）で作り、`schema_migrations` を `version` にする。
fn db_at(dir: &Path, version: u32) -> PathBuf {
    let db = dir.join("celeris.sqlite3");
    drop(SqliteStore::open(&db).unwrap_or_else(|e| panic!("open: {e}")));
    let conn = rusqlite::Connection::open(&db).unwrap_or_else(|e| panic!("open: {e}"));
    conn.execute(
        "DELETE FROM schema_migrations WHERE version > ?1",
        [version],
    )
    .unwrap_or_else(|e| panic!("delete: {e}"));
    // `version` が SCHEMA_VERSION を超えるとき・task-core の予約版数（ADR-0133 D3.2）のときも、
    // DB の版数がちょうど `version` になるように記録する。
    conn.execute(
            "INSERT OR IGNORE INTO schema_migrations (version, applied_at) VALUES (?1, '2026-09-30T22:37:52Z')",
            [version],
        )
        .unwrap_or_else(|e| panic!("insert: {e}"));
    db
}

fn task_count(db: &Path) -> usize {
    let (store, _) = SqliteStore::open_client(db).unwrap_or_else(|e| panic!("open: {e}"));
    store
        .list(None)
        .unwrap_or_else(|e| panic!("list: {e}"))
        .len()
}

const ADD: &[&str] = &[
    "add",
    "--title",
    "t",
    "--objective",
    "o",
    "--check-cmd",
    "true",
];

#[test]
fn celerisctl_refuses_an_older_db_and_leaves_its_schema_alone() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let db = db_at(dir.path(), SCHEMA_VERSION - 1);
    for args in [&["ls"][..], ADD] {
        let out = ctl(&db, args);
        let t = text(&out);
        assert!(!out.status.success(), "{args:?} must fail: {t}");
        assert!(t.contains("never migrates"), "{t}");
        assert!(
            t.contains(&format!("db schema version {}", SCHEMA_VERSION - 1)),
            "{t}"
        );
        assert_eq!(
            schema_version(&db),
            i64::from(SCHEMA_VERSION - 1),
            "celerisctl {args:?} must not migrate"
        );
    }
}

#[test]
fn celerisctl_reads_a_newer_db_but_refuses_to_write_it() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let db = db_at(dir.path(), SCHEMA_VERSION + 1);
    let list = ctl(&db, &["ls"]);
    let t = text(&list);
    assert!(list.status.success(), "reads are allowed: {t}");
    assert!(t.contains("opened read-only"), "{t}");

    let add = ctl(&db, ADD);
    let t = text(&add);
    assert!(!add.status.success(), "writes must fail: {t}");
    assert!(t.contains("attempt to write a readonly database"), "{t}");
    assert!(t.contains("celerisctl cannot write to it"), "{t}");
    assert_eq!(task_count(&db), 0, "nothing was written");
    assert_eq!(schema_version(&db), i64::from(SCHEMA_VERSION + 1));
}

#[test]
fn celerisctl_does_not_create_a_missing_db() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let db = dir.path().join("absent.sqlite3");
    let out = ctl(&db, ADD);
    let t = text(&out);
    assert!(!out.status.success(), "{t}");
    assert!(t.contains("does not exist"), "{t}");
    assert!(!db.exists(), "celerisctl must not create the db");
}

#[test]
fn celerisctl_reads_and_writes_a_db_at_its_own_version() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let db = db_at(dir.path(), SCHEMA_VERSION);
    let add = ctl(&db, ADD);
    assert!(add.status.success(), "{}", text(&add));
    assert_eq!(task_count(&db), 1);
    let list = ctl(&db, &["ls"]);
    assert!(list.status.success(), "{}", text(&list));
    assert!(!text(&list).contains("warning"), "{}", text(&list));
}
