//! Flow 5: redelivery of the same request/event and a restarted CoS run duplicate neither
//! operations nor any evaluation count. The person's observed answer (wait and active work) is
//! re-sent and re-read after reopening the log and the store; the human-ops report must not move.
#[path = "actor_trace/support.rs"]
mod support;
use serde_json::{Value, json};
use support::*;
use task_core::chat::triage::CosTriageSource;
use task_core::chat::{AuditContext, ChatActor, ChatRelayRequest, ChatRunState};
use task_core::decision::*;
use task_core::{Event, SqliteStore, TaskStore};

const HUMAN_DECISION: &str = "human-budget";
const HUMAN: &str = "human:web";
const RESTARTED_RUN: &str = "actor-trace-cos-2";
/// The person's question is open from the request (6) to the answer (8): human wait 2 s.
const WAIT: (i64, i64) = (6, 8);
/// The person works on the answer from 7 to the answer (8): human active time 1 s.
const ACTIVE: (i64, i64) = (7, 8);

/// Flow 2 material: a request outside the authorization, answered by the person.
fn raise_human_decision(h: &Harness) -> DecisionRequest {
    let option = |key: &str| DecisionOption {
        key: key.into(),
        label: key.into(),
        consequence: None,
    };
    let request = DecisionRequest {
        id: HUMAN_DECISION.into(),
        key: "budget".into(),
        kind: DecisionKind::Choice,
        question: "認可の外の予算を使うか".into(),
        options: vec![option("raise"), option("keep")],
        recommended: "keep".into(),
        cost_of_reversal: CostOfReversal::High,
        cost_note: None,
        needed_before: vec![],
        path: vec![],
        raised_by: DecisionRaisedBy {
            task_id: h.task.id,
            run_id: None,
            origin: DecisionOrigin::Planner,
        },
        status: DecisionStatus::Open,
        answer: None,
        withdrawn_reason: None,
    };
    h.append(
        &Event::DecisionRequested {
            decision: Box::new(request.clone()),
        },
        WAIT.0,
    );
    request
}

fn relay(h: &Harness) -> task_core::chat::ChatMessagePosted {
    h.store
        .chat_relay_post(
            &h.thread,
            &ChatRelayRequest {
                client_message_id: "flow5-relay-1".into(),
                text: "予算は据え置く".into(),
                said_at: ts(7),
                task_ids: vec![h.task.id.to_string()],
                expires_at: None,
            },
            "operator:claude-code",
            at(7),
        )
        .unwrap()
}

/// The human-ops view of the report: counts, times, actor counts and the history of every item.
fn human_view(report: &Value) -> Value {
    let trial = &report["trials"][0];
    json!({
        "interventions": trial["interventions"],
        "by_actor_kind": trial["interventions_by_actor_kind"],
        "human_time": trial["human_time"],
        "human_wait": trial["human_wait"],
        "items": trial["intervention_items"],
        "history": trial["intervention_items"]["value"].as_array().unwrap().iter()
            .map(|i| i["history"].clone()).collect::<Vec<_>>(),
    })
}

/// Re-send the person's observation with the same record_ids and rebuild the report.
fn resend_human_answer(
    h: &Harness,
    observed: &std::path::Path,
    base: &Outputs,
    base_counts: &Value,
) {
    let lines = std::fs::read_to_string(observed).unwrap();
    let outcomes = record_human_answer(observed, h, HUMAN_DECISION, "web", WAIT, ACTIVE);
    assert_eq!(outcomes.len(), 4);
    assert!(
        outcomes
            .iter()
            .all(|o| *o == celeris_campaign::human_ops::AppendOutcome::Duplicate)
    );
    assert_eq!(std::fs::read_to_string(observed).unwrap(), lines);
    let again = h.outputs(observed);
    assert_eq!(human_view(&again.report), human_view(&base.report));
    assert_eq!(counts(h, &again), *base_counts);
    assert_eq!(again.report, base.report);
}

/// Every count the evaluation reads, taken from the real outputs and the store.
fn counts(h: &Harness, out: &Outputs) -> Value {
    let conn = rusqlite::Connection::open(&h.db).unwrap();
    let n = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    let records = |kind: &str| {
        out.export
            .records
            .iter()
            .filter(|r| r["record_type"] == kind)
            .count()
    };
    let summary = serde_json::to_value(&out.export.summary).unwrap();
    let trial = &out.report["trials"][0];
    json!({
        "cos_operations": n("SELECT COUNT(*) FROM cos_operations"),
        "cos_inbox_items": n("SELECT COUNT(*) FROM cos_inbox_items"),
        "chat_runs": n("SELECT COUNT(*) FROM chat_runs"),
        "relay_messages": n("SELECT COUNT(*) FROM chat_messages WHERE client_message_id='flow5-relay-1'"),
        "decision_answered": h.store.events_for(h.task.id).unwrap().iter()
            .filter(|(_, e)| matches!(e, Event::DecisionAnswered { .. })).count(),
        "interventions": records("intervention"),
        "interventions_by_actor_kind": summary["interventions_by_actor_kind"],
        "usage_rows": records("usage"),
        "usage_subtotal": summary["shared_overhead_by_kind"],
        "requests": out.requests["requests"].as_array().unwrap().len(),
        "report_trials": out.report["trials"].as_array().unwrap().len(),
        "report_interventions": trial["interventions"]["value"],
        "report_by_actor_kind": trial["interventions_by_actor_kind"]["value"],
        "report_items": trial["intervention_items"]["value"].as_array().map_or(0, Vec::len),
        "report_excluded": trial["excluded_ops"],
    })
}

#[tokio::test]
async fn redelivery_and_restart_duplicate_no_operation_or_count() {
    let mut h = Harness::new();
    // First delivery: flow 1 (authorized, CoS) and flow 2 material (human answer, relay).
    let (item, revision) = h.raise_and_claim();
    let reply = fake_cos_response().await;
    let op = h.answer(&item, &revision, &reply);
    raise_human_decision(&h);
    task_ops::decision::answer(
        &h.store,
        HUMAN_DECISION,
        Some("keep"),
        None,
        HUMAN,
        at(WAIT.1),
    )
    .unwrap();
    assert!(relay(&h).created);
    let observed = record_observed_operation(&h, &op, &revision);
    let outcomes = record_human_answer(&observed, &h, HUMAN_DECISION, "web", WAIT, ACTIVE);
    assert_eq!(outcomes.len(), 4);
    assert!(
        outcomes
            .iter()
            .all(|o| *o == celeris_campaign::human_ops::AppendOutcome::Appended)
    );
    let first = h.outputs(&observed);
    let before = counts(&h, &first);
    // Baseline: one human intervention with known positive active time, kept apart from the wait.
    let base = human_view(&first.report);
    assert_eq!(
        base["interventions"]["value"]["safety_approval"], 1,
        "{base}"
    );
    assert_eq!(
        base["by_actor_kind"]["value"],
        json!({"cos_project": 1, "human": 1})
    );
    assert_eq!(
        base["human_time"]["value"]["union_secs"],
        ACTIVE.1 - ACTIVE.0
    );
    assert_eq!(base["human_time"]["value"]["sum_secs"], ACTIVE.1 - ACTIVE.0);
    assert_eq!(base["human_time"]["value"]["observed"], 1);
    assert_eq!(base["human_time"]["value"]["lower_bound"], false);
    assert_eq!(base["human_wait"]["value"]["union_secs"], WAIT.1 - WAIT.0);
    assert_eq!(base["human_wait"]["value"]["lower_bound"], false);
    assert_ne!(
        base["human_wait"]["value"]["union_secs"],
        base["human_time"]["value"]["union_secs"]
    );
    let answer = first
        .export
        .records
        .iter()
        .find(|r| r["record_type"] == "intervention" && r["actor_kind"] == "human")
        .unwrap();
    // The observation's person and task are the ones of the exported answer.
    assert_eq!(answer["actor_id"], "web");
    assert_eq!(answer["task_id"], h.task.id.to_string());
    let item = base["items"]["value"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["request_ref"] == format!("decision:decision-{HUMAN_DECISION}"))
        .unwrap();
    assert_eq!(item["human"], true);
    let history = item["history"].as_array().unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(
        history[0]["executed_by"],
        json!({"kind": "human", "id": "web"})
    );
    assert_eq!(history[0]["counts_as_human"], true);
    assert_eq!(history[0]["duration_secs"], ACTIVE.1 - ACTIVE.0);
    assert_eq!(before["cos_operations"], 1, "{before}");
    assert_eq!(before["decision_answered"], 2, "{before}");
    assert_eq!(before["interventions_by_actor_kind"]["cos_project"], 1);
    assert_eq!(before["interventions_by_actor_kind"]["human"], 1);
    assert_eq!(before["usage_rows"], 1);
    assert_eq!(before["relay_messages"], 1);
    assert_eq!(before["interventions"], 2);
    assert_eq!(before["cos_inbox_items"], 1);
    assert_eq!(before["chat_runs"], 1);
    assert_eq!(before["requests"], 1, "{before}");
    assert_eq!(before["report_excluded"]["non_human"]["cos_project"], 1);
    assert_eq!(before["report_items"], 2, "{before}");

    // Redelivery of the same event: the inbox source is re-ingested with the same revision.
    let source = CosTriageSource {
        source_kind: "decision".into(),
        source_key: format!("decision-{DECISION}"),
        source_revision: revision.clone(),
        source_event_id: None,
        operation_id: None,
        summary: "認可済みの queue を選ぶ".into(),
        policy_version: POLICY.into(),
        task_id: Some(h.task.id.to_string()),
    };
    assert_eq!(
        h.store
            .cos_triage_ingest_batch("events", "1", std::slice::from_ref(&source), at(11))
            .unwrap(),
        0
    );
    assert_eq!(
        h.store
            .cos_triage_reconcile("decision", &[source], at(11))
            .unwrap(),
        0
    );
    // Same client id resend of the relayed instruction returns the same message.
    let again = relay(&h);
    assert!(!again.created);
    // Same human answer delivered twice is refused, not recorded again.
    assert!(
        task_ops::decision::answer(&h.store, HUMAN_DECISION, Some("keep"), None, HUMAN, at(12))
            .is_err()
    );
    // Evaluator's observation re-sent: same record_id, no new line.
    let resent = record_observed_operation(&h, &op, &revision);
    assert_eq!(resent, observed);
    // The person's observation re-sent with the same record_ids: the report does not move.
    resend_human_answer(&h, &observed, &first, &before);

    // Restart: the store (dispatcher side) is reopened; run-loss recovery and re-claim find nothing.
    h.store = SqliteStore::open(&h.db).unwrap();
    assert_eq!(h.store.cos_triage_requeue_run(RUN, at(13)).unwrap(), 0);
    assert!(
        h.store
            .cos_triage_claim(&h.thread, RESTARTED_RUN, at(13))
            .unwrap()
            .is_none()
    );
    // The restarted fake CoS re-executes its answer with the same idempotency key.
    let reply = fake_cos_response().await;
    let ctx = AuditContext {
        actor: ChatActor::Cos,
        thread_id: h.thread.clone(),
        run_id: RESTARTED_RUN.into(),
        operation_id: ulid::Ulid::new().to_string(),
        reason: REASON.into(),
        policy_version: POLICY.into(),
    };
    let replay = h
        .store
        .cos_operation_apply_at(
            &ctx,
            "flow1-answer",
            "fixed-reply-v1",
            "decision",
            DECISION,
            Some(&revision),
            "decision.answer",
            &reply,
            at(14),
            |_, _| panic!("an idempotent replay must not apply again"),
        )
        .unwrap();
    assert_eq!(replay.id, op.id);
    assert_eq!(replay.run_id, RUN);
    // The original run's terminal telemetry is redelivered with different numbers; it is kept as is.
    let doubled: task_core::Usage = serde_json::from_value(json!({
        "input_tokens": 240, "output_tokens": 60, "cache_read_tokens": 80,
        "cache_creation_tokens": 20, "cost_usd": 0.004
    }))
    .unwrap();
    h.store
        .chat_run_finish_with_telemetry(
            RUN,
            ChatRunState::Completed,
            Some("local queue を選んだ"),
            None,
            at(15),
            Some(&doubled),
            0,
            Some(at(14)),
        )
        .unwrap();

    // After reopening the log and the store, the person's observation is re-sent once more.
    resend_human_answer(&h, &observed, &first, &before);

    // Projections and exports run again after the restart.
    let second = h.outputs(&observed);
    let after = counts(&h, &second);
    assert_eq!(before, after);
    assert_eq!(first.export.records_jsonl(), second.export.records_jsonl());
    assert_eq!(first.report, second.report);
    assert_eq!(first.requests, second.requests);
    let summary = serde_json::to_value(&second.export.summary).unwrap();
    assert_eq!(
        summary["shared_overhead_by_kind"]["cos:project"]["input_tokens"]["sum"],
        120.0
    );
    second.assert_actor_trace(
        &format!("decision:decision-{DECISION}"),
        &op.id,
        "cos_project",
        &h.thread,
    );
}
