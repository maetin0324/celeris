//! ADR 2026-10-11-task-wait-reasons D5/D8/D9: `GET /tasks`（一覧・`run_state`）と `GET /tasks/{id}/task-tree` の
//! `waits`・`runnable`・`stages`・unit の `waiting_on`。2026-10-11 の本番の 5 件（子待ち・依存待ち・一時停止）と、
//! dispatcher の snapshot 由来の枠待ち、01M4MEEKX4 の段の直列（edges → remove）を store に直接組んで確かめる。

mod common;

use common::*;
use serde_json::{Value, json};
use task_core::{
    CapacityScope, Event, ExecutionPlanRow, ExecutionPlanSpec, PlanOrigin, PlanStatus, Status,
    Task, TaskKind, TaskStore, TaskWait, WorkUnitContext, WorkUnitKind, WorkUnitRow, WorkUnitSpec,
    WorkUnitStatus,
};
use task_ops::daemon::SnapshotTaskWait;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

struct U<'a> {
    key: &'a str,
    kind: WorkUnitKind,
    stage: &'a str,
    status: WorkUnitStatus,
    deps: &'a [&'a str],
    child: Option<task_core::TaskId>,
}

fn spec(u: &U<'_>) -> WorkUnitSpec {
    WorkUnitSpec {
        key: u.key.to_string(),
        kind: u.kind,
        title: format!("unit {}", u.key),
        objective: "o".to_string(),
        depends_on: u.deps.iter().map(|d| d.to_string()).collect(),
        done_when: vec![],
        checks: vec![],
        context: WorkUnitContext::default(),
        harness: None,
        features: None,
        budget: None,
        outputs: vec![],
        phase: Some(u.stage.to_string()),
    }
}

fn plan(env: &TestEnv, task: &Task, units: &[U<'_>]) {
    let plan_id = format!("plan-{}", task.id);
    let spec_all = ExecutionPlanSpec {
        schema: task_core::EXECUTION_PLAN_SCHEMA.to_string(),
        rationale: "fixture".to_string(),
        phases: vec![],
        work_units: units.iter().map(spec).collect(),
        children: vec![],
        stages: vec![],
        units: vec![],
        decisions: vec![],
    };
    let rows = units
        .iter()
        .enumerate()
        .map(|(i, u)| {
            let mut row = WorkUnitRow::new(
                format!("{}-{}", task.id, u.key),
                task.id.to_string(),
                plan_id.clone(),
                i as u32,
                spec(u),
                u.status,
                "2026-10-11T09:00:00Z".to_string(),
            );
            row.phase = Some(u.stage.to_string());
            row.child_task_id = u.child.map(|c| c.to_string());
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
        spec: spec_all.clone(),
        created_at: "2026-10-11T09:00:00Z".to_string(),
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
                plan: Box::new(spec_all),
            },
        )
        .expect("adopt");
}

fn titled(title: &str, status: Status) -> Task {
    let mut t = new_task(TaskKind::Execute, status);
    t.title = title.to_string();
    t
}

fn item(list: &Value, id: task_core::TaskId) -> &Value {
    list["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|i| i["id"] == json!(id.to_string()))
        .unwrap_or_else(|| panic!("{id} not in {list}"))
}

fn ids(list: &Value) -> Vec<String> {
    let mut v: Vec<String> = list["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|i| i["id"].as_str().unwrap_or_default().to_string())
        .collect();
    v.sort();
    v
}

struct Five {
    parent: Task,
    child: Task,
    pred: Task,
    dependent: Task,
    paused: Task,
    free: Task,
    full: Task,
}

/// 2026-10-11 の 5 件の形: 子待ちの親・依存待ち・一時停止と、走れる task・snapshot で枠待ちの task。
fn five(env: &TestEnv) -> Five {
    let parent = titled("parent", Status::Ready);
    env.seed(&parent);
    let mut child = titled("child", Status::Running);
    child.parent_id = Some(parent.id);
    env.seed(&child);
    plan(
        env,
        &parent,
        &[U {
            key: "c",
            kind: WorkUnitKind::Task,
            stage: "s1",
            status: WorkUnitStatus::Running,
            deps: &[],
            child: Some(child.id),
        }],
    );
    let pred = titled("predecessor AP", Status::Blocked);
    env.seed(&pred);
    let mut dependent = titled("dependent AQ", Status::Ready);
    dependent.depends_on = vec![pred.id];
    env.seed(&dependent);
    let mut paused = titled("benchfs", Status::Ready);
    paused.paused_at = Some(OffsetDateTime::now_utc());
    env.seed(&paused);
    let free = titled("free", Status::Ready);
    env.seed(&free);
    let full = titled("full", Status::Ready);
    env.seed(&full);
    Five {
        parent,
        child,
        pred,
        dependent,
        paused,
        free,
        full,
    }
}

fn fresh_snapshot(full: &Task) -> task_ops::daemon::DaemonSnapshot {
    let mut s = snapshot(3);
    s.last_tick_at = OffsetDateTime::now_utc().format(&Rfc3339).expect("rfc3339");
    s.task_waits = vec![SnapshotTaskWait {
        task_id: full.id,
        wait: TaskWait::Capacity {
            scope: CapacityScope::Account,
            id: Some("claude-a".to_string()),
        },
    }];
    s
}

#[tokio::test]
async fn task_wait_api_list_reports_children_depends_on_paused_and_snapshot_capacity() {
    let env = TestEnv::new();
    let app = env.router();
    let f = five(&env);
    env.daemon_tx.send_replace(Some(fresh_snapshot(&f.full)));

    let resp = send(&app, get("/api/v1/tasks?limit=100")).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let list = resp.json();

    let p = item(&list, f.parent.id);
    assert_eq!(p["runnable"], json!(false));
    assert_eq!(p["waits"][0]["kind"], "children", "{p}");
    assert_eq!(p["waits"][0]["task_ids"], json!([f.child.id.to_string()]));
    assert_eq!(p["waits"][0]["unit_keys"], json!(["c"]));
    assert!(
        p.get("pending_children").is_none(),
        "old field is gone: {p}"
    );

    let d = item(&list, f.dependent.id);
    assert_eq!(d["runnable"], json!(false));
    assert_eq!(
        d["waits"],
        json!([{"kind": "depends_on", "task_ids": [f.pred.id.to_string()]}])
    );

    let z = item(&list, f.paused.id);
    assert_eq!(z["runnable"], json!(false));
    assert_eq!(
        z["waits"],
        json!([{"kind": "paused", "by": f.paused.id.to_string()}])
    );

    let c = item(&list, f.full.id);
    assert_eq!(c["runnable"], json!(false));
    assert_eq!(
        c["waits"],
        json!([{"kind": "capacity", "scope": "account", "id": "claude-a"}])
    );

    let ok = item(&list, f.free.id);
    assert_eq!(ok["runnable"], json!(true));
    assert_eq!(ok["waits"], json!([]));
    // 終端でない running の子は ready ではないので runnable ではない。
    assert_eq!(item(&list, f.child.id)["runnable"], json!(false));
}

#[tokio::test]
async fn task_wait_api_run_state_splits_runnable_and_waiting() {
    let env = TestEnv::new();
    let app = env.router();
    let f = five(&env);
    env.daemon_tx.send_replace(Some(fresh_snapshot(&f.full)));

    let resp = send(&app, get("/api/v1/tasks?run_state=runnable")).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let runnable = resp.json();
    assert_eq!(ids(&runnable), vec![f.free.id.to_string()]);
    assert_eq!(runnable["total"], 1);

    let resp = send(&app, get("/api/v1/tasks?run_state=waiting")).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let waiting = resp.json();
    let mut expected = vec![
        f.parent.id.to_string(),
        f.dependent.id.to_string(),
        f.paused.id.to_string(),
        f.full.id.to_string(),
    ];
    expected.sort();
    assert_eq!(ids(&waiting), expected);

    // 位置の cursor で頁を切る。
    let resp = send(&app, get("/api/v1/tasks?run_state=waiting&limit=3")).await;
    let page1 = resp.json();
    assert_eq!(page1["items"].as_array().map(Vec::len), Some(3));
    assert_eq!(page1["total"], 4);
    let cursor = page1["next_cursor"]
        .as_str()
        .expect("next cursor")
        .to_string();
    let resp = send(
        &app,
        get(&format!(
            "/api/v1/tasks?run_state=waiting&limit=3&cursor={cursor}"
        )),
    )
    .await;
    let page2 = resp.json();
    assert_eq!(page2["items"].as_array().map(Vec::len), Some(1));
    assert!(page2["next_cursor"].is_null());

    let resp = send(&app, get("/api/v1/tasks?run_state=sometimes")).await;
    assert_eq!(resp.status.as_u16(), 400, "{}", resp.text());
}

#[tokio::test]
async fn task_wait_api_stale_snapshot_adds_no_memory_waits() {
    let env = TestEnv::new();
    let app = env.router();
    let f = five(&env);
    let mut s = fresh_snapshot(&f.full);
    s.last_tick_at = "2026-09-14T00:00:02Z".to_string();
    env.daemon_tx.send_replace(Some(s));
    let list = send(&app, get("/api/v1/tasks")).await.json();
    let c = item(&list, f.full.id);
    assert_eq!(c["waits"], json!([]), "stale snapshot is not used: {c}");
    assert_eq!(c["runnable"], json!(true));
}

#[tokio::test]
async fn task_wait_api_detail_carries_the_same_waits() {
    let env = TestEnv::new();
    let app = env.router();
    let f = five(&env);
    let detail = send(&app, get(&format!("/api/v1/tasks/{}", f.parent.id)))
        .await
        .json();
    assert_eq!(detail["runnable"], json!(false));
    assert_eq!(detail["waits"][0]["kind"], "children", "{detail}");
    let exec = send(
        &app,
        get(&format!("/api/v1/tasks/{}/execution", f.parent.id)),
    )
    .await
    .json();
    assert_eq!(exec["waits"][0]["kind"], "children", "{exec}");
    assert!(exec.get("awaiting_children").is_none(), "{exec}");
}

/// 01M4MEEKX4 の形: 段 edges → remove の直列。dispatch-prune は cos-route を、integrate-edges は 2 つを待つ。
#[tokio::test]
async fn task_wait_api_tree_shows_unit_waiting_keys_and_stage_order() {
    let env = TestEnv::new();
    let app = env.router();
    let task = titled("runtime", Status::Running);
    env.seed(&task);
    let p = WorkUnitStatus::Pending;
    plan(
        &env,
        &task,
        &[
            U {
                key: "cos-route",
                kind: WorkUnitKind::Implement,
                stage: "edges",
                status: WorkUnitStatus::Running,
                deps: &[],
                child: None,
            },
            U {
                key: "dispatch-prune",
                kind: WorkUnitKind::Implement,
                stage: "edges",
                status: p,
                deps: &["cos-route"],
                child: None,
            },
            U {
                key: "integrate-edges",
                kind: WorkUnitKind::Integrate,
                stage: "edges",
                status: p,
                deps: &["cos-route", "dispatch-prune"],
                child: None,
            },
            U {
                key: "rm-old",
                kind: WorkUnitKind::Implement,
                stage: "remove",
                status: p,
                deps: &[],
                child: None,
            },
            U {
                key: "integrate-remove",
                kind: WorkUnitKind::Integrate,
                stage: "remove",
                status: p,
                deps: &["rm-old"],
                child: None,
            },
        ],
    );
    let resp = send(&app, get(&format!("/api/v1/tasks/{}/task-tree", task.id))).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let view = resp.json();
    let n = &view["nodes"][0];
    assert_eq!(
        n["stages"],
        json!([
            {"key": "edges", "order": 0, "state": "current", "integrate_key": "integrate-edges"},
            {"key": "remove", "order": 1, "state": "pending", "integrate_key": "integrate-remove"},
        ])
    );
    let unit = |k: &str| -> Value {
        n["units"]
            .as_array()
            .expect("units")
            .iter()
            .find(|u| u["key"] == k)
            .cloned()
            .unwrap_or_else(|| panic!("{k} in {n}"))
    };
    let prune = unit("dispatch-prune");
    assert_eq!(prune["waiting_on"], json!(["cos-route"]));
    assert_eq!(prune["depends_on"], json!(["cos-route"]));
    assert_eq!(prune["seq"], 1);
    assert!(prune.get("waiting_stage").is_none());
    assert_eq!(
        unit("integrate-edges")["waiting_on"],
        json!(["cos-route", "dispatch-prune"])
    );
    let rm = unit("rm-old");
    assert_eq!(rm["waiting_on"], json!([]));
    assert_eq!(rm["waiting_stage"], "edges");
    assert_eq!(
        unit("cos-route")["waiting_on"],
        json!([]),
        "running has no wait"
    );
    // 走っている unit があるので task の `WorkUnits` 待ちは出ない（runnable の判定と同じ）。
    assert_eq!(n["waits"], json!([]));
}

/// 子 task の節点にも待ちの理由が出る（子待ちの親と、depends_on で待つ子）。
#[tokio::test]
async fn task_wait_api_tree_child_nodes_carry_waits() {
    let env = TestEnv::new();
    let app = env.router();
    let f = five(&env);
    let view = send(
        &app,
        get(&format!("/api/v1/tasks/{}/task-tree", f.parent.id)),
    )
    .await
    .json();
    let root = &view["nodes"][0];
    assert_eq!(root["waits"][0]["kind"], "children", "{root}");
    assert!(root.get("phase").is_none(), "{root}");
}
