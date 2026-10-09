//! CoS admission and repair of existing project-less tasks, using only an in-process API.
mod common;

use common::*;
use serde_json::{Value, json};
use task_core::chat::{ChatCreateThreadRequest, ChatPostMessageRequest, ChatSendMode};
use task_core::{Status, TaskId, TaskStore};
use time::{Duration, OffsetDateTime};

fn bearer(env: &TestEnv) -> String {
    let now = OffsetDateTime::now_utc();
    let thread = env
        .store
        .chat_thread_create(
            "admin",
            &ChatCreateThreadRequest {
                title: "repository".into(),
                project_id: None,
                client_thread_id: "repo".into(),
            },
            now,
        )
        .unwrap()
        .thread;
    env.store
        .chat_message_post(
            &thread.id,
            &ChatPostMessageRequest {
                client_message_id: "input".into(),
                text: "依頼".into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
            },
            now,
        )
        .unwrap();
    env.store
        .chat_run_claim_next(&thread.id, "run-repo", &json!({}), now)
        .unwrap()
        .unwrap();
    format!(
        "Bearer {}",
        task_api::cos::issue_run_bearer(&env.store, &thread.id, "run-repo", Duration::hours(1))
            .unwrap()
    )
}

fn body() -> Value {
    json!({"title":"修正", "objective":"画面を直す", "genre":"coding",
        "acceptance":[{"type":"reviewer","text":"修正されている"}]})
}

async fn cos_create(app: &axum::Router, bearer: &str, key: &str, body: Value) -> Resp {
    send(app, post_json_with("/api/v1/cos/operations", &json!({
        "idempotency_key":key, "expected_revision":null, "reason":"人の依頼", "policy_version":"4",
        "request":{"method":"POST","path":"/api/v1/tasks","body":body}
    }), &[("authorization",bearer)])).await
}

async fn project(app: &axum::Router, path: &std::path::Path) -> String {
    let resp = send(
        app,
        post_admin("/api/v1/projects", &json!({"title":"p", "request":"r"})),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 201, "{}", resp.text());
    let id = resp.json()["id"].as_str().unwrap().to_owned();
    let repo = send(
        app,
        post_admin(
            &format!("/api/v1/projects/{id}/repos"),
            &json!({
                "name":"code", "kind":"dir", "location":{"kind":"local","path":path}
            }),
        ),
    )
    .await;
    assert_eq!(repo.status.as_u16(), 201, "{}", repo.text());
    id
}

#[tokio::test]
async fn repository_required_cos_rejects_with_reason_and_audit_then_accepts_repos() {
    let env = admin_env();
    let app = env.router();
    let bearer = bearer(&env);
    let mut repo_only = body();
    repo_only["repos"] = json!(["code"]);
    let mut cases = vec![body(), repo_only];
    for (field, value) in [
        ("objective", json!("cargo test を通す")),
        ("objective", json!("web/routes/tasks.tsx の表示を確認する")),
        (
            "acceptance",
            json!([{"type":"reviewer","text":"src/main.rs が修正される"}]),
        ),
        ("acceptance", json!([{"type":"command","cmd":"pnpm test"}])),
    ] {
        let mut b = body();
        b["genre"] = json!("writing");
        b[field] = value;
        cases.push(b);
    }
    // An explicit workspace cannot bypass coding admission.
    for extra in [
        json!({"workspace":"/srv/code"}),
        json!({"workspace":"/srv/code", "workspace_mode":"shared"}),
        json!({"workspace":"/srv/code", "cluster":"test-cluster"}),
    ] {
        let mut b = body();
        b.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        cases.push(b);
    }
    let project = project(&app, env.dir.path()).await;
    // Project alone without a registered repo does not suffice.
    let empty = send(
        &app,
        post_admin("/api/v1/projects", &json!({"title":"empty", "request":"r"})),
    )
    .await
    .json();
    let mut no_repo = body();
    no_repo["project_id"] = empty["id"].clone();
    cases.push(no_repo);
    for (i, b) in cases.iter().enumerate() {
        let resp = cos_create(&app, &bearer, &format!("reject-{i}"), b.clone()).await;
        assert_problem(&resp, 422, "repository_required");
        assert!(
            resp.text()
                .contains("リポジトリを使う task は project_id と repos を付けて起票する")
        );
    }
    let conn = rusqlite::Connection::open(&env.db_path).unwrap();
    let tasks: i64 = conn
        .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(tasks, 0);
    let rejected: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cos_operations WHERE state='rejected'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rejected, cases.len() as i64);
    let mut b = body();
    b["project_id"] = json!(project);
    b["repos"] = json!(["code"]);
    let created = cos_create(&app, &bearer, "valid", b.clone()).await;
    assert!(created.status.is_success(), "{}", created.text());
    assert_eq!(created.json()["operation"]["state"], "applied");
    let again = cos_create(&app, &bearer, "valid", b).await;
    assert!(again.status.is_success(), "{}", again.text());
    let tasks: i64 = conn
        .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(tasks, 1, "replay must not create another task");
    let mut plain = body();
    plain["genre"] = json!("writing");
    assert!(
        cos_create(&app, &bearer, "plain", plain)
            .await
            .status
            .is_success()
    );
}

#[tokio::test]
async fn repository_required_human_warning_and_patch_repair_are_atomic() {
    let env = admin_env();
    let app = env.router();
    let project = project(&app, env.dir.path()).await;
    let mut b = body();
    b["status"] = json!("draft");
    let created = send(&app, post_admin("/api/v1/tasks", &b)).await;
    assert_eq!(created.status.as_u16(), 201, "{}", created.text());
    assert!(
        created
            .header("celeris-warning")
            .unwrap()
            .contains("repository_required")
    );
    let id: TaskId = created.json()["id"].as_str().unwrap().parse().unwrap();
    let path = format!("/api/v1/tasks/{id}");
    let invalid = send(
        &app,
        patch_admin(&path, &json!({"project_id":project,"repos":["missing"]})),
    )
    .await;
    assert_eq!(invalid.status.as_u16(), 422, "{}", invalid.text());
    assert!(env.store.get(id).unwrap().unwrap().project_id.is_none());
    let attached = send(
        &app,
        patch_admin(&path, &json!({"project_id":project,"repos":["code"]})),
    )
    .await;
    assert_eq!(attached.status.as_u16(), 200, "{}", attached.text());
    let saved = env.store.get(id).unwrap().unwrap();
    assert_eq!(saved.project_id.unwrap().to_string(), project);
    assert_eq!(saved.repos[0].name, "code");
    assert_eq!(saved.status, Status::Draft);
    b["project_id"] = json!(project);
    b["repos"] = json!(["code"]);
    let created = send(&app, post_admin("/api/v1/tasks", &b)).await;
    assert_eq!(created.status.as_u16(), 201);
    assert!(created.header("celeris-warning").is_none());
}

#[tokio::test]
async fn repository_required_patch_repairs_blocked_children_but_not_running_tasks() {
    let env = admin_env();
    let app = env.router();
    let project = project(&app, env.dir.path()).await;
    let parent = new_task(task_core::TaskKind::Execute, Status::Draft);
    env.store.insert(&parent).unwrap();
    for (status, expected) in [(Status::Blocked, 200), (Status::Running, 409)] {
        let mut task = new_task(task_core::TaskKind::Execute, status);
        let id = task.id;
        task.parent_id = Some(parent.id);
        task.attempts = 1;
        env.store.insert(&task).unwrap();
        let resp = send(
            &app,
            patch_admin(
                &format!("/api/v1/tasks/{id}"),
                &json!({"project_id":project,"repos":["code"]}),
            ),
        )
        .await;
        assert_eq!(resp.status.as_u16(), expected, "{}", resp.text());
        let saved = env.store.get(id).unwrap().unwrap();
        assert_eq!(saved.project_id.is_some(), expected == 200);
        assert_eq!(saved.status, status);
    }
}
