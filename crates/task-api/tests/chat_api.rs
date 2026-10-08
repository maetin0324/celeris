//! D2 chat REST contract against a temporary SQLite database and a claimed fake run.
mod common;

use common::*;
use serde_json::{Value, json};
use task_core::chat::ChatRunState;
use time::OffsetDateTime;

const BASE: &str = "/api/v1/chat/threads";

fn thread_body(key: &str, title: &str) -> Value {
    json!({"title":title,"project_id":null,"client_thread_id":key})
}

async fn create(app: &axum::Router, key: &str, title: &str) -> Value {
    let response = send(app, post_admin(BASE, &thread_body(key, title))).await;
    assert_eq!(response.status.as_u16(), 201, "{}", response.text());
    response.json()["thread"].clone()
}

fn message_body(key: &str, text: &str) -> Value {
    json!({"client_message_id":key,"text":text,"attachment_ids":[],"reply_to_id":null,"mode":"queue","resume_queue":false})
}

#[tokio::test]
async fn chat_api_create_list_search_and_patch() {
    let env = admin_env();
    let app = env.router();
    let first = create(&app, "key-one", "Alpha moon").await;
    let id = first["id"].as_str().expect("id");
    let again = send(
        &app,
        post_admin(BASE, &thread_body("key-one", "Alpha moon")),
    )
    .await;
    assert_eq!(again.status.as_u16(), 200);
    assert_eq!(again.json()["thread"]["id"], id);
    assert_problem(
        &send(&app, post_admin(BASE, &thread_body("key-one", "other"))).await,
        409,
        "chat_conflict",
    );
    create(&app, "key-two", "Beta sun").await;
    let page = send(&app, get_admin(&format!("{BASE}?limit=1")))
        .await
        .json();
    assert_eq!(page["items"].as_array().map(Vec::len), Some(1));
    let cursor = page["next_cursor"].as_str().expect("cursor");
    let next = send(&app, get_admin(&format!("{BASE}?limit=1&before={cursor}")))
        .await
        .json();
    assert_eq!(next["items"].as_array().map(Vec::len), Some(1));
    let found = send(&app, get_admin(&format!("{BASE}?q=moon")))
        .await
        .json();
    assert_eq!(found["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(found["items"][0]["id"], id);
    let detail = send(&app, get_admin(&format!("{BASE}/{id}"))).await.json();
    assert_eq!(detail["thread"]["id"], id);
    assert_eq!(detail["last_event_id"], "1");
    let changed = send(
        &app,
        patch_admin(
            &format!("{BASE}/{id}"),
            &json!({"title":"Renamed","expected_revision":1}),
        ),
    )
    .await;
    assert_eq!(changed.status.as_u16(), 200, "{}", changed.text());
    assert_eq!(changed.json()["thread"]["title"], "Renamed");
    assert_problem(
        &send(
            &app,
            patch_admin(
                &format!("{BASE}/{id}"),
                &json!({"status":"archived","expected_revision":1}),
            ),
        )
        .await,
        409,
        "chat_conflict",
    );
    let rev = changed.json()["thread"]["revision"]
        .as_u64()
        .expect("revision");
    let archived = send(
        &app,
        patch_admin(
            &format!("{BASE}/{id}"),
            &json!({"status":"archived","expected_revision":rev}),
        ),
    )
    .await;
    assert_eq!(archived.status.as_u16(), 200);
    assert_eq!(archived.json()["thread"]["status"], "archived");
}

#[tokio::test]
async fn chat_api_post_replay_limits_and_cancel() {
    let env = admin_env();
    let app = env.router();
    let thread = create(&app, "message-thread", "Messages").await;
    let id = thread["id"].as_str().expect("id");
    let path = format!("{BASE}/{id}/messages");
    let body = message_body("one", "hello moon");
    let posted = send(&app, post_admin(&path, &body)).await;
    assert_eq!(posted.status.as_u16(), 202, "{}", posted.text());
    let result = posted.json();
    assert_eq!(result["queue_position"], 1);
    let mid = result["message"]["id"].as_str().expect("message id");
    let replay = send(&app, post_admin(&path, &body)).await;
    assert_eq!(replay.status.as_u16(), 202);
    assert_eq!(replay.json()["message"]["id"], mid);
    assert_problem(
        &send(&app, post_admin(&path, &message_body("one", "different"))).await,
        409,
        "chat_conflict",
    );
    assert_problem(
        &send(
            &app,
            post_admin(&path, &message_body("big", &"x".repeat(65537))),
        )
        .await,
        413,
        "payload_too_large",
    );
    let list = send(&app, get_admin(&path)).await.json();
    assert_eq!(list["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(list["items"][0]["id"], mid);
    assert!(list["snapshot_event_id"].is_string());
    let cancelled = send(
        &app,
        delete_with(&format!("{path}/{mid}"), &admin_headers()),
    )
    .await;
    assert_eq!(cancelled.status.as_u16(), 200);
    assert_eq!(cancelled.json()["message"]["state"], "cancelled");
    let again = send(
        &app,
        delete_with(&format!("{path}/{mid}"), &admin_headers()),
    )
    .await;
    assert_eq!(again.status.as_u16(), 200);
}

#[tokio::test]
async fn chat_api_queue_limit_and_run_stop_resume() {
    let env = admin_env();
    let app = env.router();
    let thread = create(&app, "run-thread", "Run").await;
    let id = thread["id"].as_str().expect("id").to_owned();
    let path = format!("{BASE}/{id}/messages");
    for n in 0..100 {
        let result = send(
            &app,
            post_admin(&path, &message_body(&format!("m-{n}"), "queued")),
        )
        .await;
        assert_eq!(result.status.as_u16(), 202, "{}", result.text());
    }
    assert_problem(
        &send(&app, post_admin(&path, &message_body("overflow", "queued"))).await,
        429,
        "chat_queue_full",
    );
    let revision =
        send(&app, get_admin(&format!("{BASE}/{id}"))).await.json()["thread"]["revision"]
            .as_u64()
            .expect("revision");
    assert_problem(
        &send(
            &app,
            patch_admin(
                &format!("{BASE}/{id}"),
                &json!({"status":"archived","expected_revision":revision}),
            ),
        )
        .await,
        409,
        "chat_conflict",
    );
    let claimed = env
        .store
        .chat_run_claim_next(
            &id,
            "fake-run",
            &json!({"harness":"claude-code"}),
            OffsetDateTime::now_utc(),
        )
        .expect("claim")
        .expect("run");
    assert_eq!(claimed.state, ChatRunState::Running);
    assert_problem(
        &send(
            &app,
            delete_with(
                &format!("{path}/{}", claimed.input_message_id),
                &admin_headers(),
            ),
        )
        .await,
        409,
        "chat_conflict",
    );
    let run_path = format!("{BASE}/{id}/runs/fake-run");
    let got = send(&app, get_admin(&run_path)).await;
    assert_eq!(got.status.as_u16(), 200);
    assert_eq!(got.json()["run"]["id"], "fake-run");
    let stop_path = format!("{BASE}/{id}/stop");
    let stopped = send(&app, post_admin(&stop_path, &json!({"run_id":"fake-run"}))).await;
    assert_eq!(stopped.status.as_u16(), 202, "{}", stopped.text());
    assert_eq!(stopped.json()["queue_paused"], true);
    let revision =
        send(&app, get_admin(&format!("{BASE}/{id}"))).await.json()["thread"]["revision"]
            .as_u64()
            .expect("revision");
    let resumed = send(
        &app,
        post_admin(
            &format!("{BASE}/{id}/resume-queue"),
            &json!({"expected_revision":revision}),
        ),
    )
    .await;
    assert_eq!(resumed.status.as_u16(), 200, "{}", resumed.text());
    assert_eq!(resumed.json()["thread"]["queue_paused"], false);
    env.store
        .chat_run_finish(
            "fake-run",
            ChatRunState::Stopped,
            None,
            None,
            OffsetDateTime::now_utc(),
        )
        .expect("finish fake run");
    let replay = send(&app, post_admin(&stop_path, &json!({"run_id":"fake-run"}))).await;
    assert_eq!(replay.status.as_u16(), 200);
}

#[tokio::test]
async fn chat_api_auth_and_input_errors() {
    let env = admin_env();
    let app = env.router();
    assert_problem(&send(&app, get(BASE)).await, 401, "unauthorized");
    assert_problem(
        &send(&app, post_json(BASE, &thread_body("unauth", "Title"))).await,
        401,
        "unauthorized",
    );
    assert_problem(
        &send(
            &app,
            post_json_with(
                BASE,
                &thread_body("cross-origin", "Title"),
                &[
                    ("authorization", "Bearer s3cret-token-value"),
                    ("origin", "https://example.invalid"),
                ],
            ),
        )
        .await,
        403,
        "origin_forbidden",
    );
    let mut unknown = thread_body("key", "Title");
    unknown["kind"] = json!("inbox");
    assert_problem(
        &send(&app, post_admin(BASE, &unknown)).await,
        400,
        "bad_request",
    );
    assert_problem(
        &send(
            &app,
            post_admin(
                BASE,
                &json!({"title":3,"project_id":null,"client_thread_id":"key"}),
            ),
        )
        .await,
        422,
        "validation",
    );
    assert_problem(
        &send(&app, get_admin(&format!("{BASE}/missing"))).await,
        404,
        "chat_not_found",
    );
}

#[tokio::test]
async fn chat_api_disabled_send_is_unavailable() {
    let env = admin_env();
    let app = env.router();
    let thread = create(&app, "disabled-thread", "Disabled").await;
    let id = thread["id"].as_str().expect("id");
    let disabled = task_api::router(env.state.clone().with_cos_enabled(false));
    assert_problem(
        &send(
            &disabled,
            post_admin(
                &format!("{BASE}/{id}/messages"),
                &message_body("disabled", "hello"),
            ),
        )
        .await,
        503,
        "cos_unavailable",
    );
}

#[tokio::test]
async fn chat_api_list_status_filter() {
    let env = admin_env();
    let app = env.router();
    let open = create(&app, "filter-open", "Open").await;
    let archived = create(&app, "filter-archived", "Archived").await;
    let id = archived["id"].as_str().expect("id");
    let response = send(
        &app,
        patch_admin(
            &format!("{BASE}/{id}"),
            &json!({"status":"archived","expected_revision":1}),
        ),
    )
    .await;
    assert_eq!(response.status.as_u16(), 200);
    let found = send(&app, get_admin(&format!("{BASE}?status=open")))
        .await
        .json();
    assert_eq!(found["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(found["items"][0]["id"], open["id"]);
    let found = send(&app, get_admin(&format!("{BASE}?status=archived")))
        .await
        .json();
    assert_eq!(found["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(found["items"][0]["id"], archived["id"]);
}

#[tokio::test]
async fn chat_api_search_message_body() {
    let env = admin_env();
    let app = env.router();
    let thread = create(&app, "search-body", "Plain title").await;
    let id = thread["id"].as_str().expect("id");
    assert_eq!(
        send(
            &app,
            post_admin(
                &format!("{BASE}/{id}/messages"),
                &message_body("search", "distinctive comet")
            )
        )
        .await
        .status
        .as_u16(),
        202
    );
    let found = send(&app, get_admin(&format!("{BASE}?q=comet")))
        .await
        .json();
    assert_eq!(found["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(found["items"][0]["id"], id);
}

#[tokio::test]
async fn chat_api_message_before_and_after_pages() {
    let env = admin_env();
    let app = env.router();
    let thread = create(&app, "page-messages", "Pages").await;
    let id = thread["id"].as_str().expect("id");
    let path = format!("{BASE}/{id}/messages");
    for n in 1..=3 {
        assert_eq!(
            send(
                &app,
                post_admin(
                    &path,
                    &message_body(&format!("page-{n}"), &format!("body {n}"))
                )
            )
            .await
            .status
            .as_u16(),
            202
        );
    }
    let newest = send(&app, get_admin(&format!("{path}?limit=2")))
        .await
        .json();
    assert_eq!(newest["items"][0]["seq"], 2);
    assert_eq!(newest["items"][1]["seq"], 3);
    assert_eq!(newest["next_before_seq"], 2);
    let older = send(&app, get_admin(&format!("{path}?before_seq=2")))
        .await
        .json();
    assert_eq!(older["items"][0]["seq"], 1);
    let newer = send(&app, get_admin(&format!("{path}?after_seq=1&limit=1")))
        .await
        .json();
    assert_eq!(newer["items"][0]["seq"], 2);
    assert_eq!(newer["next_after_seq"], 2);
}

#[tokio::test]
async fn chat_api_archived_thread_rejects_send() {
    let env = admin_env();
    let app = env.router();
    let thread = create(&app, "archive-send", "Archived").await;
    let id = thread["id"].as_str().expect("id");
    assert_eq!(
        send(
            &app,
            patch_admin(
                &format!("{BASE}/{id}"),
                &json!({"status":"archived","expected_revision":1})
            )
        )
        .await
        .status
        .as_u16(),
        200
    );
    assert_problem(
        &send(
            &app,
            post_admin(
                &format!("{BASE}/{id}/messages"),
                &message_body("post", "hello"),
            ),
        )
        .await,
        409,
        "chat_conflict",
    );
}

#[tokio::test]
async fn chat_api_resume_queue_stale_revision() {
    let env = admin_env();
    let app = env.router();
    let thread = create(&app, "stale-resume", "Queue").await;
    let id = thread["id"].as_str().expect("id");
    let path = format!("{BASE}/{id}/resume-queue");
    assert_problem(
        &send(&app, post_admin(&path, &json!({"expected_revision":0}))).await,
        409,
        "chat_conflict",
    );
    let response = send(&app, post_admin(&path, &json!({"expected_revision":1}))).await;
    assert_eq!(response.status.as_u16(), 200);
    assert_eq!(response.json()["thread"]["queue_paused"], false);
}

#[tokio::test]
async fn chat_api_blank_message_validation() {
    let env = admin_env();
    let app = env.router();
    let thread = create(&app, "blank-message", "Blank").await;
    let id = thread["id"].as_str().expect("id");
    assert_problem(
        &send(
            &app,
            post_admin(
                &format!("{BASE}/{id}/messages"),
                &message_body("blank", "  \n  "),
            ),
        )
        .await,
        422,
        "validation",
    );
}

#[tokio::test]
async fn chat_api_query_validation() {
    let env = admin_env();
    let app = env.router();
    let thread = create(&app, "query-errors", "Query").await;
    let id = thread["id"].as_str().expect("id");
    for path in [
        format!("{BASE}?limit=0"),
        format!("{BASE}?status=invalid"),
        format!("{BASE}?surprise=1"),
    ] {
        assert_problem(&send(&app, get_admin(&path)).await, 400, "bad_request");
    }
    let path = format!("{BASE}/{id}/messages");
    for query in ["limit=0", "before_seq=2&after_seq=1", "after_seq=bogus"] {
        assert_problem(
            &send(&app, get_admin(&format!("{path}?{query}"))).await,
            400,
            "bad_request",
        );
    }
}

#[tokio::test]
async fn chat_api_write_unknown_fields() {
    let env = admin_env();
    let app = env.router();
    let thread = create(&app, "unknown-fields", "Unknown").await;
    let id = thread["id"].as_str().expect("id");
    let mut message = message_body("extra", "hello");
    message["unexpected"] = json!(true);
    assert_problem(
        &send(&app, post_admin(&format!("{BASE}/{id}/messages"), &message)).await,
        400,
        "bad_request",
    );
    assert_problem(
        &send(
            &app,
            patch_admin(
                &format!("{BASE}/{id}"),
                &json!({"title":"changed","expected_revision":1,"unexpected":true}),
            ),
        )
        .await,
        400,
        "bad_request",
    );
    assert_problem(
        &send(
            &app,
            post_admin(
                &format!("{BASE}/{id}/stop"),
                &json!({"run_id":"r","unexpected":true}),
            ),
        )
        .await,
        400,
        "bad_request",
    );
}

#[tokio::test]
async fn chat_api_missing_resources() {
    let env = admin_env();
    let app = env.router();
    let thread = create(&app, "missing-resources", "Missing").await;
    let id = thread["id"].as_str().expect("id");
    assert_problem(
        &send(&app, get_admin(&format!("{BASE}/{id}/runs/missing"))).await,
        404,
        "chat_not_found",
    );
    assert_problem(
        &send(
            &app,
            delete_with(&format!("{BASE}/{id}/messages/missing"), &admin_headers()),
        )
        .await,
        404,
        "chat_not_found",
    );
    assert_problem(
        &send(&app, get_admin(&format!("{BASE}/missing/messages"))).await,
        404,
        "chat_not_found",
    );
}

#[tokio::test]
async fn chat_api_cross_thread_cancel_is_not_found() {
    let env = admin_env();
    let app = env.router();
    let first = create(&app, "cancel-owner", "Owner").await;
    let second = create(&app, "cancel-other", "Other").await;
    let owner = first["id"].as_str().expect("id");
    let other = second["id"].as_str().expect("id");
    let posted = send(
        &app,
        post_admin(
            &format!("{BASE}/{owner}/messages"),
            &message_body("owned", "hello"),
        ),
    )
    .await
    .json();
    let mid = posted["message"]["id"].as_str().expect("id");
    assert_problem(
        &send(
            &app,
            delete_with(&format!("{BASE}/{other}/messages/{mid}"), &admin_headers()),
        )
        .await,
        404,
        "chat_not_found",
    );
    assert_eq!(
        send(&app, get_admin(&format!("{BASE}/{owner}/messages")))
            .await
            .json()["items"][0]["state"],
        "queued"
    );
}

#[tokio::test]
async fn cos_chat_usage_api_detail_list_pagination_and_missing_usage() {
    let env = admin_env();
    let app = env.router();
    let t = create(&app, "usage-thread", "usage").await;
    let t = t["id"].as_str().expect("thread");
    let start = OffsetDateTime::from_unix_timestamp(1_791_158_400).expect("clock");
    for (id, usage) in [
        (
            "usage-run",
            Some(task_core::Usage {
                input_tokens: Some(9),
                output_tokens: Some(3),
                cache_read_tokens: Some(17),
                cache_creation_tokens: Some(5),
                cost_usd: Some(0.004),
                duplicate_reads: Some(1),
                session_resumed: Some(true),
            }),
        ),
        ("no-usage-run", None),
    ] {
        assert_eq!(
            send(
                &app,
                post_admin(&format!("{BASE}/{t}/messages"), &message_body(id, id))
            )
            .await
            .status
            .as_u16(),
            202
        );
        env.store
            .chat_run_claim_next(
                t,
                id,
                &json!({"harness":"fake","model":"lab","session_mode":"resumed"}),
                start,
            )
            .expect("claim");
        env.store
            .chat_run_finish_with_telemetry(
                id,
                ChatRunState::Completed,
                Some("reply"),
                None,
                start + time::Duration::seconds(4),
                usage.as_ref(),
                2,
                Some(start + time::Duration::seconds(1)),
            )
            .expect("finish");
    }
    let detail = send(&app, get_admin(&format!("{BASE}/{t}/runs/usage-run")))
        .await
        .json();
    assert_eq!(detail["run"]["usage"]["cache_read_tokens"], 17);
    assert_eq!(detail["run"]["usage"]["cache_creation_tokens"], 5);
    assert_eq!(detail["run"]["usage"]["cost_usd"], 0.004);
    assert_eq!(detail["run"]["usage"]["session_resumed"], true);
    assert_eq!(detail["run"]["usage"]["duplicate_reads"], 1);
    assert_eq!(detail["run"]["skill_reads"], 2);
    assert_eq!(detail["run"]["latency_ms"], 4000);
    assert_eq!(detail["run"]["time_to_first_output_ms"], 1000);
    assert_eq!(detail["run"]["model"], "lab");
    let missing = send(&app, get_admin(&format!("{BASE}/{t}/runs/no-usage-run")))
        .await
        .json();
    assert!(missing["run"].get("usage").is_none());
    let page = send(&app, get_admin(&format!("{BASE}/{t}/runs?limit=1")))
        .await
        .json();
    assert_eq!(page["items"][0], missing["run"]);
    let cursor = page["next_before"].as_str().expect("cursor");
    let next = send(
        &app,
        get_admin(&format!("{BASE}/{t}/runs?before={cursor}&limit=1")),
    )
    .await
    .json();
    assert_eq!(next["items"][0], detail["run"]);
    assert!(next["next_before"].is_null());
    assert_problem(
        &send(&app, get_admin(&format!("{BASE}/{t}/runs?before=unknown"))).await,
        400,
        "bad_request",
    );
}
