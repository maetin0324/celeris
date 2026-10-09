//! ADR 2026-10-09-cos-operations-all-mutations D3/D6: the required representatives. Each operation is
//! refused when called directly with the CoS credential (422 + rejected audit row) and applied through
//! `/cos/operations` with its row, audit envelope event and chat card; the domain write carries `cos`.
mod common;

use common::cos_ops::{OPS, audit_events, cos_bearer, op_body, run_domain};
use common::*;
use serde_json::json;
use task_core::decision::{
    CostOfReversal, DecisionKind, DecisionOption, DecisionOrigin, DecisionPathEntry,
    DecisionRaisedBy, DecisionRequest, DecisionStatus,
};
use task_core::model_catalog::{CatalogSource, DiscoveredModel};
use task_core::{Event, ModelCatalogStore, Status, TaskId, TaskKind, TaskStore, Tier};

fn raise_decision(env: &TestEnv, task: TaskId, id: &str) {
    let opt = |key: &str| DecisionOption {
        key: key.into(),
        label: key.into(),
        consequence: None,
    };
    env.store
        .append_event(
            task,
            &Event::DecisionRequested {
                decision: Box::new(DecisionRequest {
                    id: id.into(),
                    key: format!("k-{id}"),
                    kind: DecisionKind::Choice,
                    question: "which?".into(),
                    options: vec![opt("vault"), opt("manual")],
                    recommended: "vault".into(),
                    cost_of_reversal: CostOfReversal::Low,
                    cost_note: None,
                    needed_before: vec!["c".into()],
                    path: vec![DecisionPathEntry {
                        task_id: task,
                        title: "root".into(),
                        stage: Some("s1".into()),
                        unit: None,
                    }],
                    raised_by: DecisionRaisedBy {
                        task_id: task,
                        run_id: None,
                        origin: DecisionOrigin::Planner,
                    },
                    status: DecisionStatus::Open,
                    answer: None,
                    withdrawn_reason: None,
                }),
            },
        )
        .expect("raise");
}

fn plan_body(last: &str) -> serde_json::Value {
    json!({
        "schema": "celeris.execution-plan/1",
        "rationale": "直列計画",
        "work_units": [
            {"key": "a", "kind": "implement", "title": "A", "objective": "do A thoroughly and well"},
            {"key": last, "kind": "implement", "title": "B", "objective": "do B thoroughly and well", "depends_on": ["a"]}
        ]
    })
}

#[tokio::test]
async fn cos_ops_mutations_decision_revise_and_withdraw() {
    let env = admin_env();
    let app = env.router();
    let task = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&task);

    // revise: an answered choice gets a new answer from cos.
    raise_decision(&env, task.id, "dec-revise");
    let human = send(
        &app,
        post_admin(
            "/api/v1/decisions/dec-revise/answer",
            &json!({"option": "vault"}),
        ),
    )
    .await;
    assert_eq!(human.status.as_u16(), 200, "{}", human.text());
    let op = run_domain(
        &env,
        "decision-revise",
        "POST",
        "/api/v1/decisions/dec-revise/revise",
        json!({"option": "manual"}),
        "decision.revise",
    )
    .await;
    assert_eq!(op["target_kind"], "decision");
    let events = env.store.events_for(task.id).expect("events");
    assert!(events.iter().any(|(_, e)| matches!(
        e,
        Event::DecisionAnswered { id, by, option, .. }
            if id == "dec-revise" && by == "cos" && option == "manual"
    )));

    // withdraw: an open decision is withdrawn by cos (the body may be omitted).
    raise_decision(&env, task.id, "dec-withdraw");
    run_domain(
        &env,
        "decision-withdraw",
        "POST",
        "/api/v1/decisions/dec-withdraw/withdraw",
        json!({"reason": "不要になった"}),
        "decision.withdraw",
    )
    .await;
    let events = env.store.events_for(task.id).expect("events");
    assert!(events.iter().any(|(_, e)| matches!(
        e,
        Event::DecisionWithdrawn { id, reason } if id == "dec-withdraw" && reason.starts_with("cos:")
    )));
    // A second withdraw is a domain conflict: 409, recorded as rejected, no new event.
    let (thread, _, bearer) = cos_bearer(&env, "decision-withdraw-again");
    let again = send(
        &app,
        post_json_with(
            OPS,
            &op_body(
                "again",
                "POST",
                "/api/v1/decisions/dec-withdraw/withdraw",
                json!(null),
            ),
            &[("authorization", &bearer)],
        ),
    )
    .await;
    assert_eq!(again.status.as_u16(), 409, "{}", again.text());
    let row = env
        .store
        .cos_operation_find(&thread, "again")
        .expect("lookup")
        .expect("row");
    assert_eq!(row.state, "rejected");
    assert_eq!(audit_events(&env, &row.id).len(), 1);
}

#[tokio::test]
async fn cos_ops_mutations_task_gate_actions() {
    let env = admin_env();
    for (key, kind, status, suffix, body, action, expected) in [
        (
            "task-accept",
            TaskKind::Execute,
            Status::Draft,
            "accept",
            json!({}),
            "task.accept",
            Status::Ready,
        ),
        (
            "task-approve",
            TaskKind::Execute,
            Status::Draft,
            "approve",
            json!({}),
            "task.approve",
            Status::Ready,
        ),
        (
            "task-reject",
            TaskKind::Approval,
            Status::Ready,
            "reject",
            json!({"note":"no"}),
            "task.reject",
            Status::Failed,
        ),
        (
            "task-cancel",
            TaskKind::Execute,
            Status::Ready,
            "cancel",
            json!({}),
            "task.cancel",
            Status::Cancelled,
        ),
    ] {
        let task = new_task(kind, status);
        env.seed(&task);
        let path = format!("/api/v1/tasks/{}/{suffix}", task.id);
        let operation = run_domain(&env, key, "POST", &path, body, action).await;
        assert_eq!(operation["state"], "applied", "{key}: {operation}");
        assert_eq!(
            env.store
                .get(task.id)
                .expect("task lookup")
                .expect("task")
                .status,
            expected
        );
    }
}

#[tokio::test]
async fn cos_ops_mutations_task_edit_pause_resume_reopen_retry() {
    let env = admin_env();
    let ready = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&ready);
    let path = format!("/api/v1/tasks/{}", ready.id);

    let op = run_domain(
        &env,
        "task-edit",
        "PATCH",
        &path,
        json!({"title": "CoS が直した題", "expected_write_paths": ["crates/x"]}),
        "task.update",
    )
    .await;
    assert_eq!(
        op["result"]["fields"],
        json!(["title", "expected_write_paths"])
    );
    let edited = env.store.get(ready.id).expect("get").expect("task");
    assert_eq!(edited.title, "CoS が直した題");
    let events = env.store.events_for(ready.id).expect("events");
    assert!(events.iter().any(|(_, e)| matches!(
        e,
        Event::Edited { fields, by } if by == "cos" && fields == &vec!["title".to_string()]
    )));

    run_domain(
        &env,
        "task-pause",
        "POST",
        &format!("{path}/pause"),
        json!({}),
        "task.pause",
    )
    .await;
    assert!(
        env.store
            .get(ready.id)
            .expect("get")
            .expect("task")
            .paused_at
            .is_some()
    );
    run_domain(
        &env,
        "task-resume",
        "POST",
        &format!("{path}/resume"),
        json!(null),
        "task.resume",
    )
    .await;
    assert!(
        env.store
            .get(ready.id)
            .expect("get")
            .expect("task")
            .paused_at
            .is_none()
    );

    let done = new_task(TaskKind::Execute, Status::Done);
    env.seed(&done);
    let op = run_domain(
        &env,
        "task-reopen",
        "POST",
        &format!("/api/v1/tasks/{}/reopen", done.id),
        json!({"expected_status": "done"}),
        "task.reopen",
    )
    .await;
    assert_eq!(op["result"]["to"], "ready");
    assert_eq!(
        env.store.get(done.id).expect("get").expect("task").status,
        Status::Ready
    );

    let failed = new_task(TaskKind::Execute, Status::Failed);
    env.seed(&failed);
    let op = run_domain(
        &env,
        "task-retry",
        "POST",
        &format!("/api/v1/tasks/{}/retry", failed.id),
        json!({"accept": true}),
        "task.retry",
    )
    .await;
    let new_id: TaskId = op["result"]["new_task_id"]
        .as_str()
        .expect("new task")
        .parse()
        .expect("ulid");
    let copy = env.store.get(new_id).expect("get").expect("copy");
    assert_eq!(copy.status, Status::Ready);
    assert_eq!(copy.title, failed.title);
}

#[tokio::test]
async fn cos_ops_mutations_execution_plan_put_replans_an_active_plan() {
    let env = admin_env();
    let app = env.router();
    let task = new_task(TaskKind::Execute, Status::Draft);
    env.seed(&task);
    let path = format!("/api/v1/tasks/{}/execution-plan", task.id);
    let adopted = send(&app, post_admin(&path, &plan_body("b"))).await;
    assert_eq!(adopted.status.as_u16(), 201, "{}", adopted.text());

    let op = run_domain(
        &env,
        "plan-put",
        "PUT",
        &path,
        plan_body("b2"),
        "execution.put_plan",
    )
    .await;
    assert_eq!(op["result"]["version"], 2);
    let plan = env
        .store
        .execution_plan_active(task.id)
        .expect("plan")
        .expect("active");
    assert_eq!(plan.version, 2);
    let keys: Vec<String> = env
        .store
        .work_units_for(task.id)
        .expect("units")
        .into_iter()
        .filter(|u| u.status != task_core::WorkUnitStatus::Superseded)
        .map(|u| u.key)
        .collect();
    assert!(keys.contains(&"b2".to_string()), "{keys:?}");

    // Without an active plan the PUT would adopt a first plan: refused with a reason, no plan.
    let fresh = new_task(TaskKind::Execute, Status::Draft);
    env.seed(&fresh);
    let (thread, _, bearer) = cos_bearer(&env, "plan-put-first");
    let resp = send(
        &app,
        post_json_with(
            OPS,
            &op_body(
                "first",
                "PUT",
                &format!("/api/v1/tasks/{}/execution-plan", fresh.id),
                plan_body("b"),
            ),
            &[("authorization", &bearer)],
        ),
    )
    .await;
    let problem = assert_problem(&resp, 422, "cos_operation_not_allowed");
    assert!(
        problem["detail"]
            .as_str()
            .expect("detail")
            .contains("execution.adopt_plan")
    );
    assert!(
        env.store
            .execution_plan_active(fresh.id)
            .expect("plan")
            .is_none()
    );
    let row = env
        .store
        .cos_operation_find(&thread, "first")
        .expect("lookup")
        .expect("row");
    assert_eq!(row.state, "rejected");
}

#[tokio::test]
async fn cos_ops_mutations_llm_assignment_put_and_delete() {
    let env = admin_env();
    let source = CatalogSource::new("opencode-go");
    env.store
        .model_catalog_apply(&source, &[DiscoveredModel::new("glm-5")], 1_700_000_000)
        .expect("catalog");
    let path = "/api/v1/llm/models/assignments/opencode-go/cheap";

    let op = run_domain(
        &env,
        "assign-put",
        "PUT",
        path,
        json!({"model_id": "glm-5", "note": "安い枠"}),
        "model_assignment.put",
    )
    .await;
    assert_eq!(op["result"]["model_id"], "glm-5");
    let rows = env.store.model_role_assignments().expect("rows");
    let row = rows
        .iter()
        .find(|a| a.source == source && a.tier == Tier::Cheap)
        .expect("assignment");
    assert_eq!(row.model_id, "glm-5");
    assert_eq!(row.updated_by, "cos");

    run_domain(
        &env,
        "assign-delete",
        "DELETE",
        path,
        json!(null),
        "model_assignment.delete",
    )
    .await;
    assert!(
        !env.store
            .model_role_assignments()
            .expect("rows")
            .iter()
            .any(|a| a.source == source && a.tier == Tier::Cheap)
    );

    // A model outside the catalog is refused before any write, recorded with the reason.
    let app = env.router();
    let (thread, _, bearer) = cos_bearer(&env, "assign-unknown");
    let resp = send(
        &app,
        post_json_with(
            OPS,
            &op_body("unknown", "PUT", path, json!({"model_id": "nope"})),
            &[("authorization", &bearer)],
        ),
    )
    .await;
    assert_problem(&resp, 400, "model_not_in_catalog");
    let row = env
        .store
        .cos_operation_find(&thread, "unknown")
        .expect("lookup")
        .expect("row");
    assert_eq!(row.state, "rejected");
}

#[tokio::test]
async fn cos_ops_mutations_knowledge_accept() {
    let env = admin_env();
    task_ops::knowledge::init(&env.knowledge_root).expect("knowledge init");
    let candidate = task_ops::knowledge::record(
        &env.knowledge_root,
        &task_ops::knowledge::RecordRequest {
            title: "fern03 の使い方".into(),
            scope: "environment".into(),
            tags: vec!["server".into()],
            sources: vec!["task:01J1".into()],
            confidence: Some(task_core::Confidence::Medium),
            body: "ssh fern03 で入る。".into(),
            path: Some("environment/servers/fern03.md".into()),
            op: None,
        },
    )
    .expect("record");
    let op = run_domain(
        &env,
        "knowledge-accept",
        "POST",
        &format!("/api/v1/knowledge/inbox/{}/accept", candidate.id),
        json!(null),
        "knowledge.accept",
    )
    .await;
    assert_eq!(op["result"]["path"], "environment/servers/fern03.md");
    assert!(
        env.knowledge_root
            .join("environment/servers/fern03.md")
            .exists()
    );
}
