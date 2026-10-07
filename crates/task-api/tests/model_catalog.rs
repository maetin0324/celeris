//! ADR 2026-10-06 D5: `/api/v1/llm/models` の API。
mod common;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use common::*;
use serde_json::json;
use task_api::{DiscoverySummaryView, ModelDiscoveryHook};
use task_core::ModelCatalogStore;
use task_core::model_catalog::{CatalogDelta, CatalogSource, DiscoveredModel};

fn seed(env: &TestEnv) {
    env.store
        .model_catalog_apply(
            &CatalogSource::new("opencode-go"),
            &[DiscoveredModel::new("glm-5"), DiscoveredModel::new("kimi")],
            1_700_000_000,
        )
        .expect("apply");
}

#[tokio::test]
async fn override_roundtrip_shows_in_list() {
    let env = admin_env();
    seed(&env);
    let app = env.router();

    let resp = send(&app, get_admin("/api/v1/llm/models")).await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let body = resp.json();
    assert_eq!(body["items"].as_array().unwrap().len(), 2);
    assert_eq!(body["items"][0]["model_id"], "glm-5");
    assert_eq!(body["items"][0]["available"], true);
    assert_eq!(body["items"][0]["first_seen"], "2023-11-14T22:13:20Z");
    assert_eq!(body["items"][0]["override"], json!(null));
    assert_eq!(
        body["items"][0]["routing"],
        json!({"tiers": [], "deployments": []})
    );
    assert_eq!(body["last_discovery"][0]["source"], "opencode-go");
    assert_eq!(body["last_discovery"][0]["ok"], true);
    assert_eq!(body["last_discovery"][0]["count"], 2);

    let resp = send(
        &app,
        put_json_with(
            "/api/v1/llm/models/opencode-go/glm-5/override",
            &json!({"disabled": true, "tier": "cheap", "alias": "g", "note": "slow"}),
            &admin_headers(),
        ),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let item = resp.json();
    assert_eq!(item["override"]["disabled"], true);
    assert_eq!(item["override"]["tier"], "cheap");

    let body = send(&app, get_admin("/api/v1/llm/models")).await.json();
    assert_eq!(body["items"][0]["override"]["alias"], "g");
    assert_eq!(body["items"][1]["override"], json!(null));

    let resp = send(
        &app,
        delete_with(
            "/api/v1/llm/models/opencode-go/glm-5/override",
            &admin_headers(),
        ),
    )
    .await;
    assert_eq!(resp.status, 204);
    let resp = send(
        &app,
        delete_with(
            "/api/v1/llm/models/opencode-go/glm-5/override",
            &admin_headers(),
        ),
    )
    .await;
    assert_problem(&resp, 404, "model_override_not_found");
    let body = send(&app, get_admin("/api/v1/llm/models")).await.json();
    assert_eq!(body["items"][0]["override"], json!(null));
}

#[tokio::test]
async fn override_rejects_unknown_source_and_bad_tier() {
    let env = admin_env();
    let app = env.router();
    let resp = send(
        &app,
        put_json_with(
            "/api/v1/llm/models/bogus/x/override",
            &json!({"disabled": true}),
            &admin_headers(),
        ),
    )
    .await;
    assert_eq!(resp.status, 400, "{}", resp.text());
    let resp = send(
        &app,
        put_json_with(
            "/api/v1/llm/models/opencode-go/x/override",
            &json!({"tier": "godlike"}),
            &admin_headers(),
        ),
    )
    .await;
    assert_eq!(resp.status, 400, "{}", resp.text());
}

#[tokio::test]
async fn discover_without_hook_is_accepted_with_unavailable() {
    let env = admin_env();
    let app = env.router();
    let resp = send(&app, post_admin("/api/v1/llm/models/discover", &json!({}))).await;
    assert_eq!(resp.status, 202, "{}", resp.text());
    assert_eq!(resp.json(), json!({"results": [], "unavailable": true}));
}

struct Hook;

impl ModelDiscoveryHook for Hook {
    fn discover<'a>(
        &'a self,
        source: Option<String>,
    ) -> Pin<Box<dyn Future<Output = Vec<DiscoverySummaryView>> + Send + 'a>> {
        Box::pin(async move {
            vec![DiscoverySummaryView {
                source: source.unwrap_or_else(|| "all".into()),
                ok: true,
                count: 3,
                error: None,
                delta: CatalogDelta::default(),
            }]
        })
    }
}

#[tokio::test]
async fn discover_calls_the_hook_with_the_source() {
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.to_string()),
        model_discovery: Some(Arc::new(Hook)),
        ..EnvOptions::default()
    });
    let app = env.router();
    let resp = send(
        &app,
        post_admin(
            "/api/v1/llm/models/discover",
            &json!({"source": "opencode-go"}),
        ),
    )
    .await;
    assert_eq!(resp.status, 202, "{}", resp.text());
    let body = resp.json();
    assert_eq!(body["unavailable"], false);
    assert_eq!(body["results"][0]["source"], "opencode-go");
    assert_eq!(body["results"][0]["count"], 3);
    let resp = send(
        &app,
        post_admin("/api/v1/llm/models/discover", &json!({"source": "nope"})),
    )
    .await;
    assert_eq!(resp.status, 400);
}
