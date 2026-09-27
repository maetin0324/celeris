//! GUI 監査対応 Phase 29: `POST /projects/{id}/plan`（ADR-0033 D4 追記「分解は人が
//! `POST /projects/{id}/plan` で起こす」）。
//!
//! 見るもの: 正しい `goal` / `project_id` / `assignee` で plan タスクが作られること、案件・途中目標の
//! 状態遷移、管理系の 401（トークンあり構成と `token_file` 未設定構成の両方）、404（案件が無い）、
//! 422（案件に属さない途中目標）。

mod common;

use common::*;
use serde_json::{Value, json};
use task_core::{MilestoneStatus, Status, TaskKind, TaskStore};

fn g(path: &str) -> axum::http::Request<axum::body::Body> {
    get_with(
        path,
        &[("authorization", format!("Bearer {TOKEN}").as_str())],
    )
}

fn p(path: &str, body: &Value) -> axum::http::Request<axum::body::Body> {
    post_json_with(
        path,
        body,
        &[("authorization", format!("Bearer {TOKEN}").as_str())],
    )
}

fn env_with_token() -> TestEnv {
    TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        ..Default::default()
    })
}

async fn seed_secretary(app: &axum::Router) {
    let resp = send(
        app,
        p("/api/v1/org", &json!({"id": "secretary", "name": "秘書", "kind": "secretary", "brief": "案件を受け取る"})),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 201, "{}", resp.text());
}

async fn create_project(app: &axum::Router, title: &str, request: &str) -> String {
    let resp = send(
        app,
        p(
            "/api/v1/projects",
            &json!({"title": title, "request": request}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 201, "{}", resp.text());
    resp.json()["id"].as_str().expect("id").to_string()
}

#[tokio::test]
async fn plan_creates_a_ready_plan_task_with_the_goal_project_and_secretary_assignee() {
    let env = env_with_token();
    let app = env.router();
    seed_secretary(&app).await;
    let project_id = create_project(&app, "Pluvio 新テーマ", "隣接分野を探して欲しい").await;

    let resp = send(
        &app,
        p(
            &format!("/api/v1/projects/{project_id}/plan"),
            &json!({"note": "急がなくてよい"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 202, "{}", resp.text());
    let task_id = resp.json()["task_id"]
        .as_str()
        .expect("task_id")
        .to_string();

    let task = env
        .store
        .get(task_id.parse().expect("task id"))
        .expect("get")
        .expect("some");
    assert_eq!(task.kind, TaskKind::Plan);
    assert_eq!(task.status, Status::Ready, "その場で走らせる");
    assert_eq!(
        task.project_id.map(|p| p.to_string()),
        Some(project_id.clone())
    );
    assert_eq!(task.assignee.as_deref(), Some("secretary"));
    assert!(task.objective.contains("隣接分野を探して欲しい"));
    assert!(task.objective.contains("急がなくてよい"));

    // proposed だった案件が active になる。
    let project = send(&app, g(&format!("/api/v1/projects/{project_id}")))
        .await
        .json();
    assert_eq!(project["project"]["status"], "active");
}

#[tokio::test]
async fn plan_with_a_milestone_marks_it_in_progress_and_carries_it_on_the_task() {
    let env = env_with_token();
    let app = env.router();
    seed_secretary(&app).await;
    let project_id = create_project(&app, "t", "r").await;
    let milestone: Value = send(
        &app,
        p(
            &format!("/api/v1/projects/{project_id}/milestones"),
            &json!({"title": "隣接領域の調査", "status": "approved"}),
        ),
    )
    .await
    .json();
    let milestone_id = milestone["id"].as_str().expect("id").to_string();

    let resp = send(
        &app,
        p(
            &format!("/api/v1/projects/{project_id}/plan"),
            &json!({"milestone_id": milestone_id}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 202, "{}", resp.text());
    let task_id: task_core::TaskId = resp.json()["task_id"]
        .as_str()
        .expect("task_id")
        .parse()
        .expect("id");
    let task = env.store.get(task_id).expect("get").expect("some");
    assert_eq!(
        task.milestone_id.map(|m| m.to_string()),
        Some(milestone_id.clone())
    );
    assert!(task.objective.contains("隣接領域の調査"));

    let detail = send(&app, g(&format!("/api/v1/projects/{project_id}")))
        .await
        .json();
    let updated = detail["milestones"]
        .as_array()
        .expect("milestones")
        .iter()
        .find(|m| m["id"] == milestone_id)
        .expect("milestone");
    assert_eq!(updated["status"], "in_progress");
}

/// 偽プランナーの出力から作られる子は、親（plan タスク）と同じ案件に属する（`materialize` が
/// `project_id` / `milestone_id` を継ぐ。ADR-0033 D2 の監査 D-3、Phase 23 で確定済み）。
#[tokio::test]
async fn children_materialized_from_the_plan_output_stay_in_the_same_project() {
    let env = env_with_token();
    let app = env.router();
    seed_secretary(&app).await;
    let project_id = create_project(&app, "t", "r").await;
    let plan_id: task_core::TaskId = send(
        &app,
        p(&format!("/api/v1/projects/{project_id}/plan"), &json!({})),
    )
    .await
    .json()["task_id"]
        .as_str()
        .expect("task_id")
        .parse()
        .expect("id");
    let plan_task = env.store.get(plan_id).expect("get").expect("some");

    let plan_output = task_core::PlanOutput {
        tasks: vec![task_core::NewTask {
            harness: None,
            mode: Default::default(),
            skills: Vec::new(),
            repos: Vec::new(),
            title: "a".into(),
            objective: "b".into(),
            acceptance: vec![task_core::Criterion {
                text: "c".into(),
                check: task_core::Check::Human,
            }],
            depends_on: vec![],
            kind: task_core::NewTaskKind::Execute,
            tier: None,
            role: None,
            genre: None,
            assignee: None,
            workspace: None,
            category: None,
            labels: Vec::new(),
            partial_ok: None,
        }],
    };
    let children = task_core::plan::materialize(
        &plan_task,
        &plan_output,
        &[],
        &[],
        &[],
        task_core::WorkspaceContext::default(),
        time::OffsetDateTime::now_utc(),
    );
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].project_id, plan_task.project_id);
    assert_eq!(
        children[0].project_id.map(|p| p.to_string()),
        Some(project_id)
    );
}

/// ADR-0074 D3.3（Phase F4a (b)）: `mode = "milestones"` は `MILESTONES_PLAN_LABEL` の印が付いた
/// Plan タスクを作る（従来の分解タスクとは別物。既存の途中目標の文脈は goal に含めない）。
#[tokio::test]
async fn milestones_mode_labels_the_plan_task_and_ignores_existing_milestones() {
    let env = env_with_token();
    let app = env.router();
    seed_secretary(&app).await;
    let project_id = create_project(&app, "Pluvio 新テーマ", "案件全体の計画を作って欲しい").await;
    let milestone: Value = send(
        &app,
        p(
            &format!("/api/v1/projects/{project_id}/milestones"),
            &json!({"title": "旧い途中目標", "status": "approved"}),
        ),
    )
    .await
    .json();

    let resp = send(
        &app,
        p(
            &format!("/api/v1/projects/{project_id}/plan"),
            &json!({"mode": "milestones", "note": "急ぎで"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 202, "{}", resp.text());
    let task_id: task_core::TaskId = resp.json()["task_id"]
        .as_str()
        .expect("task_id")
        .parse()
        .expect("id");
    let task = env.store.get(task_id).expect("get").expect("some");
    assert_eq!(task.kind, TaskKind::Plan);
    assert_eq!(task.status, Status::Ready);
    assert_eq!(
        task.labels,
        vec![task_core::MILESTONES_PLAN_LABEL.to_string()]
    );
    assert!(task_core::is_milestones_plan_task(&task));
    assert!(task.objective.contains("急ぎで"));
    assert!(
        !task.objective.contains("旧い途中目標"),
        "{}",
        task.objective
    );
    assert_eq!(task.milestone_id, None);

    // 旧い途中目標は変わらない（milestones モードは案件全体を一から設計する）。
    let detail = send(&app, g(&format!("/api/v1/projects/{project_id}")))
        .await
        .json();
    let unchanged = detail["milestones"]
        .as_array()
        .expect("milestones")
        .iter()
        .find(|m| m["id"] == milestone["id"])
        .expect("milestone");
    assert_eq!(unchanged["status"], "approved");
}

/// ADR-0074 D3.4（Phase F4b (e)）: 同じ案件に案件計画 run が動いている間の二重の
/// `POST /plan {mode: milestones}` は 409 `project_plan_in_flight`。`PATCH /projects/{id}` の
/// `auto_advance` は往復する。
#[tokio::test]
async fn a_second_milestones_plan_request_is_409_and_auto_advance_round_trips() {
    let env = env_with_token();
    let app = env.router();
    seed_secretary(&app).await;
    let project_id = create_project(&app, "二重", "案件全体の計画を作って欲しい").await;
    let first = send(
        &app,
        p(
            &format!("/api/v1/projects/{project_id}/plan"),
            &json!({"mode": "milestones"}),
        ),
    )
    .await;
    assert_eq!(first.status.as_u16(), 202, "{}", first.text());
    let second = send(
        &app,
        p(
            &format!("/api/v1/projects/{project_id}/plan"),
            &json!({"mode": "milestones"}),
        ),
    )
    .await;
    assert_eq!(second.status.as_u16(), 409, "{}", second.text());
    assert!(
        second.text().contains("project_plan_in_flight"),
        "{}",
        second.text()
    );

    let detail = send(&app, g(&format!("/api/v1/projects/{project_id}")))
        .await
        .json();
    assert_eq!(detail["project"]["auto_advance"], false);
    let patched = send(
        &app,
        patch_json_with(
            &format!("/api/v1/projects/{project_id}"),
            &json!({"auto_advance": true}),
            &[("authorization", format!("Bearer {TOKEN}").as_str())],
        ),
    )
    .await;
    assert_eq!(patched.status.as_u16(), 200, "{}", patched.text());
    assert_eq!(patched.json()["auto_advance"], true);
}

/// `mode = "milestones"` に `milestone_id` を添えるのは 422（案件全体を計画する run に、単一の
/// 途中目標を紐づける意味が無い）。
#[tokio::test]
async fn milestones_mode_with_a_milestone_id_is_422() {
    let env = env_with_token();
    let app = env.router();
    seed_secretary(&app).await;
    let project_id = create_project(&app, "t", "r").await;
    let milestone: Value = send(
        &app,
        p(
            &format!("/api/v1/projects/{project_id}/milestones"),
            &json!({"title": "m"}),
        ),
    )
    .await
    .json();

    let resp = send(
        &app,
        p(
            &format!("/api/v1/projects/{project_id}/plan"),
            &json!({"mode": "milestones", "milestone_id": milestone["id"]}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 422, "{}", resp.text());
}

#[tokio::test]
async fn plan_on_an_unknown_project_is_404_and_a_foreign_milestone_is_422() {
    let env = env_with_token();
    let app = env.router();
    seed_secretary(&app).await;

    let resp = send(
        &app,
        p(
            "/api/v1/projects/01ARZ3NDEKTSV4RRFFQ69G5FAV/plan",
            &json!({}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 404, "{}", resp.text());

    let project_id = create_project(&app, "t", "r").await;
    let other_id = create_project(&app, "u", "s").await;
    let other_milestone: Value = send(
        &app,
        p(
            &format!("/api/v1/projects/{other_id}/milestones"),
            &json!({"title": "別案件の目標"}),
        ),
    )
    .await
    .json();
    let resp = send(
        &app,
        p(
            &format!("/api/v1/projects/{project_id}/plan"),
            &json!({"milestone_id": other_milestone["id"]}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 422, "{}", resp.text());
}

fn plan_spec_with_one_milestone() -> task_core::ProjectPlanSpec {
    task_core::ProjectPlanSpec {
        schema: task_core::PROJECT_PLAN_SCHEMA.into(),
        rationale: "1 段階で進める".into(),
        milestones: vec![task_core::MilestoneSpec {
            pause_after: None,
            key: "survey".into(),
            title: "調査".into(),
            objective: "周辺調査".into(),
            reach_criteria: "候補が出せた".into(),
            acceptance: vec![
                task_core::Criterion {
                    text: "done".into(),
                    check: task_core::Check::Human,
                },
                task_core::Criterion {
                    text: "a".into(),
                    check: task_core::Check::ArtifactExists {
                        name: "report.md".into(),
                    },
                },
            ],
            depends_on: vec![],
            genre: None,
            skills: vec![],
            repos: vec![],
            features: None,
            execution: None,
        }],
    }
}

/// ADR-0074 D3.5（Phase F4b (h)）: `GET /projects/{id}` の `project_plan` は、提案中は `pending`（節点・
/// rationale・版）、承認後は `current_version` と節点（途中目標・Task の状態・進み・止まっている理由）。
/// 案件計画の無い案件では省略。
#[tokio::test]
async fn project_detail_carries_the_plan_dag_and_the_pending_proposal() {
    let env = env_with_token();
    let app = env.router();
    seed_secretary(&app).await;
    let project_id = create_project(&app, "t", "r").await;
    let detail = send(&app, g(&format!("/api/v1/projects/{project_id}")))
        .await
        .json();
    assert!(detail.get("project_plan").is_none(), "{detail}");

    let pid: task_core::ProjectId = project_id.parse().expect("project id");
    let project = env.store.project_get(pid).expect("get").expect("some");
    let started = task_ops::project_plan::start_milestones(
        &env.store,
        &project,
        None,
        &[],
        &[],
        time::OffsetDateTime::now_utc(),
    )
    .expect("start_milestones");
    let validated = task_core::validate_project_plan(
        &plan_spec_with_one_milestone(),
        task_core::ProjectPlanLimits::default(),
        &std::collections::BTreeSet::new(),
    )
    .expect("valid plan");
    task_ops::project_plan::propose(
        &env.store,
        &started.task,
        &project,
        &validated,
        &[],
        &[],
        time::OffsetDateTime::now_utc(),
    )
    .expect("propose");

    let detail = send(&app, g(&format!("/api/v1/projects/{project_id}")))
        .await
        .json();
    let plan = &detail["project_plan"];
    assert!(plan.get("current_version").is_none(), "{plan}");
    assert_eq!(plan["nodes"].as_array().map(Vec::len), Some(0));
    assert_eq!(plan["pending"]["version"], 1);
    assert_eq!(plan["pending"]["rationale"], "1 段階で進める");
    assert_eq!(plan["pending"]["nodes"][0]["key"], "survey");
    assert_eq!(plan["pending"]["nodes"][0]["task_status"], "draft");
    assert_eq!(plan["pending"]["nodes"][0]["milestone_status"], "proposed");

    let resp = send(
        &app,
        p(
            &format!("/api/v1/projects/{project_id}/project-plan/1/decide"),
            &json!({"decision": "approve"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 202, "{}", resp.text());
    let detail = send(&app, g(&format!("/api/v1/projects/{project_id}")))
        .await
        .json();
    let plan = &detail["project_plan"];
    assert_eq!(plan["current_version"], 1);
    assert!(plan.get("pending").is_none(), "{plan}");
    let node = &plan["nodes"][0];
    assert_eq!(node["title"], "調査");
    assert_eq!(node["task_status"], "ready");
    assert_eq!(node["milestone_status"], "approved");
    assert_eq!(node["work_units_total"], 0);
    assert_eq!(node["children_total"], 0);
}

/// ADR-0074 D3.3（Phase F4a (c)）: `decide approve` は提案の途中目標を `approved`、Task を `ready`
/// にする。`reject` に `note` が無ければ 422。未知の版は 404、決定済みの版へもう一度は 409。
#[tokio::test]
async fn decide_approve_readies_the_dag_reject_needs_a_note_and_replays_are_rejected() {
    let env = env_with_token();
    let app = env.router();
    seed_secretary(&app).await;
    let project_id = create_project(&app, "t", "r").await;
    let pid: task_core::ProjectId = project_id.parse().expect("project id");
    let project = env.store.project_get(pid).expect("get").expect("some");

    let started = task_ops::project_plan::start_milestones(
        &env.store,
        &project,
        None,
        &[],
        &[],
        time::OffsetDateTime::now_utc(),
    )
    .expect("start_milestones");
    let validated = task_core::validate_project_plan(
        &plan_spec_with_one_milestone(),
        task_core::ProjectPlanLimits::default(),
        &std::collections::BTreeSet::new(),
    )
    .expect("valid plan");
    task_ops::project_plan::propose(
        &env.store,
        &started.task,
        &project,
        &validated,
        &[],
        &[],
        time::OffsetDateTime::now_utc(),
    )
    .expect("propose");

    // reject に note が無ければ 422。
    let resp = send(
        &app,
        p(
            &format!("/api/v1/projects/{project_id}/project-plan/1/decide"),
            &json!({"decision": "reject"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 422, "{}", resp.text());

    // 未知の版は 404。
    let resp = send(
        &app,
        p(
            &format!("/api/v1/projects/{project_id}/project-plan/2/decide"),
            &json!({"decision": "approve"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 404, "{}", resp.text());

    // approve は 202 で、途中目標が approved・Task が ready になる。
    let resp = send(
        &app,
        p(
            &format!("/api/v1/projects/{project_id}/project-plan/1/decide"),
            &json!({"decision": "approve"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 202, "{}", resp.text());
    let body = resp.json();
    assert_eq!(body["milestones"].as_array().expect("milestones").len(), 1);
    let task_id: task_core::TaskId = body["tasks"][0]
        .as_str()
        .expect("task id")
        .parse()
        .expect("id");
    let task = env.store.get(task_id).expect("get").expect("some");
    assert_eq!(task.status, Status::Ready);
    let milestones = env.store.milestone_list(pid).expect("list");
    assert_eq!(milestones[0].status, MilestoneStatus::Approved);

    // 決定済みの版へもう一度は 409。
    let resp = send(
        &app,
        p(
            &format!("/api/v1/projects/{project_id}/project-plan/1/decide"),
            &json!({"decision": "approve"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 409, "{}", resp.text());
}

/// `reject` は途中目標を `redesigned`、Task を `cancelled` にする。
#[tokio::test]
async fn decide_reject_redesigns_the_milestone_and_cancels_the_task() {
    let env = env_with_token();
    let app = env.router();
    seed_secretary(&app).await;
    let project_id = create_project(&app, "t", "r").await;
    let pid: task_core::ProjectId = project_id.parse().expect("project id");
    let project = env.store.project_get(pid).expect("get").expect("some");

    let started = task_ops::project_plan::start_milestones(
        &env.store,
        &project,
        None,
        &[],
        &[],
        time::OffsetDateTime::now_utc(),
    )
    .expect("start_milestones");
    let validated = task_core::validate_project_plan(
        &plan_spec_with_one_milestone(),
        task_core::ProjectPlanLimits::default(),
        &std::collections::BTreeSet::new(),
    )
    .expect("valid plan");
    task_ops::project_plan::propose(
        &env.store,
        &started.task,
        &project,
        &validated,
        &[],
        &[],
        time::OffsetDateTime::now_utc(),
    )
    .expect("propose");

    let resp = send(
        &app,
        p(
            &format!("/api/v1/projects/{project_id}/project-plan/1/decide"),
            &json!({"decision": "reject", "note": "スコープが違う"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 202, "{}", resp.text());
    let body = resp.json();
    let task_id: task_core::TaskId = body["tasks"][0]
        .as_str()
        .expect("task id")
        .parse()
        .expect("id");
    let task = env.store.get(task_id).expect("get").expect("some");
    assert_eq!(task.status, Status::Cancelled);
    let milestones = env.store.milestone_list(pid).expect("list");
    assert_eq!(milestones[0].status, MilestoneStatus::Redesigned);
}

#[tokio::test]
async fn decide_endpoint_requires_a_token() {
    let env = env_with_token();
    let app = env.router();
    seed_secretary(&app).await;
    let project_id = create_project(&app, "t", "r").await;

    let resp = send(
        &app,
        post_json(
            &format!("/api/v1/projects/{project_id}/project-plan/1/decide"),
            &json!({"decision": "approve"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 401);
}

#[tokio::test]
async fn admin_endpoints_require_a_token() {
    let env = env_with_token();
    let app = env.router();
    seed_secretary(&app).await;
    let project_id = create_project(&app, "t", "r").await;

    let resp = send(
        &app,
        post_json(&format!("/api/v1/projects/{project_id}/plan"), &json!({})),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 401);
}

#[tokio::test]
async fn admin_endpoints_are_401_even_without_a_configured_token() {
    let env = TestEnv::new();
    let app = env.router();

    let resp = send(
        &app,
        post_json(
            "/api/v1/projects/01ARZ3NDEKTSV4RRFFQ69G5FAV/plan",
            &json!({}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 401);
}
