//! ADR 2026-10-09-cos-operations-all-mutations D3 / 2026-10-09-cos-operations-external-effects
//! (WU ops-projects-cron): the project docs routes write git, so through `/cos/operations` they are
//! C-class operations — a pending record before the git effect, applied after it, a refusal that
//! changed nothing settles `rejected`, and a resent request never commits again. Direct calls with
//! the CoS credential are 422. Every repository is a local git repository in a tempdir (no network).
mod common;

use common::cos_ops::{OPS, audit_events, cos_bearer, op_body};
use common::*;
use serde_json::{Value, json};

/// The project repository's document directory (a fixture repo, not this one).
const DOCS: &str = "docs";
use std::path::Path;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn commits(repo: &Path) -> usize {
    git(repo, &["rev-list", "--count", "main"])
        .trim()
        .parse()
        .expect("count")
}

async fn project(app: &axum::Router) -> String {
    let resp = send(
        app,
        post_admin(
            "/api/v1/projects",
            &json!({"title": "文書", "request": "書く"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 201, "{}", resp.text());
    resp.json()["id"].as_str().expect("id").to_string()
}

async fn project_with_repo(app: &axum::Router, dir: &Path) -> (String, std::path::PathBuf) {
    let id = project(app).await;
    let repo = dir.join("primary");
    std::fs::create_dir_all(repo.join("docs")).expect("mkdir");
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "t"]);
    std::fs::write(repo.join("docs/README.md"), "# 案件\n").expect("write");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "first"]);
    let created = send(
        app,
        post_admin(
            &format!("/api/v1/projects/{id}/repos"),
            &json!({"location": {"kind": "local", "path": repo.to_string_lossy()}}),
        ),
    )
    .await;
    assert_eq!(created.status.as_u16(), 201, "{}", created.text());
    (id, repo)
}

/// Direct call 422, then the envelope: applied with a pending → applied audit trail.
async fn run_external(
    env: &TestEnv,
    key: &str,
    method: &str,
    path: &str,
    body: Value,
    action: &str,
) -> Value {
    let app = env.router();
    let (_, run, bearer) = cos_bearer(env, key);
    let headers = [("authorization", bearer.as_str())];
    let direct = match method {
        "POST" => send(&app, post_json_with(path, &body, &headers)).await,
        "PUT" => send(&app, put_json_with(path, &body, &headers)).await,
        "DELETE" => send(&app, delete_with(path, &headers)).await,
        other => panic!("unsupported {other}"),
    };
    assert_problem(&direct, 422, "cos_audit_context_required");
    let resp = send(
        &app,
        post_json_with(OPS, &op_body(key, method, path, body), &headers),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{key}: {}", resp.text());
    let op = resp.json()["operation"].clone();
    assert_eq!(op["state"], "applied", "{key}: {op}");
    assert_eq!(op["action"], action);
    assert_eq!(op["run_id"], run.as_str());
    let states: Vec<Value> = audit_events(env, op["id"].as_str().expect("id"))
        .into_iter()
        .map(|e| e["state"].clone())
        .collect();
    assert_eq!(states, vec![json!("pending"), json!("applied")], "{key}");
    op
}

#[tokio::test]
async fn cos_ops_projects_cron_docs_page_put_delete_are_external_once() {
    let env = admin_env();
    let app = env.router();
    let dir = tempfile::tempdir().expect("tempdir");
    let (id, repo) = project_with_repo(&app, dir.path()).await;
    let page = format!("/api/v1/projects/{id}/docs/page");
    let before = commits(&repo);

    let op = run_external(
        &env,
        "page-put",
        "PUT",
        &page,
        json!({"path": format!("{DOCS}/a.md"), "body": "# A\n"}),
        "docs.page_put",
    )
    .await;
    assert_eq!(
        git(&repo, &["show", format!("main:{DOCS}/a.md").as_str()]),
        "# A\n"
    );
    assert_eq!(commits(&repo), before + 1);
    let etag = op["result"]["etag"].as_str().expect("etag").to_string();

    // A resent request returns the record and does not commit again.
    let (_, _, bearer) = cos_bearer(&env, "page-put-resend");
    let headers = [("authorization", bearer.as_str())];
    let envelope = op_body(
        "same",
        "PUT",
        &page,
        json!({"path": format!("{DOCS}/b.md"), "body": "# B\n"}),
    );
    let first = send(&app, post_json_with(OPS, &envelope, &headers)).await;
    assert_eq!(first.status.as_u16(), 200, "{}", first.text());
    let again = send(&app, post_json_with(OPS, &envelope, &headers)).await;
    assert_eq!(
        again.json()["operation"]["id"],
        first.json()["operation"]["id"]
    );
    assert_eq!(commits(&repo), before + 2);

    // A stale etag is refused by git before any change: settled rejected, no commit.
    let stale = send(
        &app,
        post_json_with(
            OPS,
            &op_body(
                "stale",
                "PUT",
                &page,
                json!({"path": format!("{DOCS}/a.md"), "body": "# A2\n"}),
            ),
            &headers,
        ),
    )
    .await;
    assert_problem(&stale, 409, "etag_mismatch");
    let stale_id: String = common::cos_ops::db(&env)
        .query_row(
            "SELECT id FROM cos_operations WHERE idempotency_key='stale'",
            [],
            |row| row.get(0),
        )
        .expect("row");
    let states: Vec<Value> = audit_events(&env, &stale_id)
        .into_iter()
        .map(|e| e["state"].clone())
        .collect();
    assert_eq!(states, vec![json!("pending"), json!("rejected")]);
    assert_eq!(commits(&repo), before + 2);

    // Delete takes path / etag in the body (operation paths carry no query).
    run_external(
        &env,
        "page-delete",
        "DELETE",
        &page,
        json!({"path": format!("{DOCS}/a.md"), "etag": etag}),
        "docs.page_delete",
    )
    .await;
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["cat-file", "-e", format!("main:{DOCS}/a.md").as_str()])
            .status()
            .map(|s| !s.success())
            .unwrap_or(false),
        "page a is removed on main"
    );
}

#[tokio::test]
async fn cos_ops_projects_cron_docs_init_and_maintenance_are_audited() {
    let env = admin_env();
    let app = env.router();
    let bare = project(&app).await;
    let op = run_external(
        &env,
        "docs-init",
        "POST",
        &format!("/api/v1/projects/{bare}/docs/init"),
        json!({}),
        "docs.init",
    )
    .await;
    assert_eq!(op["result"]["created"], true);
    let path = op["result"]["path"].as_str().expect("path");
    assert!(Path::new(path).join(".git").exists(), "{path}");

    let dir = tempfile::tempdir().expect("tempdir");
    let (id, _) = project_with_repo(&app, dir.path()).await;
    let op = run_external(
        &env,
        "maintenance-audit",
        "POST",
        &format!("/api/v1/projects/{id}/docs/maintenance"),
        json!({"op": "audit"}),
        // The record names the maintenance step (`docs.maintenance_<op>`).
        "docs.maintenance_audit",
    )
    .await;
    assert!(op["result"]["proposal"].is_object(), "{op}");
}

#[tokio::test]
async fn cos_ops_projects_cron_knowledge_page_put_is_external_once() {
    let env = admin_env();
    task_ops::knowledge::init(&env.knowledge_root).expect("knowledge init");
    let kb_commits = |root: &Path| -> usize {
        git(root, &["rev-list", "--count", "HEAD"])
            .trim()
            .parse()
            .expect("count")
    };
    let before = kb_commits(&env.knowledge_root);
    let op = run_external(
        &env,
        "kb-put",
        "PUT",
        "/api/v1/knowledge/page",
        json!({"path": "environment/notes.md", "body": "# メモ\n\n本文\n"}),
        "knowledge.page_put",
    )
    .await;
    assert_eq!(op["target_kind"], "knowledge_page");
    assert!(
        std::fs::read_to_string(env.knowledge_root.join("environment/notes.md"))
            .expect("page")
            .contains("本文")
    );
    assert_eq!(kb_commits(&env.knowledge_root), before + 1);

    // Resent: same record, no second commit. Stale etag: rejected, no commit. `_inbox/`: 403.
    let app = env.router();
    let (_, _, bearer) = cos_bearer(&env, "kb-more");
    let headers = [("authorization", bearer.as_str())];
    let envelope = op_body(
        "kb-put",
        "PUT",
        "/api/v1/knowledge/page",
        json!({"path": "environment/other.md", "body": "# 別\n"}),
    );
    let first = send(&app, post_json_with(OPS, &envelope, &headers)).await;
    let again = send(&app, post_json_with(OPS, &envelope, &headers)).await;
    assert_eq!(
        again.json()["operation"]["id"],
        first.json()["operation"]["id"]
    );
    assert_eq!(kb_commits(&env.knowledge_root), before + 2);
    let stale = send(
        &app,
        post_json_with(
            OPS,
            &op_body(
                "kb-stale",
                "PUT",
                "/api/v1/knowledge/page",
                json!({"path": "environment/notes.md", "body": "# 上書き\n", "etag": "0000"}),
            ),
            &headers,
        ),
    )
    .await;
    assert_problem(&stale, 409, "etag_mismatch");
    assert_eq!(kb_commits(&env.knowledge_root), before + 2);
    let inbox = send(
        &app,
        post_json_with(
            OPS,
            &op_body(
                "kb-inbox",
                "PUT",
                "/api/v1/knowledge/page",
                json!({"path": "_inbox/x.md", "body": "# x\n"}),
            ),
            &headers,
        ),
    )
    .await;
    assert_eq!(inbox.status.as_u16(), 403, "{}", inbox.text());
}
