//! ADR 2026-10-28-cos-routine-delegated-answers: CoS answers routine judgments inside an approved
//! frame with a basis, and everything else is refused so CoS escalates. The CoS worker is a fake
//! harness: the test calls the API with a live CoS run credential (no LLM, no network). Waits are
//! fixtures written through the store; the derived inbox keys them like the dispatcher's intake.
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
use task_core::{Event, Status, TaskKind, TaskStore};
use time::OffsetDateTime;

const ADR: &str = "agent-docs/adr/2026-10-28-cos-routine-delegated-answers.md";

fn at(secs: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_790_000_000 + secs).expect("time")
}

fn db(env: &TestEnv) -> rusqlite::Connection {
    rusqlite::Connection::open(&env.db_path).expect("open db")
}

fn raise(
    env: &TestEnv,
    task: &task_core::Task,
    id: &str,
    kind: DecisionKind,
    options: &[&str],
    recommended: &str,
) {
    let options = options
        .iter()
        .map(|key| DecisionOption {
            key: (*key).into(),
            label: (*key).into(),
            consequence: None,
        })
        .collect();
    env.store
        .append_event(
            task.id,
            &Event::DecisionRequested {
                decision: Box::new(DecisionRequest {
                    id: id.into(),
                    key: format!("k-{id}"),
                    kind,
                    question: "どうしますか".into(),
                    options,
                    recommended: recommended.into(),
                    cost_of_reversal: CostOfReversal::Low,
                    cost_note: None,
                    needed_before: vec!["c".into()],
                    path: vec![DecisionPathEntry {
                        task_id: task.id,
                        title: "root".into(),
                        stage: Some("s1".into()),
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
                }),
            },
        )
        .expect("raise");
}

fn task(env: &TestEnv, status: Status, events: Vec<Event>) -> task_core::Task {
    let task = new_task(TaskKind::Execute, status);
    env.seed_with(&task, events);
    task
}

/// Ingest one derived wait as a triage item; returns the item row id.
fn ingest(env: &TestEnv, inbox_id: &str, secs: i64) -> String {
    let source = CosTriageSource {
        source_kind: "inbox".into(),
        source_key: inbox_id.into(),
        source_revision: "r1".into(),
        source_event_id: None,
        operation_id: None,
        summary: inbox_id.into(),
        policy_version: "5".into(),
        task_id: None,
    };
    env.store
        .cos_triage_ingest_batch("events", &secs.to_string(), &[source], at(secs))
        .expect("ingest");
    db(env)
        .query_row(
            "SELECT id FROM cos_inbox_items WHERE source_key=?1",
            [inbox_id],
            |r| r.get(0),
        )
        .expect("ingested row")
}

fn answer(key: &str, answer: Value, basis: Value) -> Value {
    json!({
        "idempotency_key": key,
        "expected_revision": "r1",
        "outcome": "answer",
        "reason": "根拠を確かめた",
        "policy_version": "5",
        "confidence": 0.95,
        "answer": answer,
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

/// An applied routine answer keeps its basis: the reason says which category, the payload holds it.
fn assert_routine(env: &TestEnv, resp: &Resp, category: &str) {
    assert_eq!(resp.status.as_u16(), 200, "{category}: {}", resp.text());
    let out = resp.json();
    assert_eq!(out["operation"]["state"], "applied", "{out}");
    assert_eq!(out["item"]["state"], "answered");
    let op = out["operation"]["id"].as_str().expect("op");
    let audits = audit_events(env, op);
    assert_eq!(audits.len(), 1, "{audits:?}");
    let reason = audits[0]["reason"].as_str().expect("reason");
    assert_eq!(reason, format!("定型の代答（{category}）: 根拠を確かめた"));
    let payload: String = db(env)
        .query_row(
            "SELECT payload_json FROM cos_operations WHERE id=?1",
            [op],
            |r| r.get(0),
        )
        .expect("payload");
    let payload: Value = serde_json::from_str(&payload).expect("json");
    assert_eq!(payload["basis"]["category"], category, "{payload}");
}

/// A refused answer is 422 with its code and stays recorded as rejected; the item is still pending.
fn assert_refused(env: &TestEnv, resp: &Resp, code: &str, item: &str) {
    assert_problem(resp, 422, code);
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

/// The escalation packet CoS sends after a refusal.
fn escalate(key: &str, web_path: &str) -> Value {
    json!({
        "idempotency_key": key,
        "expected_revision": "r1",
        "outcome": "escalate",
        "reason": "枠を超えるので人が決める",
        "policy_version": "5",
        "confidence": 0.9,
        "escalation": {
            "summary": "予算の枠を超えるので、続けるかを決めてください",
            "options": [{"key": "continue", "label": "続ける"}, {"key": "stop", "label": "止める"}],
            "recommended": null,
            "recommendation_reason": "枠を超える額は人が決める",
            "web_path": web_path,
        },
    })
}

/// (a) inside a frame a person approved: the frame decision is answered by a person, the usage was
/// read; over the frame is 422 and CoS escalates; a frame that points at nothing is 422.
#[tokio::test]
async fn cos_routine_a_within_approved_frame_answers_and_over_frame_escalates() {
    let env = admin_env();
    let frame_task = task(&env, Status::Ready, vec![]);
    raise(
        &env,
        &frame_task,
        "budget-frame",
        DecisionKind::Choice,
        &["1.5M", "1.0M"],
        "1.5M",
    );
    let human = send(
        &env.router(),
        post_admin(
            "/api/v1/decisions/budget-frame/answer",
            &json!({"option": "1.5M"}),
        ),
    )
    .await;
    assert_eq!(human.status.as_u16(), 200, "{}", human.text());

    let work = task(&env, Status::Ready, vec![]);
    raise(
        &env,
        &work,
        "dec-extend",
        DecisionKind::Choice,
        &["extend", "stop"],
        "stop",
    );
    let item = ingest(&env, "decision-dec-extend", 1);
    let cos = Cos::new(&env, "frame");
    let frame = |used: f64, requested: f64, id: &str| {
        json!({"category": "within_approved_frame",
               "refs": [{"kind": "usage", "id": "GET /api/v1/tasks/x/usage 2026-10-28T00:00Z"},
                        {"kind": "commit", "id": "0123abcd"}],
               "frame": {"kind": "decision", "id": id, "unit": "tokens",
                         "limit": 1_500_000.0, "used": used, "requested": requested}})
    };
    let choice = json!({"option": "extend", "note": "枠内で続ける"});

    let over = cos
        .resolve(
            &item,
            answer(
                "over",
                choice.clone(),
                frame(1_400_000.0, 200_000.0, "budget-frame"),
            ),
        )
        .await;
    assert_refused(&env, &over, "cos_basis_over_frame", &item);
    let nowhere = cos
        .resolve(
            &item,
            answer(
                "nowhere",
                choice.clone(),
                frame(1.0, 1.0, "no-such-decision"),
            ),
        )
        .await;
    assert_refused(&env, &nowhere, "cos_basis_invalid", &item);
    let mut no_usage = frame(1.0, 1.0, "budget-frame");
    no_usage["refs"] = json!([{"kind": "commit", "id": "0123abcd"}]);
    let missing_usage = cos
        .resolve(&item, answer("no-usage", choice.clone(), no_usage))
        .await;
    assert_refused(&env, &missing_usage, "cos_basis_invalid", &item);
    let bare = cos
        .resolve(
            &item,
            json!({
                "idempotency_key": "bare", "expected_revision": "r1", "outcome": "answer",
                "reason": "定型", "policy_version": "5", "confidence": 0.95, "answer": choice,
            }),
        )
        .await;
    assert_refused(&env, &bare, "cos_basis_missing", &item);

    let ok = cos
        .resolve(
            &item,
            answer("ok", choice, frame(1_360_000.0, 100_000.0, "budget-frame")),
        )
        .await;
    assert_routine(&env, &ok, "within_approved_frame");

    // Over the frame CoS escalates instead: the same kind of wait reaches the human.
    let other = task(&env, Status::Ready, vec![]);
    raise(
        &env,
        &other,
        "dec-extend-2",
        DecisionKind::Choice,
        &["continue", "stop"],
        "stop",
    );
    let item2 = ingest(&env, "decision-dec-extend-2", 2);
    let over2 = cos
        .resolve(
            &item2,
            answer(
                "over-2",
                json!({"option": "continue"}),
                frame(1_500_000.0, 1.0, "budget-frame"),
            ),
        )
        .await;
    assert_refused(&env, &over2, "cos_basis_over_frame", &item2);
    let routed = cos
        .resolve(&item2, escalate("esc-2", &format!("/tasks/{}", other.id)))
        .await;
    assert_eq!(routed.status.as_u16(), 200, "{}", routed.text());
    assert_eq!(routed.json()["item"]["state"], "escalated");

    // A standing rule is a frame too; one that does not exist is not.
    let rule = StandingRule {
        id: StandingRuleId::new(),
        node_id: None,
        rule: "pegasus は月 200 node-h まで".into(),
        created_at: OffsetDateTime::now_utc(),
    };
    env.store.standing_rule_append(&rule).expect("rule");
    let third = task(&env, Status::Ready, vec![]);
    raise(
        &env,
        &third,
        "dec-nodeh",
        DecisionKind::Choice,
        &["run", "wait"],
        "wait",
    );
    let item3 = ingest(&env, "decision-dec-nodeh", 3);
    let rule_frame = |id: String| {
        json!({"category": "within_approved_frame",
               "refs": [{"kind": "ledger", "id": "celerisctl eval requests 2026-10-28"}],
               "frame": {"kind": "standing_rule", "id": id, "unit": "node_hours",
                         "limit": 200.0, "used": 120.0, "requested": 16.0}})
    };
    let unknown = cos
        .resolve(
            &item3,
            answer(
                "rule-x",
                json!({"option": "run"}),
                rule_frame(StandingRuleId::new().to_string()),
            ),
        )
        .await;
    assert_refused(&env, &unknown, "cos_basis_invalid", &item3);
    let ok3 = cos
        .resolve(
            &item3,
            answer(
                "rule-ok",
                json!({"option": "run"}),
                rule_frame(rule.id.to_string()),
            ),
        )
        .await;
    assert_routine(&env, &ok3, "within_approved_frame");
}

/// (b) raise a tree limit once: progress is cited and the failure is new; the same failure again,
/// a second raise for the task, or raise-once under another category are 422.
#[tokio::test]
async fn cos_routine_b_raise_once_only_once_and_not_for_a_repeated_failure() {
    let env = admin_env();
    let limit_task = task(&env, Status::Ready, vec![]);
    let limit_options = ["raise-once", "replan", "withdraw"];
    raise(
        &env,
        &limit_task,
        "lim-1",
        DecisionKind::Limit,
        &limit_options,
        "replan",
    );
    let item = ingest(&env, "decision-lim-1", 1);
    let cos = Cos::new(&env, "raise");
    let raise_once = json!({"option": "raise-once", "note": "進捗あり"});
    let basis = |previous: &str, current: &str| {
        json!({"category": "raise_once",
               "refs": [{"kind": "commit", "id": "deadbeef12"},
                        {"kind": "progress", "id": "agent-docs/progress/2026-10-28-cos-routine-delegated-answers.md"}],
               "failures": {"previous": previous, "current": current}})
    };

    let repeated = cos
        .resolve(
            &item,
            answer(
                "same",
                raise_once.clone(),
                basis("test x が timeout", "  test x が\ttimeout "),
            ),
        )
        .await;
    assert_refused(&env, &repeated, "cos_basis_repeated_failure", &item);
    let mut no_progress = basis("a", "b");
    no_progress["refs"] = json!([{"kind": "adr", "id": ADR}]);
    let unproven = cos
        .resolve(
            &item,
            answer("no-progress", raise_once.clone(), no_progress),
        )
        .await;
    assert_refused(&env, &unproven, "cos_basis_invalid", &item);
    let wrong_category = cos
        .resolve(
            &item,
            answer(
                "as-default",
                raise_once.clone(),
                json!({"category": "documented_default", "refs": [{"kind": "adr", "id": ADR}]}),
            ),
        )
        .await;
    assert_refused(&env, &wrong_category, "cos_basis_invalid", &item);

    let ok = cos
        .resolve(
            &item,
            answer(
                "ok",
                raise_once.clone(),
                basis("test x が timeout", "test y の assert"),
            ),
        )
        .await;
    assert_routine(&env, &ok, "raise_once");

    // A second limit of the same task: CoS already raised once, so it escalates.
    raise(
        &env,
        &limit_task,
        "lim-2",
        DecisionKind::Limit,
        &limit_options,
        "replan",
    );
    let item2 = ingest(&env, "decision-lim-2", 2);
    let second = cos
        .resolve(
            &item2,
            answer(
                "again",
                raise_once,
                basis("test y の assert", "test z の build"),
            ),
        )
        .await;
    assert_refused(&env, &second, "cos_basis_raise_once_used", &item2);
}

/// (c) the worker's recommendation is a written default: only the recommended option, only with an
/// ADR or doc ref.
#[tokio::test]
async fn cos_routine_c_documented_default_is_the_recommended_option() {
    let env = admin_env();
    let work = task(&env, Status::Ready, vec![]);
    raise(
        &env,
        &work,
        "dec-release",
        DecisionKind::Choice,
        &["post-merge", "pre-merge"],
        "post-merge",
    );
    let item = ingest(&env, "decision-dec-release", 1);
    let cos = Cos::new(&env, "default");
    let default = json!({"category": "documented_default", "refs": [{"kind": "adr", "id": ADR}]});

    let not_recommended = cos
        .resolve(
            &item,
            answer("pre", json!({"option": "pre-merge"}), default.clone()),
        )
        .await;
    assert_refused(&env, &not_recommended, "cos_basis_invalid", &item);
    let no_doc = cos.resolve(&item, answer("no-doc", json!({"option": "post-merge"}),
        json!({"category": "documented_default", "refs": [{"kind": "message", "id": "m1"}]}))).await;
    assert_refused(&env, &no_doc, "cos_basis_invalid", &item);
    let outside = cos.resolve(&item, answer("outside", json!({"option": "post-merge"}),
        json!({"category": "documented_default", "refs": [{"kind": "adr", "id": concat!("agent-docs/adr/", "../../etc/x.md")}]}))).await;
    assert_refused(&env, &outside, "cos_basis_invalid", &item);

    let ok = cos
        .resolve(
            &item,
            answer("ok", json!({"option": "post-merge"}), default),
        )
        .await;
    assert_routine(&env, &ok, "documented_default");
}

/// (d) work guidance answers a worker question with a note. It cannot carry a frame, and it cannot
/// answer a decision (a replan of a limit changes the plan: CoS escalates it).
#[tokio::test]
async fn cos_routine_d_work_guidance_answers_a_question_without_replan() {
    let env = admin_env();
    let limit_task = task(&env, Status::Ready, vec![]);
    raise(
        &env,
        &limit_task,
        "lim-g",
        DecisionKind::Limit,
        &["raise-once", "replan", "withdraw"],
        "replan",
    );
    let limit_item = ingest(&env, "decision-lim-g", 0);
    let asked = task(
        &env,
        Status::Blocked,
        vec![Event::QuestionRaised {
            run_id: "r0".into(),
            text: "既定の branch に入れますか?".into(),
        }],
    );
    let item = ingest(&env, &format!("question-{}", asked.id), 1);
    let cos = Cos::new(&env, "guidance");
    let guidance = json!({"category": "work_guidance",
                          "refs": [{"kind": "doc", "id": "docs/SPEC.md"}]});

    let replan = cos
        .resolve(
            &limit_item,
            answer(
                "replan",
                json!({"option": "replan", "note": "計画を変える"}),
                guidance.clone(),
            ),
        )
        .await;
    assert_refused(&env, &replan, "cos_basis_invalid", &limit_item);
    let blank = cos
        .resolve(
            &item,
            answer(
                "blank",
                json!({"option": "answer", "note": "x"}),
                json!({"category": "work_guidance", "refs": []}),
            ),
        )
        .await;
    assert_refused(&env, &blank, "cos_basis_invalid", &item);
    let frame_on_guidance = cos.resolve(&item, answer("frame", json!({"note": "main"}),
        json!({"category": "work_guidance", "refs": [{"kind": "doc", "id": "docs/SPEC.md"}],
               "frame": {"kind": "decision", "id": "x", "unit": "u", "limit": 1.0, "used": 0.0, "requested": 1.0}}))).await;
    assert_refused(&env, &frame_on_guidance, "cos_basis_invalid", &item);

    let ok = cos
        .resolve(
            &item,
            answer(
                "ok",
                json!({"note": "受け入れ条件は変えずに main に入れる"}),
                guidance,
            ),
        )
        .await;
    assert_routine(&env, &ok, "work_guidance");
    assert_eq!(env.status_of(asked.id), Status::Ready);
}

/// A wait answered through `/cos/operations` without `instructed_by` needs the same basis.
#[tokio::test]
async fn cos_routine_operations_need_a_basis_without_instruction() {
    let env = admin_env();
    let work = task(&env, Status::Ready, vec![]);
    raise(
        &env,
        &work,
        "dec-op",
        DecisionKind::Choice,
        &["vault", "manual"],
        "vault",
    );
    let cos = Cos::new(&env, "ops");
    let body = |key: &str, basis: Option<Value>| {
        let mut body = json!({
            "idempotency_key": key, "expected_revision": null, "reason": "根拠を確かめた",
            "policy_version": "5",
            "request": {"method": "POST", "path": "/api/v1/decisions/dec-op/answer", "body": {"option": "vault"}},
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
    let ok = send(
        &cos.app,
        post_json_with(
            "/api/v1/cos/operations",
            &body(
                "ok",
                Some(
                    json!({"category": "documented_default", "refs": [{"kind": "adr", "id": ADR}]}),
                ),
            ),
            &headers,
        ),
    )
    .await;
    assert_eq!(ok.status.as_u16(), 200, "{}", ok.text());
    let op = ok.json()["operation"]["id"]
        .as_str()
        .expect("op")
        .to_string();
    let audits = audit_events(&env, &op);
    assert_eq!(
        audits[0]["reason"],
        "定型の代答（documented_default）: 根拠を確かめた"
    );
}
