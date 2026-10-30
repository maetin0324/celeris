//! ADR 2026-10-30-cos-plan-approval: CoS approves a plan gate (a replanned version included) that
//! needs no person without waiting (routine category (f) `plan_approval`), within the replan limit,
//! and leaves human checks, `human_required` labels, near tree limits, failed plan checks and an
//! objective/acceptance edit to a person. The plan's own decisions stay open human waits. The CoS
//! worker is a fake harness: the test calls the API with a live CoS run credential (no LLM, no
//! network).
mod common;

use common::cos_ops::{audit_events, cos_bearer};
use common::*;
use serde_json::{Value, json};
use task_core::chat::triage::CosTriageSource;
use task_core::decision::{
    CostOfReversal, DecisionKind, DecisionOption, DecisionOrigin, DecisionPathEntry,
    DecisionRaisedBy, DecisionRequest, DecisionStatus,
};
use task_core::{
    Check, Criterion, Event, PlanOrigin, Status, TaskId, TaskKind, TaskStore, Trigger,
};
use time::OffsetDateTime;

fn db(env: &TestEnv) -> rusqlite::Connection {
    rusqlite::Connection::open(&env.db_path).expect("open db")
}

fn spec() -> task_core::ExecutionPlanSpec {
    serde_json::from_value(json!({
        "schema": "celeris.execution-plan/1",
        "rationale": "2 段階の直列計画",
        "work_units": [
            {"key": "a", "kind": "implement", "title": "A", "objective": "do A thoroughly and well"},
            {"key": "b", "kind": "implement", "title": "B", "objective": "do B thoroughly and well", "depends_on": ["a"]}
        ]
    }))
    .expect("plan spec")
}

/// A root task judged by a reviewer (no human check unless asked).
fn root_task(env: &TestEnv, labels: &[&str], human_check: bool) -> task_core::Task {
    let mut task = new_task(TaskKind::Execute, Status::Ready);
    task.acceptance = vec![Criterion {
        text: "the reviewer accepts the change".into(),
        check: Check::Reviewer,
    }];
    if human_check {
        task.acceptance.push(Criterion {
            text: "a human is happy".into(),
            check: Check::Human,
        });
    }
    task.labels = labels.iter().map(|l| (*l).to_string()).collect();
    env.seed_with(&task, vec![]);
    task
}

/// The planner adopts `plan-<version>` (no gate).
fn adopt(env: &TestEnv, id: TaskId, version: u32) {
    env.store
        .append_event(
            id,
            &Event::ExecutionPlanned {
                plan_id: format!("plan-{version}"),
                version,
                origin: PlanOrigin::Planner,
                supersedes: (version > 1).then(|| format!("plan-{}", version - 1)),
                reason: None,
                plan: Box::new(spec()),
            },
        )
        .expect("planned");
}

/// The planner adopts `plan-<version>` and the root waits for its approval.
fn gate(env: &TestEnv, id: TaskId, version: u32, reasons: &[&str]) {
    if env.status_of(id) == Status::Ready {
        env.store
            .apply_transition(id, Trigger::Dispatch, None)
            .expect("dispatch");
    }
    adopt(env, id, version);
    let plan_id = format!("plan-{version}");
    env.store
        .apply_transition_with_events(
            id,
            Trigger::PlanGate {
                plan_id: plan_id.clone(),
            },
            vec![Event::PlanApprovalRequested {
                plan_id,
                reasons: reasons.iter().map(|r| (*r).to_string()).collect(),
            }],
        )
        .expect("plan gate");
    assert_eq!(env.status_of(id), Status::Blocked);
}

fn decision(task: TaskId, id: &str, kind: DecisionKind) -> Event {
    let opt = |key: &str| DecisionOption {
        key: key.into(),
        label: key.into(),
        consequence: None,
    };
    Event::DecisionRequested {
        decision: Box::new(DecisionRequest {
            id: id.into(),
            key: format!("k-{id}"),
            kind,
            question: "有料の試行を始めてよいか".into(),
            options: vec![opt("yes"), opt("no")],
            recommended: "no".into(),
            cost_of_reversal: CostOfReversal::Low,
            cost_note: None,
            needed_before: vec!["b".into()],
            path: vec![DecisionPathEntry {
                task_id: task,
                title: "root".into(),
                stage: Some("s1".into()),
                unit: None,
            }],
            raised_by: DecisionRaisedBy {
                task_id: task,
                run_id: None,
                origin: if kind == DecisionKind::Choice {
                    DecisionOrigin::Planner
                } else {
                    DecisionOrigin::Daemon
                },
            },
            status: DecisionStatus::Open,
            answer: None,
            withdrawn_reason: None,
        }),
    }
}

/// The task's plan gate item, ingested as a triage item at revision `rev`.
async fn ingest_gate(env: &TestEnv, app: &axum::Router, id: TaskId, rev: &str) -> String {
    let feed = send(app, get_admin("/api/v1/inbox/items")).await;
    assert_eq!(feed.status.as_u16(), 200, "{}", feed.text());
    let feed = feed.json();
    let items = feed["items"].as_array().expect("items");
    let inbox_id = items
        .iter()
        .find(|x| x["kind"] == "plan_gate" && x["task"]["id"] == id.to_string())
        .and_then(|x| x["id"].as_str())
        .unwrap_or_else(|| panic!("no plan gate item for {id}: {feed}"))
        .to_string();
    let source = CosTriageSource {
        source_kind: "inbox".into(),
        source_key: inbox_id.clone(),
        source_revision: rev.into(),
        source_event_id: None,
        operation_id: None,
        summary: inbox_id.clone(),
        detail: String::new(),
        policy_version: "8".into(),
        task_id: None,
    };
    env.store
        .cos_triage_ingest_batch("events", rev, &[source], OffsetDateTime::now_utc())
        .expect("ingest");
    db(env)
        .query_row(
            "SELECT id FROM cos_inbox_items WHERE source_key=?1 AND source_revision=?2",
            [inbox_id.as_str(), rev],
            |r| r.get(0),
        )
        .expect("ingested row")
}

const ALL_CHECKS: [&str; 7] = [
    "objective_acceptance",
    "cost_frame",
    "production",
    "external_publish",
    "destructive",
    "secrets_permissions",
    "human_steps",
];

fn plan_basis(plans: &[&str], checked: &[&str]) -> Value {
    let refs: Vec<Value> = plans
        .iter()
        .map(|p| json!({"kind": "plan", "id": p}))
        .collect();
    json!({"category": "plan_approval", "refs": refs,
           "plan": {"changes": "v1 に採点〜集計の経路の修正 1 件を足しただけ。受け入れ条件は同じ", "checked": checked}})
}

fn approve(key: &str, rev: &str, basis: Value) -> Value {
    json!({
        "idempotency_key": key,
        "expected_revision": rev,
        "outcome": "answer",
        "reason": "前の版との差分は campaign 側の修正 1 件で、費用・本番・受け入れ条件は変わらない",
        "policy_version": "1",
        "confidence": 0.95,
        "answer": {"option": "approve", "note": null, "payload": null},
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

/// The approval went through: the root left the gate and the audit names the category.
fn assert_approved(env: &TestEnv, resp: &Resp, id: TaskId) -> String {
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let out = resp.json();
    assert_eq!(out["operation"]["state"], "applied", "{out}");
    assert_eq!(out["operation"]["actor"], "cos");
    assert!(out["operation"]["event_id"].is_string(), "card: {out}");
    assert_eq!(out["item"]["state"], "answered");
    assert_ne!(env.status_of(id), Status::Blocked);
    let op = out["operation"]["id"].as_str().expect("op").to_string();
    let audits = audit_events(env, &op);
    assert_eq!(audits.len(), 1, "{audits:?}");
    assert_eq!(
        audits[0]["reason"],
        "定型の代答（plan_approval）: 前の版との差分は campaign 側の修正 1 件で、費用・本番・受け入れ条件は変わらない"
    );
    assert_eq!(audits[0]["target_kind"], "task");
    let events = env.store.events_for(id).expect("events");
    assert!(
        events.iter().any(
            |(_, e)| matches!(e, Event::PlanGateDecided { action, by, .. }
            if action == "approve" && task_core::is_cos_actor(by))
        ),
        "{events:?}"
    );
    op
}

/// A refused approval leaves the root waiting and the item pending, recorded as rejected.
fn assert_refused(env: &TestEnv, resp: &Resp, status: u16, code: &str, item: &str, id: TaskId) {
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

#[tokio::test]
async fn plan_approval_approves_a_clean_replan_and_keeps_its_human_steps() {
    let env = admin_env();
    let cos = Cos::new(&env, "approve");
    let task = root_task(&env, &[], false);
    // v1 needed no approval; v2 (the replan) holds a paid-trial decision and a human review stage.
    adopt(&env, task.id, 1);
    env.store
        .append_event(task.id, &decision(task.id, "d-paid", DecisionKind::Choice))
        .expect("decision");
    gate(
        &env,
        task.id,
        2,
        &["decisions:k-d-paid", "review_human:prep"],
    );
    let item = ingest_gate(&env, &cos.app, task.id, "r1").await;

    // Another category, a missing previous version, an unchecked item and a replan are refused.
    let as_default = cos
        .resolve(
            &item,
            approve("default", "r1", common::cos_ops::documented_default_basis()),
        )
        .await;
    assert_refused(&env, &as_default, 422, "cos_basis_invalid", &item, task.id);
    let no_previous = cos
        .resolve(
            &item,
            approve("no-prev", "r1", plan_basis(&["plan-2"], &ALL_CHECKS)),
        )
        .await;
    assert_refused(&env, &no_previous, 422, "cos_basis_invalid", &item, task.id);
    let unchecked = cos
        .resolve(
            &item,
            approve(
                "unchecked",
                "r1",
                plan_basis(&["plan-2", "plan-1"], &ALL_CHECKS[..6]),
            ),
        )
        .await;
    assert_refused(&env, &unchecked, 422, "cos_basis_invalid", &item, task.id);
    let mut replan = approve(
        "replan",
        "r1",
        plan_basis(&["plan-2", "plan-1"], &ALL_CHECKS),
    );
    replan["answer"] = json!({"option": "replan", "note": "書き直す", "payload": null});
    let replan = cos.resolve(&item, replan).await;
    assert_refused(&env, &replan, 422, "cos_basis_invalid", &item, task.id);

    let ok = cos
        .resolve(
            &item,
            approve("ok", "r1", plan_basis(&["plan-2", "plan-1"], &ALL_CHECKS)),
        )
        .await;
    let op = assert_approved(&env, &ok, task.id);

    // The plan's decision is still a human wait in the inbox.
    let decisions = env.store.decisions_list(Some(task.id)).expect("decisions");
    let paid = decisions.iter().find(|d| d.id == "d-paid").expect("d-paid");
    assert_eq!(paid.status, DecisionStatus::Open);
    let feed = send(&cos.app, get_admin("/api/v1/inbox/items"))
        .await
        .json();
    assert!(
        feed["items"]
            .as_array()
            .expect("items")
            .iter()
            .any(|x| x["kind"] == "decision" && x["task"]["id"] == task.id.to_string()),
        "{feed}"
    );

    // The record is a CoS operation card; a person can revoke it.
    let revoke = send(
        &cos.app,
        post_admin(
            &format!("/api/v1/cos/operations/{op}/override"),
            &json!({"action": "revoke", "reason": "人が計画を見直す"}),
        ),
    )
    .await;
    assert_eq!(revoke.status.as_u16(), 200, "{}", revoke.text());
    assert_ne!(revoke.json()["state"], "applied");
}

#[tokio::test]
async fn plan_approval_approves_a_first_plan_naming_only_itself() {
    let env = admin_env();
    let cos = Cos::new(&env, "first");
    let task = root_task(&env, &[], false);
    gate(&env, task.id, 1, &["review_human:prep"]);
    let item = ingest_gate(&env, &cos.app, task.id, "r1").await;
    let ok = cos
        .resolve(
            &item,
            approve("ok", "r1", plan_basis(&["plan-1"], &ALL_CHECKS)),
        )
        .await;
    assert_approved(&env, &ok, task.id);
}

#[tokio::test]
async fn plan_approval_leaves_human_checks_labels_limits_failed_checks_and_edits_to_a_person() {
    let env = admin_env();
    let cos = Cos::new(&env, "excluded");
    let basis = || plan_basis(&["plan-1"], &ALL_CHECKS);

    let checked = root_task(&env, &[], true);
    gate(&env, checked.id, 1, &["review_human:prep"]);
    let item = ingest_gate(&env, &cos.app, checked.id, "h1").await;
    let resp = cos.resolve(&item, approve("h", "h1", basis())).await;
    assert_refused(&env, &resp, 403, "cos_human_required", &item, checked.id);

    let labeled = root_task(&env, &["human_required"], false);
    gate(&env, labeled.id, 1, &["review_human:prep"]);
    let item = ingest_gate(&env, &cos.app, labeled.id, "l1").await;
    let resp = cos.resolve(&item, approve("l", "l1", basis())).await;
    assert_refused(&env, &resp, 403, "cos_human_required", &item, labeled.id);

    // A plan gated for near tree limits (cost of the tree) goes to a person.
    let near = root_task(&env, &[], false);
    gate(&env, near.id, 1, &["near_limit:max_runs:45/50"]);
    let item = ingest_gate(&env, &cos.app, near.id, "n1").await;
    let resp = cos.resolve(&item, approve("n", "n1", basis())).await;
    assert_refused(
        &env,
        &resp,
        422,
        "cos_basis_plan_needs_human",
        &item,
        near.id,
    );

    // A failed plan check (plan_invalid) in this replan cycle goes to a person.
    let invalid = root_task(&env, &[], false);
    adopt(&env, invalid.id, 1);
    env.store
        .append_event(
            invalid.id,
            &decision(invalid.id, "d-invalid", DecisionKind::PlanInvalid),
        )
        .expect("plan_invalid");
    gate(&env, invalid.id, 2, &["review_human:prep"]);
    let item = ingest_gate(&env, &cos.app, invalid.id, "i1").await;
    let resp = cos
        .resolve(
            &item,
            approve("i", "i1", plan_basis(&["plan-2", "plan-1"], &ALL_CHECKS)),
        )
        .await;
    assert_refused(
        &env,
        &resp,
        422,
        "cos_basis_plan_needs_human",
        &item,
        invalid.id,
    );

    // The person changed the acceptance after the previous plan: the new plan is theirs to judge.
    let edited = root_task(&env, &[], false);
    adopt(&env, edited.id, 1);
    env.store
        .append_event(
            edited.id,
            &Event::Edited {
                fields: vec!["acceptance".into()],
                by: "human".into(),
            },
        )
        .expect("edited");
    gate(&env, edited.id, 2, &["review_human:prep"]);
    let item = ingest_gate(&env, &cos.app, edited.id, "e1").await;
    let resp = cos
        .resolve(
            &item,
            approve("e", "e1", plan_basis(&["plan-2", "plan-1"], &ALL_CHECKS)),
        )
        .await;
    assert_refused(
        &env,
        &resp,
        422,
        "cos_basis_plan_needs_human",
        &item,
        edited.id,
    );
}

#[tokio::test]
async fn plan_approval_stops_at_the_replan_limit() {
    let env = admin_env();
    let cos = Cos::new(&env, "limit");
    let task = root_task(&env, &[], false);

    // CoS approves v1, v2 and v3 (three approvals).
    for version in 1..=3u32 {
        gate(&env, task.id, version, &["review_human:prep"]);
        let rev = format!("r{version}");
        let item = ingest_gate(&env, &cos.app, task.id, &rev).await;
        let current = format!("plan-{version}");
        let previous = format!("plan-{}", version.saturating_sub(1));
        let plans: Vec<&str> = if version == 1 {
            vec![current.as_str()]
        } else {
            vec![current.as_str(), previous.as_str()]
        };
        let ok = cos
            .resolve(
                &item,
                approve(
                    &format!("k{version}"),
                    &rev,
                    plan_basis(&plans, &ALL_CHECKS),
                ),
            )
            .await;
        assert_approved(&env, &ok, task.id);
    }

    // The fourth approval is over the limit; CoS escalates it.
    gate(&env, task.id, 4, &["review_human:prep"]);
    let item = ingest_gate(&env, &cos.app, task.id, "r4").await;
    let over = cos
        .resolve(
            &item,
            approve("k4", "r4", plan_basis(&["plan-4", "plan-3"], &ALL_CHECKS)),
        )
        .await;
    assert_refused(&env, &over, 422, "cos_basis_plan_limit", &item, task.id);
    let escalated = cos
        .resolve(
            &item,
            json!({
                "idempotency_key": "k4-esc",
                "expected_revision": "r4",
                "outcome": "escalate",
                "reason": "自動の計画の承認が上限の 3 回に達した",
                "policy_version": "1",
                "confidence": 0.9,
                "escalation": {
                    "summary": "計画の 4 版目。自動の承認は上限",
                    "options": [{"key": "approve", "label": "計画のとおり進める", "detail": "4 版目で進める"},
                                {"key": "withdraw", "label": "取り下げる", "detail": "task を取り下げる"}],
                    "recommended": null,
                    "recommendation_reason": "replan が続いているので人が決める",
                    "web_path": format!("/tasks/{}", task.id)
                }
            }),
        )
        .await;
    assert_eq!(escalated.status.as_u16(), 200, "{}", escalated.text());
    assert_eq!(escalated.json()["item"]["state"], "escalated");

    // A plan past version 4 goes to a person even if CoS approved none of the earlier ones.
    let long = root_task(&env, &[], false);
    for version in 1..=4 {
        adopt(&env, long.id, version);
    }
    gate(&env, long.id, 5, &["review_human:prep"]);
    let item = ingest_gate(&env, &cos.app, long.id, "v5").await;
    let resp = cos
        .resolve(
            &item,
            approve("v5", "v5", plan_basis(&["plan-5", "plan-4"], &ALL_CHECKS)),
        )
        .await;
    assert_refused(&env, &resp, 422, "cos_basis_plan_limit", &item, long.id);
}

#[tokio::test]
async fn plan_approval_applies_to_a_direct_plan_gate_operation() {
    let env = admin_env();
    let cos = Cos::new(&env, "ops");
    let task = root_task(&env, &[], false);
    gate(&env, task.id, 1, &["review_human:prep"]);
    let body = |key: &str, basis: Option<Value>| {
        let mut body = json!({
            "idempotency_key": key, "expected_revision": null, "reason": "初版の計画で問題が無い",
            "policy_version": "1",
            "request": {"method": "POST", "path": format!("/api/v1/tasks/{}/execution/plan-gate", task.id),
                        "body": {"action": "approve"}},
        });
        if let Some(basis) = basis {
            body["basis"] = basis;
        }
        body
    };
    let headers = [("authorization", cos.bearer.as_str())];
    let missing = send(
        &cos.app,
        post_json_with("/api/v1/cos/operations", &body("bare", None), &headers),
    )
    .await;
    assert_problem(&missing, 422, "cos_basis_missing");
    let as_default = send(
        &cos.app,
        post_json_with(
            "/api/v1/cos/operations",
            &body("default", Some(common::cos_ops::documented_default_basis())),
            &headers,
        ),
    )
    .await;
    assert_problem(&as_default, 422, "cos_basis_invalid");
    assert_eq!(env.status_of(task.id), Status::Blocked);
    let ok = send(
        &cos.app,
        post_json_with(
            "/api/v1/cos/operations",
            &body("ok", Some(plan_basis(&["plan-1"], &ALL_CHECKS))),
            &headers,
        ),
    )
    .await;
    assert_eq!(ok.status.as_u16(), 200, "{}", ok.text());
    assert_eq!(ok.json()["operation"]["state"], "applied");
    assert_ne!(env.status_of(task.id), Status::Blocked);
}
