//! `db_query` / `db_schema` の決定的な試験（一時ファイルの DB。外部ネットワークに出ない）。

use super::*;
use task_core::{McpClient, McpClientStore, SqliteStore};
use time::OffsetDateTime;

struct Db {
    _dir: tempfile::TempDir,
    path: PathBuf,
    store: SqliteStore,
}

/// 本物の migration を当てた DB（WAL）に、秘密を含む行を入れる。
fn db() -> Db {
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("celeris.sqlite3");
    let store = SqliteStore::open(&path).expect("open");
    store
        .mcp_client_create(&McpClient {
            id: "rdc".into(),
            name: "rdc".into(),
            token_hash: Some("deadbeefsecret".into()),
            scopes: vec![McpScope::DbRead],
            created_at: OffsetDateTime::now_utc(),
            last_used_at: None,
            revoked_at: None,
        })
        .expect("client");
    let conn = Connection::open(&path).expect("raw");
    conn.execute_batch(
        "INSERT INTO node_sessions (id, node_id, kind, adapter, session_id, created_at, last_used_at) \
         VALUES ('s1', 'cos', 'chat', 'claude-code', 'resume-key-secret', 't', 't'); \
         INSERT INTO browser_trusted_devices (id, name, secret_hash, prev_secret_hash, created_at, expires_at, actor) \
         VALUES ('d1', 'phone', 'device-secret', 'old-secret', 1, 2, 'human');",
    )
    .expect("seed");
    Db {
        _dir: dir,
        path,
        store,
    }
}

fn q(db: &Db, sql: &str) -> Result<serde_json::Value, ToolError> {
    run_query(&db.path, sql, &[], QueryLimits::with_rows(None))
}

fn rejected(r: Result<serde_json::Value, ToolError>) -> String {
    match r {
        Err(e) => {
            assert_eq!(
                e.code,
                super::super::ToolErrorCode::Rejected,
                "{}",
                e.message
            );
            e.message
        }
        Ok(v) => panic!("expected a rejection, got {v}"),
    }
}

#[test]
fn select_returns_columns_and_rows() {
    let db = db();
    let out = run_query(
        &db.path,
        "SELECT id, name FROM mcp_clients WHERE id = ?1",
        &[serde_json::json!("rdc")],
        QueryLimits::with_rows(None),
    )
    .expect("query");
    assert_eq!(out["columns"], serde_json::json!(["id", "name"]));
    assert_eq!(out["rows"], serde_json::json!([["rdc", "rdc"]]));
    assert_eq!(out["truncated"], serde_json::json!(false));
    assert!(out.get("masked").is_none());
}

#[test]
fn writes_are_rejected() {
    let db = db();
    for sql in [
        "INSERT INTO mcp_clients (id, name, scopes, created_at) VALUES ('x', 'x', '', 't')",
        "UPDATE mcp_clients SET scopes = 'org:write'",
        "DELETE FROM mcp_clients",
        "CREATE TABLE evil (x)",
        "CREATE TEMP TABLE evil (x)",
        "DROP TABLE mcp_calls",
        "ALTER TABLE mcp_calls ADD COLUMN y",
        "REPLACE INTO mcp_clients (id, name, scopes, created_at) VALUES ('rdc', 'x', '', 't')",
        "BEGIN",
        "VACUUM",
        "ANALYZE",
        "REINDEX",
    ] {
        let msg = rejected(q(&db, sql));
        assert!(!msg.is_empty(), "{sql}");
    }
    // 何も変わっていない。
    let n = db.store.mcp_client_list().expect("list").len();
    assert_eq!(n, 1);
}

#[test]
fn attach_and_pragma_writes_and_extensions_are_rejected() {
    let db = db();
    rejected(q(&db, "ATTACH DATABASE '/tmp/x.db' AS x"));
    rejected(q(&db, "PRAGMA query_only = OFF"));
    rejected(q(&db, "PRAGMA journal_mode = DELETE"));
    rejected(q(&db, "PRAGMA writable_schema = ON"));
    rejected(q(&db, "PRAGMA user_version"));
    rejected(q(&db, "SELECT load_extension('/tmp/evil.so')"));
    rejected(q(&db, "SELECT fts3_tokenizer('simple')"));
}

#[test]
fn read_only_pragmas_are_allowed() {
    let db = db();
    let out = q(&db, "PRAGMA table_info(mcp_calls)").expect("table_info");
    let names: Vec<&str> = out["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| r[1].as_str().expect("name"))
        .collect();
    assert!(names.contains(&"request"), "{names:?}");
    q(&db, "SELECT name FROM pragma_table_info('tasks')").expect("pragma function");
    q(&db, "PRAGMA index_list(events)").expect("index_list");
}

#[test]
fn only_one_statement_is_allowed() {
    let db = db();
    let msg = rejected(q(&db, "SELECT 1; DELETE FROM mcp_clients"));
    assert!(msg.contains("1 文"), "{msg}");
    rejected(q(&db, "SELECT 1; SELECT 2"));
    // 末尾の `;`・空白・コメントは 2 文目ではない。
    q(&db, "SELECT 1;  -- done\n").expect("trailing semicolon");
}

#[test]
fn secret_columns_read_as_null_even_in_where() {
    let db = db();
    let out = q(&db, "SELECT id, token_hash FROM mcp_clients").expect("query");
    assert_eq!(out["rows"], serde_json::json!([["rdc", null]]));
    assert_eq!(out["masked"], serde_json::json!(["mcp_clients.token_hash"]));

    let out = q(&db, "SELECT * FROM node_sessions").expect("star");
    let cols = out["columns"].as_array().expect("cols");
    let idx = cols.iter().position(|c| c == "session_id").expect("col");
    assert_eq!(out["rows"][0][idx], serde_json::Value::Null);
    assert!(!out.to_string().contains("resume-key-secret"));

    // WHERE で値を当てようとしても NULL として評価される（推測に使えない）。
    let out = q(
        &db,
        "SELECT count(*) FROM mcp_clients WHERE token_hash = 'deadbeefsecret'",
    )
    .expect("where");
    assert_eq!(out["rows"], serde_json::json!([[0]]));
    let out = q(
        &db,
        "SELECT secret_hash, prev_secret_hash, length(secret_hash) FROM browser_trusted_devices",
    )
    .expect("devices");
    assert_eq!(out["rows"], serde_json::json!([[null, null, null]]));
    // 別名や副問い合わせ・view 経由でも同じ。
    let out = q(
        &db,
        "SELECT x FROM (SELECT token_hash AS x FROM mcp_clients)",
    )
    .expect("subquery");
    assert_eq!(out["rows"], serde_json::json!([[null]]));
}

#[test]
fn row_limit_truncates() {
    let db = db();
    let sql = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 20) SELECT x FROM c";
    let out = run_query(&db.path, sql, &[], QueryLimits::with_rows(Some(5))).expect("query");
    assert_eq!(out["row_count"], serde_json::json!(5));
    assert_eq!(out["truncated"], serde_json::json!(true));
    assert_eq!(out["truncated_reason"], serde_json::json!("rows"));
    // 上限は 5000 にはめる。
    assert_eq!(QueryLimits::with_rows(Some(1_000_000)).max_rows, MAX_ROWS);
    assert_eq!(QueryLimits::with_rows(None).max_rows, DEFAULT_ROWS);
}

#[test]
fn result_size_limit_truncates() {
    let db = db();
    let sql = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 100) \
               SELECT x, printf('%.200c', 'a') FROM c";
    let limits = QueryLimits {
        max_rows: 1000,
        max_bytes: 2000,
        timeout: QUERY_TIMEOUT,
    };
    let out = run_query(&db.path, sql, &[], limits).expect("query");
    assert_eq!(out["truncated_reason"], serde_json::json!("bytes"));
    let n = out["row_count"].as_u64().expect("n");
    assert!(n > 0 && n < 100, "{n}");
}

#[test]
fn time_limit_interrupts_a_query_that_never_ends() {
    let db = db();
    let sql =
        "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c) SELECT count(*) FROM c";
    let limits = QueryLimits {
        max_rows: 10,
        max_bytes: MAX_RESULT_BYTES,
        timeout: Duration::from_millis(200),
    };
    let msg = rejected(run_query(&db.path, sql, &[], limits));
    assert!(msg.contains("実行時間"), "{msg}");
}

#[test]
fn an_open_read_does_not_block_daemon_writes() {
    let db = db();
    // 読み取り接続で文を途中まで進めて、読み取り transaction を開いたままにする。
    let hits = Arc::new(Mutex::new(BTreeSet::new()));
    let conn = open_read_only(&db.path, hits).expect("open ro");
    let mut stmt = conn.prepare("SELECT id FROM mcp_clients").expect("prepare");
    let mut rows = stmt.query([]).expect("query");
    assert!(rows.next().expect("step").is_some());
    // その間に daemon の store（WAL）が書ける（busy で待たない）。
    let started = Instant::now();
    store_write(&db.store, "second");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "write waited on the reader"
    );
    drop(rows);
    drop(stmt);
    assert_eq!(db.store.mcp_client_list().expect("list").len(), 2);
}

fn store_write(store: &SqliteStore, id: &str) {
    store
        .mcp_client_create(&McpClient {
            id: id.into(),
            name: id.into(),
            token_hash: None,
            scopes: vec![],
            created_at: OffsetDateTime::now_utc(),
            last_used_at: None,
            revoked_at: None,
        })
        .expect("write while a reader is open");
}

#[test]
fn params_must_match_placeholders_and_be_scalars() {
    let db = db();
    let err = run_query(&db.path, "SELECT ?1", &[], QueryLimits::with_rows(None)).unwrap_err();
    assert_eq!(err.code, super::super::ToolErrorCode::InvalidParams);
    let err = run_query(
        &db.path,
        "SELECT ?1",
        &[serde_json::json!({"a": 1})],
        QueryLimits::with_rows(None),
    )
    .unwrap_err();
    assert_eq!(err.code, super::super::ToolErrorCode::InvalidParams);
}

#[test]
fn schema_lists_tables_and_every_masked_column_exists() {
    let db = db();
    let out = read_schema(&db.path).expect("schema");
    let tables = out["tables"].as_array().expect("tables");
    for (table, column, _) in MASKED_COLUMNS {
        let t = tables
            .iter()
            .find(|t| t["name"] == *table)
            .unwrap_or_else(|| panic!("masked table {table} is not in the schema"));
        let c = t["columns"]
            .as_array()
            .expect("columns")
            .iter()
            .find(|c| c["name"] == *column)
            .unwrap_or_else(|| panic!("masked column {table}.{column} is not in the schema"));
        assert_eq!(c["masked"], serde_json::json!(true), "{table}.{column}");
    }
    let events = tables
        .iter()
        .find(|t| t["name"] == "events")
        .expect("events");
    assert!(!events["indexes"].as_array().expect("indexes").is_empty());
    assert_eq!(
        out["masked"].as_array().expect("masked").len(),
        MASKED_COLUMNS.len()
    );
}

#[test]
fn audit_request_keeps_sql_and_params() {
    assert_eq!(
        audit_request(&serde_json::json!({"sql": "SELECT 1"})).as_deref(),
        Some("SELECT 1")
    );
    assert_eq!(
        audit_request(&serde_json::json!({"sql": "SELECT ?1", "params": ["a"]})).as_deref(),
        Some("SELECT ?1 -- params: [\"a\"]")
    );
}

/// `docs/guides/chatgpt-rdc.md` の SQL の例は、どれも今の schema で authorizer を通って動く。
#[test]
fn guide_examples_run_against_the_current_schema() {
    let db = db();
    let guide = include_str!("../../../../docs/guides/chatgpt-rdc.md");
    let mut n = 0;
    for block in guide.split("```sql").skip(1) {
        let sql = block.split("```").next().expect("closed block");
        let params = if sql.contains("?1") {
            vec![serde_json::json!("x")]
        } else {
            vec![]
        };
        run_query(&db.path, sql, &params, QueryLimits::with_rows(None))
            .unwrap_or_else(|e| panic!("{sql}: {}", e.message));
        n += 1;
    }
    assert!(n >= 5, "{n}");
}
