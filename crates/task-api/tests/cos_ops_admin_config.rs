//! ADR 2026-10-09-cos-operations-all-mutations D3 (WU ops-admin-config): the admin routes run
//! through `/cos/operations`. Each is refused when called directly with the CoS credential (422 +
//! rejected audit row) and applied through the envelope with its row, audit event and chat card.
//! Daemon-channel and external operations (C) use fakes: the daemon admin channel, the release
//! source and the discovery hook are test doubles, so nothing outside the test process is touched.
mod common;

use common::cos_ops::{OPS, cos_bearer, db, op_body, run_domain};
use common::*;
use serde_json::{Value, json};
use task_core::model_catalog::{CatalogSource, DiscoveredModel};
use task_core::{ModelCatalogStore, Tier};

fn rejected_count(env: &TestEnv, thread: &str) -> i64 {
    db(env)
        .query_row(
            "SELECT COUNT(*) FROM cos_operations WHERE thread_id=?1 AND state='rejected'",
            [thread],
            |row| row.get(0),
        )
        .expect("count")
}

/// Send `(key, method, path, body, status)` through the envelope on one thread and expect each
/// to be refused with `status` and recorded as a rejected row.
async fn expect_rejected(
    env: &TestEnv,
    thread_key: &str,
    cases: Vec<(&str, &str, &str, Value, u16)>,
) {
    let app = env.router();
    let (thread, _, bearer) = cos_bearer(env, thread_key);
    let headers = [("authorization", bearer.as_str())];
    let n = cases.len() as i64;
    for (key, method, path, body, status) in cases {
        let resp = send(
            &app,
            post_json_with(OPS, &op_body(key, method, path, body), &headers),
        )
        .await;
        assert_eq!(resp.status.as_u16(), status, "{key}: {}", resp.text());
    }
    assert_eq!(rejected_count(env, &thread), n, "{thread_key}");
}

#[tokio::test]
async fn cos_ops_admin_config_llm_models_are_audited() {
    let env = admin_env();
    let source = CatalogSource::new("opencode-go");
    env.store
        .model_catalog_apply(
            &source,
            &[
                DiscoveredModel::new("glm-5"),
                DiscoveredModel::new("kimi-k3"),
            ],
            1_700_000_000,
        )
        .expect("catalog");

    let op = run_domain(
        &env,
        "assign-preview",
        "POST",
        "/api/v1/llm/models/assignments/preview",
        json!({"source": "opencode-go", "tier": "cheap", "model_id": "glm-5"}),
        "model_assignment.preview",
    )
    .await;
    assert!(op["result"]["impact"]["changes"].is_array(), "{op}");
    assert!(env.store.model_role_assignments().expect("rows").is_empty());

    let members = json!({"members": [
        {"source": "opencode-go", "model_id": "glm-5", "priority": 0},
        {"source": "opencode-go", "model_id": "kimi-k3", "priority": 1}
    ]});
    let op = run_domain(
        &env,
        "role-preview",
        "POST",
        "/api/v1/llm/models/assignments/roles/standard/preview",
        members.clone(),
        "model_role.preview",
    )
    .await;
    assert_eq!(op["result"]["after"].as_array().map(Vec::len), Some(2));
    assert!(env.store.model_role_assignments().expect("rows").is_empty());

    run_domain(
        &env,
        "role-replace",
        "PUT",
        "/api/v1/llm/models/assignments/roles/standard",
        members,
        "model_role.replace",
    )
    .await;
    let rows = env.store.model_role_assignments().expect("rows");
    assert_eq!(
        rows.iter()
            .filter(|a| a.tier == Tier::Standard && a.updated_by == "cos")
            .count(),
        2
    );

    let op = run_domain(
        &env,
        "override-put",
        "PUT",
        "/api/v1/llm/models/opencode-go/glm-5/override",
        json!({"disabled": true, "note": "止める"}),
        "model_override.put",
    )
    .await;
    assert_eq!(op["result"]["override"]["disabled"], true);
    let overrides = env.store.model_catalog_overrides().expect("overrides");
    assert!(overrides.iter().any(|o| o.model_id == "glm-5"));

    run_domain(
        &env,
        "override-delete",
        "DELETE",
        "/api/v1/llm/models/opencode-go/glm-5/override",
        json!(null),
        "model_override.delete",
    )
    .await;
    assert!(
        env.store
            .model_catalog_overrides()
            .expect("overrides")
            .is_empty()
    );

    expect_rejected(
        &env,
        "llm-rejected",
        vec![
            (
                "unknown-model",
                "PUT",
                "/api/v1/llm/models/assignments/roles/cheap",
                json!({"members": [{"source": "opencode-go", "model_id": "nope", "priority": 0}]}),
                400,
            ),
            (
                "bad-tier",
                "PUT",
                "/api/v1/llm/models/assignments/roles/huge",
                json!({"members": []}),
                400,
            ),
            (
                "no-override",
                "DELETE",
                "/api/v1/llm/models/opencode-go/glm-5/override",
                json!(null),
                404,
            ),
        ],
    )
    .await;
}
