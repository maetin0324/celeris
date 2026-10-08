//! ADR 2026-10-04-multi-objective-model-routing Phase 2（p2-log-index）: routing の相関欄（0048）。

use std::path::Path;

use rusqlite::{Connection, params};

use super::migrations::RESERVED_VERSIONS;
use super::*;

/// 0048 より前の版数（この版の DB を daemon が本番で持っている形）。
const OLD_VERSION: u32 = 47;

/// 0047 までを本番と同じ `apply_migration_version` で当てた DB を作る（0048 は当てない）。
fn db_at_old_version(path: &Path) {
    let mut conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
    )
    .unwrap();
    for version in (1..=OLD_VERSION).filter(|v| !RESERVED_VERSIONS.contains(v)) {
        SqliteStore::apply_migration_version(&mut conn, version).unwrap();
    }
}

#[test]
fn routing_log_correlation_migration_is_additive() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("celeris.sqlite3");
    db_at_old_version(&path);

    // 0048 より前に書かれた要求の行（llm-proxy の log.rs と同じ欄の形）。
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "INSERT INTO llm_proxy_requests (id, ts, source, account, requested_model, \
             upstream_model, latency_ms, status) \
             VALUES ('req-old', 1700000000, 'claude-oauth', 'acc-1', 'celeris/cheap', 'qwen', 120, 'ok')",
            [],
        )
        .unwrap();
    }

    // 旧版 DB を開く（ここで 0048 が当たる）。
    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(SCHEMA_VERSION, 63);

    // 既存の行と既存の欄は残る。
    {
        let conn = Connection::open(&path).unwrap();
        let (ts, source, account, requested, upstream, latency, status): (
            i64,
            String,
            String,
            String,
            String,
            i64,
            String,
        ) = conn
            .query_row(
                "SELECT ts, source, account, requested_model, upstream_model, latency_ms, status \
                 FROM llm_proxy_requests WHERE id = 'req-old'",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            (ts, source.as_str(), account.as_str(), requested.as_str()),
            (1700000000, "claude-oauth", "acc-1", "celeris/cheap")
        );
        assert_eq!(
            (upstream.as_str(), latency, status.as_str()),
            ("qwen", 120, "ok")
        );

        // 新欄は NULL のまま。
        let nulls: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_proxy_requests WHERE id = 'req-old' AND \
                 decision_id IS NULL AND snapshot_id IS NULL AND run_id IS NULL AND \
                 task_id IS NULL AND source_id IS NULL AND model IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(nulls, 1);
    }
    // account は 0022 からの既存欄なので、旧行の値が残る。
    assert_eq!(
        store.routing_correlation_get("req-old").unwrap(),
        Some(RoutingCorrelation {
            account: Some("acc-1".into()),
            ..RoutingCorrelation::default()
        })
    );

    // 相関 insert（既存行への書き込み）と select が往復する。
    let correlation = RoutingCorrelation {
        decision_id: Some("dec-1".into()),
        snapshot_id: Some("snap-1".into()),
        run_id: Some("run-1".into()),
        task_id: Some("task-1".into()),
        source_id: Some("claude-oauth:acc-1".into()),
        model: Some("claude-sonnet-5".into()),
        account: Some("acc-1".into()),
    };
    assert!(
        store
            .routing_correlation_set("req-old", &correlation)
            .unwrap()
    );
    assert_eq!(
        store.routing_correlation_get("req-old").unwrap(),
        Some(correlation)
    );
    assert_eq!(
        store.routing_request_ids_for_decision("dec-1").unwrap(),
        vec!["req-old".to_string()]
    );

    // 行が無い id は更新されず、読めば None。
    assert!(
        !store
            .routing_correlation_set("req-missing", &RoutingCorrelation::default())
            .unwrap()
    );
    assert_eq!(store.routing_correlation_get("req-missing").unwrap(), None);
}

#[test]
fn routing_decided_lookup_uses_partial_index() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = super::tests::sample_task(Status::Draft);
    store.insert(&task).unwrap();

    // task に routing_decided が無くても、索引を使う問い合わせであることを確かめる。
    assert_eq!(
        store.routing_decided_seqs(task.id).unwrap(),
        Vec::<i64>::new()
    );
    let plan_details = store
        .with_read_conn(|conn| {
            let sql = format!(
                "EXPLAIN QUERY PLAN {}",
                super::routing_log::ROUTING_DECIDED_ROWS_SQL
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(params![task.id.to_string()])?;
            let mut details = Vec::new();
            while let Some(row) = rows.next()? {
                details.push(row.get::<_, String>(3)?);
            }
            Ok(details)
        })
        .unwrap();
    assert!(
        plan_details
            .iter()
            .any(|d| d.contains("idx_events_routing_decided")),
        "query plan did not use idx_events_routing_decided: {plan_details:?}"
    );
}
