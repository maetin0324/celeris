//! Flow 1: an authorized project CoS decision remains AI work in all evaluation outputs.
#[path = "actor_trace/support.rs"]
mod support;
use serde_json::{Value, json};
use support::*;
use task_core::{Event, TaskStore};

#[tokio::test]
async fn authorized_project_cos_keeps_actor_and_consumption_through_evaluation() {
    let h = Harness::new();
    let (item, revision) = h.raise_and_claim();
    let reply = fake_cos_response().await;
    let op = h.answer(&item, &revision, &reply);
    assert_eq!(op.state, "applied");
    assert_eq!(op.thread_id, h.thread);
    assert_eq!(op.run_id, RUN);
    assert_eq!(op.reason, REASON);
    assert_eq!(op.policy_version, POLICY);
    assert_eq!(
        h.store.decision_get(DECISION).unwrap().unwrap().status,
        task_core::decision::DecisionStatus::Answered
    );
    let events = h.store.events_for(h.task.id).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|(_, e)| matches!(e, Event::DecisionAnswered { by, .. }
        if by == &format!("cos:{}", h.thread)))
            .count(),
        1
    );
    let conn = rusqlite::Connection::open(&h.db).unwrap();
    let audit: String = conn
        .query_row(
            "SELECT json FROM events WHERE json_extract(json,'$.operation_id')=?1",
            [&op.id],
            |r| r.get(0),
        )
        .unwrap();
    let audit: Value = serde_json::from_str(&audit).unwrap();
    assert_eq!(audit["actor"], "cos");
    assert_eq!(audit["reason"], REASON);
    assert_eq!(audit["run_id"], RUN);
    assert_eq!(audit["thread_id"], h.thread);
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM notifications", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let observed = record_observed_operation(&h, &op, &revision);
    let out = h.outputs(&observed);
    let summary = serde_json::to_value(&out.export.summary).unwrap();
    assert_eq!(summary["interventions_by_actor_kind"]["human"], 0);
    assert_eq!(summary["interventions_by_actor_kind"]["cos_project"], 1);
    let ivs: Vec<_> = out
        .export
        .records
        .iter()
        .filter(|r| r["record_type"] == "intervention")
        .collect();
    assert_eq!(ivs.len(), 1);
    assert_eq!(ivs[0]["actor_kind"], "cos_project");
    assert_eq!(ivs[0]["actor_id"], h.thread);
    assert_eq!(ivs[0]["at"], ts(5));
    let usages: Vec<_> = out
        .export
        .records
        .iter()
        .filter(|r| r["record_type"] == "usage")
        .collect();
    assert_eq!(usages.len(), 1);
    let u = usages[0];
    assert_eq!(u["thread_id"], h.thread);
    assert_eq!(u["thread_kind"], "project");
    assert_eq!(u["project_id"], h.task.project_id.unwrap().to_string());
    assert_eq!(u["dedup_key"], format!("decision:cos:{RUN}:1"));
    for (k, v) in [
        ("input_tokens", 120),
        ("output_tokens", 30),
        ("cache_read_tokens", 40),
        ("cache_creation_tokens", 10),
        ("wall_ms", 5000),
    ] {
        assert_eq!(u[k], v, "{k}: {u}");
    }
    assert_eq!(u["list_cost_usd"], 0.002);
    assert!(u["cash_cost_usd"].is_null());
    assert_eq!(
        summary["shared_overhead_by_kind"]["cos:project"]["input_tokens"]["sum"],
        120.0
    );
    assert_eq!(summary["total"]["n_rows"], 0);
    let requests = out.requests["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(
        request["request_ref"],
        format!("decision:decision-{DECISION}")
    );
    assert_eq!(request["operations"], json!([op.id]));
    assert_eq!(request["cos_runs"], json!([RUN]));
    assert_eq!(request["human_waits"], json!([]));
    assert_eq!(request["resolved"]["actor_kind"], ivs[0]["actor_kind"]);
    assert_eq!(request["resolved"]["actor_id"], ivs[0]["actor_id"]);
    assert_eq!(request["resolved"]["at"], ts(5));
    assert_eq!(request["first_ai_response"]["actor_kind"], "cos_project");
    assert_eq!(request["revisions"][0]["state"], "answered");
    let trials = out.report["trials"].as_array().unwrap();
    assert_eq!(trials.len(), 1, "{}", out.report);
    let trial = &trials[0];
    assert_eq!(trial["interventions"]["source"], "evaluator");
    let counts = trial["interventions"]["value"].as_object().unwrap();
    assert!(counts.values().all(|v| v.as_u64() == Some(0)), "{trial}");
    assert_eq!(
        trial["interventions_by_actor_kind"]["value"]["cos_project"],
        1
    );
    assert_eq!(trial["excluded_ops"]["non_human"]["cos_project"], 1);
    assert_eq!(trial["human_time"]["value"]["union_secs"], 0);
    let report_items = trial["intervention_items"]["value"].as_array().unwrap();
    assert_eq!(report_items.len(), 1);
    assert_eq!(report_items[0]["request_ref"], request["request_ref"]);
    let history = &report_items[0]["history"][0];
    assert_eq!(history["logical_op_id"], op.id);
    assert_eq!(history["revision"], revision);
    assert_eq!(
        history["executed_by"]["kind"],
        request["resolved"]["actor_kind"]
    );
    assert_eq!(history["executed_by"]["id"], h.thread);
    assert_eq!(history["counts_as_human"], false);
    assert_eq!(
        conn.query_row(
            "SELECT created_at FROM cos_operations WHERE id=?1",
            [&op.id],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "2026-10-28T10:00:05.000Z"
    );
    assert_eq!(
        conn.query_row(
            "SELECT ts FROM events WHERE json_extract(json,'$.operation_id')=?1",
            [&op.id],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        ts(5)
    );
    // A second read/projection must not manufacture extra usage or interventions.
    let again = h.outputs(&observed);
    assert_eq!(out.export.records_jsonl(), again.export.records_jsonl());
    assert_eq!(out.report, again.report);
    assert_eq!(out.requests, again.requests);
}
