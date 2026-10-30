//! ADR 2026-10-30-plan-conforms-human-guards: what the API sends to a person however CoS read the
//! plan. Every guarded stage names its kind and each kind needs a guard; a human check, an approval
//! reason beyond the plan's own human steps, a failed plan check and an edit of objective or
//! acceptance since the previous approval are refused, for `plan_conforms` and for
//! `within_approved_frame` alike. The CoS worker is a fake harness: the test calls the API with a
//! live CoS run credential (no LLM, no network).
mod common;

use common::cos_ops::cos_bearer;
use common::*;
use serde_json::{Value, json};
use task_core::approval::{ApprovalStore, StandingRule, StandingRuleId};
use task_core::chat::triage::CosTriageSource;
use task_core::decision::{
    CostOfReversal, DecisionKind, DecisionOption, DecisionOrigin, DecisionPathEntry,
    DecisionRaisedBy, DecisionRequest, DecisionStatus,
};
use task_core::{
    Check, EXECUTION_PLAN_SCHEMA_V3, Event, ExecutionPlanRow, ExecutionPlanSpec, PlanOrigin,
    PlanStatus, StageReview, StageSpec, Status, TaskKind, TaskStore, Trigger, WorkUnitKind,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const KINDS: [&str; 6] = [
    "billing",
    "node_hours",
    "production",
    "external_publish",
    "destructive",
    "secrets_permissions",
];

fn db(env: &TestEnv) -> rusqlite::Connection {
    rusqlite::Connection::open(&env.db_path).expect("open db")
}

fn ago(hours: i64) -> String {
    (OffsetDateTime::now_utc() - time::Duration::hours(hours))
        .format(&Rfc3339)
        .expect("time")
}

/// `prep` is reviewed by a person; `run` and `grade` are not.
fn spec() -> ExecutionPlanSpec {
    let stage = |key: &str, review| StageSpec {
        key: key.into(),
        kind: WorkUnitKind::Implement,
        title: format!("段 {key}"),
        review,
    };
    ExecutionPlanSpec {
        schema: EXECUTION_PLAN_SCHEMA_V3.to_string(),
        rationale: String::new(),
        phases: Vec::new(),
        work_units: Vec::new(),
        children: Vec::new(),
        stages: vec![
            stage("prep", StageReview::Human),
            stage("run", StageReview::None),
            stage("grade", StageReview::None),
        ],
        units: Vec::new(),
        decisions: Vec::new(),
    }
}

/// How the task waiting at the plan gate looks.
struct Setup {
    human_check: bool,
    labels: Vec<&'static str>,
    reasons: Vec<&'static str>,
    /// Events appended before the gate (edits, earlier approvals, decisions).
    before: Vec<Event>,
    /// The adoption time of each plan version (oldest first).
    created: Vec<String>,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            human_check: false,
            labels: Vec::new(),
            reasons: vec!["review_human:prep"],
            before: Vec::new(),
            created: vec![ago(0)],
        }
    }
}

fn plan_task(env: &TestEnv, setup: Setup) -> (task_core::Task, String) {
    let mut task = new_task(TaskKind::Execute, Status::Ready);
    task.labels = setup.labels.iter().map(|l| (*l).to_string()).collect();
    if !setup.human_check {
        task.acceptance[0].check = Check::Reviewer;
    }
    env.seed(&task);
    for event in &setup.before {
        env.store.append_event(task.id, event).expect("event");
    }
    let n = setup.created.len();
    let rows: Vec<ExecutionPlanRow> = setup
        .created
        .iter()
        .enumerate()
        .map(|(i, at)| ExecutionPlanRow {
            id: format!("{}-v{}", task.id, i + 1),
            task_id: task.id.to_string(),
            version: i as u32 + 1,
            origin: PlanOrigin::Planner,
            planner_run_id: None,
            status: if i + 1 == n {
                PlanStatus::Active
            } else {
                PlanStatus::Superseded
            },
            spec: spec(),
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
                reasons: setup.reasons.iter().map(|r| (*r).to_string()).collect(),
            }],
        )
        .expect("plan gate");
    (task, plan_id)
}

fn decision_event(task: &task_core::Task, kind: DecisionKind) -> Event {
    Event::DecisionRequested {
        decision: Box::new(DecisionRequest {
            id: format!("dec-{}", task.id),
            key: "plan-check".into(),
            kind,
            question: "計画が 2 回不正でした。どうしますか".into(),
            options: ["replan", "withdraw"]
                .iter()
                .map(|key| DecisionOption {
                    key: (*key).into(),
                    label: (*key).into(),
                    consequence: None,
                })
                .collect(),
            recommended: "replan".into(),
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
                origin: DecisionOrigin::Daemon,
            },
            status: DecisionStatus::Open,
            answer: None,
            withdrawn_reason: None,
        }),
    }
}

fn edited(fields: &[&str]) -> Event {
    Event::Edited {
        fields: fields.iter().map(|f| (*f).to_string()).collect(),
        by: "human".into(),
    }
}

fn approved(plan_id: &str) -> Event {
    Event::PlanGateDecided {
        plan_id: plan_id.into(),
        action: "approve".into(),
        by: "human".into(),
        note: None,
    }
}

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
        policy_version: "10".into(),
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

fn within_frame(rule: &StandingRule) -> Value {
    json!({"category": "within_approved_frame",
           "refs": [{"kind": "usage", "id": "GET /api/v1/usage 2026-10-30T00:00Z"}],
           "frame": {"kind": "standing_rule", "id": rule.id.to_string(), "unit": "node_hours",
                     "limit": 10.0, "used": 2.0, "requested": 1.0}})
}

fn answer(key: &str, basis: Value) -> Value {
    json!({
        "idempotency_key": key,
        "expected_revision": "r1",
        "outcome": "answer",
        "reason": "計画は目的のとおり",
        "policy_version": "1",
        "confidence": 0.95,
        "answer": {"option": "approve"},
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

    /// Sets up a plan-gate task, answers it with `basis(plan_id)` and returns the response.
    async fn answer_gate(
        &self,
        env: &TestEnv,
        key: &str,
        setup: Setup,
        basis: impl FnOnce(&str) -> Value,
    ) -> (Resp, task_core::TaskId) {
        let (task, plan_id) = plan_task(env, setup);
        let item = ingest_plan_gate(env, &self.app, task.id).await;
        (
            self.resolve(&item, answer(key, basis(&plan_id))).await,
            task.id,
        )
    }
}

fn assert_applied(env: &TestEnv, resp: &Resp, id: task_core::TaskId) {
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(resp.json()["operation"]["state"], "applied");
    assert_ne!(env.status_of(id), Status::Blocked);
}

fn assert_held(
    env: &TestEnv,
    resp: &Resp,
    status: u16,
    code: &str,
    id: task_core::TaskId,
) -> Value {
    let body = assert_problem(resp, status, code);
    assert_eq!(env.status_of(id), Status::Blocked, "{code}: {body}");
    body
}

#[tokio::test]
async fn every_stage_kind_needs_a_guard() {
    let env = admin_env();
    let cos = Cos::new(&env, "kinds");
    for kind in KINDS {
        // `run` is not a human review: no guard for `grade`.
        let unguarded = |plan_id: &str| {
            conforms(
                plan_id,
                json!([{"stage": "grade", "kind": kind, "guard": "review_human", "id": "run"}]),
            )
        };
        let (resp, id) = cos
            .answer_gate(&env, &format!("bare-{kind}"), Setup::default(), unguarded)
            .await;
        let body = assert_held(&env, &resp, 422, "cos_basis_plan_unguarded", id);
        assert!(
            body["detail"].as_str().unwrap_or("").contains(kind),
            "{body}"
        );

        let guarded = |plan_id: &str| {
            conforms(
                plan_id,
                json!([{"stage": "grade", "kind": kind, "guard": "review_human", "id": "prep"}]),
            )
        };
        let (resp, id) = cos
            .answer_gate(&env, &format!("ok-{kind}"), Setup::default(), guarded)
            .await;
        assert_applied(&env, &resp, id);
    }
    // The kind is required.
    let (resp, id) = cos
        .answer_gate(&env, "no-kind", Setup::default(), |plan_id| {
            conforms(
                plan_id,
                json!([{"stage": "grade", "guard": "review_human", "id": "prep"}]),
            )
        })
        .await;
    assert!(resp.status.is_client_error(), "{}", resp.text());
    assert_eq!(env.status_of(id), Status::Blocked);
}

#[tokio::test]
async fn human_check_and_human_required_label_are_for_a_person() {
    let env = admin_env();
    let cos = Cos::new(&env, "human");
    let none = |plan_id: &str| conforms(plan_id, json!([]));
    let (resp, id) = cos
        .answer_gate(
            &env,
            "check",
            Setup {
                human_check: true,
                ..Setup::default()
            },
            none,
        )
        .await;
    let body = assert_held(&env, &resp, 403, "cos_human_required", id);
    assert!(
        body["detail"]
            .as_str()
            .unwrap_or("")
            .contains("human check"),
        "{body}"
    );
    let (resp, id) = cos
        .answer_gate(
            &env,
            "label",
            Setup {
                labels: vec!["human_required"],
                ..Setup::default()
            },
            none,
        )
        .await;
    assert_held(&env, &resp, 403, "cos_human_required", id);
}

#[tokio::test]
async fn approval_reasons_beyond_the_plans_human_steps_are_for_a_person() {
    let env = admin_env();
    let cos = Cos::new(&env, "reasons");
    let none = |plan_id: &str| conforms(plan_id, json!([]));
    let (resp, id) = cos
        .answer_gate(
            &env,
            "near",
            Setup {
                reasons: vec!["review_human:prep", "near_limit:max_tree_runs:96/120"],
                ..Setup::default()
            },
            none,
        )
        .await;
    let body = assert_held(&env, &resp, 422, "cos_basis_plan_needs_human", id);
    assert!(
        body["detail"].as_str().unwrap_or("").contains("near_limit"),
        "{body}"
    );
    // A cost reason the policy does not know is for a person too.
    let (resp, id) = cos
        .answer_gate(
            &env,
            "cost",
            Setup {
                reasons: vec!["over_budget:usd:120/100"],
                ..Setup::default()
            },
            none,
        )
        .await;
    assert_held(&env, &resp, 422, "cos_basis_plan_needs_human", id);
    // The plan's own human review alone is fine.
    let (resp, id) = cos.answer_gate(&env, "own", Setup::default(), none).await;
    assert_applied(&env, &resp, id);
}

#[tokio::test]
async fn a_failed_plan_check_stays_with_a_person_and_names_its_kind() {
    let env = admin_env();
    let cos = Cos::new(&env, "checks");
    for kind in [
        DecisionKind::PlanInvalid,
        DecisionKind::Limit,
        DecisionKind::LeafTooLarge,
    ] {
        let (task, plan_id) = plan_task(&env, Setup::default());
        env.store
            .append_event(task.id, &decision_event(&task, kind))
            .expect("raise");
        let item = ingest_plan_gate(&env, &cos.app, task.id).await;
        let resp = cos
            .resolve(&item, answer(kind.as_str(), conforms(&plan_id, json!([]))))
            .await;
        let id = task.id;
        let body = assert_held(&env, &resp, 422, "cos_basis_invalid", id);
        assert!(
            body["detail"]
                .as_str()
                .unwrap_or("")
                .contains(&format!("plan-check ({})", kind.as_str())),
            "{body}"
        );
    }
}

#[tokio::test]
async fn objective_or_acceptance_edits_since_the_previous_approval_are_for_a_person() {
    let env = admin_env();
    let cos = Cos::new(&env, "edits");
    let none = |plan_id: &str| conforms(plan_id, json!([]));
    for (key, fields) in [
        ("objective", &["objective"][..]),
        ("acceptance", &["acceptance", "title"][..]),
    ] {
        let (resp, id) = cos
            .answer_gate(
                &env,
                key,
                Setup {
                    before: vec![edited(fields)],
                    ..Setup::default()
                },
                none,
            )
            .await;
        let body = assert_held(&env, &resp, 422, "cos_basis_plan_needs_human", id);
        assert!(
            body["detail"].as_str().unwrap_or("").contains(key),
            "{body}"
        );
    }
    // An edit after the previous version was approved counts.
    let (resp, id) = cos
        .answer_gate(
            &env,
            "after",
            Setup {
                before: vec![approved("v1"), edited(&["objective"])],
                created: vec![ago(30), ago(0)],
                ..Setup::default()
            },
            none,
        )
        .await;
    assert_held(&env, &resp, 422, "cos_basis_plan_needs_human", id);
    // An edit before the previous approval was seen by that person; other fields never count.
    let (resp, id) = cos
        .answer_gate(
            &env,
            "before",
            Setup {
                before: vec![
                    edited(&["objective"]),
                    approved("v1"),
                    edited(&["title", "priority"]),
                ],
                created: vec![ago(30), ago(0)],
                ..Setup::default()
            },
            none,
        )
        .await;
    assert_applied(&env, &resp, id);
}

#[tokio::test]
async fn plan_churn_keeps_its_limit() {
    let env = admin_env();
    let cos = Cos::new(&env, "churn");
    let none = |plan_id: &str| conforms(plan_id, json!([]));
    let (resp, id) = cos
        .answer_gate(
            &env,
            "three",
            Setup {
                created: vec![ago(20), ago(10), ago(0)],
                ..Setup::default()
            },
            none,
        )
        .await;
    assert_held(&env, &resp, 422, "cos_basis_plan_churn", id);
    let (resp, id) = cos
        .answer_gate(
            &env,
            "two",
            Setup {
                created: vec![ago(10), ago(0)],
                ..Setup::default()
            },
            none,
        )
        .await;
    assert_applied(&env, &resp, id);
}

#[tokio::test]
async fn within_approved_frame_on_a_plan_gate_takes_the_same_human_conditions() {
    let env = admin_env();
    let cos = Cos::new(&env, "frame");
    let rule = StandingRule {
        id: StandingRuleId::new(),
        node_id: None,
        rule: "採点の集計は既定の queue で月 10 node-h まで".into(),
        created_at: OffsetDateTime::now_utc(),
    };
    env.store.standing_rule_append(&rule).expect("rule");
    let frame = |_: &str| within_frame(&rule);
    let (resp, id) = cos
        .answer_gate(
            &env,
            "check",
            Setup {
                human_check: true,
                ..Setup::default()
            },
            frame,
        )
        .await;
    assert_held(&env, &resp, 403, "cos_human_required", id);
    let (resp, id) = cos
        .answer_gate(
            &env,
            "near",
            Setup {
                reasons: vec!["near_limit:max_stages:4/5"],
                ..Setup::default()
            },
            frame,
        )
        .await;
    assert_held(&env, &resp, 422, "cos_basis_plan_needs_human", id);
    let (resp, id) = cos
        .answer_gate(
            &env,
            "edit",
            Setup {
                before: vec![edited(&["acceptance"])],
                ..Setup::default()
            },
            frame,
        )
        .await;
    assert_held(&env, &resp, 422, "cos_basis_plan_needs_human", id);
    let (resp, id) = cos.answer_gate(&env, "ok", Setup::default(), frame).await;
    assert_applied(&env, &resp, id);
}
