//! ADR 2026-10-27-relayed-instructions: a person's instruction relayed by an external operator
//! (`POST /chat/threads/{t}/relays`) is a `role=user` message with the author, the person's
//! time and the scope; CoS cites it as `instructed_by` from the run it starts and from a triage
//! run of the same thread, and the operation records the person as the decider (relayed by the
//! operator) and the thread's CoS as the executor. Expired or out-of-scope relays are refused
//! and recorded as rejected; a person's token cannot post a relay.
mod common;

use common::*;
use serde_json::{Value, json};
use task_api::{ApiToken, Principal};
use task_core::chat::triage::CosTriageSource;
use task_core::decision::{
    CostOfReversal, DecisionKind, DecisionOption, DecisionOrigin, DecisionPathEntry,
    DecisionRaisedBy, DecisionRequest, DecisionStatus,
};
use task_core::{Event, Status, TaskKind, TaskStore};
use time::{Duration, OffsetDateTime};

const WEB_TOKEN: &str = "web-token-value-relay";
const OPERATOR_TOKEN: &str = "operator-token-value-relay";
const OPS: &str = "/api/v1/cos/operations";

fn env() -> TestEnv {
    TestEnv::with(EnvOptions {
        extra_tokens: vec![
            ApiToken::new(Principal::Human { name: "web".into() }, WEB_TOKEN),
            ApiToken::new(
                Principal::Operator {
                    name: "claude-code".into(),
                },
                OPERATOR_TOKEN,
            ),
        ],
        ..EnvOptions::default()
    })
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

fn rfc3339(at: OffsetDateTime) -> String {
    at.format(&time::format_description::well_known::Rfc3339)
        .expect("rfc3339")
}

fn raise_decision(env: &TestEnv, task: &task_core::Task, id: &str) {
    let opt = |key: &str| DecisionOption {
        key: key.into(),
        label: key.into(),
        consequence: None,
    };
    env.store
        .append_event(
            task.id,
            &Event::DecisionRequested {
                decision: Box::new(DecisionRequest {
                    id: id.into(),
                    key: "h1".into(),
                    kind: DecisionKind::Choice,
                    question: "which?".into(),
                    options: vec![opt("vault"), opt("manual")],
                    recommended: "vault".into(),
                    cost_of_reversal: CostOfReversal::Low,
                    cost_note: None,
                    needed_before: vec!["c".into()],
                    path: vec![DecisionPathEntry {
                        task_id: task.id,
                        title: "root".into(),
                        stage: Some("s1".into()),
                        unit: None,
                    }],
                    raised_by: DecisionRaisedBy {
                        task_id: task.id,
                        run_id: None,
                        origin: DecisionOrigin::Planner,
                    },
                    status: DecisionStatus::Open,
                    answer: None,
                    withdrawn_reason: None,
                }),
            },
        )
        .expect("raise");
}

/// A `human_required` task of `project` with one open decision; returns `(task, inbox item id)`.
fn human_required_wait(
    env: &TestEnv,
    project: Option<&str>,
    decision_id: &str,
) -> (task_core::Task, String) {
    let mut task = new_task(TaskKind::Execute, Status::Ready);
    task.labels = vec!["human_required".into()];
    task.project_id = project.map(|p| p.parse().expect("project id"));
    env.seed(&task);
    raise_decision(env, &task, decision_id);
    (task, format!("decision-{decision_id}"))
}

fn relay_request(key: &str, text: &str, said_at: OffsetDateTime, task_ids: &[String]) -> Value {
    json!({
        "client_message_id": key,
        "text": text,
        "said_at": rfc3339(said_at),
        "task_ids": task_ids,
    })
}

fn answer_op(key: &str, inbox_id: &str, instructed_by: Option<&str>) -> Value {
    json!({
        "idempotency_key": key,
        "expected_revision": null,
        "reason": "manual で続ける",
        "policy_version": "4",
        "instructed_by": instructed_by,
        "request": {"method": "POST", "path": format!("/api/v1/inbox/items/{inbox_id}/answer"),
                    "body": {"option": "manual", "note": "人の指示どおり"}},
    })
}

fn run_bearer(env: &TestEnv, thread: &str, run: &str) -> String {
    let bearer = task_api::cos::issue_run_bearer(&env.store, thread, run, Duration::hours(1))
        .expect("issue");
    format!("Bearer {bearer}")
}

fn decision_answered_by(env: &TestEnv, task: &task_core::Task) -> Vec<String> {
    env.store
        .events_for(task.id)
        .expect("events")
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::DecisionAnswered { by, .. } => Some(by),
            _ => None,
        })
        .collect()
}

fn audit_states(env: &TestEnv, operation_id: &str) -> Vec<String> {
    let conn = rusqlite::Connection::open(&env.db_path).expect("open db");
    let mut stmt = conn
        .prepare("SELECT json FROM events ORDER BY seq")
        .expect("prepare");
    stmt.query_map([], |row| row.get::<_, String>(0))
        .expect("query")
        .map(|raw| serde_json::from_str::<Value>(&raw.expect("row")).expect("json"))
        .filter(|e| e["type"] == "cos_operation" && e["operation_id"] == operation_id)
        .filter_map(|e| e["state"].as_str().map(str::to_owned))
        .collect()
}

/// D1/D2/D3: the operator relays the person's words into the project thread; the run that
/// message starts answers the human_required wait with `instructed_by`, and the operation says
/// who decided (the person, relayed by the operator) and who executed (the project CoS).
#[tokio::test]
async fn operator_relay_grounds_the_project_cos_answer_and_separates_decider_from_executor() {
    let env = env();
    let app = env.router();
    let thread = project_cos_thread(&env.store, "pluvio");
    let project = thread.project_id.clone().expect("project thread");
    let (task, inbox_id) = human_required_wait(&env, Some(&project), "dec-relay");
    let said_at = OffsetDateTime::now_utc() - Duration::minutes(10);
    let operator = bearer(OPERATOR_TOKEN);
    let relays = format!("/api/v1/chat/threads/{}/relays", thread.id);

    // A person's token cannot relay: the person speaks for themselves in …/messages.
    let resp = send(
        &app,
        post_json_with(
            &relays,
            &relay_request("r-web", "続けて", said_at, &[]),
            &[("authorization", &bearer(WEB_TOKEN))],
        ),
    )
    .await;
    assert_problem(&resp, 403, "chat_relay_forbidden");

    // The operator relays the person's words with the time and the task scope.
    let text = "何台落としてもいいので続けて";
    let resp = send(
        &app,
        post_json_with(
            &relays,
            &relay_request("r-1", text, said_at, &[task.id.to_string()]),
            &[("authorization", &operator)],
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 202, "{}", resp.text());
    let message = resp.json()["message"].clone();
    let message_id = message["id"].as_str().expect("id").to_owned();
    assert_eq!(message["role"], "user");
    assert_eq!(message["text"], text);
    assert_eq!(message["author"], "operator:claude-code");
    assert_eq!(message["relay"]["relayed_by"], "operator:claude-code");
    assert_eq!(message["relay"]["scope"]["project_id"], json!(project));
    assert_eq!(
        message["relay"]["scope"]["task_ids"],
        json!([task.id.to_string()])
    );
    assert!(message["relay"]["scope"].get("expires_at").is_none());
    assert_eq!(
        message["relay"]["said_at"].as_str().expect("said_at")[..16],
        rfc3339(said_at)[..16]
    );
    // The same key with the same content is the same message; the listing shows the relay.
    let resp = send(
        &app,
        post_json_with(
            &relays,
            &relay_request("r-1", text, said_at, &[task.id.to_string()]),
            &[("authorization", &operator)],
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 202, "{}", resp.text());
    assert_eq!(resp.json()["message"]["id"], json!(message_id));
    let listed = send(
        &app,
        get_with(
            &format!("/api/v1/chat/threads/{}/messages", thread.id),
            &[("authorization", &operator)],
        ),
    )
    .await
    .json();
    assert_eq!(
        listed["items"][0]["relay"]["relayed_by"],
        "operator:claude-code"
    );

    let (foreign, foreign_inbox) = human_required_wait(&env, None, "dec-foreign");
    let resp = send(
        &app,
        post_json_with(
            &relays,
            &relay_request("foreign", text, said_at, &[foreign.id.to_string()]),
            &[("authorization", &operator)],
        ),
    )
    .await;
    assert_problem(&resp, 422, "validation");
    // Even a project-wide instruction cannot ground an answer outside its project.
    let resp = send(
        &app,
        post_json_with(
            &relays,
            &relay_request("project-wide", text, said_at, &[]),
            &[("authorization", &operator)],
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 202);
    let wide_id = resp.json()["message"]["id"]
        .as_str()
        .expect("id")
        .to_owned();

    // The relay starts a run of the project CoS; its input is the relayed message.
    let run = "run-relay";
    let claimed = env
        .store
        .chat_run_claim_next(&thread.id, run, &json!({}), OffsetDateTime::now_utc())
        .expect("claim")
        .expect("run");
    assert_eq!(claimed.input_message_id, message_id);
    let cos = run_bearer(&env, &thread.id, run);
    let headers = [("authorization", cos.as_str())];

    let resp = send(
        &app,
        post_json_with(
            OPS,
            &answer_op("foreign-answer", &foreign_inbox, Some(&wide_id)),
            &headers,
        ),
    )
    .await;
    assert_problem(&resp, 422, "cos_instruction_invalid");
    assert!(decision_answered_by(&env, &foreign).is_empty());

    // Without the instruction the human_required wait is refused as before.
    let resp = send(
        &app,
        post_json_with(OPS, &answer_op("k-none", &inbox_id, None), &headers),
    )
    .await;
    assert_problem(&resp, 403, "cos_human_required");

    // With the relayed instruction the answer applies through the decision's own path.
    let resp = send(
        &app,
        post_json_with(
            OPS,
            &answer_op("k-relay", &inbox_id, Some(&message_id)),
            &headers,
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let op = resp.json()["operation"].clone();
    assert_eq!(op["state"], "applied");
    assert_eq!(op["action"], "decision.answer");
    let instructed = &op["payload"]["instructed_by"];
    assert_eq!(instructed["message_id"], json!(message_id));
    assert_eq!(instructed["seq"], message["seq"]);
    assert_eq!(instructed["relay"]["relayed_by"], "operator:claude-code");
    assert_eq!(instructed["relay"]["said_at"], message["relay"]["said_at"]);
    assert_eq!(
        instructed["decided_by"],
        json!({"kind": "human", "id": null,
               "relayed_by": {"kind": "external_operator", "id": "claude-code"}})
    );
    assert_eq!(
        instructed["executed_by"],
        json!({"kind": "cos_project", "id": thread.id})
    );
    let reason = op["reason"].as_str().expect("reason");
    assert!(
        reason.starts_with(&format!(
            "人の指示（seq {}、operator:claude-code の取り次ぎ）: ",
            message["seq"]
        )),
        "{reason}"
    );
    assert_eq!(decision_answered_by(&env, &task).len(), 1);
    assert_eq!(
        audit_states(&env, op["id"].as_str().expect("id")),
        ["applied"]
    );
}

/// D3: a triage run (system input) of the general thread may cite a relayed instruction of that
/// thread; one scoped to another task and one that expired are refused and recorded as rejected.
#[tokio::test]
async fn triage_run_cites_a_relay_in_scope_and_refuses_out_of_scope_or_expired_ones() {
    let env = env();
    let app = env.router();
    let now = OffsetDateTime::now_utc();
    let general = env
        .store
        .cos_thread_open(
            &task_core::chat::ChatOpenThreadRequest { project_id: None },
            now,
        )
        .expect("general")
        .thread;
    let (task_a, inbox_a) = human_required_wait(&env, None, "dec-a");
    let (task_b, _inbox_b) = human_required_wait(&env, None, "dec-b");
    let operator = bearer(OPERATOR_TOKEN);
    let relays = format!("/api/v1/chat/threads/{}/relays", general.id);
    let post_relay = |key: &str, text: &str, task_ids: Vec<String>| {
        let body = relay_request(key, text, now - Duration::minutes(5), &task_ids);
        let relays = relays.clone();
        let operator = operator.clone();
        let app = app.clone();
        async move {
            let resp = send(
                &app,
                post_json_with(&relays, &body, &[("authorization", &operator)]),
            )
            .await;
            assert_eq!(resp.status.as_u16(), 202, "{}", resp.text());
            resp.json()["message"]["id"]
                .as_str()
                .expect("id")
                .to_owned()
        }
    };
    let only_b = post_relay("r-b", "B は manual で", vec![task_b.id.to_string()]).await;
    let whole = post_relay("r-all", "全部 manual で進めて", vec![]).await;
    let cancelled = post_relay("cancelled", "取り消した指示", vec![]).await;
    env.store
        .chat_message_cancel(&general.id, &cancelled, now)
        .expect("cancel");
    // An expired relay, posted earlier through the store with the injected clock of that time.
    let earlier = now - Duration::hours(2);
    let expired = env
        .store
        .chat_relay_post(
            &general.id,
            &task_core::chat::ChatRelayRequest {
                client_message_id: "r-old".into(),
                text: "1 時間だけ manual で".into(),
                said_at: rfc3339(earlier - Duration::minutes(1)),
                task_ids: vec![],
                expires_at: Some(rfc3339(earlier + Duration::hours(1))),
            },
            "operator:claude-code",
            earlier,
        )
        .expect("expired relay")
        .response
        .message
        .id;
    // Validation at the route: a future said_at, a reversed expiry, an unknown task.
    for (key, body) in [
        (
            "bad-future",
            relay_request("bad-future", "x", now + Duration::hours(1), &[]),
        ),
        (
            "bad-expiry",
            json!({"client_message_id": "bad-expiry", "text": "x",
                   "said_at": rfc3339(now - Duration::minutes(1)),
                   "expires_at": rfc3339(now - Duration::minutes(2))}),
        ),
        (
            "bad-task",
            relay_request(
                "bad-task",
                "x",
                now - Duration::minutes(1),
                &[task_core::TaskId::new().to_string()],
            ),
        ),
    ] {
        let resp = send(
            &app,
            post_json_with(&relays, &body, &[("authorization", &operator)]),
        )
        .await;
        assert_problem(&resp, 422, "validation");
        assert!(resp.text().len() > key.len());
    }

    // The triage run of the pending item has a system input, not a human message.
    env.store
        .cos_triage_ingest_batch(
            "decision",
            "batch-1",
            &[CosTriageSource {
                source_kind: "decision".into(),
                source_key: inbox_a.clone(),
                source_revision: "r1".into(),
                source_event_id: None,
                operation_id: None,
                summary: "decision dec-a".into(),
                policy_version: "4".into(),
                task_id: Some(task_a.id.to_string()),
            }],
            now,
        )
        .expect("ingest");
    let run = "run-triage";
    let claim = env
        .store
        .cos_triage_claim(&general.id, run, now)
        .expect("claim")
        .expect("triage run");
    assert_ne!(claim.message_id, whole);
    let cos = run_bearer(&env, &general.id, run);
    let headers = [("authorization", cos.as_str())];

    // Scoped to task B: does not cover task A's wait.
    let resp = send(
        &app,
        post_json_with(OPS, &answer_op("k-b", &inbox_a, Some(&only_b)), &headers),
    )
    .await;
    let problem = assert_problem(&resp, 422, "cos_instruction_invalid");
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|d| d.contains("applies to tasks")),
        "{problem}"
    );
    // Expired: refused.
    let resp = send(
        &app,
        post_json_with(OPS, &answer_op("k-old", &inbox_a, Some(&expired)), &headers),
    )
    .await;
    let problem = assert_problem(&resp, 422, "cos_instruction_invalid");
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|d| d.contains("expired")),
        "{problem}"
    );
    // The triage run's own system message is still not a human instruction.
    let resp = send(
        &app,
        post_json_with(
            OPS,
            &answer_op("k-sys", &inbox_a, Some(&claim.message_id)),
            &headers,
        ),
    )
    .await;
    assert_problem(&resp, 422, "cos_instruction_invalid");
    assert!(decision_answered_by(&env, &task_a).is_empty());
    let resp = send(
        &app,
        post_json_with(
            OPS,
            &answer_op("k-cancelled", &inbox_a, Some(&cancelled)),
            &headers,
        ),
    )
    .await;
    assert_problem(&resp, 422, "cos_instruction_invalid");
    // All refusals were recorded as rejected operations.
    let rejected: i64 = rusqlite::Connection::open(&env.db_path)
        .expect("db")
        .query_row(
            "SELECT COUNT(*) FROM cos_operations WHERE thread_id=?1 AND state='rejected'",
            [&general.id],
            |r| r.get(0),
        )
        .expect("count");
    assert_eq!(rejected, 4);

    // The relay for the whole thread grounds the triage run's answer; the executor is the
    // general CoS.
    let resp = send(
        &app,
        post_json_with(OPS, &answer_op("k-all", &inbox_a, Some(&whole)), &headers),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let op = resp.json()["operation"].clone();
    assert_eq!(op["state"], "applied");
    let instructed = &op["payload"]["instructed_by"];
    assert_eq!(instructed["message_id"], json!(whole));
    assert_eq!(instructed["decided_by"]["kind"], "human");
    assert_eq!(
        instructed["decided_by"]["relayed_by"],
        json!({"kind": "external_operator", "id": "claude-code"})
    );
    assert_eq!(
        instructed["executed_by"],
        json!({"kind": "cos_general", "id": general.id})
    );
    assert_eq!(decision_answered_by(&env, &task_a).len(), 1);
    // The live listing for the prompt has the two unexpired relays, newest first.
    let live = env
        .store
        .chat_relayed_instructions(&general.id, OffsetDateTime::now_utc())
        .expect("live");
    assert_eq!(
        live.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        [whole.as_str(), only_b.as_str()]
    );
}

/// D1: a person's own message through the API records the authenticated principal as its author
/// and carries no relay.
#[tokio::test]
async fn a_person_message_records_its_author_without_a_relay() {
    let env = env();
    let app = env.router();
    let thread = project_cos_thread(&env.store, "own");
    let resp = send(
        &app,
        post_json_with(
            &format!("/api/v1/chat/threads/{}/messages", thread.id),
            &json!({"client_message_id": "own-1", "text": "自分で決めた", "attachment_ids": [],
                    "reply_to_id": null, "mode": "queue", "resume_queue": false}),
            &[("authorization", &bearer(WEB_TOKEN))],
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 202, "{}", resp.text());
    let message = resp.json()["message"].clone();
    assert_eq!(message["author"], "human:web");
    assert!(message.get("relay").is_none(), "{message}");
}
