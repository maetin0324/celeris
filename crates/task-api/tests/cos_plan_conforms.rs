//! ADR 2026-10-30-cos-plan-conforms-approve: CoS approves a plan gate whose plan has nothing out of
//! the ordinary (routine category (f) `plan_conforms`) without waiting for a person. The API refuses
//! another answer than the recommended approve, a stale plan version, an unguarded sensitive stage,
//! an open decision, a human_required label and plan churn, so CoS escalates them. The CoS worker is
//! a fake harness: the test calls the API with a live CoS run credential (no LLM, no network).
mod common;

use common::cos_ops::{audit_events, cos_bearer};
use common::*;
use serde_json::{Value, json};
use task_core::approval::{ApprovalStore, StandingRule, StandingRuleId};
use task_core::chat::triage::CosTriageSource;
use task_core::decision::{
    CostOfReversal, DecisionKind, DecisionOption, DecisionOrigin, DecisionPathEntry,
    DecisionRaisedBy, DecisionRequest, DecisionStatus,
};
use task_core::{
    EXECUTION_PLAN_SCHEMA_V3, Event, ExecutionPlanRow, ExecutionPlanSpec, PlanOrigin, PlanStatus,
    StageReview, StageSpec, Status, TaskKind, TaskStore, Trigger, WorkUnitKind,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

fn db(env: &TestEnv) -> rusqlite::Connection {
    rusqlite::Connection::open(&env.db_path).expect("open db")
}

fn ago(hours: i64) -> String {
    (OffsetDateTime::now_utc() - time::Duration::hours(hours))
        .format(&Rfc3339)
        .expect("time")
}

fn spec(stages: &[(&str, bool)]) -> ExecutionPlanSpec {
    ExecutionPlanSpec {
        schema: EXECUTION_PLAN_SCHEMA_V3.to_string(),
        rationale: String::new(),
        phases: Vec::new(),
        work_units: Vec::new(),
        children: Vec::new(),
        stages: stages
            .iter()
            .map(|(key, human)| StageSpec {
                key: (*key).into(),
                kind: WorkUnitKind::Implement,
                title: format!("段 {key}"),
                review: if *human {
                    StageReview::Human
                } else {
                    StageReview::None
                },
            })
            .collect(),
        units: Vec::new(),
        decisions: Vec::new(),
    }
}

/// A root task waiting for the approval of its last plan version. `created` gives each version's
/// adoption time (oldest first); the stages are those of every version.
fn plan_task(
    env: &TestEnv,
    labels: &[&str],
    stages: &[(&str, bool)],
    created: &[String],
) -> (task_core::Task, String) {
    let mut task = new_task(TaskKind::Execute, Status::Ready);
    task.labels = labels.iter().map(|l| (*l).to_string()).collect();
    env.seed(&task);
    let rows: Vec<ExecutionPlanRow> = created
        .iter()
        .enumerate()
        .map(|(i, at)| ExecutionPlanRow {
            id: format!("{}-v{}", task.id, i + 1),
            task_id: task.id.to_string(),
            version: i as u32 + 1,
            origin: PlanOrigin::Planner,
            planner_run_id: None,
            status: if i + 1 == created.len() {
                PlanStatus::Active
            } else {
                PlanStatus::Superseded
            },
            spec: spec(stages),
            created_at: at.clone(),
            superseded_at: None,
        })
        .collect();
    let plan_id = rows.last().expect("a version").id.clone();
    env.store
        .execution_plans_replace(task.id, rows)
        .expect("plans");
    env.store
        .apply_transition(task.id, Trigger::Dispatch, None)
        .expect("dispatch");
    env.store
        .apply_transition_with_events(
            task.id,
            Trigger::PlanGate {
                plan_id: plan_id.clone(),
            },
            vec![Event::PlanApprovalRequested {
                plan_id: plan_id.clone(),
                reasons: vec!["review_human:prep".into()],
            }],
        )
        .expect("plan gate");
    (task, plan_id)
}

fn raise_open_decision(env: &TestEnv, task: &task_core::Task) {
    let decision = DecisionRequest {
        id: format!("dec-{}", task.id),
        key: "k-pick".into(),
        kind: DecisionKind::Choice,
        question: "どちらで進めますか".into(),
        options: ["a", "b"]
            .iter()
            .map(|key| DecisionOption {
                key: (*key).into(),
                label: (*key).into(),
                consequence: None,
            })
            .collect(),
        recommended: "a".into(),
        cost_of_reversal: CostOfReversal::Low,
        cost_note: None,
        needed_before: vec!["run".into()],
        path: vec![DecisionPathEntry {
            task_id: task.id,
            title: "root".into(),
            stage: Some("run".into()),
            unit: None,
        }],
        raised_by: DecisionRaisedBy {
            task_id: task.id,
            run_id: None,
            origin: DecisionOrigin::Planner,
        },
        status: DecisionStatus::Open,
        answer: None,
        withdrawn_reason: None,
    };
    env.store
        .append_event(
            task.id,
            &Event::DecisionRequested {
                decision: Box::new(decision),
            },
        )
        .expect("raise");
}

/// The plan gate's inbox item, ingested as a triage item; returns the item row id.
async fn ingest_plan_gate(env: &TestEnv, app: &axum::Router, id: task_core::TaskId) -> String {
    let feed = send(app, get_admin("/api/v1/inbox/items")).await;
    assert_eq!(feed.status.as_u16(), 200, "{}", feed.text());
    let feed = feed.json();
    let inbox_id = feed["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|x| x["kind"] == "plan_gate" && x["task"]["id"] == id.to_string())
        .and_then(|x| x["id"].as_str())
        .unwrap_or_else(|| panic!("no plan gate for {id}: {feed}"))
        .to_string();
    let source = CosTriageSource {
        source_kind: "inbox".into(),
        source_key: inbox_id.clone(),
        source_revision: "r1".into(),
        source_event_id: None,
        operation_id: None,
        summary: inbox_id.clone(),
        detail: String::new(),
        policy_version: "8".into(),
        task_id: None,
    };
    env.store
        .cos_triage_ingest_batch(
            "events",
            &id.to_string(),
            &[source],
            OffsetDateTime::now_utc(),
        )
        .expect("ingest");
    db(env)
        .query_row(
            "SELECT id FROM cos_inbox_items WHERE source_key=?1",
            [inbox_id.as_str()],
            |r| r.get(0),
        )
        .expect("ingested row")
}

fn conforms(plan_id: &str, guarded: Value) -> Value {
    json!({"category": "plan_conforms",
           "refs": [{"kind": "message", "id": "GET /api/v1/tasks/x/tree 2026-10-30T00:00Z"}],
           "plan": {"plan_id": plan_id, "guarded_stages": guarded}})
}

fn answer(key: &str, option: &str, basis: Value) -> Value {
    json!({
        "idempotency_key": key,
        "expected_revision": "r1",
        "outcome": "answer",
        "reason": "前の版に採点の経路の修正 1 段を足しただけで、課金の段は人の確認の後",
        "policy_version": "1",
        "confidence": 0.95,
        "answer": {"option": option, "note": if option == "replan" { json!("段を減らす") } else { Value::Null }},
        "basis": basis,
    })
}

struct Cos {
    app: axum::Router,
    bearer: String,
}

impl Cos {
    fn new(env: &TestEnv, key: &str) -> Self {
        Self {
            app: env.router(),
            bearer: cos_bearer(env, key).2,
        }
    }

    async fn resolve(&self, item: &str, body: Value) -> Resp {
        send(
            &self.app,
            post_json_with(
                &format!("/api/v1/cos/inbox/{item}/resolve"),
                &body,
                &[("authorization", self.bearer.as_str())],
            ),
        )
        .await
    }
}

/// A refused answer keeps the task waiting and the item pending, recorded as rejected.
fn assert_refused(
    env: &TestEnv,
    resp: &Resp,
    status: u16,
    code: &str,
    item: &str,
    id: task_core::TaskId,
) {
    assert_problem(resp, status, code);
    assert_eq!(env.status_of(id), Status::Blocked, "{code}");
    let state: String = db(env)
        .query_row(
            "SELECT state FROM cos_inbox_items WHERE id=?1",
            [item],
            |r| r.get(0),
        )
        .expect("item");
    assert_eq!(state, "pending", "{code}");
    let rejected: i64 = db(env)
        .query_row(
            "SELECT COUNT(*) FROM cos_operations WHERE state='rejected' AND target_id=?1",
            [item],
            |r| r.get(0),
        )
        .expect("count");
    assert!(rejected >= 1, "{code} is not recorded as rejected");
}

const STAGES: &[(&str, bool)] = &[("prep", true), ("run", false), ("grade", false)];

#[tokio::test]
async fn plan_conforms_approves_a_plan_whose_sensitive_stages_are_guarded() {
    let env = admin_env();
    let cos = Cos::new(&env, "conforms");
    let rule = StandingRule {
        id: StandingRuleId::new(),
        node_id: None,
        rule: "採点の集計は既定の queue で月 10 node-h まで".into(),
        created_at: OffsetDateTime::now_utc(),
    };
    env.store.standing_rule_append(&rule).expect("rule");
    // v2 a day and a half ago, v3 now: two versions within 24 hours is not churn.
    let (task, plan_id) = plan_task(&env, &[], STAGES, &[ago(36), ago(30), ago(0)]);
    let item = ingest_plan_gate(&env, &cos.app, task.id).await;
    let basis = conforms(
        &plan_id,
        json!([{"stage": "run", "guard": "review_human", "id": "prep"},
               {"stage": "grade", "guard": "standing_rule", "id": rule.id.to_string()}]),
    );
    let resp = cos.resolve(&item, answer("ok", "approve", basis)).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let out = resp.json();
    assert_eq!(out["operation"]["state"], "applied", "{out}");
    assert_eq!(out["item"]["state"], "answered");
    assert_ne!(env.status_of(task.id), Status::Blocked);
    let op = out["operation"]["id"].as_str().expect("op");
    let audits = audit_events(&env, op);
    assert_eq!(audits.len(), 1, "{audits:?}");
    assert_eq!(
        audits[0]["reason"],
        "定型の代答（plan_conforms）: 前の版に採点の経路の修正 1 段を足しただけで、課金の段は人の確認の後"
    );
    let payload: String = db(&env)
        .query_row(
            "SELECT payload_json FROM cos_operations WHERE id=?1",
            [op],
            |r| r.get(0),
        )
        .expect("payload");
    let payload: Value = serde_json::from_str(&payload).expect("json");
    assert_eq!(payload["basis"]["category"], "plan_conforms", "{payload}");
    assert_eq!(payload["basis"]["plan"]["plan_id"], plan_id.as_str());
    assert!(env.store.events_for(task.id).expect("events").iter().any(
        |(_, e)| matches!(e, Event::PlanGateDecided { action, by, .. }
                if action == "approve" && task_core::is_cos_actor(by))
    ));
}

#[tokio::test]
async fn plan_conforms_refuses_other_answers_stale_versions_and_unguarded_stages() {
    let env = admin_env();
    let cos = Cos::new(&env, "refuse");
    let (task, plan_id) = plan_task(&env, &[], STAGES, &[ago(0)]);
    let item = ingest_plan_gate(&env, &cos.app, task.id).await;
    let none = json!([]);
    let cases = [
        // The gate recommends approve, but documented_default skips the plan checks.
        (
            "default",
            "approve",
            json!({"category": "documented_default",
                   "refs": [{"kind": "adr", "id": "agent-docs/adr/2026-10-28-cos-routine-delegated-answers.md"}]}),
            "cos_basis_invalid",
        ),
        // Replan and withdraw change the plan: not the recommended approve.
        (
            "replan",
            "replan",
            conforms(&plan_id, none.clone()),
            "cos_basis_invalid",
        ),
        (
            "withdraw",
            "withdraw",
            conforms(&plan_id, none.clone()),
            "cos_basis_invalid",
        ),
        // The basis names a version that is not the one waiting.
        (
            "stale",
            "approve",
            conforms("plan-old", none.clone()),
            "cos_basis_invalid",
        ),
        (
            "no-plan",
            "approve",
            json!({"category": "plan_conforms", "refs": [{"kind": "message", "id": "seq 4118"}]}),
            "cos_basis_invalid",
        ),
        // basis.plan belongs to plan_conforms only.
        (
            "plan-elsewhere",
            "approve",
            json!({"category": "within_approved_frame",
                   "refs": [{"kind": "usage", "id": "GET /api/v1/usage"}],
                   "plan": {"plan_id": plan_id}}),
            "cos_basis_invalid",
        ),
        (
            "unknown-stage",
            "approve",
            conforms(
                &plan_id,
                json!([{"stage": "publish", "guard": "review_human", "id": "prep"}]),
            ),
            "cos_basis_invalid",
        ),
        // The human review comes after the sensitive stage, not before it.
        (
            "review-after",
            "approve",
            conforms(
                &plan_id,
                json!([{"stage": "prep", "guard": "review_human", "id": "prep"}]),
            ),
            "cos_basis_plan_unguarded",
        ),
        // `run` has no human review.
        (
            "review-none",
            "approve",
            conforms(
                &plan_id,
                json!([{"stage": "grade", "guard": "review_human", "id": "run"}]),
            ),
            "cos_basis_plan_unguarded",
        ),
        (
            "no-approval",
            "approve",
            conforms(
                &plan_id,
                json!([{"stage": "run", "guard": "decision", "id": "dec-none"}]),
            ),
            "cos_basis_plan_unguarded",
        ),
    ];
    for (key, option, basis, code) in cases {
        let resp = cos.resolve(&item, answer(key, option, basis)).await;
        assert_refused(&env, &resp, 422, code, &item, task.id);
    }
}

#[tokio::test]
async fn plan_conforms_leaves_human_required_open_decisions_and_churn_to_a_person() {
    let env = admin_env();
    let cos = Cos::new(&env, "people");

    let (marked, marked_plan) = plan_task(&env, &["human_required"], STAGES, &[ago(0)]);
    let item = ingest_plan_gate(&env, &cos.app, marked.id).await;
    let resp = cos
        .resolve(
            &item,
            answer("marked", "approve", conforms(&marked_plan, json!([]))),
        )
        .await;
    assert_problem(&resp, 403, "cos_human_required");
    assert_eq!(env.status_of(marked.id), Status::Blocked);

    let (open, open_plan) = plan_task(&env, &[], STAGES, &[ago(0)]);
    raise_open_decision(&env, &open);
    let item = ingest_plan_gate(&env, &cos.app, open.id).await;
    let resp = cos
        .resolve(
            &item,
            answer("open", "approve", conforms(&open_plan, json!([]))),
        )
        .await;
    assert_refused(&env, &resp, 422, "cos_basis_invalid", &item, open.id);

    // Three versions within 24 hours: the planner keeps rewriting the plan.
    let (churn, churn_plan) = plan_task(&env, &[], STAGES, &[ago(20), ago(10), ago(0)]);
    let item = ingest_plan_gate(&env, &cos.app, churn.id).await;
    let resp = cos
        .resolve(
            &item,
            answer("churn", "approve", conforms(&churn_plan, json!([]))),
        )
        .await;
    assert_refused(&env, &resp, 422, "cos_basis_plan_churn", &item, churn.id);
}

/// `/cos/operations` `execution.plan_gate` without `instructed_by` takes the same basis.
#[tokio::test]
async fn plan_conforms_applies_to_a_direct_plan_gate_operation() {
    let env = admin_env();
    let cos = Cos::new(&env, "ops");
    let (task, plan_id) = plan_task(&env, &[], STAGES, &[ago(0)]);
    let path = format!("/api/v1/tasks/{}/execution/plan-gate", task.id);
    let headers = [("authorization", cos.bearer.as_str())];
    let body = |key: &str, action: &str, basis: Option<Value>| {
        let mut body = json!({
            "idempotency_key": key, "expected_revision": null, "reason": "計画は目的のとおり",
            "policy_version": "1",
            "request": {"method": "POST", "path": path, "body": {"action": action, "note": "n"}},
        });
        if let Some(basis) = basis {
            body["basis"] = basis;
        }
        body
    };
    let missing = send(
        &cos.app,
        post_json_with(
            "/api/v1/cos/operations",
            &body("bare", "approve", None),
            &headers,
        ),
    )
    .await;
    assert_problem(&missing, 422, "cos_basis_missing");
    let replan = send(
        &cos.app,
        post_json_with(
            "/api/v1/cos/operations",
            &body("replan", "replan", Some(conforms(&plan_id, json!([])))),
            &headers,
        ),
    )
    .await;
    assert_problem(&replan, 422, "cos_basis_invalid");
    assert_eq!(env.status_of(task.id), Status::Blocked);
    let ok = send(
        &cos.app,
        post_json_with(
            "/api/v1/cos/operations",
            &body("ok", "approve", Some(conforms(&plan_id, json!([])))),
            &headers,
        ),
    )
    .await;
    assert_eq!(ok.status.as_u16(), 200, "{}", ok.text());
    let op = ok.json()["operation"]["id"]
        .as_str()
        .expect("op")
        .to_string();
    assert_eq!(
        audit_events(&env, &op)[0]["reason"],
        "定型の代答（plan_conforms）: 計画は目的のとおり"
    );
    assert_ne!(env.status_of(task.id), Status::Blocked);
}
