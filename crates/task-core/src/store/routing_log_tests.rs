//! ADR 2026-10-04-multi-objective-model-routing Phase 2（p2-log-index）: routing の相関欄（0048）。

use std::path::Path;

use rusqlite::{Connection, params};

use super::*;
use crate::migration_registry::RESERVED_VERSIONS;

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
             VALUES ('req-old', 1700000000, 'qwen', 'acc-1', 'qwen/qwen3.8-27b', 'qwen3.8-27b', 120, 'ok')",
            [],
        )
        .unwrap();
    }

    // 旧版 DB を開く（ここで 0048 が当たる）。
    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(SCHEMA_VERSION, 127);

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
            (1700000000, "qwen", "acc-1", "qwen/qwen3.8-27b")
        );
        assert_eq!(
            (upstream.as_str(), latency, status.as_str()),
            ("qwen3.8-27b", 120, "ok")
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
}

/// ADR 2026-10-11-llm-source-model-routing-evaluation D4: 0075 は旧い shadow 予約の表と index を消し、
/// 過去の `routing_shadow_recorded` event（events 表の行）は残す。
#[test]
fn migration_0075_drops_shadow_reservations_and_keeps_shadow_events() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("celeris.sqlite3");
    let task_id = crate::TaskId::new();
    {
        let mut conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
        )
        .unwrap();
        for version in (1..=74).filter(|v| !RESERVED_VERSIONS.contains(v)) {
            SqliteStore::apply_migration_version(&mut conn, version).unwrap();
        }
        conn.execute(
            "INSERT INTO routing_shadow_reservations (id, day, shadow_id, owner, state, \
             reserved_tokens, reserved_micro_usd, charged_tokens, charged_micro_usd, reserved_at) \
             VALUES ('res-old', '2026-10-05', 'shadow-old', 'test', 'timed_out', 500, 200000, \
             500, 200000, '2026-10-05T10:00:00Z')",
            [],
        )
        .unwrap();
        let json = r#"{"type":"routing_shadow_recorded","shadow_id":"shadow-old","primary_decision_id":"decision-1","run_id":"run-1","kind":"execution","status":"failed","reason":"timeout","policy_version":"p4","reservation_id":"res-old"}"#;
        conn.execute(
            "INSERT INTO events (task_id, seq, ts, json) VALUES (?1, 1, '2026-10-05T10:00:00Z', ?2)",
            params![task_id.to_string(), json],
        )
        .unwrap();
    }

    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(SCHEMA_VERSION, 127);
    {
        let conn = Connection::open(&path).unwrap();
        let left: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name IN \
                 ('routing_shadow_reservations', 'idx_routing_shadow_reservations_day')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, 0);
    }
    let events = store.events_for(task_id).unwrap();
    assert_eq!(events.len(), 1);
    let crate::Event::RoutingShadowRecorded { record } = &events[0].1 else {
        panic!("shadow event lost: {:?}", events[0].1)
    };
    assert_eq!(record.shadow_id, "shadow-old");
    assert_eq!(record.reservation_id.as_deref(), Some("res-old"));
}
