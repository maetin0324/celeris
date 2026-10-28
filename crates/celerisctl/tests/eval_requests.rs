//! CLI coverage for `eval requests`（ADR 2026-10-27-eval-request-ledger）: 一時 DB で、同じ要求の再通知と
//! 担当の再起動が 1 行のままになり、範囲外の要求が出ず、DB を書き換えないこと。
mod common;

use serde_json::Value;
use std::process::{Command, Output};
use task_core::SqliteStore;
use task_core::chat::triage::CosTriageSource;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

fn ctl(args: &[&str]) -> Output {
    Command::new(common::celerisctl_path())
        .args(args)
        .env_remove("CELERIS_DB")
        .env_remove("CELERIS_CONFIG")
        .env_remove("CELERIS_RUN_DB")
        .output()
        .expect("spawn celerisctl")
}

fn at(s: &str) -> OffsetDateTime {
    OffsetDateTime::parse(s, &Rfc3339).unwrap()
}

fn question(key: &str, revision: &str) -> CosTriageSource {
    CosTriageSource {
        source_kind: "question".into(),
        source_key: key.into(),
        source_revision: revision.into(),
        source_event_id: None,
        operation_id: None,
        summary: "q".into(),
        detail: String::new(),
        policy_version: "4".into(),
        task_id: None,
    }
}

#[test]
fn renotify_and_restart_do_not_add_rows() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("celeris.sqlite3");
    {
        let store = SqliteStore::open(&db).unwrap();
        let q = question("question-A", "2026-10-27T10:00:00Z");
        for ts in [
            "2026-10-27T10:00:01Z",
            "2026-10-27T10:00:02Z",
            "2026-10-27T10:00:03Z",
            "2026-10-27T10:00:04Z",
        ] {
            store
                .cos_triage_ingest_batch("events", ts, std::slice::from_ref(&q), at(ts))
                .unwrap();
        }
        let old = question("question-OLD", "2026-10-01T00:00:00Z");
        store
            .cos_triage_ingest_batch("events", "old", &[old], at("2026-10-01T00:00:01Z"))
            .unwrap();
        let thread = store
            .cos_triage_pending_threads()
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        store
            .cos_triage_claim(&thread, "R1", at("2026-10-27T10:01:00Z"))
            .unwrap()
            .unwrap();
        store
            .cos_triage_requeue_run("R1", at("2026-10-27T10:02:00Z"))
            .unwrap();
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute(
            "UPDATE chat_runs SET state='interrupted' WHERE run_id='R1'",
            [],
        )
        .unwrap();
        store
            .cos_triage_claim(&thread, "R2", at("2026-10-27T10:03:00Z"))
            .unwrap()
            .unwrap();
        conn.execute(
            "UPDATE chat_runs SET first_output_at='2026-10-27T10:03:30.000Z' WHERE run_id='R2'",
            [],
        )
        .unwrap();
    }
    let bytes_before = std::fs::read(&db).unwrap();
    let db_s = db.to_str().unwrap();
    let out = ctl(&[
        "--db",
        db_s,
        "eval",
        "requests",
        "--since",
        "2026-10-27T00:00:00Z",
        "--until",
        "2026-10-28T00:00:00Z",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["schema"], "celeris.eval-requests/1");
    assert_eq!(v["summary"]["requests"], 1);
    assert_eq!(v["summary"]["items"], 1);
    let row = &v["requests"][0];
    assert_eq!(row["request_ref"], "question:question-A");
    assert_eq!(
        row["revisions"][0]["run_ids"],
        serde_json::json!(["R1", "R2"])
    );
    assert_eq!(row["first_ai_response"]["at"], "2026-10-27T10:03:30.000Z");
    assert_eq!(row["first_ai_response"]["actor_kind"], "cos_general");
    // 解決・再開・評価の対応は記録が無いので null。
    assert!(row["resolved"].is_null());
    assert!(row["resumed_at"].is_null());
    assert!(row["campaign"].is_null());
    assert_eq!(
        std::fs::read(&db).unwrap(),
        bytes_before,
        "eval requests must not write the db"
    );

    let file = dir.path().join("requests.json");
    let out = ctl(&[
        "--db",
        db_s,
        "eval",
        "requests",
        "--since",
        "2026-10-27T00:00:00Z",
        "--until",
        "2026-10-28T00:00:00Z",
        "--out",
        file.to_str().unwrap(),
    ]);
    assert!(out.status.success());
    let written: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(written, v);

    let bad = ctl(&[
        "--db",
        db_s,
        "eval",
        "requests",
        "--since",
        "2026-10-28T00:00:00Z",
        "--until",
        "2026-10-27T00:00:00Z",
    ]);
    assert!(!bad.status.success());
}
