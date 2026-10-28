//! Flow 2: requests that need a person go to the person through the inbox and CoS. The person
//! decides; the AI executes (the project CoS through a relayed instruction, B3, or the external
//! operator itself). Decider and executor stay separate in the decision log export, the human-ops
//! report and the request ledger, and the person's decision is not replaced by the executor's.
#[path = "actor_trace/support.rs"]
mod support;
use celeris_campaign::human_ops::*;
use celeris_campaign::manifest::ConfigId;
use serde_json::{Value, json};
use std::path::PathBuf;
use support::*;
use task_core::chat::triage::CosTriageSource;
use task_core::chat::{AuditContext, ChatActor, ChatRelayRequest, ChatRunState};
use task_core::decision::*;
use task_core::{Event, SqliteStore, TaskStore};

/// Answered by the person on the web after the escalation.
const D_WEB: &str = "direction-web";
/// The person tells the operator; the operator relays it and the project CoS executes (B3).
const D_RELAY: &str = "queue-relay";
/// The person tells the operator; the operator executes with its own principal.
const D_OPERATOR: &str = "budget-operator";
const ESCALATION_RUN: &str = "actor-trace-cos-escalate";
const RELAY_RUN: &str = "actor-trace-cos-relay";
const OPERATOR: &str = "claude-code";
const PERSON: &str = "owner";

fn request_ref(decision: &str) -> String {
    format!("decision:decision-{decision}")
}

fn raise(h: &Harness, id: &str) {
    let option = |key: &str| DecisionOption {
        key: key.into(),
        label: key.into(),
        consequence: None,
    };
    h.append(
        &Event::DecisionRequested {
            decision: Box::new(DecisionRequest {
                id: id.into(),
                key: id.into(),
                kind: DecisionKind::Choice,
                question: format!("{id}: 人が決める"),
                options: vec![option("local"), option("other")],
                recommended: "local".into(),
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
            }),
        },
        1,
    );
}

/// Ingest the three human-inbox items and let one triage run claim them. Returns
/// (decision id → (CoS item id, revision)).
fn ingest_and_claim(h: &Harness) -> Vec<(String, String, String)> {
    let view = task_ops::view::ViewContext {
        workspace_roots: task_core::local_machine::WorkspaceRoots::new(h.dir.path().into(), None),
        retry_backoff_base: std::time::Duration::from_secs(1),
        retry_backoff_max: std::time::Duration::from_secs(60),
        max_requeues: 3,
        clusters: Default::default(),
    };
    let inbox =
        task_ops::human_inbox::human_inbox(&h.store, None, &view, at(2), &|_, _| vec![]).unwrap();
    let mut sources = Vec::new();
    for d in [D_WEB, D_RELAY, D_OPERATOR] {
        let item = inbox
            .items
            .iter()
            .find(|i| i.id == format!("decision-{d}"))
            .unwrap();
        sources.push(CosTriageSource {
            source_kind: item.kind.as_str().into(),
            source_key: item.id.clone(),
            source_revision: item.created_at.clone(),
            source_event_id: None,
            operation_id: None,
            summary: item.title.clone(),
            detail: String::new(),
            policy_version: POLICY.into(),
            task_id: Some(h.task.id.to_string()),
        });
    }
    h.store
        .cos_triage_ingest_batch("events", "1", &sources, at(2))
        .unwrap();
    let claim = h
        .store
        .cos_triage_claim(&h.thread, ESCALATION_RUN, at(3))
        .unwrap()
        .unwrap();
    assert_eq!(claim.item_ids.len(), 3);
    let conn = rusqlite::Connection::open(&h.db).unwrap();
    [D_WEB, D_RELAY, D_OPERATOR]
        .iter()
        .zip(&sources)
        .map(|(d, s)| {
            let id: String = conn
                .query_row(
                    "SELECT id FROM cos_inbox_items WHERE source_key=?1",
                    [&s.source_key],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(claim.item_ids.contains(&id));
            (d.to_string(), id, s.source_revision.clone())
        })
        .collect()
}

/// The person (web) or the operator (its own principal) answers outside CoS: the same plan and
/// transaction as the decision route without an audit context, at a fixed time.
fn answer_directly(h: &Harness, decision: &str, by: &str, seconds: i64) {
    let plan =
        task_ops::decision::plan_answer(&h.store, decision, Some("local"), None, by, at(seconds))
            .unwrap();
    let mut conn = rusqlite::Connection::open(&h.db).unwrap();
    let tx = conn.transaction().unwrap();
    assert!(
        SqliteStore::decision_resolve_apply_tx_at(
            &tx,
            plan.node_id,
            &plan.decision_id,
            plan.expect,
            plan.rows(),
            plan.events(),
            at(seconds),
        )
        .unwrap()
    );
    tx.commit().unwrap();
    task_ops::decision::finish_answer(&h.store, plan, at(seconds)).unwrap();
}

fn usage(reply: &Value) -> task_core::Usage {
    serde_json::from_value(reply["usage"].clone()).unwrap()
}

struct Op<'a> {
    request: &'a str,
    revision: &'a str,
    logical: String,
    kind: OpKind,
    actor: Actor,
    decided_by: Actor,
    credential: &'a str,
    start: i64,
    end: i64,
    policy_ref: Option<String>,
}

fn actor(kind: ActorKind, id: &str) -> Actor {
    Actor {
        kind,
        id: id.into(),
    }
}

/// Evaluator observations: each op as start/end, the person's ops as manual evaluator-live time.
fn observe(h: &Harness, ops: &[Op], waits: &[(&str, &str, String, i64, i64)]) -> PathBuf {
    let file = h.dir.path().join("human-ops.jsonl");
    let log = HumanOpLog::new(&file);
    for op in ops {
        let human = op.actor.kind == ActorKind::Human;
        for rt in [RecordType::OpStart, RecordType::OpEnd] {
            let mut r = human_record_base(ExperimentSet::MainAb, ConfigId::B, rt);
            r.task_id = Some(h.task.id.to_string());
            r.logical_op_id = Some(op.logical.clone());
            r.attempt = Some(1);
            if rt == RecordType::OpStart {
                r.start = Some(ts(op.start));
            } else {
                r.end = Some(ts(op.end));
                r.outcome = Some(Outcome::Ok);
            }
            r.time_basis = Some(if human {
                TimeBasis::EvaluatorLive
            } else {
                TimeBasis::HarnessTranscript
            });
            r.time_precision = Some(TimePrecision::Second);
            r.op_kind = Some(op.kind);
            r.mode = Some(if human { Mode::Manual } else { Mode::Automatic });
            r.intent = Some(Intent::Research);
            r.actor = Some(op.actor.clone());
            r.decided_by = Some(op.decided_by.clone());
            r.executed_by = Some(op.actor.clone());
            r.credential = Some(op.credential.into());
            r.request_ref = Some(op.request.into());
            r.request_revision = Some(op.revision.into());
            r.policy_ref = op.policy_ref.clone();
            if op.kind == OpKind::ManualHandoff {
                r.reason = Some("人の判断を取り次ぐ".into());
            }
            r.trigger = Some(Trigger::SystemRequest);
            log.append(&r.with_record_id()).unwrap();
        }
    }
    for (request, revision, logical, start, end) in waits {
        for rt in [RecordType::WaitStart, RecordType::WaitEnd] {
            let mut r = human_record_base(ExperimentSet::MainAb, ConfigId::B, rt);
            r.task_id = Some(h.task.id.to_string());
            r.logical_op_id = Some(logical.clone());
            r.attempt = Some(1);
            if rt == RecordType::WaitStart {
                r.start = Some(ts(*start));
            } else {
                r.end = Some(ts(*end));
            }
            r.time_basis = Some(TimeBasis::EvaluatorLive);
            r.time_precision = Some(TimePrecision::Second);
            r.wait_kind = Some("decision".into());
            r.request_ref = Some((*request).into());
            r.request_revision = Some((*revision).into());
            log.append(&r.with_record_id()).unwrap();
        }
    }
    file
}

#[tokio::test]
async fn forwarded_requests_keep_the_human_decider_apart_from_cos_and_operator_executors() {
    let h = Harness::new();
    for d in [D_WEB, D_RELAY, D_OPERATOR] {
        raise(&h, d);
    }
    let items = ingest_and_claim(&h);
    let (item_web, rev_web) = (&items[0].1, &items[0].2);
    let (item_relay, rev_relay) = (&items[1].1, &items[1].2);
    let (item_op, rev_op) = (&items[2].1, &items[2].2);

    // The triage run forwards all three to the person (escalation route = the human wait starts).
    let escalation = fake_cos_response().await;
    for item in [item_web, item_relay, item_op] {
        assert!(
            h.store
                .cos_triage_outbox_claim(item, "escalation", "人の判断が要る", at(4))
                .unwrap()
                .is_some()
        );
        assert!(
            h.store
                .cos_triage_resolve(item, "escalated", None, Some("human_required"), at(4))
                .unwrap()
        );
    }
    h.store
        .chat_run_finish_with_telemetry(
            ESCALATION_RUN,
            ChatRunState::Completed,
            Some("人へ回した"),
            None,
            at(6),
            Some(&usage(&escalation)),
            0,
            Some(at(4)),
        )
        .unwrap();

    // 1. The person answers on the web.
    answer_directly(&h, D_WEB, "human:web", 20);

    // 2. B3: the person told the operator at 30; the operator relays it at 31 (scope: this task).
    let relay = h
        .store
        .chat_relay_post(
            &h.thread,
            &ChatRelayRequest {
                client_message_id: "relay-1".into(),
                text: format!("{D_RELAY} は local で進めてよい"),
                said_at: ts(30),
                task_ids: vec![h.task.id.to_string()],
                expires_at: Some(ts(600)),
            },
            &format!("operator:{OPERATOR}"),
            at(31),
        )
        .unwrap()
        .response
        .message;
    let relay_meta = relay.relay.clone().unwrap();
    assert_eq!(relay.author.as_deref(), Some("operator:claude-code"));
    assert_eq!(relay_meta.relayed_by, "operator:claude-code");
    assert_eq!(relay_meta.said_at, "2026-10-28T10:00:30.000Z");
    let run = h
        .store
        .chat_run_claim_next(&h.thread, RELAY_RUN, &json!({}), at(32))
        .unwrap()
        .unwrap();
    assert_eq!(run.input_message_id, relay.id);
    let reply = fake_cos_response().await;
    let mut ctx = AuditContext {
        actor: ChatActor::Cos,
        thread_id: h.thread.clone(),
        run_id: RELAY_RUN.into(),
        operation_id: ulid::Ulid::new().to_string(),
        reason: REASON.into(),
        policy_version: POLICY.into(),
    };
    let inbox_item = format!("decision-{D_RELAY}");
    let task_id = h.task.id.to_string();
    // The route's D3 check at a fixed time: in scope now, refused after expiry or for another task.
    let instruction = task_api::cos::operations::inbox_answer_instruction_at(
        &h.store,
        &ctx,
        &relay.id,
        &inbox_item,
        Some(&task_id),
        at(33),
    )
    .unwrap();
    let expired = task_api::cos::operations::inbox_answer_instruction_at(
        &h.store,
        &ctx,
        &relay.id,
        &inbox_item,
        Some(&task_id),
        at(601),
    )
    .unwrap_err();
    assert!(expired.contains("expired"), "{expired}");
    let other = task_core::TaskId::new().to_string();
    assert!(
        task_api::cos::operations::inbox_answer_instruction_at(
            &h.store,
            &ctx,
            &relay.id,
            &inbox_item,
            Some(&other),
            at(33),
        )
        .is_err()
    );
    let by = &instruction.payload;
    assert_eq!(by["message_id"], relay.id);
    assert_eq!(by["decided_by"]["kind"], "human");
    assert!(by["decided_by"]["id"].is_null());
    assert_eq!(by["decided_by"]["relayed_by"]["kind"], "external_operator");
    assert_eq!(by["decided_by"]["relayed_by"]["id"], OPERATOR);
    assert_eq!(by["executed_by"]["kind"], "cos_project");
    assert_eq!(by["executed_by"]["id"], h.thread);
    assert_eq!(by["relay"]["said_at"], relay_meta.said_at);
    assert_eq!(by["relay"]["scope"]["task_ids"], json!([task_id]));
    ctx.reason = format!("{}: {REASON}", instruction.reason_prefix);
    let plan = task_ops::decision::plan_answer(
        &h.store,
        D_RELAY,
        reply["option"].as_str(),
        reply["note"].as_str(),
        &format!("cos:{}", h.thread),
        at(34),
    )
    .unwrap();
    let payload = json!({
        "method": "POST",
        "path": format!("/api/v1/inbox/items/{inbox_item}/answer"),
        "body": {"option": reply["option"]},
        "instructed_by": instruction.payload,
    });
    let op = h
        .store
        .cos_operation_apply_at(
            &ctx,
            "flow2-relay-answer",
            "fixed-reply-v1",
            "decision",
            D_RELAY,
            Some(rev_relay),
            "decision.answer",
            &payload,
            at(34),
            |tx, _| {
                assert!(SqliteStore::decision_resolve_apply_tx_at(
                    tx,
                    plan.node_id,
                    &plan.decision_id,
                    plan.expect,
                    plan.rows(),
                    plan.events(),
                    at(34)
                )?);
                Ok(json!({"decision_id": D_RELAY, "task_id": task_id}))
            },
        )
        .unwrap();
    task_ops::decision::finish_answer(&h.store, plan, at(34)).unwrap();
    assert_eq!(op.state, "applied");
    assert!(
        op.reason.contains("operator:claude-code の取り次ぎ"),
        "{}",
        op.reason
    );
    h.store
        .chat_run_finish_with_telemetry(
            RELAY_RUN,
            ChatRunState::Completed,
            Some("取り次がれた指示で local を選んだ"),
            None,
            at(36),
            Some(&usage(&reply)),
            0,
            Some(at(33)),
        )
        .unwrap();

    // 3. The person told the operator at 40; the operator executes with its own principal at 41.
    answer_directly(&h, D_OPERATOR, &format!("operator:{OPERATOR}"), 41);

    for d in [D_WEB, D_RELAY, D_OPERATOR] {
        assert_eq!(
            h.store.decision_get(d).unwrap().unwrap().status,
            DecisionStatus::Answered
        );
    }
    let answered_by: Vec<String> = h
        .store
        .events_for(h.task.id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::DecisionAnswered { by, .. } => Some(by),
            _ => None,
        })
        .collect();
    assert_eq!(
        answered_by,
        vec![
            "human:web".to_string(),
            format!("cos:{}", h.thread),
            "operator:claude-code".into()
        ]
    );

    let person = actor(ActorKind::Human, PERSON);
    let operator = actor(ActorKind::ExternalOperator, OPERATOR);
    let cos = actor(ActorKind::CosProject, &h.thread);
    let (r_web, r_relay, r_op) = (
        request_ref(D_WEB),
        request_ref(D_RELAY),
        request_ref(D_OPERATOR),
    );
    let ops = [
        Op {
            request: &r_web,
            revision: rev_web,
            logical: "person-web-answer".into(),
            kind: OpKind::ResearchDirection,
            actor: person.clone(),
            decided_by: person.clone(),
            credential: "web",
            start: 15,
            end: 20,
            policy_ref: None,
        },
        Op {
            request: &r_relay,
            revision: rev_relay,
            logical: "person-tells-operator-relay".into(),
            kind: OpKind::ResearchDirection,
            actor: person.clone(),
            decided_by: person.clone(),
            credential: "none",
            start: 28,
            end: 30,
            policy_ref: None,
        },
        Op {
            request: &r_relay,
            revision: rev_relay,
            logical: "operator-relay".into(),
            kind: OpKind::ManualHandoff,
            actor: operator.clone(),
            decided_by: person.clone(),
            credential: "operator:claude-code",
            start: 30,
            end: 31,
            policy_ref: None,
        },
        Op {
            request: &r_relay,
            revision: rev_relay,
            logical: op.id.clone(),
            kind: OpKind::ResearchDirection,
            actor: cos.clone(),
            decided_by: person.clone(),
            credential: "cos_run",
            start: 32,
            end: 34,
            policy_ref: Some(format!("instructed_by:{}", relay.id)),
        },
        Op {
            request: &r_op,
            revision: rev_op,
            logical: "person-tells-operator".into(),
            kind: OpKind::ResearchDirection,
            actor: person.clone(),
            decided_by: person.clone(),
            credential: "none",
            start: 38,
            end: 40,
            policy_ref: None,
        },
        Op {
            request: &r_op,
            revision: rev_op,
            logical: "operator-answer".into(),
            kind: OpKind::ResearchDirection,
            actor: operator.clone(),
            decided_by: person.clone(),
            credential: "operator:claude-code",
            start: 40,
            end: 41,
            policy_ref: None,
        },
    ];
    let waits = [(
        r_web.as_str(),
        rev_web.as_str(),
        "person-web-answer".to_string(),
        4,
        20,
    )];
    let observed = observe(&h, &ops, &waits);
    let out = h.outputs(&observed);

    // Export: one intervention per executor, the person's own answer stays human.
    let summary = serde_json::to_value(&out.export.summary).unwrap();
    let by_kind = &summary["interventions_by_actor_kind"];
    assert_eq!(by_kind["human"], 1, "{summary}");
    assert_eq!(by_kind["cos_project"], 1);
    assert_eq!(by_kind["external_operator"], 1);
    let ivs: Vec<(Value, Value, Value)> = out
        .export
        .records
        .iter()
        .filter(|r| r["record_type"] == "intervention")
        .map(|r| {
            (
                r["actor_kind"].clone(),
                r["actor_id"].clone(),
                r["at"].clone(),
            )
        })
        .collect();
    assert_eq!(
        ivs,
        vec![
            (json!("human"), json!("web"), json!(ts(20))),
            (json!("cos_project"), json!(h.thread), json!(ts(34))),
            (json!("external_operator"), json!(OPERATOR), json!(ts(41))),
        ]
    );
    // `celerisctl campaign export` appends these wait rows to the export.
    let waits: Vec<(Value, Value)> = h
        .store
        .decision_log_human_waits(CAMPAIGN)
        .unwrap()
        .iter()
        .map(|r| (r["end_actor_kind"].clone(), r["ended_at"].clone()))
        .collect();
    assert_eq!(
        waits,
        vec![
            (json!("human"), json!(ts(20))),
            (json!("cos_project"), json!(ts(34))),
            (json!("external_operator"), json!(ts(41))),
        ]
    );
    let usage_runs: Vec<&Value> = out
        .export
        .records
        .iter()
        .filter(|r| r["record_type"] == "usage")
        .map(|r| &r["dedup_key"])
        .collect();
    assert_eq!(
        usage_runs,
        vec![
            &json!(format!("decision:cos:{ESCALATION_RUN}:1")),
            &json!(format!("decision:cos:{RELAY_RUN}:1")),
        ]
    );

    // Request ledger: the escalated wait ends with the executor, the decider stays in policy_refs.
    let requests = out.requests["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 3);
    let row = |r: &str| requests.iter().find(|x| x["request_ref"] == r).unwrap();
    for (r, kind, id, end) in [
        (&r_web, "human", "web", 20),
        (&r_relay, "cos_project", h.thread.as_str(), 34),
        (&r_op, "external_operator", OPERATOR, 41),
    ] {
        let x = row(r);
        assert_eq!(x["resolved"]["actor_kind"], kind, "{x}");
        assert_eq!(x["resolved"]["actor_id"], id);
        assert_eq!(x["resolved"]["at"], ts(end));
        assert_eq!(x["cos_runs"], json!([ESCALATION_RUN]));
        assert_eq!(x["revisions"][0]["state"], "escalated");
        let w = &x["human_waits"];
        assert_eq!(w.as_array().unwrap().len(), 1, "{x}");
        assert_eq!(w[0]["route"], "escalation");
        assert_eq!(w[0]["started_at"], "2026-10-28T10:00:04.000Z");
        assert_eq!(w[0]["ended_at"], ts(end));
        assert_eq!(w[0]["end_actor_kind"], kind);
        assert_eq!(x["first_ai_response"]["actor_kind"], "cos_project");
    }
    assert_eq!(row(&r_web)["operations"], json!([]));
    assert_eq!(row(&r_op)["operations"], json!([]));
    let relayed = row(&r_relay);
    assert_eq!(relayed["operations"], json!([op.id]));
    let refs = relayed["policy_refs"].as_array().unwrap();
    assert!(refs.contains(&json!(format!("instructed_by:{}", relay.id))));
    assert!(refs.contains(&json!(format!("relay:external_operator:{OPERATOR}"))));
    assert!(refs.contains(&json!(format!("cos_operation_policy:{POLICY}"))));
    assert_eq!(
        out.requests["summary"]["resolved_by_actor_kind"]["human"],
        1
    );
    assert_eq!(
        out.requests["summary"]["resolved_by_actor_kind"]["cos_project"],
        1
    );
    assert_eq!(
        out.requests["summary"]["resolved_by_actor_kind"]["external_operator"],
        1
    );
    assert_eq!(out.requests["summary"]["open_human_waits"], 0);

    // All three outputs join the relayed request to the same CoS operation and executor.
    out.assert_actor_trace(&r_relay, &op.id, "cos_project", &h.thread);

    // Report: the person decided all three (counted once per request); executors are not human.
    let trials = out.report["trials"].as_array().unwrap();
    assert_eq!(trials.len(), 1, "{}", out.report);
    let trial = &trials[0];
    assert_eq!(trial["interventions"]["value"]["research_direction"], 3);
    let by_actor = &trial["interventions_by_actor_kind"]["value"];
    assert_eq!(by_actor["human"], 3, "{trial}");
    assert_eq!(by_actor["external_operator"], 2);
    assert_eq!(by_actor["cos_project"], 1);
    assert_eq!(trial["excluded_ops"]["non_human"]["external_operator"], 2);
    assert_eq!(trial["excluded_ops"]["non_human"]["cos_project"], 1);
    // Human time is the person's own ops (5 + 2 + 2 s); waiting for the person is separate.
    assert_eq!(trial["human_time"]["value"]["union_secs"], 9, "{trial}");
    assert_eq!(trial["human_wait"]["value"]["union_secs"], 16, "{trial}");
    let items = trial["intervention_items"]["value"].as_array().unwrap();
    for (r, executors) in [
        (&r_web, vec!["human"]),
        (&r_relay, vec!["human", "external_operator", "cos_project"]),
        (&r_op, vec!["human", "external_operator"]),
    ] {
        let item = items.iter().find(|i| i["request_ref"] == *r).unwrap();
        assert_eq!(item["human"], true, "{item}");
        let history = item["history"].as_array().unwrap();
        let got: Vec<&str> = history
            .iter()
            .map(|h| h["executed_by"]["kind"].as_str().unwrap())
            .collect();
        assert_eq!(got, executors, "{item}");
        for entry in history {
            assert_eq!(entry["decided_by"]["kind"], "human");
            assert_eq!(entry["decided_by"]["id"], PERSON);
            let is_person = entry["executed_by"]["kind"] == "human";
            assert_eq!(entry["counts_as_human"], is_person, "{entry}");
        }
    }
    let relayed_item = items.iter().find(|i| i["request_ref"] == r_relay).unwrap();
    let cos_entry = relayed_item["history"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["logical_op_id"] == op.id)
        .unwrap();
    assert_eq!(
        cos_entry["policy_ref"],
        relayed["policy_refs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p.as_str().unwrap().starts_with("instructed_by:"))
            .unwrap()
            .clone()
    );

    // Projection and export are idempotent: no executor turns into an extra human intervention.
    let again = h.outputs(&observed);
    assert_eq!(out.export.records_jsonl(), again.export.records_jsonl());
    assert_eq!(out.report, again.report);
    assert_eq!(out.requests, again.requests);
}
