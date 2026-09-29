//! ADR-0079 D7（Phase R3a）: 人への決定の要求の一覧・回答・取り下げ・revise・受信箱・daemon snapshot の結合テスト。
//!
//! 見るもの: `GET /decisions` / `GET /tasks/{id}/decisions` の絞り込みと 404、`POST /decisions/{id}/answer`
//! の 200/409/422/404/401/400、自由記述の回答、`withdraw`、`revise`、`GET /inbox` の `decisions` 節と
//! `counts.decisions`、`GET /daemon` の `decisions_open`。

mod common;

use axum::body::Body;
use axum::http::Request;
use common::*;
use serde_json::json;
use task_core::decision::{
    CostOfReversal, DecisionKind, DecisionOption, DecisionOrigin, DecisionPathEntry,
    DecisionRaisedBy, DecisionRequest, DecisionStatus,
};
use task_core::{Event, Status, TaskId, TaskKind, TaskStore};

fn opt(key: &str, label: &str) -> DecisionOption {
    DecisionOption {
        key: key.into(),
        label: label.into(),
        consequence: None,
    }
}

/// `kind = choice` の決定要求。`task_id` = 出した節点、`root_id` = 木の root（単独の task なら同じ id）。
fn choice_request(id: &str, key: &str, task_id: TaskId, root_id: TaskId) -> DecisionRequest {
    DecisionRequest {
        id: id.into(),
        key: key.into(),
        kind: DecisionKind::Choice,
        question: format!("question {key}?"),
        options: vec![opt("vault", "org vault"), opt("manual", "manual")],
        recommended: "vault".into(),
        cost_of_reversal: CostOfReversal::Low,
        cost_note: None,
        needed_before: vec!["c".into()],
        path: vec![DecisionPathEntry {
            task_id: root_id,
            title: "root".into(),
            stage: Some("s1".into()),
            unit: None,
        }],
        raised_by: DecisionRaisedBy {
            task_id,
            run_id: None,
            origin: DecisionOrigin::Planner,
        },
        status: DecisionStatus::Open,
        answer: None,
        withdrawn_reason: None,
    }
}

/// `kind = limit` の決定要求（daemon が出す。`option` 無しの回答は 422）。
fn limit_request(id: &str, key: &str, task_id: TaskId, root_id: TaskId) -> DecisionRequest {
    DecisionRequest {
        id: id.into(),
        key: key.into(),
        kind: DecisionKind::Limit,
        question: "too many open decisions on this node".into(),
        options: vec![
            opt("raise-once", "raise once"),
            opt("replan", "replan"),
            opt("withdraw", "withdraw"),
        ],
        recommended: "raise-once".into(),
        cost_of_reversal: CostOfReversal::Medium,
        cost_note: None,
        needed_before: vec!["stage:s1".into()],
        path: vec![DecisionPathEntry {
            task_id: root_id,
            title: "root".into(),
            stage: Some("s1".into()),
            unit: None,
        }],
        raised_by: DecisionRaisedBy {
            task_id,
            run_id: None,
            origin: DecisionOrigin::Daemon,
        },
        status: DecisionStatus::Open,
        answer: None,
        withdrawn_reason: None,
    }
}

fn ready_task(env: &TestEnv) -> task_core::Task {
    let task = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&task);
    task
}

fn raise(env: &TestEnv, task_id: TaskId, request: DecisionRequest) {
    env.store
        .append_event(
            task_id,
            &Event::DecisionRequested {
                decision: Box::new(request),
            },
        )
        .expect("append decision");
}

fn a(path: &str) -> Request<Body> {
    get_admin(path)
}

fn p(path: &str, body: &serde_json::Value) -> Request<Body> {
    post_admin(path, body)
}

/// 管理系のトークンは設定済みだが `Authorization` を付けない要求（`admin_env()` の 401 の片方）。
fn no_auth_post(path: &str, body: &serde_json::Value) -> Request<Body> {
    post_json(path, body)
}

fn raw_json_post_admin(path: &str, raw: &str) -> Request<Body> {
    Request::post(path)
        .header("host", HOST)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {TOKEN}"))
        .body(Body::from(raw.to_string()))
        .expect("request")
}

fn admin_env() -> TestEnv {
    TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        ..Default::default()
    })
}

// ---------------------------------------------------------------------------
// GET /decisions・GET /tasks/{id}/decisions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_decisions_filters_by_open_and_root_id() {
    let env = admin_env();
    let app = env.router();
    let root1 = ready_task(&env);
    raise(
        &env,
        root1.id,
        choice_request("dec-1", "h1", root1.id, root1.id),
    );
    let root2 = ready_task(&env);
    raise(
        &env,
        root2.id,
        choice_request("dec-2", "h2", root2.id, root2.id),
    );

    let resp = send(&app, a("/api/v1/decisions?open=true")).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let items = resp.json()["items"].as_array().cloned().expect("items");
    assert_eq!(items.len(), 2, "{items:?}");

    let resp = send(&app, a("/api/v1/decisions?open=false")).await;
    let items = resp.json()["items"].as_array().cloned().expect("items");
    assert!(items.is_empty(), "{items:?}");

    let resp = send(&app, a(&format!("/api/v1/decisions?root_id={}", root1.id))).await;
    let items = resp.json()["items"].as_array().cloned().expect("items");
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0]["decision"]["id"], "dec-1");
}

#[tokio::test]
async fn list_decisions_on_an_empty_db_is_empty_not_404() {
    let env = admin_env();
    let app = env.router();

    let resp = send(&app, a("/api/v1/decisions")).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(resp.json(), json!({"items": []}));
}

/// `GET /tasks/{id}/decisions` はその task の subtree（子孫が出した決定も含む。`path` にその task を含む）。
#[tokio::test]
async fn task_decisions_returns_the_subtree_and_404_for_unknown_task() {
    let env = admin_env();
    let app = env.router();
    let root = ready_task(&env);
    // 子孫（実際の task 行は無いが、path に root を祖先として持つ決定を出したことにする）。
    let child_id = TaskId::new();
    let mut request = choice_request("dec-child", "h1", child_id, root.id);
    request.path.push(DecisionPathEntry {
        task_id: child_id,
        title: "child".into(),
        stage: None,
        unit: None,
    });
    raise(&env, child_id, request);
    // root 自身が出した決定。
    raise(
        &env,
        root.id,
        choice_request("dec-root", "h2", root.id, root.id),
    );

    let resp = send(&app, a(&format!("/api/v1/tasks/{}/decisions", root.id))).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let items = resp.json()["items"].as_array().cloned().expect("items");
    let ids: Vec<String> = items
        .iter()
        .map(|v| v["decision"]["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids.len(), 2, "{ids:?}");
    assert!(ids.contains(&"dec-child".to_string()));
    assert!(ids.contains(&"dec-root".to_string()));

    let resp = send(
        &app,
        a(&format!("/api/v1/tasks/{}/decisions", TaskId::new())),
    )
    .await;
    assert_problem(&resp, 404, "task_not_found");
}

// ---------------------------------------------------------------------------
// POST /decisions/{id}/answer
// ---------------------------------------------------------------------------

#[tokio::test]
async fn answering_an_open_decision_resumes_and_records_the_event() {
    let env = admin_env();
    let app = env.router();
    let root = ready_task(&env);
    raise(
        &env,
        root.id,
        choice_request("dec-1", "h1", root.id, root.id),
    );

    let resp = send(
        &app,
        p(
            "/api/v1/decisions/dec-1/answer",
            &json!({"option": "vault"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let body = resp.json();
    assert_eq!(body["effect"], "resume", "{body}");
    assert_eq!(body["decision"]["decision"]["status"], "answered", "{body}");
    assert_eq!(body["decision"]["decision"]["answer"]["option"], "vault");
    assert_eq!(body["decision"]["decision"]["answer"]["by"], "human");

    let events = env.store.events_for(root.id).expect("events");
    let answered = events.iter().any(|(_, e)| {
        matches!(e, Event::DecisionAnswered { id, option, by, .. }
            if id == "dec-1" && option == "vault" && by == "human")
    });
    assert!(answered, "{events:?}");

    // 2 回目の回答は 409。
    let resp = send(
        &app,
        p(
            "/api/v1/decisions/dec-1/answer",
            &json!({"option": "manual"}),
        ),
    )
    .await;
    assert_problem(&resp, 409, "decision_not_open");
}

#[tokio::test]
async fn answering_with_an_unknown_option_is_422_and_an_unknown_id_is_404() {
    let env = admin_env();
    let app = env.router();
    let root = ready_task(&env);
    raise(
        &env,
        root.id,
        choice_request("dec-1", "h1", root.id, root.id),
    );

    let resp = send(
        &app,
        p(
            "/api/v1/decisions/dec-1/answer",
            &json!({"option": "not-an-option"}),
        ),
    )
    .await;
    assert_problem(&resp, 422, "validation");

    let resp = send(
        &app,
        p(
            "/api/v1/decisions/unknown-id/answer",
            &json!({"option": "vault"}),
        ),
    )
    .await;
    assert_problem(&resp, 404, "decision_not_found");
}

#[tokio::test]
async fn answer_requires_a_token_when_one_is_configured_and_even_without_one() {
    let env = admin_env();
    let app = env.router();
    let root = ready_task(&env);
    raise(
        &env,
        root.id,
        choice_request("dec-1", "h1", root.id, root.id),
    );

    // トークンは設定済みだが `Authorization` を付けない要求。
    let resp = send(
        &app,
        no_auth_post(
            "/api/v1/decisions/dec-1/answer",
            &json!({"option": "vault"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 401, "{}", resp.text());

    // `token_file` 未設定の構成でも管理系は 401（ADR-0017 D1）。
    let env2 = TestEnv::new();
    let app2 = env2.router();
    let root2 = ready_task(&env2);
    raise(
        &env2,
        root2.id,
        choice_request("dec-1", "h1", root2.id, root2.id),
    );
    let resp = send(
        &app2,
        post_json(
            "/api/v1/decisions/dec-1/answer",
            &json!({"option": "vault"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 401, "{}", resp.text());
}

#[tokio::test]
async fn answer_with_invalid_json_body_is_400() {
    let env = admin_env();
    let app = env.router();
    let root = ready_task(&env);
    raise(
        &env,
        root.id,
        choice_request("dec-1", "h1", root.id, root.id),
    );

    let resp = send(
        &app,
        raw_json_post_admin("/api/v1/decisions/dec-1/answer", "{\"option\": "),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 400, "{}", resp.text());
}

// ---------------------------------------------------------------------------
// 自由記述・daemon の決定は option が要る
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_choice_decision_can_be_answered_with_only_a_note() {
    let env = admin_env();
    let app = env.router();
    let root = ready_task(&env);
    raise(
        &env,
        root.id,
        choice_request("dec-1", "h1", root.id, root.id),
    );

    let resp = send(
        &app,
        p(
            "/api/v1/decisions/dec-1/answer",
            &json!({"note": "use the org vault, trust me"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let body = resp.json();
    assert_eq!(body["effect"], "resume", "{body}");
    assert_eq!(body["decision"]["decision"]["answer"]["option"], "other");
    assert_eq!(
        body["decision"]["decision"]["answer"]["note"],
        "use the org vault, trust me"
    );
}

#[tokio::test]
async fn a_limit_decision_without_an_option_is_422() {
    let env = admin_env();
    let app = env.router();
    let root = ready_task(&env);
    raise(
        &env,
        root.id,
        limit_request("dec-1", "limit-1", root.id, root.id),
    );

    let resp = send(&app, p("/api/v1/decisions/dec-1/answer", &json!({}))).await;
    assert_problem(&resp, 422, "validation");

    let resp = send(
        &app,
        p(
            "/api/v1/decisions/dec-1/answer",
            &json!({"note": "just raise it once"}),
        ),
    )
    .await;
    assert_problem(&resp, 422, "validation");
}

// ---------------------------------------------------------------------------
// withdraw・revise
// ---------------------------------------------------------------------------

#[tokio::test]
async fn withdraw_closes_the_decision_and_a_further_answer_is_409() {
    let env = admin_env();
    let app = env.router();
    let root = ready_task(&env);
    raise(
        &env,
        root.id,
        choice_request("dec-1", "h1", root.id, root.id),
    );

    let resp = send(
        &app,
        p(
            "/api/v1/decisions/dec-1/withdraw",
            &json!({"reason": "no longer needed"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let body = resp.json();
    assert_eq!(
        body["decision"]["decision"]["status"], "withdrawn",
        "{body}"
    );
    assert_eq!(body["effect"], "withdraw");

    let resp = send(
        &app,
        p(
            "/api/v1/decisions/dec-1/answer",
            &json!({"option": "vault"}),
        ),
    )
    .await;
    assert_problem(&resp, 409, "decision_not_open");
}

#[tokio::test]
async fn revise_requires_an_answered_choice_decision() {
    let env = admin_env();
    let app = env.router();
    let root = ready_task(&env);
    raise(
        &env,
        root.id,
        choice_request("dec-1", "h1", root.id, root.id),
    );

    // まだ未回答: revise は 409。
    let resp = send(
        &app,
        p(
            "/api/v1/decisions/dec-1/revise",
            &json!({"option": "manual"}),
        ),
    )
    .await;
    assert_problem(&resp, 409, "decision_not_open");

    // 回答してから revise すると新しい選択肢が記録される。
    let resp = send(
        &app,
        p(
            "/api/v1/decisions/dec-1/answer",
            &json!({"option": "vault"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());

    let resp = send(
        &app,
        p(
            "/api/v1/decisions/dec-1/revise",
            &json!({"option": "manual"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(
        resp.json()["decision"]["decision"]["answer"]["option"],
        "manual"
    );

    // daemon の決定（limit）は回答済みでも revise できない: 409。
    raise(
        &env,
        root.id,
        limit_request("dec-2", "limit-1", root.id, root.id),
    );
    let resp = send(
        &app,
        p(
            "/api/v1/decisions/dec-2/answer",
            &json!({"option": "raise-once"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let resp = send(
        &app,
        p(
            "/api/v1/decisions/dec-2/revise",
            &json!({"option": "replan"}),
        ),
    )
    .await;
    assert_problem(&resp, 409, "decision_not_open");
}

// ---------------------------------------------------------------------------
// 受信箱・daemon snapshot
// ---------------------------------------------------------------------------

#[tokio::test]
async fn inbox_lists_the_open_decision_and_the_count_drops_after_answering() {
    let env = admin_env();
    let app = env.router();
    let root = ready_task(&env);
    raise(
        &env,
        root.id,
        choice_request("dec-1", "h1", root.id, root.id),
    );

    let resp = send(&app, a("/api/v1/inbox")).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let body = resp.json();
    let decisions = body["decisions"].as_array().cloned().expect("decisions");
    assert_eq!(decisions.len(), 1, "{decisions:?}");
    assert_eq!(decisions[0]["id"], "dec-1");
    assert_eq!(decisions[0]["question"], "question h1?");
    assert_eq!(decisions[0]["recommended"], "vault");
    assert!(decisions[0]["path"].is_array());
    assert!(decisions[0].get("needed_before").is_some());
    assert_eq!(body["counts"]["decisions"], 1, "{body}");

    let resp = send(
        &app,
        p(
            "/api/v1/decisions/dec-1/answer",
            &json!({"option": "vault"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());

    let resp = send(&app, a("/api/v1/inbox")).await;
    let body = resp.json();
    assert!(
        body["decisions"].as_array().expect("decisions").is_empty(),
        "{body}"
    );
    assert_eq!(body["counts"]["decisions"], 0, "{body}");
}

#[tokio::test]
async fn the_daemon_snapshot_carries_the_open_decision_count() {
    let env = TestEnv::new();
    let app = env.router();
    env.daemon_tx.send_replace(Some(snapshot(3)));

    let resp = send(&app, get("/api/v1/daemon")).await;
    assert_eq!(
        resp.json()["snapshot"]["decisions_open"],
        0,
        "{}",
        resp.text()
    );

    let root = ready_task(&env);
    raise(
        &env,
        root.id,
        choice_request("dec-1", "h1", root.id, root.id),
    );

    let resp = send(&app, get("/api/v1/daemon")).await;
    assert_eq!(
        resp.json()["snapshot"]["decisions_open"],
        1,
        "{}",
        resp.text()
    );

    env.store
        .append_event(
            root.id,
            &Event::DecisionAnswered {
                id: "dec-1".into(),
                option: "vault".into(),
                note: None,
                by: "human".into(),
            },
        )
        .expect("answer directly");

    let resp = send(&app, get("/api/v1/daemon")).await;
    assert_eq!(
        resp.json()["snapshot"]["decisions_open"],
        0,
        "{}",
        resp.text()
    );
}
