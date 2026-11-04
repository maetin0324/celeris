//! ADR 2026-11-04-project-controller A3（D5・D8）: 案件 CoS が人の目的・受け入れ条件・controller の起票・
//! 本番を独断で変えられないことの拒否の試験（接頭辞 `project_cos_`）。LLM を呼ばない（fake の CoS run の credential）。
//! `PROJECT_COS_RAW_DIR` があれば拒否の code を `celeris.long-horizon-record/1` の JSONL で書く。

mod common;

use common::*;
use serde_json::{Value, json};
use task_api::{ApiToken, Principal};
use task_core::project_wake::ControllerMode;
use task_core::{Status, TaskKind, TaskStore};

use time::{Duration, OffsetDateTime};

const WEB_TOKEN: &str = "web-token-value-a3";
const OPS: &str = "/api/v1/cos/operations";

fn env() -> TestEnv {
    TestEnv::with(EnvOptions {
        extra_tokens: vec![ApiToken::new(
            Principal::Human { name: "web".into() },
            WEB_TOKEN,
        )],
        ..EnvOptions::default()
    })
}

struct Cos {
    env: TestEnv,
    app: axum::Router,
    thread: String,
    project: String,
    said: String,
    bearer: String,
}

/// 案件 thread に人が 1 回発言し、案件 CoS の run がそれを取る（`instructed_by` に引ける発言）。
async fn project_cos() -> Cos {
    let env = env();
    let app = env.router();
    let thread = project_cos_thread(&env.store, "pilot:a3-guard");
    let project = thread.project_id.clone().expect("project");
    let resp = send(
        &app,
        post_json_with(
            &format!("/api/v1/chat/threads/{}/messages", thread.id),
            &json!({"client_message_id": "h-1", "text": "依頼文を『計測を 5 回』に直して", "attachment_ids": [],
                    "reply_to_id": null, "mode": "queue", "resume_queue": false}),
            &[("authorization", &format!("Bearer {WEB_TOKEN}"))],
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 202, "{}", resp.text());
    let said = resp.json()["message"]["id"]
        .as_str()
        .expect("id")
        .to_owned();
    let run = "run-a3";
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
        thread: thread.id,
        project,
        said,
        bearer: format!("Bearer {bearer}"),
    }
}

impl Cos {
    async fn op(&self, key: &str, method: &str, path: &str, body: Value, instructed: bool) -> Resp {
        let mut op = json!({
            "idempotency_key": key, "expected_revision": null, "reason": "案件 CoS の判断",
            "policy_version": "5", "request": {"method": method, "path": path, "body": body},
        });
        if instructed {
            op["instructed_by"] = json!(self.said);
        }
        send(
            &self.app,
            post_json_with(OPS, &op, &[("authorization", &self.bearer)]),
        )
        .await
    }

    fn rejected(&self) -> i64 {
        rusqlite::Connection::open(&self.env.db_path)
            .expect("db")
            .query_row(
                "SELECT COUNT(*) FROM cos_operations WHERE state = 'rejected'",
                [],
                |r| r.get(0),
            )
            .expect("count")
    }

    fn task_in_project(&self, status: Status) -> task_core::Task {
        let mut t = new_task(TaskKind::Execute, status);
        t.project_id = Some(self.project.parse().expect("pid"));
        self.env.seed(&t);
        t
    }
}

fn raw(test: &str, code: &str, status: u16) {
    let Ok(dir) = std::env::var("PROJECT_COS_RAW_DIR") else {
        return;
    };
    let row = json!({"schema": "celeris.long-horizon-record/1", "test": test, "refusal_code": code, "status": status});
    use std::io::Write;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::path::Path::new(&dir).join("refusals.jsonl"))
        .and_then(|mut f| f.write_all(format!("{row}\n").as_bytes()))
        .expect("raw");
}

#[tokio::test]
async fn project_cos_refuses_request_edit() {
    let c = project_cos().await;
    let path = format!("/api/v1/projects/{}", c.project);
    let before = c.rejected();
    let resp = c
        .op(
            "req-1",
            "PATCH",
            &path,
            json!({"request": "CoS が書き換えた依頼"}),
            false,
        )
        .await;
    assert_problem(&resp, 422, "project_request_human_only");
    assert_eq!(c.rejected(), before + 1, "a rejected audit row");
    let p = c
        .env
        .store
        .project_get(c.project.parse().expect("pid"))
        .expect("get")
        .expect("project");
    assert_eq!(p.request, "pilot:a3-guard");
    raw(
        "project_cos_refuses_request_edit",
        "project_request_human_only",
        422,
    );
    // 人の発言を引けば通る（CoS は取り次ぐだけ）。
    let ok = c
        .op(
            "req-2",
            "PATCH",
            &path,
            json!({"request": "計測を 5 回"}),
            true,
        )
        .await;
    assert_eq!(ok.status.as_u16(), 200, "{}", ok.text());
}

#[tokio::test]
async fn project_cos_refuses_goal_edit() {
    let c = project_cos().await;
    let t = c.task_in_project(Status::Ready);
    for (i, body) in [
        json!({"objective": "CoS が狭めた目的"}),
        json!({"acceptance": [{"type": "reviewer", "text": "何でも可"}]}),
    ]
    .into_iter()
    .enumerate()
    {
        let resp = c
            .op(
                &format!("goal-{i}"),
                "PATCH",
                &format!("/api/v1/tasks/{}", t.id),
                body,
                false,
            )
            .await;
        assert_problem(&resp, 422, "task_goal_human_only");
    }
    let after = c.env.store.get(t.id).expect("get").expect("task");
    assert_eq!(after.objective, t.objective);
    assert_eq!(after.acceptance, t.acceptance);
    raw("project_cos_refuses_goal_edit", "task_goal_human_only", 422);
    // draft は CoS が起票の途中で直せる。
    let draft = c.task_in_project(Status::Draft);
    let ok = c
        .op(
            "goal-draft",
            "PATCH",
            &format!("/api/v1/tasks/{}", draft.id),
            json!({"objective": "下書きの目的"}),
            false,
        )
        .await;
    assert_eq!(ok.status.as_u16(), 200, "{}", ok.text());
}

#[tokio::test]
async fn project_cos_controller_owns_launch() {
    let c = project_cos().await;
    c.env
        .store
        .project_controller_set_mode(
            &c.project,
            ControllerMode::Enabled,
            None,
            "human:web",
            OffsetDateTime::now_utc(),
        )
        .expect("mode");
    let body = json!({"title": "CoS の独断の root", "objective": "続き", "project_id": c.project,
                      "acceptance": [{"type": "reviewer", "text": "続きが終わる"}]});
    let resp = c
        .op("launch-1", "POST", "/api/v1/tasks", body.clone(), false)
        .await;
    assert_problem(&resp, 422, "project_controller_owns_launch");
    raw(
        "project_cos_controller_owns_launch",
        "project_controller_owns_launch",
        422,
    );
    // off の案件ではこの拒否は出ない（従来の検査に任せる）。
    c.env
        .store
        .project_controller_set_mode(
            &c.project,
            ControllerMode::Off,
            None,
            "human:web",
            OffsetDateTime::now_utc(),
        )
        .expect("mode");
    let resp = c.op("launch-2", "POST", "/api/v1/tasks", body, false).await;
    assert_ne!(
        resp.json()["code"].as_str(),
        Some("project_controller_owns_launch"),
        "{}",
        resp.text()
    );
}

/// 本番 promote・secret・人だけの操作は、案件 CoS が独断で行えない。
#[tokio::test]
async fn project_cos_refuses_production() {
    let c = project_cos().await;
    // 自律運営の中（controller が off でない案件）の案件 CoS。
    c.env
        .store
        .project_controller_set_mode(
            &c.project,
            ControllerMode::Shadow,
            None,
            "human:web",
            OffsetDateTime::now_utc(),
        )
        .expect("mode");
    let promote = c
        .op(
            "prod-1",
            "POST",
            "/api/v1/releases/aaaaaaaaaaaa/promote",
            json!(null),
            false,
        )
        .await;
    assert_problem(&promote, 422, "production_requires_human");
    raw(
        "project_cos_refuses_production",
        "production_requires_human",
        422,
    );
    let secret = c
        .op(
            "prod-2",
            "PUT",
            "/api/v1/secrets/s1",
            json!({"value": "x"}),
            false,
        )
        .await;
    assert_eq!(secret.status.as_u16(), 422, "{}", secret.text());
    let code = secret.json()["code"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    raw("project_cos_refuses_production", &code, 422);
    let stage = c
        .op("prod-3", "POST", "/api/v1/routing/stages", json!({}), false)
        .await;
    assert_eq!(stage.status.as_u16(), 422, "{}", stage.text());
    assert!(c.rejected() >= 3);
}

/// 受信箱の待ちへの代答は、既存の 7 類型の basis が無ければ断られる（新しい代答の種類を足していない）。
#[tokio::test]
async fn project_cos_triage_types_reused() {
    let c = project_cos().await;
    let mut t = new_task(TaskKind::Execute, Status::Blocked);
    t.project_id = Some(c.project.parse().expect("pid"));
    c.env.seed_with(
        &t,
        vec![task_core::Event::QuestionRaised {
            run_id: "r0".into(),
            text: "どちらにしますか".into(),
        }],
    );
    let resp = c
        .op(
            "answer-1",
            "POST",
            &format!("/api/v1/tasks/{}/answer", t.id),
            json!({"answer": "A にする"}),
            false,
        )
        .await;
    assert_problem(&resp, 422, "cos_basis_missing");
    raw("project_cos_triage_types_reused", "cos_basis_missing", 422);
}

/// 案件 CoS は進捗に応じて brief を既存の `project.status_put` で更新でき、`updated_by = cos:<thread>` が残る。
/// brief は派生の要約で、依頼文（正本）は変わらない。
#[tokio::test]
async fn project_cos_brief_updated_by_the_project_cos() {
    let c = project_cos().await;
    let path = format!("/api/v1/projects/{}/status", c.project);
    let resp = c
        .op("brief-1", "PUT", &path, json!({"objective": "計測の土台", "state": "root1 が review 不合格（spec_gap）",
            "open_issues": ["受け入れ条件の解釈"], "next_actions": ["修正の root を controller が起票"],
            "waiting_on": [], "evidence_refs": []}), false)
        .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let pid: task_core::ProjectId = c.project.parse().expect("pid");
    let brief = c
        .env
        .store
        .project_brief_get(pid)
        .expect("get")
        .expect("brief");
    assert_eq!(brief.updated_by, format!("cos:{}", c.thread));
    assert_eq!(brief.state, "root1 が review 不合格（spec_gap）");
    let p = c.env.store.project_get(pid).expect("get").expect("project");
    assert_eq!(p.request, "pilot:a3-guard");
}

/// mode `dry_run`（付記 A3 の 7）は人だけが置ける。置くと評価の頼み（wake `dry_run:<revision>`）が 1 件だけ積まれ、
/// 人の発言（`human:<epoch>`）では積まれない。外部待ちの解除（`POST …/controller/wake`）でもう 1 回頼める。
#[tokio::test]
async fn project_cos_dry_run_mode_is_human_requested() {
    let c = project_cos().await;
    let path = format!("/api/v1/projects/{}/controller", c.project);
    let by_cos = c
        .op(
            "a3-dry-run",
            "PUT",
            &path,
            json!({"mode": "dry_run", "expected_revision": 0, "reason": "試す"}),
            false,
        )
        .await;
    assert_eq!(by_cos.status.as_u16(), 403, "{}", by_cos.text());
    assert!(
        c.env
            .store
            .project_wakes_pending(&c.project)
            .unwrap()
            .is_empty()
    );
    let human = [("authorization", format!("Bearer {WEB_TOKEN}"))];
    let human: Vec<(&str, &str)> = human.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let put = send(
        &c.app,
        put_json_with(
            &path,
            &json!({"mode": "dry_run", "expected_revision": 0, "reason": "判定だけ見る"}),
            &human,
        ),
    )
    .await;
    assert_eq!(put.status.as_u16(), 200, "{}", put.text());
    let keys = |c: &Cos| -> Vec<String> {
        c.env
            .store
            .project_wakes_pending(&c.project)
            .unwrap()
            .into_iter()
            .map(|w| w.wake_key)
            .collect()
    };
    assert_eq!(keys(&c), vec!["dry_run:1".to_owned()]);
    // 人の発言は human_epoch を進めるが、dry_run では評価を頼まない。
    let said = send(
        &c.app,
        post_json_with(
            &format!("/api/v1/chat/threads/{}/messages", c.thread),
            &json!({"client_message_id": "h-2", "text": "様子を見る", "attachment_ids": [],
                    "reply_to_id": null, "mode": "queue", "resume_queue": false}),
            &human,
        ),
    )
    .await;
    assert_eq!(said.status.as_u16(), 202, "{}", said.text());
    assert_eq!(keys(&c), vec!["dry_run:1".to_owned()]);
    let wake = send(
        &c.app,
        post_json_with(
            &format!("{path}/wake"),
            &json!({"external_ref": "dry-run", "note": "もう一度", "idempotency_key": "k1"}),
            &human,
        ),
    )
    .await;
    assert_eq!(wake.status.as_u16(), 200, "{}", wake.text());
    assert_eq!(wake.json()["added"], true);
    assert_eq!(keys(&c).len(), 2);
    raw(
        "project_cos_dry_run_mode_is_human_requested",
        "project_controller_mode_requires_human",
        by_cos.status.as_u16(),
    );
}
