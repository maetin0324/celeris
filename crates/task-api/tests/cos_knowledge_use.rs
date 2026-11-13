//! ADR 2026-11-12-knowledge-usage-per-run D1・D3: CoS run の KB の操作は API の受け側（`GET /knowledge/tree`・
//! `GET /knowledge/page`・operation `knowledge.record`・`POST /api/v1/cos/knowledge-use`）で数え、run
//! credential が決めた run の `chat_runs.knowledge_use_json` に残る。
//!
//! 守る不変条件: 数は credential の run に付き（body に run id を書かせない）、CoS が常設の文脈の案内どおり API で
//! KB を読んでも数えられ、人の token の呼び出しは数えず、計測していない run（列が NULL）を 0 回にしない。harness の
//! log は読まない。既存の CoS の試験は KB の操作の回数を扱っていない。
mod common;

use common::*;
use serde_json::{Value, json};
use task_core::KnowledgeUseStore;
use task_core::chat::{ChatPostMessageRequest, ChatSendMode};
use time::{Duration, OffsetDateTime};

/// A live CoS run on a project thread: `(project_id, thread_id, run_id)`.
fn live_run(env: &TestEnv, key: &str) -> (task_core::ProjectId, String, String) {
    let store = &env.store;
    let now = OffsetDateTime::now_utc();
    let thread = common::project_cos_thread(store, &task_core::ProjectId::new().to_string());
    let project = thread
        .project_id
        .as_deref()
        .and_then(|p| p.parse().ok())
        .expect("project thread");
    store
        .chat_message_post(
            &thread.id,
            &ChatPostMessageRequest {
                client_message_id: format!("message-{key}"),
                text: "input".into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
            },
            now,
        )
        .expect("message");
    let run_id = format!("run-{key}");
    store
        .chat_run_claim_next(&thread.id, &run_id, &json!({}), now)
        .expect("claim")
        .expect("run");
    (project, thread.id, run_id)
}

fn use_body(op: &str, ok: bool) -> Value {
    json!({ "op": op, "ok": ok })
}

#[tokio::test]
async fn cos_run_knowledge_ops_are_counted_on_the_credential_run() {
    let env = admin_env();
    let app = env.router();
    let (project, thread, run) = live_run(&env, "kb");
    let bearer = format!(
        "Bearer {}",
        task_api::cos::issue_run_bearer(&env.store, &thread, &run, Duration::hours(1))
            .expect("issue")
    );
    let headers = [("authorization", bearer.as_str())];
    for (op, ok) in [
        ("search", true),
        ("search", true),
        ("get", false),
        ("get", true),
    ] {
        let res = send(
            &app,
            post_json_with("/api/v1/cos/knowledge-use", &use_body(op, ok), &headers),
        )
        .await;
        assert_eq!(res.status.as_u16(), 200, "{}", res.text());
        assert_eq!(res.json()["run_id"], json!(run));
    }
    let uses = env.store.chat_run_knowledge_uses().expect("uses");
    assert_eq!(
        uses,
        vec![(
            Some(project),
            task_core::KnowledgeUse {
                search: 2,
                get: 2,
                record: 0,
                failed: 1
            }
        )]
    );

    // record は operation `knowledge.record` の受け側が数えるので、ここでは受けない（二重に数えない）。
    let res = send(
        &app,
        post_json_with(
            "/api/v1/cos/knowledge-use",
            &use_body("record", true),
            &headers,
        ),
    )
    .await;
    assert_eq!(res.status.as_u16(), 422, "{}", res.text());

    // run id を body で名乗っても受けない（未知の欄は 400）。
    let res = send(
        &app,
        post_json_with(
            "/api/v1/cos/knowledge-use",
            &json!({ "op": "get", "ok": true, "run_id": "other" }),
            &headers,
        ),
    )
    .await;
    assert_eq!(res.status.as_u16(), 400, "{}", res.text());

    // CoS の run credential でない呼び出し（人の token）は数えない。
    let res = send(
        &app,
        post_json_with(
            "/api/v1/cos/knowledge-use",
            &use_body("get", true),
            &admin_headers(),
        ),
    )
    .await;
    assert!(res.status.is_client_error(), "{}", res.text());
}

#[test]
fn unmeasured_cos_runs_stay_null_and_are_not_counted() {
    let env = admin_env();
    let (_, _, run) = live_run(&env, "old");
    let conn = rusqlite::Connection::open(&env.db_path).expect("open db");
    conn.execute(
        "UPDATE chat_runs SET knowledge_use_json = NULL WHERE run_id = ?1",
        [&run],
    )
    .expect("null");
    assert_eq!(
        env.store
            .chat_run_knowledge_use_add(&run, task_core::KnowledgeOp::Get, true)
            .expect("add"),
        None
    );
    assert!(
        env.store
            .chat_run_knowledge_uses()
            .expect("uses")
            .is_empty()
    );
}

/// CoS の常設の文脈が案内する API（tree で探す・page で読む・operation `knowledge.record` で記録する）を、受け側が
/// credential の run に数える。人の token の同じ呼び出しは数えない。
#[tokio::test]
async fn cos_reads_and_records_through_the_knowledge_api_are_counted_on_the_credential_run() {
    let env = admin_env();
    task_ops::knowledge::init(&env.knowledge_root).expect("knowledge init");
    let app = env.router();
    let (project, thread, run) = live_run(&env, "api");
    env.store
        .chat_run_record_policy(&run, "1", "test")
        .expect("policy");
    let bearer = format!(
        "Bearer {}",
        task_api::cos::issue_run_bearer(&env.store, &thread, &run, Duration::hours(1))
            .expect("issue")
    );
    let cos = [("authorization", bearer.as_str())];

    let tree = send(&app, get_with("/api/v1/knowledge/tree?q=fern03", &cos)).await;
    assert_eq!(tree.status.as_u16(), 200, "{}", tree.text());
    let missing = send(
        &app,
        get_with("/api/v1/knowledge/page?path=user/missing.md", &cos),
    )
    .await;
    assert_eq!(missing.status.as_u16(), 404, "{}", missing.text());
    let record = send(
        &app,
        post_json_with(
            "/api/v1/cos/operations",
            &json!({
                "idempotency_key": "kb-use-1",
                "expected_revision": null,
                "reason": "人が KB に残すよう依頼した",
                "policy_version": "1",
                "request": {"method": "POST", "path": "/api/v1/knowledge/inbox", "body": {
                    "title": "fern03 の使い方",
                    "scope": "user",
                    "body": "要点。",
                    "sources": ["message:01J00000000000000000000000"],
                }},
            }),
            &cos,
        ),
    )
    .await;
    assert_eq!(record.status.as_u16(), 200, "{}", record.text());
    assert_eq!(
        record.json()["operation"]["state"],
        "applied",
        "{}",
        record.text()
    );

    // 人の token の読み取りは CoS run に数えない。
    let human = send(&app, get_admin("/api/v1/knowledge/tree?q=fern03")).await;
    assert_eq!(human.status.as_u16(), 200, "{}", human.text());

    assert_eq!(
        env.store.chat_run_knowledge_uses().expect("uses"),
        vec![(
            Some(project),
            task_core::KnowledgeUse {
                search: 1,
                get: 1,
                record: 1,
                failed: 1
            }
        )]
    );
}
