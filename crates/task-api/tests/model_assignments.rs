//! ADR 2026-10-06 model-role-assignments D4/D6: `/api/v1/llm/models/assignments` の API。
mod common;

use std::sync::Arc;

use common::*;
use serde_json::{Value, json};
use task_api::routing_catalog::CatalogDeploymentView;
use task_api::types::ProviderConfigView;
use task_api::{RoutingCatalogReader, RoutingCatalogView};
use task_core::model_catalog::catalog_event_task_id;
use task_core::model_catalog::{CatalogOverride, CatalogSource, DiscoveredModel};
use task_core::model_router::{policy::RoutingMode, profiles::Billing};
use task_core::{Event, ModelCatalogStore, TaskStore, Tier};

/// routing catalog: opencode-go の provider（`oc`）と proxy の cheap lane。`.0` は proxy lane の upstream model
/// （割り当てを反映済みの hook を真似るときは割り当てと同じ値にする）。provider の config 値は provider の一覧から取る。
struct Catalog(&'static str);

impl RoutingCatalogReader for Catalog {
    fn view(&self) -> RoutingCatalogView {
        let dep =
            |id: &str, source_ref: &str, upstream: &str, lanes: Vec<Tier>| CatalogDeploymentView {
                id: id.into(),
                source_ref: source_ref.into(),
                model_profile_id: upstream.into(),
                upstream_model: upstream.into(),
                billing: Billing::Subscription,
                allowed_lanes: lanes,
                price_override: None,
            };
        RoutingCatalogView {
            catalog_version: "test".into(),
            mode: RoutingMode::Legacy,
            models: vec![],
            deployments: vec![
                dep(
                    "provider:oc",
                    "opencode-go",
                    "opencode-go/kimi",
                    vec![Tier::Standard, Tier::Cheap],
                ),
                dep(
                    "provider:oc/cheap",
                    "opencode-go",
                    "opencode-go/glm-5",
                    vec![Tier::Cheap],
                ),
                dep(
                    "legacy:opencode_go:Cheap",
                    "opencode_go",
                    self.0,
                    vec![Tier::Cheap],
                ),
            ],
            policies: vec![],
            warnings: vec![],
        }
    }
}

/// opencode-go の provider 行（config: standard は `model`、cheap は `tier_models`）。
fn oc_provider() -> ProviderConfigView {
    let mut tier_models = task_core::model_routing::TierModels::new();
    tier_models.insert(
        Tier::Cheap,
        task_core::model_routing::ModelBinding {
            name: "glm-5".into(),
            model_id: Some("opencode-go/glm-5".into()),
            unavailable_reason: None,
            reasoning_effort: None,
        },
    );
    ProviderConfigView {
        kind: Default::default(),
        llm_source: Some(task_core::ResolvedLlmSource {
            source: task_core::LlmSourceRef::OpencodeGo,
            origin: task_core::SourceOrigin::Explicit,
        }),
        credential_refs: Default::default(),
        tier_models,
        account_id: None,
        id: "oc".into(),
        adapter: "acp".into(),
        tiers: vec![Tier::Standard, Tier::Cheap],
        concurrency: 1,
        model: Some("opencode-go/kimi".into()),
        env_keys: vec![],
        account_pool: true,
    }
}

fn env() -> TestEnv {
    env_with(Catalog("kimi"))
}

fn env_with(catalog: Catalog) -> TestEnv {
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.to_string()),
        routing_catalog: Some(Arc::new(catalog)),
        extra_providers: vec![oc_provider()],
        ..EnvOptions::default()
    });
    env.store
        .model_catalog_apply(
            &CatalogSource::new("opencode-go"),
            &[
                DiscoveredModel::new("glm-5"),
                DiscoveredModel::new("kimi"),
                DiscoveredModel::new("qwen"),
            ],
            1_700_000_000,
        )
        .expect("apply");
    env
}

const BASE: &str = "/api/v1/llm/models/assignments";

fn slot<'a>(body: &'a Value, source: &str, tier: &str) -> &'a Value {
    body["effective"]
        .as_array()
        .and_then(|a| {
            a.iter()
                .find(|s| s["source"] == source && s["tier"] == tier)
        })
        .unwrap_or_else(|| panic!("no slot {source}/{tier} in {body}"))
}

#[tokio::test]
async fn empty_list_shows_config_origin_slots() {
    let env = env();
    let app = env.router();
    let resp = send(&app, get_admin(BASE)).await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let body = resp.json();
    assert_eq!(body["items"], json!([]));
    // opencode-go は 3 tier 分の枠が全部ある（他の source は test env の provider 由来）。
    let n = body["effective"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["source"] == "opencode-go")
        .count();
    assert_eq!(n, 4); // Both config models of the cheap role are retained.
    let cheap = slot(&body, "opencode-go", "cheap");
    assert_eq!(cheap["model_id"], "glm-5");
    assert_eq!(cheap["origin"], "config");
    assert_eq!(cheap["providers"], json!(["oc"]));
    assert_eq!(cheap["proxy"], true);
    assert_eq!(cheap["available"], true);
    let standard = slot(&body, "opencode-go", "standard");
    assert_eq!(standard["model_id"], "kimi");
    assert_eq!(standard["proxy"], false);
    let frontier = slot(&body, "opencode-go", "frontier");
    assert_eq!(frontier["model_id"], json!(null));
    assert_eq!(frontier["origin"], json!(null));
    assert_eq!(frontier["providers"], json!([]));
}

#[tokio::test]
async fn put_assigns_records_an_event_and_reports_impact() {
    let env = env();
    let app = env.router();
    let resp = send(
        &app,
        put_json_with(
            &format!("{BASE}/opencode-go/cheap"),
            &json!({"model_id": "qwen", "note": "trial"}),
            &admin_headers(),
        ),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let body = resp.json();
    assert_eq!(body["item"]["source"], "opencode-go");
    assert_eq!(body["item"]["tier"], "cheap");
    assert_eq!(body["item"]["model_id"], "qwen");
    assert_eq!(body["item"]["state"], "assigned");
    assert_eq!(body["item"]["note"], "trial");
    assert_eq!(body["item"]["updated_by"], "admin");
    let changes = body["impact"]["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0]["kind"], "provider");
    assert_eq!(changes[0]["id"], "oc");
    assert_eq!(changes[0]["before"], "glm-5");
    assert_eq!(changes[0]["after"], "qwen");
    assert_eq!(changes[1]["kind"], "proxy");
    assert_eq!(changes[1]["before"], "kimi");

    let events: Vec<_> = env
        .store
        .events_for(catalog_event_task_id())
        .expect("events")
        .into_iter()
        .filter_map(|(_, event)| match event {
            Event::ModelRoleAssignmentChanged {
                source,
                model_id,
                previous,
                actor,
                ..
            } => Some((source, model_id, previous, actor)),
            _ => None,
        })
        .collect();
    assert_eq!(
        events,
        vec![(
            "opencode-go".to_string(),
            Some("qwen".to_string()),
            None,
            "admin".to_string()
        )]
    );

    let body = send(&app, get_admin(BASE)).await.json();
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    let cheap = slot(&body, "opencode-go", "cheap");
    assert_eq!(cheap["model_id"], "qwen");
    assert_eq!(cheap["origin"], "assignment");

    let models = send(&app, get_admin("/api/v1/llm/models")).await.json();
    let by_id = |id: &str| {
        models["items"]
            .as_array()
            .and_then(|a| a.iter().find(|i| i["model_id"] == id))
            .cloned()
            .unwrap_or(Value::Null)
    };
    assert_eq!(by_id("qwen")["assigned_tiers"], json!(["cheap"]));
    assert_eq!(by_id("glm-5")["assigned_tiers"], json!([]));
}

#[tokio::test]
async fn put_rejects_unknown_model_source_and_tier() {
    let env = env();
    let app = env.router();
    let put = |path: &str, body: Value| put_json_with(path, &body, &admin_headers());
    let resp = send(
        &app,
        put(
            &format!("{BASE}/opencode-go/cheap"),
            json!({"model_id": "nope"}),
        ),
    )
    .await;
    assert_problem(&resp, 400, "model_not_in_catalog");
    let resp = send(
        &app,
        put(&format!("{BASE}/bogus/cheap"), json!({"model_id": "glm-5"})),
    )
    .await;
    assert_eq!(resp.status, 400, "{}", resp.text());
    let resp = send(
        &app,
        put(
            &format!("{BASE}/opencode-go/godlike"),
            json!({"model_id": "glm-5"}),
        ),
    )
    .await;
    assert_eq!(resp.status, 400, "{}", resp.text());
    let resp = send(
        &app,
        put(
            &format!("{BASE}/opencode-go/cheap"),
            json!({"model_id": "glm-5", "extra": 1}),
        ),
    )
    .await;
    assert_eq!(resp.status, 400, "{}", resp.text());
    assert!(env.store.model_role_assignments().expect("rows").is_empty());
}

#[tokio::test]
async fn delete_removes_then_404() {
    let env = env();
    let app = env.router();
    let path = format!("{BASE}/opencode-go/standard");
    let resp = send(
        &app,
        put_json_with(&path, &json!({"model_id": "glm-5"}), &admin_headers()),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let resp = send(&app, delete_with(&path, &admin_headers())).await;
    assert_eq!(resp.status, 204);
    let resp = send(&app, delete_with(&path, &admin_headers())).await;
    assert_problem(&resp, 404, "model_assignment_not_found");
    let body = send(&app, get_admin(BASE)).await.json();
    assert_eq!(slot(&body, "opencode-go", "standard")["origin"], "config");
}

#[tokio::test]
async fn excluded_assignment_reports_reason_and_preview_does_not_write() {
    let env = env();
    env.store
        .model_catalog_set_override(
            &CatalogSource::new("opencode-go"),
            "qwen",
            &CatalogOverride {
                disabled: true,
                ..CatalogOverride::default()
            },
            1_700_000_100,
        )
        .expect("override");
    let app = env.router();
    let resp = send(
        &app,
        post_admin(
            &format!("{BASE}/preview"),
            &json!({"source": "opencode-go", "tier": "cheap", "model_id": "qwen"}),
        ),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let changes = resp.json()["impact"]["changes"].clone();
    assert_eq!(changes[0]["kind"], "provider");
    assert_eq!(changes[0]["before"], "glm-5");
    assert_eq!(changes[0]["after"], "qwen");
    assert_eq!(changes[0]["excluded_reason"], "override:disabled");
    assert_eq!(changes[1]["kind"], "proxy");
    assert!(env.store.model_role_assignments().expect("rows").is_empty());

    // 置くと Excluded として見える。
    let resp = send(
        &app,
        put_json_with(
            &format!("{BASE}/opencode-go/cheap"),
            &json!({"model_id": "qwen"}),
            &admin_headers(),
        ),
    )
    .await;
    assert_eq!(resp.json()["item"]["state"], "excluded");
    let body = send(&app, get_admin(BASE)).await.json();
    assert_eq!(body["items"][0]["excluded_reason"], "override:disabled");
    assert_eq!(
        slot(&body, "opencode-go", "cheap")["excluded_reason"],
        "override:disabled"
    );

    // 解除のプレビューは config の値へ戻る。
    let resp = send(
        &app,
        post_admin(
            &format!("{BASE}/preview"),
            &json!({"source": "opencode-go", "tier": "cheap", "model_id": null}),
        ),
    )
    .await;
    let changes = resp.json()["impact"]["changes"].clone();
    assert_eq!(changes[0]["before"], "qwen");
    assert_eq!(changes[0]["after"], "glm-5");

    let resp = send(
        &app,
        post_admin(
            &format!("{BASE}/preview"),
            &json!({"source": "opencode-go", "tier": "cheap", "model_id": "nope"}),
        ),
    )
    .await;
    assert_problem(&resp, 400, "model_not_in_catalog");
}

/// provider の config 値は provider の一覧から取る。routing hook の catalog が割り当てを既に反映していても、
/// 割り当て中の枠は origin = assignment、解除のプレビューの provider の `after` は config の値。
/// proxy の lane は config の値を判別できないので `after = null`。
#[tokio::test]
async fn provider_config_value_survives_a_catalog_that_reflects_the_assignment() {
    // hook は既に割り当て（qwen）を反映している。provider（`oc`）の config は cheap = glm-5。
    let env = env_with(Catalog("qwen"));
    let app = env.router();
    let resp = send(
        &app,
        put_json_with(
            &format!("{BASE}/opencode-go/cheap"),
            &json!({"model_id": "qwen"}),
            &admin_headers(),
        ),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let body = send(&app, get_admin(BASE)).await.json();
    let cheap = slot(&body, "opencode-go", "cheap");
    assert_eq!(cheap["model_id"], "qwen");
    assert_eq!(cheap["origin"], "assignment");

    let resp = send(
        &app,
        post_admin(
            &format!("{BASE}/preview"),
            &json!({"source": "opencode-go", "tier": "cheap", "model_id": null}),
        ),
    )
    .await;
    let changes = resp.json()["impact"]["changes"].clone();
    assert_eq!(changes[0]["kind"], "provider");
    assert_eq!(changes[0]["before"], "qwen");
    assert_eq!(changes[0]["after"], "glm-5");
    assert_eq!(changes[1]["kind"], "proxy");
    assert_eq!(changes[1]["after"], json!(null));

    // 解除すると config の値（glm-5）に戻る。
    let resp = send(
        &app,
        delete_with(&format!("{BASE}/opencode-go/cheap"), &admin_headers()),
    )
    .await;
    assert_eq!(resp.status, 204);
    let body = send(&app, get_admin(BASE)).await.json();
    let cheap = slot(&body, "opencode-go", "cheap");
    assert_eq!(cheap["model_id"], "glm-5");
    assert_eq!(cheap["origin"], "config");
}

#[tokio::test]
async fn role_members_api_preview_multiple_roles_and_empty_no_config_revival() {
    let env = env();
    let app = env.router();
    let put_admin = |path: &str, body: Value| put_json_with(path, &body, &admin_headers());
    let path = format!("{BASE}/roles/standard");
    let members = json!({"members": [
        {"source":"opencode-go","model_id":"glm-5","priority":5},
        {"source":"opencode-go","model_id":"kimi","priority":1}
    ]});
    let response = send(&app, post_admin(&format!("{path}/preview"), &members)).await;
    assert_eq!(response.status, 200, "{}", response.text());
    assert_eq!(response.json()["after"].as_array().unwrap().len(), 2);
    assert!(env.store.model_role_assignments().unwrap().is_empty());
    let response = send(&app, put_admin(&path, members.clone())).await;
    assert_eq!(response.status, 200, "{}", response.text());
    let response = send(&app, put_admin(&format!("{BASE}/roles/frontier"), members)).await;
    assert_eq!(response.status, 200, "{}", response.text());
    let response = send(&app, get_admin(BASE)).await;
    let body = response.json();
    assert_eq!(body["items"].as_array().unwrap().len(), 4);
    assert_eq!(body["items"][0]["priority"], 1);
    let response = send(&app, put_admin(&path, json!({"members":[]}))).await;
    assert_eq!(response.status, 200, "{}", response.text());
    let response = send(&app, get_admin(BASE)).await;
    assert_eq!(
        slot(&response.json(), "opencode-go", "standard")["model_id"],
        Value::Null
    );
    let response = send(
        &app,
        put_admin(
            &path,
            json!({"members":[{"source":"opencode-go","model_id":"unknown","priority":0}]}),
        ),
    )
    .await;
    assert_eq!(response.status, 400);
    assert_eq!(env.store.model_role_assignments().unwrap().len(), 2);
}
