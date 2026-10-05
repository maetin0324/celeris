use super::*;
use crate::store::SqliteStore;
use rusqlite::{Connection, params};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

fn roundtrip<T: DeserializeOwned + Serialize>(value: Value) {
    let model: T = serde_json::from_value(value.clone()).expect("decode ADR example");
    assert_eq!(serde_json::to_value(model).expect("encode"), value);
}

fn thread() -> Value {
    json!({"id":"t","kind":"human","title":"画面の修正","project_id":null,
      "status":"open","queue_paused":false,"active_run_id":null,"queued_count":0,
      "revision":1,"created_at":"2026-10-05T00:00:00Z","updated_at":"2026-10-05T00:00:00Z"})
}
fn message() -> Value {
    json!({"id":"m","thread_id":"t","seq":1,"role":"user","text":"直して",
      "state":"queued","client_message_id":"c","reply_to_id":null,"run_id":null,
      "attachment_ids":["a"],"cards":[],"created_at":"2026-10-05T00:00:00Z",
      "updated_at":"2026-10-05T00:00:00Z"})
}
fn run() -> Value {
    json!({"id":"r","thread_id":"t","input_message_id":"m","output_message_id":"m2",
      "state":"running","reason":null,"harness":"claude-code","llm_source":"claude_oauth",
      "provider":"claude-pool","account_id":"account","model":"resolved-model",
      "tier":"frontier","session_mode":"resumed","started_at":"2026-10-05T00:00:00Z",
      "finished_at":null})
}
fn card() -> Value {
    json!({"kind":"task","id":"task-id","title":"修正","state":"running",
      "href":"/tasks/task-id","actor":"cos","reason":null,"operation_id":null})
}
#[test]
fn chat_model_adr_examples_roundtrip() {
    roundtrip::<ChatThread>(thread());
    roundtrip::<ChatMessage>(message());
    roundtrip::<ChatRun>(run());
    roundtrip::<ChatCard>(card());
    roundtrip::<ChatAttachment>(json!({"id":"a","thread_id":"t","name":"screen.png",
      "media_type":"image/png","size_bytes":123,"sha256":"64桁hex","state":"ready",
      "preview_url":"/api/v1/chat/attachments/a/preview",
      "download_url":"/api/v1/chat/attachments/a/content","expires_at":null}));
}
#[test]
fn chat_model_event_data_roundtrip() {
    let data = [
        ("message", json!({"message":message()})),
        ("text_delta", json!({"offset":0,"text":"確認します"})),
        (
            "status",
            json!({"phase":"thinking","summary":"対象を確認中"}),
        ),
        (
            "tool",
            json!({"call_id":"call-1","name":"Bash","state":"running","summary":"ファイルを確認","detail":null,"error":false,"truncated":false}),
        ),
        ("run", json!({"run":run()})),
        ("queue", json!({"message_ids":["m3"],"paused":false})),
        ("card", json!({"card":card()})),
        ("thread", json!({"thread":thread()})),
    ];
    for (event_type, data) in data {
        roundtrip::<ChatEvent>(json!({"id":"124","type":event_type,"thread_id":"t",
          "run_id":"r","message_id":"m2","at":"2026-10-05T00:00:00Z","data":data}));
    }
}
#[test]
fn chat_model_requests_reject_unknown_fields() {
    let good = json!({"client_message_id":"c","text":"直して","attachment_ids":["a"],
      "reply_to_id":null,"mode":"queue","resume_queue":false});
    roundtrip::<ChatPostMessageRequest>(good.clone());
    let mut bad = good;
    bad["unexpected"] = json!(true);
    assert!(serde_json::from_value::<ChatPostMessageRequest>(bad).is_err());
    assert!(
        serde_json::from_value::<ChatCreateThreadRequest>(json!({
      "title":"t","project_id":null,"client_thread_id":"k","kind":"inbox"}))
        .is_err()
    );
}

fn new_db() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("temporary DB directory");
    let path = dir.path().join("chat.sqlite3");
    drop(SqliteStore::open(&path).expect("migrate"));
    (dir, path)
}
fn has_table(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE name=?1",
        [name],
        |r| r.get::<_, i64>(0),
    )
    .expect("sqlite_master")
        == 1
}
#[test]
fn chat_migration_new_database_has_all_tables_and_constraints() {
    let (_dir, path) = new_db();
    let conn = Connection::open(path).expect("open");
    for name in [
        "chat_threads",
        "chat_messages",
        "chat_runs",
        "chat_events",
        "chat_attachments",
        "chat_attachment_refs",
        "cos_inbox_items",
        "cos_operations",
        "cos_notification_routes",
        "chat_client_requests",
        "chat_upload_reservations",
        "chat_search",
    ] {
        assert!(has_table(&conn, name), "missing {name}");
    }
    let now = "2026-10-05T00:00:00Z";
    let insert_thread = "INSERT INTO chat_threads(id,kind,title,status,created_at,updated_at) VALUES(?1,?2,?3,'open',?4,?4)";
    conn.execute(insert_thread, params!["t", "inbox", "受信箱", now])
        .expect("inbox");
    assert!(
        conn.execute(insert_thread, params!["t2", "inbox", "duplicate", now])
            .is_err()
    );
    conn.execute(insert_thread, params!["t2", "human", "画面の修正", now])
        .expect("human");
    assert!(
        conn.execute(insert_thread, params!["bad", "invalid", "bad", now])
            .is_err()
    );
    let insert_message = "INSERT INTO chat_messages(id,thread_id,seq,role,text,state,created_at,updated_at) VALUES(?1,'t2',?2,'user',?3,'queued',?4,?4)";
    conn.execute(insert_message, params!["m", 1, "直して", now])
        .expect("message");
    assert!(
        conn.execute(insert_message, params!["m2", 1, "duplicate seq", now])
            .is_err()
    );
    let hits: i64 = conn
        .query_row(
            "SELECT count(*) FROM chat_search WHERE chat_search MATCH '直して'",
            [],
            |r| r.get(0),
        )
        .expect("FTS message");
    assert_eq!(hits, 1);
    conn.execute(
        "UPDATE chat_messages SET text='確認します' WHERE id='m'",
        [],
    )
    .expect("update text");
    let hits: i64 = conn
        .query_row(
            "SELECT count(*) FROM chat_search WHERE chat_search MATCH '直して'",
            [],
            |r| r.get(0),
        )
        .expect("FTS old text");
    assert_eq!(hits, 0);
    conn.execute("DELETE FROM chat_messages WHERE id='m'", [])
        .expect("delete message");
    let hits: i64 = conn
        .query_row(
            "SELECT count(*) FROM chat_search WHERE chat_search MATCH '確認します'",
            [],
            |r| r.get(0),
        )
        .expect("FTS deleted text");
    assert_eq!(hits, 0);
    let title_hits: i64 = conn
        .query_row(
            "SELECT count(*) FROM chat_search WHERE chat_search MATCH '画面の修正'",
            [],
            |r| r.get(0),
        )
        .expect("FTS title");
    assert_eq!(title_hits, 1);
    conn.execute("UPDATE chat_threads SET title='別題' WHERE id='t2'", [])
        .expect("update title");
    let title_hits: i64 = conn
        .query_row(
            "SELECT count(*) FROM chat_search WHERE chat_search MATCH '画面の修正'",
            [],
            |r| r.get(0),
        )
        .expect("FTS old title");
    assert_eq!(title_hits, 0);
    let insert_session = "INSERT INTO node_sessions(id,node_id,kind,adapter,session_id,created_at,last_used_at,thread_id) VALUES(?1,'cos','cos_chat','claude-code','s',?2,?2,?3)";
    assert!(
        conn.execute(insert_session, params!["s0", now, Option::<String>::None])
            .is_err()
    );
    conn.execute(insert_session, params!["s1", now, "t2"])
        .expect("active session");
    assert!(
        conn.execute(insert_session, params!["s2", now, "t2"])
            .is_err()
    );
    conn.execute(
        "UPDATE node_sessions SET retired_at=?1 WHERE id='s1'",
        [now],
    )
    .expect("retire");
    conn.execute(insert_session, params!["s2", now, "t2"])
        .expect("replacement");
}
#[test]
fn chat_migration_upgrade_from_47_preserves_existing_rows() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("old.sqlite3");
    let mut conn = Connection::open(&path).expect("open");
    conn.execute_batch(
        "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
    )
    .expect("migration ledger");
    for version in (1..=37).chain(41..=47) {
        SqliteStore::apply_migration_version(&mut conn, version).expect("build version 47");
    }
    conn.execute("INSERT INTO node_sessions(id,node_id,kind,adapter,session_id,created_at,last_used_at) VALUES('old','cos','conversation','claude-code','session','2026-10-05T00:00:00Z','2026-10-05T00:00:00Z')",[]).expect("old session");
    drop(conn);
    drop(SqliteStore::open(&path).expect("upgrade"));
    let conn = Connection::open(path).expect("reopen");
    let row: (String, Option<String>, i64) = conn
        .query_row(
            "SELECT kind,thread_id,summary_through_seq FROM node_sessions WHERE id='old'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("preserved session");
    assert_eq!(row, ("conversation".into(), None, 0));
    assert!(has_table(&conn, "chat_search"));
    conn.execute(
        "INSERT INTO chat_threads(id,kind,title,status,created_at,updated_at) VALUES('upgraded','human','旧版からの会話','open','2026-10-05T00:00:00Z','2026-10-05T00:00:00Z')",
        [],
    )
    .expect("thread after upgrade");
    conn.execute(
        "INSERT INTO chat_messages(id,thread_id,seq,role,text,state,created_at,updated_at) VALUES('upgraded-m','upgraded',1,'user','移行後の本文','queued','2026-10-05T00:00:00Z','2026-10-05T00:00:00Z')",
        [],
    )
    .expect("message after upgrade");
    assert!(conn.execute(
        "INSERT INTO chat_messages(id,thread_id,seq,role,text,state,created_at,updated_at) VALUES('duplicate','upgraded',1,'user','重複','queued','2026-10-05T00:00:00Z','2026-10-05T00:00:00Z')",
        [],
    ).is_err());
    let hits: i64 = conn
        .query_row(
            "SELECT count(*) FROM chat_search WHERE chat_search MATCH '移行後の本文'",
            [],
            |r| r.get(0),
        )
        .expect("FTS after upgrade");
    assert_eq!(hits, 1);
    let version: u32 = conn
        .query_row("SELECT max(version) FROM schema_migrations", [], |r| {
            r.get(0)
        })
        .expect("version");
    assert_eq!(version, 50);
}
