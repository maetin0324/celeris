//! ADR 2026-10-30-cos-job-wait-extend: CoS answers the authorization of a cluster job wait limit
//! with "extend" (routine category (g) `job_wait_extend`) while the jobs are still queued, up to the
//! limit, and everything else (cancelling the jobs, withdrawing, a standing rule, jobs not queued,
//! a question that is not a wait limit, a human_required label) is refused so CoS escalates. The CoS
//! worker is a fake harness: the test calls the API with a live CoS run credential (no LLM, no
//! network). Waits and approvals are fixtures written through the store like the dispatcher's
//! timeout (`time_out_cluster_job_wait`).
mod common;

use common::cos_ops::{audit_events, cos_bearer};
use common::*;
use serde_json::{Value, json};
use task_core::approval::{Approval, ApprovalId, ApprovalStore, Decision};
use task_core::chat::triage::CosTriageSource;
use task_core::cluster_job::{
    ClusterJobState, ClusterJobStatus, ClusterJobWait, ClusterJobWaitState, ClusterScheduler,
};
use task_core::{Event, Status, TaskKind};
use time::OffsetDateTime;

fn db(env: &TestEnv) -> rusqlite::Connection {
    rusqlite::Connection::open(&env.db_path).expect("open db")
}

fn status(job: &str, state: ClusterJobState) -> ClusterJobStatus {
    ClusterJobStatus {
        job_id: job.into(),
        state,
        exit_status: None,
        raw_state: Some("Q".into()),
    }
}

/// One wait of `jobs` that timed out with the jobs in `state`.
fn timed_out_wait(wait_id: &str, jobs: &[&str], state: ClusterJobState) -> Vec<Event> {
    let last: Vec<ClusterJobStatus> = jobs.iter().map(|j| status(j, state)).collect();
    vec![
        Event::ClusterJobWaitStarted {
            wait: Box::new(ClusterJobWait {
                wait_id: wait_id.into(),
                task_id: task_core::TaskId::new(),
                work_unit_id: None,
                run_id: "run-1".into(),
                cluster: "sirius".into(),
                scheduler: ClusterScheduler::Pbs,
                jobs: jobs.iter().map(|j| (*j).to_string()).collect(),
                poll_secs: 300,
                timeout_secs: 28800,
                summary: "BenchFS の測定 job".into(),
                checkpoint: None,
                created_at: "2026-10-29T07:12:00Z".into(),
                deadline: "2026-10-29T15:12:00Z".into(),
                last_polled_at: None,
                finished_at: None,
                state: ClusterJobWaitState::Waiting,
                last_status: Vec::new(),
            }),
        },
        Event::ClusterJobWaitPolled {
            wait_id: wait_id.into(),
            jobs: last.clone(),
        },
        Event::ClusterJobWaitFinished {
            wait_id: wait_id.into(),
            state: ClusterJobWaitState::TimedOut,
            jobs: last,
            detail: "the deadline 2026-10-29T15:12:00Z passed".into(),
        },
    ]
}

/// A blocked task whose last event is the question `text`, with the pending approval for it.
/// Returns `(task, approval id)`.
fn authorization(
    env: &TestEnv,
    mut events: Vec<Event>,
    text: &str,
    labels: &[&str],
) -> (task_core::Task, ApprovalId) {
    let mut task = new_task(TaskKind::Execute, Status::Blocked);
    task.labels = labels.iter().map(|l| (*l).to_string()).collect();
    events.push(Event::QuestionRaised {
        run_id: "run-1".into(),
        text: text.into(),
    });
    env.seed_with(&task, events);
    let approval = Approval {
        id: ApprovalId::new(),
        project_id: task.project_id,
        node_id: "cluster-hpc".into(),
        task_id: Some(task.id),
        question: text.into(),
        decision: None,
        answer: None,
        created_at: OffsetDateTime::now_utc(),
        decided_at: None,
    };
    env.store.approval_append(&approval).expect("approval");
    (task, approval.id)
}

const LIMIT_TEXT: &str = "クラスタ `sirius` の job の待ちが上限（28800 秒）に達しました: 48196 Q。\
    延長する（job を待ち直す）／job を取り消して続ける／取り下げる（この task を中止）のどれにするか答えてください";

/// Ingest the authorization as a triage item; returns the item row id.
fn ingest(env: &TestEnv, approval: ApprovalId) -> String {
    let inbox_id = format!("authorization-{approval}");
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
        .cos_triage_ingest_batch("events", &inbox_id, &[source], OffsetDateTime::now_utc())
        .expect("ingest");
    db(env)
        .query_row(
            "SELECT id FROM cos_inbox_items WHERE source_key=?1",
            [inbox_id.as_str()],
            |r| r.get(0),
        )
        .expect("ingested row")
}

fn extend_basis(approval: ApprovalId) -> Value {
    json!({"category": "job_wait_extend",
           "refs": [{"kind": "approval", "id": approval.to_string()},
                    {"kind": "message", "id": "GET /api/v1/tasks/x/events 2026-10-29T15:20Z"}]})
}

fn answer(key: &str, option: &str, note: &str, basis: Value) -> Value {
    json!({
        "idempotency_key": key,
        "expected_revision": "r1",
        "outcome": "answer",
        "reason": "job は Q のままで、待つだけでは node-h を使わない",
        "policy_version": "1",
        "confidence": 0.95,
        "answer": {"option": option, "note": note, "payload": null},
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

/// A refused answer keeps the approval pending and the item pending, recorded as rejected.
fn assert_refused(env: &TestEnv, resp: &Resp, status: u16, code: &str, item: &str, id: ApprovalId) {
    assert_problem(resp, status, code);
    let approval = env.store.approval_get(id).expect("get").expect("approval");
    assert_eq!(approval.decision, None, "{code}");
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
async fn job_wait_extend_answers_once_extend_for_queued_jobs() {
    let env = admin_env();
    let cos = Cos::new(&env, "extend");
    let (task, approval) = authorization(
        &env,
        timed_out_wait("w1", &["48196"], ClusterJobState::Queued),
        LIMIT_TEXT,
        &[],
    );
    let item = ingest(&env, approval);

    // Only the extension: cancelling, withdrawing, a standing rule or another note go to a person.
    for (key, option, note) in [
        ("denied", "denied", "取り下げ"),
        ("standing", "standing", "延長"),
        ("cancel", "once", "job を取り消して続ける"),
        ("resize", "once", "延長して台数を 4 に減らす"),
    ] {
        let resp = cos
            .resolve(&item, answer(key, option, note, extend_basis(approval)))
            .await;
        assert_refused(&env, &resp, 422, "cos_basis_invalid", &item, approval);
    }
    // The basis names the approval it decides.
    let no_ref = json!({"category": "job_wait_extend",
                        "refs": [{"kind": "message", "id": "events"}]});
    let resp = cos
        .resolve(&item, answer("no-ref", "once", "延長", no_ref))
        .await;
    assert_refused(&env, &resp, 422, "cos_basis_invalid", &item, approval);
    // Another category cannot carry the extension without its frame.
    let resp = cos
        .resolve(
            &item,
            answer(
                "frame",
                "once",
                "延長",
                json!({"category": "within_approved_frame",
                       "refs": [{"kind": "approval", "id": approval.to_string()}]}),
            ),
        )
        .await;
    assert_refused(&env, &resp, 422, "cos_basis_invalid", &item, approval);

    let ok = cos
        .resolve(
            &item,
            answer("ok", "once", " 延長 ", extend_basis(approval)),
        )
        .await;
    assert_eq!(ok.status.as_u16(), 200, "{}", ok.text());
    let out = ok.json();
    assert_eq!(out["operation"]["state"], "applied", "{out}");
    assert_eq!(out["item"]["state"], "answered");
    let decided = env
        .store
        .approval_get(approval)
        .expect("get")
        .expect("approval");
    assert_eq!(decided.decision, Some(Decision::Once));
    assert_eq!(decided.answer.as_deref().map(str::trim), Some("延長"));
    assert_eq!(env.status_of(task.id), Status::Ready);
    let op = out["operation"]["id"].as_str().expect("op");
    let audits = audit_events(&env, op);
    assert_eq!(audits.len(), 1, "{audits:?}");
    assert_eq!(
        audits[0]["reason"],
        "定型の代答（job_wait_extend）: job は Q のままで、待つだけでは node-h を使わない"
    );
}

#[tokio::test]
async fn job_wait_extend_refuses_jobs_not_queued_other_questions_and_the_limit() {
    let env = admin_env();
    let cos = Cos::new(&env, "refuse");

    // A job already running (or held) is not a plain wait: a person decides.
    for (key, state) in [
        ("running", ClusterJobState::Running),
        ("held", ClusterJobState::Held),
    ] {
        let (_, approval) = authorization(&env, timed_out_wait(key, &["1", "2"], state), key, &[]);
        let item = ingest(&env, approval);
        let resp = cos
            .resolve(&item, answer(key, "once", "延長", extend_basis(approval)))
            .await;
        assert_refused(
            &env,
            &resp,
            422,
            "cos_basis_job_not_queued",
            &item,
            approval,
        );
    }

    // An authorization that no wait limit raised.
    let (_, approval) = authorization(&env, vec![], "本番 DB に書いてよいですか", &[]);
    let item = ingest(&env, approval);
    let resp = cos
        .resolve(
            &item,
            answer("plain", "once", "延長", extend_basis(approval)),
        )
        .await;
    assert_refused(&env, &resp, 422, "cos_basis_invalid", &item, approval);

    // The fourth wait limit of the same job goes to a person; another job's timeout does not count.
    let mut events = timed_out_wait("other", &["9"], ClusterJobState::Queued);
    for w in ["l1", "l2", "l3"] {
        events.extend(timed_out_wait(w, &["48196"], ClusterJobState::Queued));
    }
    let (_, third) = authorization(&env, events.clone(), "third", &[]);
    let item = ingest(&env, third);
    let resp = cos
        .resolve(&item, answer("third", "once", "延長", extend_basis(third)))
        .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    events.extend(timed_out_wait("l4", &["48196"], ClusterJobState::Queued));
    let (_, fourth) = authorization(&env, events, "fourth", &[]);
    let item = ingest(&env, fourth);
    let resp = cos
        .resolve(
            &item,
            answer("fourth", "once", "延長", extend_basis(fourth)),
        )
        .await;
    assert_refused(&env, &resp, 422, "cos_basis_extend_limit", &item, fourth);

    // An explicit human_required label is 403 before the basis is checked.
    let (_, approval) = authorization(
        &env,
        timed_out_wait("human", &["5"], ClusterJobState::Queued),
        "human",
        &["human_required"],
    );
    let item = ingest(&env, approval);
    let resp = cos
        .resolve(
            &item,
            answer("human", "once", "延長", extend_basis(approval)),
        )
        .await;
    assert_refused(&env, &resp, 403, "cos_human_required", &item, approval);
}

/// `/cos/operations` `approval.decide` without `instructed_by` needs the same basis; the extension
/// text is the approval's `answer`.
#[tokio::test]
async fn job_wait_extend_through_operations_reads_the_approval_answer() {
    let env = admin_env();
    let cos = Cos::new(&env, "ops");
    let (task, approval) = authorization(
        &env,
        timed_out_wait("w1", &["48196"], ClusterJobState::Queued),
        LIMIT_TEXT,
        &[],
    );
    let headers = [("authorization", cos.bearer.as_str())];
    let body = |key: &str, text: &str, basis: Option<Value>| {
        let mut body = json!({
            "idempotency_key": key, "expected_revision": null,
            "reason": "job は Q のまま", "policy_version": "1",
            "request": {"method": "POST", "path": format!("/api/v1/approvals/{approval}/decide"),
                        "body": {"decision": "once", "answer": text}},
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
            &body("bare", "延長", None),
            &headers,
        ),
    )
    .await;
    assert_problem(&missing, 422, "cos_basis_missing");
    let cancel = send(
        &cos.app,
        post_json_with(
            "/api/v1/cos/operations",
            &body("cancel", "取り消して続ける", Some(extend_basis(approval))),
            &headers,
        ),
    )
    .await;
    assert_problem(&cancel, 422, "cos_basis_invalid");
    let ok = send(
        &cos.app,
        post_json_with(
            "/api/v1/cos/operations",
            &body("ok", "延長", Some(extend_basis(approval))),
            &headers,
        ),
    )
    .await;
    assert_eq!(ok.status.as_u16(), 200, "{}", ok.text());
    let decided = env
        .store
        .approval_get(approval)
        .expect("get")
        .expect("approval");
    assert_eq!(decided.decision, Some(Decision::Once));
    assert_eq!(env.status_of(task.id), Status::Ready);
}
