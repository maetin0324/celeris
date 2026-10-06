//! Human correction of CoS answers: no network or timer waits.
mod common;
use common::*;
use serde_json::{Value, json};
use task_core::chat::{
    AuditContext, ChatActor, ChatCreateThreadRequest, ChatPostMessageRequest, ChatSendMode,
};
use task_core::decision::{
    CostOfReversal, DecisionKind, DecisionOption, DecisionOrigin, DecisionPathEntry,
    DecisionRaisedBy, DecisionRequest, DecisionStatus,
};
use task_core::execution_plan::{WorkUnitBlockedReason, WorkUnitRow, WorkUnitSpec, WorkUnitStatus};
use task_core::tree::{ParentUnit, TreeInfo};
use task_core::{Event, Status, TaskKind, TaskStore};
use time::{Duration, OffsetDateTime};
use ulid::Ulid;

fn decision_fixture(env: &TestEnv, action: &str) -> (String, String, String) {
    let task = new_task(TaskKind::Execute, Status::Ready);
    let task_id = task.id.to_string();
    env.seed(&task);
    let decision_id = Ulid::new().to_string();
    let request = DecisionRequest {
        id: decision_id.clone(),
        key: "choice".into(),
        kind: DecisionKind::Choice,
        question: "Choose".into(),
        options: vec![
            DecisionOption {
                key: "yes".into(),
                label: "Yes".into(),
                consequence: None,
            },
            DecisionOption {
                key: "no".into(),
                label: "No".into(),
                consequence: None,
            },
        ],
        recommended: "yes".into(),
        cost_of_reversal: CostOfReversal::Low,
        cost_note: None,
        needed_before: vec!["unit".into()],
        path: vec![DecisionPathEntry {
            task_id: task.id,
            title: task.title.clone(),
            stage: None,
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
                decision: Box::new(request),
            },
        )
        .expect("request");
    let op_id = Ulid::new().to_string();
    let ctx = AuditContext {
        actor: ChatActor::Cos,
        thread_id: "thread".into(),
        run_id: "cos-run".into(),
        operation_id: op_id.clone(),
        reason: "routine answer".into(),
        policy_version: "1".into(),
    };
    let op = env
        .store
        .cos_operation_apply(
            &ctx,
            "key",
            "hash",
            "decision",
            &decision_id,
            Some("r1"),
            action,
            &json!({}),
            |tx, _| {
                if action == "decision.answer" {
                    let applied = task_core::SqliteStore::decision_resolve_apply_tx(
                        tx,
                        task.id,
                        &decision_id,
                        DecisionStatus::Open,
                        vec![],
                        vec![Event::DecisionAnswered {
                            id: decision_id.clone(),
                            option: "yes".into(),
                            note: None,
                            by: "cos".into(),
                        }],
                    )?;
                    assert!(applied);
                }
                Ok(json!({"ok":true}))
            },
        )
        .expect("operation");
    assert_eq!(op.id, op_id);
    (task_id, decision_id, op_id)
}

fn live_cos_bearer(env: &TestEnv, key: &str) -> String {
    let now = OffsetDateTime::now_utc();
    let thread = env
        .store
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
    env.store
        .chat_message_post(
            &thread.id,
            &ChatPostMessageRequest {
                client_message_id: format!("message-{key}"),
                text: "go".into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
            },
            now,
        )
        .expect("message");
    let run = format!("run-{key}");
    env.store
        .chat_run_claim_next(&thread.id, &run, &json!({}), now)
        .expect("claim")
        .expect("run");
    format!(
        "Bearer {}",
        task_api::cos::issue_run_bearer(&env.store, &thread.id, &run, Duration::hours(1))
            .expect("bearer")
    )
}

fn conn(env: &TestEnv) -> rusqlite::Connection {
    rusqlite::Connection::open(&env.db_path).expect("db")
}
fn override_path(id: &str) -> String {
    format!("/api/v1/cos/operations/{id}/override")
}
fn payload(action: &str) -> Value {
    json!({"action":action,"reason":"The human corrected the choice"})
}
fn events(env: &TestEnv, task: &str) -> Vec<Value> {
    let db = conn(env);
    let mut stmt = db
        .prepare("SELECT json FROM events WHERE task_id=?1 ORDER BY seq")
        .expect("prepare");
    stmt.query_map([task], |r| r.get::<_, String>(0))
        .expect("query")
        .map(|r| serde_json::from_str(&r.expect("row")).expect("event"))
        .collect()
}

#[tokio::test]
async fn cos_chat_triage_override_revoke_reopens_unconsumed_decision_and_preserves_audit() {
    let env = admin_env();
    let app = env.router();
    let (task, old, op) = decision_fixture(&env, "decision.answer");
    let db = conn(&env);
    db.execute("INSERT INTO cos_inbox_items(id,source_kind,source_key,source_revision,thread_id,message_id,state,policy_version,operation_id,created_at,updated_at) VALUES('old-item','decision',?1,'r1','thread','','answered','1',?2,'2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",[format!("decision-{old}"),op.clone()]).expect("item");
    let before = events(&env, &task);
    let resp = send(&app, post_admin(&override_path(&op), &payload("revoke"))).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let body = resp.json();
    assert_eq!(body["state"], "superseded");
    let new_id = body["new_wait_id"].as_str().expect("new wait");
    assert_ne!(new_id, old);
    let new_items:i64=conn(&env).query_row("SELECT COUNT(*) FROM cos_inbox_items WHERE source_kind='decision' AND source_key=?1 AND state='pending'",[format!("decision-{new_id}")],|r|r.get(0)).expect("new item");
    assert_eq!(new_items, 1);
    let old_row = env.store.decision_get(&old).expect("read").expect("old");
    assert_eq!(old_row.status, DecisionStatus::Withdrawn);
    let new_row = env.store.decision_get(new_id).expect("read").expect("new");
    assert_eq!(new_row.status, DecisionStatus::Open);
    let cos_bearer = live_cos_bearer(&env, "override-answer");
    let blocked=send(&app,post_json_with("/api/v1/cos/operations",&json!({
        "idempotency_key":"old-epoch","expected_revision":null,"reason":"try again","policy_version":"1",
        "request":{"method":"POST","path":format!("/api/v1/decisions/{new_id}/answer"),"body":{"option":"no"}}
    }),&[("authorization",cos_bearer.as_str())])).await;
    assert_eq!(blocked.status.as_u16(), 403, "{}", blocked.text());
    assert!(
        body["paused_task_ids"]
            .as_array()
            .expect("paused")
            .is_empty()
    );
    let after = events(&env, &task);
    assert_eq!(&after[..before.len()], &before);
    assert!(after.iter().any(|e| e["type"] == "cos_operation"
        && e["operation_id"] == op
        && e["reason"] == "The human corrected the choice"));
}

#[tokio::test]
async fn cos_chat_triage_override_return_pauses_consumed_task_and_blocks_old_epoch() {
    let env = admin_env();
    let app = env.router();
    let (task, old, op) = decision_fixture(&env, "decision.answer");
    let task_key = task.parse().expect("task id");
    let parent = env
        .store
        .get(task_key)
        .expect("get parent")
        .expect("parent");
    let mut adopted = new_task(TaskKind::Execute, Status::Ready);
    adopted.tree = Some(TreeInfo::child_of(
        &parent,
        ParentUnit {
            task_id: task_key,
            plan_id: "plan".into(),
            unit_key: "child".into(),
            stage: "stage".into(),
            attempt: 1,
        },
        None,
    ));
    let adopted_id = adopted.id;
    env.seed(&adopted);
    assert!(
        env.store
            .acquire_lease(task_key, "later", std::time::Duration::from_secs(60))
            .expect("lease")
    );
    conn(&env).execute("INSERT INTO runs(run_id,task_id,role,seq,status,started_at) VALUES('later',?1,'worker',1,'running','9999-01-01T00:00:00Z')",[&task]).expect("later run");
    let resp = send(&app, post_admin(&override_path(&op), &payload("return"))).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let body = resp.json();
    assert_eq!(body["state"], "superseded");
    assert!(
        body["paused_task_ids"]
            .as_array()
            .expect("paused")
            .iter()
            .any(|v| v == &task)
    );
    let task_id = task.parse().expect("task id");
    let stopped = env.store.get(task_id).expect("get").expect("task");
    assert!(stopped.paused_at.is_some());
    assert_eq!(stopped.status, Status::Ready);
    assert!(stopped.lease.is_none());
    assert!(
        env.store
            .get(adopted_id)
            .expect("get adopted")
            .expect("adopted")
            .paused_at
            .is_some()
    );
    let stale = send(
        &app,
        post_admin(
            &format!("/api/v1/decisions/{old}/answer"),
            &json!({"option":"no"}),
        ),
    )
    .await;
    assert_eq!(stale.status.as_u16(), 409, "{}", stale.text());
    let duplicate = send(&app, post_admin(&override_path(&op), &payload("return"))).await;
    assert_eq!(duplicate.status.as_u16(), 409, "{}", duplicate.text());
}

#[tokio::test]
async fn cos_chat_triage_override_irreversible_op_creates_remediation_task() {
    let env = admin_env();
    let app = env.router();
    let (task, _, op) = decision_fixture(&env, "external.publish");
    let resp = send(&app, post_admin(&override_path(&op), &payload("return"))).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let body = resp.json();
    assert_eq!(body["state"], "needs_remediation");
    let remediation = body["remediation_task_id"]
        .as_str()
        .expect("remediation id");
    let child = env
        .store
        .get(remediation.parse().expect("id"))
        .expect("get")
        .expect("child");
    assert_eq!(child.parent_id.expect("parent").to_string(), task);
    assert!(
        events(&env, &task)
            .iter()
            .any(|e| e["type"] == "cos_operation"
                && e["operation_id"] == op
                && e["state"] == "needs_remediation")
    );
}

#[tokio::test]
async fn cos_chat_triage_override_rejects_cos_credential_and_missing_reason() {
    let env = admin_env();
    let app = env.router();
    let (_, _, op) = decision_fixture(&env, "decision.answer");
    let now = OffsetDateTime::now_utc();
    let thread = env
        .store
        .chat_thread_create(
            "admin",
            &ChatCreateThreadRequest {
                title: "override auth".into(),
                project_id: None,
                client_thread_id: "override-auth".into(),
            },
            now,
        )
        .expect("thread")
        .thread;
    env.store
        .chat_message_post(
            &thread.id,
            &ChatPostMessageRequest {
                client_message_id: "auth-message".into(),
                text: "go".into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
            },
            now,
        )
        .expect("message");
    let run = "auth-run";
    env.store
        .chat_run_claim_next(&thread.id, run, &json!({}), now)
        .expect("claim")
        .expect("run");
    let bearer = task_api::cos::issue_run_bearer(&env.store, &thread.id, run, Duration::hours(1))
        .expect("bearer");
    let auth = format!("Bearer {bearer}");
    let cos = send(
        &app,
        post_json_with(
            &override_path(&op),
            &payload("revoke"),
            &[("authorization", auth.as_str())],
        ),
    )
    .await;
    assert_eq!(cos.status.as_u16(), 403, "{}", cos.text());
    let invalid = send(
        &app,
        post_admin(
            &override_path(&op),
            &json!({"action":"revoke","reason":" "}),
        ),
    )
    .await;
    assert_eq!(invalid.status.as_u16(), 422, "{}", invalid.text());
}

#[tokio::test]
async fn cos_chat_triage_override_revoke_preserves_approval_and_opens_new_authorization() {
    let env = admin_env();
    let app = env.router();
    let task = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&task);
    let old = Ulid::new().to_string();
    conn(&env).execute("INSERT INTO approvals(id,node_id,task_id,question,decision,answer,created_at,decided_at) VALUES(?1,'cos',?2,'May this run?',NULL,NULL,'2026-01-01T00:00:00Z',NULL)",[old.clone(),task.id.to_string()]).expect("approval");
    let op_id = Ulid::new().to_string();
    let rule_id = Ulid::new().to_string();
    let ctx = AuditContext {
        actor: ChatActor::Cos,
        thread_id: "thread".into(),
        run_id: "cos-run".into(),
        operation_id: op_id.clone(),
        reason: "existing authorization".into(),
        policy_version: "1".into(),
    };
    env.store.cos_operation_apply(&ctx,"approval-key","hash","approval",&old,Some("r1"),"approval.decide",&json!({}),|tx,_|{
        tx.execute("UPDATE approvals SET decision='standing',answer='yes',decided_at='2026-01-02T00:00:00Z' WHERE id=?1",[&old])?;
        tx.execute("INSERT INTO standing_rules(id,node_id,rule,created_at) VALUES(?1,'cos','allow','2026-01-02T00:00:00Z')",[&rule_id])?;
        Ok(json!({"decision":"standing","standing_rule_id":rule_id}))
    }).expect("operation");
    let response = send(&app, post_admin(&override_path(&op_id), &payload("revoke"))).await;
    assert_eq!(response.status.as_u16(), 200, "{}", response.text());
    let body = response.json();
    let new = body["new_wait_id"].as_str().expect("new approval");
    assert_ne!(new, old);
    let db = conn(&env);
    let old_decision: String = db
        .query_row("SELECT decision FROM approvals WHERE id=?1", [&old], |r| {
            r.get(0)
        })
        .expect("old decision");
    assert_eq!(old_decision, "standing");
    let pending: Option<String> = db
        .query_row("SELECT decision FROM approvals WHERE id=?1", [new], |r| {
            r.get(0)
        })
        .expect("new decision");
    assert!(pending.is_none());
    assert_eq!(env.status_of(task.id), Status::Blocked);
    let rules: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM standing_rules WHERE id=?1",
            [&rule_id],
            |r| r.get(0),
        )
        .expect("rule count");
    assert_eq!(rules, 0);
    let cos_bearer = live_cos_bearer(&env, "override-approval");
    let blocked=send(&app,post_json_with("/api/v1/cos/operations",&json!({
        "idempotency_key":"approval-reanswer","expected_revision":null,"reason":"try again","policy_version":"1",
        "request":{"method":"POST","path":format!("/api/v1/approvals/{new}/decide"),"body":{"decision":"once","answer":"yes"}}
    }),&[("authorization",cos_bearer.as_str())])).await;
    assert_eq!(blocked.status.as_u16(), 403, "{}", blocked.text());
    let human = send(
        &app,
        post_admin(
            &format!("/api/v1/approvals/{new}/decide"),
            &json!({"decision":"once","answer":"yes"}),
        ),
    )
    .await;
    assert_eq!(human.status.as_u16(), 200, "{}", human.text());
    assert_eq!(env.status_of(task.id), Status::Ready);
    assert!(
        events(&env, &task.id.to_string())
            .iter()
            .any(|e| e["type"] == "cos_operation" && e["operation_id"] == op_id)
    );
}

#[tokio::test]
async fn cos_chat_triage_override_external_effect_without_task_still_creates_remediation() {
    let env = admin_env();
    let app = env.router();
    let op_id = Ulid::new().to_string();
    let ctx = AuditContext {
        actor: ChatActor::Cos,
        thread_id: "thread".into(),
        run_id: "cos-run".into(),
        operation_id: op_id.clone(),
        reason: "sent".into(),
        policy_version: "1".into(),
    };
    env.store
        .cos_operation_apply(
            &ctx,
            "external-key",
            "hash",
            "external",
            "published",
            None,
            "external.publish",
            &json!({}),
            |_, _| Ok(json!({"sent":true})),
        )
        .expect("operation");
    let response = send(&app, post_admin(&override_path(&op_id), &payload("return"))).await;
    assert_eq!(response.status.as_u16(), 200, "{}", response.text());
    let body = response.json();
    assert_eq!(body["state"], "needs_remediation");
    let child = body["remediation_task_id"].as_str().expect("task id");
    assert!(
        env.store
            .get(child.parse().expect("id"))
            .expect("get")
            .is_some()
    );
}

#[tokio::test]
async fn cos_chat_triage_override_human_revision_wins_race_with_cos_answer() {
    let env = admin_env();
    let app = env.router();
    let (task, decision, op) = decision_fixture(&env, "decision.answer");
    env.store
        .append_event(
            task.parse().expect("task"),
            &Event::DecisionAnswered {
                id: decision,
                option: "no".into(),
                note: None,
                by: "human".into(),
            },
        )
        .expect("human revision");
    let response = send(&app, post_admin(&override_path(&op), &payload("revoke"))).await;
    assert_eq!(response.status.as_u16(), 409, "{}", response.text());
    assert_eq!(
        env.store
            .cos_operation_get(&op)
            .expect("get")
            .expect("operation")
            .state,
        "applied"
    );
}

#[tokio::test]
async fn cos_chat_triage_override_reblocks_released_work_unit_until_human_answer() {
    let env = admin_env();
    let app = env.router();
    let (task, _, op) = decision_fixture(&env, "decision.answer");
    let spec:WorkUnitSpec=serde_json::from_value(json!({"key":"unit","kind":"implement","title":"Dependent unit","objective":"wait for choice"})).expect("spec");
    let mut unit = WorkUnitRow::new(
        Ulid::new().to_string(),
        task.clone(),
        "plan".into(),
        0,
        spec,
        WorkUnitStatus::Ready,
        "2026-01-01T00:00:00Z".into(),
    );
    unit.needs_decisions = vec!["choice".into()];
    env.store
        .work_units_apply(
            task.parse().expect("task"),
            vec![unit.clone()],
            vec![],
            vec![],
        )
        .expect("unit");
    let response = send(&app, post_admin(&override_path(&op), &payload("revoke"))).await;
    assert_eq!(response.status.as_u16(), 200, "{}", response.text());
    let after = env
        .store
        .work_units_for(task.parse().expect("task"))
        .expect("units");
    assert_eq!(after[0].status, WorkUnitStatus::Blocked);
    assert_eq!(
        after[0].blocked_reason,
        Some(WorkUnitBlockedReason::Decision)
    );
    assert!(
        events(&env, &task)
            .iter()
            .any(|e| e["type"] == "work_unit_transitioned"
                && e["work_unit_id"] == unit.id
                && e["reason"] == format!("human_override:{op}"))
    );
}
