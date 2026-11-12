//! ADR 2026-11-12-knowledge-usage-per-run D1・D3: CoS run の KB の操作は API の受け側
//! （`POST /api/v1/cos/knowledge-use`）で数え、run credential が決めた run の `chat_runs.knowledge_use_json` に残る。
//!
//! 守る不変条件: 数は credential の run に付き（body に run id を書かせない）、計測していない run（列が NULL）を
//! 0 回にしない。harness の log は読まない。既存の CoS の試験は KB の操作の回数を扱っていない。
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
        ("record", true),
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
                get: 1,
                record: 1,
                failed: 1
            }
        )]
    );

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
