//! ADR 2026-10-23-mcp-read-only-db-and-api D1: `celerisctl mcp client scopes` は token を変えずに scope を
//! 置き換える（人が chatgpt-rdc に `db:read` / `api:read` を足す手順）。

mod common;

use std::path::Path;
use std::process::{Command, Output};

use task_core::{McpClientStore, McpScope, SqliteStore};

fn ctl(db: &Path, args: &[&str]) -> Output {
    Command::new(common::celerisctl_path())
        .arg("--db")
        .arg(db)
        .args(args)
        .env_remove("CELERIS_DB")
        .env_remove("CELERIS_CONFIG")
        .env_remove("CELERIS_FOLLOWUPS_FILE")
        .env_remove("CELERIS_RUN_DB")
        .output()
        .unwrap_or_else(|e| panic!("spawn celerisctl: {e}"))
}

#[test]
fn scopes_replaces_the_scope_list_and_keeps_the_token() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tmp: {e}"));
    let db = dir.path().join("celeris.sqlite3");
    drop(SqliteStore::open(&db).unwrap_or_else(|e| panic!("open: {e}")));

    let out = ctl(
        &db,
        &[
            "mcp",
            "client",
            "add",
            "chatgpt-rdc",
            "--scope",
            "tasks:read",
        ],
    );
    assert!(out.status.success(), "{out:?}");
    let before = SqliteStore::open(&db)
        .and_then(|s| s.mcp_client_get("chatgpt-rdc"))
        .unwrap_or_else(|e| panic!("get: {e}"))
        .unwrap_or_else(|| panic!("client"));

    let out = ctl(
        &db,
        &[
            "mcp",
            "client",
            "scopes",
            "chatgpt-rdc",
            "--scope",
            "tasks:read,db:read,api:read",
        ],
    );
    assert!(out.status.success(), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("tasks:read,db:read,api:read"));
    let after = SqliteStore::open(&db)
        .and_then(|s| s.mcp_client_get("chatgpt-rdc"))
        .unwrap_or_else(|e| panic!("get: {e}"))
        .unwrap_or_else(|| panic!("client"));
    assert_eq!(
        after.scopes,
        vec![McpScope::TasksRead, McpScope::DbRead, McpScope::ApiRead]
    );
    assert_eq!(after.token_hash, before.token_hash);

    let out = ctl(
        &db,
        &["mcp", "client", "scopes", "nobody", "--scope", "db:read"],
    );
    assert!(!out.status.success());
    let out = ctl(
        &db,
        &[
            "mcp",
            "client",
            "scopes",
            "chatgpt-rdc",
            "--scope",
            "db:write",
        ],
    );
    assert!(!out.status.success());
}
