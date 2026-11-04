//! ADR 2026-11-04-project-controller A9: 非本番 pilot の API 側の検査（接頭辞 `long_horizon_pilot_`）。
//! 自動運営が有効な案件でも本番の操作と人の確認は人のままで、`off` に戻すと CoS の起票が従来の検査に戻る。
//! 一時 DB だけを開き、LLM を呼ばない。`LONG_HORIZON_PILOT_RAW_DIR` があれば `gates.jsonl` を書く。

mod common;

use common::*;
use serde_json::{Value, json};
use task_api::{ApiToken, Principal};
use task_core::project_wake::ControllerMode;
use task_core::{Check, Status, TaskKind, TaskStore};
use time::{Duration, OffsetDateTime};

const WEB_TOKEN: &str = "web-token-value-a9";
const OPS: &str = "/api/v1/cos/operations";

struct Cos {
    env: TestEnv,
    app: axum::Router,
    project: String,
    bearer: String,
}

async fn project_cos() -> Cos {
    let env = TestEnv::with(EnvOptions {
        extra_tokens: vec![ApiToken::new(
            Principal::Human { name: "web".into() },
            WEB_TOKEN,
        )],
        ..EnvOptions::default()
    });
    let app = env.router();
    let thread = project_cos_thread(&env.store, "pilot:a9-gates");
    let project = thread.project_id.clone().expect("project");
    let run = "run-a9";
    send(
        &app,
        post_json_with(
            &format!("/api/v1/chat/threads/{}/messages", thread.id),
            &json!({"client_message_id": "h-1", "text": "進めて", "attachment_ids": [],
                    "reply_to_id": null, "mode": "queue", "resume_queue": false}),
            &[("authorization", &format!("Bearer {WEB_TOKEN}"))],
        ),
    )
    .await;
    env.store
        .chat_run_claim_next(&thread.id, run, &json!({}), OffsetDateTime::now_utc())
        .expect("claim")
        .expect("run");
    let bearer = task_api::cos::issue_run_bearer(&env.store, &thread.id, run, Duration::hours(1))
        .expect("issue");
    env.store
        .chat_run_record_policy(run, "5", "test")
        .expect("policy");
    Cos {
        env,
        app,
        project,
        bearer: format!("Bearer {bearer}"),
    }
}

impl Cos {
    async fn op(&self, key: &str, method: &str, path: &str, body: Value) -> Resp {
        send(
            &self.app,
            post_json_with(
                OPS,
                &json!({"idempotency_key": key, "expected_revision": null, "reason": "pilot の検査",
                        "policy_version": "5", "request": {"method": method, "path": path, "body": body}}),
                &[("authorization", &self.bearer)],
            ),
        )
        .await
    }

    fn set_mode(&self, mode: ControllerMode) {
        self.env
            .store
            .project_controller_set_mode(
                &self.project,
                mode,
                None,
                "human:web",
                OffsetDateTime::now_utc(),
            )
            .expect("mode");
    }
}

fn raw(test: &str, op: &str, status: u16, code: &str) {
    let Ok(dir) = std::env::var("LONG_HORIZON_PILOT_RAW_DIR") else {
        return;
    };
    let row = json!({"schema": "celeris.long-horizon-record/1", "dataset": "long_horizon_pilot_virtual",
                     "test": test, "op": op, "status": status, "code": code});
    use std::io::Write;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::path::Path::new(&dir).join("gates.jsonl"))
        .and_then(|mut f| f.write_all(format!("{row}\n").as_bytes()))
        .expect("raw");
}

fn code_of(resp: &Resp) -> String {
    resp.json()["code"].as_str().unwrap_or_default().to_owned()
}

#[tokio::test]
async fn long_horizon_pilot_api_production_gate_is_kept() {
    let test = "long_horizon_pilot_api_production_gate_is_kept";
    let c = project_cos().await;
    c.set_mode(ControllerMode::Enabled);
    let promote = c
        .op(
            "prod-1",
            "POST",
            "/api/v1/releases/aaaaaaaaaaaa/promote",
            json!(null),
        )
        .await;
    assert_problem(&promote, 422, "production_requires_human");
    raw(test, "release.promote", 422, &code_of(&promote));
    let secret = c
        .op("prod-2", "PUT", "/api/v1/secrets/s1", json!({"value": "x"}))
        .await;
    assert_eq!(secret.status.as_u16(), 422, "{}", secret.text());
    raw(test, "secret.put", 422, &code_of(&secret));
    // CoS は mode を上げられない（下げるだけ）。
    let raise = c
        .op(
            "mode-1",
            "PUT",
            &format!("/api/v1/projects/{}/controller", c.project),
            json!({"mode": "enabled", "expected_revision": 1, "reason": "CoS が上げる"}),
        )
        .await;
    assert!(
        raise.status.is_client_error(),
        "CoS raised the mode: {}",
        raise.text()
    );
    raw(
        test,
        "project.controller_put",
        raise.status.as_u16(),
        &code_of(&raise),
    );
}

#[tokio::test]
async fn long_horizon_pilot_api_human_check_stays_with_the_human() {
    let test = "long_horizon_pilot_api_human_check_stays_with_the_human";
    let c = project_cos().await;
    c.set_mode(ControllerMode::Enabled);
    let mut t = new_task(TaskKind::Execute, Status::Reviewing);
    t.project_id = Some(c.project.parse().expect("pid"));
    t.acceptance[0].check = Check::Human;
    c.env.seed(&t);
    let approve = c
        .op(
            "check-1",
            "POST",
            &format!("/api/v1/tasks/{}/approve", t.id),
            json!({"reason": "CoS が代わりに通す"}),
        )
        .await;
    assert!(
        approve.status.is_client_error(),
        "CoS approved a human check: {}",
        approve.text()
    );
    raw(
        test,
        "task.approve",
        approve.status.as_u16(),
        &code_of(&approve),
    );
    let stored = c.env.store.get(t.id).expect("get").expect("task");
    assert_eq!(stored.status, Status::Reviewing, "the task did not move");
}

#[tokio::test]
async fn long_horizon_pilot_api_off_restores_the_legacy_cos_launch() {
    let test = "long_horizon_pilot_api_off_restores_the_legacy_cos_launch";
    let c = project_cos().await;
    c.set_mode(ControllerMode::Enabled);
    let body = json!({"title": "CoS の root", "objective": "続き", "project_id": c.project,
                      "acceptance": [{"type": "reviewer", "text": "続きが終わる"}]});
    let refused = c
        .op("launch-1", "POST", "/api/v1/tasks", body.clone())
        .await;
    assert_problem(&refused, 422, "project_controller_owns_launch");
    raw(test, "task.create(enabled)", 422, &code_of(&refused));
    // 誰でも off に戻せる: CoS が `project.controller_put` で下げる。
    let revision = c
        .env
        .store
        .project_controller_state(&c.project)
        .expect("state")
        .expect("row")
        .revision;
    let off = c
        .op(
            "mode-off",
            "PUT",
            &format!("/api/v1/projects/{}/controller", c.project),
            json!({"mode": "off", "expected_revision": revision, "reason": "従来の CoS に戻す"}),
        )
        .await;
    assert!(off.status.is_success(), "{}", off.text());
    raw(test, "project.controller_put(off)", off.status.as_u16(), "");
    let legacy = c.op("launch-2", "POST", "/api/v1/tasks", body).await;
    assert_ne!(
        code_of(&legacy),
        "project_controller_owns_launch",
        "{}",
        legacy.text()
    );
    raw(
        test,
        "task.create(off)",
        legacy.status.as_u16(),
        &code_of(&legacy),
    );
}
