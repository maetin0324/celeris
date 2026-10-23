//! ADR 2026-10-23-mcp-read-only-db-and-api D2/D3: `db_schema` / `db_query`（scope `db:read`）。
//!
//! daemon の store とは**別の**読み取り専用接続を呼び出しごとに開き（`file:<db>?mode=ro`、
//! `PRAGMA query_only=ON`）、authorizer で SELECT・READ・関数呼び出しと一部の読み取りの PRAGMA だけを許す。
//! 秘密の列（`MASKED_COLUMNS`）は authorizer の `SQLITE_IGNORE` で NULL に置き換える。行数・結果の大きさ・
//! 実行時間（progress handler）に上限を置く。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::limits::Limit;
use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{Batch, Connection, ErrorCode, OpenFlags};
use schemars::JsonSchema;
use serde::Deserialize;
use task_core::McpScope;

use super::{ToolCallFuture, ToolDef, ToolError, ToolOutput, schema};
use crate::auth::AuthedClient;
use crate::state::McpState;

/// 伏せる列（表, 列, 理由）。ADR 2026-10-23-mcp-read-only-db-and-api D3 の表の正本。
pub const MASKED_COLUMNS: &[(&str, &str, &str)] = &[
    ("mcp_clients", "token_hash", "MCP の Bearer token の hash"),
    (
        "cos_run_credentials",
        "token_hash",
        "CoS run の credential token の hash",
    ),
    (
        "browser_trusted_devices",
        "secret_hash",
        "GUI/web の信頼端末の秘密の hash",
    ),
    (
        "browser_trusted_devices",
        "prev_secret_hash",
        "GUI/web の信頼端末の秘密（前世代）の hash",
    ),
    (
        "browser_identities",
        "sealed_blob",
        "保存した browser の login 状態（cookie 等）",
    ),
    (
        "browser_approvals",
        "owner_session_hash",
        "承認の持ち主 session の hash",
    ),
    ("browser_approvals", "nonce", "承認の再利用防止の nonce"),
    ("browser_waits", "resume_key", "待ちの再開に使う鍵"),
    (
        "browser_control_actions",
        "command_json",
        "browser 操作の引数（入力した文字列を含み得る）",
    ),
    (
        "browser_live_events",
        "body_json",
        "Live View の出来事の本文（画面の内容を含み得る）",
    ),
    (
        "node_sessions",
        "session_id",
        "harness の会話 session の再開鍵",
    ),
];

/// 許す PRAGMA（どれも読むだけ。`data_version` は FTS5 の表（`chat_search`）を開くときに SQLite 自身が使う）。
const ALLOWED_PRAGMAS: &[&str] = &[
    "data_version",
    "table_info",
    "table_xinfo",
    "index_list",
    "index_info",
    "index_xinfo",
    "foreign_key_list",
];

/// 呼ばせない関数（拡張の読み込み・ファイル入出力・FTS の tokenizer 差し替え）。
const DENIED_FUNCTIONS: &[&str] = &[
    "load_extension",
    "fts3_tokenizer",
    "readfile",
    "writefile",
    "edit",
];

/// 既定の行数。
pub const DEFAULT_ROWS: usize = 500;
/// 行数の上限。
pub const MAX_ROWS: usize = 5000;
/// 結果（行の JSON）の大きさの上限。
pub const MAX_RESULT_BYTES: usize = 1 << 20;
/// 1 つの値（文字列・BLOB）の長さの上限（`SQLITE_LIMIT_LENGTH`）。
pub const MAX_VALUE_BYTES: i32 = 16 << 20;
/// 実行時間の上限。
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(10);
/// 同時に走らせる問い合わせの数。
pub const MAX_CONCURRENT_QUERIES: usize = 2;

/// 1 回の問い合わせの上限（試験は小さい値を渡す）。
#[derive(Debug, Clone, Copy)]
pub struct QueryLimits {
    pub max_rows: usize,
    pub max_bytes: usize,
    pub timeout: Duration,
}

impl QueryLimits {
    pub fn with_rows(limit: Option<usize>) -> Self {
        Self {
            max_rows: limit.unwrap_or(DEFAULT_ROWS).clamp(1, MAX_ROWS),
            max_bytes: MAX_RESULT_BYTES,
            timeout: QUERY_TIMEOUT,
        }
    }
}

fn is_masked(table: &str, column: &str) -> bool {
    MASKED_COLUMNS
        .iter()
        .any(|(t, c, _)| t.eq_ignore_ascii_case(table) && c.eq_ignore_ascii_case(column))
}

/// authorizer（`sqlite3_set_authorizer`）。準備（prepare）のときに文の各操作について呼ばれる。
fn authorize(ctx: AuthContext<'_>, hits: &Mutex<BTreeSet<String>>) -> Authorization {
    match ctx.action {
        AuthAction::Select | AuthAction::Recursive => Authorization::Allow,
        AuthAction::Read {
            table_name,
            column_name,
        } => {
            if is_masked(table_name, column_name) {
                hits.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(format!("{table_name}.{column_name}"));
                Authorization::Ignore
            } else {
                Authorization::Allow
            }
        }
        AuthAction::Function { function_name } => {
            if DENIED_FUNCTIONS
                .iter()
                .any(|f| f.eq_ignore_ascii_case(function_name))
            {
                Authorization::Deny
            } else {
                Authorization::Allow
            }
        }
        AuthAction::Pragma { pragma_name, .. } => {
            if ALLOWED_PRAGMAS
                .iter()
                .any(|p| p.eq_ignore_ascii_case(pragma_name))
            {
                Authorization::Allow
            } else {
                Authorization::Deny
            }
        }
        _ => Authorization::Deny,
    }
}

/// `file:` URI の path 部分で意味を持つ文字だけを逃がす。
fn uri_path(path: &Path) -> String {
    let mut out = String::new();
    for ch in path.to_string_lossy().chars() {
        match ch {
            '%' => out.push_str("%25"),
            '?' => out.push_str("%3F"),
            '#' => out.push_str("%23"),
            c => out.push(c),
        }
    }
    out
}

fn sql_error(e: rusqlite::Error, timeout: Duration) -> ToolError {
    let (code, detail) = match &e {
        rusqlite::Error::SqliteFailure(err, msg) => {
            (err.code, msg.clone().unwrap_or_else(|| err.to_string()))
        }
        rusqlite::Error::SqlInputError { error, msg, .. } => (error.code, msg.clone()),
        other => return ToolError::invalid_params(format!("SQL の誤り: {other}")),
    };
    // 関数の拒否は SQLITE_ERROR（"not authorized to use function"）で返る。
    if code == ErrorCode::AuthorizationForStatementDenied || detail.contains("not authorized") {
        return ToolError::rejected(format!(
            "読み取り専用の問い合わせで許されない操作です（SELECT・読み取りの関数・table_info 等の PRAGMA だけ）: {detail}"
        ));
    }
    match code {
        ErrorCode::OperationInterrupted => ToolError::rejected(format!(
            "実行時間の上限（{} ms）を超えたので打ち切りました。WHERE や LIMIT で絞ってください",
            timeout.as_millis()
        )),
        ErrorCode::ReadOnly => ToolError::rejected(format!("書き込みはできません: {detail}")),
        _ => ToolError::invalid_params(format!("SQL の誤り: {detail}")),
    }
}

/// 読み取り専用の接続を開き、上限と authorizer を付ける。`hits` には伏せた列（`表.列`）が溜まる。
pub fn open_read_only(
    path: &Path,
    hits: Arc<Mutex<BTreeSet<String>>>,
) -> Result<Connection, ToolError> {
    let uri = format!("file:{}?mode=ro", uri_path(path));
    let conn = Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| ToolError::internal(format!("DB を読み取り専用で開けません: {e}")))?;
    conn.busy_timeout(Duration::from_secs(2))
        .map_err(|e| ToolError::internal(format!("busy_timeout: {e}")))?;
    conn.execute_batch("PRAGMA query_only = ON")
        .map_err(|e| ToolError::internal(format!("query_only: {e}")))?;
    conn.set_limit(Limit::SQLITE_LIMIT_LENGTH, MAX_VALUE_BYTES);
    conn.set_limit(Limit::SQLITE_LIMIT_ATTACHED, 0);
    conn.authorizer(Some(move |ctx: AuthContext<'_>| authorize(ctx, &hits)));
    Ok(conn)
}

fn bind_value(v: &serde_json::Value) -> Result<SqlValue, ToolError> {
    Ok(match v {
        serde_json::Value::Null => SqlValue::Null,
        serde_json::Value::Bool(b) => SqlValue::Integer(i64::from(*b)),
        serde_json::Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => SqlValue::Integer(i),
            (None, Some(f)) => SqlValue::Real(f),
            _ => return Err(ToolError::invalid_params(format!("数として扱えない: {n}"))),
        },
        serde_json::Value::String(s) => SqlValue::Text(s.clone()),
        other => {
            return Err(ToolError::invalid_params(format!(
                "params は文字列・数・真偽・null だけ: {other}"
            )));
        }
    })
}

fn cell(v: ValueRef<'_>) -> serde_json::Value {
    match v {
        ValueRef::Null => serde_json::Value::Null,
        ValueRef::Integer(i) => serde_json::json!(i),
        ValueRef::Real(f) => serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        ValueRef::Text(t) => serde_json::Value::String(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => serde_json::json!({"blob_bytes": b.len()}),
    }
}

/// `db_query` の本体（同期。`spawn_blocking` から呼ぶ）。
pub fn run_query(
    path: &Path,
    sql: &str,
    params: &[serde_json::Value],
    limits: QueryLimits,
) -> Result<serde_json::Value, ToolError> {
    if sql.trim().is_empty() {
        return Err(ToolError::invalid_params("sql が空です"));
    }
    let started = Instant::now();
    let hits = Arc::new(Mutex::new(BTreeSet::new()));
    let conn = open_read_only(path, Arc::clone(&hits))?;
    let deadline = started + limits.timeout;
    conn.progress_handler(1000, Some(move || Instant::now() > deadline));

    let mut batch = Batch::new(&conn, sql);
    let mut stmt = match batch.next() {
        Ok(Some(stmt)) => stmt,
        Ok(None) => return Err(ToolError::invalid_params("sql に文がありません")),
        Err(e) => return Err(sql_error(e, limits.timeout)),
    };
    if !matches!(batch.next(), Ok(None)) {
        return Err(ToolError::rejected(
            "SQL は 1 文だけにしてください（`;` で続けた 2 文目があります）",
        ));
    }
    if !stmt.readonly() {
        return Err(ToolError::rejected(
            "読み取りの文（SELECT 等）だけを実行できます",
        ));
    }
    if stmt.parameter_count() != params.len() {
        return Err(ToolError::invalid_params(format!(
            "params の数（{}）が SQL のプレースホルダの数（{}）と合いません",
            params.len(),
            stmt.parameter_count()
        )));
    }
    let values = params
        .iter()
        .map(bind_value)
        .collect::<Result<Vec<_>, _>>()?;
    let columns: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
    let mut rows_out = Vec::new();
    let mut bytes = 0usize;
    let mut truncated: Option<&str> = None;
    let mut rows = stmt
        .query(rusqlite::params_from_iter(values))
        .map_err(|e| sql_error(e, limits.timeout))?;
    loop {
        let row = match rows.next() {
            Ok(Some(row)) => row,
            Ok(None) => break,
            Err(e) => return Err(sql_error(e, limits.timeout)),
        };
        if rows_out.len() >= limits.max_rows {
            truncated = Some("rows");
            break;
        }
        let mut out = Vec::with_capacity(columns.len());
        for i in 0..columns.len() {
            let v = row
                .get_ref(i)
                .map_err(|e| ToolError::internal(format!("列 {i} を読めません: {e}")))?;
            out.push(cell(v));
        }
        let value = serde_json::Value::Array(out);
        let size = serde_json::to_string(&value).map(|s| s.len()).unwrap_or(0);
        if bytes + size > limits.max_bytes {
            truncated = Some("bytes");
            break;
        }
        bytes += size;
        rows_out.push(value);
    }
    drop(rows);
    drop(stmt);
    let masked: Vec<String> = hits
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .cloned()
        .collect();
    let mut out = serde_json::json!({
        "columns": columns,
        "row_count": rows_out.len(),
        "rows": rows_out,
        "truncated": truncated.is_some(),
        "elapsed_ms": started.elapsed().as_millis() as u64,
    });
    if let Some(reason) = truncated {
        out["truncated_reason"] = serde_json::json!(reason);
    }
    if !masked.is_empty() {
        out["masked"] = serde_json::json!(masked);
    }
    Ok(out)
}

/// `db_schema` の本体（同期）。表・view ごとに列（伏せた列の印付き）と index。
pub fn read_schema(path: &Path) -> Result<serde_json::Value, ToolError> {
    let hits = Arc::new(Mutex::new(BTreeSet::new()));
    let conn = open_read_only(path, hits)?;
    let err = |e: rusqlite::Error| sql_error(e, QUERY_TIMEOUT);
    let mut stmt = conn
        .prepare(
            "SELECT type, name FROM sqlite_master WHERE type IN ('table', 'view') \
             AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .map_err(err)?;
    let objects: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(err)?
        .collect::<Result<_, _>>()
        .map_err(err)?;
    let mut col_stmt = conn
        .prepare("SELECT name, type, \"notnull\", pk FROM pragma_table_info(?1) ORDER BY cid")
        .map_err(err)?;
    let mut idx_stmt = conn
        .prepare(
            "SELECT name, sql FROM sqlite_master WHERE type = 'index' AND tbl_name = ?1 ORDER BY name",
        )
        .map_err(err)?;
    let mut tables = Vec::with_capacity(objects.len());
    for (kind, name) in objects {
        let columns: Vec<serde_json::Value> = col_stmt
            .query_map([&name], |r| {
                let col: String = r.get(0)?;
                let ty: String = r.get(1)?;
                let notnull: i64 = r.get(2)?;
                let pk: i64 = r.get(3)?;
                Ok((col, ty, notnull, pk))
            })
            .map_err(err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?
            .into_iter()
            .map(|(col, ty, notnull, pk)| {
                let mut c = serde_json::json!({"name": col, "type": ty});
                if notnull != 0 {
                    c["notnull"] = serde_json::json!(true);
                }
                if pk != 0 {
                    c["pk"] = serde_json::json!(pk);
                }
                if is_masked(&name, &col) {
                    c["masked"] = serde_json::json!(true);
                }
                c
            })
            .collect();
        let indexes: Vec<serde_json::Value> = idx_stmt
            .query_map([&name], |r| {
                let n: String = r.get(0)?;
                let sql: Option<String> = r.get(1)?;
                Ok(serde_json::json!({"name": n, "sql": sql}))
            })
            .map_err(err)?
            .collect::<Result<_, _>>()
            .map_err(err)?;
        tables.push(serde_json::json!({
            "name": name,
            "kind": kind,
            "columns": columns,
            "indexes": indexes,
        }));
    }
    let masked: Vec<serde_json::Value> = MASKED_COLUMNS
        .iter()
        .map(|(t, c, why)| serde_json::json!({"table": t, "column": c, "reason": why}))
        .collect();
    Ok(serde_json::json!({
        "tables": tables,
        "masked": masked,
        "masked_note": "masked の列は db_query で常に NULL として読まれる（WHERE・JOIN の中でも NULL）",
        "limits": {
            "default_rows": DEFAULT_ROWS,
            "max_rows": MAX_ROWS,
            "max_result_bytes": MAX_RESULT_BYTES,
            "timeout_ms": QUERY_TIMEOUT.as_millis() as u64,
            "max_concurrent": MAX_CONCURRENT_QUERIES,
        },
        "allowed": "SELECT（WITH・WITH RECURSIVE を含む）1 文、読み取りの関数、PRAGMA table_info / table_xinfo / index_list / index_info / index_xinfo / foreign_key_list",
    }))
}

fn db_path(state: &McpState) -> Result<PathBuf, ToolError> {
    state.db_read_path.get().cloned().ok_or_else(|| {
        ToolError::internal("この MCP サーバーには読み取り用の DB が設定されていません")
    })
}

// ---------------------------------------------------------------------------
// 道具の定義
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbSchemaArgs {}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbQueryArgs {
    /// 1 文の SELECT（WITH を含む）。`?` / `?1` のプレースホルダに `params` を順に結び付ける。
    pub sql: String,
    /// プレースホルダの値（文字列・数・真偽・null）。
    #[serde(default)]
    pub params: Vec<serde_json::Value>,
    /// 返す行数（既定 500、上限 5000）。
    #[serde(default)]
    pub limit: Option<usize>,
}

pub fn schema_def() -> ToolDef {
    ToolDef {
        name: "db_schema",
        description: "Celeris の SQLite の表・view・列・index の一覧（読み取り専用）。masked の列は db_query で NULL になる。上限と許される SQL も返す。",
        scope: McpScope::DbRead,
        input_schema: schema::<DbSchemaArgs>,
        call: call_schema,
    }
}

pub fn query_def() -> ToolDef {
    ToolDef {
        name: "db_query",
        description: "Celeris の SQLite に読み取り専用で SELECT を 1 文実行する。返り値は {columns, rows（配列の配列）, row_count, truncated, truncated_reason?, masked?, elapsed_ms}。行数は既定 500・上限 5000、結果 1 MiB、10 秒で打ち切り。秘密の列は NULL（db_schema の masked）。",
        scope: McpScope::DbRead,
        input_schema: schema::<DbQueryArgs>,
        call: call_query,
    }
}

fn call_schema<'a>(
    state: &'a Arc<McpState>,
    _client: &'a AuthedClient,
    args: serde_json::Value,
) -> ToolCallFuture<'a> {
    Box::pin(async move {
        let _: DbSchemaArgs = serde_json::from_value(args)
            .map_err(|e| ToolError::invalid_params(format!("引数が不正です: {e}")))?;
        let path = db_path(state)?;
        let value = tokio::task::spawn_blocking(move || read_schema(&path))
            .await
            .map_err(|e| ToolError::internal(format!("db_schema の実行に失敗しました: {e}")))??;
        Ok(ToolOutput::from_value(value))
    })
}

fn call_query<'a>(
    state: &'a Arc<McpState>,
    _client: &'a AuthedClient,
    args: serde_json::Value,
) -> ToolCallFuture<'a> {
    Box::pin(async move {
        let args: DbQueryArgs = serde_json::from_value(args)
            .map_err(|e| ToolError::invalid_params(format!("引数が不正です: {e}")))?;
        let path = db_path(state)?;
        let Ok(_permit) = Arc::clone(&state.db_query_slots).try_acquire_owned() else {
            return Err(ToolError::rejected(format!(
                "同時に走らせられる db_query は {MAX_CONCURRENT_QUERIES} 本までです。終わってから呼び直してください"
            )));
        };
        let limits = QueryLimits::with_rows(args.limit);
        let value =
            tokio::task::spawn_blocking(move || run_query(&path, &args.sql, &args.params, limits))
                .await
                .map_err(|e| {
                    ToolError::internal(format!("db_query の実行に失敗しました: {e}"))
                })??;
        Ok(ToolOutput::from_value(value))
    })
}

/// `mcp_calls.request` に残す要求（SQL 本文と params）。
pub fn audit_request(args: &serde_json::Value) -> Option<String> {
    let sql = args.get("sql")?.as_str()?;
    match args.get("params") {
        Some(p) if p.as_array().is_some_and(|a| !a.is_empty()) => {
            Some(format!("{sql} -- params: {p}"))
        }
        _ => Some(sql.to_string()),
    }
}

#[cfg(test)]
#[path = "db_tests.rs"]
mod tests;
