//! ADR 2026-10-05 D3: `GET /api/v1/cos/inbox` and `POST /api/v1/cos/inbox/{i}/resolve`.
//! answer runs through the shared CoS operation layer, a stale revision is 409, explicit
//! human_required is refused on resolve and on `/cos/operations` alike, observe of a wait that needs
//! a judgment is 422, the escalation packet and its web_path are validated, and escalate claims one
//! durable outbox row. Item times come from an injected clock; nothing sleeps.
mod common;

use common::*;
use serde_json::{Value, json};
use task_core::chat::triage::CosTriageSource;
use task_core::chat::{ChatCreateThreadRequest, ChatPostMessageRequest, ChatSendMode};
use task_core::decision::{
    CostOfReversal, DecisionKind, DecisionOption, DecisionOrigin, DecisionPathEntry,
    DecisionRaisedBy, DecisionRequest, DecisionStatus,
};
use task_core::{Event, Status, TaskKind, TaskStore};
use time::{Duration, OffsetDateTime};

const OPS: &str = "/api/v1/cos/operations";

/// The injected clock of the ingest side: `T0 + secs`.
fn at(secs: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_790_000_000 + secs).expect("time")
}

/// A live CoS run of a new thread: `(thread_id, run_id, "Bearer …")`.
fn cos_bearer(env: &TestEnv, key: &str) -> (String, String, String) {
    let store = &env.store;
    let now = OffsetDateTime::now_utc();
    let thread = store
        .chat_thread_create(
            "admin",
            &ChatCreateThreadRequest {
                title: format!("thread {key}"),
                project_id: None,
                client_thread_id: key.into(),
            },
            now,
        )
        .expect("thread")
        .thread;
    store
        .chat_message_post(
            &thread.id,
            &ChatPostMessageRequest {
                client_message_id: format!("message-{key}"),
                text: "input".into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
            },
            now,
        )
        .expect("message");
    let run = format!("run-{key}");
    store
        .chat_run_claim_next(&thread.id, &run, &json!({}), now)
        .expect("claim")
        .expect("run");
    let bearer = task_api::cos::issue_run_bearer(store, &thread.id, &run, Duration::hours(1))
        .expect("issue");
    (thread.id, run, format!("Bearer {bearer}"))
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

/// A task with one open decision; returns `(task, derived inbox item id)`.
fn decision_wait(env: &TestEnv, decision_id: &str, labels: &[&str]) -> (task_core::Task, String) {
    let mut task = new_task(TaskKind::Execute, Status::Ready);
    task.labels = labels.iter().map(|l| l.to_string()).collect();
    env.seed(&task);
    raise_decision(env, &task, decision_id);
    (task, format!("decision-{decision_id}"))
}

/// Ingest one triage item and return its row id.
fn ingest(env: &TestEnv, kind: &str, key: &str, revision: &str, secs: i64) -> String {
    let source = CosTriageSource {
        source_kind: kind.into(),
        source_key: key.into(),
        source_revision: revision.into(),
        source_event_id: None,
        operation_id: None,
        summary: format!("{kind} {key}"),
        policy_version: "1".into(),
    };
    env.store
        .cos_triage_ingest_batch(kind, &format!("{secs}"), &[source], at(secs))
        .expect("ingest");
    db(env)
        .query_row(
            "SELECT id FROM cos_inbox_items WHERE source_kind=?1 AND source_key=?2 AND source_revision=?3",
            [kind, key, revision],
            |r| r.get(0),
        )
        .expect("ingested row")
}

fn resolve_path(item: &str) -> String {
    format!("/api/v1/cos/inbox/{item}/resolve")
}

fn resolve_body(key: &str, revision: &str, outcome: &str, extra: Value) -> Value {
    let mut body = json!({
        "idempotency_key": key,
        "expected_revision": revision,
        "outcome": outcome,
        "reason": "既存の指示の範囲内の定型選択",
        "policy_version": "1",
    });
    if let (Some(object), Some(more)) = (body.as_object_mut(), extra.as_object()) {
        for (k, v) in more {
            object.insert(k.clone(), v.clone());
        }
    }
    body
}

fn packet(options: Value, recommended: Value, web_path: &str) -> Value {
    json!({
        "summary": "公開前の確認",
        "options": options,
        "recommended": recommended,
        "recommendation_reason": "公開先が未確認",
        "web_path": web_path,
    })
}

fn db(env: &TestEnv) -> rusqlite::Connection {
    rusqlite::Connection::open(&env.db_path).expect("open db")
}

fn item_state(env: &TestEnv, item: &str) -> String {
    db(env)
        .query_row(
            "SELECT state FROM cos_inbox_items WHERE id=?1",
            [item],
            |r| r.get(0),
        )
        .expect("item")
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

fn audit_events(env: &TestEnv, operation_id: &str) -> Vec<Value> {
    let conn = db(env);
    let mut stmt = conn
        .prepare("SELECT json FROM events ORDER BY seq")
        .expect("prepare");
    stmt.query_map([], |row| row.get::<_, String>(0))
        .expect("query")
        .map(|raw| serde_json::from_str::<Value>(&raw.expect("row")).expect("json"))
        .filter(|e| e["type"] == "cos_operation" && e["operation_id"] == operation_id)
        .collect()
}

#[tokio::test]
async fn cos_chat_triage_resolve_answer_runs_the_shared_operation() {
    let env = admin_env();
    let app = env.router();
    let (task, inbox_id) = decision_wait(&env, "dec-a", &[]);
    let item = ingest(&env, "inbox", &inbox_id, "r1", 0);
    let (thread, run, bearer) = cos_bearer(&env, "answer");
    let headers = [("authorization", bearer.as_str())];

    let resp = send(
        &app,
        post_json_with(
            &resolve_path(&item),
            &resolve_body(
                "k-answer",
                "r1",
                "answer",
                json!({"answer": {"option": "vault", "note": "既定どおり"}}),
            ),
            &headers,
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let out = resp.json();
    let op = &out["operation"];
    assert_eq!(op["state"], "applied", "{op}");
    assert_eq!(op["action"], "decision.answer");
    assert_eq!(op["actor"], "cos");
    assert_eq!(op["thread_id"], thread.as_str());
    assert_eq!(op["run_id"], run.as_str());
    assert!(
        op["event_id"].is_string(),
        "the chat card was written: {op}"
    );
    assert_eq!(out["item"]["state"], "answered");
    assert_eq!(out["item"]["operation_id"], op["id"]);
    assert_eq!(item_state(&env, &item), "answered");
    assert_eq!(decision_answered_by(&env, &task), vec!["cos".to_string()]);

    // Criterion 1: actor=cos and the reason are in the audit envelope event.
    let op_id = op["id"].as_str().expect("op id");
    let audits = audit_events(&env, op_id);
    assert_eq!(audits.len(), 1, "{audits:?}");
    let envelope = serde_json::to_string(&audits[0]).expect("json");
    assert!(envelope.contains("\"cos\""), "{envelope}");
    assert!(
        envelope.contains("既存の指示の範囲内の定型選択"),
        "{envelope}"
    );

    // A replay of the same idempotency key returns the same operation, no second answer.
    let again = send(
        &app,
        post_json_with(
            &resolve_path(&item),
            &resolve_body(
                "k-answer",
                "r1",
                "answer",
                json!({"answer": {"option": "vault", "note": "既定どおり"}}),
            ),
            &headers,
        ),
    )
    .await;
    assert_eq!(again.status.as_u16(), 200, "{}", again.text());
    assert_eq!(again.json()["operation"]["id"], op["id"]);
    assert_eq!(decision_answered_by(&env, &task).len(), 1);

    // GET /cos/inbox shows the item with its state.
    let list = send(&app, get_admin("/api/v1/cos/inbox?state=answered")).await;
    assert_eq!(list.status.as_u16(), 200, "{}", list.text());
    let items = list.json()["items"].as_array().expect("items").clone();
    assert!(items.iter().any(|i| i["id"] == item.as_str()), "{items:?}");
}

#[tokio::test]
async fn cos_chat_triage_resolve_rejects_human_credential() {
    let env = admin_env();
    let app = env.router();
    let (_task, inbox_id) = decision_wait(&env, "dec-h", &[]);
    let item = ingest(&env, "inbox", &inbox_id, "r1", 0);
    let body = resolve_body(
        "k-human",
        "r1",
        "answer",
        json!({"answer": {"option": "vault"}}),
    );
    let resp = send(&app, post_admin(&resolve_path(&item), &body)).await;
    assert_problem(&resp, 403, "cos_credential_required");
    assert_eq!(item_state(&env, &item), "pending");

    // The body cannot claim the actor either.
    let (_t, _r, bearer) = cos_bearer(&env, "claim");
    let mut claimed = body.clone();
    claimed["actor"] = json!("human");
    let resp = send(
        &app,
        post_json_with(
            &resolve_path(&item),
            &claimed,
            &[("authorization", bearer.as_str())],
        ),
    )
    .await;
    assert_problem(&resp, 422, "cos_identity_claim");
}

#[tokio::test]
async fn cos_chat_triage_resolve_conflicts_on_stale_revision() {
    let env = admin_env();
    let app = env.router();
    let (task, inbox_id) = decision_wait(&env, "dec-r", &[]);
    let item = ingest(&env, "inbox", &inbox_id, "r1", 0);
    let (_t, _r, bearer) = cos_bearer(&env, "stale");
    let headers = [("authorization", bearer.as_str())];
    let answer = json!({"answer": {"option": "vault"}});

    // Wrong expected revision.
    let resp = send(
        &app,
        post_json_with(
            &resolve_path(&item),
            &resolve_body("k-1", "r0", "answer", answer.clone()),
            &headers,
        ),
    )
    .await;
    assert_problem(&resp, 409, "cos_inbox_revision_conflict");

    // A newer revision of the same source was ingested: the old row is stale.
    let newer = ingest(&env, "inbox", &inbox_id, "r2", 10);
    let resp = send(
        &app,
        post_json_with(
            &resolve_path(&item),
            &resolve_body("k-2", "r1", "answer", answer.clone()),
            &headers,
        ),
    )
    .await;
    assert_problem(&resp, 409, "cos_inbox_revision_conflict");

    // The human answers first through the normal inbox: CoS does not overwrite it.
    let human = send(
        &app,
        post_admin(
            &format!("/api/v1/inbox/items/{inbox_id}/answer"),
            &json!({"option": "manual"}),
        ),
    )
    .await;
    assert_eq!(human.status.as_u16(), 200, "{}", human.text());
    let resp = send(
        &app,
        post_json_with(
            &resolve_path(&newer),
            &resolve_body("k-3", "r2", "answer", answer),
            &headers,
        ),
    )
    .await;
    assert_problem(&resp, 409, "cos_inbox_revision_conflict");
    assert_eq!(decision_answered_by(&env, &task), vec!["human".to_string()]);
    assert_eq!(item_state(&env, &newer), "pending");
}

#[tokio::test]
async fn cos_chat_triage_human_required_refused_on_resolve_and_operations() {
    let env = admin_env();
    let app = env.router();
    let (task, inbox_id) = decision_wait(&env, "dec-x", &["human_required"]);
    let item = ingest(&env, "inbox", &inbox_id, "r1", 0);
    let (thread, _r, bearer) = cos_bearer(&env, "human-required");
    let headers = [("authorization", bearer.as_str())];

    let resp = send(
        &app,
        post_json_with(
            &resolve_path(&item),
            &resolve_body(
                "k-hr",
                "r1",
                "answer",
                json!({"answer": {"option": "vault"}}),
            ),
            &headers,
        ),
    )
    .await;
    assert_problem(&resp, 403, "cos_human_required");
    assert_eq!(item_state(&env, &item), "pending");

    // The same answer through the general operation layer is refused the same way.
    let resp = send(
        &app,
        post_json_with(
            OPS,
            &json!({
                "idempotency_key": "k-hr-op",
                "expected_revision": null,
                "reason": "定型",
                "policy_version": "1",
                "request": {"method": "POST", "path": "/api/v1/decisions/dec-x/answer", "body": {"option": "vault"}},
            }),
            &headers,
        ),
    )
    .await;
    assert_problem(&resp, 403, "cos_human_required");
    assert!(decision_answered_by(&env, &task).is_empty());

    // Both refusals are recorded as rejected operations with their reason.
    let conn = db(&env);
    let rejected: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cos_operations WHERE thread_id=?1 AND state='rejected' AND reason LIKE '%human_required%'",
            [&thread],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(rejected, 2);

    // Escalating it is allowed.
    let resp = send(
        &app,
        post_json_with(
            &resolve_path(&item),
            &resolve_body(
                "k-hr-esc",
                "r1",
                "escalate",
                json!({"escalation": packet(
                    json!([{"key": "vault", "label": "vault"}, {"key": "manual", "label": "manual"}]),
                    json!("vault"),
                    &format!("/tasks/{}", task.id),
                )}),
            ),
            &headers,
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
}

#[tokio::test]
async fn cos_chat_triage_observe_needs_no_judgment() {
    let env = admin_env();
    let app = env.router();
    let (_task, inbox_id) = decision_wait(&env, "dec-o", &[]);
    let open = ingest(&env, "inbox", &inbox_id, "r1", 0);
    let notice = ingest(&env, "notice", "notice-1", "v1", 1);
    let (_t, _r, bearer) = cos_bearer(&env, "observe");
    let headers = [("authorization", bearer.as_str())];

    let resp = send(
        &app,
        post_json_with(
            &resolve_path(&open),
            &resolve_body("k-o1", "r1", "observe", json!({})),
            &headers,
        ),
    )
    .await;
    assert_problem(&resp, 422, "cos_observe_needs_judgment");
    assert_eq!(item_state(&env, &open), "pending");

    // observe with an escalation packet is 422.
    let resp = send(
        &app,
        post_json_with(
            &resolve_path(&notice),
            &resolve_body(
                "k-o2",
                "v1",
                "observe",
                json!({"escalation": packet(json!([{"key": "reply", "label": "web で回答"}]), json!(null), "/notifications")}),
            ),
            &headers,
        ),
    )
    .await;
    assert_problem(&resp, 422, "validation");

    let resp = send(
        &app,
        post_json_with(
            &resolve_path(&notice),
            &resolve_body("k-o3", "v1", "observe", json!({})),
            &headers,
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(resp.json()["operation"]["action"], "inbox.observe");
    assert_eq!(item_state(&env, &notice), "observed");
    let outbox: i64 = db(&env)
        .query_row(
            "SELECT COUNT(*) FROM notifications WHERE kind IN ('cos_escalation','cos_fallback')",
            [],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(outbox, 0, "observe sends nothing");
}

#[tokio::test]
async fn cos_chat_triage_escalation_packet_is_validated() {
    let env = admin_env();
    let app = env.router();
    let (task, inbox_id) = decision_wait(&env, "dec-p", &[]);
    let item = ingest(&env, "inbox", &inbox_id, "r1", 0);
    let (_t, _r, bearer) = cos_bearer(&env, "packet");
    let headers = [("authorization", bearer.as_str())];
    let web = format!("/tasks/{}", task.id);
    let both = json!([{"key": "vault", "label": "vault"}, {"key": "manual", "label": "manual"}]);
    let cases = [
        ("missing", json!({}), "cos_escalation_invalid"),
        (
            "recommended-not-an-option",
            json!({"escalation": packet(both.clone(), json!("other"), &web)}),
            "cos_escalation_invalid",
        ),
        (
            "unknown-option",
            json!({"escalation": packet(json!([{"key": "delete", "label": "消す"}]), json!(null), &web)}),
            "cos_escalation_invalid",
        ),
        (
            "reply-on-a-choice",
            json!({"escalation": packet(json!([{"key": "reply", "label": "web で回答"}]), json!(null), &web)}),
            "cos_escalation_invalid",
        ),
        (
            "no-options",
            json!({"escalation": packet(json!([]), json!(null), &web)}),
            "cos_escalation_invalid",
        ),
        (
            "answer-and-escalation",
            json!({"answer": {"option": "vault"}, "escalation": packet(both.clone(), json!("vault"), &web)}),
            "validation",
        ),
    ];
    for (key, extra, code) in cases {
        let resp = send(
            &app,
            post_json_with(
                &resolve_path(&item),
                &resolve_body(&format!("k-{key}"), "r1", "escalate", extra),
                &headers,
            ),
        )
        .await;
        assert_problem(&resp, 422, code);
    }
    // escalation must be null for answer as well.
    let resp = send(
        &app,
        post_json_with(
            &resolve_path(&item),
            &resolve_body(
                "k-answer-esc",
                "r1",
                "answer",
                json!({"answer": {"option": "vault"}, "escalation": packet(both.clone(), json!("vault"), &web)}),
            ),
            &headers,
        ),
    )
    .await;
    assert_problem(&resp, 422, "validation");
    // Empty recommendation reason, even with recommended=null.
    let mut no_reason = packet(both, json!(null), &web);
    no_reason["recommendation_reason"] = json!(" ");
    let resp = send(
        &app,
        post_json_with(
            &resolve_path(&item),
            &resolve_body(
                "k-no-reason",
                "r1",
                "escalate",
                json!({"escalation": no_reason}),
            ),
            &headers,
        ),
    )
    .await;
    assert_problem(&resp, 422, "cos_escalation_invalid");
    assert_eq!(item_state(&env, &item), "pending");
}

#[tokio::test]
async fn cos_chat_triage_web_path_must_match_the_target() {
    let env = admin_env();
    let app = env.router();
    let (_task, inbox_id) = decision_wait(&env, "dec-w", &[]);
    let item = ingest(&env, "inbox", &inbox_id, "r1", 0);
    let (_t, _r, bearer) = cos_bearer(&env, "web-path");
    let headers = [("authorization", bearer.as_str())];
    let opts = json!([{"key": "vault", "label": "vault"}]);
    let other = new_task(TaskKind::Execute, Status::Ready);
    for (key, path) in [
        ("external", "https://evil.example/tasks/x".to_string()),
        ("scheme-relative", "//evil.example/tasks".to_string()),
        ("other-task", format!("/tasks/{}", other.id)),
        ("relative", "tasks/x".to_string()),
    ] {
        let resp = send(
            &app,
            post_json_with(
                &resolve_path(&item),
                &resolve_body(
                    &format!("k-{key}"),
                    "r1",
                    "escalate",
                    json!({"escalation": packet(opts.clone(), json!("vault"), &path)}),
                ),
                &headers,
            ),
        )
        .await;
        assert_problem(&resp, 422, "cos_escalation_invalid");
    }
    let outbox: i64 = db(&env)
        .query_row(
            "SELECT COUNT(*) FROM notifications WHERE kind='cos_escalation'",
            [],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(outbox, 0);
}

#[tokio::test]
async fn cos_chat_triage_escalate_claims_one_outbox_row() {
    let env = admin_env();
    let app = env.router();
    let (task, inbox_id) = decision_wait(&env, "dec-e", &[]);
    let item = ingest(&env, "inbox", &inbox_id, "r1", 0);
    let (_t, _r, bearer) = cos_bearer(&env, "escalate");
    let headers = [("authorization", bearer.as_str())];
    let body = resolve_body(
        "k-esc",
        "r1",
        "escalate",
        json!({"escalation": packet(
            json!([{"key": "vault", "label": "vault を使う"}, {"key": "manual", "label": "手で入れる"}]),
            json!("vault"),
            &format!("/tasks/{}", task.id),
        )}),
    );
    let resp = send(&app, post_json_with(&resolve_path(&item), &body, &headers)).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let out = resp.json();
    assert_eq!(out["operation"]["action"], "inbox.escalate");
    assert_eq!(out["item"]["state"], "escalated");
    let notification = out["notification_id"]
        .as_str()
        .expect("outbox id")
        .to_string();

    let conn = db(&env);
    let (kind, key, raw): (String, String, String) = conn
        .query_row(
            "SELECT kind,key,body FROM notifications WHERE kind='cos_escalation'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("one outbox row");
    assert_eq!(kind, "cos_escalation");
    assert_eq!(key, item);
    let sent: Value = serde_json::from_str(&raw).expect("outbox body");
    assert_eq!(sent["summary"], "公開前の確認");
    assert_eq!(sent["recommended"], "vault");
    assert_eq!(sent["recommendation_reason"], "公開先が未確認");
    assert_eq!(sent["web_path"], format!("/tasks/{}", task.id));
    assert_eq!(sent["options"].as_array().expect("options").len(), 2);
    let routes: (i64, String) = conn
        .query_row(
            "SELECT COUNT(*),MAX(notification_id) FROM cos_notification_routes WHERE item_id=?1",
            [&item],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("routes");
    assert_eq!(routes, (1, notification));
    // The source wait stays open for the human.
    assert!(decision_answered_by(&env, &task).is_empty());

    // Same key: same operation. New key on the escalated item: 409, still one outbox row.
    let replay = send(&app, post_json_with(&resolve_path(&item), &body, &headers)).await;
    assert_eq!(replay.status.as_u16(), 200, "{}", replay.text());
    assert_eq!(replay.json()["operation"]["id"], out["operation"]["id"]);
    let mut second = body.clone();
    second["idempotency_key"] = json!("k-esc-2");
    let again = send(
        &app,
        post_json_with(&resolve_path(&item), &second, &headers),
    )
    .await;
    assert_problem(&again, 409, "cos_inbox_revision_conflict");
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM notifications WHERE kind='cos_escalation'",
            [],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(count, 1);
}
