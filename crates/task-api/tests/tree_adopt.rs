//! ADR-0079 R5b-prep: 人の /3 の計画（`PUT/POST /tasks/{id}/execution-plan`、origin human）が planner の計画と
//! 同じ経路（unit の gate・決定の要求・木の上限・unit の `adopt`）を通ること、承認（PlanGate）を挟まないこと、
//! 木が無効なら 422 `TreeDisabled` のままであること、`POST /tasks/{id}/tree/adopt`（D15）の正常系と各拒否
//! （409 / 422）。store と HTTP だけ（dispatcher は走らせない。統合が採用した task を飛ばすことは
//! `task-dispatch` の `adopted_done_task_already_in_base_is_skipped_by_integration`）。

mod common;

use common::*;
use serde_json::{Value, json};
use task_core::{
    Event, Project, ProjectId, Status, Task, TaskId, TaskKind, TaskStore, Trigger,
    WorkUnitBlockedReason, WorkUnitStatus,
};
use time::OffsetDateTime;

fn tree_env(limits: task_core::TreeLimits) -> TestEnv {
    TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        tree_limits: limits,
        ..EnvOptions::default()
    })
}

fn enabled() -> task_core::TreeLimits {
    task_core::TreeLimits {
        enabled: true,
        ..task_core::TreeLimits::default()
    }
}

fn project(env: &TestEnv, title: &str) -> Project {
    let now = OffsetDateTime::now_utc();
    let project = Project {
        auto_advance: false,
        slug: None,
        id: ProjectId::new(),
        title: title.to_string(),
        request: "direction".into(),
        status: task_core::ProjectStatus::Active,
        secretary_summary: None,
        workspace: None,
        archived_at: None,
        paused_from: None,
        created_at: now,
        updated_at: now,
    };
    env.store.project_create(&project).expect("project");
    project
}

fn task_in(env: &TestEnv, project: &Project, status: Status, title: &str) -> Task {
    let mut t = new_task(TaskKind::Execute, status);
    t.title = title.to_string();
    t.project_id = Some(project.id);
    env.seed(&t);
    t
}

fn reviewer() -> Value {
    json!([{"text": "reviewer が受け入れ条件を確認する", "check": {"type": "reviewer"}}])
}

fn task_unit(key: &str, stage: &str, adopt: Option<TaskId>) -> Value {
    let mut u = json!({
        "key": key, "stage": stage, "kind": "task",
        "title": format!("Phase {key}"),
        "objective": format!("deliver {key} as its own reviewed task"),
        "acceptance": reviewer(),
    });
    if let Some(id) = adopt {
        u["adopt"] = json!(id.to_string());
    }
    u
}

fn leaf(key: &str, stage: &str) -> Value {
    json!({
        "key": key, "stage": stage, "kind": "design",
        "title": format!("Leaf {key}"),
        "objective": format!("write the {key} note thoroughly"),
        "done_when": [format!("artifacts/{key}.md がある")],
        "checks": [{"cmd": format!("test -s artifacts/{key}.md"), "expect_exit": 0}],
        "budget": {"max_turns": 20}
    })
}

fn decision(key: &str, needed_before: &[&str]) -> Value {
    json!({
        "key": key, "question": format!("{key} をどうするか"),
        "options": [{"key": "a", "label": "A 案"}, {"key": "b", "label": "B 案"}],
        "recommended": "a", "cost_of_reversal": "medium", "cost_note": "移行が要る",
        "needed_before": needed_before
    })
}

fn plan(stages: &[&str], units: Vec<Value>, decisions: Vec<Value>) -> Value {
    json!({
        "schema": "celeris.execution-plan/3",
        "rationale": "製品の Phase を段階にし、既に終わった Phase は採用する",
        "stages": stages.iter().map(|s| json!({"key": s, "kind": "implement", "title": format!("Stage {s}")})).collect::<Vec<_>>(),
        "units": units,
        "decisions": decisions,
    })
}

fn put_admin(path: &str, body: &Value) -> axum::http::Request<axum::body::Body> {
    put_json_with(path, body, &admin_headers())
}

fn plan_path(id: TaskId) -> String {
    format!("/api/v1/tasks/{id}/execution-plan")
}

fn adopt_path(id: TaskId) -> String {
    format!("/api/v1/tasks/{id}/tree/adopt")
}

fn events(env: &TestEnv, id: TaskId) -> Vec<Event> {
    env.store
        .events_for(id)
        .expect("events")
        .into_iter()
        .map(|(_, e)| e)
        .collect()
}

fn assert_replay_is_clean(env: &TestEnv) {
    let report = task_ops::replay::replay(&env.store).expect("replay");
    assert!(report.mismatches.is_empty(), "{:?}", report.mismatches);
    let (wu, _runs, plans, _) =
        task_ops::replay::check_and_apply_execution(&env.store, false).expect("replay execution");
    assert!(wu.is_empty(), "{wu:?}");
    assert!(plans.is_empty(), "{plans:?}");
    let (decisions, _) =
        task_ops::replay::check_and_apply_decisions(&env.store, false).expect("replay decisions");
    assert!(decisions.is_empty(), "{decisions:?}");
}

/// R5b-prep (1): 木が有効なら人の `PUT` の /3 は planner の計画と同じ経路を通る: unit の gate（`adopt` の unit は
/// 構造上の理由で `kept_task`、`UnitGateOverridden`）、計画の決定（origin human、path 付き）、答えの無い決定を待つ leaf
/// の `blocked(decision)`、木の上限（`max_tree_leaves` を超える leaf は `kind: limit` の決定で止まる）、`adopt` は同じ
/// トランザクションで結ばれ unit は `done`（`ChildAdopted`）。承認（PlanGate）は挟まない（task は draft のまま、
/// `PlanApprovalRequested` は無い）。`adopt` の unit は計画あたりの子 task の上限に数えない。replay は差分 0。
#[tokio::test]
async fn human_put_v3_goes_through_the_tree_path_without_plan_gate() {
    let env = tree_env(task_core::TreeLimits {
        enabled: true,
        max_tree_leaves: 1,
        max_child_tasks_per_plan: 1,
        ..task_core::TreeLimits::default()
    });
    let app = env.router();
    let p = project(&env, "agent-platform");
    let done = task_in(&env, &p, Status::Done, "Phase 1 (done)");
    let root = task_in(&env, &p, Status::Draft, "browser capability");
    let body = plan(
        &["phase-1", "phase-2"],
        vec![
            task_unit("p1", "phase-1", Some(done.id)),
            {
                let mut u = task_unit("p2", "phase-2", None);
                u["depends_on"] = json!(["p1"]);
                u
            },
            leaf("doc", "phase-2"),
            leaf("extra", "phase-2"),
        ],
        vec![decision("h4", &["p2"]), decision("h7", &["doc"])],
    );
    let resp = send(&app, put_admin(&plan_path(root.id), &body)).await;
    assert_eq!(resp.status, 201, "{}", resp.text());
    let view = resp.json();
    assert_eq!(view["origin"], "human");
    assert_eq!(view["decisions_raised"], 2, "{view}");
    let adoptions = view["adoptions"].as_array().expect("adoptions");
    assert_eq!(adoptions.len(), 1, "{view}");
    assert_eq!(adoptions[0]["unit_key"], "p1");
    assert_eq!(adoptions[0]["adopted"], true);
    assert_eq!(adoptions[0]["unit_status"], "done");

    // unit の行: p1 は採用で done、doc は h7 を待って blocked(decision)、extra は木の leaf の上限で blocked(decision)、
    // p2 は段階の障壁で pending（kind task の unit は h4 を待って子を作らない）。
    let units = env.store.work_units_for(root.id).expect("units");
    let row = |k: &str| {
        units
            .iter()
            .find(|u| u.key == k)
            .unwrap_or_else(|| panic!("{k}: {units:?}"))
    };
    assert_eq!(row("p1").status, WorkUnitStatus::Done);
    assert_eq!(
        row("p1").child_task_id.as_deref(),
        Some(done.id.to_string().as_str())
    );
    for k in ["doc", "extra"] {
        assert_eq!(row(k).status, WorkUnitStatus::Blocked, "{k}");
        assert_eq!(
            row(k).blocked_reason,
            Some(WorkUnitBlockedReason::Decision),
            "{k}"
        );
    }
    assert_eq!(row("p2").status, WorkUnitStatus::Pending);
    assert_eq!(row("p2").child_task_id, None);

    // 決定: h4 / h7（origin human、run なし、path は root）と木の leaf の上限（daemon）。
    let decisions = env.store.decisions_list(Some(root.id)).expect("decisions");
    let mut keys: Vec<(String, task_core::DecisionOrigin)> = decisions
        .iter()
        .map(|d| (d.key.clone(), d.request.raised_by.origin))
        .collect();
    keys.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        keys,
        vec![
            ("h4".to_string(), task_core::DecisionOrigin::Human),
            ("h7".to_string(), task_core::DecisionOrigin::Human),
            (
                "limit:max_tree_leaves".to_string(),
                task_core::DecisionOrigin::Daemon
            ),
        ]
    );
    let h4 = decisions.iter().find(|d| d.key == "h4").expect("h4");
    assert_eq!(h4.request.raised_by.run_id, None);
    assert_eq!(h4.request.path[0].task_id, root.id);
    // 受信箱の決定の一覧にも出る（`GET /tasks/{id}/decisions`）。
    let resp = send(
        &app,
        get_admin(&format!("/api/v1/tasks/{}/decisions", root.id)),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    assert!(resp.text().contains("h4 をどうするか"), "{}", resp.text());

    let ev = events(&env, root.id);
    // R5b-fix3: 人の計画の kind task の unit は `human/explicit` の compound（宣言と一致するので記録なし）。
    assert!(
        !ev.iter().any(|e| matches!(
            e,
            Event::UnitGateOverridden { unit_key, .. } if unit_key == "p1" || unit_key == "p2"
        )),
        "{ev:?}"
    );
    assert!(ev.iter().any(|e| matches!(
        e,
        Event::ChildAdopted { unit_key, child_task_id, stage, .. }
            if unit_key == "p1" && *child_task_id == done.id && stage == "phase-1"
    )));
    assert!(
        !ev.iter()
            .any(|e| matches!(e, Event::PlanApprovalRequested { .. })),
        "a human plan does not wait for the plan approval"
    );
    assert_eq!(env.status_of(root.id), Status::Draft);

    // 採用した task: 状態はそのまま、木の子（深さ 2、親の unit p1）、`parent_id` は root。
    let adopted = env.store.get(done.id).expect("get").expect("adopted");
    assert_eq!(adopted.status, Status::Done);
    assert_eq!(adopted.parent_id, Some(root.id));
    let tree = adopted.tree.expect("tree");
    assert_eq!(tree.root_id, root.id);
    assert_eq!(tree.depth, 2);
    assert!(events(&env, done.id).iter().any(|e| matches!(
        e,
        Event::Edited { fields, by } if fields == &vec!["tree".to_string(), "parent_id".to_string()] && by == "human"
    )));
    // root は案件の root task のまま、採用した task は root task ではなくなる。
    assert!(task_core::is_root_task(
        &env.store.get(root.id).expect("get").expect("root")
    ));
    assert!(!task_core::is_root_task(
        &env.store.get(done.id).expect("get").expect("adopted")
    ));
    assert_replay_is_clean(&env);
}

/// R5b-prep (1): 木が無効（既定）なら人の `PUT` の /3 は今どおり 422 `TreeDisabled`（`POST` の既存の試験と同じ）。
#[tokio::test]
async fn human_put_v3_is_422_while_the_tree_is_disabled() {
    let env = tree_env(task_core::TreeLimits::default());
    let app = env.router();
    let p = project(&env, "agent-platform");
    let done = task_in(&env, &p, Status::Done, "Phase 1");
    let root = task_in(&env, &p, Status::Draft, "root");
    let body = plan(
        &["phase-1"],
        vec![task_unit("p1", "phase-1", Some(done.id))],
        vec![],
    );
    let resp = send(&app, put_admin(&plan_path(root.id), &body)).await;
    assert_eq!(resp.status, 422, "{}", resp.text());
    assert!(
        resp.text().contains("[execution.tree] enabled = true"),
        "{}",
        resp.text()
    );
    assert!(env.store.work_units_for(root.id).expect("units").is_empty());
    assert!(
        env.store
            .get(done.id)
            .expect("get")
            .expect("t")
            .tree
            .is_none()
    );
}

/// R5b-prep (2): 人の計画の `adopt` の拒否（計画全体を拒否し、何も書かない）: 別の案件 422、自分自身（祖先）422、
/// 他の木に属する 409、中止済み 409、存在しない 422、2 つの unit で同じ task 422。
#[tokio::test]
async fn human_plan_adopt_refusals_write_nothing() {
    let env = tree_env(enabled());
    let app = env.router();
    let p = project(&env, "agent-platform");
    let other = project(&env, "benchfs");
    let root = task_in(&env, &p, Status::Draft, "root");
    let foreign = task_in(&env, &other, Status::Done, "other project");
    let cancelled = task_in(&env, &p, Status::Cancelled, "cancelled");
    let done = task_in(&env, &p, Status::Done, "done");
    let other_root = task_in(&env, &p, Status::Ready, "another tree's root");
    let mut in_tree = new_task(TaskKind::Execute, Status::Done);
    in_tree.project_id = Some(p.id);
    in_tree.tree = Some(task_core::TreeInfo::child_of(
        &other_root,
        task_core::ParentUnit {
            task_id: other_root.id,
            plan_id: "plan-x".into(),
            unit_key: "c".into(),
            stage: "s".into(),
            attempt: 1,
        },
        None,
    ));
    env.seed(&in_tree);
    let cases: Vec<(Vec<Value>, u16, &str)> = vec![
        (
            vec![task_unit("p1", "s1", Some(foreign.id))],
            422,
            "adopt_other_project",
        ),
        (
            vec![task_unit("p1", "s1", Some(root.id))],
            422,
            "adopt_ancestor",
        ),
        (
            vec![task_unit("p1", "s1", Some(in_tree.id))],
            409,
            "adopt_target_in_tree",
        ),
        (
            vec![task_unit("p1", "s1", Some(other_root.id))],
            409,
            "adopt_target_in_tree",
        ),
        (
            vec![task_unit("p1", "s1", Some(cancelled.id))],
            409,
            "adopt_target_cancelled",
        ),
        (
            vec![task_unit("p1", "s1", Some(TaskId::new()))],
            422,
            "adopt_target_not_found",
        ),
        (
            vec![
                task_unit("p1", "s1", Some(done.id)),
                task_unit("p2", "s1", Some(done.id)),
            ],
            422,
            "adopt_duplicate",
        ),
    ];
    for (units, status, code) in cases {
        let resp = send(
            &app,
            put_admin(&plan_path(root.id), &plan(&["s1"], units, vec![])),
        )
        .await;
        assert_problem(&resp, status, code);
        assert!(
            env.store
                .execution_plan_active(root.id)
                .expect("plan")
                .is_none(),
            "{code}: nothing is written"
        );
    }
    assert!(
        env.store
            .get(done.id)
            .expect("get")
            .expect("t")
            .tree
            .is_none()
    );
    // leaf に `adopt` は書けない（検証の 422）。
    let mut bad = leaf("l", "s1");
    bad["adopt"] = json!(done.id.to_string());
    let resp = send(
        &app,
        put_admin(&plan_path(root.id), &plan(&["s1"], vec![bad], vec![])),
    )
    .await;
    assert_eq!(resp.status, 422, "{}", resp.text());
}

/// R5b-prep (2): `POST /tasks/{id}/tree/adopt`。人の計画の `adopt` の対象がまだ終端でなければ unit は結ばれずに
/// 待ち（`adopted: false`、子は作らない）、後からの採用は対象が終端になるまで 409。unit が leaf・`adopt` の id が違う・
/// 段階が違うは 422、トークン無しは 401。対象が done になれば 200 で結ばれ（unit は done、`ChildAdopted`）、もう一度は
/// 409。replay は差分 0。
#[tokio::test]
async fn adopt_endpoint_happy_path_and_refusals() {
    let env = tree_env(enabled());
    let app = env.router();
    let p = project(&env, "agent-platform");
    let target = task_in(&env, &p, Status::Reviewing, "Phase 2 (under review)");
    let other_done = task_in(&env, &p, Status::Done, "another done task");
    let root = task_in(&env, &p, Status::Draft, "root");
    let body = plan(
        &["s1", "s2"],
        vec![task_unit("p1", "s1", Some(target.id)), leaf("x", "s2")],
        vec![],
    );
    let resp = send(&app, put_admin(&plan_path(root.id), &body)).await;
    assert_eq!(resp.status, 201, "{}", resp.text());
    let view = resp.json();
    assert_eq!(view["adoptions"][0]["adopted"], false, "{view}");
    assert_eq!(view["adoptions"][0]["task_status"], "reviewing");
    let p1 = |env: &TestEnv| {
        env.store
            .work_units_for(root.id)
            .expect("units")
            .into_iter()
            .find(|u| u.key == "p1")
            .expect("p1")
    };
    assert_eq!(p1(&env).status, WorkUnitStatus::Ready);
    assert_eq!(p1(&env).child_task_id, None);

    let req = |task: TaskId, stage: &str, unit: &str| json!({"task_id": task.to_string(), "stage": stage, "unit_key": unit});
    // トークン無し。
    let resp = send(
        &app,
        post_json(&adopt_path(root.id), &req(target.id, "s1", "p1")),
    )
    .await;
    assert_eq!(resp.status, 401, "{}", resp.text());
    // 対象が終端でない。
    let resp = send(
        &app,
        post_admin(&adopt_path(root.id), &req(target.id, "s1", "p1")),
    )
    .await;
    assert_problem(&resp, 409, "adopt_target_not_terminal");
    // unit が leaf。
    let resp = send(
        &app,
        post_admin(&adopt_path(root.id), &req(target.id, "s2", "x")),
    )
    .await;
    assert_problem(&resp, 422, "adopt_unit_not_task");
    // `adopt` の id が違う。
    let resp = send(
        &app,
        post_admin(&adopt_path(root.id), &req(other_done.id, "s1", "p1")),
    )
    .await;
    assert_problem(&resp, 422, "adopt_id_mismatch");
    // 段階が違う。
    let resp = send(
        &app,
        post_admin(&adopt_path(root.id), &req(target.id, "s2", "p1")),
    )
    .await;
    assert_problem(&resp, 422, "adopt_stage_mismatch");
    // 無い unit、無い task。
    let resp = send(
        &app,
        post_admin(&adopt_path(root.id), &req(target.id, "s1", "nope")),
    )
    .await;
    assert_problem(&resp, 422, "adopt_unit_not_found");
    let resp = send(
        &app,
        post_admin(&adopt_path(TaskId::new()), &req(target.id, "s1", "p1")),
    )
    .await;
    assert_problem(&resp, 404, "task_not_found");
    // 未知の欄は 400 / 422（deny_unknown_fields）。
    let resp = send(
        &app,
        post_admin(
            &adopt_path(root.id),
            &json!({"task_id": target.id.to_string(), "stage": "s1", "unit_key": "p1", "force": true}),
        ),
    )
    .await;
    assert!(resp.status == 400 || resp.status == 422, "{}", resp.text());
    assert_eq!(p1(&env).child_task_id, None, "no refusal wrote anything");

    // 対象が done になる → 採用できる。
    env.store
        .apply_transition(target.id, Trigger::ReviewPass, None)
        .expect("review pass");
    let resp = send(
        &app,
        post_admin(&adopt_path(root.id), &req(target.id, "s1", "p1")),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let out = resp.json();
    assert_eq!(out["adopted"], true);
    assert_eq!(out["unit_status"], "done");
    assert_eq!(out["task_status"], "done");
    assert_eq!(p1(&env).status, WorkUnitStatus::Done);
    assert_eq!(
        p1(&env).child_task_id.as_deref(),
        Some(target.id.to_string().as_str())
    );
    assert!(events(&env, root.id).iter().any(|e| matches!(
        e,
        Event::ChildAdopted { unit_key, child_task_id, .. } if unit_key == "p1" && *child_task_id == target.id
    )));
    let adopted = env.store.get(target.id).expect("get").expect("t");
    assert_eq!(adopted.status, Status::Done);
    assert_eq!(adopted.parent_id, Some(root.id));
    // もう一度: 対象は木に属している。
    let resp = send(
        &app,
        post_admin(&adopt_path(root.id), &req(target.id, "s1", "p1")),
    )
    .await;
    assert_problem(&resp, 409, "adopt_target_in_tree");
    assert_replay_is_clean(&env);
}

/// R5b-prep (2): `POST /tasks/{id}/tree/adopt` は木が無効なら 422 `tree_disabled`、計画が /3 でなければ 422。
#[tokio::test]
async fn adopt_endpoint_requires_the_tree_and_a_v3_plan() {
    let env = tree_env(task_core::TreeLimits::default());
    let app = env.router();
    let p = project(&env, "agent-platform");
    let done = task_in(&env, &p, Status::Done, "done");
    let root = task_in(&env, &p, Status::Draft, "root");
    let req = json!({"task_id": done.id.to_string(), "stage": "s1", "unit_key": "p1"});
    let resp = send(&app, post_admin(&adopt_path(root.id), &req)).await;
    assert_problem(&resp, 422, "tree_disabled");

    let env = tree_env(enabled());
    let app = env.router();
    let p = project(&env, "agent-platform");
    let done = task_in(&env, &p, Status::Done, "done");
    let root = task_in(&env, &p, Status::Draft, "root");
    let req = json!({"task_id": done.id.to_string(), "stage": "s1", "unit_key": "p1"});
    let resp = send(&app, post_admin(&adopt_path(root.id), &req)).await;
    assert_problem(&resp, 422, "adopt_no_tree_plan");
}

/// 旧 R5b runbook（ADR-0128 で削除）の `cat > <name> <<'EOF'` … `EOF` の中身を
/// `tests/fixtures/<name>` として切り出したもの。
fn runbook_json(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn fixed_task(env: &TestEnv, id: &str, project: ProjectId, status: Status, parent: Option<TaskId>) {
    let mut t = new_task(TaskKind::Execute, status);
    t.id = id.parse().expect("task id");
    t.project_id = Some(project);
    t.parent_id = parent;
    env.seed(&t);
}

fn fixed_project(env: &TestEnv, id: &str, title: &str) -> ProjectId {
    let mut p = project(env, title);
    // `project()` は新しい id で作るので、手順書の id の案件を別に作る。
    p.id = id.parse().expect("project id");
    env.store.project_create(&p).expect("project");
    p.id
}

/// R5b の手順書（`tests/fixtures/*.json`、旧 ADR-0079 runbook から切り出し）の JSON がそのまま通る: browser と BenchFS の root task の
/// `POST /tasks`、/3 の計画の `PUT`（本番と同じ id・状態の task を採用: Phase 1 = done、Phase 2 = failed、BenchFS の
/// Phase0 / Phase1 の 6 件 = done で `parent_id` は `kind = plan` の task）。すべて採用され、決定は browser 3 件・
/// BenchFS 1 件、承認（PlanGate）を挟まず root は draft のまま。
#[tokio::test]
async fn the_r5b_runbook_plans_are_accepted_as_written() {
    let env = tree_env(enabled());
    let app = env.router();
    let ap = fixed_project(
        &env,
        "01M2WTS3DKNZBSZ2JMVB4CZMBW",
        "agent-platform の自己改善",
    );
    let bf = fixed_project(
        &env,
        "01M35WRV77A2JPYGERQGXF6V7K",
        "BenchFS 国際会議フルペーパー化",
    );
    fixed_task(&env, "01M3MFS5T52FXA63W4V10XGC4S", ap, Status::Done, None);
    fixed_task(&env, "01M3MZKB3DFYJNBH015MJGQ0BT", ap, Status::Failed, None);
    let plan_task: TaskId = "01M35X04345ZNDM09VE6FT168Z".parse().expect("id");
    let mut kind_plan = new_task(TaskKind::Plan, Status::Done);
    kind_plan.id = plan_task;
    kind_plan.project_id = Some(bf);
    env.seed(&kind_plan);
    for id in [
        "01M35X86XT4T3CSNMB8RNQ9WG9",
        "01M35X86XTHHN9C6XDAYD2FZ7T",
        "01M35X86XTEPXVZMBY7HSSEP7X",
        "01M37JKZ4Q97KYPY70WQ30DRMD",
        "01M3768090QQQMQEFK2JK6SCNB",
        "01M35X86XTK84F97QW0CN5PGMR",
    ] {
        fixed_task(&env, id, bf, Status::Done, Some(plan_task));
    }

    for (root_file, plan_file, adopted, decisions) in [
        ("browser-root.json", "browser-plan.json", 2usize, 3u64),
        ("benchfs-root.json", "benchfs-plan.json", 6, 1),
    ] {
        let resp = send(&app, post_admin("/api/v1/tasks", &runbook_json(root_file))).await;
        assert_eq!(resp.status, 201, "{root_file}: {}", resp.text());
        let root: TaskId = resp.json()["id"]
            .as_str()
            .expect("id")
            .parse()
            .expect("task id");
        assert_eq!(env.status_of(root), Status::Draft, "{root_file}");
        let resp = send(&app, put_admin(&plan_path(root), &runbook_json(plan_file))).await;
        assert_eq!(resp.status, 201, "{plan_file}: {}", resp.text());
        let view = resp.json();
        let adoptions = view["adoptions"].as_array().expect("adoptions");
        assert_eq!(adoptions.len(), adopted, "{plan_file}: {view}");
        assert!(
            adoptions.iter().all(|a| a["adopted"] == true),
            "{plan_file}: {view}"
        );
        assert_eq!(view["decisions_raised"], decisions, "{plan_file}: {view}");
        assert!(
            !events(&env, root)
                .iter()
                .any(|e| matches!(e, Event::PlanApprovalRequested { .. })),
            "{plan_file}"
        );
        assert_eq!(env.status_of(root), Status::Draft, "{plan_file}");
    }
    // BenchFS の採用した task の `parent_id` は `kind = plan` の task のまま（書き換えない）。
    let framing = env
        .store
        .get("01M35X86XTK84F97QW0CN5PGMR".parse().expect("id"))
        .expect("get")
        .expect("framing");
    assert_eq!(framing.parent_id, Some(plan_task));
    assert!(framing.tree.is_some());
    assert_replay_is_clean(&env);
}
