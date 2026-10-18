//! api.md §8.9（daemon の watch）、`GET /clusters`（ADR-0018 受け入れ条件8）、`GET /config`、
//! api.md §8.10（`GET /schema`）。

mod common;

use std::time::Duration;

use common::*;
use serde_json::{Value, json};
use task_core::{Status, TaskKind};

#[tokio::test]
async fn daemon_view_is_null_until_a_snapshot_is_sent() {
    let env = TestEnv::new();
    let app = env.router();

    let before = send(&app, get("/api/v1/daemon")).await;
    assert_eq!(before.status, 200);
    let body = before.json();
    assert!(body["snapshot"].is_null());
    assert!(body["now"].as_str().is_some_and(|s| s.ends_with('Z')));

    let snapshot = snapshot(7);
    env.daemon_tx
        .send(Some(snapshot.clone()))
        .expect("send snapshot");
    let after = send(&app, get("/api/v1/daemon")).await.json();
    // ADR-0033 D3（Phase 25）: `reports` だけは API が応答を組むときに埋める（未読が無ければ通知もしない）。
    let mut expected = serde_json::to_value(&snapshot).expect("json");
    expected["reports"] =
        serde_json::json!({"unread_secretary": 0, "unread_bad_news": 0, "notify_now": false});
    assert_eq!(after["snapshot"], expected);
    assert_eq!(after["snapshot"]["cooldowns"][0]["reason"], "throttled");
    assert_eq!(after["snapshot"]["in_flight"][0]["kind"], "worker");
}

#[tokio::test]
async fn stream_sends_daemon_events_when_the_snapshot_changes() {
    let env = TestEnv::new();
    let task = new_task(TaskKind::Execute, Status::Draft);
    env.seed(&task);
    let app = env.router();

    let mut sse = open_stream(&app, get(&format!("/api/v1/stream?task_id={}", task.id))).await;
    let hello = sse.next_frame(EVENT_WAIT).await.expect("hello");
    assert!(hello.data["daemon"].is_null());

    let first = snapshot(1);
    env.daemon_tx.send(Some(first.clone())).expect("send");
    let frame = sse
        .next_named("daemon", EVENT_WAIT)
        .await
        .expect("daemon event");
    assert_eq!(frame.data, serde_json::to_value(&first).expect("json"));
    assert!(frame.id.is_none());

    let second = snapshot(2);
    env.daemon_tx.send(Some(second.clone())).expect("send");
    let frame = sse
        .next_named("daemon", EVENT_WAIT)
        .await
        .expect("daemon event");
    assert_eq!(frame.data["ticks"], 2);

    let mut late = open_stream(&app, get("/api/v1/stream")).await;
    let hello = late.next_frame(EVENT_WAIT).await.expect("hello");
    assert_eq!(
        hello.data["daemon"],
        serde_json::to_value(&second).expect("json")
    );
    assert!(
        late.next_named("daemon", Duration::from_millis(300))
            .await
            .is_none(),
        "unchanged snapshot is not resent"
    );
}

#[tokio::test]
async fn clusters_combine_config_and_snapshot() {
    let env = TestEnv::new();
    let app = env.router();

    let before = send(&app, get("/api/v1/clusters")).await;
    assert_eq!(before.status, 200, "{}", before.text());
    let text = before.text();
    assert!(!text.contains("\"env\""), "{text}");
    assert!(!text.contains("\"setup\""), "{text}");
    let items = before.json()["items"].as_array().cloned().expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], "pegasus");
    assert_eq!(items[0]["host"], "pegasus");
    assert_eq!(items[0]["concurrency"], 2);
    assert_eq!(items[0]["sync"], "rsync");
    assert_eq!(items[0]["delete_on_push"], false);
    assert_eq!(items[0]["has_setup"], true);
    assert_eq!(items[0]["env_keys"], json!(["OMP_NUM_THREADS"]));
    assert_eq!(items[0]["rsync_excludes"], json!([".git/"]));
    assert!(items[0]["in_use"].is_null(), "no snapshot yet");
    assert!(items[0]["connected"].is_null(), "no snapshot yet");
    assert!(items[0]["cooldown_until"].is_null(), "no snapshot yet");
    assert!(
        items[0]["cooldown_remaining_secs"].is_null(),
        "no snapshot yet"
    );

    env.daemon_tx
        .send(Some(snapshot(1)))
        .expect("send snapshot");
    let after = send(&app, get("/api/v1/clusters")).await;
    let items = after.json()["items"].as_array().cloned().expect("items");
    assert_eq!(items[0]["in_use"], 1);
    assert_eq!(items[0]["connected"], false);
    assert_eq!(items[0]["cooldown_until"], "2099-01-01T00:00:00Z");
    assert!(
        items[0]["cooldown_remaining_secs"]
            .as_u64()
            .expect("remaining secs")
            > 0,
        "{}",
        items[0]
    );
}

#[tokio::test]
async fn clusters_endpoint_rejects_query_parameters() {
    let env = TestEnv::new();
    let app = env.router();
    let resp = send(&app, get("/api/v1/clusters?x=1")).await;
    assert_problem(&resp, 400, "bad_request");
}

/// daemon-identity-api: `identity` は認証付き `GET /config` だけに載り、無認証の `/health` には出ない。
/// 応答のどこにも token の値は出ない。
#[tokio::test]
async fn config_identity_is_authenticated_only_and_has_no_secret() {
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        ..Default::default()
    });
    let app = env.router();
    let anon = send(&app, get("/api/v1/config")).await;
    assert_eq!(anon.status, 401, "{}", anon.text());
    let auth = format!("Bearer {TOKEN}");
    let resp = send(
        &app,
        get_with("/api/v1/config", &[("authorization", &auth)]),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let id = resp.json()["identity"].clone();
    assert_eq!(id["instance_id"], "01J00000000000000000000000");
    assert!(id["effective"]["api_listen"].is_string());
    assert!(!resp.text().contains(TOKEN), "{}", resp.text());
    let health = send(&app, get("/api/v1/health")).await;
    assert_eq!(health.status, 200);
    assert!(health.json().get("identity").is_none());
    assert!(!health.text().contains("selfdeploy"), "{}", health.text());
}

#[tokio::test]
async fn config_is_returned_as_given_and_secrets_never_appear() {
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        ..Default::default()
    });
    let app = env.router();
    let auth = format!("Bearer {TOKEN}");
    let resp = send(
        &app,
        get_with("/api/v1/config", &[("authorization", &auth)]),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    assert_eq!(
        resp.json(),
        serde_json::to_value(config_view()).expect("json")
    );
    assert_eq!(resp.json()["clusters"][0]["has_setup"], true);
    let text = resp.text();
    assert!(!text.contains("\"setup\""), "{text}");
    assert!(!text.contains("\"env\""), "{text}");

    for path in [
        "/api/v1/config",
        "/api/v1/clusters",
        "/api/v1/daemon",
        "/api/v1/health",
        "/api/v1/events",
    ] {
        let resp = send(&app, get_with(path, &[("authorization", &auth)])).await;
        assert_eq!(resp.status, 200, "{path}");
        assert!(!resp.text().contains(TOKEN), "{path} leaks the token");
    }
    let unauthorized = send(&app, get("/api/v1/config")).await;
    assert!(!unauthorized.text().contains(TOKEN));
}

#[tokio::test]
async fn schema_endpoint_returns_the_committed_file() {
    let env = TestEnv::new();
    let resp = send(&env.router(), get("/api/v1/schema")).await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.header("content-type"), Some("application/schema+json"));
    let committed = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/api/v1/api-v1.schema.json"
    ))
    .expect("committed schema");
    assert_eq!(resp.text(), committed);
    assert_eq!(resp.text(), task_api::API_V1_SCHEMA_JSON);
    let value: Value = resp.json();
    assert_eq!(value["title"], "ApiV1Schema");
    assert!(value["$defs"].is_object());
}
