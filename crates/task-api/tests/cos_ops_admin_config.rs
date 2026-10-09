//! ADR 2026-10-09-cos-operations-all-mutations D3 (WU ops-admin-config): the admin routes run
//! through `/cos/operations`. Each is refused when called directly with the CoS credential (422 +
//! rejected audit row) and applied through the envelope with its row, audit event and chat card.
//! Daemon-channel and external operations (C) use fakes: the daemon admin channel, the release
//! source and the discovery hook are test doubles, so nothing outside the test process is touched.
mod common;

use common::cos_ops::{OPS, cos_bearer, db, op_body, run_domain, run_external};
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

const SKILL_MD: &str =
    "---\nname: rust-review\ndescription: Rust の差分を読む\n---\n\n# rust-review\n";

#[tokio::test]
async fn cos_ops_admin_config_org_and_skills_are_audited() {
    let env = admin_env();
    task_ops::knowledge::init(&env.knowledge_root).expect("knowledge init");
    use task_core::TaskStore;

    let op = run_domain(
        &env,
        "org-create-root",
        "POST",
        "/api/v1/org",
        json!({"id": "secretary", "name": "秘書", "kind": "secretary"}),
        "org.create",
    )
    .await;
    assert_eq!(op["target_id"], "secretary");
    run_domain(
        &env,
        "org-create",
        "POST",
        "/api/v1/org",
        json!({"id": "coding", "name": "コーディング部", "kind": "department", "parent_id": "secretary"}),
        "org.create",
    )
    .await;
    run_domain(
        &env,
        "org-patch",
        "PATCH",
        "/api/v1/org/coding",
        json!({"brief": "コードを書く"}),
        "org.update",
    )
    .await;
    assert_eq!(
        env.store
            .org_get("coding")
            .expect("get")
            .expect("node")
            .brief,
        "コードを書く"
    );

    let op = run_external(
        &env,
        "skill-put",
        "PUT",
        "/api/v1/skills/rust-review",
        json!({"skill_md": SKILL_MD}),
        "skill.put",
    )
    .await;
    assert_eq!(op["result"]["path"], "skills/rust-review/SKILL.md");
    assert!(
        env.knowledge_root
            .join("skills/rust-review/SKILL.md")
            .exists()
    );

    run_domain(
        &env,
        "skill-mount",
        "POST",
        "/api/v1/org/coding/skills",
        json!({"skill": "rust-review"}),
        "org.skill_mount",
    )
    .await;
    let node = env.store.org_get("coding").expect("get").expect("node");
    assert_eq!(node.profile.skills_mounts, vec!["rust-review".to_string()]);

    // A mounted skill cannot be deleted (409), an unknown node is 404, a bad browser grant is 422.
    expect_rejected(
        &env,
        "org-rejected",
        vec![
            (
                "mounted",
                "DELETE",
                "/api/v1/skills/rust-review",
                json!(null),
                409,
            ),
            (
                "dup",
                "POST",
                "/api/v1/org",
                json!({"id": "coding", "name": "x", "kind": "section"}),
                409,
            ),
            (
                "missing",
                "PATCH",
                "/api/v1/org/nope",
                json!({"brief": "x"}),
                404,
            ),
            (
                "no-grant",
                "PATCH",
                "/api/v1/org/coding/browser-settings",
                json!({"allowed_domains": []}),
                422,
            ),
            (
                "has-children",
                "DELETE",
                "/api/v1/org/secretary",
                json!(null),
                409,
            ),
        ],
    )
    .await;
    assert!(
        env.knowledge_root
            .join("skills/rust-review/SKILL.md")
            .exists()
    );

    run_domain(
        &env,
        "skill-unmount",
        "DELETE",
        "/api/v1/org/coding/skills/rust-review",
        json!(null),
        "org.skill_unmount",
    )
    .await;
    let op = run_external(
        &env,
        "skill-delete",
        "DELETE",
        "/api/v1/skills/rust-review",
        json!(null),
        "skill.delete",
    )
    .await;
    assert_eq!(op["result"]["deleted"], true);
    assert!(
        !env.knowledge_root
            .join("skills/rust-review/SKILL.md")
            .exists()
    );

    run_domain(
        &env,
        "org-delete",
        "DELETE",
        "/api/v1/org/coding",
        json!(null),
        "org.delete",
    )
    .await;
    assert!(env.store.org_get("coding").expect("get").is_none());
}

#[tokio::test]
async fn cos_ops_admin_config_repos_and_cluster_settings_are_audited() {
    use task_core::TaskStore;
    let env = admin_env();
    let app = env.router();
    let created = send(
        &app,
        post_admin(
            "/api/v1/projects",
            &json!({"title": "案件", "request": "作る"}),
        ),
    )
    .await;
    assert_eq!(created.status.as_u16(), 201, "{}", created.text());
    let project = created.json()["id"].as_str().expect("id").to_string();
    let dir = tempfile::tempdir().expect("tempdir");

    let op = run_domain(
        &env,
        "repo-create",
        "POST",
        &format!("/api/v1/projects/{project}/repos"),
        json!({"name": "app", "kind": "dir", "location": {"kind": "local", "path": dir.path().to_string_lossy()}}),
        "repo.create",
    )
    .await;
    let repo_id = op["result"]["id"].as_str().expect("repo id").to_string();
    assert_eq!(op["target_id"], repo_id.as_str());
    let repo = env
        .store
        .repo_get(repo_id.parse().expect("repo id"))
        .expect("get")
        .expect("repo");
    assert_eq!(repo.name, "app");

    run_domain(
        &env,
        "repo-patch",
        "PATCH",
        &format!("/api/v1/repos/{repo_id}"),
        json!({"name": "app2"}),
        "repo.update",
    )
    .await;
    let repo = env.store.repo_get(repo.id).expect("get").expect("repo");
    assert_eq!(repo.name, "app2");

    run_domain(
        &env,
        "cluster-settings",
        "PUT",
        "/api/v1/clusters/pegasus/settings",
        json!({"work_dir": "/work/NBB/rmaeda"}),
        "cluster.settings_put",
    )
    .await;
    let settings = env
        .store
        .cluster_settings_get("pegasus")
        .expect("get")
        .expect("settings");
    assert_eq!(settings.work_dir.as_deref(), Some("/work/NBB/rmaeda"));

    expect_rejected(
        &env,
        "repos-rejected",
        vec![
            (
                "bad-name",
                "PATCH",
                &format!("/api/v1/repos/{repo_id}"),
                json!({"name": "Bad Name"}),
                422,
            ),
            (
                "no-repo",
                "DELETE",
                "/api/v1/repos/01M4G00000000000000000000A",
                json!(null),
                404,
            ),
            (
                "no-cluster",
                "PUT",
                "/api/v1/clusters/nope/settings",
                json!({"work_dir": "/x"}),
                404,
            ),
            (
                "relative",
                "PUT",
                "/api/v1/clusters/pegasus/settings",
                json!({"work_dir": "rel"}),
                422,
            ),
        ],
    )
    .await;

    run_domain(
        &env,
        "repo-delete",
        "DELETE",
        &format!("/api/v1/repos/{repo_id}"),
        json!(null),
        "repo.delete",
    )
    .await;
    assert!(env.store.repo_get(repo.id).expect("get").is_none());
}
