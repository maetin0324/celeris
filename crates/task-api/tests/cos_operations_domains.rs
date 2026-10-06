//! ADR 2026-10-05 D3: every registered domain of `/api/v1/cos/operations` (task・comment・decision・
//! approval・execution・project・knowledge) runs its handler's shared operation function with the audit
//! context, and the same mutation called directly with the CoS run credential is 422.
mod common;

use common::*;
use serde_json::{Value, json};
use task_core::approval::{Approval, ApprovalId, ApprovalStore, Decision};
use task_core::chat::{ChatCreateThreadRequest, ChatPostMessageRequest, ChatSendMode};
use task_core::decision::{
    CostOfReversal, DecisionKind, DecisionOption, DecisionOrigin, DecisionPathEntry,
    DecisionRaisedBy, DecisionRequest, DecisionStatus,
};
use task_core::{Event, Status, TaskKind, TaskStore};
use time::{Duration, OffsetDateTime};

const OPS: &str = "/api/v1/cos/operations";

/// A live CoS run of a new thread: `(thread_id, run_id, "Bearer …")`.
fn cos_bearer(env: &TestEnv, key: &str) -> (String, String, String) {
    let store = &env.store;
    let now = OffsetDateTime::now_utc();
    let thread = store
        .chat_thread_create(
            "admin",
            &ChatCreateThreadRequest {
                title: format!("thread {key}"),
                project_id: None,
                client_thread_id: key.into(),
            },
            now,
        )
        .expect("thread")
        .thread;
    store
        .chat_message_post(
            &thread.id,
            &ChatPostMessageRequest {
                client_message_id: format!("message-{key}"),
                text: "input".into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
            },
            now,
        )
        .expect("message");
    let run = format!("run-{key}");
    store
        .chat_run_claim_next(&thread.id, &run, &json!({}), now)
        .expect("claim")
        .expect("run");
    let bearer = task_api::cos::issue_run_bearer(store, &thread.id, &run, Duration::hours(1))
        .expect("issue");
    (thread.id, run, format!("Bearer {bearer}"))
}

fn op_body(key: &str, method: &str, path: &str, body: Value) -> Value {
    json!({
        "idempotency_key": key,
        "expected_revision": null,
        "reason": "人が依頼した",
        "policy_version": "1",
        "request": {"method": method, "path": path, "body": body},
    })
}

fn db(env: &TestEnv) -> rusqlite::Connection {
    rusqlite::Connection::open(&env.db_path).expect("open db")
}

/// One domain: the operation succeeds with `action`, leaves its audit envelope event and its chat
/// card, and the same request sent directly with the CoS credential is 422 without a domain write.
async fn run_domain(
    env: &TestEnv,
    key: &str,
    method: &str,
    path: &str,
    body: Value,
    action: &str,
) -> Value {
    let app = env.router();
    let (thread, run, bearer) = cos_bearer(env, key);
    let headers = [("authorization", bearer.as_str())];

    // Direct call first: the domain state stays untouched, so the operation below still applies.
    let direct = match method {
        "PATCH" => send(&app, patch_json_with(path, &body, &headers)).await,
        _ => send(&app, post_json_with(path, &body, &headers)).await,
    };
    let problem = assert_problem(&direct, 422, "cos_audit_context_required");
    assert_eq!(problem["instead"], OPS, "{key}");

    let resp = send(
        &app,
        post_json_with(OPS, &op_body(key, method, path, body), &headers),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{key}: {}", resp.text());
    let op = resp.json()["operation"].clone();
    assert_eq!(op["state"], "applied", "{key}: {op}");
    assert_eq!(op["action"], action, "{key}");
    assert_eq!(op["actor"], "cos");
    assert_eq!(op["thread_id"], thread.as_str());
    assert_eq!(op["run_id"], run.as_str());
    let op_id = op["id"].as_str().expect("id").to_string();

    // The audit envelope event of this operation.
    let conn = db(env);
    let mut stmt = conn
        .prepare("SELECT json FROM events ORDER BY seq")
        .expect("prepare");
    let audits: Vec<Value> = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query")
        .map(|raw| serde_json::from_str::<Value>(&raw.expect("row")).expect("json"))
        .filter(|e| e["type"] == "cos_operation" && e["operation_id"] == op_id.as_str())
        .collect();
    assert_eq!(audits.len(), 1, "{key}: {audits:?}");
    for (field, want) in [
        ("actor", "cos"),
        ("thread_id", thread.as_str()),
        ("run_id", run.as_str()),
        ("reason", "人が依頼した"),
        ("policy_version", "1"),
        ("state", "applied"),
    ] {
        assert_eq!(audits[0][field], want, "{key}: {field}");
    }
    // The chat card event the operation points at.
    let card: String = conn
        .query_row(
            "SELECT payload_json FROM chat_events WHERE thread_id=?1 AND type='card' AND id=?2",
            rusqlite::params![
                thread,
                op["event_id"]
                    .as_str()
                    .expect("event id")
                    .parse::<i64>()
                    .expect("int")
            ],
            |row| row.get(0),
        )
        .expect("card event");
    assert!(card.contains(&op_id), "{key}: {card}");
    let rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cos_operations WHERE id=?1 AND state='applied'",
            [&op_id],
            |row| row.get(0),
        )
        .expect("row");
    assert_eq!(rows, 1, "{key}");
    op
}

fn raise_decision(env: &TestEnv, task: &task_core::Task, id: &str) {
    let opt = |key: &str| DecisionOption {
        key: key.into(),
        label: key.into(),
        consequence: None,
    };
    env.store
        .append_event(
            task.id,
            &Event::DecisionRequested {
                decision: Box::new(DecisionRequest {
                    id: id.into(),
                    key: "h1".into(),
                    kind: DecisionKind::Choice,
                    question: "which?".into(),
                    options: vec![opt("vault"), opt("manual")],
                    recommended: "vault".into(),
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

#[tokio::test]
async fn cos_chat_ops_domain_task_create() {
    let env = admin_env();
    let op = run_domain(
        &env,
        "task",
        "POST",
        "/api/v1/tasks",
        json!({"title": "画面修正", "objective": "依頼の全文", "acceptance": [{"type": "artifact_exists", "name": "result.md"}]}),
        "task.create",
    )
    .await;
    let id = op["result"]["task_id"].as_str().expect("task id");
    let task = env
        .store
        .get(id.parse().expect("ulid"))
        .expect("get")
        .expect("task");
    assert_eq!(task.title, "画面修正");
}

#[tokio::test]
async fn cos_chat_ops_domain_comment_create() {
    let env = admin_env();
    let task = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&task);
    run_domain(
        &env,
        "comment",
        "POST",
        &format!("/api/v1/tasks/{}/comments", task.id),
        json!({"body": "補足"}),
        "comment.create",
    )
    .await;
    let comments = env.store.comments_for(task.id).expect("comments");
    assert_eq!(comments.len(), 1, "the direct call wrote no comment");
    assert_eq!(comments[0].author.as_deref(), Some("cos"));
}

#[tokio::test]
async fn cos_chat_ops_domain_decision_answer() {
    let env = admin_env();
    let task = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&task);
    raise_decision(&env, &task, "dec-d");
    run_domain(
        &env,
        "decision",
        "POST",
        "/api/v1/decisions/dec-d/answer",
        json!({"option": "vault"}),
        "decision.answer",
    )
    .await;
    let events = env.store.events_for(task.id).expect("events");
    let answered: Vec<_> = events
        .iter()
        .filter(|(_, e)| matches!(e, Event::DecisionAnswered { by, .. } if by == "cos"))
        .collect();
    assert_eq!(answered.len(), 1);
}

#[tokio::test]
async fn cos_chat_ops_domain_approval_decide() {
    let env = admin_env();
    let now = OffsetDateTime::now_utc();
    let approval = Approval {
        id: ApprovalId::new(),
        project_id: None,
        node_id: "engineering".into(),
        task_id: None,
        question: "どのクラスタを使いますか".into(),
        decision: None,
        answer: None,
        created_at: now,
        decided_at: None,
    };
    env.store.approval_append(&approval).expect("append");
    let op = run_domain(
        &env,
        "approval",
        "POST",
        &format!("/api/v1/approvals/{}/decide", approval.id),
        json!({"decision": "standing", "answer": "sirius を使う", "scope": "node"}),
        "approval.decide",
    )
    .await;
    assert_eq!(op["target_kind"], "approval");
    let decided = env
        .store
        .approval_get(approval.id)
        .expect("get")
        .expect("approval");
    assert_eq!(decided.decision, Some(Decision::Standing));
    assert_eq!(decided.answer.as_deref(), Some("sirius を使う"));
    // The standing rule is written in the same transaction.
    let rules = env
        .store
        .standing_rule_list(Some("engineering"))
        .expect("rules");
    assert_eq!(rules.len(), 1);
    assert_eq!(
        op["result"]["standing_rule_id"],
        rules[0].id.to_string().as_str()
    );
}

#[tokio::test]
async fn cos_chat_ops_domain_execution_phase_gate() {
    let env = admin_env();
    let task = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&task);
    env.store
        .apply_transition(task.id, task_core::Trigger::Dispatch, None)
        .expect("dispatch");
    env.store
        .apply_transition_with_events(
            task.id,
            task_core::Trigger::PhaseGate {
                phase: "design".into(),
            },
            vec![Event::PhaseReported {
                phase: "design".into(),
                report: Box::new(task_core::PhaseReport {
                    phase: "design".into(),
                    phase_title: "設計".into(),
                    next_phase: Some("build".into()),
                    ..Default::default()
                }),
            }],
        )
        .expect("phase gate");
    let op = run_domain(
        &env,
        "execution",
        "POST",
        &format!("/api/v1/tasks/{}/execution/phase-gate", task.id),
        json!({"action": "continue", "note": "build は小さく"}),
        "execution.phase_gate",
    )
    .await;
    assert_eq!(op["result"]["to"], "ready", "{op}");
    let task_now = env.store.get(task.id).expect("get").expect("task");
    assert_eq!(task_now.status, Status::Ready);
    let events = env.store.events_for(task.id).expect("events");
    assert!(events.iter().any(|(_, e)| matches!(
        e,
        Event::Answered { answer, .. } if answer == "build は小さく"
    )));

    // withdraw cascades a cancellation, so CoS is refused (recorded as rejected).
    let other = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&other);
    let app = env.router();
    let (_, _, bearer) = cos_bearer(&env, "execution-withdraw");
    let resp = send(
        &app,
        post_json_with(
            OPS,
            &op_body(
                "withdraw",
                "POST",
                &format!("/api/v1/tasks/{}/execution/phase-gate", other.id),
                json!({"action": "withdraw"}),
            ),
            &[("authorization", bearer.as_str())],
        ),
    )
    .await;
    assert_eq!(
        resp.status.as_u16(),
        409,
        "not awaiting_human: {}",
        resp.text()
    );
}

#[tokio::test]
async fn cos_chat_ops_domain_project_update() {
    let env = admin_env();
    let app = env.router();
    let created = send(
        &app,
        post_admin(
            "/api/v1/projects",
            &json!({"title": "旧い名前", "request": "旧い説明"}),
        ),
    )
    .await;
    assert_eq!(created.status.as_u16(), 201, "{}", created.text());
    let id = created.json()["id"].as_str().expect("id").to_string();
    let op = run_domain(
        &env,
        "project",
        "PATCH",
        &format!("/api/v1/projects/{id}"),
        json!({"title": "新しい名前"}),
        "project.update",
    )
    .await;
    assert_eq!(op["result"]["old_title"], "旧い名前");
    let project = env
        .store
        .project_get(id.parse().expect("project id"))
        .expect("get")
        .expect("project");
    assert_eq!(project.title, "新しい名前");
    assert_eq!(project.request, "旧い説明");

    // CoS may change only title / request.
    let (_, _, bearer) = cos_bearer(&env, "project-status");
    let resp = send(
        &app,
        post_json_with(
            OPS,
            &op_body(
                "status",
                "PATCH",
                &format!("/api/v1/projects/{id}"),
                json!({"status": "done"}),
            ),
            &[("authorization", bearer.as_str())],
        ),
    )
    .await;
    assert_problem(&resp, 422, "validation");
}

#[tokio::test]
async fn cos_chat_ops_domain_knowledge_reject() {
    let env = admin_env();
    task_ops::knowledge::init(&env.knowledge_root).expect("knowledge init");
    let candidate = task_ops::knowledge::record(
        &env.knowledge_root,
        &task_ops::knowledge::RecordRequest {
            title: "fern03 の使い方".into(),
            scope: "environment".into(),
            tags: vec![],
            sources: vec!["task:01J1".into()],
            confidence: None,
            body: "ssh fern03 で入る。".into(),
            path: Some("environment/servers/fern03.md".into()),
            op: None,
        },
    )
    .expect("record");
    let op = run_domain(
        &env,
        "knowledge",
        "POST",
        &format!("/api/v1/knowledge/inbox/{}/reject", candidate.id),
        json!({}),
        "knowledge.reject",
    )
    .await;
    assert_eq!(op["target_kind"], "knowledge");
    assert!(op["result"]["sha"].as_str().is_some_and(|s| !s.is_empty()));
    // The candidate is gone; rejecting again is a 404 recorded as a failed operation.
    let app = env.router();
    let (_, _, bearer) = cos_bearer(&env, "knowledge-again");
    let again = send(
        &app,
        post_json_with(
            OPS,
            &op_body(
                "again",
                "POST",
                &format!("/api/v1/knowledge/inbox/{}/reject", candidate.id),
                json!({}),
            ),
            &[("authorization", bearer.as_str())],
        ),
    )
    .await;
    assert_problem(&again, 404, "candidate_not_found");
}
