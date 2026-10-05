use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};
use serde_json::Value;

use super::SqliteStore;

/// Builds a pre-cos_chat database with old CoS rows in two projects and the
/// global scope, one non-CoS row and one live CoS session, then opens the
/// store so the migration and the legacy projection run.
fn migrated_fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("legacy.sqlite3");
    let mut conn = Connection::open(&path).expect("open fixture");
    conn.execute_batch(
        "CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY,applied_at TEXT NOT NULL)",
    )
    .expect("ledger");
    for version in (1..=37).chain(41..=47) {
        SqliteStore::apply_migration_version(&mut conn, version).expect("old schema");
    }
    let old_rows = [
        (
            "b",
            Some("p1"),
            "node",
            "回答特有語",
            Some("run-b"),
            Some("task-b"),
            "2026-10-05T00:00:02Z",
        ),
        (
            "c",
            Some("p2"),
            "user",
            "別の案件",
            None,
            None,
            "2026-10-05T00:00:01Z",
        ),
        (
            "a",
            Some("p1"),
            "user",
            "質問特有語",
            None,
            Some("task-a"),
            "2026-10-05T00:00:01Z",
        ),
        (
            "global",
            None,
            "node",
            "全体特有語",
            Some("run-global"),
            None,
            "2026-10-05T00:00:01Z",
        ),
        (
            "same-time",
            Some("p1"),
            "node",
            "同時刻",
            None,
            None,
            "2026-10-05T00:00:01Z",
        ),
    ];
    for (id, project, role, body, run, task, at) in old_rows {
        conn.execute(
            "INSERT INTO messages(id,node_id,project_id,role,text,run_id,created_at,task_id,metadata_json) \
             VALUES(?1,'cos',?2,?3,?4,?5,?6,?7,'{\"action\":\"kept\"}')",
            params![id, project, role, body, run, at, task],
        )
        .expect("old message");
    }
    conn.execute(
        "INSERT INTO messages(id,node_id,project_id,role,text,created_at) \
         VALUES('other','engineer','p1','node','非 CoS 本文','2026-10-05T00:00:00Z')",
        [],
    )
    .expect("non CoS");
    conn.execute(
        "INSERT INTO node_sessions(id,node_id,kind,adapter,session_id,created_at,last_used_at) \
         VALUES('cos-session','cos','conversation','claude-code','session','2026-10-05T00:00:00Z','2026-10-05T00:00:00Z')",
        [],
    )
    .expect("session");
    conn.execute(
        "INSERT INTO node_sessions(id,node_id,kind,adapter,session_id,created_at,last_used_at) \
         VALUES('eng-session','engineer','conversation','claude-code','session2','2026-10-05T00:00:00Z','2026-10-05T00:00:00Z')",
        [],
    )
    .expect("non CoS session");
    drop(conn);
    drop(SqliteStore::open(&path).expect("migrate and import"));
    (dir, path)
}

fn count(path: &Path, sql: &str) -> i64 {
    let conn = Connection::open(path).expect("inspect");
    conn.query_row(sql, [], |r| r.get(0)).expect("count")
}

#[test]
fn chat_legacy_one_thread_per_project_and_global() {
    let (_dir, path) = migrated_fixture();
    let conn = Connection::open(&path).expect("inspect");
    let scoped: Vec<(String, Option<String>)> = conn
        .prepare("SELECT id,project_id FROM chat_threads WHERE kind='legacy' ORDER BY id")
        .expect("scopes")
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("scope rows")
        .map(|r| r.expect("scope"))
        .collect();
    assert_eq!(
        scoped,
        vec![
            ("legacy:global".into(), None),
            ("legacy:project:p1".into(), Some("p1".into())),
            ("legacy:project:p2".into(), Some("p2".into())),
        ]
    );
    assert_eq!(
        count(
            &path,
            "SELECT count(*) FROM chat_messages WHERE thread_id='legacy:global'"
        ),
        1
    );
    assert_eq!(
        count(
            &path,
            "SELECT count(*) FROM chat_messages WHERE thread_id='legacy:project:p2'"
        ),
        1
    );
}

#[test]
fn chat_legacy_seq_follows_created_at_then_id_and_maps_roles() {
    let (_dir, path) = migrated_fixture();
    let conn = Connection::open(&path).expect("inspect");
    let mut stmt = conn
        .prepare(
            "SELECT legacy_message_id,seq,role,run_id,metadata_json FROM chat_messages \
             WHERE thread_id='legacy:project:p1' ORDER BY seq",
        )
        .expect("messages");
    let rows: Vec<(String, i64, String, Option<String>, String)> = stmt
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .expect("query")
        .map(|r| r.expect("row"))
        .collect();
    assert_eq!(
        rows.iter()
            .map(|r| (r.0.as_str(), r.1, r.2.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("a", 1, "user"),
            ("same-time", 2, "assistant"),
            ("b", 3, "assistant")
        ]
    );
    assert_eq!(rows[2].3.as_deref(), Some("run-b"));
    let meta: Value = serde_json::from_str(&rows[2].4).expect("metadata");
    assert_eq!(meta["legacy_source"]["task_id"], "task-b");
    assert_eq!(meta["legacy_source"]["metadata"]["action"], "kept");
}

#[test]
fn chat_legacy_reopen_is_idempotent_and_catches_up_late_rows() {
    let (_dir, path) = migrated_fixture();
    assert_eq!(count(&path, "SELECT count(*) FROM chat_messages"), 5);
    drop(SqliteStore::open(&path).expect("reopen"));
    drop(SqliteStore::open(&path).expect("reopen twice"));
    assert_eq!(count(&path, "SELECT count(*) FROM chat_messages"), 5);
    assert_eq!(
        count(
            &path,
            "SELECT count(*) FROM chat_threads WHERE kind='legacy'"
        ),
        3
    );
    assert_eq!(
        count(
            &path,
            "SELECT count(*) FROM chat_search WHERE chat_search MATCH '回答特有語'"
        ),
        1
    );

    let conn = Connection::open(&path).expect("late write");
    conn.execute(
        "INSERT INTO messages(id,node_id,project_id,role,text,created_at) \
         VALUES('new','cos','p1','user','後続本文','2026-10-05T00:00:03Z')",
        [],
    )
    .expect("late old write");
    drop(conn);
    drop(SqliteStore::open(&path).expect("catch up"));
    assert_eq!(
        count(
            &path,
            "SELECT seq FROM chat_messages WHERE legacy_message_id='new'"
        ),
        4
    );
    assert_eq!(count(&path, "SELECT count(*) FROM chat_messages"), 6);
}

#[test]
fn chat_legacy_keeps_old_and_non_cos_messages() {
    let (_dir, path) = migrated_fixture();
    assert_eq!(count(&path, "SELECT count(*) FROM messages"), 6);
    assert_eq!(
        count(
            &path,
            "SELECT count(*) FROM messages WHERE id='other' AND text='非 CoS 本文'"
        ),
        1
    );
    assert_eq!(
        count(
            &path,
            "SELECT count(*) FROM chat_messages WHERE legacy_message_id='other'"
        ),
        0
    );
}

#[test]
fn chat_legacy_fts_finds_imported_bodies() {
    let (_dir, path) = migrated_fixture();
    for term in ["回答特有語", "質問特有語", "全体特有語"] {
        let sql = format!("SELECT count(*) FROM chat_search WHERE chat_search MATCH '{term}'");
        assert_eq!(count(&path, &sql), 1, "{term}");
    }
    assert_eq!(
        count(
            &path,
            "SELECT count(*) FROM chat_search WHERE chat_search MATCH '非'"
        ),
        0
    );
}

#[test]
fn chat_legacy_retires_only_cos_sessions() {
    let (_dir, path) = migrated_fixture();
    assert_eq!(
        count(
            &path,
            "SELECT count(*) FROM node_sessions WHERE id='cos-session' AND retired_at IS NOT NULL"
        ),
        1
    );
    assert_eq!(
        count(
            &path,
            "SELECT count(*) FROM node_sessions WHERE id='eng-session' AND retired_at IS NULL"
        ),
        1
    );
}
