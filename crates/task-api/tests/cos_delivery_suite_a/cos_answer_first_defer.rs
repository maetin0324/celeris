//! ADR 2026-11-12-discord-person-or-operator D2: an escalation to the person names the options only
//! the person may choose (`reserved_options`). When CoS recommends an option outside them, the API
//! refuses the escalation (no Discord) and CoS answers the recommended option inside the approved
//! frame, writing the person's part into the answer's note as the question the worker asks later.
//! Matters the person keeps (beyond the frame, a research judgment, an explicit `human_required`)
//! still reach the person's outbox. The CoS worker is a fake harness: the test calls the API with a
//! live CoS run credential (no LLM, no network); the outbox is only `notifications` rows.
//!
//! Why the existing tests miss it: `cos_routine_a_within_approved_frame_answers_and_over_frame_escalates`
//! checks that an in-frame answer is accepted, but nothing stopped CoS from escalating the same
//! in-frame recommendation to the person instead (case 1 of the ADR, task 01M782BS on 2026-11-12).
use crate::common;

use common::cos_ops::{cos_bearer, db};
use common::*;
use serde_json::{Value, json};
use task_core::approval::{Approval, ApprovalId, ApprovalStore, Decision};
use task_core::chat::triage::CosTriageSource;
use task_core::{Event, Status, TaskKind};
use time::OffsetDateTime;

/// The worker's question of case 1 (shortened): three ways to end a re-measurement.
const CASE1: &str = "診断 job の途中で a09 が node down になったので投入を止めました。node-h は 19.532 / 20 です。\
    1 ここで終える（w5 × client 10 host は欠測）。2（推奨）a17・a22 で OOM の記録を読むだけの job を 1 本出して終える\
    （0.167 node-h、合計 19.70 / 20）。3 node-h の上限を上げて測り直す。";

/// A blocked task whose last event is `text`, with its pending authorization (`labels` on the task).
fn authorization(env: &TestEnv, text: &str, labels: &[&str]) -> (task_core::Task, ApprovalId) {
    let mut task = new_task(TaskKind::Execute, Status::Blocked);
    task.labels = labels.iter().map(|l| (*l).to_string()).collect();
    env.seed_with(
        &task,
        vec![Event::QuestionRaised {
            run_id: "run-1".into(),
            text: text.into(),
        }],
    );
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

/// The person's earlier approval of the 20 node-h frame.
fn frame_approval(env: &TestEnv) -> ApprovalId {
    let approval = Approval {
        id: ApprovalId::new(),
        project_id: None,
        node_id: "cluster-hpc".into(),
        task_id: None,
        question: "BenchFS の測り直しに node-h を 20 まで使ってよいか".into(),
        decision: Some(Decision::Once),
        answer: Some("20 node-h まで".into()),
        created_at: OffsetDateTime::now_utc(),
        decided_at: Some(OffsetDateTime::now_utc()),
    };
    env.store.approval_append(&approval).expect("frame");
    approval.id
}

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
        policy_version: "13".into(),
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

/// The options of case 1 (the authorization's buttons).
fn options() -> Value {
    json!([{"key": "once", "label": "今回だけ認める（案 2: 記録を読む job を出して終える）"},
           {"key": "denied", "label": "認めない（案 1: ここで終える）"},
           {"key": "standing", "label": "今後ずっと認める"}])
}

fn escalate(key: &str, task: &task_core::Task, recommended: &str, reserved: &[&str]) -> Value {
    json!({
        "idempotency_key": key,
        "expected_revision": "r1",
        "outcome": "escalate",
        "reason": "欠測のまま閉じるかは研究上の判断、案 3 は枠を超える",
        "policy_version": "1",
        "confidence": 0.9,
        "escalation": {
            "to": "human", "summary": "測り直しの終え方を決めてください",
            "options": options(),
            "recommended": recommended,
            "reserved_options": reserved,
            "recommendation_reason": "予約 0.167 node-h で上限 20 の内側に収まる",
            "web_path": format!("/tasks/{}", task.id),
        },
    })
}

fn answer_in_frame(key: &str, frame: ApprovalId, requested: f64, note: &str) -> Value {
    json!({
        "idempotency_key": key,
        "expected_revision": "r1",
        "outcome": "answer",
        "reason": "推奨の案 2 は人が承認した 20 node-h の枠の内",
        "policy_version": "1",
        "confidence": 0.9,
        "answer": {"option": "once", "note": note, "payload": null},
        "basis": {"category": "within_approved_frame",
                  "refs": [{"kind": "usage", "id": "GET /api/v1/tasks/x/usage 2026-11-12T23:18Z"}],
                  "frame": {"kind": "approval", "id": frame.to_string(), "unit": "node_hours",
                            "limit": 20.0, "used": 19.532, "requested": requested}},
    })
}

async fn resolve(env: &TestEnv, bearer: &str, item: &str, body: Value) -> Resp {
    send(
        &env.router(),
        post_json_with(
            &format!("/api/v1/cos/inbox/{item}/resolve"),
            &body,
            &[("authorization", bearer)],
        ),
    )
    .await
}

fn escalation_rows(env: &TestEnv, item: &str) -> i64 {
    db(env)
        .query_row(
            "SELECT COUNT(*) FROM notifications WHERE kind='cos_escalation' AND key=?1",
            [item],
            |r| r.get(0),
        )
        .expect("count")
}

fn item_state(env: &TestEnv, item: &str) -> String {
    db(env)
        .query_row(
            "SELECT state FROM cos_inbox_items WHERE id=?1",
            [item],
            |r| r.get(0),
        )
        .expect("item")
}

/// 不変条件: 推奨の選択肢が人に残す選択肢でない件（事例 1: 推奨の案 2 が人の承認した枠の内）は、人宛ての
/// escalate が 422 で断られて Discord の行ができず、CoS が枠の内の答えを出し、人の判断（欠測のまま閉じるか・
/// 上限の変更）は答えの note で後の問いに回る。
#[tokio::test]
async fn answer_first_case1_in_frame_recommendation_is_answered_and_not_sent() {
    let env = admin_env();
    let bearer = cos_bearer(&env, "case1").2;
    let frame = frame_approval(&env);
    let (task, approval) = authorization(&env, CASE1, &[]);
    let item = ingest(&env, approval);

    // Case 1 as CoS sent it on 2026-11-12: the whole item to the person, recommending option 2.
    let refused = resolve(
        &env,
        &bearer,
        &item,
        escalate("esc", &task, "once", &["denied", "standing"]),
    )
    .await;
    assert_problem(&refused, 422, "cos_answer_unreserved_recommendation");
    assert_eq!(escalation_rows(&env, &item), 0, "nothing for Discord");
    assert_eq!(item_state(&env, &item), "pending");
    // An escalation to the person must say which options are the person's.
    let unnamed = resolve(
        &env,
        &bearer,
        &item,
        escalate("esc-none", &task, "once", &[]),
    )
    .await;
    assert_problem(&unnamed, 422, "cos_escalation_invalid");
    assert_eq!(escalation_rows(&env, &item), 0);

    // CoS answers the in-frame part and defers the person's part to a later question.
    let later = "案 2 を実行する。記録を読んだ後に task を閉じず、欠測のまま閉じるか・上限を上げて測り直すかを新しい認可で人に聞く";
    let ok = resolve(
        &env,
        &bearer,
        &item,
        answer_in_frame("ok", frame, 0.167, later),
    )
    .await;
    assert_eq!(ok.status.as_u16(), 200, "{}", ok.text());
    assert_eq!(ok.json()["item"]["state"], "answered");
    assert_eq!(escalation_rows(&env, &item), 0, "no Discord for case 1");
    let decided = env
        .store
        .approval_get(approval)
        .expect("get")
        .expect("approval");
    assert_eq!(decided.decision, Some(Decision::Once));
    assert!(
        decided
            .answer
            .as_deref()
            .is_some_and(|a| a.contains("新しい認可で人に聞く")),
        "{decided:?}"
    );
    assert_eq!(env.status_of(task.id), Status::Ready);
}

/// 不変条件: 人に残す件は今までどおり人の outbox（Discord）に行が 1 つできる。枠を超える推奨（beyond_approval）、
/// 推奨そのものが研究上の判断（research_judgment）、人が `human_required` を付けた件（`reserved_options` なしで可）。
#[tokio::test]
async fn answer_first_reserved_matters_still_reach_the_person() {
    let env = admin_env();
    let bearer = cos_bearer(&env, "reserved").2;
    let frame = frame_approval(&env);

    // Beyond the frame: the in-frame answer is refused, and the escalation is sent.
    let (task, approval) = authorization(&env, CASE1, &[]);
    let item = ingest(&env, approval);
    let over = resolve(
        &env,
        &bearer,
        &item,
        answer_in_frame("over", frame, 0.5, "続ける"),
    )
    .await;
    assert_problem(&over, 422, "cos_basis_over_frame");
    let sent = resolve(
        &env,
        &bearer,
        &item,
        escalate("esc-over", &task, "once", &["once", "standing"]),
    )
    .await;
    assert_eq!(sent.status.as_u16(), 200, "{}", sent.text());
    assert_eq!(sent.json()["item"]["state"], "escalated");
    assert_eq!(escalation_rows(&env, &item), 1);

    // A research judgment recommended as such (close with the missing condition).
    let (task, approval) = authorization(&env, CASE1, &[]);
    let item = ingest(&env, approval);
    let sent = resolve(
        &env,
        &bearer,
        &item,
        escalate("esc-research", &task, "denied", &["denied"]),
    )
    .await;
    assert_eq!(sent.status.as_u16(), 200, "{}", sent.text());
    assert_eq!(escalation_rows(&env, &item), 1);

    // The person marked the task human_required: the whole item is theirs.
    let (task, approval) = authorization(&env, CASE1, &["human_required"]);
    let item = ingest(&env, approval);
    let sent = resolve(&env, &bearer, &item, escalate("esc-hr", &task, "once", &[])).await;
    assert_eq!(sent.status.as_u16(), 200, "{}", sent.text());
    assert_eq!(escalation_rows(&env, &item), 1);
}
