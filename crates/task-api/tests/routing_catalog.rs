mod common;

use std::sync::Arc;

use common::*;
use task_api::routing_catalog::{CatalogCapabilitiesView, CatalogDeploymentView, CatalogModelView};
use task_api::{
    LlmSourceView, LlmSourcesReader, LlmSourcesView, RoutingCatalogReader, RoutingCatalogView,
};
use task_core::model_router::{
    policy::{RoutingMode, RoutingPolicy},
    profiles::{Billing, ContextLimits},
};
use task_core::{Status, TaskKind, Tier};

struct Catalog;

impl RoutingCatalogReader for Catalog {
    fn view(&self) -> RoutingCatalogView {
        RoutingCatalogView {
            catalog_version: "phase1-v1".into(),
            mode: RoutingMode::Legacy,
            models: vec![CatalogModelView {
                id: "qwen".into(),
                revision: "unknown".into(),
                family: "qwen".into(),
                capabilities: CatalogCapabilitiesView {
                    tools: None,
                    structured_output: None,
                    vision: None,
                    streaming: None,
                    reasoning_efforts: None,
                },
                context_limits: ContextLimits {
                    input: None,
                    output: None,
                    total: None,
                },
                quality: None,
                pricing: None,
            }],
            deployments: vec![CatalogDeploymentView {
                id: "qwen-local".into(),
                source_ref: "openai_compatible:qwen".into(),
                model_profile_id: "qwen".into(),
                upstream_model: "qwen3".into(),
                billing: Billing::SelfHosted,
                allowed_lanes: vec![Tier::Cheap],
                price_override: None,
            }],
            policies: vec![RoutingPolicy::defaults(Tier::Cheap, RoutingMode::Legacy)],
            warnings: vec!["legacy model mapping".into()],
        }
    }
}

struct Sources;

#[async_trait::async_trait]
impl LlmSourcesReader for Sources {
    async fn view(&self, _now: i64) -> LlmSourcesView {
        LlmSourcesView {
            sources: vec![LlmSourceView {
                id: "openai_compatible:qwen".into(),
                kind: "openai-compatible".into(),
                enabled: true,
                reachable: None,
                unreachable_reason: None,
                accounts: vec![],
                last_hour_requests: 2,
                last_hour_prompt_tokens: 3,
                last_hour_completion_tokens: 4,
            }],
            celeris_tiers: vec![],
        }
    }
}

fn authed(path: &str) -> axum::http::Request<axum::body::Body> {
    get_with(path, &[("authorization", &format!("Bearer {TOKEN}"))])
}

#[tokio::test]
async fn routing_catalog_redacts_secrets_and_keeps_legacy_fields() {
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        routing_catalog: Some(Arc::new(Catalog)),
        llm_sources: Some(Arc::new(Sources)),
        ..Default::default()
    });
    let task = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&task);
    let app = env.router();

    let resp = send(&app, authed("/api/v1/llm/routing/catalog")).await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let body = resp.json();
    assert_eq!(body["mode"], "legacy");
    assert_eq!(body["catalog_version"], "phase1-v1");
    assert_eq!(body["models"][0]["quality"], serde_json::Value::Null);
    assert_eq!(
        body["models"][0]["capabilities"]["tools"],
        serde_json::Value::Null
    );
    assert_eq!(body["models"][0]["pricing"], serde_json::Value::Null);
    assert_eq!(
        body["models"][0]["context_limits"]["input"],
        serde_json::Value::Null
    );
    assert_eq!(
        body["deployments"][0]["price_override"],
        serde_json::Value::Null
    );
    for secret_key in [
        "token",
        "credential",
        "accounts_dir",
        "endpoint",
        "api_key",
        "host",
    ] {
        assert!(
            !resp.text().contains(&format!("\"{secret_key}\"")),
            "{secret_key}"
        );
    }
    assert_eq!(body["policies"][0]["lane"], "cheap");
    assert_eq!(body["warnings"][0], "legacy model mapping");

    let source = send(&app, authed("/api/v1/llm/sources")).await;
    assert_eq!(source.status, 200);
    assert_eq!(source.json()["sources"][0]["last_hour_requests"], 2);
    assert_eq!(source.json()["sources"][0]["kind"], "openai-compatible");

    let routing = send(&app, authed(&format!("/api/v1/tasks/{}/routing", task.id))).await;
    assert_eq!(routing.status, 200);
    assert_eq!(routing.json()["task_id"], task.id.to_string());
    assert_eq!(routing.json()["runs"], serde_json::json!([]));
}

#[tokio::test]
async fn catalog_requires_auth_and_rejects_query() {
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        routing_catalog: Some(Arc::new(Catalog)),
        ..Default::default()
    });
    let app = env.router();
    assert_eq!(
        send(&app, get("/api/v1/llm/routing/catalog")).await.status,
        401
    );
    assert_eq!(
        send(&app, authed("/api/v1/llm/routing/catalog?x=1"))
            .await
            .status,
        400
    );
}
