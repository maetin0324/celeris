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

#[tokio::test]
async fn cos_ops_admin_config_providers_and_account_create_are_audited() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let providers = tmp.path().join("providers.d");
    std::fs::create_dir_all(&providers).expect("mkdir");
    let accounts = tmp.path().join("accounts");
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        providers_dir: Some(providers.clone()),
        accounts_root: Some(accounts.clone()),
        ..Default::default()
    });

    let op = run_domain(
        &env,
        "provider-create",
        "POST",
        "/api/v1/providers",
        json!({"id": "fake-a", "adapter": "fake", "kind": "adapter", "llm_source": "none"}),
        "provider.create",
    )
    .await;
    assert_eq!(op["target_id"], "fake-a");
    assert!(providers.join("fake-a.toml").exists());

    run_domain(
        &env,
        "provider-patch",
        "PATCH",
        "/api/v1/providers/fake-a",
        json!({"concurrency": 2}),
        "provider.update",
    )
    .await;
    let text = std::fs::read_to_string(providers.join("fake-a.toml")).expect("read");
    assert!(text.contains("concurrency = 2"), "{text}");

    run_domain(
        &env,
        "account-create",
        "POST",
        "/api/v1/accounts",
        json!({"id": "acct1"}),
        "account.create",
    )
    .await;
    assert!(accounts.join("acct1").is_dir());

    // Inline credential values are secret operations: refused, and the value is not recorded.
    let app = env.router();
    let (thread, _, bearer) = cos_bearer(&env, "provider-secret");
    let resp = send(
        &app,
        post_json_with(
            OPS,
            &op_body(
                "secret",
                "POST",
                "/api/v1/providers",
                json!({"id": "leaky", "adapter": "fake", "env": {"OPENAI_API_KEY": "sk-should-not-be-recorded"}}),
            ),
            &[("authorization", bearer.as_str())],
        ),
    )
    .await;
    assert_problem(&resp, 422, "secret_operations");
    let payload: String = db(&env)
        .query_row(
            "SELECT payload_json FROM cos_operations WHERE thread_id=?1",
            [&thread],
            |row| row.get(0),
        )
        .expect("row");
    assert!(!payload.contains("sk-should-not-be-recorded"), "{payload}");
    assert!(!providers.join("leaky.toml").exists());

    expect_rejected(
        &env,
        "providers-rejected",
        vec![
            (
                "dup",
                "POST",
                "/api/v1/providers",
                json!({"id": "fake-a", "adapter": "fake"}),
                409,
            ),
            (
                "missing",
                "DELETE",
                "/api/v1/providers/nope",
                json!(null),
                404,
            ),
            (
                "account-dup",
                "POST",
                "/api/v1/accounts",
                json!({"id": "acct1"}),
                409,
            ),
        ],
    )
    .await;

    run_domain(
        &env,
        "provider-delete",
        "DELETE",
        "/api/v1/providers/fake-a",
        json!(null),
        "provider.delete",
    )
    .await;
    assert!(!providers.join("fake-a.toml").exists());
}

/// A stand-in for the daemon's admin channel: answers every request and counts them by kind.
fn fake_daemon() -> (
    tokio::sync::mpsc::Sender<task_api::AdminRequest>,
    std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>,
) {
    use task_api::{
        AccountCheckOutcome, AdminRequest, NotifyTestOutcome, ProviderCheckOutcome,
        ProviderCheckResult,
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel::<AdminRequest>(8);
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        while let Some(request) = rx.recv().await {
            let kind = match request {
                AdminRequest::Reload { reply } => {
                    let _ = reply.send(Ok(()));
                    "reload"
                }
                AdminRequest::Check { provider_id, reply } => {
                    let _ = reply.send(if provider_id == "nope" {
                        Err(task_api::CheckError::NotFound)
                    } else {
                        Ok(ProviderCheckOutcome {
                            result: ProviderCheckResult::Ok,
                            detail: None,
                        })
                    });
                    "provider_check"
                }
                AdminRequest::AccountCheck { reply, .. } => {
                    let _ = reply.send(Ok(AccountCheckOutcome {
                        result: ProviderCheckResult::Ok,
                        detail: None,
                        observation: None,
                    }));
                    "account_check"
                }
                AdminRequest::AccountRemove { reply, .. } => {
                    let _ = reply.send(Ok(()));
                    "account_remove"
                }
                AdminRequest::NotifyTest { reply } => {
                    let _ = reply.send(Ok(NotifyTestOutcome {
                        ok: true,
                        detail: None,
                    }));
                    "notify_test"
                }
                _ => "other",
            };
            log.lock().expect("log").push(kind);
        }
    });
    (tx, seen)
}

struct FakeReleases {
    promoted: std::sync::Mutex<Vec<String>>,
}

impl task_api::ReleaseSource for FakeReleases {
    fn list(&self) -> task_api::ReleasesFs {
        task_api::ReleasesFs {
            current: Some("aaaaaaaaaaaa".into()),
            previous: None,
            items: vec![],
        }
    }
    fn promote(
        &self,
        sha12: &str,
    ) -> Result<task_api::types::ReleasePromoteAccepted, task_api::ReleasePromoteError> {
        if sha12 == "aaaaaaaaaaaa" {
            return Err(task_api::ReleasePromoteError::AlreadyCurrent);
        }
        self.promoted.lock().expect("lock").push(sha12.to_string());
        Ok(task_api::types::ReleasePromoteAccepted {
            sha12: sha12.to_string(),
            log: "/dev/null".into(),
            started_at: "2026-10-09T00:00:00Z".into(),
            script_from: "current".into(),
        })
    }
}

struct Hook(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl task_api::ModelDiscoveryHook for Hook {
    fn discover<'a>(
        &'a self,
        source: Option<String>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Vec<task_api::DiscoverySummaryView>> + Send + 'a>,
    > {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move {
            vec![task_api::DiscoverySummaryView {
                source: source.unwrap_or_else(|| "all".into()),
                ok: true,
                count: 1,
                error: None,
                delta: Default::default(),
            }]
        })
    }
}

#[tokio::test]
async fn cos_ops_admin_config_daemon_and_external_effects_run_once() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let secrets = tmp.path().join("secrets");
    std::fs::create_dir_all(&secrets).expect("mkdir");
    std::fs::write(
        secrets.join("discord-webhook"),
        "https://discord.example/api/webhooks/1/x\n",
    )
    .expect("webhook");
    let (admin_tx, seen) = fake_daemon();
    let releases = std::sync::Arc::new(FakeReleases {
        promoted: std::sync::Mutex::new(Vec::new()),
    });
    let discovered = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        admin_tx: Some(admin_tx),
        secrets_dir: Some(secrets),
        accounts_root: Some(tmp.path().join("accounts")),
        releases: Some(releases.clone()),
        model_discovery: Some(std::sync::Arc::new(Hook(discovered.clone()))),
        ..Default::default()
    });

    // Each run_external also resends the same envelope and expects the recorded operation back.
    let op = run_external(
        &env,
        "reload",
        "POST",
        "/api/v1/reload",
        json!({}),
        "daemon.reload",
    )
    .await;
    assert_eq!(op["result"]["reloaded"], true);
    run_external(
        &env,
        "notify",
        "POST",
        "/api/v1/notify/test",
        json!(null),
        "notify.test",
    )
    .await;
    run_external(
        &env,
        "pcheck",
        "POST",
        "/api/v1/providers/fake-a/check",
        json!({}),
        "provider.check",
    )
    .await;
    run_external(
        &env,
        "acheck",
        "POST",
        "/api/v1/accounts/acct1/check",
        json!({"adapter": "claude-code"}),
        "account.check",
    )
    .await;
    let op = run_external(
        &env,
        "aremove",
        "DELETE",
        "/api/v1/accounts/acct1",
        json!(null),
        "account.delete",
    )
    .await;
    assert_eq!(op["result"]["removed"], true);
    let mut kinds = seen.lock().expect("seen").clone();
    kinds.sort_unstable();
    assert_eq!(
        kinds,
        vec![
            "account_check",
            "account_remove",
            "notify_test",
            "provider_check",
            "reload"
        ],
        "each effect ran exactly once despite the resend"
    );

    let op = run_external(
        &env,
        "discover",
        "POST",
        "/api/v1/llm/models/discover",
        json!({"source": "opencode-go"}),
        "model_catalog.discover",
    )
    .await;
    assert_eq!(op["result"]["results"][0]["source"], "opencode-go");
    assert_eq!(discovered.load(std::sync::atomic::Ordering::SeqCst), 1);

    run_external(
        &env,
        "promote",
        "POST",
        "/api/v1/releases/bbbbbbbbbbbb/promote",
        json!(null),
        "release.promote",
    )
    .await;
    assert_eq!(
        *releases.promoted.lock().expect("lock"),
        vec!["bbbbbbbbbbbb".to_string()]
    );

    let op = run_external(
        &env,
        "replay",
        "POST",
        "/api/v1/replay",
        json!({}),
        "daemon.replay",
    )
    .await;
    assert!(op["result"].is_object(), "{op}");

    // Refusals of the external system settle `rejected` without a second attempt.
    expect_rejected(
        &env,
        "external-rejected",
        vec![
            (
                "current",
                "POST",
                "/api/v1/releases/aaaaaaaaaaaa/promote",
                json!(null),
                409,
            ),
            (
                "no-provider",
                "POST",
                "/api/v1/providers/nope/check",
                json!({}),
                404,
            ),
            (
                "bad-adapter",
                "POST",
                "/api/v1/accounts/acct1/check",
                json!({"adapter": "x"}),
                400,
            ),
            (
                "bad-source",
                "POST",
                "/api/v1/llm/models/discover",
                json!({"source": "??"}),
                400,
            ),
        ],
    )
    .await;
    assert_eq!(releases.promoted.lock().expect("lock").len(), 1);
}
