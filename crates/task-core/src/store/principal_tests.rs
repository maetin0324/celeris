//! ADR 2026-10-27-event-principals: 作成・retry の event に主体が残る。migration 0097 が comment の
//! `author_kind` を author から直し、events は書き換えない。
use rusqlite::{Connection, params};

use super::tests::sample_task;
use super::*;
use crate::comment::{CommentAuthorKind, TaskComment};

fn created_by(store: &SqliteStore, id: TaskId) -> Vec<Option<String>> {
    store
        .events_for(id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::Created { created_by, .. } => Some(created_by),
            _ => None,
        })
        .collect()
}

#[test]
fn create_and_retry_record_the_principal() {
    let store = SqliteStore::open_in_memory().unwrap();
    let daemon = sample_task(Status::Draft);
    store.create_task(&daemon, Vec::new()).unwrap();
    assert_eq!(created_by(&store, daemon.id), [Some("daemon".to_string())]);

    for actor in [
        "human:web",
        "operator:claude-code",
        "mcp:chatgpt-rdc",
        "cos:t1",
    ] {
        let task = sample_task(Status::Draft);
        store.create_task_by(&task, actor, Vec::new()).unwrap();
        assert_eq!(created_by(&store, task.id), [Some(actor.to_string())]);
    }

    let failed = sample_task(Status::Failed);
    store.insert(&failed).unwrap();
    let retry = sample_task(Status::Draft);
    store
        .retry_task(failed.id, &retry, "operator:claude-code")
        .unwrap();
    assert_eq!(
        created_by(&store, retry.id),
        [Some("operator:claude-code".to_string())]
    );
    assert!(store.events_for(retry.id).unwrap().iter().any(|(_, e)| matches!(
        e,
        Event::Retried { from, by: Some(by) } if *from == failed.id && by == "operator:claude-code"
    )));
}

#[test]
fn migration_0097_fixes_the_author_kind_of_misrecorded_comments() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("celeris.db");
    let task = sample_task(Status::Draft);
    {
        let store = SqliteStore::open(&path).unwrap();
        store.create_task(&task, Vec::new()).unwrap();
    }
    let events_before: i64;
    {
        // 0095 の形（author_kind は human/node/system だけ）へ戻し、誤記録の行を入れる。
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "DROP TABLE task_comments;
             CREATE TABLE task_comments (
                 id TEXT PRIMARY KEY, task_id TEXT NOT NULL,
                 author_kind TEXT NOT NULL CHECK(author_kind IN ('human','node','system')),
                 author TEXT, body TEXT NOT NULL, run_id TEXT, created_at TEXT NOT NULL);
             DELETE FROM schema_migrations WHERE version = 97;",
        )
        .unwrap();
        for (i, (kind, author)) in [
            ("human", Some("cos")),
            ("human", Some("cos:thread-1")),
            ("human", Some("mcp:chatgpt-rdc")),
            ("human", Some("operator:claude-code")),
            ("human", Some("human:web")),
            ("human", None),
            ("node", Some("software-engineering")),
            ("system", Some("cos-not-a-principal")),
        ]
        .into_iter()
        .enumerate()
        {
            conn.execute(
                "INSERT INTO task_comments (id, task_id, author_kind, author, body, run_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, 'b', NULL, ?5)",
                params![
                    crate::comment::CommentId::new().to_string(),
                    task.id.to_string(),
                    kind,
                    author,
                    format!("2026-10-27T00:00:0{i}Z")
                ],
            )
            .unwrap();
        }
        events_before = conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
    }
    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    let conn = Connection::open(&path).unwrap();
    let misrecorded: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM task_comments WHERE author_kind = 'human'
               AND (author = 'cos' OR author LIKE 'cos:%' OR author LIKE 'mcp:%' OR author LIKE 'operator:%')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(misrecorded, 0);
    let events_after: i64 = conn
        .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(events_after, events_before);

    let kinds: Vec<(Option<String>, CommentAuthorKind)> = store
        .comments_for(task.id)
        .unwrap()
        .into_iter()
        .map(|c: TaskComment| (c.author, c.author_kind))
        .collect();
    use CommentAuthorKind::*;
    let s = |a: &str| Some(a.to_string());
    assert_eq!(
        kinds,
        [
            (s("cos"), Cos),
            (s("cos:thread-1"), Cos),
            (s("mcp:chatgpt-rdc"), Mcp),
            (s("operator:claude-code"), Operator),
            (s("human:web"), Human),
            (None, Human),
            (s("software-engineering"), Node),
            (s("cos-not-a-principal"), System),
        ]
    );
    // 新しい種別は書ける（CHECK が広がった）。
    let c = TaskComment::new(
        task.id,
        Cos,
        s("cos:t2"),
        "x".into(),
        None,
        time::OffsetDateTime::now_utc(),
    );
    store.comment_add(&c, None).unwrap();
}
