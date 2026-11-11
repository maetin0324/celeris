//! Fixtures shared by the plan_conforms tests (`cos_plan_conforms` and `cos_plan_conforms_guards`,
//! ADR 2026-10-30-cos-plan-conforms-approve): the plan gate ingested as a CoS triage item and the
//! `plan_conforms` basis body.
use super::*;
use serde_json::{Value, json};
use task_core::chat::triage::CosTriageSource;
use time::format_description::well_known::Rfc3339;

/// `hours` before now, as RFC 3339.
pub fn ago(hours: i64) -> String {
    (OffsetDateTime::now_utc() - time::Duration::hours(hours))
        .format(&Rfc3339)
        .expect("time")
}

/// The plan gate's inbox item, ingested as a triage item; returns the item row id.
pub async fn ingest_plan_gate(env: &TestEnv, app: &axum::Router, id: task_core::TaskId) -> String {
    let feed = send(app, get_admin("/api/v1/inbox/items")).await;
    assert_eq!(feed.status.as_u16(), 200, "{}", feed.text());
    let feed = feed.json();
    let inbox_id = feed["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|x| x["kind"] == "plan_gate" && x["task"]["id"] == id.to_string())
        .and_then(|x| x["id"].as_str())
        .unwrap_or_else(|| panic!("no plan gate for {id}: {feed}"))
        .to_string();
    let source = CosTriageSource {
        source_kind: "inbox".into(),
        source_key: inbox_id.clone(),
        source_revision: "r1".into(),
        source_event_id: None,
        operation_id: None,
        summary: inbox_id.clone(),
        detail: String::new(),
        policy_version: "10".into(),
        task_id: None,
    };
    env.store
        .cos_triage_ingest_batch(
            "events",
            &id.to_string(),
            &[source],
            OffsetDateTime::now_utc(),
        )
        .expect("ingest");
    super::cos_ops::db(env)
        .query_row(
            "SELECT id FROM cos_inbox_items WHERE source_key=?1",
            [inbox_id.as_str()],
            |r| r.get(0),
        )
        .expect("ingested row")
}

/// The `plan_conforms` basis for `plan_id` with its guarded stages.
pub fn conforms(plan_id: &str, guarded: Value) -> Value {
    json!({"category": "plan_conforms",
           "refs": [{"kind": "message", "id": "GET /api/v1/tasks/x/tree 2026-10-30T00:00Z"}],
           "plan": {"plan_id": plan_id, "guarded_stages": guarded}})
}
