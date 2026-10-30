//! ADR 2026-10-30-cos-review-open-waits D4: the store side of re-judging an escalated plan
//! approval after the person answers the task's decision. Injected times only.
use super::triage::CosTriageSource;
use super::{ChatOpenThreadRequest, ChatRunState};
use crate::store::SqliteStore;
use time::OffsetDateTime;

fn at(n: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_791_158_400 + n).expect("time")
}

fn rfc3339(t: OffsetDateTime) -> String {
    t.format(&time::format_description::well_known::Rfc3339)
        .expect("rfc3339")
}

fn decision(s: &SqliteStore, id: &str, status: &str, answered: Option<i64>) {
    s.lock()
        .expect("lock")
        .execute(
            "INSERT OR REPLACE INTO decisions(id,root_id,task_id,key,kind,status,json,created_at,answered_at) \
             VALUES(?1,'T1','T1',?1,'choice',?2,'{}',?3,?4)",
            rusqlite::params![id, status, rfc3339(at(0)), answered.map(|n| rfc3339(at(n)))],
        )
        .expect("decision");
}

#[test]
fn answered_decision_hands_the_escalated_plan_approval_to_cos_once() {
    let s = SqliteStore::open_in_memory().expect("store");
    let thread = s
        .cos_thread_open(&ChatOpenThreadRequest { project_id: None }, at(0))
        .expect("thread")
        .thread
        .id;
    // 11:43:36Z の筋: 決定が未回答のまま一次対応が計画の承認を人に回す。
    decision(&s, "iso-boundary", "open", None);
    s.cos_triage_ingest_batch(
        "events",
        "1",
        &[CosTriageSource {
            source_kind: "plan_gate".into(),
            source_key: "plan_gate-T1-v1".into(),
            source_revision: "r1".into(),
            source_event_id: None,
            operation_id: None,
            summary: "計画の承認".into(),
            detail: String::new(),
            policy_version: "10".into(),
            task_id: None,
        }],
        at(1),
    )
    .expect("ingest");
    let claim = s
        .cos_triage_claim(&thread, "run-triage", at(2))
        .expect("claim")
        .expect("claimed");
    let item = claim.item_ids[0].clone();
    assert!(
        s.cos_triage_resolve(&item, "escalated", None, Some("未回答の決定がある"), at(3))
            .expect("resolve")
    );
    s.chat_run_finish("run-triage", ChatRunState::Completed, None, None, at(4))
        .expect("finish");
    assert!(
        s.cos_triage_plan_rejudge_candidates()
            .expect("candidates")
            .is_empty(),
        "the decision is still open"
    );
    // 11:43:38Z: 人が決定に答える。
    decision(&s, "iso-boundary", "answered", Some(5));
    let candidates = s.cos_triage_plan_rejudge_candidates().expect("candidates");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].item_id, item);
    let rejudge = s
        .cos_triage_plan_rejudge_claim(&thread, "run-rejudge", &[item.clone()], at(6))
        .expect("claim")
        .expect("handed to CoS");
    assert_eq!(rejudge.item_ids, vec![item.clone()]);
    // CoS が §3 (f) で再判定して approve すると、項目は閉じる（決定は人が答えたまま）。
    assert!(
        s.cos_triage_resolve(
            &item,
            "answered",
            None,
            Some("決定は回答済みで計画は決定に沿う"),
            at(7)
        )
        .expect("resolve")
    );
    s.chat_run_finish("run-rejudge", ChatRunState::Completed, None, None, at(8))
        .expect("finish");
    assert!(
        s.cos_triage_plan_rejudge_candidates()
            .expect("candidates")
            .is_empty()
    );
}

#[test]
fn a_rejudged_item_escalated_again_is_not_handed_over_twice() {
    let s = SqliteStore::open_in_memory().expect("store");
    let thread = s
        .cos_thread_open(&ChatOpenThreadRequest { project_id: None }, at(0))
        .expect("thread")
        .thread
        .id;
    decision(&s, "d1", "answered", Some(0));
    s.cos_triage_ingest_batch(
        "events",
        "1",
        &[CosTriageSource {
            source_kind: "plan_gate".into(),
            source_key: "plan_gate-T1-v1".into(),
            source_revision: "r1".into(),
            source_event_id: None,
            operation_id: None,
            summary: "計画の承認".into(),
            detail: String::new(),
            policy_version: "10".into(),
            task_id: None,
        }],
        at(1),
    )
    .expect("ingest");
    let claim = s
        .cos_triage_claim(&thread, "run-triage", at(2))
        .expect("claim")
        .expect("claimed");
    let item = claim.item_ids[0].clone();
    s.cos_triage_resolve(
        &item,
        "escalated",
        None,
        Some("計画が決定に沿わない"),
        at(3),
    )
    .expect("resolve");
    // 判定より前に答えた決定では渡さない（CoS はその回答を見て人に回した）。
    assert!(
        s.cos_triage_plan_rejudge_candidates()
            .expect("candidates")
            .is_empty()
    );
}
