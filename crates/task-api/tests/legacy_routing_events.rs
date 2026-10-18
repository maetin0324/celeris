//! ADR 2026-10-18-resolved-routing-evidence-mode-default: `evidence_mode` の無い旧 shape の event が
//! DB にあっても `GET /llm/accounts` は 200 で usage（stats）を返す。

mod common;

use common::*;
use serde_json::json;
use task_core::{Event, Status, TaskKind};

#[tokio::test]
async fn accounts_list_survives_legacy_routing_events_without_evidence_mode() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("claude-accounts");
    std::fs::create_dir_all(root.join("b")).unwrap();
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        accounts_root: Some(root),
        max_runs_per_account: 2,
        ..Default::default()
    });
    let task = new_task(TaskKind::Execute, Status::Done);
    env.seed_with(
        &task,
        vec![Event::WorkerStarted {
            run_id: "r1".into(),
            adapter: "claude-code".into(),
            model: "m".into(),
            provider: Some("pool".into()),
            account: Some("b".into()),
            role: None,
            task_role: None,
        }],
    );
    let resolved =
        json!({"mode":"fixed","source":"task","lower_policy":"lane-policy/1","online_update":true});
    let legacy = [
        json!({"type":"llm_route_selected","source_id":"claude_oauth","model_id":"m","account_id":"b",
               "metadata":{"routing_resolved":resolved}}),
        json!({"type":"worker_progress","run_id":"r1","msg":"working"}),
        json!({"type":"worker_finished","run_id":"r1","outcome":"done: ok",
               "usage":{"input_tokens":10,"output_tokens":4}}),
    ];
    let conn = rusqlite::Connection::open(&env.db_path).expect("open db");
    for (i, ev) in legacy.iter().enumerate() {
        conn.execute(
            "INSERT INTO events (task_id, seq, ts, json) VALUES (?1, ?2, '2026-10-17T05:15:00Z', ?3)",
            rusqlite::params![task.id.to_string(), 1000 + i as i64, ev.to_string()],
        )
        .expect("insert legacy event");
    }
    drop(conn);

    let app = env.router();
    let resp = send(
        &app,
        get_with(
            "/api/v1/llm/accounts",
            &[("authorization", &format!("Bearer {TOKEN}"))],
        ),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let v = resp.json();
    let b = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|it| it["account_id"] == "b")
        .expect("account b");
    assert_eq!(b["stats"]["runs"], json!(1));
    assert_eq!(b["stats"]["input_tokens"], json!(10));
}
