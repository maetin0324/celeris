//! ADR 2026-10-29-cos-review-rework-reopen: CoS reopens a task the reviewer failed for a technical
//! defect without waiting for a person (routine category (e) `review_rework`), within the limit and
//! never for a repeated rejection, a human check, a human_required label or a failure that is not a
//! reviewer verdict. The CoS worker is a fake harness: the test calls the API with a live CoS run
//! credential (no LLM, no network).
mod common;

use common::cos_ops::{audit_events, cos_bearer};
use common::*;
use serde_json::{Value, json};
use task_core::chat::triage::CosTriageSource;
use task_core::{Check, Criterion, Event, Status, TaskId, TaskKind, TaskStore, Trigger};
use time::OffsetDateTime;

fn db(env: &TestEnv) -> rusqlite::Connection {
    rusqlite::Connection::open(&env.db_path).expect("open db")
}

/// A ready task judged by a reviewer, failing on its first rejected review.
fn reviewed_task(env: &TestEnv, labels: &[&str], human_check: bool) -> task_core::Task {
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
    task.budget.max_retries = 0;
    task.labels = labels.iter().map(|l| (*l).to_string()).collect();
    env.seed_with(&task, vec![]);
    task
}

/// One attempt: a worker run, then the verdicts that fail the task.
fn fail_attempt(env: &TestEnv, id: TaskId, run: &str, verdicts: &[&str]) {
    env.store
        .apply_transition(
            id,
            Trigger::Dispatch,
            Some(Event::WorkerStarted {
                run_id: run.into(),
                adapter: "fake".into(),
                model: "m".into(),
                provider: None,
                account: None,
                role: None,
                task_role: None,
            }),
        )
        .expect("dispatch");
    env.store
        .apply_transition(id, Trigger::WorkerDone, None)
        .expect("done");
    let verdicts = verdicts
        .iter()
        .map(|reason| Event::ReviewVerdict {
            run_id: run.into(),
            criterion_idx: 0,
            pass: false,
            reason: (*reason).into(),
        })
        .collect();
    env.store
        .apply_transition_with_events(id, Trigger::ReviewFail, verdicts)
        .expect("review fail");
    assert_eq!(env.status_of(id), Status::Failed);
}

/// The failed task's inbox item, ingested as a triage item; returns `(triage item, revision)`.
async fn ingest_failed(env: &TestEnv, app: &axum::Router, id: TaskId, rev: &str) -> String {
    let feed = send(app, get_admin("/api/v1/inbox/items")).await;
    assert_eq!(feed.status.as_u16(), 200, "{}", feed.text());
    let feed = feed.json();
    let items = feed["items"].as_array().expect("items");
    let inbox_id = items
        .iter()
        .find(|x| x["kind"] == "failed" && x["task"]["id"] == id.to_string())
        .and_then(|x| x["id"].as_str())
        .unwrap_or_else(|| panic!("no failed item for {id}: {feed}"))
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

fn rework(reviewer: &str) -> Value {
    json!({"category": "review_rework",
           "refs": [{"kind": "review", "id": reviewer}]})
}

fn reopen(key: &str, rev: &str, basis: Value) -> Value {
    json!({
        "idempotency_key": key,
        "expected_revision": rev,
        "outcome": "answer",
        "reason": "差し戻しは生成物の混入だけで、直し方が書いてある",
        "policy_version": "1",
        "confidence": 0.95,
        "answer": {"option": "reopen", "note": null, "payload": null},
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

/// The reopen went through: the task is ready and the audit names the category.
fn assert_reopened(env: &TestEnv, resp: &Resp, id: TaskId) -> String {
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let out = resp.json();
    assert_eq!(out["operation"]["state"], "applied", "{out}");
    assert_eq!(out["item"]["state"], "answered");
    assert_eq!(env.status_of(id), Status::Ready);
    let op = out["operation"]["id"].as_str().expect("op").to_string();
    let audits = audit_events(env, &op);
    assert_eq!(audits.len(), 1, "{audits:?}");
    assert_eq!(
        audits[0]["reason"],
        "定型の代答（review_rework）: 差し戻しは生成物の混入だけで、直し方が書いてある"
    );
    assert_eq!(audits[0]["target_kind"], "task");
    op
}

/// A refused reopen leaves the task failed and the item pending, recorded as rejected.
fn assert_refused(env: &TestEnv, resp: &Resp, status: u16, code: &str, item: &str, id: TaskId) {
    assert_problem(resp, status, code);
    assert_eq!(env.status_of(id), Status::Failed, "{code}");
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
async fn review_rework_reopens_and_hands_the_rejection_to_the_next_attempt() {
    let env = admin_env();
    let cos = Cos::new(&env, "rework");
    let task = reviewed_task(&env, &[], false);
    fail_attempt(
        &env,
        task.id,
        "run-1",
        &["reviewer(rev-1): 依頼範囲外の生成物 index.json が混入している。消して再提出する"],
    );
    let item = ingest_failed(&env, &cos.app, task.id, "r1").await;

    // Other categories, a stale reviewer and a missing ref are refused.
    let as_default = cos
        .resolve(
            &item,
            reopen(
                "default",
                "r1",
                json!({"category": "documented_default",
                       "refs": [{"kind": "adr", "id": "agent-docs/adr/2026-10-29-cos-review-rework-reopen.md"}]}),
            ),
        )
        .await;
    assert_refused(&env, &as_default, 422, "cos_basis_invalid", &item, task.id);
    let wrong_review = cos
        .resolve(&item, reopen("wrong", "r1", rework("rev-9")))
        .await;
    assert_refused(
        &env,
        &wrong_review,
        422,
        "cos_basis_invalid",
        &item,
        task.id,
    );
    let no_review = cos
        .resolve(
            &item,
            reopen(
                "no-review",
                "r1",
                json!({"category": "review_rework", "refs": [{"kind": "message", "id": "seq 1616"}]}),
            ),
        )
        .await;
    assert_refused(&env, &no_review, 422, "cos_basis_invalid", &item, task.id);

    let ok = cos
        .resolve(&item, reopen("ok", "r1", rework("rev-1")))
        .await;
    let op = assert_reopened(&env, &ok, task.id);

    // The rejection reaches the next attempt as `context.prior_review`.
    let events = env.store.events_for(task.id).expect("events");
    let prior = task_ops::derive::prior_review_from_events(&events);
    assert_eq!(prior.len(), 1, "{prior:?}");
    assert!(!prior[0].pass);
    assert!(prior[0].reason.contains("index.json が混入"), "{prior:?}");

    // The record is a CoS operation card; a person can revoke it.
    let revoke = send(
        &cos.app,
        post_admin(
            &format!("/api/v1/cos/operations/{op}/override"),
            &json!({"action": "revoke", "reason": "人が見直す"}),
        ),
    )
    .await;
    assert_eq!(revoke.status.as_u16(), 200, "{}", revoke.text());
}

#[tokio::test]
async fn review_rework_stops_at_the_limit_and_on_a_repeated_rejection() {
    let env = admin_env();
    let cos = Cos::new(&env, "limit");
    let task = reviewed_task(&env, &[], false);

    fail_attempt(
        &env,
        task.id,
        "run-1",
        &["reviewer(rev-1): index.json が混入"],
    );
    let first = ingest_failed(&env, &cos.app, task.id, "r1").await;
    let ok = cos
        .resolve(&first, reopen("k1", "r1", rework("rev-1")))
        .await;
    assert_reopened(&env, &ok, task.id);

    // The same rejection again (whitespace aside): a person decides.
    fail_attempt(
        &env,
        task.id,
        "run-2",
        &["reviewer(rev-2):  index.json  が混入"],
    );
    let same = ingest_failed(&env, &cos.app, task.id, "r2").await;
    let repeated = cos
        .resolve(&same, reopen("k2", "r2", rework("rev-2")))
        .await;
    assert_refused(
        &env,
        &repeated,
        422,
        "cos_basis_repeated_failure",
        &same,
        task.id,
    );

    // A person reopens it (not counted against CoS); a new rejection is reopened once more by
    // CoS (the second of two).
    env.store
        .apply_transition(task.id, Trigger::Reopen, None)
        .expect("human reopen");
    fail_attempt(
        &env,
        task.id,
        "run-3",
        &["reviewer(rev-3): 試験 x が足りない"],
    );
    let second = ingest_failed(&env, &cos.app, task.id, "r3").await;
    let ok = cos
        .resolve(&second, reopen("k3", "r3", rework("rev-3")))
        .await;
    assert_reopened(&env, &ok, task.id);

    // The third reopen is over the limit even with a new rejection; CoS escalates it.
    fail_attempt(
        &env,
        task.id,
        "run-4",
        &["reviewer(rev-4): fmt が崩れている"],
    );
    let third = ingest_failed(&env, &cos.app, task.id, "r4").await;
    let over = cos
        .resolve(&third, reopen("k4", "r4", rework("rev-4")))
        .await;
    assert_refused(&env, &over, 422, "cos_basis_rework_limit", &third, task.id);
    let escalated = cos
        .resolve(
            &third,
            json!({
                "idempotency_key": "k4-esc",
                "expected_revision": "r4",
                "outcome": "escalate",
                "reason": "自動の開き直しが上限の 2 回に達した",
                "policy_version": "1",
                "confidence": 0.9,
                "escalation": {
                    "summary": "reviewer の差し戻しで 3 回目の失敗。自動の開き直しは上限",
                    "options": [{"key": "reopen", "label": "開き直す", "detail": "同じ task をもう一度実行する"},
                                {"key": "cancel", "label": "諦める", "detail": "task を取り消す"}],
                    "recommended": null,
                    "recommendation_reason": "上限を超えたので人が決める",
                    "web_path": format!("/tasks/{}", task.id)
                }
            }),
        )
        .await;
    assert_eq!(escalated.status.as_u16(), 200, "{}", escalated.text());
    assert_eq!(escalated.json()["item"]["state"], "escalated");
}

#[tokio::test]
async fn review_rework_leaves_human_checks_labels_and_other_failures_to_a_person() {
    let env = admin_env();
    let cos = Cos::new(&env, "excluded");

    let checked = reviewed_task(&env, &[], true);
    fail_attempt(&env, checked.id, "run-1", &["reviewer(rev-1): fix it"]);
    let item = ingest_failed(&env, &cos.app, checked.id, "h1").await;
    let resp = cos.resolve(&item, reopen("h", "h1", rework("rev-1"))).await;
    assert_refused(&env, &resp, 403, "cos_human_required", &item, checked.id);

    let labeled = reviewed_task(&env, &["human_required"], false);
    fail_attempt(&env, labeled.id, "run-1", &["reviewer(rev-1): fix it"]);
    let item = ingest_failed(&env, &cos.app, labeled.id, "l1").await;
    let resp = cos.resolve(&item, reopen("l", "l1", rework("rev-1"))).await;
    assert_refused(&env, &resp, 403, "cos_human_required", &item, labeled.id);

    // A command check or a reviewer infra failure is not a reviewer's technical rejection.
    let command = reviewed_task(&env, &[], false);
    fail_attempt(
        &env,
        command.id,
        "run-1",
        &[
            "command `cargo test` exit 101",
            "reviewer infra failure ×3: rate limited",
        ],
    );
    let item = ingest_failed(&env, &cos.app, command.id, "c1").await;
    let resp = cos.resolve(&item, reopen("c", "c1", rework("rev-1"))).await;
    assert_refused(&env, &resp, 422, "cos_basis_invalid", &item, command.id);
}

#[tokio::test]
async fn review_rework_applies_to_a_direct_reopen_operation() {
    let env = admin_env();
    let cos = Cos::new(&env, "ops");
    let task = reviewed_task(&env, &[], false);
    fail_attempt(
        &env,
        task.id,
        "run-1",
        &["reviewer(rev-1): index.json が混入"],
    );
    let body = |key: &str, basis: Option<Value>| {
        let mut body = json!({
            "idempotency_key": key, "expected_revision": null, "reason": "生成物の混入だけ",
            "policy_version": "1",
            "request": {"method": "POST", "path": format!("/api/v1/tasks/{}/reopen", task.id), "body": {}},
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
    assert_eq!(env.status_of(task.id), Status::Failed);
    let ok = send(
        &cos.app,
        post_json_with(
            "/api/v1/cos/operations",
            &body("ok", Some(rework("rev-1"))),
            &headers,
        ),
    )
    .await;
    assert_eq!(ok.status.as_u16(), 200, "{}", ok.text());
    assert_eq!(ok.json()["operation"]["state"], "applied");
    assert_eq!(env.status_of(task.id), Status::Ready);
}
