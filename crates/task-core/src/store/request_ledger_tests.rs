//! ADR 2026-10-27-eval-request-ledger: 再通知・改訂・差戻し・担当の再起動で行が増えないこと、
//! 各時刻と主体、不明が null のままであること。
use rusqlite::params;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::super::SqliteStore;
use crate::chat::triage::CosTriageSource;
use crate::decision_log::CampaignIdentity;
use crate::model::{Event, Status, TaskId};
use crate::store::TaskStore;

fn at(s: &str) -> OffsetDateTime {
    OffsetDateTime::parse(s, &Rfc3339).unwrap()
}

fn source(kind: &str, key: &str, revision: &str, task: TaskId) -> CosTriageSource {
    CosTriageSource {
        source_kind: kind.into(),
        source_key: key.into(),
        source_revision: revision.into(),
        source_event_id: None,
        operation_id: None,
        summary: "要求".into(),
        detail: String::new(),
        policy_version: "4".into(),
        task_id: Some(task.to_string()),
    }
}

fn append_at(s: &SqliteStore, task: TaskId, event: &Event, ts: &str) {
    let seq = s.append_event(task, event).unwrap();
    s.lock()
        .unwrap()
        .execute(
            "UPDATE events SET ts=?1 WHERE task_id=?2 AND seq=?3",
            params![ts, task.to_string(), seq as i64],
        )
        .unwrap();
}

fn requested(id: &str, raised_on: TaskId) -> Event {
    use crate::decision::*;
    let opt = |k: &str| DecisionOption {
        key: k.into(),
        label: k.to_uppercase(),
        consequence: None,
    };
    Event::DecisionRequested {
        decision: Box::new(DecisionRequest {
            id: id.into(),
            key: format!("k-{id}"),
            kind: DecisionKind::Choice,
            question: "which?".into(),
            options: vec![opt("a"), opt("b")],
            recommended: "a".into(),
            cost_of_reversal: CostOfReversal::Low,
            cost_note: None,
            needed_before: vec!["self".into()],
            path: Vec::new(),
            raised_by: DecisionRaisedBy {
                task_id: raised_on,
                run_id: None,
                origin: DecisionOrigin::Planner,
            },
            status: DecisionStatus::Open,
            answer: None,
            withdrawn_reason: None,
        }),
    }
}

fn item_id(s: &SqliteStore, key: &str, revision: &str) -> String {
    s.lock()
        .unwrap()
        .query_row(
            "SELECT id FROM cos_inbox_items WHERE source_key=?1 AND source_revision=?2",
            [key, revision],
            |r| r.get(0),
        )
        .unwrap()
}

fn thread_of(s: &SqliteStore, item: &str) -> String {
    s.lock()
        .unwrap()
        .query_row(
            "SELECT thread_id FROM cos_inbox_items WHERE id=?1",
            [item],
            |r| r.get(0),
        )
        .unwrap()
}

/// decision の要求: 再通知 3 回（同じ triple の再投入）、担当の再起動 1 回、CoS の回答を人が差し戻し（key が変わる）、
/// 改訂 1 回、人への escalation と人の回答、その後の run の再開。
#[test]
fn renotified_revised_and_returned_request_is_one_row_with_times_and_actors() {
    let s = SqliteStore::open_in_memory().unwrap();
    let task = super::super::tests::sample_task(Status::Draft);
    s.create_task(&task, vec![]).unwrap();
    let t = task.id;
    s.task_campaign_set(
        &t.to_string(),
        &CampaignIdentity {
            campaign_id: "camp-r".into(),
            case_id: Some("case-1".into()),
            arm: Some("B".into()),
            repetition: Some(2),
            manifest_hash: None,
            routing_setting: None,
            variant: None,
            variant_org: None,
        },
        at("2026-10-27T09:00:00Z"),
    )
    .unwrap();
    {
        let conn = s.lock().unwrap();
        // 解決の前の run は再開に数えない。
        conn.execute(
            "INSERT INTO runs(run_id,task_id,role,seq,status,started_at) VALUES \
             ('run-before',?1,'worker',1,'completed','2026-10-27T09:30:00Z'), \
             ('run-after',?1,'worker',2,'running','2026-10-27T10:31:00Z')",
            [t.to_string()],
        )
        .unwrap();
    }

    append_at(&s, t, &requested("D1", t), "2026-10-27T10:00:00Z");

    // 初回と再通知 3 回: 同じ triple は item を増やさない。
    let first = source("decision", "decision-D1", "2026-10-27T10:00:00Z", t);
    for (n, ts) in [
        "2026-10-27T10:00:01Z",
        "2026-10-27T10:00:02Z",
        "2026-10-27T10:00:03Z",
        "2026-10-27T10:00:04Z",
    ]
    .iter()
    .enumerate()
    {
        let inserted = s
            .cos_triage_ingest_batch(
                "events",
                &n.to_string(),
                std::slice::from_ref(&first),
                at(ts),
            )
            .unwrap();
        assert_eq!(inserted, usize::from(n == 0));
    }
    let i1 = item_id(&s, "decision-D1", "2026-10-27T10:00:00Z");
    let thread = thread_of(&s, &i1);

    // 担当の再起動: R1 が取り、run を失って戻し、R2 が取り直す。
    s.cos_triage_claim(&thread, "R1", at("2026-10-27T10:01:00Z"))
        .unwrap()
        .unwrap();
    s.cos_triage_requeue_run("R1", at("2026-10-27T10:02:00Z"))
        .unwrap();
    {
        let conn = s.lock().unwrap();
        conn.execute(
            "UPDATE chat_runs SET state='interrupted' WHERE run_id='R1'",
            [],
        )
        .unwrap();
    }
    s.cos_triage_claim(&thread, "R2", at("2026-10-27T10:03:00Z"))
        .unwrap()
        .unwrap();
    {
        let conn = s.lock().unwrap();
        conn.execute(
            "UPDATE chat_runs SET first_output_at='2026-10-27T10:03:30.000Z' WHERE run_id='R2'",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cos_operations(id,thread_id,run_id,item_id,idempotency_key,request_hash,target_kind,target_id,\
             action,payload_json,reason,policy_version,state,created_at,updated_at) \
             VALUES('OP1',?1,'R2',?2,'k1','h','decision','D1','decision.answer',?3,'r','4','applied',\
             '2026-10-27T10:04:00.000Z','2026-10-27T10:04:00.000Z')",
            params![
                thread,
                i1,
                r#"{"instructed_by":{"message_id":"m1","seq":3,"decided_by":{"kind":"human","id":null,"relayed_by":{"kind":"external_operator","id":"claude-code"}}}}"#
            ],
        )
        .unwrap();
    }
    append_at(
        &s,
        t,
        &Event::DecisionAnswered {
            id: "D1".into(),
            option: "a".into(),
            note: None,
            by: format!("cos:{thread}"),
        },
        "2026-10-27T10:04:00Z",
    );
    assert!(
        s.cos_triage_resolve(
            &i1,
            "answered",
            Some("OP1"),
            None,
            at("2026-10-27T10:04:00Z")
        )
        .unwrap()
    );

    // 差戻し: 人が OP1 を覆し、新しい decision D2 の item が同じ transaction で作られる。
    {
        let conn = s.lock().unwrap();
        conn.execute(
            "UPDATE cos_operations SET state='superseded',updated_at='2026-10-27T10:10:00.000Z' WHERE id='OP1'",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cos_inbox_items(id,source_kind,source_key,source_revision,thread_id,message_id,state,policy_version,created_at,updated_at) \
             VALUES('ITEM-RETURNED','decision','decision-D2','2026-10-27T10:10:00.000Z',?1,'','pending','4','2026-10-27T10:10:00.000Z','2026-10-27T10:10:00.000Z')",
            [&thread],
        )
        .unwrap();
    }
    append_at(&s, t, &requested("D2", t), "2026-10-27T10:10:00Z");
    append_at(
        &s,
        t,
        &Event::DecisionWithdrawn {
            id: "D1".into(),
            reason: "overridden OP1: 違う".into(),
        },
        "2026-10-27T10:10:00Z",
    );

    // 改訂 1 回（D2 の新しい revision）と人への escalation。
    let revised = source("decision", "decision-D2", "2026-10-27T10:20:00Z", t);
    s.cos_triage_ingest_batch("events", "r", &[revised], at("2026-10-27T10:20:01Z"))
        .unwrap();
    let i3 = item_id(&s, "decision-D2", "2026-10-27T10:20:00Z");
    s.cos_triage_outbox_claim(&i3, "escalation", "{}", at("2026-10-27T10:21:00Z"))
        .unwrap()
        .unwrap();
    append_at(
        &s,
        t,
        &Event::DecisionAnswered {
            id: "D2".into(),
            option: "b".into(),
            note: None,
            by: "human:alice".into(),
        },
        "2026-10-27T10:30:00Z",
    );

    let ledger = s
        .request_ledger(at("2026-10-27T00:00:00Z"), at("2026-10-28T00:00:00Z"))
        .unwrap();
    assert_eq!(ledger.requests.len(), 1, "{ledger:#?}");
    assert_eq!(ledger.summary.requests, 1);
    assert_eq!(ledger.summary.items, 3);
    assert_eq!(ledger.summary.returns, 1);
    let row = &ledger.requests[0];
    assert_eq!(row.request_ref, "decision:decision-D1");
    assert_eq!(row.keys, vec!["decision-D1", "decision-D2"]);
    let events: Vec<&str> = row.revisions.iter().map(|r| r.event).collect();
    assert_eq!(events, vec!["initial", "returned", "revision"]);
    assert_eq!(row.revisions[0].run_ids, vec!["R1", "R2"]);
    assert_eq!(row.cos_runs, vec!["R1", "R2"]);
    assert_eq!(row.returns[0].operation_id, "OP1");
    assert_eq!(row.returns[0].from_item_id, i1);
    assert_eq!(row.returns[0].to_item_id.as_deref(), Some("ITEM-RETURNED"));
    assert_eq!(row.returns[0].at, "2026-10-27T10:10:00.000Z");

    // 初回の AI 応答は R2 の最初の出力（R1 は出力なし）。
    let ai = row.first_ai_response.as_ref().unwrap();
    assert_eq!(ai.at, "2026-10-27T10:03:30.000Z");
    assert_eq!(ai.source, "chat_run:R2");
    assert_eq!(ai.actor_kind, "cos_general");

    // 差し戻された revision は CoS が解決していた。
    let r1 = row.revisions[0].resolved.as_ref().unwrap();
    assert_eq!(
        (r1.actor_kind.as_str(), r1.at.as_str()),
        ("cos_general", "2026-10-27T10:04:00Z")
    );
    // 差戻しの item は改訂までに解決していない。
    assert_eq!(row.revisions[1].resolved, None);

    // 人の待ちと解決。
    assert_eq!(row.human_waits.len(), 1);
    let wait = &row.human_waits[0];
    assert_eq!(wait.item_id, i3);
    assert_eq!(wait.route, "escalation");
    assert_eq!(wait.started_at, "2026-10-27T10:21:00.000Z");
    assert_eq!(wait.ended_at.as_deref(), Some("2026-10-27T10:30:00Z"));
    assert_eq!(wait.end_actor_kind.as_deref(), Some("human"));
    let resolved = row.resolved.as_ref().unwrap();
    assert_eq!(resolved.actor_kind, "human");
    assert_eq!(resolved.actor_id.as_deref(), Some("alice"));
    assert_eq!(resolved.at, "2026-10-27T10:30:00Z");
    assert_eq!(ledger.summary.resolved_by_actor_kind.get("human"), Some(&1));

    // 再開は解決の後の最初の run。
    assert_eq!(row.resumed_at.as_deref(), Some("2026-10-27T10:31:00Z"));
    assert_eq!(row.resume_run_id.as_deref(), Some("run-after"));

    // 方針・承認の参照と評価の対応。
    assert!(row.policy_refs.contains(&"cos_triage_policy:4".to_string()));
    assert!(
        row.policy_refs
            .contains(&"cos_operation_policy:4".to_string())
    );
    assert!(row.policy_refs.contains(&"instructed_by:m1".to_string()));
    assert!(
        row.policy_refs
            .contains(&"relay:external_operator:claude-code".to_string())
    );
    assert_eq!(row.operations, vec!["OP1"]);
    assert_eq!(row.tasks, vec![t.to_string()]);
    let campaign = row.campaign.as_ref().unwrap();
    assert_eq!(campaign.campaign_id, "camp-r");
    assert_eq!(campaign.case_id.as_deref(), Some("case-1"));
    assert_eq!(campaign.arm.as_deref(), Some("B"));
    assert_eq!(campaign.repetition, Some(2));

    // 同じ DB を読み直しても同じ（読み取りだけ）。
    let again = s
        .request_ledger(at("2026-10-27T00:00:00Z"), at("2026-10-28T00:00:00Z"))
        .unwrap();
    assert_eq!(again, ledger);
}

/// notice の再通知（count の revision）は 1 行。主体の記録が無い要求は null のまま。範囲外の要求は出ない。
#[test]
fn renotified_notice_is_one_row_and_unknowns_stay_null() {
    let s = SqliteStore::open_in_memory().unwrap();
    let task = super::super::tests::sample_task(Status::Draft);
    s.create_task(&task, vec![]).unwrap();
    for (n, ts) in [
        "2026-10-27T11:00:00Z",
        "2026-10-27T11:05:00Z",
        "2026-10-27T11:10:00Z",
    ]
    .iter()
    .enumerate()
    {
        let notice = source("notice", "N1", &(n + 1).to_string(), task.id);
        s.cos_triage_ingest_batch("notices", ts, &[notice], at(ts))
            .unwrap();
    }
    let old = source(
        "question",
        &format!("question-{}", task.id),
        "2026-10-01T00:00:00Z",
        task.id,
    );
    s.cos_triage_ingest_batch("events", "old", &[old], at("2026-10-01T00:00:01Z"))
        .unwrap();

    let ledger = s
        .request_ledger(at("2026-10-27T00:00:00Z"), at("2026-10-28T00:00:00Z"))
        .unwrap();
    assert_eq!(ledger.requests.len(), 1);
    let row = &ledger.requests[0];
    assert_eq!(row.request_ref, "notice:N1");
    let revisions: Vec<&str> = row.revisions.iter().map(|r| r.revision.as_str()).collect();
    assert_eq!(revisions, vec!["1", "2", "3"]);
    let events: Vec<&str> = row.revisions.iter().map(|r| r.event).collect();
    assert_eq!(events, vec!["initial", "revision", "revision"]);
    assert_eq!(row.first_ai_response, None);
    assert_eq!(row.resolved, None);
    assert_eq!(row.resumed_at, None);
    assert_eq!(row.campaign, None);
    assert!(row.human_waits.is_empty());
    assert_eq!(ledger.summary.first_ai_response_missing, 1);
    assert_eq!(
        ledger.summary.resolved_by_actor_kind.get("unresolved"),
        Some(&1)
    );

    let json = serde_json::to_value(&ledger).unwrap();
    assert!(json["requests"][0]["first_ai_response"].is_null());
    assert!(json["requests"][0]["resolved"].is_null());
    assert!(json["requests"][0]["campaign"].is_null());
}

/// 人へ回した要求（item は escalated のまま operation を持たない）に後から CoS が取り次ぎを根拠に答えた operation は、
/// 対象の decision で同じ要求に寄り、根拠の参照が `policy_refs` に残る。別の decision への operation は寄せない。
#[test]
fn later_operation_on_an_escalated_request_is_linked_by_its_target() {
    use crate::chat::{AuditContext, ChatActor, ChatOpenThreadRequest};
    let s = SqliteStore::open_in_memory().unwrap();
    let task = super::super::tests::sample_task(Status::Draft);
    s.create_task(&task, vec![]).unwrap();
    let t = task.id;
    append_at(&s, t, &requested("D9", t), "2026-10-27T10:00:00Z");
    let src = source("decision", "decision-D9", "2026-10-27T10:00:00Z", t);
    s.cos_triage_ingest_batch("events", "1", &[src], at("2026-10-27T10:00:01Z"))
        .unwrap();
    let item = item_id(&s, "decision-D9", "2026-10-27T10:00:00Z");
    let thread = thread_of(&s, &item);
    s.cos_triage_claim(&thread, "run-triage", at("2026-10-27T10:00:02Z"))
        .unwrap()
        .unwrap();
    assert!(
        s.cos_triage_resolve(&item, "escalated", None, None, at("2026-10-27T10:00:03Z"))
            .unwrap()
    );
    let other = s
        .cos_thread_open(
            &ChatOpenThreadRequest { project_id: None },
            at("2026-10-27T10:00:00Z"),
        )
        .unwrap()
        .thread
        .id;
    let apply = |thread: &str, key: &str, decision: &str, ts: &str| {
        let ctx = AuditContext {
            actor: ChatActor::Cos,
            thread_id: thread.into(),
            run_id: "run-relay".into(),
            operation_id: ulid::Ulid::new().to_string(),
            reason: "人の指示".into(),
            policy_version: "5".into(),
        };
        let payload = serde_json::json!({"instructed_by": {"message_id": "m-relay",
            "decided_by": {"kind": "human", "id": null,
                "relayed_by": {"kind": "external_operator", "id": "claude-code"}}}});
        s.cos_operation_apply_at(
            &ctx,
            key,
            "h",
            "decision",
            decision,
            None,
            "decision.answer",
            &payload,
            at(ts),
            |_, _| Ok(serde_json::json!({})),
        )
        .unwrap()
    };
    let op = apply(&thread, "k1", "D9", "2026-10-27T10:05:00Z");
    apply(&other, "k2", "D-other", "2026-10-27T10:06:00Z");
    let ledger = s
        .request_ledger(at("2026-10-27T09:00:00Z"), at("2026-10-27T11:00:00Z"))
        .unwrap();
    assert_eq!(ledger.requests.len(), 1);
    let row = &ledger.requests[0];
    assert_eq!(row.operations, vec![op.id]);
    for r in [
        "instructed_by:m-relay",
        "relay:external_operator:claude-code",
        "cos_operation_policy:5",
    ] {
        assert!(
            row.policy_refs.iter().any(|p| p == r),
            "{r}: {:?}",
            row.policy_refs
        );
    }
}
