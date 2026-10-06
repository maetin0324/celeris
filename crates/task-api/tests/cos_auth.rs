//! ADR 2026-10-05 D2/D3: CoS run credential middleware and the checkpoint API.
mod common;

use common::*;
use serde_json::{Value, json};
use task_core::chat::{ChatCreateThreadRequest, ChatPostMessageRequest, ChatSendMode};
use time::{Duration, OffsetDateTime};

/// A live CoS run: `(thread_id, run_id, delivered input seq)`.
fn live_run(env: &TestEnv, key: &str) -> (String, String, u64) {
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
    let run_id = format!("run-{key}");
    let run = store
        .chat_run_claim_next(&thread.id, &run_id, &json!({}), now)
        .expect("claim")
        .expect("run");
    let conn = rusqlite::Connection::open(&env.db_path).expect("open db");
    let seq: i64 = conn
        .query_row(
            "SELECT seq FROM chat_messages WHERE id=?1",
            [&run.input_message_id],
            |row| row.get(0),
        )
        .expect("input seq");
    (thread.id, run_id, seq as u64)
}

fn env_with_token(token: bool) -> TestEnv {
    if token { admin_env() } else { TestEnv::new() }
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

fn issue(env: &TestEnv, thread: &str, run: &str) -> String {
    task_api::cos::issue_run_bearer(&env.store, thread, run, Duration::hours(1)).expect("issue")
}

fn checkpoint_path(thread: &str) -> String {
    format!("/api/v1/cos/threads/{thread}/checkpoint")
}

fn checkpoint_body(run: &str, summary: &str, through: u64, expected: u64) -> Value {
    json!({"run_id":run,"summary":summary,"through_seq":through,"expected_summary_through_seq":expected})
}

fn rejected_audit_events(env: &TestEnv) -> Vec<Value> {
    let conn = rusqlite::Connection::open(&env.db_path).expect("open db");
    let mut stmt = conn
        .prepare("SELECT json FROM events ORDER BY seq")
        .expect("prepare");
    stmt.query_map([], |row| row.get::<_, String>(0))
        .expect("query")
        .map(|row| serde_json::from_str::<Value>(&row.expect("row")).expect("json"))
        .filter(|event| event.to_string().contains("\"cos_operation\""))
        .collect()
}

#[tokio::test]
async fn cos_chat_ops_auth_expired_revoked_and_unknown_are_401() {
    let env = env_with_token(true);
    let app = env.router();
    let (thread, run, _) = live_run(&env, "expired");
    let expired = task_api::cos::issue_run_bearer_at(
        &env.store,
        &thread,
        &run,
        Duration::minutes(1),
        OffsetDateTime::now_utc() - Duration::hours(1),
    )
    .expect("issue");
    let resp = send(
        &app,
        get_with(
            "/api/v1/chat/threads",
            &[("authorization", &bearer(&expired))],
        ),
    )
    .await;
    let problem = assert_problem(&resp, 401, "unauthorized");
    assert_eq!(problem["detail"], "expired CoS run credential");
    assert!(!resp.text().contains(&expired), "credential echoed");

    let (thread, run, _) = live_run(&env, "revoked");
    let revoked = issue(&env, &thread, &run);
    env.store
        .cos_run_credential_revoke(&run, OffsetDateTime::now_utc())
        .expect("revoke");
    let problem = assert_problem(
        &send(
            &app,
            get_with(
                "/api/v1/chat/threads",
                &[("authorization", &bearer(&revoked))],
            ),
        )
        .await,
        401,
        "unauthorized",
    );
    assert_eq!(problem["detail"], "revoked CoS run credential");

    let problem = assert_problem(
        &send(
            &app,
            get_with(
                "/api/v1/chat/threads",
                &[("authorization", "Bearer celeris-cos-run.00ff")],
            ),
        )
        .await,
        401,
        "unauthorized",
    );
    assert_eq!(problem["detail"], "unknown CoS run credential");
}

#[tokio::test]
async fn cos_chat_ops_auth_cos_shape_never_falls_back_to_admin() {
    // Without an admin token every bearer used to pass; a CoS-shaped bearer is still verified.
    let env = env_with_token(false);
    let app = env.router();
    assert_problem(
        &send(
            &app,
            get_with(
                "/api/v1/tasks",
                &[("authorization", "Bearer celeris-cos-run.bogus")],
            ),
        )
        .await,
        401,
        "unauthorized",
    );
    // The admin token prefixed as a CoS bearer is not the admin token.
    let env = env_with_token(true);
    let app = env.router();
    assert_problem(
        &send(
            &app,
            get_with(
                "/api/v1/tasks",
                &[("authorization", "Bearer celeris-cos-run.s3cret-token-value")],
            ),
        )
        .await,
        401,
        "unauthorized",
    );
}

#[tokio::test]
async fn cos_chat_ops_auth_get_allowed_and_direct_mutation_is_422_with_audit() {
    let env = env_with_token(true);
    let app = env.router();
    let (thread, run, _) = live_run(&env, "direct");
    let token = issue(&env, &thread, &run);
    let auth = bearer(&token);
    let headers = [("authorization", auth.as_str())];
    let listed = send(&app, get_with("/api/v1/chat/threads", &headers)).await;
    assert_eq!(listed.status.as_u16(), 200, "{}", listed.text());
    assert_eq!(
        send(&app, get_with("/api/v1/tasks", &headers))
            .await
            .status
            .as_u16(),
        200
    );

    let created = send(
        &app,
        post_json_with(
            "/api/v1/tasks",
            &json!({"title":"t","objective":"o"}),
            &headers,
        ),
    )
    .await;
    let problem = assert_problem(&created, 422, "cos_audit_context_required");
    assert_eq!(problem["instead"], "/api/v1/cos/operations");
    assert_problem(
        &send(
            &app,
            patch_json_with(
                &format!("/api/v1/chat/threads/{thread}"),
                &json!({"title":"x","expected_revision":1}),
                &headers,
            ),
        )
        .await,
        422,
        "cos_audit_context_required",
    );
    assert_problem(
        &send(
            &app,
            delete_with(
                &format!("/api/v1/chat/threads/{thread}/messages/m"),
                &headers,
            ),
        )
        .await,
        422,
        "cos_audit_context_required",
    );
    // No domain write happened.
    let conn = rusqlite::Connection::open(&env.db_path).expect("open db");
    let tasks: i64 = conn
        .query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))
        .expect("tasks");
    assert_eq!(tasks, 0, "direct POST must not create a task");
    let audits = rejected_audit_events(&env);
    assert_eq!(audits.len(), 3, "{audits:?}");
    for audit in &audits {
        let text = audit.to_string();
        assert!(text.contains("\"rejected\""), "{text}");
        assert!(text.contains(&run) && text.contains(&thread), "{text}");
        assert!(
            text.contains("/api/v1/cos/operations"),
            "reason missing: {text}"
        );
        assert!(!text.contains(&token), "credential leaked into events");
    }
    let rejected: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cos_operations WHERE state='rejected' AND run_id=?1",
            [&run],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(rejected, 3);
}

#[tokio::test]
async fn cos_chat_ops_auth_identity_claims_are_ignored_or_422() {
    let env = env_with_token(true);
    let app = env.router();
    let (thread, run, seq) = live_run(&env, "spoof");
    let token = issue(&env, &thread, &run);
    let auth = bearer(&token);
    // Headers claiming a human actor or another run change nothing: the mutation is still CoS.
    let spoofed = [
        ("authorization", auth.as_str()),
        ("x-celeris-actor", "human"),
        ("x-celeris-run-id", "run-other"),
    ];
    assert_problem(
        &send(
            &app,
            post_json_with(
                "/api/v1/tasks",
                &json!({"title":"t","objective":"o"}),
                &spoofed,
            ),
        )
        .await,
        422,
        "cos_audit_context_required",
    );
    // Identity in the body is refused outright.
    let mut body = checkpoint_body(&run, "s", seq, 0);
    body["actor"] = json!("human");
    assert_problem(
        &send(
            &app,
            post_json_with(&checkpoint_path(&thread), &body, &spoofed),
        )
        .await,
        422,
        "cos_identity_claim",
    );
    // Claiming another run in the body is 403, not a switch of identity.
    let (other_thread, other_run, _) = live_run(&env, "spoof-other");
    assert_problem(
        &send(
            &app,
            post_json_with(
                &checkpoint_path(&other_thread),
                &checkpoint_body(&other_run, "s", 1, 0),
                &spoofed,
            ),
        )
        .await,
        403,
        "cos_credential_required",
    );
}

#[tokio::test]
async fn cos_chat_ops_auth_admin_bearer_unchanged() {
    let env = env_with_token(true);
    let app = env.router();
    let created = send(
        &app,
        post_admin(
            "/api/v1/chat/threads",
            &json!({"title":"a","project_id":null,"client_thread_id":"adm"}),
        ),
    )
    .await;
    assert_eq!(created.status.as_u16(), 201, "{}", created.text());
    assert_problem(
        &send(
            &app,
            get_with("/api/v1/tasks", &[("authorization", "Bearer wrong")]),
        )
        .await,
        401,
        "unauthorized",
    );
    assert!(rejected_audit_events(&env).is_empty());
}

#[tokio::test]
async fn cos_chat_ops_checkpoint_api_200_and_conflicts() {
    let env = env_with_token(true);
    let app = env.router();
    let (thread, run, seq) = live_run(&env, "ok");
    let auth = bearer(&issue(&env, &thread, &run));
    let headers = [("authorization", auth.as_str())];
    let saved = send(
        &app,
        post_json_with(
            &checkpoint_path(&thread),
            &checkpoint_body(&run, "決定と残作業", seq, 0),
            &headers,
        ),
    )
    .await;
    assert_eq!(saved.status.as_u16(), 200, "{}", saved.text());
    assert_eq!(
        saved.json(),
        json!({"thread_id":thread,"summary_through_seq":seq})
    );
    // Stale expected cursor.
    assert_problem(
        &send(
            &app,
            post_json_with(
                &checkpoint_path(&thread),
                &checkpoint_body(&run, "again", seq, 0),
                &headers,
            ),
        )
        .await,
        409,
        "chat_conflict",
    );
    // Beyond the delivered range.
    assert_problem(
        &send(
            &app,
            post_json_with(
                &checkpoint_path(&thread),
                &checkpoint_body(&run, "ahead", seq + 10, seq),
                &headers,
            ),
        )
        .await,
        422,
        "validation",
    );
    // Over 32 KiB.
    let big = "a".repeat(32 * 1024 + 1);
    assert_problem(
        &send(
            &app,
            post_json_with(
                &checkpoint_path(&thread),
                &checkpoint_body(&run, &big, seq, seq),
                &headers,
            ),
        )
        .await,
        413,
        "payload_too_large",
    );
}

#[tokio::test]
async fn cos_chat_ops_checkpoint_api_403_for_human_and_other_run() {
    let env = env_with_token(true);
    let app = env.router();
    let (thread, run, seq) = live_run(&env, "mine");
    let (other_thread, other_run, _) = live_run(&env, "theirs");
    // A human (admin) bearer cannot checkpoint.
    assert_problem(
        &send(
            &app,
            post_admin(
                &checkpoint_path(&thread),
                &checkpoint_body(&run, "s", seq, 0),
            ),
        )
        .await,
        403,
        "cos_credential_required",
    );
    // Another run's credential cannot checkpoint this thread, even naming this run.
    let other = bearer(&issue(&env, &other_thread, &other_run));
    assert_problem(
        &send(
            &app,
            post_json_with(
                &checkpoint_path(&thread),
                &checkpoint_body(&run, "s", seq, 0),
                &[("authorization", other.as_str())],
            ),
        )
        .await,
        403,
        "cos_credential_required",
    );
    // The right thread, but a body naming another run.
    let mine = bearer(&issue(&env, &thread, &run));
    assert_problem(
        &send(
            &app,
            post_json_with(
                &checkpoint_path(&thread),
                &checkpoint_body(&other_run, "s", seq, 0),
                &[("authorization", mine.as_str())],
            ),
        )
        .await,
        403,
        "cos_credential_required",
    );
}
