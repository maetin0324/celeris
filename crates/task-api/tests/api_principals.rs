//! ADR 2026-10-27-api-principals: 要求の主体を token ごとに区別する。
//!
//! 2 本の名前付き token（`human:web` と `operator:claude-code`）・MCP 専用 token・2 つの CoS thread の
//! run credential で同じ操作（コメント・判断の回答）を行い、DB に残る `by`・`author` が主体ごとに分かれることを見る。
//! 偽の主体 header・未知の bearer・MCP 専用 token の変更系は拒否する。
mod common;

use common::cos_ops::{OPS, cos_bearer, op_body};
use common::*;
use serde_json::json;
use task_api::{ApiToken, Principal};
use task_core::{Event, Status, TaskKind, TaskStore};

const WEB_TOKEN: &str = "web-token-value-1";
const OPERATOR_TOKEN: &str = "operator-token-value-2";
const MCP_TOKEN: &str = "celeris-mcp.0123456789abcdef";

fn env() -> TestEnv {
    TestEnv::with(EnvOptions {
        extra_tokens: vec![
            ApiToken::new(Principal::Human { name: "web".into() }, WEB_TOKEN),
            ApiToken::new(
                Principal::Operator {
                    name: "claude-code".into(),
                },
                OPERATOR_TOKEN,
            ),
        ],
        mcp_token: Some(MCP_TOKEN.into()),
        ..EnvOptions::default()
    })
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

async fn comment(app: &axum::Router, task: &task_core::Task, auth: &str, text: &str) -> Resp {
    send(
        app,
        post_json_with(
            &format!("/api/v1/tasks/{}/comments", task.id),
            &json!({"body": text}),
            &[("authorization", auth)],
        ),
    )
    .await
}

fn authors(env: &TestEnv, task: &task_core::Task) -> Vec<String> {
    env.store
        .comments_for(task.id)
        .expect("comments")
        .into_iter()
        .map(|c| c.author.unwrap_or_default())
        .collect()
}

#[tokio::test]
async fn api_principals_two_tokens_are_recorded_as_different_actors() {
    let env = env();
    let app = env.router();
    let task = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&task);

    for (token, text) in [(WEB_TOKEN, "web から"), (OPERATOR_TOKEN, "operator から")] {
        let resp = comment(&app, &task, &bearer(token), text).await;
        assert_eq!(resp.status.as_u16(), 201, "{}", resp.text());
    }
    assert_eq!(authors(&env, &task), ["human:web", "operator:claude-code"]);

    // 判断の回答（棚卸しで operator の操作が人の名義になっていた経路）も主体ごとに残る。
    let approval = new_task(TaskKind::Approval, Status::Ready);
    env.seed(&approval);
    let resp = send(
        &app,
        post_json_with(
            &format!("/api/v1/tasks/{}/approve", approval.id),
            &json!({"note": "ok"}),
            &[("authorization", &bearer(OPERATOR_TOKEN))],
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let by: Vec<String> = env
        .store
        .events_for(approval.id)
        .expect("events")
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::ApprovalDecided { by, .. } => Some(by),
            _ => None,
        })
        .collect();
    assert_eq!(by, ["operator:claude-code"]);
}

#[tokio::test]
async fn api_principals_unknown_bearer_and_spoofed_headers_are_not_trusted() {
    let env = env();
    let app = env.router();
    let task = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&task);

    // 未知の bearer は人へ倒さず 401。
    let resp = comment(&app, &task, &bearer("not-a-token"), "x").await;
    assert_problem(&resp, 401, "unauthorized");
    assert!(!resp.text().contains("not-a-token"));

    // config の token に MCP の client header を添えても主体は token のまま（header を名乗りに使わない）。
    let resp = send(
        &app,
        post_json_with(
            &format!("/api/v1/tasks/{}/comments", task.id),
            &json!({"body": "偽の名乗り"}),
            &[
                ("authorization", &bearer(WEB_TOKEN)),
                ("x-celeris-mcp-client", "C-SPOOF"),
            ],
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 201, "{}", resp.text());
    assert_eq!(authors(&env, &task), ["human:web"]);
}

#[tokio::test]
async fn api_principals_mcp_token_reads_with_a_client_id_and_never_writes() {
    let env = env();
    let app = env.router();
    let task = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&task);

    // client id を添えた読み取りは通る（管理系の読み取りも）。
    let resp = send(
        &app,
        get_with(
            "/api/v1/tasks",
            &[
                ("authorization", &bearer(MCP_TOKEN)),
                ("x-celeris-mcp-client", "01MCPCLIENTA"),
            ],
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    // client id が無い・形が違う MCP token は 401。
    for headers in [
        vec![("authorization", bearer(MCP_TOKEN))],
        vec![
            ("authorization", bearer(MCP_TOKEN)),
            ("x-celeris-mcp-client", "bad id!".to_string()),
        ],
    ] {
        let headers: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let resp = send(&app, get_with("/api/v1/tasks", &headers)).await;
        assert_problem(&resp, 401, "unauthorized");
    }
    // MCP token の変更系は 403（読み取り専用）。
    let resp = send(
        &app,
        post_json_with(
            &format!("/api/v1/tasks/{}/comments", task.id),
            &json!({"body": "MCP から書く"}),
            &[
                ("authorization", &bearer(MCP_TOKEN)),
                ("x-celeris-mcp-client", "01MCPCLIENTA"),
            ],
        ),
    )
    .await;
    assert_problem(&resp, 403, "forbidden");
    assert!(authors(&env, &task).is_empty());
}

#[tokio::test]
async fn api_principals_cos_runs_are_recorded_with_their_thread() {
    let env = env();
    let app = env.router();
    let task = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&task);

    let mut threads = Vec::new();
    for key in ["thread-a", "thread-b"] {
        let (thread_id, _, auth) = cos_bearer(&env, key);
        let resp = send(
            &app,
            post_json_with(
                OPS,
                &op_body(
                    &format!("comment-{key}"),
                    "POST",
                    &format!("/api/v1/tasks/{}/comments", task.id),
                    json!({"body": format!("{key} から")}),
                ),
                &[("authorization", &auth)],
            ),
        )
        .await;
        assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
        threads.push(format!("cos:{thread_id}"));
    }
    assert_ne!(threads[0], threads[1]);
    assert_eq!(authors(&env, &task), threads);
}
