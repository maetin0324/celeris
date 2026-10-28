//! ADR 2026-10-28-single-human-window-general-cos: the person talks only to the general CoS.
//! Through test without an LLM. As in `cos_chat_triage_scenarios.rs`, the fake harness is the test
//! itself calling the API with each CoS run's credential (claimed runs of the general and the
//! project thread); no external LLM or network:
//! the project CoS escalates → one card (札) in the general thread → the person answers in the
//! general thread → the general CoS forwards the words (`chat.relay_forward`) → the project CoS
//! answers with `instructed_by` → the source closes and the card reads as resolved.
//! Out-of-scope / expired forwards, the general CoS answering itself, and direct calls of the
//! forwards route are refused.
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

const WEB_TOKEN: &str = "web-token-value-window";
const OPERATOR_TOKEN: &str = "operator-token-value-window";
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
fn wait(env: &TestEnv, project: &str, decision_id: &str) -> (task_core::Task, String) {
    let mut task = new_task(TaskKind::Execute, Status::Ready);
    task.labels = vec!["human_required".into()];
    task.project_id = Some(project.parse().expect("project id"));
    env.seed(&task);
    raise_decision(env, &task, decision_id);
    (task, format!("decision-{decision_id}"))
}

fn run_bearer(env: &TestEnv, thread: &str, run: &str) -> String {
    let bearer = task_api::cos::issue_run_bearer(&env.store, thread, run, Duration::hours(1))
        .expect("issue");
    env.store
        .chat_run_record_policy(run, "5", "test")
        .expect("policy");
    format!("Bearer {bearer}")
}

fn finish_run(env: &TestEnv, run: &str) {
    env.store
        .chat_run_finish(
            run,
            task_core::chat::ChatRunState::Completed,
            None,
            None,
            OffsetDateTime::now_utc(),
        )
        .expect("finish");
}

fn answer_op(key: &str, inbox_id: &str, instructed_by: &str) -> Value {
    json!({
        "idempotency_key": key,
        "expected_revision": null,
        "reason": "人の答えどおり manual",
        "policy_version": "5",
        "instructed_by": instructed_by,
        "request": {"method": "POST", "path": format!("/api/v1/inbox/items/{inbox_id}/answer"),
                    "body": {"option": "manual", "note": "人の指示どおり"}},
    })
}

fn forward_op(key: &str, to_thread: &str, message: &str, tasks: &[String], expires: &str) -> Value {
    json!({
        "idempotency_key": key,
        "expected_revision": null,
        "reason": "人が統括で答えた件を案件の CoS へ取り次ぐ",
        "policy_version": "5",
        "request": {"method": "POST", "path": format!("/api/v1/chat/threads/{to_thread}/forwards"),
                    "body": {"client_message_id": format!("forward:{message}:{to_thread}"),
                             "message_id": message, "task_ids": tasks, "expires_at": expires}},
    })
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

async fn cards(app: &axum::Router, general: &str) -> Vec<Value> {
    let listed = send(
        app,
        get_with(
            &format!("/api/v1/chat/threads/{general}/messages"),
            &[("authorization", &bearer(WEB_TOKEN))],
        ),
    )
    .await;
    assert_eq!(listed.status.as_u16(), 200, "{}", listed.text());
    listed.json()["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter(|m| m.get("escalation").is_some())
        .cloned()
        .collect()
}

#[tokio::test]
async fn project_escalation_is_answered_through_the_general_cos_and_relayed_to_the_project_cos() {
    let env = env();
    let app = env.router();
    let now = OffsetDateTime::now_utc();
    let project_thread = project_cos_thread(&env.store, "pluvio");
    let project = project_thread.project_id.clone().expect("project");
    let general = env
        .store
        .cos_thread_open(
            &task_core::chat::ChatOpenThreadRequest { project_id: None },
            now,
        )
        .expect("general")
        .thread;
    let (task, inbox_id) = wait(&env, &project, "dec-window");
    let (other, _other_inbox) = wait(&env, &project, "dec-other");

    // 1. The project CoS triages the item in its own thread and escalates it to the person.
    env.store
        .cos_triage_ingest_batch(
            "decision",
            "w-1",
            &[CosTriageSource {
                source_kind: "decision".into(),
                source_key: inbox_id.clone(),
                source_revision: "r1".into(),
                source_event_id: None,
                operation_id: None,
                summary: "decision dec-window".into(),
                policy_version: "5".into(),
                task_id: Some(task.id.to_string()),
            }],
            now,
        )
        .expect("ingest");
    let claim = env
        .store
        .cos_triage_claim(&project_thread.id, "run-triage", now)
        .expect("claim")
        .expect("triage run in the project thread");
    let item = claim.item_ids[0].clone();
    let triage = run_bearer(&env, &project_thread.id, "run-triage");
    let escalate = json!({
        "idempotency_key": "esc-1",
        "expected_revision": "r1",
        "outcome": "escalate",
        "reason": "human_required の判断",
        "policy_version": "5",
        "escalation": {
            "summary": "公開先を決める",
            "options": [{"key": "vault", "label": "vault を使う"}, {"key": "manual", "label": "手で入れる"}],
            "recommended": "vault",
            "recommendation_reason": "既定の経路",
            "web_path": format!("/tasks/{}", task.id),
        },
    });
    let resolve = format!("/api/v1/cos/inbox/{item}/resolve");
    let resp = send(
        &app,
        post_json_with(&resolve, &escalate, &[("authorization", &triage)]),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    // The same resolve again (replay) adds no second card.
    let resp = send(
        &app,
        post_json_with(&resolve, &escalate, &[("authorization", &triage)]),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    finish_run(&env, "run-triage");

    // 2. One card in the general thread: project, task, what to decide, options, source thread.
    let posted = cards(&app, &general.id).await;
    assert_eq!(posted.len(), 1, "{posted:?}");
    let card = &posted[0];
    assert_eq!(card["role"], "assistant");
    assert_eq!(card["escalation"]["item_id"], json!(item));
    assert_eq!(
        card["escalation"]["source_thread_id"],
        json!(project_thread.id)
    );
    assert_eq!(card["escalation"]["task_id"], json!(task.id.to_string()));
    assert_eq!(card["escalation"]["open"], json!(true));
    assert_eq!(card["cards"][0]["kind"], "decision");
    assert_eq!(card["cards"][0]["id"], json!(inbox_id));
    let text = card["text"].as_str().expect("text");
    for part in [
        "pluvio",
        "公開先を決める",
        "vault を使う / 手で入れる",
        &task.id.to_string(),
    ] {
        assert!(text.contains(part), "{part} missing in {text}");
    }

    // 3. The person answers in the general thread; that message starts the general CoS run.
    let resp = send(
        &app,
        post_json_with(
            &format!("/api/v1/chat/threads/{}/messages", general.id),
            &json!({"client_message_id": "h-1", "text": "manual で進めて", "attachment_ids": [],
                    "reply_to_id": null, "mode": "queue", "resume_queue": false}),
            &[("authorization", &bearer(WEB_TOKEN))],
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 202, "{}", resp.text());
    let said = resp.json()["message"]["id"]
        .as_str()
        .expect("id")
        .to_owned();
    let claimed = env
        .store
        .chat_run_claim_next(
            &general.id,
            "run-general",
            &json!({}),
            OffsetDateTime::now_utc(),
        )
        .expect("claim")
        .expect("general run");
    assert_eq!(claimed.input_message_id, said);
    let general_cos = run_bearer(&env, &general.id, "run-general");
    let general_headers = [("authorization", general_cos.as_str())];

    // D4: the general CoS does not answer the project's item itself.
    let resp = send(
        &app,
        post_json_with(
            OPS,
            &answer_op("g-answer", &inbox_id, &said),
            &general_headers,
        ),
    )
    .await;
    assert_problem(&resp, 422, "cos_instruction_invalid");
    assert!(decision_answered_by(&env, &task).is_empty());
    // The forwards route is a CoS operation only: a person's or operator's token is 403.
    let direct = format!("/api/v1/chat/threads/{}/forwards", project_thread.id);
    for token in [WEB_TOKEN, OPERATOR_TOKEN] {
        let resp = send(
            &app,
            post_json_with(&direct, &json!({}), &[("authorization", &bearer(token))]),
        )
        .await;
        assert_problem(&resp, 403, "chat_forward_cos_only");
    }
    let expires = rfc3339(OffsetDateTime::now_utc() + Duration::hours(24));
    let tasks = vec![task.id.to_string()];
    // A task of another project is out of scope.
    let foreign = project_cos_thread(&env.store, "moshi");
    let resp = send(
        &app,
        post_json_with(
            OPS,
            &forward_op("f-foreign", &foreign.id, &said, &tasks, &expires),
            &general_headers,
        ),
    )
    .await;
    assert_problem(&resp, 422, "validation");

    // 4. The general CoS forwards the person's words to the project thread.
    let resp = send(
        &app,
        post_json_with(
            OPS,
            &forward_op("f-1", &project_thread.id, &said, &tasks, &expires),
            &general_headers,
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let op = resp.json()["operation"].clone();
    assert_eq!(op["state"], "applied");
    assert_eq!(op["action"], "chat.relay_forward");
    assert_eq!(op["thread_id"], json!(general.id));
    let relayed = op["result"]["message"].clone();
    let relayed_id = relayed["id"].as_str().expect("relayed id").to_owned();
    assert_eq!(relayed["thread_id"], json!(project_thread.id));
    assert_eq!(relayed["text"], "manual で進めて");
    assert_eq!(
        relayed["relay"]["relayed_by"],
        json!(format!("cos:{}", general.id))
    );
    assert_eq!(relayed["relay"]["source"]["message_id"], json!(said));
    assert_eq!(relayed["relay"]["source"]["author"], "human:web");
    finish_run(&env, "run-general");

    // 5. The forward starts the project CoS run, which answers with instructed_by.
    let claimed = env
        .store
        .chat_run_claim_next(
            &project_thread.id,
            "run-project",
            &json!({}),
            OffsetDateTime::now_utc(),
        )
        .expect("claim")
        .expect("project run");
    assert_eq!(claimed.input_message_id, relayed_id);
    let project_cos = run_bearer(&env, &project_thread.id, "run-project");
    let project_headers = [("authorization", project_cos.as_str())];
    // The project CoS cannot forward (only the general CoS does).
    let resp = send(
        &app,
        post_json_with(
            OPS,
            &forward_op("p-forward", &project_thread.id, &said, &tasks, &expires),
            &project_headers,
        ),
    )
    .await;
    assert_problem(&resp, 422, "validation");
    // Out of the forward's task scope: refused and recorded.
    let other_inbox = "decision-dec-other";
    let resp = send(
        &app,
        post_json_with(
            OPS,
            &answer_op("p-other", other_inbox, &relayed_id),
            &project_headers,
        ),
    )
    .await;
    assert_problem(&resp, 422, "cos_instruction_invalid");
    assert!(decision_answered_by(&env, &other).is_empty());
    let resp = send(
        &app,
        post_json_with(
            OPS,
            &answer_op("p-answer", &inbox_id, &relayed_id),
            &project_headers,
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let op = resp.json()["operation"].clone();
    assert_eq!(op["state"], "applied");
    assert_eq!(op["action"], "decision.answer");
    let instructed = &op["payload"]["instructed_by"];
    assert_eq!(instructed["message_id"], json!(relayed_id));
    assert_eq!(
        instructed["decided_by"],
        json!({"kind": "human", "id": "web",
               "relayed_by": {"kind": "cos_general", "id": general.id}})
    );
    assert_eq!(
        instructed["executed_by"],
        json!({"kind": "cos_project", "id": project_thread.id})
    );
    assert_eq!(decision_answered_by(&env, &task).len(), 1);

    // An expired forward (made earlier with the clock of that time) no longer grounds anything.
    let earlier = OffsetDateTime::now_utc() - Duration::hours(3);
    let old_said = env
        .store
        .chat_message_post_as(
            &general.id,
            &task_core::chat::ChatPostMessageRequest {
                client_message_id: "h-old".into(),
                text: "1 時間だけ manual で".into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: task_core::chat::ChatSendMode::Queue,
                resume_queue: false,
            },
            "human:web",
            earlier,
        )
        .expect("old message")
        .response
        .message
        .id;
    let expired = env
        .store
        .chat_relay_forward(
            &general.id,
            &project_thread.id,
            &task_core::chat::ChatForwardRequest {
                client_message_id: "forward-old".into(),
                message_id: old_said,
                task_ids: vec![other.id.to_string()],
                expires_at: rfc3339(earlier + Duration::hours(1)),
            },
            &format!("cos:{}", general.id),
            earlier + Duration::minutes(1),
        )
        .expect("expired forward")
        .response
        .message
        .id;
    let resp = send(
        &app,
        post_json_with(
            OPS,
            &answer_op("p-expired", other_inbox, &expired),
            &project_headers,
        ),
    )
    .await;
    assert_problem(&resp, 422, "cos_instruction_invalid");
    assert!(decision_answered_by(&env, &other).is_empty());

    // 6. The source wait closed: reconcile against the current derived inbox (as the dispatcher
    // does) resolves the item, and the card reads as resolved.
    let current: Vec<CosTriageSource> = task_ops::human_inbox::human_inbox(
        &env.store,
        None,
        &env.view_context(),
        OffsetDateTime::now_utc(),
        &|_, _| Vec::new(),
    )
    .expect("human inbox")
    .items
    .into_iter()
    .filter(|i| i.kind.as_str() == "decision")
    .map(|i| CosTriageSource {
        source_kind: "decision".into(),
        source_key: i.id,
        source_revision: "r1".into(),
        source_event_id: None,
        operation_id: None,
        summary: i.title,
        policy_version: "5".into(),
        task_id: None,
    })
    .collect();
    assert!(
        current.iter().all(|c| c.source_key != inbox_id),
        "answered wait closed"
    );
    assert!(
        current.iter().any(|c| c.source_key == other_inbox),
        "other wait still open"
    );
    env.store
        .cos_triage_reconcile("decision", &current, OffsetDateTime::now_utc())
        .expect("reconcile");
    let posted = cards(&app, &general.id).await;
    assert_eq!(posted.len(), 1);
    assert_eq!(posted[0]["escalation"]["open"], json!(false));
    assert_eq!(posted[0]["escalation"]["state"], "resolved");
    assert_eq!(posted[0]["cards"][0]["state"], "resolved");
}
