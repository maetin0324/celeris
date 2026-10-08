//! ADR 2026-10-05 D6「導入順と受け入れ条件」の通し試験（API 側）: 一次対応 A（人不要の代答）、
//! B（人に回す）、代答の修正（resolve の代答を人が取り消す）。CoS worker の役は試験が CoS run の
//! credential で resolve を呼ぶ偽 harness が務める（外部 LLM・外部ネットワーク無し）。待ちの取り込みは
//! dispatcher と同じ導出（`task_ops::human_inbox` の kind・id・created_at）で store に入れる。
//! 時刻は注入した値で、sleep しない。
mod common;

use common::*;
use serde_json::{Value, json};
use task_core::approval::{Approval, ApprovalId, ApprovalStore};
use task_core::chat::triage::CosTriageSource;
use task_core::chat::{ChatCreateThreadRequest, ChatPostMessageRequest, ChatSendMode};
use task_core::decision::{
    CostOfReversal, DecisionKind, DecisionOption, DecisionOrigin, DecisionPathEntry,
    DecisionRaisedBy, DecisionRequest, DecisionStatus,
};
use task_core::{Event, Status, TaskKind, TaskStore};
use time::{Duration, OffsetDateTime};

fn at(secs: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_790_000_000 + secs).expect("time")
}

fn db(env: &TestEnv) -> rusqlite::Connection {
    rusqlite::Connection::open(&env.db_path).expect("open db")
}

fn count(env: &TestEnv, sql: &str) -> i64 {
    db(env).query_row(sql, [], |r| r.get(0)).expect("count")
}

/// A live CoS run of the inbox thread: the fake harness's credential.
fn cos_bearer(env: &TestEnv, key: &str) -> String {
    let store = &env.store;
    let now = OffsetDateTime::now_utc();
    let thread = store
        .chat_thread_create(
            "admin",
            &ChatCreateThreadRequest {
                title: format!("受信箱 {key}"),
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
                text: "item_ids=…".into(),
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
    format!("Bearer {bearer}")
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
                    key: format!("h-{id}"),
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

fn decision_task(env: &TestEnv, id: &str, labels: &[&str]) -> task_core::Task {
    let mut task = new_task(TaskKind::Execute, Status::Ready);
    task.labels = labels.iter().map(|l| l.to_string()).collect();
    env.seed(&task);
    raise_decision(env, &task, id);
    task
}

fn question_task(env: &TestEnv, labels: &[&str]) -> task_core::Task {
    let mut task = new_task(TaskKind::Execute, Status::Blocked);
    task.labels = labels.iter().map(|l| l.to_string()).collect();
    env.seed_with(
        &task,
        vec![Event::QuestionRaised {
            run_id: "r0".into(),
            text: "どの branch に入れますか?".into(),
        }],
    );
    task
}

/// A task blocked on a question with a pending approval row: one `authorization` item.
fn approval(env: &TestEnv) -> Approval {
    let task = question_task(env, &[]);
    let approval = Approval {
        id: ApprovalId::new(),
        project_id: None,
        node_id: "engineering".into(),
        task_id: Some(task.id),
        question: "既定の queue を使いますか".into(),
        decision: None,
        answer: None,
        created_at: OffsetDateTime::now_utc(),
        decided_at: None,
    };
    env.store.approval_append(&approval).expect("append");
    approval
}

/// A root task waiting for its plan approval (`blocked(awaiting_plan_approval)`).
fn plan_gate_task(env: &TestEnv) -> task_core::Task {
    let task = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&task);
    env.store
        .apply_transition(task.id, task_core::Trigger::Dispatch, None)
        .expect("dispatch");
    env.store
        .apply_transition_with_events(
            task.id,
            task_core::Trigger::PlanGate {
                plan_id: "plan-1".into(),
            },
            vec![Event::PlanApprovalRequested {
                plan_id: "plan-1".into(),
                reasons: vec!["review_human:phase-2".into()],
            }],
        )
        .expect("plan gate");
    task
}

/// The derived inbox items, keyed the way the dispatcher's intake keys them.
fn derived(env: &TestEnv) -> Vec<task_ops::human_inbox::InboxItem> {
    task_ops::human_inbox::human_inbox(
        &env.store,
        None,
        &env.view_context(),
        OffsetDateTime::now_utc(),
        &|_, _| Vec::new(),
    )
    .expect("human inbox")
    .items
}

/// Ingest every derived wait (as the dispatcher does) and return `(inbox id, item row id, revision)`.
fn ingest_all(env: &TestEnv, cursor: i64) -> Vec<(String, String, String)> {
    let sources: Vec<CosTriageSource> = derived(env)
        .iter()
        .map(|item| CosTriageSource {
            source_kind: item.kind.as_str().to_owned(),
            source_key: item.id.clone(),
            source_revision: item.created_at.clone(),
            source_event_id: None,
            operation_id: None,
            summary: item.title.clone(),
            policy_version: "1".into(),
        })
        .collect();
    env.store
        .cos_triage_ingest_batch("events", &cursor.to_string(), &sources, at(cursor))
        .expect("ingest");
    sources
        .iter()
        .map(|s| {
            let id: String = db(env)
                .query_row(
                    "SELECT id FROM cos_inbox_items WHERE source_kind=?1 AND source_key=?2 AND source_revision=?3",
                    [&s.source_kind, &s.source_key, &s.source_revision],
                    |r| r.get(0),
                )
                .expect("ingested row");
            (s.source_key.clone(), id, s.source_revision.clone())
        })
        .collect()
}

fn find<'a>(rows: &'a [(String, String, String)], prefix: &str) -> &'a (String, String, String) {
    rows.iter()
        .find(|(key, _, _)| key.starts_with(prefix))
        .unwrap_or_else(|| panic!("no derived item {prefix} in {rows:?}"))
}

fn resolve_path(item: &str) -> String {
    format!("/api/v1/cos/inbox/{item}/resolve")
}

fn body(key: &str, revision: &str, outcome: &str, reason: &str, extra: Value) -> Value {
    let mut body = json!({
        "idempotency_key": key,
        "expected_revision": revision,
        "outcome": outcome,
        "reason": reason,
        "policy_version": "1",
    });
    if let (Some(object), Some(more)) = (body.as_object_mut(), extra.as_object()) {
        for (k, v) in more {
            object.insert(k.clone(), v.clone());
        }
    }
    body
}

fn cos_audits(env: &TestEnv) -> Vec<Value> {
    let conn = db(env);
    let mut stmt = conn
        .prepare("SELECT json FROM events ORDER BY seq")
        .expect("prepare");
    stmt.query_map([], |row| row.get::<_, String>(0))
        .expect("query")
        .map(|raw| serde_json::from_str::<Value>(&raw.expect("row")).expect("json"))
        .filter(|e| e["type"] == "cos_operation")
        .collect()
}

/// D6 一次対応 A: 承認済み範囲の decision・question・approval・plan gate を偽 CoS が answer する。
/// 元の待ちが解消し、監査は actor=cos と理由・カードつき、webhook の outbox は 0 通、同じ待ちの
/// 再配送では item も再回答も増えない。
#[tokio::test]
async fn cos_chat_triage_a_answers_each_wait_kind_without_webhook() {
    let env = admin_env();
    let app = env.router();
    let decision = decision_task(&env, "dec-a", &[]);
    let question = question_task(&env, &[]);
    let approval = approval(&env);
    let plan = plan_gate_task(&env);
    let rows = ingest_all(&env, 1);
    assert_eq!(rows.len(), 4, "{rows:?}");
    let bearer = cos_bearer(&env, "a");
    let headers = [("authorization", bearer.as_str())];
    let cases = [
        (
            "decision-",
            json!({"option": "vault", "note": "既定どおり"}),
        ),
        (
            "question-",
            json!({"option": "answer", "note": "main に入れる"}),
        ),
        (
            "authorization-",
            json!({"option": "once", "note": "既定の queue"}),
        ),
        (
            "plan_gate-",
            json!({"option": "approve", "note": "既承認計画の具体化"}),
        ),
    ];
    let mut ops = Vec::new();
    for (prefix, answer) in cases {
        let (_, item, revision) = find(&rows, prefix);
        let resp = send(
            &app,
            post_json_with(
                &resolve_path(item),
                &body(
                    &format!("k-{prefix}"),
                    revision,
                    "answer",
                    &format!("{prefix} は既存の指示の範囲内"),
                    json!({"answer": answer}),
                ),
                &headers,
            ),
        )
        .await;
        assert_eq!(resp.status.as_u16(), 200, "{prefix}: {}", resp.text());
        let out = resp.json();
        assert_eq!(out["operation"]["state"], "applied", "{prefix}: {out}");
        assert_eq!(out["operation"]["actor"], "cos");
        assert!(out["operation"]["event_id"].is_string(), "card: {out}");
        assert_eq!(out["item"]["state"], "answered");
        ops.push(out["operation"]["id"].as_str().expect("op").to_string());
    }
    // The source waits are gone: decision answered by cos, question/plan resumed, approval decided.
    assert!(derived(&env).is_empty(), "{:?}", derived(&env));
    assert!(
        env.store
            .events_for(decision.id)
            .expect("events")
            .iter()
            .any(|(_, e)| matches!(e, Event::DecisionAnswered { by, .. } if by == "cos"))
    );
    assert_eq!(env.status_of(question.id), Status::Ready);
    assert_ne!(env.status_of(plan.id), Status::Blocked);
    let decided = env
        .store
        .approval_get(approval.id)
        .expect("get")
        .expect("approval");
    assert!(decided.decision.is_some());
    // actor=cos and the reason are audited for each operation.
    let audits = cos_audits(&env);
    for op in &ops {
        let found = audits
            .iter()
            .find(|e| e["operation_id"] == op.as_str())
            .unwrap_or_else(|| panic!("no audit for {op}: {audits:?}"));
        let text = found.to_string();
        assert!(text.contains("\"cos\""), "{text}");
        assert!(text.contains("は既存の指示の範囲内"), "{text}");
    }
    // No Discord outbox at all on row A.
    assert_eq!(count(&env, "SELECT COUNT(*) FROM notifications"), 0);
    // Redelivery of the same waits: no new item, no second answer.
    let again = ingest_all(&env, 2);
    assert!(again.is_empty(), "resolved waits are not re-offered");
    assert_eq!(count(&env, "SELECT COUNT(*) FROM cos_inbox_items"), 4);
    let (_, item, revision) = find(&rows, "decision-");
    let resp = send(
        &app,
        post_json_with(
            &resolve_path(item),
            &body(
                "k-replay-new-key",
                revision,
                "answer",
                "再配送",
                json!({"answer": {"option": "manual"}}),
            ),
            &headers,
        ),
    )
    .await;
    assert_problem(&resp, 409, "cos_inbox_revision_conflict");
    let answered = env
        .store
        .events_for(decision.id)
        .expect("events")
        .into_iter()
        .filter(|(_, e)| matches!(e, Event::DecisionAnswered { .. }))
        .count();
    assert_eq!(answered, 1);
}

/// D6 一次対応 B: 外部 push・設計変更・明示 human・低確信の各 1 件を escalate する。outbox に
/// 要点・選択肢・推奨・理由・web path が入り、元の待ちは残る。明示 human は answer が 403 で
/// escalate だけが通る。人が web で答えると item は待ちから外れ、CoS は答えられない（409）。
#[tokio::test]
async fn cos_chat_triage_b_escalates_human_matters_and_keeps_the_wait() {
    let env = admin_env();
    let app = env.router();
    let push = decision_task(&env, "dec-push", &[]);
    let design = decision_task(&env, "dec-design", &[]);
    let explicit = decision_task(&env, "dec-human", &["human_required"]);
    let low = question_task(&env, &[]);
    let rows = ingest_all(&env, 1);
    assert_eq!(rows.len(), 4, "{rows:?}");
    let bearer = cos_bearer(&env, "b");
    let headers = [("authorization", bearer.as_str())];

    // Explicit human_required: answer is refused deterministically.
    let (_, explicit_item, explicit_rev) = find(&rows, "decision-dec-human");
    let refused = send(
        &app,
        post_json_with(
            &resolve_path(explicit_item),
            &body(
                "k-try",
                explicit_rev,
                "answer",
                "定型",
                json!({"answer": {"option": "vault"}}),
            ),
            &headers,
        ),
    )
    .await;
    assert_problem(&refused, 403, "cos_human_required");

    let decision_options = json!([
        {"key": "vault", "label": "vault を使う"},
        {"key": "manual", "label": "手で入れる"}
    ]);
    let cases = [
        (
            "decision-dec-push",
            push.id,
            "外部 repo への push",
            decision_options.clone(),
            json!("manual"),
            "push 先が未認可",
        ),
        (
            "decision-dec-design",
            design.id,
            "設計の根本変更",
            decision_options.clone(),
            json!(null),
            "受け入れ条件が変わる",
        ),
        (
            "decision-dec-human",
            explicit.id,
            "人の判断指定",
            decision_options.clone(),
            json!("vault"),
            "human_required の待ち",
        ),
        (
            "question-",
            low.id,
            "低確信の質問",
            json!([{"key": "reply", "label": "web で回答"}]),
            json!(null),
            "confidence 0.4 < 0.85",
        ),
    ];
    for (prefix, task_id, summary, options, recommended, why) in cases {
        let (_, item, revision) = find(&rows, prefix);
        let resp = send(
            &app,
            post_json_with(
                &resolve_path(item),
                &body(
                    &format!("k-{prefix}"),
                    revision,
                    "escalate",
                    why,
                    json!({"escalation": {
                        "summary": summary,
                        "options": options,
                        "recommended": recommended,
                        "recommendation_reason": why,
                        "web_path": format!("/tasks/{task_id}"),
                    }}),
                ),
                &headers,
            ),
        )
        .await;
        assert_eq!(resp.status.as_u16(), 200, "{prefix}: {}", resp.text());
        assert_eq!(resp.json()["item"]["state"], "escalated");
        let raw: String = db(&env)
            .query_row(
                "SELECT body FROM notifications WHERE kind='cos_escalation' AND key=?1",
                [item],
                |r| r.get(0),
            )
            .expect("one outbox row per escalation");
        let sent: Value = serde_json::from_str(&raw).expect("outbox body");
        assert_eq!(sent["summary"], summary);
        assert_eq!(sent["options"], options);
        assert_eq!(sent["recommended"], recommended);
        assert_eq!(sent["recommendation_reason"], why);
        assert_eq!(sent["web_path"], format!("/tasks/{task_id}"));
    }
    assert_eq!(
        count(
            &env,
            "SELECT COUNT(*) FROM notifications WHERE kind='cos_escalation'"
        ),
        4
    );
    // The waits stay open for the human.
    assert_eq!(derived(&env).len(), 4);
    assert_eq!(env.status_of(low.id), Status::Blocked);

    // The human answers on the web; CoS cannot answer the stale wait afterwards.
    let human = send(
        &app,
        post_admin(
            "/api/v1/inbox/items/decision-dec-push/answer",
            &json!({"option": "manual"}),
        ),
    )
    .await;
    assert_eq!(human.status.as_u16(), 200, "{}", human.text());
    assert_eq!(derived(&env).len(), 3);
    let (_, push_item, push_rev) = find(&rows, "decision-dec-push");
    let late = send(
        &app,
        post_json_with(
            &resolve_path(push_item),
            &body(
                "k-late",
                push_rev,
                "answer",
                "遅れた代答",
                json!({"answer": {"option": "vault"}}),
            ),
            &headers,
        ),
    )
    .await;
    assert_problem(&late, 409, "cos_inbox_revision_conflict");
}

/// D6 代答の修正（通し）: resolve で代答した decision を人が revoke すると、旧回答は superseded・
/// 新しい人待ちが開き、同じ操作の二度目の override は 409、CoS は新しい待ちに代答できない。
#[tokio::test]
async fn cos_chat_triage_override_revokes_a_resolved_answer_end_to_end() {
    let env = admin_env();
    let app = env.router();
    let task = decision_task(&env, "dec-o", &[]);
    let rows = ingest_all(&env, 1);
    let (_, item, revision) = find(&rows, "decision-dec-o");
    let bearer = cos_bearer(&env, "o");
    let headers = [("authorization", bearer.as_str())];
    let resp = send(
        &app,
        post_json_with(
            &resolve_path(item),
            &body(
                "k-o",
                revision,
                "answer",
                "定型",
                json!({"answer": {"option": "vault"}}),
            ),
            &headers,
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let op = resp.json()["operation"]["id"]
        .as_str()
        .expect("op")
        .to_string();
    let path = format!("/api/v1/cos/operations/{op}/override");
    let revoke = send(
        &app,
        post_admin(
            &path,
            &json!({"action": "revoke", "reason": "人が選び直す"}),
        ),
    )
    .await;
    assert_eq!(revoke.status.as_u16(), 200, "{}", revoke.text());
    let out = revoke.json();
    assert_eq!(out["state"], "superseded", "{out}");
    let new_wait = out["new_wait_id"].as_str().expect("new wait").to_string();
    assert_ne!(new_wait, "dec-o");
    let twice = send(
        &app,
        post_admin(&path, &json!({"action": "return", "reason": "二度目"})),
    )
    .await;
    assert_eq!(twice.status.as_u16(), 409, "{}", twice.text());
    // CoS cannot answer the reopened wait (the human's revision wins).
    let retry = send(
        &app,
        post_json_with(
            "/api/v1/cos/operations",
            &json!({
                "idempotency_key": "k-o-retry",
                "expected_revision": null,
                "reason": "再代答",
                "policy_version": "1",
                "request": {"method": "POST", "path": format!("/api/v1/decisions/{new_wait}/answer"), "body": {"option": "vault"}},
            }),
            &headers,
        ),
    )
    .await;
    assert_eq!(retry.status.as_u16(), 403, "{}", retry.text());
    // The audit history is append-only: the original answer and the override reason are both there.
    let events: Vec<String> = env
        .store
        .events_for(task.id)
        .expect("events")
        .into_iter()
        .map(|(_, e)| serde_json::to_string(&e).expect("json"))
        .collect();
    assert!(
        events.iter().any(|e| e.contains("人が選び直す")),
        "{events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| e.contains("DecisionAnswered") || e.contains("decision_answered"))
    );
}
