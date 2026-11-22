//! ADR 2026-11-21-chat-discussion-mode D5 / D8.1 (WU edit-retract) through the HTTP stack:
//! `PATCH /chat/threads/{t}/messages/{m}`, `POST …/retract` and `GET …/revisions`. Only the
//! person who wrote a message changes it; an operator, another person, an MCP client and a CoS run
//! are refused and nothing is written; the change does not count as a new human instruction
//! (`human_epoch`); `DELETE …/messages/{m}` and the queue behave as before.
mod common;

use common::cos_ops::{cos_run_bearer, op_body, post_op};
use common::*;
use serde_json::{Value, json};
use time::OffsetDateTime;

const OTHER_HUMAN: &str = "other-human-token-revise";
const OPERATOR: &str = "operator-token-revise";
const MCP: &str = "mcp-token-revise";

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

/// `human:test` (TOKEN), `human:web`, `operator:claude-code` and the MCP token.
fn env() -> TestEnv {
    TestEnv::with(EnvOptions {
        token: Some(TOKEN.to_string()),
        extra_tokens: vec![
            task_api::ApiToken::new(
                task_api::Principal::Human { name: "web".into() },
                OTHER_HUMAN,
            ),
            task_api::ApiToken::new(
                task_api::Principal::Operator {
                    name: "claude-code".into(),
                },
                OPERATOR,
            ),
        ],
        mcp_token: Some(MCP.into()),
        ..EnvOptions::default()
    })
}

fn msg_path(t: &str, m: &str) -> String {
    format!("/api/v1/chat/threads/{t}/messages/{m}")
}

/// `human:test` posts `text` to `t` through the API (queued input; returns the message id).
async fn post_mine(app: &axum::Router, t: &str, key: &str, text: &str) -> String {
    let resp = send(
        app,
        post_admin(
            &format!("/api/v1/chat/threads/{t}/messages"),
            &json!({"client_message_id": key, "text": text, "attachment_ids": [],
                    "reply_to_id": null, "mode": "queue", "resume_queue": false}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 202, "{}", resp.text());
    resp.json()["message"]["id"]
        .as_str()
        .expect("id")
        .to_string()
}

/// Claims and completes the next queued input of `t` as `run` (the input becomes delivered).
fn deliver(env: &TestEnv, t: &str, run: &str) {
    let now = OffsetDateTime::now_utc();
    env.store
        .chat_run_claim_next(t, run, &json!({}), now)
        .expect("claim")
        .expect("run");
    env.store
        .chat_run_finish(
            run,
            task_core::chat::ChatRunState::Completed,
            Some("ok"),
            None,
            now,
        )
        .expect("finish");
}

fn edit_body(text: &str, rev: u64) -> Value {
    json!({"text": text, "expected_revision": rev})
}

fn text_of(env: &TestEnv, t: &str, m: &str) -> (String, u64) {
    let msg = env.store.chat_message_get(t, m).expect("message");
    (msg.text, msg.revision)
}

/// Invariant: the author edits and retracts their message over HTTP, the wire message carries
/// `revision` / `edited` / `retracted`, and `GET …/revisions` returns every previous body. Before
/// this WU `PATCH` was 405 and `…/retract`・`…/revisions` were 404 (no route).
#[tokio::test]
async fn chat_message_edit_by_author_keeps_previous_body_in_chat_events_api() {
    let env = env();
    let app = env.router();
    let t = project_cos_thread(&env.store, "revise-author").id;
    let m = post_mine(&app, &t, "k1", "first draft").await;

    let edited = send(
        &app,
        patch_admin(&msg_path(&t, &m), &edit_body("second draft", 1)),
    )
    .await;
    assert_eq!(edited.status.as_u16(), 200, "{}", edited.text());
    let wire = edited.json()["message"].clone();
    assert_eq!(wire["text"], "second draft");
    assert_eq!(wire["revision"], 2);
    assert_eq!(wire["edited"]["by"], "human:test");
    assert_eq!(wire["state"], "queued", "a queued input stays queued");

    deliver(&env, &t, "run-1");
    let retracted = send(
        &app,
        post_admin(
            &format!("{}/retract", msg_path(&t, &m)),
            &json!({"expected_revision": 2, "reason": "not what I meant"}),
        ),
    )
    .await;
    assert_eq!(retracted.status.as_u16(), 200, "{}", retracted.text());
    let wire = retracted.json()["message"].clone();
    assert_eq!(
        (wire["text"].as_str(), wire["revision"].as_u64()),
        (Some(""), Some(3))
    );
    assert_eq!(wire["retracted"]["reason"], "not what I meant");

    let revisions = send(&app, get_admin(&format!("{}/revisions", msg_path(&t, &m)))).await;
    assert_eq!(revisions.status.as_u16(), 200, "{}", revisions.text());
    let items = revisions.json()["items"]
        .as_array()
        .cloned()
        .expect("items");
    let previous: Vec<_> = items
        .iter()
        .map(|e| {
            assert_eq!(e["type"], "message_revised");
            (
                e["data"]["kind"].as_str().unwrap_or_default().to_string(),
                e["data"]["previous_text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    assert_eq!(
        previous,
        vec![
            ("edit".to_string(), "first draft".to_string()),
            ("retract".to_string(), "second draft".to_string()),
        ]
    );
    // The thread search no longer finds the retracted words.
    let search = send(&app, get_admin("/api/v1/chat/threads?q=draft")).await;
    assert_eq!(search.status.as_u16(), 200, "{}", search.text());
    assert_eq!(search.json()["items"].as_array().map(Vec::len), Some(0));
}

/// Invariant: an operator, another person, an MCP client and a CoS run cannot edit or retract a
/// person's message, nor can anyone change a relayed message; nothing is written.
#[tokio::test]
async fn chat_message_edit_forbidden_for_non_author_principals_api() {
    let env = env();
    let app = env.router();
    let t = project_cos_thread(&env.store, "revise-principals").id;
    let m = post_mine(&app, &t, "k1", "only mine").await;
    deliver(&env, &t, "run-1");
    let edit = edit_body("someone else's words", 1);
    let retract = json!({"expected_revision": 1, "reason": ""});
    let retract_path = format!("{}/retract", msg_path(&t, &m));

    for token in [OPERATOR, OTHER_HUMAN] {
        let auth = bearer(token);
        let headers = [("authorization", auth.as_str())];
        let resp = send(&app, patch_json_with(&msg_path(&t, &m), &edit, &headers)).await;
        assert_problem(&resp, 403, "chat_message_edit_forbidden");
        let resp = send(&app, post_json_with(&retract_path, &retract, &headers)).await;
        assert_problem(&resp, 403, "chat_message_edit_forbidden");
    }
    // The MCP token is read-only: the guard refuses the write before the handler (403).
    let mcp = bearer(MCP);
    let headers = [
        ("authorization", mcp.as_str()),
        ("x-celeris-mcp-client", "chatgpt"),
    ];
    let resp = send(&app, patch_json_with(&msg_path(&t, &m), &edit, &headers)).await;
    assert_eq!(resp.status.as_u16(), 403, "{}", resp.text());
    let resp = send(&app, post_json_with(&retract_path, &retract, &headers)).await;
    assert_eq!(resp.status.as_u16(), 403, "{}", resp.text());

    // A CoS run: a direct write is refused and recorded; through /cos/operations the path is
    // not in the allowlist, so it is refused too.
    let cos = cos_run_bearer(&env, &t, "cos-1");
    let headers = [("authorization", cos.as_str())];
    let resp = send(&app, patch_json_with(&msg_path(&t, &m), &edit, &headers)).await;
    assert_problem(&resp, 422, "cos_audit_context_required");
    let resp = send(&app, post_json_with(&retract_path, &retract, &headers)).await;
    assert_problem(&resp, 422, "cos_audit_context_required");
    for (key, method, path, body) in [
        ("rev-1", "PATCH", msg_path(&t, &m), edit.clone()),
        ("rev-2", "POST", retract_path.clone(), retract.clone()),
    ] {
        let resp = post_op(&app, &cos, &op_body(key, method, &path, body)).await;
        assert_eq!(resp.status.as_u16(), 422, "{key}: {}", resp.text());
    }
    assert_eq!(
        count(
            &env,
            "SELECT COUNT(*) FROM cos_operations WHERE state='applied'"
        ),
        0
    );

    // A relayed message is the operator's record of the person's words: nobody edits it.
    let relay = send(
        &app,
        post_json_with(
            &format!("/api/v1/chat/threads/{t}/relays"),
            &json!({"client_message_id": "r1", "text": "relayed words",
                    "said_at": "2026-11-21T00:00:00Z"}),
            &[("authorization", &bearer(OPERATOR))],
        ),
    )
    .await;
    assert!(relay.status.is_success(), "{}", relay.text());
    let r = relay.json()["message"]["id"]
        .as_str()
        .expect("relay id")
        .to_string();
    for token in [OPERATOR, TOKEN] {
        let auth = bearer(token);
        let resp = send(
            &app,
            patch_json_with(
                &msg_path(&t, &r),
                &edit_body("rewritten relay", 1),
                &[("authorization", auth.as_str())],
            ),
        )
        .await;
        assert_problem(&resp, 403, "chat_message_edit_forbidden");
    }

    assert_eq!(text_of(&env, &t, &m), ("only mine".to_string(), 1));
    assert_eq!(text_of(&env, &t, &r), ("relayed words".to_string(), 1));
    assert_eq!(
        count(
            &env,
            "SELECT COUNT(*) FROM chat_events WHERE type='message_revised'"
        ),
        0
    );
}

/// Invariant: state and revision conflicts are 409 `chat_conflict`; an unknown field is 400.
#[tokio::test]
async fn chat_message_edit_stale_revision_is_409_api() {
    let env = env();
    let app = env.router();
    let t = project_cos_thread(&env.store, "revise-conflict").id;
    let m = post_mine(&app, &t, "k1", "queued").await;
    let resp = send(
        &app,
        post_admin(
            &format!("{}/retract", msg_path(&t, &m)),
            &json!({"expected_revision": 1, "reason": ""}),
        ),
    )
    .await;
    assert_problem(&resp, 409, "chat_conflict");
    let resp = send(&app, patch_admin(&msg_path(&t, &m), &edit_body("v2", 1))).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let resp = send(&app, patch_admin(&msg_path(&t, &m), &edit_body("v3", 1))).await;
    assert_problem(&resp, 409, "chat_conflict");
    let resp = send(
        &app,
        patch_admin(
            &msg_path(&t, &m),
            &json!({"text": "v3", "expected_revision": 2, "discussion_mode": "execute"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 400, "{}", resp.text());
}

/// Invariant (D5.1): editing or retracting is not a new instruction, so the project
/// controller's `human_epoch` does not move (posting the message did move it).
#[tokio::test]
async fn chat_message_edit_does_not_bump_human_epoch() {
    let env = env();
    let thread = project_cos_thread(&env.store, "revise-epoch");
    let t = thread.id.clone();
    let project = thread.project_id.clone().expect("project thread");
    let app = task_api::router(
        env.state
            .clone()
            .with_project_controller(true, vec![project.clone()]),
    );
    let put = send(
        &app,
        put_json_with(
            &format!("/api/v1/projects/{project}/controller"),
            &json!({"mode": "shadow", "expected_revision": 0, "reason": "人が試す"}),
            &admin_headers(),
        ),
    )
    .await;
    assert_eq!(put.status.as_u16(), 200, "{}", put.text());
    let epoch = || {
        env.store
            .project_controller_state(&project)
            .expect("state")
            .expect("row")
            .human_epoch
    };
    let before_post = epoch();
    let m = post_mine(&app, &t, "k1", "an instruction").await;
    assert_eq!(epoch(), before_post + 1, "the post itself counts");
    let before = epoch();
    let resp = send(
        &app,
        patch_admin(&msg_path(&t, &m), &edit_body("edited", 1)),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    deliver(&env, &t, "run-1");
    let resp = send(
        &app,
        post_admin(
            &format!("{}/retract", msg_path(&t, &m)),
            &json!({"expected_revision": 2, "reason": "withdrawn"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(epoch(), before);
}

/// Regression: `DELETE …/messages/{m}` still cancels a queued input (and only that), the queue
/// event follows, and the new routes did not take its place.
#[tokio::test]
async fn chat_message_retract_keeps_delete_cancel_and_queue() {
    let env = env();
    let app = env.router();
    let t = project_cos_thread(&env.store, "revise-delete").id;
    let a = post_mine(&app, &t, "k1", "first").await;
    let b = post_mine(&app, &t, "k2", "second").await;
    let resp = send(&app, delete_admin(&msg_path(&t, &b))).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(resp.json()["message"]["state"], "cancelled");
    let queue = env
        .store
        .chat_events_page(&t, &task_core::chat::ChatEventQuery::default())
        .expect("events")
        .items
        .into_iter()
        .rev()
        .find(|e| e.event_type == task_core::chat::ChatEventType::Queue)
        .expect("queue event");
    match queue.data {
        task_core::chat::ChatEventData::Queue(q) => assert_eq!(q.message_ids, vec![a.clone()]),
        other => panic!("{other:?}"),
    }
    deliver(&env, &t, "run-1");
    let resp = send(&app, delete_admin(&msg_path(&t, &a))).await;
    assert_problem(&resp, 409, "chat_conflict");
    assert_eq!(text_of(&env, &t, &a), ("first".to_string(), 1));
}
