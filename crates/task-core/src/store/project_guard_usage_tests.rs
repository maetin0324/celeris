//! ADR 2026-11-06-cos-run-launch-accounting-and-drain D1: 予算の和 `project_run_usage_between` と
//! 起動前除外数 `project_not_launched_runs_between` の回帰試験（一時 SQLite・注入時刻）。

use serde_json::json;
use time::OffsetDateTime;

use super::SqliteStore;
use crate::Usage;
use crate::chat::*;

fn at(secs: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_791_158_400 + secs).expect("valid ts")
}

fn claim(s: &SqliteStore, thread: &str, run: &str, secs: i64) {
    s.chat_message_post(
        thread,
        &ChatPostMessageRequest {
            client_message_id: format!("m-{run}"),
            text: "hi".into(),
            attachment_ids: vec![],
            reply_to_id: None,
            mode: ChatSendMode::Queue,
            resume_queue: false,
        },
        at(secs),
    )
    .expect("post");
    s.chat_run_claim_next(thread, run, &json!({"model":"m1"}), at(secs))
        .expect("claim")
        .expect("claimed");
}

fn usage(input: u64) -> Usage {
    Usage {
        input_tokens: Some(input),
        output_tokens: Some(1),
        ..Usage::default()
    }
}

fn conn_exec(s: &SqliteStore, sql: &str, run: &str) {
    s.lock()
        .expect("lock")
        .execute(sql, [run])
        .expect("update row");
}

const SINCE: &str = "2026-10-05T00:00:00Z";
const UNTIL: &str = "2026-10-06T00:00:00Z";

fn sum(s: &SqliteStore, p: &str) -> Vec<super::ProjectRunUsage> {
    s.project_run_usage_between(p, SINCE, UNTIL).expect("sum")
}

fn not_launched(s: &SqliteStore, p: &str) -> u32 {
    s.project_not_launched_runs_between(p, SINCE, UNTIL)
        .expect("count")
}

const REASONS: [ChatPrelaunchReason; 6] = [
    ChatPrelaunchReason::RouteUnavailable,
    ChatPrelaunchReason::SkillsUnavailable,
    ChatPrelaunchReason::WallExpiredBeforeLaunch,
    ChatPrelaunchReason::SlotWaitExpired,
    ChatPrelaunchReason::DaemonRestartBeforeLaunch,
    ChatPrelaunchReason::StoppedBeforeLaunch,
];

/// 修正前に失敗させた最小の回帰: 起動前に閉じた run（`prelaunch_reason` あり）が予算の和に残る。
#[test]
fn prelaunch_run_is_not_returned_by_usage_sum() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = crate::chat::test_project_thread(&s, "p1", at(0)).id;
    claim(&s, &t, "r1", 1);
    s.chat_run_finish_prelaunch(
        "r1",
        ChatRunState::Failed,
        ChatPrelaunchReason::RouteUnavailable,
        Some("no route"),
        at(2),
    )
    .expect("finish");
    let rows = sum(&s, "p1");
    assert!(
        rows.is_empty(),
        "prelaunch run must not be summed: {rows:?}"
    );
}

#[test]
fn six_reasons_are_excluded_and_counted_with_the_view_decision() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = crate::chat::test_project_thread(&s, "p1", at(0)).id;
    for (i, reason) in REASONS.iter().enumerate() {
        let id = format!("r{i}");
        claim(&s, &t, &id, 1 + 2 * i as i64);
        let run = s
            .chat_run_finish_prelaunch(&id, ChatRunState::Failed, *reason, None, at(100 + i as i64))
            .expect("finish");
        assert_eq!(run.cost_state, ChatCostState::NotLaunched);
    }
    assert!(sum(&s, "p1").is_empty());
    assert_eq!(not_launched(&s, "p1"), 6);
}

/// 表: 除外されない行（費用不明のまま残る・既知）と、除外件数が view の判定と一致すること。
#[test]
fn unknown_boundaries_stay_in_the_sum_as_unknown() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = crate::chat::test_project_thread(&s, "p1", at(0)).id;
    let mut n = 0;
    let mut next = |s: &SqliteStore| {
        n += 1;
        let id = format!("r{n}");
        claim(s, &t, &id, 10 * n);
        id
    };
    // 1 旧行（launch_tracked=0）: 起動前に終わった形でも除外しない。
    let id = next(&s);
    s.chat_run_finish_prelaunch(&id, ChatRunState::Failed, REASONS[0], None, at(1000))
        .expect("finish");
    conn_exec(
        &s,
        "UPDATE chat_runs SET launch_tracked=0 WHERE run_id=?1",
        &id,
    );
    // 2 未知 reason
    let id = next(&s);
    s.chat_run_finish_prelaunch(&id, ChatRunState::Failed, REASONS[1], None, at(1001))
        .expect("finish");
    conn_exec(
        &s,
        "UPDATE chat_runs SET prelaunch_reason='weird' WHERE run_id=?1",
        &id,
    );
    // 3 reason 無し・launched_at 無し
    let id = next(&s);
    s.chat_run_finish(&id, ChatRunState::Failed, None, Some("x"), at(1002))
        .expect("finish");
    // 4 launched_at あり・usage 無し（reason 無し）
    let id = next(&s);
    s.chat_run_mark_launched(&id, at(1003)).expect("launch");
    s.chat_run_finish_with_telemetry(
        &id,
        ChatRunState::Failed,
        None,
        None,
        at(1004),
        None,
        0,
        None,
    )
    .expect("finish");
    // 5 launched_at あり + 既知 usage（known）
    let id = next(&s);
    s.chat_run_mark_launched(&id, at(1005)).expect("launch");
    s.chat_run_finish_with_telemetry(
        &id,
        ChatRunState::Completed,
        None,
        None,
        at(1006),
        Some(&usage(100)),
        0,
        None,
    )
    .expect("finish");
    // 6 usage 破損（JSON として正しいが Usage ではない。列の CHECK が不正な JSON を拒む）
    let id = next(&s);
    s.chat_run_mark_launched(&id, at(1007)).expect("launch");
    s.chat_run_finish_with_telemetry(
        &id,
        ChatRunState::Failed,
        None,
        None,
        at(1008),
        None,
        0,
        None,
    )
    .expect("finish");
    conn_exec(
        &s,
        "UPDATE chat_runs SET usage_json='[1]' WHERE run_id=?1",
        &id,
    );
    // 7 attempt 欠測: 既知 usage を残し、予算用は不明
    let id = next(&s);
    s.chat_run_mark_launched(&id, at(1009)).expect("launch");
    s.chat_run_record_attempt_usage(&id, Some(&usage(7)))
        .expect("a1");
    s.chat_run_record_attempt_usage(&id, None).expect("a2");
    s.chat_run_finish_with_telemetry(
        &id,
        ChatRunState::Failed,
        None,
        None,
        at(1010),
        None,
        0,
        None,
    )
    .expect("finish");
    // 8 進行中（finished_at 無し）
    let _running = next(&s);

    let rows = sum(&s, "p1");
    assert_eq!(
        rows.len(),
        7,
        "running row excluded, the rest returned: {rows:?}"
    );
    assert_eq!(not_launched(&s, "p1"), 0);
    let known: Vec<_> = rows.iter().filter(|r| r.usage.is_some()).collect();
    assert_eq!(known.len(), 1, "only row 5 is known: {rows:?}");
    assert_eq!(
        known[0].usage.as_ref().and_then(|u| u.input_tokens),
        Some(100)
    );
    let missing = rows
        .iter()
        .find(|r| r.usage.is_none() && r.known_usage.is_some())
        .expect("attempt-missing row keeps its known usage");
    assert_eq!(
        missing.known_usage.as_ref().and_then(|u| u.input_tokens),
        Some(7)
    );
    // 既存の view は読めない usage の行を一覧ごとエラーにする（run_store の挙動。未解決事項に記録）。
    // view との整合は、その行の usage を NULL（同じ unknown）に戻して比べる。
    conn_exec(
        &s,
        "UPDATE chat_runs SET usage_json=NULL WHERE usage_json='[1]' AND run_id<>?1",
        "none",
    );
    // 件数 + 和の行 + 進行中 = 全 run（整合）、view の cost_state の not_launched 数と一致。
    let views = s.chat_run_list(&t, None, Some(50)).expect("runs").items;
    let view_nl = views
        .iter()
        .filter(|r| r.cost_state == ChatCostState::NotLaunched)
        .count();
    assert_eq!(view_nl as u32, not_launched(&s, "p1"));
    let finished = views.iter().filter(|r| r.finished_at.is_some()).count();
    assert_eq!(finished as u32, rows.len() as u32 + not_launched(&s, "p1"));
}

#[test]
fn other_project_and_period_boundaries_are_not_counted() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t1 = crate::chat::test_project_thread(&s, "p1", at(0)).id;
    let t2 = crate::chat::test_project_thread(&s, "p2", at(0)).id;
    // p2 の起動前 run は p1 の数に入らない。
    claim(&s, &t2, "other", 5);
    s.chat_run_finish_prelaunch("other", ChatRunState::Failed, REASONS[0], None, at(6))
        .expect("finish");
    // 期間の境界: started_at == since は含み、== until は含まない。
    let close = |id: &str, secs: i64| {
        claim(&s, &t1, id, secs);
        s.chat_run_finish_prelaunch(id, ChatRunState::Failed, REASONS[2], None, at(secs + 1))
            .expect("finish");
    };
    close("at-since", 0); // 2026-10-05T00:00:00.000Z（>= since）
    close("in", 3600);
    close("at-until", 86_400); // 2026-10-06T00:00:00.000Z（< until を満たさない）
    assert_eq!(not_launched(&s, "p1"), 2);
    assert_eq!(not_launched(&s, "p2"), 1);
    assert!(sum(&s, "p1").is_empty());
}
