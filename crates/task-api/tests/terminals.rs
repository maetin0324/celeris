//! ADR 2026-10-27-web-terminal D3・D4: `POST /terminals` と `POST /terminals/{id}/close`。
//!
//! 主体 `human:web` だけが開ける（operator・MCP・別の人の token・CoS は 403）。クラスタは ControlMaster が
//! 繋がっていて絶対の作業 dir があるときだけ開け、`BatchMode=yes` の ssh を返す。開始・終了は追記専用の表に残る。
mod common;

use common::*;
use serde_json::json;
use task_api::{ApiToken, Principal};
use task_core::TaskStore;

const WEB_TOKEN: &str = "web-token-value-1";
const OPERATOR_TOKEN: &str = "operator-token-value-2";
const OTHER_HUMAN_TOKEN: &str = "human-other-token-3";
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
            ApiToken::new(Principal::Human { name: "cli".into() }, OTHER_HUMAN_TOKEN),
        ],
        mcp_token: Some(MCP_TOKEN.into()),
        ..EnvOptions::default()
    })
}

fn web() -> String {
    format!("Bearer {WEB_TOKEN}")
}

async fn open(app: &axum::Router, auth: &[(&str, &str)], target: &str) -> Resp {
    send(
        app,
        post_json_with(
            "/api/v1/terminals",
            &json!({"target": target, "client": "localhost:7720 127.0.0.1"}),
            auth,
        ),
    )
    .await
}

fn connected(env: &TestEnv, yes: bool) {
    let mut snap = snapshot(1);
    snap.clusters[0].connected = yes;
    env.daemon_tx.send(Some(snap)).expect("snapshot");
}

#[tokio::test]
async fn terminals_only_human_web_can_open() {
    let env = env();
    let app = env.router();
    let operator = format!("Bearer {OPERATOR_TOKEN}");
    let other = format!("Bearer {OTHER_HUMAN_TOKEN}");
    let mcp = format!("Bearer {MCP_TOKEN}");
    for headers in [
        vec![("authorization", operator.as_str())],
        vec![("authorization", other.as_str())],
        vec![
            ("authorization", mcp.as_str()),
            ("x-celeris-mcp-client", "01MCPCLIENTA"),
        ],
    ] {
        let resp = open(&app, &headers, "local").await;
        assert_eq!(resp.status.as_u16(), 403, "{}", resp.text());
        let resp = send(
            &app,
            post_json_with(
                "/api/v1/terminals/01ABC/close",
                &json!({"reason": "user"}),
                &headers,
            ),
        )
        .await;
        assert_eq!(resp.status.as_u16(), 403, "{}", resp.text());
    }
    // token の無い要求は 401（認証の前に落ちる）。
    let resp = open(&app, &[], "local").await;
    assert_eq!(resp.status.as_u16(), 401, "{}", resp.text());
    assert!(
        env.store
            .terminal_session_events_recent(10)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn terminals_without_tokens_config_is_forbidden() {
    // token が 1 本も無い構成の主体（human:local）も開けない。
    let env = TestEnv::new();
    let app = env.router();
    let resp = open(&app, &[], "local").await;
    assert_problem(&resp, 403, "terminal_forbidden");
}

#[tokio::test]
async fn terminals_local_open_and_close_are_recorded() {
    let env = env();
    let app = env.router();
    let auth = web();
    let resp = open(&app, &[("authorization", &auth)], "local").await;
    assert_eq!(resp.status.as_u16(), 201, "{}", resp.text());
    let body = resp.json();
    let id = body["session_id"].as_str().expect("id").to_string();
    assert_eq!(body["target"], "local");
    assert_eq!(body["cwd"], std::env::var("HOME").unwrap());
    let argv: Vec<String> = serde_json::from_value(body["argv"].clone()).unwrap();
    assert!(argv[0].starts_with('/'), "{argv:?}");
    assert_eq!(argv[1], "-l");

    let resp = send(
        &app,
        post_json_with(
            &format!("/api/v1/terminals/{id}/close"),
            &json!({"reason": "idle_timeout", "duration_secs": 1800, "exit_code": 129}),
            &[("authorization", &auth)],
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 204, "{}", resp.text());
    let rows = env.store.terminal_session_events_recent(10).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].kind, "closed");
    assert_eq!(rows[0].reason.as_deref(), Some("idle_timeout"));
    assert_eq!(rows[0].duration_secs, Some(1800));
    assert_eq!(rows[1].kind, "opened");
    assert_eq!(rows[1].actor, "human:web");
    assert_eq!(rows[1].target, "local");
    assert_eq!(rows[1].client.as_deref(), Some("localhost:7720 127.0.0.1"));

    // 未知の理由は 400、知らない session は 404。
    let resp = send(
        &app,
        post_json_with(
            &format!("/api/v1/terminals/{id}/close"),
            &json!({"reason": "because"}),
            &[("authorization", &auth)],
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 400, "{}", resp.text());
    let resp = send(
        &app,
        post_json_with(
            "/api/v1/terminals/01NOPE/close",
            &json!({"reason": "user"}),
            &[("authorization", &auth)],
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 404, "{}", resp.text());
}

#[tokio::test]
async fn terminals_cluster_requires_connection_and_work_dir() {
    let env = env();
    let app = env.router();
    let auth = web();
    let headers = [("authorization", auth.as_str())];

    // ControlMaster が無い（snapshot が無い・切れている）ときは開かない。
    let resp = open(&app, &headers, "cluster:pegasus").await;
    assert_problem(&resp, 409, "cluster_not_connected");
    connected(&env, false);
    let resp = open(&app, &headers, "cluster:pegasus").await;
    assert_problem(&resp, 409, "cluster_not_connected");

    // 繋がっていても作業 dir が無ければ開かない（`~` には入らない）。
    connected(&env, true);
    let resp = open(&app, &headers, "cluster:pegasus").await;
    assert_problem(&resp, 409, "work_dir_unset");

    env.store
        .cluster_settings_set(
            "pegasus",
            Some("/work/NBB/it's"),
            time::OffsetDateTime::now_utc(),
        )
        .unwrap();
    let resp = open(&app, &headers, "cluster:pegasus").await;
    assert_eq!(resp.status.as_u16(), 201, "{}", resp.text());
    let body = resp.json();
    assert_eq!(body["remote_dir"], "/work/NBB/it's");
    let argv: Vec<String> = serde_json::from_value(body["argv"].clone()).unwrap();
    assert_eq!(
        argv,
        [
            "ssh",
            "-tt",
            "-o",
            "BatchMode=yes",
            "-o",
            "ControlMaster=no",
            "pegasus",
            "--",
            "cd '/work/NBB/it'\\''s' && exec \"${SHELL:-/bin/sh}\" -l",
        ]
    );

    // 知らないクラスタ・形の違う target。
    let resp = open(&app, &headers, "cluster:nowhere").await;
    assert_eq!(resp.status.as_u16(), 404, "{}", resp.text());
    let resp = open(&app, &headers, "sirius").await;
    assert_eq!(resp.status.as_u16(), 400, "{}", resp.text());
    let rows = env.store.terminal_session_events_recent(10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].target, "cluster:pegasus");
}
