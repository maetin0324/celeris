//! ADR 2026-10-05 D2/D3: `POST /api/v1/cos/operations` and `GET /api/v1/cos/operations/{o}`.
mod common;

use common::*;
use serde_json::{Value, json};
use task_core::chat::{ChatCreateThreadRequest, ChatPostMessageRequest, ChatSendMode};
use task_core::decision::{
    CostOfReversal, DecisionKind, DecisionOption, DecisionOrigin, DecisionPathEntry,
    DecisionRaisedBy, DecisionRequest, DecisionStatus,
};
use task_core::{Event, Status, TaskKind, TaskStore};
use time::{Duration, OffsetDateTime};

const OPS: &str = "/api/v1/cos/operations";

/// A live CoS run of a new thread and its bearer: `(thread_id, run_id, "Bearer …")`.
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

fn op_body(key: &str, method: &str, path: &str, body: Value) -> Value {
    json!({
        "idempotency_key": key,
        "expected_revision": null,
        "reason": "人が依頼した",
        "policy_version": "1",
        "request": {"method": method, "path": path, "body": body},
    })
}

async fn post_op(app: &axum::Router, bearer: &str, body: &Value) -> Resp {
    send(app, post_json_with(OPS, body, &[("authorization", bearer)])).await
}

fn db(env: &TestEnv) -> rusqlite::Connection {
    rusqlite::Connection::open(&env.db_path).expect("open db")
}

fn cos_events(env: &TestEnv) -> Vec<(String, Value)> {
    let conn = db(env);
    let mut stmt = conn
        .prepare("SELECT task_id, json FROM events ORDER BY seq")
        .expect("prepare");
    stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })
    .expect("query")
    .map(|row| {
        let (task, raw) = row.expect("row");
        (task, serde_json::from_str::<Value>(&raw).expect("json"))
    })
    .filter(|(_, event)| event["type"] == "cos_operation")
    .collect()
}

fn count(env: &TestEnv, sql: &str) -> i64 {
    db(env).query_row(sql, [], |row| row.get(0)).expect("count")
}

#[tokio::test]
async fn cos_chat_ops_api_rejects_paths_outside_the_allowlist_with_reasoned_events() {
    let env = admin_env();
    let app = env.router();
    let (thread, run, bearer) = cos_bearer(&env, "deny");
    let cases = [
        ("ext", "POST", "https://example.com/api/v1/tasks"),
        ("host", "POST", "//example.com/api/v1/tasks"),
        ("dots", "POST", "/api/v1/tasks/../providers"),
        ("recursive", "POST", "/api/v1/cos/operations"),
        ("checkpoint", "POST", "/api/v1/cos/threads/t/checkpoint"),
        ("unregistered", "POST", "/api/v1/providers"),
        ("method", "DELETE", "/api/v1/tasks"),
        ("query", "POST", "/api/v1/tasks?x=1"),
    ];
    for (key, method, path) in cases {
        let resp = post_op(&app, &bearer, &op_body(key, method, path, json!({}))).await;
        assert_problem(&resp, 422, "cos_operation_not_allowed");
    }
    assert_eq!(count(&env, "SELECT COUNT(*) FROM tasks"), 0);
    assert_eq!(
        count(
            &env,
            "SELECT COUNT(*) FROM cos_operations WHERE state='rejected'"
        ),
        cases.len() as i64
    );
    let events = cos_events(&env);
    assert_eq!(events.len(), cases.len(), "{events:?}");
    for (_, event) in &events {
        assert_eq!(event["state"], "rejected");
        assert_eq!(event["actor"], "cos");
        assert_eq!(event["thread_id"], thread.as_str());
        assert_eq!(event["run_id"], run.as_str());
        let reason = event["reason"].as_str().expect("reason");
        assert!(reason.starts_with("rejected: "), "{reason}");
        assert!(reason.contains("人が依頼した"), "{reason}");
    }
    let reasons: Vec<String> = events
        .iter()
        .map(|(_, e)| e["reason"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(reasons.iter().any(|r| r.contains("not a URL or host")));
    assert!(reasons.iter().any(|r| r.contains("recursive_cos")));
    assert!(
        reasons
            .iter()
            .any(|r| r.contains("not a registered CoS operation"))
    );
}

#[tokio::test]
async fn cos_chat_ops_api_idempotency_same_hash_same_operation_different_hash_409() {
    let env = admin_env();
    let app = env.router();
    let (_, _, bearer) = cos_bearer(&env, "idem");
    let body = op_body(
        "create-1",
        "POST",
        "/api/v1/tasks",
        json!({"title": "画面修正", "objective": "依頼の全文", "acceptance": [{"type": "artifact_exists", "name": "result.md"}]}),
    );
    let first = post_op(&app, &bearer, &body).await;
    assert_eq!(first.status.as_u16(), 200, "{}", first.text());
    // Same request with keys in another order hashes the same.
    let reordered = op_body(
        "create-1",
        "post",
        "/api/v1/tasks",
        json!({"objective": "依頼の全文", "title": "画面修正", "acceptance": [{"type": "artifact_exists", "name": "result.md"}]}),
    );
    let second = post_op(&app, &bearer, &reordered).await;
    assert_eq!(second.status.as_u16(), 200, "{}", second.text());
    assert_eq!(
        first.json()["operation"]["id"],
        second.json()["operation"]["id"]
    );
    assert_eq!(count(&env, "SELECT COUNT(*) FROM tasks"), 1);

    let changed = op_body(
        "create-1",
        "POST",
        "/api/v1/tasks",
        json!({"title": "別の依頼", "objective": "依頼の全文", "acceptance": [{"type": "artifact_exists", "name": "result.md"}]}),
    );
    assert_problem(
        &post_op(&app, &bearer, &changed).await,
        409,
        "chat_conflict",
    );
    assert_eq!(count(&env, "SELECT COUNT(*) FROM tasks"), 1);
    assert_eq!(count(&env, "SELECT COUNT(*) FROM cos_operations"), 1);
}

#[tokio::test]
async fn cos_chat_ops_api_task_create_writes_domain_audit_and_card_together() {
    let env = admin_env();
    let app = env.router();
    let (thread, run, bearer) = cos_bearer(&env, "create");
    let resp = post_op(
        &app,
        &bearer,
        &op_body(
            "create-1",
            "POST",
            "/api/v1/tasks",
            json!({"title": "画面修正", "objective": "依頼の全文", "acceptance": [{"type": "artifact_exists", "name": "result.md"}]}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let op = resp.json()["operation"].clone();
    assert_eq!(op["state"], "applied");
    assert_eq!(op["actor"], "cos");
    assert_eq!(op["action"], "task.create");
    assert_eq!(op["thread_id"], thread.as_str());
    assert_eq!(op["run_id"], run.as_str());
    let task_id = op["result"]["task_id"]
        .as_str()
        .expect("task id")
        .to_string();
    assert_eq!(op["target_id"], task_id.as_str());

    let task = env
        .store
        .get(task_id.parse().expect("ulid"))
        .expect("get")
        .expect("task exists");
    assert_eq!(task.title, "画面修正");
    assert_eq!(task.status, Status::Ready);

    // Audit envelope in the task's own event stream, after Created.
    let events = cos_events(&env);
    assert_eq!(events.len(), 1, "{events:?}");
    let (stream, event) = &events[0];
    assert_eq!(stream, &task_id);
    for (field, want) in [
        ("actor", "cos"),
        ("thread_id", thread.as_str()),
        ("run_id", run.as_str()),
        ("operation_id", op["id"].as_str().expect("id")),
        ("reason", "人が依頼した"),
        ("policy_version", "1"),
        ("state", "applied"),
    ] {
        assert_eq!(event[field], want, "{field}");
    }
    // The card event and the operation row point at each other.
    let (card_id, payload): (i64, String) = db(&env)
        .query_row(
            "SELECT id, payload_json FROM chat_events WHERE thread_id=?1 AND type='card'",
            [&thread],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("card event");
    assert_eq!(op["event_id"], card_id.to_string());
    assert!(
        payload.contains(op["id"].as_str().expect("id")),
        "{payload}"
    );

    // A domain failure leaves neither task nor applied row nor applied audit event behind.
    let bad = post_op(
        &app,
        &bearer,
        &op_body(
            "create-2",
            "POST",
            "/api/v1/tasks",
            json!({"title": "no objective"}),
        ),
    )
    .await;
    assert_problem(&bad, 422, "validation");
    assert_eq!(count(&env, "SELECT COUNT(*) FROM tasks"), 1);
    assert_eq!(
        count(
            &env,
            "SELECT COUNT(*) FROM cos_operations WHERE state='applied'"
        ),
        1
    );
}

#[tokio::test]
async fn cos_chat_ops_api_comment_and_decision_answer_use_the_shared_operations() {
    let env = admin_env();
    let app = env.router();
    let (_, _, bearer) = cos_bearer(&env, "domains");
    let task = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&task);

    let resp = post_op(
        &app,
        &bearer,
        &op_body(
            "comment-1",
            "POST",
            &format!("/api/v1/tasks/{}/comments", task.id),
            json!({"body": "人からの補足"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(resp.json()["operation"]["action"], "comment.create");
    let comments = env.store.comments_for(task.id).expect("comments");
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].author.as_deref(), Some("cos"));

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
                    id: "dec-1".into(),
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
    let resp = post_op(
        &app,
        &bearer,
        &op_body(
            "answer-1",
            "POST",
            "/api/v1/decisions/dec-1/answer",
            json!({"option": "vault"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let events = env.store.events_for(task.id).expect("events");
    assert!(events.iter().any(|(_, e)| matches!(
        e,
        Event::DecisionAnswered { by, option, .. } if by == "cos" && option == "vault"
    )));
    // Answering again is a domain conflict: rejected, recorded, the decision unchanged.
    let again = post_op(
        &app,
        &bearer,
        &op_body(
            "answer-2",
            "POST",
            "/api/v1/decisions/dec-1/answer",
            json!({"option": "manual"}),
        ),
    )
    .await;
    assert_eq!(again.status.as_u16(), 409, "{}", again.text());
    assert_eq!(
        count(
            &env,
            "SELECT COUNT(*) FROM cos_operations WHERE state='rejected'"
        ),
        1
    );
    // The human handler keeps answering as `human` through the same function.
    let human = send(
        &app,
        post_admin(
            "/api/v1/decisions/dec-1/answer",
            &json!({"option": "vault"}),
        ),
    )
    .await;
    assert_eq!(human.status.as_u16(), 409, "{}", human.text());
}

#[tokio::test]
async fn cos_chat_ops_api_body_shape_and_identity_claims_are_rejected() {
    let env = admin_env();
    let app = env.router();
    let (_, _, bearer) = cos_bearer(&env, "shape");
    let base = op_body(
        "k",
        "POST",
        "/api/v1/tasks",
        json!({"title":"t","objective":"o"}),
    );

    let mut unknown = base.clone();
    unknown["extra"] = json!(1);
    assert_problem(&post_op(&app, &bearer, &unknown).await, 400, "bad_request");

    let mut empty_reason = base.clone();
    empty_reason["reason"] = json!("  ");
    assert_problem(
        &post_op(&app, &bearer, &empty_reason).await,
        422,
        "validation",
    );

    let mut no_policy = base.clone();
    no_policy
        .as_object_mut()
        .expect("object")
        .remove("policy_version");
    assert_problem(&post_op(&app, &bearer, &no_policy).await, 422, "validation");

    let mut no_revision = base.clone();
    no_revision
        .as_object_mut()
        .expect("object")
        .remove("expected_revision");
    assert_problem(
        &post_op(&app, &bearer, &no_revision).await,
        422,
        "validation",
    );

    let mut actor = base.clone();
    actor["actor"] = json!("human");
    assert_problem(
        &post_op(&app, &bearer, &actor).await,
        422,
        "cos_identity_claim",
    );

    let claim = op_body(
        "claim",
        "POST",
        "/api/v1/tasks",
        json!({"title":"t","objective":"o","actor":"human"}),
    );
    assert_problem(
        &post_op(&app, &bearer, &claim).await,
        422,
        "cos_identity_claim",
    );

    // The admin token is not a CoS run credential.
    let resp = send(&app, post_admin(OPS, &base)).await;
    assert_problem(&resp, 403, "cos_credential_required");
    assert_eq!(count(&env, "SELECT COUNT(*) FROM tasks"), 0);
}

#[tokio::test]
async fn cos_chat_ops_api_get_operation_is_scoped_to_the_thread() {
    let env = admin_env();
    let app = env.router();
    let (_, _, bearer) = cos_bearer(&env, "owner");
    let (_, _, other) = cos_bearer(&env, "other");
    let resp = post_op(
        &app,
        &bearer,
        &op_body(
            "create-1",
            "POST",
            "/api/v1/tasks",
            json!({"title": "t", "objective": "o", "acceptance": [{"type": "artifact_exists", "name": "result.md"}]}),
        ),
    )
    .await;
    let id = resp.json()["operation"]["id"]
        .as_str()
        .expect("id")
        .to_string();
    let path = format!("{OPS}/{id}");

    let own = send(&app, get_with(&path, &[("authorization", &bearer)])).await;
    assert_eq!(own.status.as_u16(), 200, "{}", own.text());
    assert_eq!(own.json()["operation"]["id"], id.as_str());

    let foreign = send(&app, get_with(&path, &[("authorization", &other)])).await;
    assert_problem(&foreign, 404, "chat_not_found");

    let admin = send(&app, get_admin(&path)).await;
    assert_eq!(admin.status.as_u16(), 200, "{}", admin.text());
    let missing = send(
        &app,
        get_admin(&format!("{OPS}/01ARZ3NDEKTSV4RRFFQ69G5FAV")),
    )
    .await;
    assert_problem(&missing, 404, "chat_not_found");
}

/// ADR 2026-10-08-cos-chat-prompt-cache D1.7/T5: authorization is the API's, not the skill's.
/// The operations the old cos-operator skill listed but `/cos/operations` never registered are
/// refused with 422 and recorded, whatever skill the run did or did not read (the API has no
/// notion of skills).
#[tokio::test]
async fn cos_chat_ops_api_rejects_unregistered_operations_regardless_of_skill() {
    let env = admin_env();
    let app = env.router();
    let (_thread, _run, bearer) = cos_bearer(&env, "noskill");
    let cases = [
        ("cron", "POST", "/api/v1/cron-jobs"),
        ("project-pause", "POST", "/api/v1/projects/p1/pause"),
        ("standing", "POST", "/api/v1/standing-rules"),
    ];
    for (key, method, path) in cases {
        let resp = post_op(&app, &bearer, &op_body(key, method, path, json!({}))).await;
        assert_problem(&resp, 422, "cos_operation_not_allowed");
    }
    assert_eq!(
        count(
            &env,
            "SELECT COUNT(*) FROM cos_operations WHERE state='rejected'"
        ),
        cases.len() as i64
    );
}
