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
    // 本番の routing_decided: record.features.decision.routing_resolved に evidence_mode が無い。
    let quota = std::collections::BTreeMap::new();
    let decision = task_core::route_select::build_route_decision(
        &task_core::route_select::RouteDecisionInput {
            decision_id: "run:r1",
            run_id: Some("r1"),
            subject_kind: "task",
            subject_id: "t1",
            features: None,
            genre: "coding",
            harness_id: "claude_code",
            harness_version: "2.1.0",
            stage: task_core::route_select::RoutingStage::Enforce,
            lane: Some("standard"),
            model_pin: None,
            quota_remaining: &quota,
            considered: &[],
            exclusions: &[],
            evidence: &[],
            route_defaults: &[],
            output: None,
        },
    );
    let mut decision = serde_json::to_value(&decision).unwrap();
    decision["routing_resolved"] = resolved.clone();
    let legacy = [
        json!({"type":"routing_decided","run_id":"r1","record":{
            "org_node":"software-engineering","harness":"coding",
            "features":{"schema":"s","template":"t","hash":"h","decision":decision},
            "decision":{"lane":"standard","proposed":"standard","source":"default",
                "rule_id":"standard/default","policy_version":"v1",
                "features":{"judgment":"low","ambiguity":"low","verifiability":"high",
                    "reversibility":"high","consequence":"low","context_size":"medium",
                    "tool_intensity":"medium","expected_length":"medium","cross_cutting":"low"},
                "reasons":["no rule above matched"]},
            "resolution":{"lane":"standard","adapter":"claude-code","provider":"pool",
                "model_id":"m","reasoning_effort":"medium"}}}),
        json!({"type":"llm_route_selected","source_id":"claude_oauth","model_id":"m","account_id":"b",
               "metadata":{"routing_resolved":resolved}}),
        json!({"type":"worker_progress","run_id":"r1","msg":"working","routing_resolved":resolved}),
        json!({"type":"worker_finished","run_id":"r1","outcome":"done: ok","routing_resolved":resolved,
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

    // 同じ DB で /routing も読める（routing_decided の旧 shape）。
    let resp = send(
        &app,
        get_with(
            &format!("/api/v1/tasks/{}/routing", task.id),
            &[("authorization", &format!("Bearer {TOKEN}"))],
        ),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let t = send(
        &app,
        get_with(
            &format!("/api/v1/tasks/{}", task.id),
            &[("authorization", &format!("Bearer {TOKEN}"))],
        ),
    )
    .await;
    assert_eq!(t.status, 200, "{}", t.text());
}
