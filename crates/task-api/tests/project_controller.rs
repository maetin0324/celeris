//! ADR 2026-11-04-project-controller A1: `/projects/{id}/controller` の API と CoS op
//! `project.controller_put`・`project.controller_wake`、人の操作の `human_epoch`（接頭辞 `project_wake_api_`）。
mod common;

use common::cos_ops::{OPS, op_body};
use common::*;
use serde_json::{Value, json};
use task_core::ProjectId;
use task_core::chat::{ChatPostMessageRequest, ChatSendMode};
use time::{Duration, OffsetDateTime};

fn bearer_for(env: &TestEnv, thread_id: &str, key: &str) -> String {
    let now = OffsetDateTime::now_utc();
    env.store
        .chat_message_post(
            thread_id,
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
    let run = format!("run-{key}");
    env.store
        .chat_run_claim_next(thread_id, &run, &json!({}), now)
        .expect("claim")
        .expect("run");
    let bearer = task_api::cos::issue_run_bearer(&env.store, thread_id, &run, Duration::hours(1))
        .expect("issue");
    env.store
        .chat_run_record_policy(&run, "1", "test")
        .expect("policy");
    format!("Bearer {bearer}")
}

/// 案件 thread と、その案件だけを許可リストに載せた router。
fn setup(
    allow: bool,
) -> (
    TestEnv,
    axum::Router,
    task_core::chat::ChatThread,
    ProjectId,
) {
    let env = admin_env();
    let thread = project_cos_thread(&env.store, "pilot:controller");
    let project: ProjectId = thread
        .project_id
        .as_deref()
        .expect("project thread")
        .parse()
        .expect("id");
    let allowed = if allow {
        vec![project.to_string()]
    } else {
        vec![]
    };
    let app = task_api::router(env.state.clone().with_project_controller(true, allowed));
    (env, app, thread, project)
}

fn put_body(mode: &str, rev: i64) -> Value {
    json!({"mode": mode, "expected_revision": rev, "reason": "人が試す"})
}

#[tokio::test]
async fn project_wake_api_get_defaults_to_off_and_404s_unknown_projects() {
    let (_env, app, _thread, project) = setup(true);
    let resp = send(
        &app,
        get_admin(&format!("/api/v1/projects/{project}/controller")),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let v = resp.json();
    assert_eq!(v["state"]["mode"], "off");
    assert_eq!(v["state"]["revision"], 0);
    assert_eq!(v["pending_wakes"], json!([]));
    let missing = send(
        &app,
        get_admin(&format!("/api/v1/projects/{}/controller", ProjectId::new())),
    )
    .await;
    assert_eq!(missing.status.as_u16(), 404, "{}", missing.text());
    let decisions = send(
        &app,
        get_admin(&format!(
            "/api/v1/projects/{project}/controller/decisions?limit=5"
        )),
    )
    .await;
    assert_eq!(decisions.status.as_u16(), 200, "{}", decisions.text());
    assert_eq!(decisions.json()["items"], json!([]));
}

#[tokio::test]
async fn project_wake_api_put_needs_allowlist_and_revision() {
    let (env, app, _thread, project) = setup(false);
    let path = format!("/api/v1/projects/{project}/controller");
    let anon = send(&app, put_json_with(&path, &put_body("shadow", 0), &[])).await;
    assert_eq!(anon.status.as_u16(), 401, "{}", anon.text());
    let denied = send(
        &app,
        put_json_with(&path, &put_body("shadow", 0), &admin_headers()),
    )
    .await;
    assert_problem(&denied, 403, "project_controller_not_allowed");

    let app = task_api::router(
        env.state
            .clone()
            .with_project_controller(true, vec![project.to_string()]),
    );
    let ok = send(
        &app,
        put_json_with(&path, &put_body("shadow", 0), &admin_headers()),
    )
    .await;
    assert_eq!(ok.status.as_u16(), 200, "{}", ok.text());
    assert_eq!(ok.json()["state"]["mode"], "shadow");
    assert_eq!(ok.json()["state"]["revision"], 1);
    let stale = send(
        &app,
        put_json_with(&path, &put_body("enabled", 0), &admin_headers()),
    )
    .await;
    assert_problem(&stale, 409, "project_controller_revision_conflict");
    let disabled = task_api::router(
        env.state
            .clone()
            .with_project_controller(false, vec![project.to_string()]),
    );
    let off_daemon = send(
        &disabled,
        put_json_with(&path, &put_body("enabled", 1), &admin_headers()),
    )
    .await;
    assert_problem(&off_daemon, 403, "project_controller_disabled");
}

#[tokio::test]
async fn project_wake_api_wake_is_idempotent_and_inert_when_off() {
    let (env, app, _thread, project) = setup(true);
    let wake = format!("/api/v1/projects/{project}/controller/wake");
    let body = json!({"external_ref": "job-1", "note": "終わった", "idempotency_key": "k1"});
    let off = send(&app, post_admin(&wake, &body)).await;
    assert_eq!(off.status.as_u16(), 200, "{}", off.text());
    assert_eq!(off.json()["added"], false, "mode off takes no wake");
    send(
        &app,
        put_json_with(
            &format!("/api/v1/projects/{project}/controller"),
            &put_body("shadow", 0),
            &admin_headers(),
        ),
    )
    .await;
    let mut added = Vec::new();
    for _ in 0..10 {
        let r = send(&app, post_admin(&wake, &body)).await;
        assert_eq!(r.status.as_u16(), 200, "{}", r.text());
        assert_eq!(r.json()["wake_key"], "external:job-1:k1");
        added.push(r.json()["added"].as_bool().unwrap());
    }
    assert_eq!(added.iter().filter(|a| **a).count(), 1, "{added:?}");
    let pending = env
        .store
        .project_wakes_pending(&project.to_string())
        .unwrap();
    assert_eq!(pending.len(), 1);
    let bad = send(
        &app,
        post_admin(
            &wake,
            &json!({"external_ref": "a:b", "idempotency_key": "k"}),
        ),
    )
    .await;
    assert_eq!(bad.status.as_u16(), 400, "{}", bad.text());
}

#[tokio::test]
async fn project_wake_api_cos_may_only_turn_it_off() {
    let (env, app, thread, project) = setup(true);
    let path = format!("/api/v1/projects/{project}/controller");
    let ok = send(
        &app,
        put_json_with(&path, &put_body("enabled", 0), &admin_headers()),
    )
    .await;
    assert_eq!(ok.status.as_u16(), 200, "{}", ok.text());
    let bearer = bearer_for(&env, &thread.id, "pc");
    let headers = [("authorization", bearer.as_str())];
    let raise = send(
        &app,
        post_json_with(
            OPS,
            &op_body("pc-raise", "PUT", &path, put_body("shadow", 1)),
            &headers,
        ),
    )
    .await;
    assert_problem(&raise, 403, "project_controller_mode_requires_human");
    let wake = send(
        &app,
        post_json_with(
            OPS,
            &op_body(
                "pc-wake",
                "POST",
                &format!("{path}/wake"),
                json!({"external_ref": "job-2", "note": "", "idempotency_key": "w"}),
            ),
            &headers,
        ),
    )
    .await;
    assert_eq!(wake.status.as_u16(), 200, "{}", wake.text());
    assert_eq!(wake.json()["operation"]["state"], "applied");
    let off = send(
        &app,
        post_json_with(
            OPS,
            &op_body("pc-off", "PUT", &path, put_body("off", 1)),
            &headers,
        ),
    )
    .await;
    assert_eq!(off.status.as_u16(), 200, "{}", off.text());
    assert_eq!(off.json()["operation"]["state"], "applied");
    let state = env
        .store
        .project_controller_state(&project.to_string())
        .unwrap()
        .unwrap();
    assert_eq!(state.mode, task_core::project_wake::ControllerMode::Off);
}

#[tokio::test]
async fn project_wake_api_human_patch_bumps_human_epoch() {
    let (env, app, _thread, project) = setup(true);
    send(
        &app,
        put_json_with(
            &format!("/api/v1/projects/{project}/controller"),
            &put_body("shadow", 0),
            &admin_headers(),
        ),
    )
    .await;
    let patched = send(
        &app,
        patch_admin(
            &format!("/api/v1/projects/{project}"),
            &json!({"title": "pilot:renamed"}),
        ),
    )
    .await;
    assert_eq!(patched.status.as_u16(), 200, "{}", patched.text());
    let state = env
        .store
        .project_controller_state(&project.to_string())
        .unwrap()
        .unwrap();
    assert_eq!(state.human_epoch, 1);
    let kinds: Vec<_> = env
        .store
        .project_wakes_pending(&project.to_string())
        .unwrap()
        .into_iter()
        .map(|w| (w.kind, w.wake_key))
        .collect();
    assert_eq!(
        kinds,
        vec![(
            task_core::project_wake::ProjectWakeKind::HumanChanged,
            "human:1".to_string()
        )]
    );
}
