use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};
use serde_json::Value;

use super::SqliteStore;

/// Builds a pre-cos_chat database with old CoS rows in two projects and the
/// global scope, one non-CoS row and one live CoS session, then opens the
/// store so the migration and the Console projection run. No legacy thread
/// exists in such a database, so the rows land on the fixed CoS threads
/// (ADR 2026-10-22-cos-fixed-threads D1).
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
    for (id, title) in [("p1", "案件一"), ("p2", "案件二")] {
        conn.execute(
            "INSERT INTO projects(id,title,request,status,created_at,updated_at) \
             VALUES(?1,?2,?2,'active','2026-10-05T00:00:00Z','2026-10-05T00:00:00Z')",
            params![id, title],
        )
        .expect("project");
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

/// The fixed CoS thread of a scope (`None` = general).
fn cos_thread(path: &Path, project: Option<&str>) -> String {
    let conn = Connection::open(path).expect("inspect");
    conn.query_row(
        "SELECT id FROM chat_threads WHERE (?1 IS NULL AND kind='general') \
         OR (?1 IS NOT NULL AND kind='project' AND project_id=?1)",
        [project],
        |r| r.get(0),
    )
    .expect("cos thread")
}

#[test]
fn chat_console_projection_lands_on_fixed_cos_threads_without_legacy_threads() {
    let (_dir, path) = migrated_fixture();
    let conn = Connection::open(&path).expect("inspect");
    let scoped: Vec<(String, Option<String>, String)> = conn
        .prepare("SELECT kind,project_id,title FROM chat_threads ORDER BY kind,project_id")
        .expect("scopes")
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .expect("scope rows")
        .map(|r| r.expect("scope"))
        .collect();
    assert_eq!(
        scoped,
        vec![
            ("general".into(), None, "統括 CoS".into()),
            ("project".into(), Some("p1".into()), "CoS: 案件一".into()),
            ("project".into(), Some("p2".into()), "CoS: 案件二".into()),
        ]
    );
    let general = cos_thread(&path, None);
    let p2 = cos_thread(&path, Some("p2"));
    assert_eq!(
        count(
            &path,
            &format!("SELECT count(*) FROM chat_messages WHERE thread_id='{general}'")
        ),
        1
    );
    assert_eq!(
        count(
            &path,
            &format!("SELECT count(*) FROM chat_messages WHERE thread_id='{p2}'")
        ),
        1
    );
}

/// A database that migration 0092 left with legacy threads keeps projecting old Console rows
/// into them; the projection never creates a legacy thread.
#[test]
fn chat_console_projection_reuses_legacy_threads_kept_by_migration_0092() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("v91.sqlite3");
    let mut conn = Connection::open(&path).expect("open fixture");
    conn.execute_batch(
        "CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY,applied_at TEXT NOT NULL)",
    )
    .expect("ledger");
    for version in (1..=37).chain(41..=67).chain(70..=91) {
        SqliteStore::apply_migration_version(&mut conn, version).expect("schema 91");
    }
    conn.execute_batch(
        "INSERT INTO projects(id,title,request,status,created_at,updated_at) \
           VALUES('p1','案件一','案件一','active','2026-10-05T00:00:00Z','2026-10-05T00:00:00Z');
         INSERT INTO chat_threads(id,kind,title,status,created_at,updated_at) \
           VALUES('legacy:global','legacy','旧 CoS 会話','open','2026-10-05T00:00:00Z','2026-10-05T00:00:00Z');
         INSERT INTO messages(id,node_id,project_id,role,text,created_at) \
           VALUES('old-global','cos',NULL,'user','全体の旧本文','2026-10-05T00:00:01Z'),
                 ('old-p1','cos','p1','user','案件の旧本文','2026-10-05T00:00:02Z');",
    )
    .expect("v91 rows");
    drop(conn);
    drop(SqliteStore::open(&path).expect("migrate to 92 and project"));
    assert_eq!(
        count(
            &path,
            "SELECT count(*) FROM chat_messages WHERE legacy_message_id='old-global' \
             AND thread_id='legacy:global'"
        ),
        1
    );
    // The project had no legacy thread: its row lands on the project's CoS thread.
    let p1 = cos_thread(&path, Some("p1"));
    assert_eq!(
        count(
            &path,
            &format!(
                "SELECT count(*) FROM chat_messages WHERE legacy_message_id='old-p1' AND thread_id='{p1}'"
            )
        ),
        1
    );
    assert_eq!(
        count(
            &path,
            "SELECT count(*) FROM chat_threads WHERE kind='legacy'"
        ),
        1
    );
}

#[test]
fn chat_legacy_seq_follows_created_at_then_id_and_maps_roles() {
    let (_dir, path) = migrated_fixture();
    let conn = Connection::open(&path).expect("inspect");
    let p1 = cos_thread(&path, Some("p1"));
    let mut stmt = conn
        .prepare(
            "SELECT legacy_message_id,seq,role,run_id,metadata_json FROM chat_messages \
             WHERE thread_id=?1 ORDER BY seq",
        )
        .expect("messages");
    let rows: Vec<(String, i64, String, Option<String>, String)> = stmt
        .query_map([&p1], |r| {
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
    assert_eq!(count(&path, "SELECT count(*) FROM chat_threads"), 3);
    assert_eq!(
        count(
            &path,
            "SELECT count(*) FROM chat_threads WHERE kind='legacy'"
        ),
        0
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
