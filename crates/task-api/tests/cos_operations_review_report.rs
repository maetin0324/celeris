//! ADR 2026-10-28-cos-review-back-thread D1/D4'/D6: the review run's report operation
//! (`chat.review_report`), the read-only review thread and `GET /cos/review-thread`.
mod common;
use axum::body::Body;
use axum::http::Request;
use common::cos_ops::{OPS, cos_bearer, db, op_body};
use common::*;
use serde_json::{Value, json};
use time::{Duration, OffsetDateTime};

const REPORT: &str = "/api/v1/cos/review-report";

/// The review thread with one cron input and a live run of it: `(thread, input id, run, bearer)`.
fn review_run(env: &TestEnv, key: &str) -> (String, String, String, String) {
    let now = OffsetDateTime::now_utc();
    let input = env
        .store
        .cos_review_input_post(&format!("cron-cos-review:job:{key}"), "定期見直し", now)
        .expect("review input");
    let thread = input.response.message.thread_id.clone();
    let run = format!("run-review-{key}");
    env.store
        .chat_run_claim_next(&thread, &run, &json!({}), now)
        .expect("claim")
        .expect("review run");
    let bearer = task_api::cos::issue_run_bearer(&env.store, &thread, &run, Duration::hours(1))
        .expect("issue");
    env.store
        .chat_run_record_policy(&run, "1", "test")
        .expect("policy");
    (
        thread,
        input.response.message.id.clone(),
        run,
        format!("Bearer {bearer}"),
    )
}

fn report_body(key: &str, review_message_id: &str, text: &str) -> Value {
    op_body(
        key,
        "POST",
        REPORT,
        json!({"review_message_id": review_message_id, "text": text}),
    )
}

/// `(state, count)` of the `cos_operations` rows of `thread` with `idempotency_key = key`.
fn op_rows(env: &TestEnv, thread: &str, key: &str) -> Vec<(String, String)> {
    let conn = db(env);
    let mut stmt = conn
        .prepare(
            "SELECT state, action FROM cos_operations WHERE thread_id=?1 AND idempotency_key=?2",
        )
        .expect("prepare");
    stmt.query_map(rusqlite::params![thread, key], |row| {
        Ok((row.get(0)?, row.get(1)?))
    })
    .expect("query")
    .map(|row| row.expect("row"))
    .collect()
}

/// Assistant messages of the general thread whose key marks a review report.
fn general_reports(env: &TestEnv) -> Vec<(String, String)> {
    let conn = db(env);
    let mut stmt = conn
        .prepare(
            "SELECT m.id, m.text FROM chat_messages m JOIN chat_threads t ON t.id=m.thread_id \
             WHERE t.kind='general' AND m.client_message_id LIKE 'cos-review-report:%'",
        )
        .expect("prepare");
    stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("query")
        .map(|row| row.expect("row"))
        .collect()
}

#[tokio::test]
async fn review_report_from_the_review_run_writes_one_general_message_and_one_audit_row() {
    let env = admin_env();
    let app = env.router();
    let (thread, input, run, bearer) = review_run(&env, "one");

    let resp = send(
        &app,
        post_json_with(
            OPS,
            &report_body("report-1", &input, "案件 A が停滞。人の判断が要る。"),
            &[("authorization", &bearer)],
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let op = resp.json()["operation"].clone();
    assert_eq!(op["state"], "applied", "{op}");
    assert_eq!(op["action"], "chat.review_report");
    assert_eq!(op["thread_id"], thread.as_str());
    assert_eq!(op["target_kind"], "chat_thread");
    assert_eq!(op["result"]["created"], true);
    let message = op["result"]["message"].clone();
    assert_eq!(message["role"], "assistant");
    assert_eq!(message["state"], "completed");
    assert_eq!(message["provenance"]["kind"], "cos_review_report");
    assert_eq!(message["provenance"]["review_thread_id"], thread.as_str());
    assert_eq!(message["provenance"]["review_message_id"], input.as_str());
    assert_eq!(message["provenance"]["run_id"], run.as_str());
    let general = message["thread_id"].as_str().expect("general").to_owned();
    assert_ne!(general, thread);
    assert_eq!(general_reports(&env).len(), 1);
    assert_eq!(
        op_rows(&env, &thread, "report-1"),
        vec![("applied".to_string(), "chat.review_report".to_string())]
    );

    // The same envelope again: the recorded operation, no second row or message.
    let again = send(
        &app,
        post_json_with(
            OPS,
            &report_body("report-1", &input, "案件 A が停滞。人の判断が要る。"),
            &[("authorization", &bearer)],
        ),
    )
    .await;
    assert_eq!(again.status.as_u16(), 200, "{}", again.text());
    assert_eq!(again.json()["operation"]["id"], op["id"]);
    assert_eq!(op_rows(&env, &thread, "report-1").len(), 1);
    // A new key for the same input: the first report comes back, nothing new in the general thread.
    let other = send(
        &app,
        post_json_with(
            OPS,
            &report_body("report-2", &input, "別の文"),
            &[("authorization", &bearer)],
        ),
    )
    .await;
    assert_eq!(other.status.as_u16(), 200, "{}", other.text());
    let other = other.json()["operation"].clone();
    assert_eq!(other["result"]["created"], false, "{other}");
    assert_eq!(other["result"]["message"]["id"], message["id"]);
    let reports = general_reports(&env);
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].1, "案件 A が停滞。人の判断が要る。");

    // The report is not a queued input: no general CoS run starts.
    let claimed = env
        .store
        .chat_run_claim_next(
            &general,
            "run-general",
            &json!({}),
            OffsetDateTime::now_utc(),
        )
        .expect("claim");
    assert!(claimed.is_none(), "{claimed:?}");
    // The person sees it in the general thread.
    let listed = send(
        &app,
        get_admin(&format!("/api/v1/chat/threads/{general}/messages")),
    )
    .await;
    assert_eq!(listed.status.as_u16(), 200, "{}", listed.text());
    let items = listed.json()["items"].as_array().cloned().expect("items");
    assert!(
        items
            .iter()
            .any(|m| m["id"] == message["id"] && m["provenance"]["kind"] == "cos_review_report"),
        "{items:?}"
    );
}

#[tokio::test]
async fn review_report_from_another_run_a_person_or_a_wrong_input_is_422() {
    let env = admin_env();
    let app = env.router();
    let (review, input, _, review_bearer) = review_run(&env, "two");

    // A run of another (project) thread: 422, recorded as rejected, nothing written.
    let (project, _, project_bearer) = cos_bearer(&env, "not-review");
    let resp = send(
        &app,
        post_json_with(
            OPS,
            &report_body("report-other", &input, "報告"),
            &[("authorization", &project_bearer)],
        ),
    )
    .await;
    assert_problem(&resp, 422, "cos_review_report_invalid");
    assert_eq!(
        op_rows(&env, &project, "report-other"),
        vec![("rejected".to_string(), "chat.review_report".to_string())]
    );
    // The general thread's run is refused the same way.
    let now = OffsetDateTime::now_utc();
    let general = env
        .store
        .cos_thread_open(
            &task_core::chat::ChatOpenThreadRequest { project_id: None },
            now,
        )
        .expect("general")
        .thread
        .id;
    env.store
        .chat_message_post(
            &general,
            &task_core::chat::ChatPostMessageRequest {
                client_message_id: "human-1".into(),
                text: "やあ".into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: task_core::chat::ChatSendMode::Queue,
                resume_queue: false,
            },
            now,
        )
        .expect("human message");
    env.store
        .chat_run_claim_next(&general, "run-general", &json!({}), now)
        .expect("claim")
        .expect("general run");
    let general_bearer =
        task_api::cos::issue_run_bearer(&env.store, &general, "run-general", Duration::hours(1))
            .expect("issue");
    env.store
        .chat_run_record_policy("run-general", "1", "test")
        .expect("policy");
    let resp = send(
        &app,
        post_json_with(
            OPS,
            &report_body("report-general", &input, "報告"),
            &[("authorization", &format!("Bearer {general_bearer}"))],
        ),
    )
    .await;
    assert_problem(&resp, 422, "cos_review_report_invalid");

    // The review run naming a message that is not a review input of the review thread.
    let human_message = env
        .store
        .chat_message_list(&general, &task_core::chat::ChatMessageQuery::default())
        .expect("messages")
        .items[0]
        .id
        .clone();
    for (key, id) in [
        ("wrong-thread", human_message.as_str()),
        ("missing", "nope"),
    ] {
        let resp = send(
            &app,
            post_json_with(
                OPS,
                &report_body(key, id, "報告"),
                &[("authorization", &review_bearer)],
            ),
        )
        .await;
        assert_problem(&resp, 422, "cos_review_report_invalid");
        assert_eq!(
            op_rows(&env, &review, key),
            vec![("rejected".to_string(), "chat.review_report".to_string())]
        );
    }
    // Blank text.
    let resp = send(
        &app,
        post_json_with(
            OPS,
            &report_body("blank", &input, "  "),
            &[("authorization", &review_bearer)],
        ),
    )
    .await;
    assert_problem(&resp, 422, "cos_review_report_invalid");

    // A person (admin token) calling the route: 422; the operation envelope is CoS only (403).
    let body = json!({"review_message_id": input, "text": "報告"});
    assert_problem(
        &send(&app, post_admin(REPORT, &body)).await,
        422,
        "cos_review_report_invalid",
    );
    assert_problem(
        &send(&app, post_admin(OPS, &report_body("human", &input, "報告"))).await,
        403,
        "cos_credential_required",
    );
    // The review run calling the route directly: 422, needs the operation envelope.
    let direct = send(
        &app,
        post_json_with(REPORT, &body, &[("authorization", &review_bearer)]),
    )
    .await;
    assert_problem(&direct, 422, "cos_audit_context_required");

    assert!(general_reports(&env).is_empty());
}

fn multipart(path: &str, key: &str) -> Request<Body> {
    let body = format!(
        "--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\n\
         Content-Type: application/octet-stream\r\n\r\nhello\r\n--boundary\r\n\
         Content-Disposition: form-data; name=\"client_upload_id\"\r\n\r\n{key}\r\n--boundary--\r\n"
    );
    Request::post(path)
        .header("host", HOST)
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "multipart/form-data; boundary=boundary")
        .body(Body::from(body))
        .expect("multipart request")
}

#[tokio::test]
async fn review_thread_upload_is_403_and_review_thread_lookup_is_404_until_it_exists() {
    let mut env = admin_env();
    env.state = env.state.clone().with_chat_attachments(
        env.dir.path().to_path_buf(),
        task_core::chat::attachments::ChatAttachmentLimits::default(),
    );
    let app = env.router();

    // Not created by the lookup.
    assert_problem(
        &send(&app, get_admin("/api/v1/cos/review-thread")).await,
        404,
        "chat_not_found",
    );
    assert_problem(
        &send(&app, get_admin("/api/v1/cos/review-thread")).await,
        404,
        "chat_not_found",
    );
    let (thread, _, _, bearer) = review_run(&env, "three");
    let found = send(&app, get_admin("/api/v1/cos/review-thread")).await;
    assert_eq!(found.status.as_u16(), 200, "{}", found.text());
    assert_eq!(found.json()["thread"]["id"], thread.as_str());
    assert_eq!(found.json()["thread"]["kind"], "review");

    // The person's upload: 403 chat_thread_read_only, nothing stored.
    let path = format!("/api/v1/chat/threads/{thread}/attachments");
    assert_problem(
        &send(&app, multipart(&path, "upload-1")).await,
        403,
        "chat_thread_read_only",
    );
    // CoS's upload through the operation: 403, recorded as rejected.
    let resp = send(
        &app,
        post_json_with(
            OPS,
            &op_body(
                "upload-cos",
                "POST",
                &path,
                json!({"client_upload_id": "upload-2", "name": "a.txt", "content_base64": "aGVsbG8="}),
            ),
            &[("authorization", &bearer)],
        ),
    )
    .await;
    assert_problem(&resp, 403, "chat_thread_read_only");
    let rows = op_rows(&env, &thread, "upload-cos");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, "rejected");
    let stored: i64 = db(&env)
        .query_row(
            "SELECT COUNT(*) FROM chat_attachments WHERE thread_id=?1",
            [&thread],
            |row| row.get(0),
        )
        .unwrap_or(0);
    assert_eq!(stored, 0);
}
