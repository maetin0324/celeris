//! ops-r1 intake を実 task-api router（in-process、一時 DB）に当てる。案件 CoS thread の message として受付が入り、
//! task は 1 件も作られず（`POST /api/v1/tasks` を呼ばない）、記録の actor は token の主体（`human:test`）になる。
//! 実 LLM・外部ネットワークは使わない（CoS run は起こさない）。
#[path = "../../task-api/tests/common/mod.rs"]
mod api_common;

use std::sync::{Arc, Mutex};

use api_common::{TEST_ACTOR, admin_env, get_admin, post_admin, project_cos_thread, send};
use celeris_campaign::{
    celeris_exec::{HttpResponse, Transport},
    human_ops::ActorKind,
    ops_r1_intake::{IntakeInput, Mode, client_message_id, intake, sha256_hex},
};
use serde_json::{Value, json};

const REQUEST: &str = "固定依頼: K4 の case を解いて報告する。\n";

struct RouterTransport {
    app: axum::Router,
    rt: tokio::runtime::Handle,
    calls: Arc<Mutex<Vec<(String, String)>>>,
}

impl Transport for RouterTransport {
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<HttpResponse, String> {
        self.calls
            .lock()
            .map_err(|e| e.to_string())?
            .push((method.into(), path.into()));
        let req = match (method, body) {
            ("GET", None) => get_admin(path),
            ("POST", Some(b)) => post_admin(path, b),
            _ => return Err(format!("unexpected {method} {path}")),
        };
        let r = self.rt.block_on(send(&self.app, req));
        let body = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
        Ok(HttpResponse {
            status: r.status.as_u16(),
            body,
        })
    }
}

fn task_count(db: &std::path::Path) -> i64 {
    rusqlite::Connection::open(db)
        .expect("db")
        .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
        .expect("count")
}

#[tokio::test(flavor = "multi_thread")]
async fn ops_r1_intake_real_router_send_and_bind_without_task_api() {
    let env = admin_env();
    let thread = project_cos_thread(&env.store, "ops-r1-measure");
    let app = env.router();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let t = RouterTransport {
        app: app.clone(),
        rt: tokio::runtime::Handle::current(),
        calls: calls.clone(),
    };
    let tid = thread.id.clone();
    let input = move |trial: &str, mode: Mode| IntakeInput {
        thread_id: tid.clone(),
        trial_id: trial.into(),
        condition_id: "ops-r1-B".into(),
        request_text: REQUEST.into(),
        mode,
    };
    // --send: 人の token で固定依頼文を送る。
    let (sent, t) = tokio::task::spawn_blocking(move || {
        let r = intake(&t, &input("ops-r1-B-k4-1", Mode::Send));
        (r, t)
    })
    .await
    .expect("join");
    let sent = sent.expect("send");
    assert_eq!(sent.thread_id, thread.id);
    assert_eq!(sent.project_id, thread.project_id.clone().expect("project"));
    assert_eq!(sent.actor.kind, ActorKind::Human);
    assert_eq!(sent.actor.id, TEST_ACTOR);
    assert_eq!(sent.request_sha256, sha256_hex(REQUEST.as_bytes()));
    assert_eq!(
        sent.client_message_id,
        Some(client_message_id("ops-r1-B-k4-1").expect("key"))
    );
    assert!(sent.accepted_at.ends_with('Z'), "{}", sent.accepted_at);

    // 人が web で送ったのと同じ経路（POST …/messages）の message を --bind-message で結ぶ。
    let posted = send(
        &app,
        post_admin(
            &format!("/api/v1/chat/threads/{}/messages", thread.id),
            &json!({"client_message_id": "web-1", "text": REQUEST, "attachment_ids": [],
                "reply_to_id": null, "mode": "queue", "resume_queue": false}),
        ),
    )
    .await;
    assert_eq!(posted.status.as_u16(), 202, "{}", posted.text());
    let mid = posted.json()["message"]["id"]
        .as_str()
        .expect("id")
        .to_string();
    let thread2 = thread.clone();
    let bound = tokio::task::spawn_blocking(move || {
        intake(
            &t,
            &IntakeInput {
                thread_id: thread2.id.clone(),
                trial_id: "ops-r1-B-k4-2".into(),
                condition_id: "ops-r1-B".into(),
                request_text: REQUEST.into(),
                mode: Mode::Bind(mid),
            },
        )
    })
    .await
    .expect("join")
    .expect("bind");
    assert_eq!(bound.intake_mode, "bind");
    assert_eq!(bound.actor.id, TEST_ACTOR);
    assert!(bound.message_seq > sent.message_seq);

    // task は作られず、呼んだのは chat thread の endpoint だけ。
    assert_eq!(task_count(&env.db_path), 0);
    let calls = calls.lock().expect("calls").clone();
    assert!(
        calls
            .iter()
            .all(|(_, p)| p.starts_with("/api/v1/chat/threads/")),
        "{calls:?}"
    );
    let listed = send(
        &app,
        get_admin(&format!("/api/v1/chat/threads/{}/messages", thread.id)),
    )
    .await;
    let items = listed.json()["items"].as_array().expect("items").len();
    assert_eq!(items, 2, "one message per intake");
}
