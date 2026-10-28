//! Flow 3: cross-project stagnation reaches the general CoS, with evidence and separate actors.
#[path = "actor_trace/support.rs"]
mod support;

use serde_json::json;
use support::*;
use task_core::chat::triage::CosTriageSource;
use task_core::chat::{AuditContext, ChatActor, ChatOpenThreadRequest, ChatRunState};
use task_core::decision::*;
use task_core::decision_log::{CampaignIdentity, DecisionLogProposal};
use task_core::{Event, Project, ProjectId, ProjectStatus, SqliteStore, TaskStore};
use task_ops::stagnation::{self, StagnationPolicy};

const POLICY3: &str = "5";

fn fixture() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("trace.db");
    let store = SqliteStore::open(&db).unwrap();
    let mut tasks = Vec::new();
    for n in 0..2 {
        let project = Project {
            id: ProjectId::new(),
            title: format!("stalled project {n}"),
            request: REASON.into(),
            status: ProjectStatus::Active,
            workspace: None,
            archived_at: None,
            paused_from: None,
            auto_advance: false,
            auto_integrate: false,
            slug: None,
            created_at: at(0),
            updated_at: at(0),
        };
        store.project_create(&project).unwrap();
        let mut task = new_task();
        task.project_id = Some(project.id);
        // insert preserves the fixed fixture clock, without a wall-clock Created event.
        store.insert(&task).unwrap();
        tasks.push(task);
    }
    let thread = store
        .cos_thread_open(&ChatOpenThreadRequest { project_id: None }, at(0))
        .unwrap()
        .thread
        .id;
    let task = tasks.remove(0);
    let identity = CampaignIdentity {
        campaign_id: CAMPAIGN.into(),
        case_id: Some("flow3".into()),
        arm: Some("B".into()),
        repetition: Some(1),
        manifest_hash: Some(HASH.into()),
        routing_setting: None,
        variant: None,
        variant_org: None,
    };
    store
        .task_campaign_set(&task.id.to_string(), &identity, at(0))
        .unwrap();
    let mut proposal = DecisionLogProposal {
        run_id: "flow3-routing".into(),
        task_id: Some(task.id.to_string()),
        project_id: task.project_id.map(|p| p.to_string()),
        ..Default::default()
    };
    proposal.apply_campaign(&identity);
    store
        .decision_log_record_proposal(&proposal, at(0))
        .unwrap();
    let h = Harness {
        dir,
        db,
        store,
        task,
        thread,
    };
    // Distinct last events prove that revisions come from durable evidence, not scan time.
    h.append(
        &Event::Edited {
            fields: vec!["priority".into()],
            by: "daemon".into(),
        },
        0,
    );
    let mut conn = rusqlite::Connection::open(&h.db).unwrap();
    let tx = conn.transaction().unwrap();
    SqliteStore::append_event_tx_at(
        &tx,
        tasks[0].id,
        &Event::Edited {
            fields: vec!["priority".into()],
            by: "daemon".into(),
        },
        at(1),
    )
    .unwrap();
    tx.commit().unwrap();
    h
}

fn detect_items(h: &Harness) -> Vec<task_ops::human_inbox::InboxItem> {
    let policy = StagnationPolicy {
        project_idle_secs: 2,
        human_wait_secs: 0,
        release_unpromoted_secs: 0,
        task_unscheduled_secs: 0,
    };
    let empty = Default::default();
    let facts = stagnation::gather(
        &h.store,
        None,
        &policy,
        at(3),
        &empty,
        &empty,
        &Default::default(),
    )
    .unwrap();
    assert_eq!(facts.projects.len(), 2);
    assert!(stagnation::detect(&facts, &policy, at(1)).is_empty());
    assert_eq!(stagnation::detect(&facts, &policy, at(2)).len(), 1);
    let findings = stagnation::detect(&facts, &policy, at(3));
    assert_eq!(findings.len(), 2);
    let view = task_ops::view::ViewContext {
        workspace_roots: task_core::local_machine::WorkspaceRoots::new(h.dir.path().into(), None),
        retry_backoff_base: std::time::Duration::from_secs(1),
        retry_backoff_max: std::time::Duration::from_secs(60),
        max_requeues: 3,
        clusters: Default::default(),
    };
    let mut inbox = task_ops::inbox::inbox(&h.store, None, &view, at(3), &|_, _| vec![]).unwrap();
    inbox.stagnation = findings.clone();
    let items = task_ops::human_inbox::from_inbox(&inbox, &Default::default(), at(3)).items;
    let items: Vec<_> = items
        .into_iter()
        .filter(|i| i.kind.as_str() == "project_stalled")
        .collect();
    assert_eq!(items.len(), 2);
    for item in &items {
        let f = findings.iter().find(|f| f.key == item.id).unwrap();
        assert_eq!(item.created_at, f.since);
        let fact = facts
            .projects
            .iter()
            .find(|p| Some(p.project_id) == f.project_id)
            .unwrap();
        assert_eq!(item.id, format!("project_stalled-{}", fact.project_id));
        assert_eq!(
            item.created_at,
            fact.last_event_at
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap()
        );
    }
    items
}

fn requested_decision(h: &Harness, evidence: &str) -> Event {
    Event::DecisionRequested {
        decision: Box::new(DecisionRequest {
            id: DECISION.into(),
            key: "stagnation-next-step".into(),
            kind: DecisionKind::Choice,
            question: format!("{evidence}: 停滞した案件の queue を選ぶ"),
            options: ["local", "other"]
                .into_iter()
                .map(|key| DecisionOption {
                    key: key.into(),
                    label: key.into(),
                    consequence: None,
                })
                .collect(),
            recommended: "local".into(),
            cost_of_reversal: CostOfReversal::Low,
            cost_note: None,
            needed_before: vec![],
            path: vec![],
            raised_by: DecisionRaisedBy {
                task_id: h.task.id,
                run_id: Some(RUN.into()),
                origin: DecisionOrigin::Planner,
            },
            status: DecisionStatus::Open,
            answer: None,
            withdrawn_reason: None,
        }),
    }
}

async fn run_flow(escalate: bool) {
    let h = fixture();
    let items = detect_items(&h);
    let sources: Vec<_> = items
        .iter()
        .map(|item| CosTriageSource {
            source_kind: item.kind.as_str().into(),
            source_key: item.id.clone(),
            source_revision: item.created_at.clone(),
            source_event_id: None,
            operation_id: None,
            summary: item.title.clone(),
            detail: String::new(),
            policy_version: POLICY3.into(),
            task_id: None,
        })
        .collect();
    h.store
        .cos_triage_ingest_batch("snapshot", "3", &sources, at(3))
        .unwrap();
    let claim = h
        .store
        .cos_triage_claim(&h.thread, RUN, at(3))
        .unwrap()
        .unwrap();
    assert_eq!(claim.item_ids.len(), 2);
    let item = items
        .iter()
        .find(|i| i.id == format!("project_stalled-{}", h.task.project_id.unwrap()))
        .unwrap();
    let claimed = claim
        .item_ids
        .iter()
        .find(|id| {
            h.store
                .cos_triage_item_report(id)
                .unwrap()
                .unwrap()
                .source_key
                == item.id
        })
        .unwrap();
    let evidence = format!("{} revision={}", item.id, item.created_at);
    let reason = format!(
        "{evidence}: {}",
        if escalate {
            "認可外の queue は人が決める"
        } else {
            REASON
        }
    );
    let reply = fake_stagnation_response(&evidence, escalate).await;
    assert_eq!(
        reply["action"],
        if escalate { "escalate" } else { "answer" }
    );
    let requested = requested_decision(&h, &evidence);
    if !escalate {
        h.append(&requested, 4);
    }
    let ctx = AuditContext {
        actor: ChatActor::Cos,
        thread_id: h.thread.clone(),
        run_id: RUN.into(),
        operation_id: ulid::Ulid::new().to_string(),
        reason: reason.clone(),
        policy_version: POLICY3.into(),
    };
    let payload = json!({"source_key": item.id, "source_revision": item.created_at, "item_id": claimed,
        "decision_id": DECISION, "option": reply["option"], "note": reason});
    let plan = (!escalate).then(|| {
        task_ops::decision::plan_answer(
            &h.store,
            DECISION,
            reply["option"].as_str(),
            Some(&reason),
            &format!("cos:{}", h.thread),
            at(5),
        )
        .unwrap()
    });
    let target = if escalate {
        claimed.clone()
    } else {
        h.task.id.to_string()
    };
    let op = h
        .store
        .cos_operation_apply_at(
            &ctx,
            "flow3-response",
            "fixed-response",
            if escalate { "cos_inbox_item" } else { "task" },
            &target,
            Some(&item.created_at),
            if escalate {
                "inbox.escalate"
            } else {
                "decision.answer"
            },
            &payload,
            at(5),
            |tx, _| {
                if escalate {
                    SqliteStore::append_event_tx_at(tx, h.task.id, &requested, at(4))?;
                }
                if let Some(plan) = &plan {
                    assert!(SqliteStore::decision_resolve_apply_tx_at(
                        tx,
                        plan.node_id,
                        &plan.decision_id,
                        plan.expect,
                        plan.rows(),
                        plan.events(),
                        at(5)
                    )?);
                }
                Ok(json!({"decision_id": DECISION, "source_key": item.id}))
            },
        )
        .unwrap();
    if let Some(plan) = plan {
        task_ops::decision::finish_answer(&h.store, plan, at(5)).unwrap();
    }
    if escalate {
        let body = json!({"item_id": claimed, "source_kind": "project_stalled", "source_key": item.id,
            "source_revision": item.created_at, "operation_id": op.id, "summary": reason,
            "options": [{"key": "inspect", "label": "案件を開いて次の手を決める"}, {"key": "pause", "label": "案件を一時停止する"}],
            "recommended": "inspect", "recommendation_reason": reason,
            "web_path": format!("/projects/{}", h.task.project_id.unwrap()), "decision_id": DECISION}).to_string();
        assert!(
            h.store
                .cos_triage_outbox_claim(claimed, "escalation", &body, at(5))
                .unwrap()
                .is_some()
        );
    }
    assert!(
        h.store
            .cos_triage_resolve(
                claimed,
                if escalate { "escalated" } else { "answered" },
                Some(&op.id),
                Some(&reason),
                at(5)
            )
            .unwrap()
    );
    let usage = serde_json::from_value(reply["usage"].clone()).unwrap();
    h.store
        .chat_run_finish_with_telemetry(
            RUN,
            ChatRunState::Completed,
            Some(&reason),
            None,
            at(8),
            Some(&usage),
            0,
            Some(at(4)),
        )
        .unwrap();
    // Leave the other project's finding open: one response must not resolve unrelated work.
    let other = claim.item_ids.iter().find(|id| *id != claimed).unwrap();
    assert_eq!(
        h.store
            .cos_triage_item_report(other)
            .unwrap()
            .unwrap()
            .state,
        "running"
    );
    let decision_source = CosTriageSource {
        source_kind: "decision".into(),
        source_key: format!("decision-{DECISION}"),
        source_revision: ts(4),
        source_event_id: None,
        operation_id: None,
        summary: reason.clone(),
        detail: String::new(),
        policy_version: POLICY3.into(),
        task_id: None,
    };
    if escalate {
        h.store.cos_triage_direct_escalate(&decision_source,
            &json!({"summary": reason, "options": [{"key": "local", "label": "local"}, {"key": "other", "label": "other"}], "recommended": null, "recommendation_reason": reason, "web_path": format!("/tasks/{}", h.task.id)}).to_string(), at(5)).unwrap();
    } else {
        h.store
            .cos_triage_ingest_batch("decisions", "4", &[decision_source], at(5))
            .unwrap();
        let id = h
            .store
            .cos_triage_open_items(None, 100)
            .unwrap()
            .into_iter()
            .find(|i| i.source_kind == "decision")
            .unwrap()
            .item_id;
        assert!(
            h.store
                .cos_triage_resolve(&id, "answered", Some(&op.id), Some(&reason), at(5))
                .unwrap()
        );
    }
    let human_log = h.dir.path().join("human-ops.jsonl");
    std::fs::write(&human_log, "").unwrap();
    let out = h.outputs(&human_log);
    let request = out.requests["requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["source_key"] == item.id)
        .unwrap();
    assert_eq!(request["operations"], json!([op.id]));
    assert_eq!(request["cos_runs"], json!([RUN]));
    assert_eq!(request["first_ai_response"]["actor_kind"], "cos_general");
    assert_eq!(request["revisions"][0]["revision"], item.created_at);
    assert_eq!(request["revisions"][0]["reason"], reason);
    assert_eq!(request["revisions"][0]["thread_kind"], "general");
    assert!(
        request["policy_refs"]
            .as_array()
            .unwrap()
            .contains(&json!("cos_operation_policy:5"))
    );
    if escalate {
        assert!(request["resolved"].is_null());
        assert_eq!(request["human_waits"][0]["route"], "escalation");
        assert!(request["human_waits"][0]["ended_at"].is_null());
        // A human answers the explicit follow-up decision; never attribute this judgment to CoS.
        let plan = task_ops::decision::plan_answer(
            &h.store,
            DECISION,
            Some("local"),
            Some(&evidence),
            "human:web",
            at(9),
        )
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
                at(9)
            )
            .unwrap()
        );
        tx.commit().unwrap();
        task_ops::decision::finish_answer(&h.store, plan, at(9)).unwrap();
    } else {
        assert_eq!(request["resolved"]["actor_kind"], "cos_general");
        assert_eq!(request["resolved"]["actor_id"], h.thread);
        assert_eq!(request["human_waits"], json!([]));
    }
    let human_log = observe(&h, &op, &item.created_at, &item.id, escalate);
    let out = h.outputs(&human_log);
    let decision = out.requests["requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["source_kind"] == "decision")
        .unwrap();
    assert_eq!(
        decision["resolved"]["actor_kind"],
        if escalate { "human" } else { "cos_general" }
    );
    let decision_actor = if escalate {
        "human:web".into()
    } else {
        format!("cos:{}", h.thread)
    };
    assert_eq!(
        decision["resolved"]["source"],
        iv_source(&h, &decision_actor)
    );
    if escalate {
        assert_eq!(decision["human_waits"][0]["end_actor_kind"], "human");
        assert_eq!(decision["human_waits"][0]["ended_at"], ts(9));
    }
    let reported_items = out.report["trials"][0]["intervention_items"]["value"]
        .as_array()
        .unwrap();
    let expected_ref = if escalate {
        format!("decision:decision-{DECISION}")
    } else {
        format!("project_stalled:{}", item.id)
    };
    let history = &reported_items
        .iter()
        .find(|r| r["request_ref"] == expected_ref)
        .unwrap()["history"][0];
    assert_eq!(
        out.report["trials"][0]["interventions_by_actor_kind"]["value"]["cos_general"],
        1
    );
    if escalate {
        assert_eq!(
            out.report["trials"][0]["interventions_by_actor_kind"]["value"]["human"],
            1
        );
    }
    assert_eq!(
        history["executed_by"]["kind"],
        if escalate { "human" } else { "cos_general" }
    );
    assert_eq!(history["counts_as_human"], escalate);
    let report_cos = reported_items
        .iter()
        .find(|r| r["request_ref"] == format!("project_stalled:{}", item.id))
        .unwrap();
    assert_eq!(report_cos["history"][0]["logical_op_id"], op.id);
    assert_eq!(report_cos["history"][0]["revision"], item.created_at);
    assert_eq!(report_cos["history"][0]["executed_by"]["id"], h.thread);
    let ivs: Vec<_> = out
        .export
        .records
        .iter()
        .filter(|r| {
            r["record_type"] == "intervention"
                && (r["actor_kind"] == "human" || r["actor_kind"] == "cos_general")
        })
        .collect();
    assert_eq!(ivs.len(), 1, "{ivs:?}");
    assert_eq!(
        ivs[0]["actor_kind"],
        if escalate { "human" } else { "cos_general" }
    );
    assert_eq!(ivs[0]["actor_id"], if escalate { "web" } else { &h.thread });
    assert_eq!(ivs[0]["source_event"], decision["resolved"]["source"]);
    if escalate {
        assert_eq!(history["logical_op_id"], ivs[0]["source_event"]);
        assert_eq!(history["revision"], decision["revisions"][0]["revision"]);
    }
    let usages: Vec<_> = out
        .export
        .records
        .iter()
        .filter(|r| r["record_type"] == "usage")
        .collect();
    assert_eq!(usages.len(), 1);
    assert_eq!(usages[0]["thread_kind"], "general");
    assert_eq!(usages[0]["thread_id"], h.thread);
    assert!(usages[0]["project_id"].is_null());
    assert_eq!(usages[0]["input_tokens"], 120);
    assert_eq!(usages[0]["list_cost_usd"], 0.002);
    let summary = serde_json::to_value(&out.export.summary).unwrap();
    assert_eq!(
        summary["shared_overhead_by_kind"]["cos:general"]["input_tokens"]["sum"],
        120.0
    );
    assert_eq!(
        summary["interventions_by_actor_kind"]["human"],
        usize::from(escalate)
    );
    if escalate {
        let conn = rusqlite::Connection::open(&h.db).unwrap();
        let body: String = conn
            .query_row(
                "SELECT body FROM notifications WHERE key=?1",
                [claimed],
                |r| r.get(0),
            )
            .unwrap();
        let packet: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(packet["decision_id"], DECISION);
        assert_eq!(packet["summary"], reason);
        assert!(packet["summary"].as_str().unwrap().contains(&evidence));
    }
    let saved = h.store.cos_operation_get(&op.id).unwrap().unwrap();
    assert_eq!(saved.reason, reason);
    assert_eq!(
        saved.expected_revision.as_deref(),
        Some(item.created_at.as_str())
    );
    assert_eq!(saved.payload, payload);
}

#[tokio::test]
async fn general_cos_responds_to_stagnation_with_source_evidence() {
    run_flow(false).await;
}

#[tokio::test]
async fn general_cos_escalates_stagnation_and_keeps_human_judgment_separate() {
    run_flow(true).await;
}

fn iv_source(h: &Harness, by: &str) -> String {
    let conn = rusqlite::Connection::open(&h.db).unwrap();
    let seq: i64 = conn.query_row("SELECT seq FROM events WHERE task_id=?1 AND json_extract(json,'$.type')='decision_answered' AND json_extract(json,'$.by')=?2",
        rusqlite::params![h.task.id.to_string(), by], |r| r.get(0)).unwrap();
    format!("event:{}:{seq}", h.task.id)
}

fn observe(
    h: &Harness,
    op: &task_core::chat::CosOperation,
    revision: &str,
    key: &str,
    human: bool,
) -> std::path::PathBuf {
    use celeris_campaign::{human_ops::*, ledger::TrialKey, manifest::ConfigId};
    // Reuse the evaluator's observation format; attach the actual flow3 receipts and actors.
    let path = record_observed_operation(h, op, revision);
    let original = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, "").unwrap();
    let log = HumanOpLog::new(&path);
    for human in if human {
        vec![false, true]
    } else {
        vec![false]
    } {
        for line in original.lines() {
            let mut r: HumanOpRecord = serde_json::from_str(line).unwrap();
            r.case_id = Some("flow3".into());
            r.trial_key = Some(
                TrialKey {
                    manifest_sha256: HASH.into(),
                    problem_id: "flow3".into(),
                    arm: ConfigId::B,
                    trial_no: 1,
                    rep: 1,
                }
                .idempotency_key(),
            );
            r.actor = Some(Actor {
                kind: if human {
                    ActorKind::Human
                } else {
                    ActorKind::CosGeneral
                },
                id: if human {
                    "web".into()
                } else {
                    h.thread.clone()
                },
            });
            r.decided_by = r.actor.clone();
            r.executed_by = r.actor.clone();
            r.request_ref = Some(if human {
                format!("decision:decision-{DECISION}")
            } else {
                format!("project_stalled:{key}")
            });
            r.request_revision = Some(if human { ts(4) } else { revision.into() });
            r.reason = Some(op.reason.clone());
            if human {
                r.logical_op_id = Some(iv_source(h, "human:web"));
                r.credential = Some("web".into());
                if r.start.is_some() {
                    r.start = Some(ts(8));
                }
                if r.end.is_some() {
                    r.end = Some(ts(9));
                }
            }
            log.append(&r.with_record_id()).unwrap();
        }
    }
    path
}

#[path = "../../../tests/support/http_async.rs"]
mod fake_http;

async fn fake_stagnation_response(evidence: &str, escalate: bool) -> serde_json::Value {
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/cos", listener.local_addr().unwrap());
    let expected = evidence.to_string();
    let server = tokio::spawn(async move {
        let (mut socket, raw) = fake_http::accept(&listener, "POST", "/cos").await;
        assert!(raw.contains(&expected));
        let body = json!({"action": if escalate { "escalate" } else { "answer" },
        "option": "local", "evidence": expected, "usage": {
            "input_tokens": 120, "output_tokens": 30, "cache_read_tokens": 40,
            "cache_creation_tokens": 10, "cost_usd": 0.002
        }})
        .to_string();
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
    });
    let reply = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(url)
        .json(&json!({"evidence": evidence}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    server.await.unwrap();
    reply
}
