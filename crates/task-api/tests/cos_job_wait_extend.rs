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
use task_api::{ApiToken, Principal};
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
        scheduling: Vec::new(),
        queued_since: None,
        queue_notified: false,
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
    events: Vec<Event>,
    text: &str,
    labels: &[&str],
) -> (task_core::Task, ApprovalId) {
    authorization_in(env, None, events, text, labels)
}

/// [`authorization`] for a task of `project` (a random project when `None`).
fn authorization_in(
    env: &TestEnv,
    project: Option<task_core::ProjectId>,
    mut events: Vec<Event>,
    text: &str,
    labels: &[&str],
) -> (task_core::Task, ApprovalId) {
    let mut task = new_task(TaskKind::Execute, Status::Blocked);
    if project.is_some() {
        task.project_id = project;
    }
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
        policy_version: "10".into(),
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

const WEB_TOKEN: &str = "web-token-standing-grant";

/// 不変条件（ADR 2026-11-11-cos-standing-context D4）: 人が統括 thread で期限も task も限らずに任せた指示は、
/// CoS の提案 → 人の受け入れで global の grant になり、その grant に裏付けられた取り次ぎは task と期限なしで通り、
/// 案件 CoS は 3 回の上限を超えた job 待ちの件を grant の id を根拠に人へ回さず答え、監査には判断の根拠（grant）と
/// 実行した CoS が別々に残る。既存の試験は取り次ぎに task と期限を必ず求め、4 回目の上限を必ず人に回す前提で、
/// 常設の委任の経路（global の範囲・grant_id 付きの取り次ぎ・上限の解除・監査）を通らないので、この試験で固定する。
#[tokio::test]
async fn standing_job_wait_grant_from_the_general_thread_answers_the_project_wait_limit() {
    let env = TestEnv::with(EnvOptions {
        extra_tokens: vec![ApiToken::new(
            Principal::Human { name: "web".into() },
            WEB_TOKEN,
        )],
        ..EnvOptions::default()
    });
    let app = env.router();
    let human = format!("Bearer {WEB_TOKEN}");
    let as_human = [("authorization", human.as_str())];
    let now = OffsetDateTime::now_utc();

    // 1. 人が統括 thread で「job待ちは常に延長」と書く。
    let general = env
        .store
        .cos_thread_open(
            &task_core::chat::ChatOpenThreadRequest { project_id: None },
            now,
        )
        .expect("general")
        .thread;
    let said = send(
        &app,
        post_json_with(
            &format!("/api/v1/chat/threads/{}/messages", general.id),
            &json!({"client_message_id": "h-standing", "text": "job待ちは常に延長してください",
                    "attachment_ids": [], "reply_to_id": null, "mode": "queue", "resume_queue": false}),
            &as_human,
        ),
    )
    .await;
    assert_eq!(said.status.as_u16(), 202, "{}", said.text());
    let said = said.json()["message"]["id"]
        .as_str()
        .expect("id")
        .to_owned();
    env.store
        .chat_run_claim_next(&general.id, "run-general", &json!({}), now)
        .expect("claim")
        .expect("run");
    let general_bearer = format!(
        "Bearer {}",
        task_api::cos::issue_run_bearer(
            &env.store,
            &general.id,
            "run-general",
            time::Duration::hours(1)
        )
        .expect("issue")
    );
    env.store
        .chat_run_record_policy("run-general", "1", "test")
        .expect("policy");
    let general_op = |key: &str, path: String, body: Value| {
        let op = json!({"idempotency_key": key, "expected_revision": null,
                        "reason": "人の常設の指示", "policy_version": "1",
                        "request": {"method": "POST", "path": path, "body": body}});
        let app = app.clone();
        let bearer = general_bearer.clone();
        async move {
            send(
                &app,
                post_json_with(
                    "/api/v1/cos/operations",
                    &op,
                    &[("authorization", bearer.as_str())],
                ),
            )
            .await
        }
    };
    let expires = (now + time::Duration::days(3650))
        .format(&time::format_description::well_known::Rfc3339)
        .expect("format");
    let grant_body = json!({"scope": {"kind": "global", "ref": "*"}, "action_class": "job_wait_extend",
        "source": {"kind": "human_message", "ref": said}, "expires_at": expires, "budget": 1000,
        "note": "人の発言のとおり、cluster job の待ちの上限は常に延長する"});

    // 2. 統括 CoS が grant を提案する（global の台帳）。
    let mut proposal = grant_body.clone();
    proposal["question_key"] = json!("job-wait-always-extend");
    let proposed = general_op(
        "propose",
        "/api/v1/projects/global/authority/proposals".into(),
        proposal,
    )
    .await;
    assert_eq!(proposed.status.as_u16(), 200, "{}", proposed.text());
    assert_eq!(
        proposed.json()["operation"]["action"],
        "authority.grant_propose"
    );
    let proposal_id = proposed.json()["operation"]["result"]["proposal"]["id"]
        .as_str()
        .expect("proposal id")
        .to_owned();
    // global の範囲は定型の代答だけ（本番の変更などは案件の範囲で人が与える）。
    let mut production = grant_body.clone();
    production["action_class"] = json!("production");
    let refused = send(
        &app,
        post_json_with(
            "/api/v1/projects/global/authority/grants",
            &production,
            &as_human,
        ),
    )
    .await;
    assert_problem(&refused, 422, "authority_grant_invalid");

    // 3. 人が受け入れる（grant になる）。
    let mut accept = grant_body.clone();
    accept["proposal_id"] = json!(proposal_id);
    let granted = send(
        &app,
        post_json_with(
            "/api/v1/projects/global/authority/grants",
            &accept,
            &as_human,
        ),
    )
    .await;
    assert_eq!(granted.status.as_u16(), 201, "{}", granted.text());
    let grant = granted.json()["id"].as_str().expect("grant").to_owned();
    assert_eq!(granted.json()["project_id"], "*");
    assert_eq!(granted.json()["state"], "live");

    // 4. grant に裏付けられた取り次ぎは task と期限なしで通る（裏付けが無ければ従来どおり断る）。
    let (project_thread, _, project_bearer) = cos_ops::cos_bearer(&env, "standing");
    let project = env
        .store
        .chat_thread_get(&project_thread)
        .expect("thread")
        .expect("thread")
        .project_id
        .expect("project");
    let forwards = format!("/api/v1/chat/threads/{project_thread}/forwards");
    let bare = general_op(
        "forward-bare",
        forwards.clone(),
        json!({"client_message_id": "forward:bare", "message_id": said}),
    )
    .await;
    assert_eq!(bare.status.as_u16(), 422, "{}", bare.text());
    let forwarded = general_op(
        "forward-grant",
        forwards,
        json!({"client_message_id": "forward:grant", "message_id": said, "grant_id": grant}),
    )
    .await;
    assert_eq!(forwarded.status.as_u16(), 200, "{}", forwarded.text());
    let relay: Value = db(&env)
        .query_row(
            "SELECT metadata_json FROM chat_messages WHERE thread_id=?1 AND metadata_json LIKE '%grant_id%'",
            [project_thread.as_str()],
            |r| r.get::<_, String>(0),
        )
        .map(|m| serde_json::from_str::<Value>(&m).expect("json")["relay"].clone())
        .expect("relayed message");
    assert_eq!(relay["grant_id"], grant.as_str());
    assert_eq!(relay["source"]["message_id"], said.as_str());
    assert!(relay["scope"].get("task_ids").is_none(), "{relay}");
    assert!(relay["scope"].get("expires_at").is_none(), "{relay}");

    // 5. 案件 CoS が 4 回目の job 待ちの上限を grant の id を根拠に答え、人に回さない。
    let project_id: task_core::ProjectId = project.parse().expect("project id");
    let mut events = Vec::new();
    for w in ["s1", "s2", "s3", "s4"] {
        events.extend(timed_out_wait(w, &["48196"], ClusterJobState::Queued));
    }
    let (task, approval) = authorization_in(&env, Some(project_id), events, LIMIT_TEXT, &[]);
    let item = ingest(&env, approval);
    let cos = Cos {
        app: app.clone(),
        bearer: project_bearer,
    };
    let without = cos
        .resolve(
            &item,
            answer("no-grant", "once", "延長", extend_basis(approval)),
        )
        .await;
    assert_refused(
        &env,
        &without,
        422,
        "cos_basis_extend_limit",
        &item,
        approval,
    );
    let mut basis = extend_basis(approval);
    basis["refs"]
        .as_array_mut()
        .expect("refs")
        .push(json!({"kind": "authority_grant", "id": grant}));
    let ok = cos
        .resolve(&item, answer("with-grant", "once", "延長", basis))
        .await;
    assert_eq!(ok.status.as_u16(), 200, "{}", ok.text());
    assert_eq!(ok.json()["item"]["state"], "answered");
    let decided = env
        .store
        .approval_get(approval)
        .expect("get")
        .expect("approval");
    assert_eq!(decided.decision, Some(Decision::Once));
    assert_eq!(env.status_of(task.id), Status::Ready);

    // 判断の根拠（grant と人の発言）と実行者（案件 CoS）が別々に残る。
    let (grant_id, decision_ref, actor, outcome, audit_project): (
        Option<String>,
        Option<String>,
        String,
        String,
        String,
    ) = db(&env)
        .query_row(
            "SELECT grant_id, human_decision_ref, actual_actor, outcome_state, project_id
             FROM authority_audit WHERE action_class = 'job_wait_extend'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .expect("one audit row");
    assert_eq!(grant_id.as_deref(), Some(grant.as_str()));
    assert_eq!(decision_ref, Some(format!("authority_grant:{grant}")));
    assert!(actor.contains(&project_thread), "{actor}");
    assert_eq!(outcome, "applied");
    assert_eq!(audit_project, project);
    let granted = env
        .store
        .authority_grant(&grant)
        .expect("read")
        .expect("grant");
    assert_eq!(granted.source.reference, said);
    assert_eq!(granted.consumed, 1);

    // 人が取り消すと、取り次ぎの裏付けも消える。
    let backs = |env: &TestEnv| {
        let row = env.store.authority_grant(&grant).expect("read");
        task_core::authority::forward_gap(
            Some(row),
            true,
            Some(&project),
            OffsetDateTime::now_utc(),
        )
    };
    assert_eq!(backs(&env), Ok(()));
    let revoked = send(
        &app,
        post_json_with(
            &format!("/api/v1/projects/global/authority/grants/{grant}/revoke"),
            &json!({"reason": "やめる"}),
            &as_human,
        ),
    )
    .await;
    assert_eq!(revoked.status.as_u16(), 200, "{}", revoked.text());
    assert_eq!(backs(&env).err().as_deref(), Some("grant_revoked"));
}
