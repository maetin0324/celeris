mod common;

use common::*;
use serde_json::json;

fn admin() -> [(&'static str, &'static str); 1] {
    [("authorization", "Bearer s3cret-token-value")]
}

#[tokio::test]
async fn integration_api_lists_and_filters_states() {
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.to_string()),
        ..EnvOptions::default()
    });
    let response = send(
        &env.router(),
        get_with("/api/v1/integrations?state=unintegrated", &admin()),
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.text());
    assert_eq!(response.json()["items"], json!([]));
}

#[tokio::test]
async fn integration_api_disposition_requires_human_auth_and_nonempty_reason() {
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.to_string()),
        ..EnvOptions::default()
    });
    let path = format!(
        "/api/v1/tasks/{}/integrations/repo/discard",
        task_core::TaskId::new()
    );
    let denied = send(&env.router(), post_json(&path, &json!({"reason":"x"}))).await;
    assert_eq!(denied.status, 401);
    let empty = send(
        &env.router(),
        post_json_with(&path, &json!({"reason":"  "}), &admin()),
    )
    .await;
    assert_eq!(empty.status, 400, "{}", empty.text());
}

#[tokio::test]
async fn integration_api_rejects_unknown_state_filter() {
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.to_string()),
        ..EnvOptions::default()
    });
    let response = send(
        &env.router(),
        get_with("/api/v1/integrations?state=unknown", &admin()),
    )
    .await;
    assert_eq!(response.status, 400, "{}", response.text());
}
