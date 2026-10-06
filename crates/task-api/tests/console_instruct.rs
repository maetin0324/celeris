//! ADR-0048 D3（Phase 60b）: `POST /console/instruct`。
//!
//! 見るもの: 素の文（scope 無し）は CoS（根ノード）への対話、`@<node-id> ` 始まりの文と
//! `scope=node:<id>` はそのノードへの対話、`scope=project:<id>` は CoS にその案件を紐づけること、
//! 知らないノード・CoS が居ない組織は 404、空文は 422、管理系の 401（両構成）。

mod common;

use common::*;
use serde_json::{Value, json};
use task_core::chat::{ChatMessageQuery, ChatPostMessageRequest, ChatSendMode};
use task_core::{GenreSpec, RoleSpec, Status, TaskStore, Tier};

fn p(path: &str, body: &Value) -> axum::http::Request<axum::body::Body> {
    post_json_with(
        path,
        body,
        &[("authorization", format!("Bearer {TOKEN}").as_str())],
    )
}

fn g(path: &str) -> axum::http::Request<axum::body::Body> {
    get_with(
        path,
        &[("authorization", format!("Bearer {TOKEN}").as_str())],
    )
}

fn env_with_token() -> TestEnv {
    TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        roles: vec![RoleSpec {
            id: "secretary".into(),
            tier: Some(Tier::Standard),
            adapter: Some("claude-code".into()),
            ..RoleSpec::default()
        }],
        genres: vec![GenreSpec {
            id: "secretary".into(),
            description: "人と話す".into(),
            default_role: Some("secretary".into()),
            roles: vec!["secretary".into()],
            ..GenreSpec::default()
        }],
        ..Default::default()
    })
}

/// CoS（`cos`）→ Engineering → Software Engineering。
async fn seed_org(app: &axum::Router) {
    for body in [
        json!({"id": "cos", "name": "Chief of Staff", "kind": "secretary", "genre": "secretary",
               "brief": "人と話す"}),
        json!({"id": "engineering", "name": "Engineering", "kind": "department", "parent_id": "cos"}),
        json!({"id": "software-engineering", "name": "Software Engineering", "kind": "section",
               "parent_id": "engineering", "brief": "コードを直す"}),
    ] {
        let resp = send(app, p("/api/v1/org", &body)).await;
        assert_eq!(resp.status.as_u16(), 201, "{}", resp.text());
    }
}

fn legacy_items(
    env: &TestEnv,
    project_id: Option<task_core::ProjectId>,
) -> Vec<task_core::chat::ChatMessage> {
    let thread = env
        .store
        .chat_legacy_default_thread(project_id, time::OffsetDateTime::now_utc())
        .unwrap();
    env.store
        .chat_message_list(&thread, &ChatMessageQuery::default())
        .unwrap()
        .items
}

#[tokio::test]
async fn cos_chat_legacy_console_instruct_queues_and_keeps_ids() {
    let env = env_with_token();
    let app = env.router();
    seed_org(&app).await;
    let result = send(
        &app,
        p("/api/v1/console/instruct", &json!({"text":"互換入力"})),
    )
    .await;
    assert_eq!(result.status.as_u16(), 202, "{}", result.text());
    let body = result.json();
    assert_eq!(body["node_id"], "cos");
    let task_id = body["task_id"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        env.store.get(task_id).unwrap().unwrap().status,
        Status::Draft
    );
    let items = legacy_items(&env, None);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].text, "互換入力");
    assert_eq!(items[0].state, task_core::chat::ChatMessageState::Queued);
    let old = send(&app, g("/api/v1/org/cos/messages")).await.json();
    assert_eq!(old["items"][0]["id"], body["message_id"]);
}

#[tokio::test]
async fn cos_chat_legacy_org_message_uses_same_queue() {
    let env = env_with_token();
    let app = env.router();
    seed_org(&app).await;
    let result = send(
        &app,
        p("/api/v1/org/cos/messages", &json!({"text":"組織入口"})),
    )
    .await;
    assert_eq!(result.status.as_u16(), 202, "{}", result.text());
    assert_eq!(legacy_items(&env, None)[0].text, "組織入口");
    assert!(result.json()["task_id"].as_str().is_some());
}

#[tokio::test]
async fn cos_chat_legacy_project_has_own_default() {
    let env = env_with_token();
    let app = env.router();
    seed_org(&app).await;
    let created = send(
        &app,
        p(
            "/api/v1/projects",
            &json!({"title":"Scope","request":"調査"}),
        ),
    )
    .await
    .json();
    let project: task_core::ProjectId = created["id"].as_str().unwrap().parse().unwrap();
    let result = send(
        &app,
        p(
            "/api/v1/console/instruct",
            &json!({"text":"案件側", "scope":format!("project:{project}")}),
        ),
    )
    .await;
    assert_eq!(result.status.as_u16(), 202, "{}", result.text());
    assert!(
        legacy_items(&env, Some(project))
            .iter()
            .any(|m| m.text == "案件側")
    );
    assert!(!legacy_items(&env, None).iter().any(|m| m.text == "案件側"));
}

#[tokio::test]
async fn cos_chat_legacy_new_conversation_rotates_only_default() {
    let env = env_with_token();
    let app = env.router();
    seed_org(&app).await;
    let first = env
        .store
        .chat_legacy_default_thread(None, time::OffsetDateTime::now_utc())
        .unwrap();
    let created = send(
        &app,
        p(
            "/api/v1/projects",
            &json!({"title":"Separate","request":"調査"}),
        ),
    )
    .await
    .json();
    let project: task_core::ProjectId = created["id"].as_str().unwrap().parse().unwrap();
    let project_first = env
        .store
        .chat_legacy_default_thread(Some(project), time::OffsetDateTime::now_utc())
        .unwrap();
    let scoped = send(
        &app,
        p(
            &format!("/api/v1/console/new-conversation?scope=project:{project}"),
            &json!({}),
        ),
    )
    .await;
    assert_eq!(scoped.status.as_u16(), 204, "{}", scoped.text());
    assert_ne!(
        project_first,
        env.store
            .chat_legacy_default_thread(Some(project), time::OffsetDateTime::now_utc())
            .unwrap()
    );
    assert_eq!(
        first,
        env.store
            .chat_legacy_default_thread(None, time::OffsetDateTime::now_utc())
            .unwrap()
    );
    let resp = send(&app, p("/api/v1/console/new-conversation", &json!({}))).await;
    assert_eq!(resp.status.as_u16(), 204, "{}", resp.text());
    let second = env
        .store
        .chat_legacy_default_thread(None, time::OffsetDateTime::now_utc())
        .unwrap();
    assert_ne!(first, second);
    assert_eq!(
        env.store.chat_thread_get(&first).unwrap().unwrap().status,
        task_core::chat::ChatThreadStatus::Open
    );
}

#[tokio::test]
async fn cos_chat_legacy_non_cos_keeps_old_path() {
    let env = env_with_token();
    let app = env.router();
    seed_org(&app).await;
    let resp = send(
        &app,
        p(
            "/api/v1/console/instruct",
            &json!({"text":"@software-engineering 修正"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 202, "{}", resp.text());
    let task_id = resp.json()["task_id"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        env.store.get(task_id).unwrap().unwrap().status,
        Status::Ready
    );
    assert!(legacy_items(&env, None).is_empty());
}

#[tokio::test]
async fn cos_chat_legacy_console_reads_chat_without_duplicate_projection() {
    let env = env_with_token();
    let app = env.router();
    seed_org(&app).await;
    let old = send(
        &app,
        p("/api/v1/console/instruct", &json!({"text":"旧から"})),
    )
    .await
    .json();
    let thread = env
        .store
        .chat_legacy_default_thread(None, time::OffsetDateTime::now_utc())
        .unwrap();
    env.store
        .chat_message_post(
            &thread,
            &ChatPostMessageRequest {
                client_message_id: "new-ui".into(),
                text: "新から".into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
            },
            time::OffsetDateTime::now_utc(),
        )
        .unwrap();
    let page = send(&app, g("/api/v1/console?scope=all")).await.json();
    let texts: Vec<_> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|b| b["text"].as_str())
        .collect();
    assert_eq!(texts.iter().filter(|t| **t == "旧から").count(), 1);
    assert_eq!(texts.iter().filter(|t| **t == "新から").count(), 1);
    assert!(old["message_id"].as_str().is_some());

    env.store
        .chat_run_claim_next(
            &thread,
            "legacy-run",
            &json!({}),
            time::OffsetDateTime::now_utc(),
        )
        .unwrap();
    env.store
        .chat_run_finish(
            "legacy-run",
            task_core::chat::ChatRunState::Completed,
            Some("同じ返信"),
            None,
            time::OffsetDateTime::now_utc(),
        )
        .unwrap();
    env.store
        .message_append(&task_core::Message {
            id: task_core::MessageId::new(),
            node_id: "cos".into(),
            project_id: None,
            role: task_core::MessageRole::Node,
            text: "同じ返信".into(),
            run_id: Some("legacy-run".into()),
            task_id: Some(old["task_id"].as_str().unwrap().parse().unwrap()),
            metadata: None,
            created_at: time::OffsetDateTime::now_utc(),
        })
        .unwrap();
    let page = send(&app, g("/api/v1/console?scope=all")).await.json();
    let texts: Vec<_> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|b| b["text"].as_str())
        .collect();
    assert_eq!(texts.iter().filter(|t| **t == "同じ返信").count(), 1);

    env.store
        .chat_run_claim_next(
            &thread,
            "new-run",
            &json!({}),
            time::OffsetDateTime::now_utc(),
        )
        .unwrap();
    env.store
        .chat_run_finish(
            "new-run",
            task_core::chat::ChatRunState::Completed,
            Some("新しい返信"),
            None,
            time::OffsetDateTime::now_utc(),
        )
        .unwrap();
    let page = send(&app, g("/api/v1/console?scope=all")).await.json();
    let reply = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["text"] == "新しい返信")
        .expect("new reply");
    assert_eq!(reply["run_id"], "new-run");
}

/// scope 無しの素の文は CoS（`OrgKind::Secretary` の根ノード）への対話になる。
#[tokio::test]
async fn a_plain_instruction_talks_to_the_cos() {
    let env = env_with_token();
    let app = env.router();
    seed_org(&app).await;

    let resp = send(
        &app,
        p(
            "/api/v1/console/instruct",
            &json!({"text": "今週の状況を教えて"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 202, "{}", resp.text());
    let body = resp.json();
    assert_eq!(body["node_id"], "cos");
    let task_id: task_core::TaskId = body["task_id"]
        .as_str()
        .expect("task_id")
        .parse()
        .expect("ulid");
    let task = env.store.get(task_id).expect("get").expect("task");
    assert_eq!(task.assignee.as_deref(), Some("cos"));
    assert_eq!(task.objective, "今週の状況を教えて");

    let items = send(&app, g("/api/v1/org/cos/messages")).await.json()["items"]
        .as_array()
        .cloned()
        .expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["text"], "今週の状況を教えて");
}

/// `@<node-id> ` 始まりの文はそのノードへの対話になり、`@mention` は本文から取り除かれる。
#[tokio::test]
async fn an_at_mention_talks_to_that_node_and_strips_the_mention() {
    let env = env_with_token();
    let app = env.router();
    seed_org(&app).await;

    let resp = send(
        &app,
        p(
            "/api/v1/console/instruct",
            &json!({"text": "@software-engineering このバグを直して"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 202, "{}", resp.text());
    let body = resp.json();
    assert_eq!(body["node_id"], "software-engineering");

    let items = send(&app, g("/api/v1/org/software-engineering/messages"))
        .await
        .json()["items"]
        .as_array()
        .cloned()
        .expect("items");
    assert_eq!(
        items[0]["text"], "このバグを直して",
        "@mention は取り除かれる"
    );
}

/// `scope=node:<id>` は `@mention` が無くてもそのノードへの対話になる。
#[tokio::test]
async fn an_explicit_node_scope_routes_without_a_mention() {
    let env = env_with_token();
    let app = env.router();
    seed_org(&app).await;

    let resp = send(
        &app,
        p(
            "/api/v1/console/instruct",
            &json!({"text": "このバグを直して", "scope": "node:software-engineering"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 202, "{}", resp.text());
    assert_eq!(resp.json()["node_id"], "software-engineering");
}

/// `scope=project:<id>` は CoS への対話をその案件に紐づける。
#[tokio::test]
async fn a_project_scope_binds_the_cos_conversation_to_that_project() {
    let env = env_with_token();
    let app = env.router();
    seed_org(&app).await;

    let created = send(
        &app,
        p(
            "/api/v1/projects",
            &json!({"title": "Pluvio", "request": "調べて"}),
        ),
    )
    .await;
    assert_eq!(created.status.as_u16(), 201, "{}", created.text());
    let project_id = created.json()["id"].as_str().expect("id").to_string();

    let resp = send(
        &app,
        p(
            "/api/v1/console/instruct",
            &json!({"text": "進捗どうですか", "scope": format!("project:{project_id}")}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 202, "{}", resp.text());
    assert_eq!(resp.json()["node_id"], "cos");
    let task_id: task_core::TaskId = resp.json()["task_id"]
        .as_str()
        .expect("task_id")
        .parse()
        .expect("ulid");
    let task = env.store.get(task_id).expect("get").expect("task");
    assert_eq!(task.project_id.map(|p| p.to_string()), Some(project_id));
}

/// 知らないノードへの `@mention` / `scope=node:` は 404。CoS が組織に居なければそれも 404。
#[tokio::test]
async fn unknown_targets_are_404() {
    let env = env_with_token();
    let app = env.router();
    seed_org(&app).await;

    assert_problem(
        &send(
            &app,
            p("/api/v1/console/instruct", &json!({"text": "@ghost hi"})),
        )
        .await,
        404,
        "org_node_not_found",
    );
    assert_problem(
        &send(
            &app,
            p(
                "/api/v1/console/instruct",
                &json!({"text": "hi", "scope": "node:ghost"}),
            ),
        )
        .await,
        404,
        "org_node_not_found",
    );

    // 組織を種蒔きしていない（CoS が居ない）環境。
    let empty = env_with_token();
    let empty_app = empty.router();
    assert_problem(
        &send(
            &empty_app,
            p("/api/v1/console/instruct", &json!({"text": "hi"})),
        )
        .await,
        404,
        "org_node_not_found",
    );
}

/// 空白だけの本文は 422（`task_ops::conversation::start` の検証）。形の違う `scope` は 400。
#[tokio::test]
async fn blank_text_is_422_and_a_malformed_scope_is_400() {
    let env = env_with_token();
    let app = env.router();
    seed_org(&app).await;

    assert_problem(
        &send(&app, p("/api/v1/console/instruct", &json!({"text": "   "}))).await,
        422,
        "validation",
    );
    assert_problem(
        &send(
            &app,
            p(
                "/api/v1/console/instruct",
                &json!({"text": "hi", "scope": "bogus"}),
            ),
        )
        .await,
        400,
        "bad_request",
    );
}

/// 管理系: トークンを付けない要求は 401。`token_file` 未設定構成でも 401。
#[tokio::test]
async fn instructing_the_console_is_an_admin_endpoint() {
    let env = env_with_token();
    let app = env.router();
    seed_org(&app).await;
    assert_problem(
        &send(
            &app,
            post_json("/api/v1/console/instruct", &json!({"text": "hi"})),
        )
        .await,
        401,
        "unauthorized",
    );

    let open = TestEnv::with(EnvOptions::default());
    let open_app = open.router();
    assert_problem(
        &send(
            &open_app,
            post_json("/api/v1/console/instruct", &json!({"text": "hi"})),
        )
        .await,
        401,
        "unauthorized",
    );
}
