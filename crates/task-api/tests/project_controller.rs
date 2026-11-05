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

/// 第 3 フェーズ P2-J: `GET …/controller` の `summary` が未達・達成・待機の理由・許可の範囲・次の wake・guard の上限・
/// 最後の判断の追跡を出す（接頭辞 `project_wake_api_summary_`）。
#[tokio::test]
async fn project_wake_api_summary_shows_outcomes_wait_authority_and_next_wake() {
    let (env, _app, _thread, project) = setup(true);
    let limits = task_ops::project_budget::BudgetLimits {
        max_roots_per_day: 7,
        max_tokens_per_day: 1234,
        ..Default::default()
    };
    let app = task_api::router(
        env.state
            .clone()
            .with_project_controller(true, vec![project.to_string()])
            .with_project_controller_budget(limits.clone()),
    );
    let request = "要約の試験。\n```celeris-outcomes\n[{\"id\":\"o1\",\"title\":\"一つ目\",\"acceptance\":[\"o1 が通る\"]},{\"id\":\"o2\",\"title\":\"二つ目\",\"acceptance\":[\"o2 が通る\"]}]\n```\n";
    env.store
        .write_tx::<_, task_core::StoreError, _>(|tx| {
            task_core::store::SqliteStore::project_set_text_tx(tx, project, None, Some(request))
        })
        .unwrap();
    let path = format!("/api/v1/projects/{project}/controller");
    let get = |app: axum::Router| {
        let path = path.clone();
        async move {
            let r = send(&app, get_admin(&path)).await;
            assert_eq!(r.status.as_u16(), 200, "{}", r.text());
            r.json()["summary"].clone()
        }
    };
    // off: 未達 2 件、待機なし、許可なし、上限は daemon の値、判断なし。
    let s = get(app.clone()).await;
    let ids: Vec<&str> = s["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["o1", "o2"]);
    assert!(
        s["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|o| o["reached"] == false && o["recorded"] == false)
    );
    assert_eq!(s["outcomes_invalid"], false);
    assert_eq!(s["waiting"], Value::Null);
    assert_eq!(s["trace"], Value::Null);
    assert_eq!(s["authority"]["launch_root_allowed"], false);
    assert_eq!(s["authority"]["grants"], json!([]));
    assert_eq!(s["budget_limits"]["max_roots_per_day"], 7);
    assert_eq!(s["budget_limits"]["max_tokens_per_day"], 1234);
    assert_eq!(s["budget_limits"]["ttl_secs"], limits.ttl_secs);

    // 人が enabled にすると起票の許可が 1 件でき、範囲（案件・回数・期限）が出る。
    let ok = send(
        &app,
        put_json_with(&path, &put_body("enabled", 0), &admin_headers()),
    )
    .await;
    assert_eq!(ok.status.as_u16(), 200, "{}", ok.text());
    assert_eq!(
        ok.json()["summary"]["authority"]["launch_root_allowed"],
        true
    );
    let wake = send(
        &app,
        post_admin(
            &format!("{path}/wake"),
            &json!({"external_ref": "job-1", "idempotency_key": "k1"}),
        ),
    )
    .await;
    assert_eq!(wake.json()["added"], true, "{}", wake.text());
    let s = get(app.clone()).await;
    let grants = s["authority"]["grants"].as_array().unwrap();
    assert_eq!(grants.len(), 1, "{s}");
    assert_eq!(grants[0]["action_class"], "launch_root");
    assert_eq!(grants[0]["scope_kind"], "project");
    assert_eq!(grants[0]["scope_ref"], project.to_string());
    assert_eq!(grants[0]["budget_remaining"], 10);
    assert!(grants[0]["expires_at"].as_str().is_some());
    assert_eq!(
        s["next_wake"]["pending_kinds"],
        json!(["external_released"])
    );

    // guard（日次予算）の判断を書くと、待機の理由と追跡にその規則・問いが出る。
    let pid = project.to_string();
    let now = OffsetDateTime::now_utc();
    let lease = env
        .store
        .project_controller_lease(&pid, "test", 60, now)
        .unwrap()
        .expect("lease");
    let ids: Vec<i64> = lease.wakes.iter().map(|w| w.id).collect();
    let decision = task_core::store::NewProjectDecision {
        id: "pcd-summary-1".into(),
        based_on_revision: lease.state.revision,
        human_epoch: lease.state.human_epoch,
        snapshot_sha256: "0".repeat(64),
        request_sha256: "1".repeat(64),
        generation_id: None,
        proposal_json: json!({"schema": "celeris.project-guard/1", "guard": "budget", "detail": {"kind": "tokens"}}).to_string(),
        validation: "pc/guard/budget".into(),
        effect_json: json!({"kind": "open_escalation", "question_key": "budget:tokens:2026-11-05", "category": "guard_budget", "summary": "今日の token の上限に達しました"}).to_string(),
        has_effect: true,
        next_wake_at: Some(now + Duration::hours(3)),
        phase: task_core::project_wake::ControllerPhase::Waiting,
        evidence_mark: None,
    };
    env.store
        .project_controller_commit(&pid, "test", &ids, &decision, now)
        .unwrap();
    let s = get(app.clone()).await;
    let w = &s["waiting"];
    assert_eq!(w["phase"], "waiting", "{s}");
    assert_eq!(w["kind"], "guard");
    assert_eq!(w["rule_id"], "pc/guard/budget");
    assert_eq!(w["decision_id"], "pcd-summary-1");
    let details: Vec<&str> = w["details"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap())
        .collect();
    assert!(
        details.iter().any(|d| d.starts_with("guard budget")),
        "{details:?}"
    );
    assert!(
        details
            .iter()
            .any(|d| d.contains("budget:tokens:2026-11-05") && d.contains("token の上限")),
        "{details:?}"
    );
    assert_eq!(s["trace"]["decision_id"], "pcd-summary-1");
    assert_eq!(s["trace"]["rule_id"], "pc/guard/budget");
    assert_eq!(s["trace"]["effect_state"], "pending");
    assert!(s["next_wake"]["at"].as_str().is_some(), "{s}");
    assert_eq!(s["next_wake"]["pending_kinds"], json!([]));
}
