//! ADR 2026-10-09-cos-operations-all-mutations D3 (WU ops-admin-config): the admin routes run
//! through `/cos/operations`. Each is refused when called directly with the CoS credential (422 +
//! rejected audit row) and applied through the envelope with its row, audit event and chat card.
//! Daemon-channel and external operations (C) use fakes: the daemon admin channel, the release
//! source and the discovery hook are test doubles, so nothing outside the test process is touched.
use crate::common;

use common::cos_ops::{OPS, audit_events, cos_bearer, db, op_body, run_domain, run_external};
use common::*;
use serde_json::{Value, json};

use task_core::model_catalog::{CatalogSource, DiscoveredModel};

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
        vec![(
            "no-override",
            "DELETE",
            "/api/v1/llm/models/opencode-go/glm-5/override",
            json!(null),
            404,
        )],
    )
    .await;
}

const SKILL_MD: &str =
    "---\nname: rust-review\ndescription: Rust の差分を読む\n---\n\n# rust-review\n";

#[tokio::test]
async fn cos_ops_admin_config_org_and_skills_are_audited() {
    let env = admin_env();
    task_ops::knowledge::init(&env.knowledge_root).expect("knowledge init");

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
            // ADR 2026-11-14-repo-auto-integrate D6: CoS は auto_integrate を書けない（値によらず 422）。
            (
                "auto-integrate-update",
                "PATCH",
                &format!("/api/v1/repos/{repo_id}"),
                json!({"auto_integrate": true}),
                422,
            ),
            (
                "auto-integrate-create",
                "POST",
                &format!("/api/v1/projects/{project}/repos"),
                json!({"name": "other", "kind": "dir", "location": {"kind": "local", "path": "/srv/other"}, "auto_integrate": false}),
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

/// A stand-in for the daemon's admin channel: answers every request and counts them by kind.
fn fake_daemon() -> (
    tokio::sync::mpsc::Sender<task_api::AdminRequest>,
    std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>,
) {
    use task_api::{AccountCheckOutcome, AdminRequest, NotifyTestOutcome};
    use task_core::AccountCheckResult;
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
                AdminRequest::AccountCheck { reply, .. } => {
                    let _ = reply.send(Ok(AccountCheckOutcome {
                        result: AccountCheckResult::Ok,
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
        audit: &task_api::releases::DeploymentContext,
    ) -> Result<task_api::types::ReleasePromoteAccepted, task_api::ReleasePromoteError> {
        // ADR 2026-10-27-human-ops-actor-roles-request-ref D2: the test threads are project CoS threads.
        let thread = audit
            .actor
            .strip_prefix("cos_project:")
            .expect("cos_project actor");
        assert!(!thread.is_empty(), "{}", audit.actor);
        assert!(!audit.reason.trim().is_empty());
        assert!(!audit.policy_ref.trim().is_empty());
        assert_eq!(audit.decision_refs.len(), 1);
        assert!(audit.decision_refs[0].starts_with("cos_operation:"));
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
        "acheck",
        "POST",
        "/api/v1/llm/sources/claude_oauth/accounts/acct1/check",
        json!({}),
        "account.check",
    )
    .await;
    let op = run_external(
        &env,
        "aremove",
        "DELETE",
        "/api/v1/llm/sources/claude_oauth/accounts/acct1",
        json!(null),
        "account.delete",
    )
    .await;
    assert_eq!(op["result"]["removed"], true);
    let mut kinds = seen.lock().expect("seen").clone();
    kinds.sort_unstable();
    assert_eq!(
        kinds,
        vec!["account_check", "account_remove", "notify_test", "reload"],
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
                "bad-account-source",
                "POST",
                "/api/v1/llm/sources/nope/accounts/acct1/check",
                json!({}),
                404,
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

// ---- account.* の契約（アーキv4 H R2 contract。handlers::accounts の処理を SCC の外へ移す前に今の挙動を固定する）----

const QWEN: &str = "openai_compatible:qwen";

/// The daemon side of the account operations: `AccountLimitSet` writes the env DB (implicit ids
/// other than [`QWEN`] are `NotFound`), `AccountCheck` answers `auth_failed` with a detail, and
/// `AccountRemove` refuses `busy` as in use. Every request is logged as `"<kind> <adapter> <id>
/// <max_runs>"`.
fn spawn_account_daemon(
    mut rx: tokio::sync::mpsc::Receiver<task_api::AdminRequest>,
    db_path: std::path::PathBuf,
) -> std::sync::Arc<std::sync::Mutex<Vec<String>>> {
    use task_api::{AccountAdminError, AccountCheckOutcome, AdminRequest};
    use task_core::AccountCheckResult;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        while let Some(request) = rx.recv().await {
            let line = match request {
                AdminRequest::AccountLimitSet {
                    adapter,
                    id,
                    max_runs,
                    reply,
                } => {
                    let line = format!("limit {adapter} {id} {max_runs:?}");
                    let result = if adapter == "implicit" && id != QWEN {
                        Err(AccountAdminError::NotFound)
                    } else {
                        task_core::SqliteStore::open(&db_path)
                            .and_then(|store| store.set_account_limit(&adapter, &id, max_runs))
                            .map_err(|e| AccountAdminError::Unavailable(e.to_string()))
                    };
                    let _ = reply.send(result);
                    line
                }
                AdminRequest::AccountCheck { adapter, id, reply } => {
                    let _ = reply.send(Ok(AccountCheckOutcome {
                        result: AccountCheckResult::AuthFailed,
                        detail: Some("token expired".into()),
                        observation: None,
                    }));
                    format!("check {adapter} {id}")
                }
                AdminRequest::AccountRemove { adapter, id, reply } => {
                    let _ = reply.send(if id == "busy" {
                        Err(AccountAdminError::InUse)
                    } else {
                        Ok(())
                    });
                    format!("remove {adapter} {id}")
                }
                _ => "other".to_string(),
            };
            log.lock().expect("log").push(line);
        }
    });
    seen
}

/// An env with the claude pool root (no codex root), the account daemon double, and the implicit
/// account [`QWEN`] in the snapshot: `(env, tempdir, claude root, daemon log)`.
fn account_env() -> (
    TestEnv,
    tempfile::TempDir,
    std::path::PathBuf,
    std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("claude-accounts");
    std::fs::create_dir_all(&root).expect("root");
    let (admin_tx, admin_rx) = tokio::sync::mpsc::channel(8);
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        accounts_root: Some(root.clone()),
        max_runs_per_account: 2,
        admin_tx: Some(admin_tx),
        ..Default::default()
    });
    let seen = spawn_account_daemon(admin_rx, env.db_path.clone());
    let mut snap = snapshot(1);
    snap.implicit_accounts = vec![task_ops::daemon::ImplicitAccountLive {
        source: QWEN.into(),
        models: vec!["qwen3.8-27b".into()],
        in_use: 0,
    }];
    env.daemon_tx.send(Some(snap)).expect("snapshot");
    (env, tmp, root, seen)
}

/// 守る不変条件: CoS の `account.create`・`account.limit_set`・`account.check`・`account.delete` が成功したとき、
/// 操作の記録の `result` は HTTP の同じ route が返す値と同じ形（create と limit_set は `AccountView` の全欄、
/// check は `AccountCheckResponse`）で、delete は `{id, adapter, removed}`（adapter は内部の adapter 名）。
/// create は作り直しで古い上限を引き継がない（daemon へ adapter 名 `claude-code` で解除を送る）。
/// 既存の試験で検出できない理由: 既存の `cos_ops_admin_config_daemon_and_external_effects_run_once` は
/// check と delete が 1 回走ることと `removed == true` だけを見て、create と limit_set は CoS から一度も
/// 呼んでいない。result の欄の欠落・改名や、上限の解除の key の変化は、account の処理を別の module へ
/// 移したときに既存の試験を通り抜ける。
#[tokio::test]
async fn cos_ops_admin_config_account_success_results_keep_their_shape() {
    let (env, _tmp, root, seen) = account_env();
    // A stored limit left over from an earlier account with the same id.
    env.store
        .set_account_limit("claude-code", "acct-new", Some(5))
        .expect("stale limit");

    let op = run_domain(
        &env,
        "acreate",
        "POST",
        "/api/v1/llm/sources/claude_oauth/accounts",
        json!({"id": "acct-new"}),
        "account.create",
    )
    .await;
    assert_eq!(op["target_kind"], "account");
    assert_eq!(op["target_id"], "acct-new");
    let dir = root.join("acct-new");
    assert_eq!(
        op["result"],
        json!({
            "source_id": "claude_oauth",
            "account_id": "acct-new",
            "dir": dir.display().to_string(),
            "logged_in": false,
            "in_use": 0,
            "usage": null,
            "score": null,
            "excluded_reason": null,
            "cooldown": null,
            "last_check": null,
            "login_pending": false,
            "stats": {"runs": 0, "done": 0, "error": 0, "input_tokens": 0, "output_tokens": 0},
            "max_runs": null,
            "effective_max_runs": 2,
            "max_runs_source": "default",
            "implicit": false,
            "models": [],
        })
    );
    assert!(dir.is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&dir).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }
    assert!(
        env.store
            .account_limits()
            .expect("limits")
            .iter()
            .all(|l| l.account_id != "acct-new"),
        "the re-created account does not inherit the stale limit"
    );

    let op = run_external(
        &env,
        "alimit",
        "PATCH",
        "/api/v1/llm/sources/claude_oauth/accounts/acct-new",
        json!({"max_runs": 3}),
        "account.limit_set",
    )
    .await;
    assert_eq!(op["target_kind"], "account");
    assert_eq!(op["target_id"], "acct-new");
    let view = &op["result"];
    assert_eq!(view["source_id"], "claude_oauth");
    assert_eq!(view["account_id"], "acct-new");
    assert_eq!(view["max_runs"], 3);
    assert_eq!(view["effective_max_runs"], 3);
    assert_eq!(view["max_runs_source"], "account");
    assert_eq!(view["implicit"], false);

    let op = run_external(
        &env,
        "alimit-implicit",
        "PATCH",
        &format!("/api/v1/llm/sources/{QWEN}/accounts/{QWEN}"),
        json!({"max_runs": 4}),
        "account.limit_set",
    )
    .await;
    assert_eq!(op["target_id"], QWEN);
    assert_eq!(
        op["result"],
        json!({
            "source_id": QWEN,
            "account_id": QWEN,
            "dir": null,
            "logged_in": true,
            "in_use": 0,
            "usage": null,
            "score": null,
            "excluded_reason": null,
            "cooldown": null,
            "last_check": null,
            "login_pending": false,
            "stats": {"runs": 0, "done": 0, "error": 0, "input_tokens": 0, "output_tokens": 0},
            "max_runs": 4,
            "effective_max_runs": 4,
            "max_runs_source": "account",
            "implicit": true,
            "models": ["qwen3.8-27b"],
        })
    );

    let op = run_external(
        &env,
        "acheck",
        "POST",
        "/api/v1/llm/sources/claude_oauth/accounts/acct-new/check",
        json!({}),
        "account.check",
    )
    .await;
    assert_eq!(op["target_id"], "acct-new");
    let result = op["result"].as_object().expect("check result");
    let mut keys: Vec<&str> = result.keys().map(String::as_str).collect();
    keys.sort_unstable();
    // `usage` is absent when the check observed none (skip_serializing_if).
    assert_eq!(keys, vec!["checked_at", "detail", "result"], "{result:?}");
    assert_eq!(result["result"], "auth_failed");
    assert_eq!(result["detail"], "token expired");
    assert!(
        time::OffsetDateTime::parse(
            result["checked_at"].as_str().expect("checked_at"),
            &time::format_description::well_known::Rfc3339
        )
        .is_ok(),
        "{result:?}"
    );

    let op = run_external(
        &env,
        "aremove",
        "DELETE",
        "/api/v1/llm/sources/claude_oauth/accounts/acct-new",
        json!(null),
        "account.delete",
    )
    .await;
    assert_eq!(op["target_id"], "acct-new");
    assert_eq!(
        op["result"],
        json!({"id": "acct-new", "adapter": "claude-code", "removed": true})
    );

    assert_eq!(
        *seen.lock().expect("seen"),
        vec![
            "limit claude-code acct-new None".to_string(),
            "limit claude-code acct-new Some(3)".to_string(),
            format!("limit implicit {QWEN} Some(4)"),
            "check claude-code acct-new".to_string(),
            "remove claude-code acct-new".to_string(),
        ],
        "each effect reached the daemon once, keyed by the internal adapter"
    );
}

/// One refused account operation: the request, the problem it returns, the text its record keeps
/// (in the reason or the result), and the `(target_kind, target_id)` of the record.
struct Refusal<'a> {
    key: &'a str,
    method: &'a str,
    path: String,
    body: Value,
    status: u16,
    code: &'a str,
    detail: String,
    target: (&'a str, &'a str),
}

/// Send each refusal through the envelope on one CoS thread. Each returns its problem (status,
/// `code`, `type`) and leaves exactly one rejected row for its idempotency key, on its target, whose
/// reason or result holds the detail, with audit events on the same target ending `rejected`.
async fn expect_refusals(env: &TestEnv, thread_key: &str, cases: Vec<Refusal<'_>>) {
    let app = env.router();
    let (thread, _, bearer) = cos_bearer(env, thread_key);
    let headers = [("authorization", bearer.as_str())];
    for case in cases {
        let key = case.key;
        let resp = send(
            &app,
            post_json_with(
                OPS,
                &op_body(key, case.method, &case.path, case.body),
                &headers,
            ),
        )
        .await;
        let problem = assert_problem(&resp, case.status, case.code);
        assert!(
            problem["detail"]
                .as_str()
                .is_some_and(|d| d.contains(&case.detail)),
            "{key}: {problem}"
        );
        let conn = db(env);
        let rows: Vec<(String, String, String, String, String)> = conn
            .prepare(
                "SELECT id, state, target_kind, target_id, reason || ' ' || COALESCE(result_json, '')
                 FROM cos_operations WHERE thread_id=?1 AND idempotency_key=?2",
            )
            .expect("prepare")
            .query_map(rusqlite::params![thread, key], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
            })
            .expect("query")
            .map(|row| row.expect("row"))
            .collect();
        assert_eq!(rows.len(), 1, "{key}: {rows:?}");
        let (id, state, kind, target, recorded) = &rows[0];
        assert_eq!(state, "rejected", "{key}");
        assert_eq!((kind.as_str(), target.as_str()), case.target, "{key}");
        assert!(recorded.contains(&case.detail), "{key}: {recorded}");
        let events = audit_events(env, id);
        assert!(!events.is_empty(), "{key}: no audit event");
        for event in &events {
            assert_eq!(event["actor"], "cos", "{key}");
            assert_eq!(event["target_kind"], case.target.0, "{key}");
            assert_eq!(event["target_id"], case.target.1, "{key}");
            assert_ne!(event["state"], "applied", "{key}: {events:?}");
        }
        assert_eq!(
            events.last().map(|e| e["state"].clone()),
            Some(json!("rejected")),
            "{key}: {events:?}"
        );
    }
}

/// 守る不変条件: CoS の account の 4 操作が拒まれたとき、応答は route と同じ problem（status・code・type）で、
/// 操作の行が 1 件 rejected で残り、その target と同じ target の監査 event が rejected で終わる（effect は
/// 走らない）。拒否の理由（managed pool の無い source の create は 400 と理由の文、未知の source・root の無い
/// pool・暗黙の account 名の不一致は 404 account_not_found、resolve_pool_account の 409 implicit_account、
/// daemon の 409 account_in_use、本文の 400）を今の値で固定する。
/// 既存の試験で検出できない理由: 既存の `cos_ops_admin_config_daemon_and_external_effects_run_once` の拒否は
/// 未知の source の check 1 件の status と rejected の行の数だけで、create・limit_set・delete の拒否、
/// 理由の文、記録の target、監査 event は見ていない。拒否を組み立てる処理を別の module へ移すと、
/// 監査の記録を落としても status だけは同じになりうる。
#[tokio::test]
async fn cos_ops_admin_config_account_refusals_are_recorded_with_their_reason() {
    let (env, _tmp, root, seen) = account_env();
    std::fs::create_dir_all(root.join("dup")).expect("dup");
    let pool = "/api/v1/llm/sources/claude_oauth/accounts";
    let implicit = format!("/api/v1/llm/sources/{QWEN}/accounts/{QWEN}");
    let r = |key, method, path: String, body, status, code, detail: String, target| Refusal {
        key,
        method,
        path,
        body,
        status,
        code,
        detail,
        target,
    };
    let no_pool = "source does not have a managed account pool".to_string();
    expect_refusals(
        &env,
        "account-refusals",
        vec![
            // account.create
            r(
                "create-unknown-source",
                "POST",
                "/api/v1/llm/sources/nope/accounts".into(),
                json!({"id": "x"}),
                400,
                "bad_request",
                no_pool.clone(),
                ("account", "x"),
            ),
            r(
                "create-implicit-source",
                "POST",
                format!("/api/v1/llm/sources/{QWEN}/accounts"),
                json!({"id": "x"}),
                400,
                "bad_request",
                no_pool,
                ("account", "x"),
            ),
            r(
                "create-pool-without-root",
                "POST",
                "/api/v1/llm/sources/codex_oauth/accounts".into(),
                json!({"id": "x"}),
                409,
                "accounts_unavailable",
                "the [accounts] section is not configured".into(),
                ("account", "x"),
            ),
            r(
                "create-bad-id",
                "POST",
                pool.into(),
                json!({"id": "../escape"}),
                400,
                "bad_request",
                "id must be 1-64 ASCII alphanumeric".into(),
                ("account", "../escape"),
            ),
            r(
                "create-dup",
                "POST",
                pool.into(),
                json!({"id": "dup"}),
                409,
                "account_exists",
                "account already exists: dup".into(),
                ("account", "dup"),
            ),
            r(
                "create-unknown-field",
                "POST",
                pool.into(),
                json!({"id": "y", "adapter": "codex"}),
                422,
                "validation",
                "request.body".into(),
                ("api", pool),
            ),
            // account.limit_set
            r(
                "limit-unknown-source",
                "PATCH",
                "/api/v1/llm/sources/nope/accounts/a".into(),
                json!({"max_runs": 2}),
                404,
                "account_not_found",
                "account not found: nope".into(),
                ("account", "nope"),
            ),
            r(
                "limit-pool-without-root",
                "PATCH",
                "/api/v1/llm/sources/codex_oauth/accounts/a".into(),
                json!({"max_runs": 2}),
                404,
                "account_not_found",
                "account not found: codex_oauth".into(),
                ("account", "codex_oauth"),
            ),
            r(
                "limit-implicit-other-name",
                "PATCH",
                format!("/api/v1/llm/sources/{QWEN}/accounts/other"),
                json!({"max_runs": 2}),
                404,
                "account_not_found",
                "account not found: other".into(),
                ("account", "other"),
            ),
            r(
                "limit-out-of-range",
                "PATCH",
                format!("{pool}/dup"),
                json!({"max_runs": 0}),
                400,
                "bad_request",
                "max_runs must be an integer in 1..=32 or null".into(),
                ("account", "dup"),
            ),
            // account.delete / account.check (resolve_pool_account and the daemon's refusal)
            r(
                "delete-unknown-source",
                "DELETE",
                "/api/v1/llm/sources/nope/accounts/acct1".into(),
                json!(null),
                404,
                "account_not_found",
                "account not found: nope".into(),
                ("account", "acct1"),
            ),
            r(
                "delete-pool-without-root",
                "DELETE",
                "/api/v1/llm/sources/codex_oauth/accounts/acct1".into(),
                json!(null),
                404,
                "account_not_found",
                "account not found: codex_oauth".into(),
                ("account", "acct1"),
            ),
            r(
                "delete-implicit",
                "DELETE",
                implicit.clone(),
                json!(null),
                409,
                "implicit_account",
                format!("implicit account of source {QWEN} cannot be deleted"),
                ("account", QWEN),
            ),
            r(
                "delete-implicit-other-name",
                "DELETE",
                format!("/api/v1/llm/sources/{QWEN}/accounts/other"),
                json!(null),
                404,
                "account_not_found",
                "account not found: other".into(),
                ("account", "other"),
            ),
            r(
                "delete-in-use",
                "DELETE",
                format!("{pool}/busy"),
                json!(null),
                409,
                "account_in_use",
                "account is in use: busy".into(),
                ("account", "busy"),
            ),
            r(
                "check-implicit",
                "POST",
                format!("{implicit}/check"),
                json!({}),
                409,
                "implicit_account",
                format!("implicit account of source {QWEN} cannot be checked"),
                ("account", QWEN),
            ),
            r(
                "check-pool-without-root",
                "POST",
                "/api/v1/llm/sources/codex_oauth/accounts/a/check".into(),
                json!({}),
                404,
                "account_not_found",
                "account not found: codex_oauth".into(),
                ("account", "a"),
            ),
        ],
    )
    .await;
    // Nothing was created, and only the in-use delete reached the daemon (and was refused there).
    assert!(!root.join("x").exists() && !root.join("y").exists());
    assert!(root.join("dup").is_dir());
    assert_eq!(
        *seen.lock().expect("seen"),
        vec!["remove claude-code busy".to_string()]
    );
}
