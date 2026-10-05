use rusqlite::{Connection, params};
use serde_json::Value;

use super::SqliteStore;

#[test]
fn chat_legacy_projects_order_idempotence_fts_and_session_retire() {
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
    drop(conn);

    drop(SqliteStore::open(&path).expect("migrate and import"));
    let conn = Connection::open(&path).expect("inspect");
    let threads: i64 = conn
        .query_row(
            "SELECT count(*) FROM chat_threads WHERE kind='legacy'",
            [],
            |r| r.get(0),
        )
        .expect("threads");
    assert_eq!(threads, 3);
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
    let mut stmt = conn
        .prepare(
            "SELECT legacy_message_id,seq,role,run_id,metadata_json FROM chat_messages \
         WHERE thread_id='legacy:project:p1' ORDER BY seq",
        )
        .expect("messages");
    let rows: Vec<_> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?,
            ))
        })
        .expect("query")
        .map(|r| r.expect("row"))
        .collect();
    assert_eq!(
        rows.iter()
            .map(|r| (&r.0, r.1, r.2.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (&"a".to_owned(), 1, "user"),
            (&"same-time".to_owned(), 2, "assistant"),
            (&"b".to_owned(), 3, "assistant")
        ]
    );
    assert_eq!(rows[2].3.as_deref(), Some("run-b"));
    let meta: Value = serde_json::from_str(&rows[2].4).expect("metadata");
    assert_eq!(meta["legacy_source"]["task_id"], "task-b");
    assert_eq!(meta["legacy_source"]["metadata"]["action"], "kept");
    drop(stmt);
    let hits: i64 = conn
        .query_row(
            "SELECT count(*) FROM chat_search WHERE chat_search MATCH '回答特有語'",
            [],
            |r| r.get(0),
        )
        .expect("fts");
    assert_eq!(hits, 1);
    let retired: i64 = conn
        .query_row(
            "SELECT count(*) FROM node_sessions WHERE id='cos-session' AND retired_at IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .expect("retired");
    assert_eq!(retired, 1);
    let old_count: i64 = conn
        .query_row("SELECT count(*) FROM messages", [], |r| r.get(0))
        .expect("old count");
    assert_eq!(old_count, 6);
    let non_cos: i64 = conn
        .query_row(
            "SELECT count(*) FROM messages WHERE id='other' AND text='非 CoS 本文'",
            [],
            |r| r.get(0),
        )
        .expect("non CoS preserved");
    assert_eq!(non_cos, 1);
    drop(conn);

    drop(SqliteStore::open(&path).expect("reopen"));
    let conn = Connection::open(&path).expect("inspect again");
    let count: i64 = conn
        .query_row("SELECT count(*) FROM chat_messages", [], |r| r.get(0))
        .expect("count");
    assert_eq!(count, 5);
    conn.execute(
        "INSERT INTO messages(id,node_id,project_id,role,text,created_at) \
         VALUES('new','cos','p1','user','後続本文','2026-10-05T00:00:03Z')",
        [],
    )
    .expect("late old write");
    drop(conn);
    drop(SqliteStore::open(&path).expect("catch up"));
    let conn = Connection::open(&path).expect("final inspect");
    let seq: i64 = conn
        .query_row(
            "SELECT seq FROM chat_messages WHERE legacy_message_id='new'",
            [],
            |r| r.get(0),
        )
        .expect("new projection");
    assert_eq!(seq, 4);
    let count: i64 = conn
        .query_row("SELECT count(*) FROM chat_messages", [], |r| r.get(0))
        .expect("final count");
    assert_eq!(count, 6);
}
