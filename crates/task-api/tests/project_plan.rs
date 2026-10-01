//! ADR-0079 D13 / D12 / U-R6 / U-R8（Phase R5a）: 案件モデルの変更の API。
//!
//! 見るもの:
//! - 案件計画・途中目標の書き込み・`POST /plans` が 410（`urn:celeris:problem:removed_by_adr_0079`）、`auto_advance` の
//!   書き込みが 422、読み取り（`GET /projects/{id}`）は動く（`project_plan_endpoints_are_gone`）。
//! - 途中目標は既定で隠れ、`?include_frozen=true` で読み取り専用で返る。状態は変わらない。
//! - `is_root_task` が案件の task・一覧・詳細に出る（root / 子 / 対話）。
//! - `POST /tasks/{id}/pause|resume`（subtree の一時停止）と、`POST /tasks` の `stages_hint`。

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

fn pa(path: &str, body: &Value) -> axum::http::Request<axum::body::Body> {
    patch_json_with(
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

fn assert_gone(resp: &Resp) {
    let body = assert_problem(resp, 410, "removed_by_adr_0079");
    assert_eq!(body["type"], "urn:celeris:problem:removed_by_adr_0079");
    assert_eq!(body["adr"], "ADR-0079");
    assert!(
        body["instead"].as_str().is_some_and(|s| !s.is_empty()),
        "{body}"
    );
}

/// 受け入れ (a): 消した API がすべて 410 / 422 を返し、`GET /projects/{id}` は読める。行は消えず状態も変わらない。
#[tokio::test]
async fn project_plan_endpoints_are_gone() {
    let env = env_with_token();
    let app = env.router();
    seed_secretary(&app).await;
    let project_id = create_project(&app, "Pluvio 新テーマ", "隣接分野を探して欲しい").await;
    let pid: task_core::ProjectId = project_id.parse().expect("project id");
    // 凍結する既存の途中目標（本番の 25 行の代わり）。
    let frozen = env
        .store
        .milestone_create(pid, "隣接領域の調査", "", MilestoneStatus::InProgress)
        .expect("milestone");
    let mid = frozen.id.to_string();
    let tasks_before = env.store.list(None).expect("list").len();

    // 案件計画（両 mode）と提案の決定。
    for body in [
        json!({}),
        json!({"mode": "decompose", "note": "急がなくてよい"}),
        json!({"mode": "milestones"}),
    ] {
        let resp = send(
            &app,
            p(&format!("/api/v1/projects/{project_id}/plan"), &body),
        )
        .await;
        assert_gone(&resp);
        assert!(
            resp.json()["detail"]
                .as_str()
                .is_some_and(|d| d.contains("案件は計画を持たない"))
        );
    }
    let resp = send(
        &app,
        p(
            &format!("/api/v1/projects/{project_id}/project-plan/1/decide"),
            &json!({"decision": "approve"}),
        ),
    )
    .await;
    assert_gone(&resp);

    // 途中目標の書き込み（作成・状態・判定・中止・一時停止・再開）。
    let resp = send(
        &app,
        p(
            &format!("/api/v1/projects/{project_id}/milestones"),
            &json!({"title": "新しい途中目標"}),
        ),
    )
    .await;
    assert_gone(&resp);
    let resp = send(
        &app,
        pa(
            &format!("/api/v1/milestones/{mid}"),
            &json!({"status": "reached"}),
        ),
    )
    .await;
    assert_gone(&resp);
    let resp = send(
        &app,
        p(
            &format!("/api/v1/milestones/{mid}/decide"),
            &json!({"decision": "ok"}),
        ),
    )
    .await;
    assert_gone(&resp);
    for action in ["cancel", "pause", "resume"] {
        let resp = send(
            &app,
            p(&format!("/api/v1/milestones/{mid}/{action}"), &json!({})),
        )
        .await;
        assert_gone(&resp);
    }

    // ADR-0028 の `POST /plans`（U-R6）。
    let resp = send(&app, p("/api/v1/plans", &json!({"goal": "分解して"}))).await;
    assert_gone(&resp);

    // `auto_advance` の書き込みは 422（他の欄と一緒でも）。
    let resp = send(
        &app,
        pa(
            &format!("/api/v1/projects/{project_id}"),
            &json!({"auto_advance": true}),
        ),
    )
    .await;
    let body = assert_problem(&resp, 422, "validation");
    assert!(body.to_string().contains("auto_advance"), "{body}");
    let resp = send(
        &app,
        pa(
            &format!("/api/v1/projects/{project_id}"),
            &json!({"auto_advance": false, "title": "変えない"}),
        ),
    )
    .await;
    assert_problem(&resp, 422, "validation");

    // トークン無しは 410 ではなく 401（管理系のまま）。
    let resp = send(
        &app,
        post_json(&format!("/api/v1/projects/{project_id}/plan"), &json!({})),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 401, "{}", resp.text());

    // 読み取りは動き、何も作られず、凍結した行の状態は変わらない。
    let resp = send(&app, g(&format!("/api/v1/projects/{project_id}"))).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(resp.json()["project"]["title"], "Pluvio 新テーマ");
    assert_eq!(resp.json()["project"]["auto_advance"], false);
    let milestones = env.store.milestone_list(pid).expect("list");
    assert_eq!(milestones.len(), 1);
    assert_eq!(milestones[0].status, MilestoneStatus::InProgress);
    assert_eq!(env.store.list(None).expect("list").len(), tasks_before);
}

/// U-R8: 途中目標の行は既定で隠れ（`milestones: []`、`milestones_frozen` は件数）、`?include_frozen=true` で返る。
#[tokio::test]
async fn project_detail_hides_frozen_milestones_by_default() {
    let env = env_with_token();
    let app = env.router();
    seed_secretary(&app).await;
    let project_id = create_project(&app, "t", "r").await;
    let pid: task_core::ProjectId = project_id.parse().expect("project id");
    for (title, status) in [
        ("済", MilestoneStatus::Reached),
        ("途中", MilestoneStatus::InProgress),
        ("承認済み", MilestoneStatus::Approved),
    ] {
        env.store
            .milestone_create(pid, title, "", status)
            .expect("milestone");
    }

    let hidden = send(&app, g(&format!("/api/v1/projects/{project_id}"))).await;
    assert_eq!(hidden.status.as_u16(), 200, "{}", hidden.text());
    let hidden = hidden.json();
    assert_eq!(hidden["milestones"], json!([]));
    assert_eq!(hidden["milestones_frozen"], 3);
    // ADR-0079 R6-4: 終端でない（途中・承認済み）まま凍結した行の数。
    assert_eq!(hidden["milestones_frozen_open"], 2);
    assert!(hidden.get("project_plan").is_none());

    let shown = send(
        &app,
        g(&format!(
            "/api/v1/projects/{project_id}?include_frozen=true"
        )),
    )
    .await
    .json();
    let titles: Vec<&str> = shown["milestones"]
        .as_array()
        .expect("milestones")
        .iter()
        .filter_map(|m| m["title"].as_str())
        .collect();
    assert_eq!(titles, vec!["済", "途中", "承認済み"]);
    assert_eq!(shown["milestones_frozen"], 3);
    assert_eq!(shown["milestones_frozen_open"], 2);
    assert_eq!(shown["milestones"][1]["status"], "in_progress");

    // 明示の false も既定と同じ。知らない値は 400 系。
    let explicit = send(
        &app,
        g(&format!(
            "/api/v1/projects/{project_id}?include_frozen=false"
        )),
    )
    .await
    .json();
    assert_eq!(explicit["milestones"], json!([]));
    let bad = send(
        &app,
        g(&format!(
            "/api/v1/projects/{project_id}?include_frozen=maybe"
        )),
    )
    .await;
    assert!(bad.status.is_client_error(), "{}", bad.text());
}

/// D13: `is_root_task` は案件直下の task だけ（子・対話は false）。案件の task・一覧・詳細に同じ値が出る。
/// `POST /tasks` の `stages_hint` は `Task.routing.stages_hint` に入る。案件直下に作っても途中目標の行はできない。
#[tokio::test]
async fn is_root_task_and_stages_hint_are_exposed() {
    let env = env_with_token();
    let app = env.router();
    seed_secretary(&app).await;
    let project_id = create_project(&app, "t", "r").await;
    let pid: task_core::ProjectId = project_id.parse().expect("project id");

    let resp = send(
        &app,
        p(
            "/api/v1/tasks",
            &json!({
                "title": "browser capability（Phase 1〜4）",
                "objective": "Phase 1〜4 のすべて",
                "acceptance": [{"type": "artifact_exists", "name": "result.md"}],
                "project_id": project_id,
                "stages_hint": [{"title": "Phase 1", "scope": "MVP"}, {"title": "Phase 2"}],
            }),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 201, "{}", resp.text());
    let root_id = resp.json()["id"].as_str().expect("id").to_string();
    let root = env
        .store
        .get(root_id.parse().expect("task id"))
        .expect("get")
        .expect("some");
    let hints = root.routing.as_ref().expect("routing").stages_hint.clone();
    assert_eq!(hints.len(), 2);
    assert_eq!(hints[0].title, "Phase 1");
    assert_eq!(hints[0].scope, "MVP");
    assert_eq!(root.milestone_id, None);
    assert!(env.store.milestone_list(pid).expect("list").is_empty());

    // 形の検証（空の title）は 422。
    let resp = send(
        &app,
        p(
            "/api/v1/tasks",
            &json!({
                "title": "t",
                "objective": "o",
                "acceptance": [{"type": "artifact_exists", "name": "result.md"}],
                "stages_hint": [{"title": "  "}],
            }),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 422, "{}", resp.text());

    // 子（委譲・計画の子の形: parent_id あり）と対話。
    let mut child = new_task(TaskKind::Execute, Status::Ready);
    child.project_id = Some(pid);
    child.parent_id = Some(root.id);
    env.seed(&child);
    let mut conv = new_task(TaskKind::Execute, Status::Ready);
    conv.project_id = Some(pid);
    conv.conversation = Some(task_core::MessageId::new());
    env.seed(&conv);

    let detail = send(&app, g(&format!("/api/v1/projects/{project_id}")))
        .await
        .json();
    let flag = |id: &str| {
        detail["tasks"]
            .as_array()
            .expect("tasks")
            .iter()
            .find(|t| t["id"] == id)
            .map(|t| t["is_root_task"].clone())
    };
    assert_eq!(flag(&root_id), Some(json!(true)));
    assert_eq!(flag(&child.id.to_string()), Some(json!(false)));
    assert_eq!(flag(&conv.id.to_string()), Some(json!(false)));
    assert_eq!(detail["root_totals"]["root_tasks"], 1);

    let list = send(&app, g(&format!("/api/v1/tasks?project={project_id}")))
        .await
        .json();
    for item in list["items"].as_array().expect("items") {
        let expected = item["id"] == root_id.as_str();
        assert_eq!(item["is_root_task"], json!(expected), "{item}");
        assert_eq!(item["paused"], json!(false));
    }
    let task_detail = send(&app, g(&format!("/api/v1/tasks/{root_id}")))
        .await
        .json();
    assert_eq!(task_detail["is_root_task"], true);
    let child_detail = send(&app, g(&format!("/api/v1/tasks/{}", child.id)))
        .await
        .json();
    assert_eq!(child_detail["is_root_task"], false);
}

/// D13（R5a）: `POST /tasks/{id}/pause|resume` は subtree を止め・戻す（管理系、409 の規則、詳細の `paused_by`）。
#[tokio::test]
async fn task_pause_and_resume_cover_the_subtree() {
    let env = env_with_token();
    let app = env.router();
    let root = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&root);
    let mut child = new_task(TaskKind::Execute, Status::Ready);
    child.parent_id = Some(root.id);
    env.seed(&child);

    // トークン無しは 401。
    let resp = send(
        &app,
        post_json(&format!("/api/v1/tasks/{}/pause", root.id), &json!({})),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 401, "{}", resp.text());

    let resp = send(
        &app,
        p(&format!("/api/v1/tasks/{}/pause", root.id), &json!({})),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let body = resp.json();
    assert!(body["paused_at"].as_str().is_some());
    assert_eq!(body["subtree"][0]["id"], child.id.to_string());
    let ready: Vec<_> = env
        .store
        .ready_tasks(10)
        .expect("ready")
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert!(!ready.contains(&root.id) && !ready.contains(&child.id));
    let child_detail = send(&app, g(&format!("/api/v1/tasks/{}", child.id)))
        .await
        .json();
    assert_eq!(child_detail["paused_by"], root.id.to_string());
    assert_eq!(
        child_detail["task"]["status"], "ready",
        "状態機械は触らない"
    );

    let resp = send(
        &app,
        p(&format!("/api/v1/tasks/{}/pause", root.id), &json!({})),
    )
    .await;
    assert_problem(&resp, 409, "invalid_transition");

    let resp = send(
        &app,
        p(&format!("/api/v1/tasks/{}/resume", root.id), &json!({})),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert!(resp.json().get("paused_at").is_none());
    let ready: Vec<_> = env
        .store
        .ready_tasks(10)
        .expect("ready")
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert!(ready.contains(&root.id) && ready.contains(&child.id));
    let child_detail = send(&app, g(&format!("/api/v1/tasks/{}", child.id)))
        .await
        .json();
    assert!(child_detail.get("paused_by").is_none());

    // 知らない task は 404。
    let resp = send(
        &app,
        p(
            &format!("/api/v1/tasks/{}/pause", task_core::TaskId::new()),
            &json!({}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 404, "{}", resp.text());
}
