//! ADR-0079 D11 / §7 R4a: `GET /tasks/{id}/task-tree`（木と roll-up）、`GET /projects/{id}` の root task の合計、
//! `GET /metrics/execution?group_by=depth`（U-R7 の深さ別の指標）の結合テスト。
//!
//! 木は store に直接組む（root → 子 → 孫の 3 段。run は `runs` の索引、unit は `work_units`、quota は
//! `QuotaEstimated`、決定は `DecisionRequested`）。期待値はすべて手で計算した値。

mod common;

use common::*;
use serde_json::{Value, json};
use task_core::decision::{
    CostOfReversal, DecisionKind, DecisionOption, DecisionOrigin, DecisionPathEntry,
    DecisionRaisedBy, DecisionRequest, DecisionStatus,
};
use task_core::quota::{QuotaMethod, QuotaWindow, QuotaWindowUse};
use task_core::{
    Event, ExecutionPlanRow, ExecutionPlanSpec, ParentUnit, PlanOrigin, PlanStatus, Project,
    ProjectId, RunIndexRole, RunIndexStatus, RunRow, Status, Task, TaskId, TaskKind, TaskStore,
    TreeInfo, Usage, WorkUnitContext, WorkUnitKind, WorkUnitRow, WorkUnitSpec, WorkUnitStatus,
};
use time::OffsetDateTime;

fn project(env: &TestEnv) -> Project {
    let now = OffsetDateTime::now_utc();
    let project = Project {
        auto_advance: false,
        slug: None,
        id: ProjectId::new(),
        title: "browser".to_string(),
        request: "do it".into(),
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

fn node(title: &str, status: Status, project: ProjectId, parent: Option<(&Task, &str)>) -> Task {
    let mut t = new_task(TaskKind::Execute, status);
    t.title = title.to_string();
    t.project_id = Some(project);
    if let Some((p, unit)) = parent {
        t.parent_id = Some(p.id);
        t.tree = Some(TreeInfo {
            root_id: task_core::tree::root_id_of(p),
            depth: task_core::tree::depth_of(p) + 1,
            parent_unit: Some(ParentUnit {
                task_id: p.id,
                plan_id: format!("plan-{}", p.id),
                unit_key: unit.to_string(),
                stage: "s1".to_string(),
                attempt: 1,
            }),
            base_commit: None,
        });
    }
    t
}

fn wu(key: &str, kind: WorkUnitKind) -> WorkUnitSpec {
    WorkUnitSpec {
        expected_write_paths: None,
        key: key.to_string(),
        kind,
        title: format!("unit {key}"),
        objective: "o".to_string(),
        depends_on: vec![],
        done_when: vec![],
        checks: vec![],
        context: WorkUnitContext::default(),
        harness: None,
        features: None,
        budget: None,
        outputs: vec![],
        phase: Some("s1".to_string()),
    }
}

/// `task` に計画（/1 の形。段階は `s1`）と unit の行を置く（`(key, kind, status, child)`）。
fn plan(
    env: &TestEnv,
    task: &Task,
    units: &[(&str, WorkUnitKind, WorkUnitStatus, Option<TaskId>)],
) {
    let plan_id = format!("plan-{}", task.id);
    let spec = ExecutionPlanSpec {
        schema: task_core::EXECUTION_PLAN_SCHEMA.to_string(),
        rationale: "fixture".to_string(),
        phases: vec![],
        work_units: units.iter().map(|(k, kind, _, _)| wu(k, *kind)).collect(),
        children: vec![],
        stages: vec![],
        units: vec![],
        decisions: vec![],
    };
    let rows = units
        .iter()
        .enumerate()
        .map(|(i, (k, kind, status, child))| {
            let mut row = WorkUnitRow::new(
                format!("{}-{k}", task.id),
                task.id.to_string(),
                plan_id.clone(),
                i as u32,
                wu(k, *kind),
                *status,
                "2026-09-29T09:00:00Z".to_string(),
            );
            row.phase = Some("s1".to_string());
            row.child_task_id = child.map(|c| c.to_string());
            row
        })
        .collect();
    let row = ExecutionPlanRow {
        id: plan_id.clone(),
        task_id: task.id.to_string(),
        version: 1,
        origin: PlanOrigin::Fixture,
        planner_run_id: None,
        status: PlanStatus::Active,
        spec: spec.clone(),
        created_at: "2026-09-29T09:00:00Z".to_string(),
        superseded_at: None,
    };
    env.store
        .execution_plan_adopt(
            task.id,
            row,
            rows,
            vec![],
            Event::ExecutionPlanned {
                plan_id,
                version: 1,
                origin: PlanOrigin::Fixture,
                supersedes: None,
                reason: None,
                plan: Box::new(spec),
            },
        )
        .expect("adopt");
}

#[allow(clippy::too_many_arguments)]
fn run(
    env: &TestEnv,
    task: &Task,
    id: &str,
    role: RunIndexRole,
    start: &str,
    end: &str,
    tokens: (u64, u64),
    cost: f64,
) {
    env.store
        .run_index_start(RunRow {
            run_id: id.to_string(),
            task_id: task.id.to_string(),
            work_unit_id: None,
            role,
            seq: 1,
            status: RunIndexStatus::Completed,
            adapter: Some("fake".into()),
            model: Some("m".into()),
            account: None,
            session_id: None,
            checkpoint: None,
            usage: Some(Usage {
                input_tokens: Some(tokens.0),
                output_tokens: Some(tokens.1),
                cache_read_tokens: None,
                cache_creation_tokens: None,
                cost_usd: Some(cost),
                duplicate_reads: None,
                session_resumed: None,
            }),
            metrics: None,
            started_at: start.to_string(),
            finished_at: Some(end.to_string()),
        })
        .expect("run");
}

fn quota(env: &TestEnv, task: &Task, run_id: &str, pct: f64) {
    env.store
        .append_event(
            task.id,
            &Event::QuotaEstimated {
                run_id: run_id.to_string(),
                work_unit_id: None,
                source: "claude-oauth".to_string(),
                account: Some("acct-1".to_string()),
                windows: vec![QuotaWindowUse {
                    window: QuotaWindow::FiveHour,
                    before: None,
                    after: None,
                    resets_at: None,
                    used_pct: Some(pct),
                    method: QuotaMethod::Measured,
                }],
                weighted_tokens: 0.0,
                method: QuotaMethod::Measured,
                weights_version: "quota-weights/1".to_string(),
                calibration: None,
                list_price_usd: None,
            },
        )
        .expect("quota");
}

fn decision(env: &TestEnv, node: &Task, root: &Task, key: &str, needed_before: &str) {
    let request = DecisionRequest {
        id: format!("dec-{key}"),
        key: key.into(),
        kind: DecisionKind::Choice,
        question: format!("{key}?"),
        options: vec![
            DecisionOption {
                key: "a".into(),
                label: "A".into(),
                consequence: None,
            },
            DecisionOption {
                key: "b".into(),
                label: "B".into(),
                consequence: None,
            },
        ],
        recommended: "a".into(),
        cost_of_reversal: CostOfReversal::Low,
        cost_note: None,
        needed_before: vec![needed_before.into()],
        path: vec![DecisionPathEntry {
            task_id: root.id,
            title: root.title.clone(),
            stage: Some("s1".into()),
            unit: None,
        }],
        raised_by: DecisionRaisedBy {
            task_id: node.id,
            run_id: None,
            origin: DecisionOrigin::Planner,
        },
        status: DecisionStatus::Open,
        answer: None,
        withdrawn_reason: None,
    };
    env.store
        .append_event(
            node.id,
            &Event::DecisionRequested {
                decision: Box::new(request),
            },
        )
        .expect("decision");
}

struct Fixture {
    project: Project,
    root: Task,
    child: Task,
    grandchild: Task,
    other_root: Task,
}

/// root（ready、子待ち）→ 子（ready、`self` の決定で止まる）→ 孫（done）の 3 段と、同じ案件の別の root（done）。
///
/// | 節点 | run | トークン（in / out） | 定価 | quota（5h） | unit | 決定 |
/// |---|---|---|---|---|---|---|
/// | root | planner 10:00〜10:01 | 100 / 10 | $0.10 | — | a leaf done・c task running（子）・integrate-s1 | 1（c の前） |
/// | 子 | worker 10:10〜10:20 | 1000 / 100 | $1.00 | 3.0pt | x leaf done・g task done（孫） | 1（self） |
/// | 孫 | worker 10:30〜11:00、reviewer 11:00〜11:05 | 300 / 30、50 / 5 | $0.25、$0.05 | 1.0pt | — | — |
/// | 別の root | worker 08:00〜08:30 | 10 / 1 | $0.50 | — | — | — |
fn fixture(env: &TestEnv) -> Fixture {
    let project = project(env);
    let root = node("browser root", Status::Ready, project.id, None);
    env.seed(&root);
    let child = node("phase 2", Status::Ready, project.id, Some((&root, "c")));
    env.seed(&child);
    let grandchild = node("p2-a", Status::Done, project.id, Some((&child, "g")));
    env.seed(&grandchild);
    let other_root = node("docs refresh", Status::Done, project.id, None);
    env.seed(&other_root);

    plan(
        env,
        &root,
        &[
            ("a", WorkUnitKind::Implement, WorkUnitStatus::Done, None),
            (
                "c",
                WorkUnitKind::Task,
                WorkUnitStatus::Running,
                Some(child.id),
            ),
            (
                "integrate-s1",
                WorkUnitKind::Integrate,
                WorkUnitStatus::Pending,
                None,
            ),
        ],
    );
    plan(
        env,
        &child,
        &[
            ("x", WorkUnitKind::Implement, WorkUnitStatus::Done, None),
            (
                "g",
                WorkUnitKind::Task,
                WorkUnitStatus::Done,
                Some(grandchild.id),
            ),
        ],
    );
    run(
        env,
        &root,
        "r-plan",
        RunIndexRole::Planner,
        "2026-09-29T10:00:00Z",
        "2026-09-29T10:01:00Z",
        (100, 10),
        0.10,
    );
    run(
        env,
        &child,
        "c-w",
        RunIndexRole::Worker,
        "2026-09-29T10:10:00Z",
        "2026-09-29T10:20:00Z",
        (1000, 100),
        1.00,
    );
    quota(env, &child, "c-w", 3.0);
    run(
        env,
        &grandchild,
        "g-w",
        RunIndexRole::Worker,
        "2026-09-29T10:30:00Z",
        "2026-09-29T11:00:00Z",
        (300, 30),
        0.25,
    );
    quota(env, &grandchild, "g-w", 1.0);
    run(
        env,
        &grandchild,
        "g-rev",
        RunIndexRole::Reviewer,
        "2026-09-29T11:00:00Z",
        "2026-09-29T11:05:00Z",
        (50, 5),
        0.05,
    );
    run(
        env,
        &other_root,
        "o-w",
        RunIndexRole::Worker,
        "2026-09-29T08:00:00Z",
        "2026-09-29T08:30:00Z",
        (10, 1),
        0.50,
    );
    decision(env, &root, &root, "h1", "c");
    decision(env, &child, &root, "h2", "self");
    Fixture {
        project,
        root,
        child,
        grandchild,
        other_root,
    }
}

fn close(v: &Value, expected: f64) {
    let got = v.as_f64().unwrap_or(f64::NAN);
    assert!((got - expected).abs() < 1e-9, "{got} != {expected}");
}

fn node_of(view: &Value, id: TaskId) -> &Value {
    view["nodes"]
        .as_array()
        .and_then(|n| n.iter().find(|n| n["id"] == json!(id.to_string())))
        .unwrap_or_else(|| panic!("node {id} missing: {view}"))
}

fn tree_env() -> TestEnv {
    TestEnv::with(EnvOptions {
        tree_limits: task_core::TreeLimits {
            enabled: true,
            ..task_core::TreeLimits::default()
        },
        ..Default::default()
    })
}

/// §7 R4a (a)(b): 3 段の木の各節点の roll-up が手計算と一致し、root の合計は subtree の和。
#[tokio::test]
async fn task_tree_rolls_up_a_three_level_tree_by_hand() {
    let env = tree_env();
    let app = env.router();
    let f = fixture(&env);

    let resp = send(&app, get(&format!("/api/v1/tasks/{}/task-tree", f.root.id))).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let view = resp.json();
    assert_eq!(view["root_id"], json!(f.root.id.to_string()));
    assert_eq!(view["subtree_root"], json!(f.root.id.to_string()));
    assert_eq!(view["tree_enabled"], json!(true));
    let ids: Vec<&str> = view["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .map(|n| n["id"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        ids,
        vec![
            f.root.id.to_string(),
            f.child.id.to_string(),
            f.grandchild.id.to_string()
        ],
        "pre-order, parent before child"
    );

    // 孫: 自分の分 = subtree。
    let g = node_of(&view, f.grandchild.id);
    assert_eq!(g["depth"], 3);
    assert_eq!(g["parent_id"], json!(f.child.id.to_string()));
    assert_eq!(g["parent_unit_key"], "g");
    assert_eq!(g["own"], g["subtree"]);
    assert_eq!(g["subtree"]["runs"], 1);
    assert_eq!(g["subtree"]["reviewer_runs"], 1);
    assert_eq!(g["subtree"]["tokens"], 385);
    close(&g["subtree"]["cost_usd"], 0.30);
    close(&g["subtree"]["reviewer_cost_usd"], 0.05);
    assert_eq!(g["subtree"]["wall_ms"], 35 * 60 * 1000);
    assert!(g.get("phase").is_none(), "done nodes have no phase: {g}");

    // 子: self の決定で止まっている。subtree = 子 + 孫。
    let c = node_of(&view, f.child.id);
    assert_eq!(c["depth"], 2);
    assert_eq!(c["parent_unit_key"], "c");
    assert_eq!(c["parent_stage"], "s1");
    assert_eq!(c["plan_version"], 1);
    assert_eq!(c["open_decisions"], 1);
    assert_eq!(c["phase"], "held_on_decision");
    assert_eq!(c["children"], json!([f.grandchild.id.to_string()]));
    assert_eq!(c["own"]["runs"], 1);
    assert_eq!(c["own"]["leaves_done"], 1);
    assert_eq!(c["own"]["leaves_total"], 1);
    assert_eq!(c["own"]["child_tasks_done"], 1);
    assert_eq!(c["own"]["child_tasks_total"], 1);
    assert_eq!(c["subtree"]["tasks"], 2);
    assert_eq!(c["subtree"]["runs"], 2);
    assert_eq!(c["subtree"]["reviewer_runs"], 1);
    assert_eq!(
        c["subtree"]["runs_by_role"],
        json!({"reviewer": 1, "worker": 2})
    );
    assert_eq!(c["subtree"]["tokens"], 1100 + 385);
    close(&c["subtree"]["cost_usd"], 1.30);
    assert_eq!(c["subtree"]["first_run_started_at"], "2026-09-29T10:10:00Z");
    assert_eq!(c["subtree"]["last_run_finished_at"], "2026-09-29T11:05:00Z");
    assert_eq!(c["subtree"]["wall_ms"], 55 * 60 * 1000);
    assert_eq!(c["subtree"]["busy_ms"], (10 + 30 + 5) * 60 * 1000);
    assert_eq!(c["subtree"]["quota"][0]["runs"], 2);
    close(&c["subtree"]["quota"][0]["used_pct"], 4.0);

    // root: 子を待っている。3 節点の合計。
    let r = node_of(&view, f.root.id);
    assert_eq!(r["depth"], 1);
    assert!(r.get("parent_id").is_none());
    assert_eq!(r["phase"], "awaiting_children");
    assert_eq!(r["open_decisions"], 1);
    let units: Vec<(&str, &str)> = r["units"]
        .as_array()
        .expect("units")
        .iter()
        .map(|u| {
            (
                u["key"].as_str().unwrap_or_default(),
                u["status"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        units,
        vec![("a", "done"), ("c", "running"), ("integrate-s1", "pending")]
    );
    assert_eq!(
        r["units"][1]["child_task_id"],
        json!(f.child.id.to_string())
    );
    let s = &r["subtree"];
    assert_eq!(s["tasks"], 3);
    assert_eq!(s["runs"], 3, "planner 1 + worker 2");
    assert_eq!(s["reviewer_runs"], 1);
    assert_eq!(
        s["runs_by_role"],
        json!({"planner": 1, "reviewer": 1, "worker": 2})
    );
    assert_eq!(s["input_tokens"], 100 + 1000 + 300 + 50);
    assert_eq!(s["output_tokens"], 10 + 100 + 30 + 5);
    assert_eq!(s["tokens"], 1595);
    close(&s["cost_usd"], 1.40);
    close(&s["reviewer_cost_usd"], 0.05);
    assert_eq!(s["cost_usd_complete"], true);
    assert_eq!(s["leaves_done"], 2);
    assert_eq!(s["leaves_total"], 2);
    assert_eq!(s["child_tasks_done"], 1);
    assert_eq!(s["child_tasks_total"], 2);
    assert_eq!(s["open_decisions"], 2);
    assert_eq!(s["wall_ms"], 65 * 60 * 1000, "10:00 → 11:05");
    assert_eq!(s["busy_ms"], (1 + 10 + 30 + 5) * 60 * 1000);
    assert_eq!(s["quota"][0]["account"], "acct-1");
    assert_eq!(s["quota"][0]["runs"], 2);
    close(&s["quota"][0]["used_pct"], 4.0);
    assert_eq!(view["totals"], *s, "totals = the view root's subtree");

    // root の合計 = 各節点の自分の分の和（件数・トークン・定価）。
    for field in [
        "runs",
        "reviewer_runs",
        "tokens",
        "busy_ms",
        "open_decisions",
    ] {
        let sum: u64 = [r, c, g]
            .iter()
            .map(|n| n["own"][field].as_u64().unwrap_or(0))
            .sum();
        assert_eq!(s[field].as_u64(), Some(sum), "{field}");
    }

    // 木の上限の使用（root の view だけ）: leaf 2（a・x）、run 3（reviewer を除く）、replan 0、決定 2。
    let limits = &view["limits"];
    assert_eq!(limits["leaves"], 2);
    // ADR-0079 付記「R6-2」: 木の上限の既定は leaf 120 / run 400 / replan 30（40 / 120 / 10 から）。
    assert_eq!(limits["max_leaves"], 120);
    assert_eq!(limits["runs"], 3);
    assert_eq!(limits["max_runs"], 400);
    assert_eq!(limits["replans"], 0);
    assert_eq!(limits["max_replans"], 30);
    assert_eq!(limits["open_decisions"], 2);
    assert_eq!(limits["max_open_decisions"], 12);

    // 子の subtree（既定）: 子と孫だけ、上限の使用は出さない。`?root=true` なら孫からでも root の木。
    let resp = send(
        &app,
        get(&format!("/api/v1/tasks/{}/task-tree", f.child.id)),
    )
    .await;
    let sub = resp.json();
    assert_eq!(sub["subtree_root"], json!(f.child.id.to_string()));
    assert_eq!(sub["nodes"].as_array().map(Vec::len), Some(2));
    assert!(sub.get("limits").is_none(), "{sub}");
    assert!(sub["nodes"][0].get("parent_id").is_none());
    assert_eq!(sub["totals"], c["subtree"]);
    let resp = send(
        &app,
        get(&format!(
            "/api/v1/tasks/{}/task-tree?root=true",
            f.grandchild.id
        )),
    )
    .await;
    let whole = resp.json();
    assert_eq!(whole["subtree_root"], json!(f.root.id.to_string()));
    assert_eq!(whole["totals"], view["totals"]);
}

/// §7 R4a (b) `legacy_task_is_a_single_node_tree`: 木の無い task（`[execution.tree] enabled = false`）は 1 節点の木
/// （深さ 1、親なし、自分の分 = subtree）。不明な task は 404、`root` が真偽値でなければ 400。
#[tokio::test]
async fn legacy_task_is_a_single_node_tree() {
    let env = TestEnv::new();
    let app = env.router();
    let task = new_task(TaskKind::Execute, Status::Done);
    env.seed(&task);
    run(
        &env,
        &task,
        "w1",
        RunIndexRole::Worker,
        "2026-09-29T08:00:00Z",
        "2026-09-29T08:10:00Z",
        (40, 4),
        0.20,
    );
    run(
        &env,
        &task,
        "rv1",
        RunIndexRole::Reviewer,
        "2026-09-29T08:10:00Z",
        "2026-09-29T08:12:00Z",
        (4, 1),
        0.02,
    );
    for q in ["", "?root=true"] {
        let resp = send(
            &app,
            get(&format!("/api/v1/tasks/{}/task-tree{q}", task.id)),
        )
        .await;
        assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
        let view = resp.json();
        assert_eq!(view["tree_enabled"], json!(false));
        assert_eq!(view["root_id"], json!(task.id.to_string()));
        let nodes = view["nodes"].as_array().expect("nodes");
        assert_eq!(nodes.len(), 1, "{view}");
        let n = &nodes[0];
        assert_eq!(n["depth"], 1);
        assert!(n.get("parent_id").is_none());
        assert_eq!(n["children"], json!([]));
        assert_eq!(n["units"], json!([]));
        assert_eq!(n["own"], n["subtree"]);
        assert_eq!(n["own"]["runs"], 1);
        assert_eq!(n["own"]["reviewer_runs"], 1);
        close(&n["own"]["cost_usd"], 0.22);
        assert_eq!(n["own"]["wall_ms"], 12 * 60 * 1000);
        assert_eq!(view["limits"]["runs"], 1);
    }
    let resp = send(
        &app,
        get(&format!("/api/v1/tasks/{}/task-tree", TaskId::new())),
    )
    .await;
    assert_problem(&resp, 404, "task_not_found");
    let resp = send(
        &app,
        get(&format!("/api/v1/tasks/{}/task-tree?root=maybe", task.id)),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 400, "{}", resp.text());
    let resp = send(
        &app,
        get(&format!("/api/v1/tasks/{}/task-tree?depth=2", task.id)),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 400, "{}", resp.text());
    // 作業ツリーの閲覧（ADR-0043 D6、`GET /tasks/{id}/tree`）はそのまま（作業ツリーの無い task は 404）。
    let resp = send(&app, get(&format!("/api/v1/tasks/{}/tree", task.id))).await;
    assert_eq!(resp.status.as_u16(), 404, "{}", resp.text());
}

/// D11: `GET /projects/{id}` の `root_totals` は root task の数（状態ごと）と subtree の和（子・孫は root に数える）。
#[tokio::test]
async fn project_detail_sums_the_root_task_subtrees() {
    let env = tree_env();
    let app = env.router();
    let f = fixture(&env);
    // 同じ案件の対話 task は root に数えない。
    let mut chat = new_task(TaskKind::Execute, Status::Done);
    chat.project_id = Some(f.project.id);
    chat.conversation = Some(task_core::MessageId::new());
    env.seed(&chat);

    let resp = send(&app, get(&format!("/api/v1/projects/{}", f.project.id))).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let body = resp.json();
    let totals = &body["root_totals"];
    assert_eq!(totals["root_tasks"], 2, "{totals}");
    assert_eq!(totals["by_status"], json!({"done": 1, "ready": 1}));
    let t = &totals["totals"];
    assert_eq!(t["tasks"], 4, "root + child + grandchild + the other root");
    assert_eq!(t["runs"], 4);
    assert_eq!(t["reviewer_runs"], 1);
    close(&t["cost_usd"], 1.90);
    close(&t["reviewer_cost_usd"], 0.05);
    assert_eq!(t["tokens"], 1595 + 11);
    assert_eq!(t["open_decisions"], 2);
    assert_eq!(
        t["quota"],
        json!([]),
        "the project page does not read events"
    );
    assert_eq!(t["first_run_started_at"], "2026-09-29T08:00:00Z");
    assert_eq!(t["last_run_finished_at"], "2026-09-29T11:05:00Z");
    // 案件の task の一覧（従来の欄）は変わらない。
    assert_eq!(body["tasks"].as_array().map(Vec::len), Some(5));
    let _ = &f.other_root;
}

/// U-R7: `GET /metrics/execution?group_by=depth` は深さごとに task の数・role ごとの run（reviewer を含む）・定価・
/// quota・壁時計を出す（深さ 2 以下の reviewer の run と定価が 1 本ずつ見える）。
#[tokio::test]
async fn execution_metrics_group_by_depth_counts_reviewer_runs_per_depth() {
    let env = tree_env();
    let app = env.router();
    let _f = fixture(&env);

    let resp = send(&app, get("/api/v1/metrics/execution?group_by=depth")).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let body = resp.json();
    assert_eq!(body["group_by"], "depth");
    let groups = body["groups"].as_array().expect("groups");
    let keys: Vec<&str> = groups
        .iter()
        .map(|g| g["key"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(keys, vec!["1", "2", "3"]);
    let d1 = &groups[0];
    assert_eq!(d1["tasks"], 2, "two roots");
    assert_eq!(
        d1["rollup"]["runs_by_role"],
        json!({"planner": 1, "worker": 1})
    );
    assert_eq!(d1["rollup"]["reviewer_runs"], 0);
    close(&d1["rollup"]["cost_usd"], 0.60);
    assert_eq!(
        d1["rollup"]["wall_ms"],
        (2 * 60 + 1) * 60 * 1000,
        "08:00 → 10:01"
    );
    let d2 = &groups[1];
    assert_eq!(d2["tasks"], 1);
    assert_eq!(d2["rollup"]["runs"], 1);
    close(&d2["rollup"]["cost_usd"], 1.00);
    assert_eq!(d2["rollup"]["quota"][0]["runs"], 1);
    close(&d2["rollup"]["quota"][0]["used_pct"], 3.0);
    assert_eq!(d2["quota"][0]["runs"], 1, "the group's own quota agrees");
    let d3 = &groups[2];
    assert_eq!(d3["rollup"]["reviewer_runs"], 1);
    close(&d3["rollup"]["reviewer_cost_usd"], 0.05);
    close(&d3["rollup"]["cost_usd"], 0.30);
    assert_eq!(
        d3["rollup"]["runs_by_role"],
        json!({"reviewer": 1, "worker": 1})
    );

    // 他の group_by には `rollup` を出さない（互換）。
    let resp = send(&app, get("/api/v1/metrics/execution?group_by=genre")).await;
    let body = resp.json();
    assert!(
        body["groups"]
            .as_array()
            .expect("groups")
            .iter()
            .all(|g| g.get("rollup").is_none()),
        "{body}"
    );
    let resp = send(&app, get("/api/v1/metrics/execution?group_by=height")).await;
    assert_eq!(resp.status.as_u16(), 400);
    assert!(resp.text().contains("depth"), "{}", resp.text());
}
