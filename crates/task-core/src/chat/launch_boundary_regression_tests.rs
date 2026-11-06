//! ADR 2026-11-06-chat-run-launch-boundary-store: 実装前に現行実装で落ちることを確かめた回帰条件
//! （永続列の有無と、claim が境界の印を付けること）。実装後は同じ試験が通る。

use serde_json::json;
use time::OffsetDateTime;

use super::*;
use crate::store::SqliteStore;

fn at(secs: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_791_158_400 + secs).expect("valid ts")
}

#[test]
fn chat_runs_has_launch_boundary_columns() {
    let s = SqliteStore::open_in_memory().expect("store");
    let conn = super::store::writer(&s).expect("conn");
    let mut stmt = conn
        .prepare("SELECT name FROM pragma_table_info('chat_runs')")
        .expect("prepare");
    let cols: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows");
    for want in [
        "launched_at",
        "prelaunch_reason",
        "launch_tracked",
        "attempts_recorded",
        "attempts_usage_missing",
    ] {
        assert!(
            cols.iter().any(|c| c == want),
            "missing column {want}: {cols:?}"
        );
    }
}

#[test]
fn claim_marks_new_rows_as_launch_tracked_with_no_launch_time() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = crate::chat::test_project_thread(&s, "regression", at(0)).id;
    s.chat_message_post(
        &t,
        &ChatPostMessageRequest {
            client_message_id: "m1".into(),
            text: "hi".into(),
            attachment_ids: vec![],
            reply_to_id: None,
            mode: ChatSendMode::Queue,
            resume_queue: false,
        },
        at(1),
    )
    .expect("post");
    s.chat_run_claim_next(&t, "r1", &json!({}), at(1))
        .expect("claim")
        .expect("claimed");
    let conn = super::store::writer(&s).expect("conn");
    let (tracked, launched): (i64, Option<String>) = conn
        .query_row(
            "SELECT launch_tracked, launched_at FROM chat_runs WHERE run_id='r1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("columns exist");
    assert_eq!(tracked, 1);
    assert_eq!(launched, None);
}

// ---- 実装後の契約試験（一時 SQLite・注入時刻。sleep なし）----

use crate::Usage;

const ALL_REASONS: [(&str, ChatPrelaunchReason); 6] = [
    ("route_unavailable", ChatPrelaunchReason::RouteUnavailable),
    ("skills_unavailable", ChatPrelaunchReason::SkillsUnavailable),
    (
        "wall_expired_before_launch",
        ChatPrelaunchReason::WallExpiredBeforeLaunch,
    ),
    ("slot_wait_expired", ChatPrelaunchReason::SlotWaitExpired),
    (
        "daemon_restart_before_launch",
        ChatPrelaunchReason::DaemonRestartBeforeLaunch,
    ),
    (
        "stopped_before_launch",
        ChatPrelaunchReason::StoppedBeforeLaunch,
    ),
];

fn store() -> SqliteStore {
    SqliteStore::open_in_memory().expect("store")
}

fn claim(s: &SqliteStore, thread: &str, run: &str, secs: i64) -> ChatRun {
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
        .expect("claimed")
}

fn run_events(s: &SqliteStore, thread: &str, run: &str) -> usize {
    s.chat_events_page(
        thread,
        &ChatEventQuery {
            run_id: Some(run.into()),
            limit: Some(500),
            ..Default::default()
        },
    )
    .expect("events")
    .items
    .iter()
    .filter(|e| e.event_type == ChatEventType::Run)
    .count()
}

fn usage(input: u64, cost: Option<f64>) -> Usage {
    Usage {
        input_tokens: Some(input),
        output_tokens: Some(1),
        cost_usd: cost,
        ..Usage::default()
    }
}

fn thread(s: &SqliteStore, key: &str) -> String {
    crate::chat::test_project_thread(s, key, at(0)).id
}

#[test]
fn launched_at_is_written_once_and_resend_appends_nothing() {
    let s = store();
    let t = thread(&s, "once");
    claim(&s, &t, "r1", 1);
    let first = s.chat_run_mark_launched("r1", at(10)).expect("first");
    assert_eq!(
        first.launched_at.as_deref(),
        Some("2026-10-05T00:00:10.000Z")
    );
    let events = run_events(&s, &t, "r1");
    let again = s.chat_run_mark_launched("r1", at(99)).expect("resend");
    assert_eq!(again.launched_at, first.launched_at, "first time kept");
    assert_eq!(run_events(&s, &t, "r1"), events, "no event on resend");
}

#[test]
fn six_reasons_roundtrip_and_unknown_reason_is_rejected() {
    let s = store();
    let t = thread(&s, "reasons");
    for (i, (text, reason)) in ALL_REASONS.iter().enumerate() {
        let id = format!("r{i}");
        claim(&s, &t, &id, 1 + 2 * i as i64);
        let run = s
            .chat_run_finish_prelaunch(
                &id,
                ChatRunState::Failed,
                *reason,
                Some("why"),
                at(50 + i as i64),
            )
            .expect("finish");
        assert_eq!(run.prelaunch_reason, Some(*reason));
        assert_eq!(run.cost_state, ChatCostState::NotLaunched);
        assert_eq!(run.launched_at, None);
        let stored: String = super::store::writer(&s)
            .expect("conn")
            .query_row(
                "SELECT prelaunch_reason FROM chat_runs WHERE run_id=?1",
                [&id],
                |r| r.get(0),
            )
            .expect("row");
        assert_eq!(&stored, text);
        assert_eq!(ChatPrelaunchReason::parse(text).expect("parse"), *reason);
    }
    for bad in ["", "Route_Unavailable", "route unavailable", "nope"] {
        assert!(matches!(
            ChatPrelaunchReason::parse(bad),
            Err(ChatError::Invalid(_))
        ));
    }
}

#[test]
fn unlisted_stored_reason_and_legacy_rows_are_unknown_not_not_launched() {
    let s = store();
    let t = thread(&s, "closed-set");
    claim(&s, &t, "bad", 1);
    claim_finish_plain(&s, "bad", 5);
    {
        let conn = super::store::writer(&s).expect("conn");
        conn.execute(
            "UPDATE chat_runs SET prelaunch_reason='made_up' WHERE run_id='bad'",
            [],
        )
        .expect("update");
    }
    let run = s.chat_run_get(&t, "bad").expect("get");
    assert_eq!(run.prelaunch_reason, None);
    assert_eq!(run.cost_state, ChatCostState::Unknown);

    claim(&s, &t, "legacy", 20);
    claim_finish_plain(&s, "legacy", 25);
    {
        let conn = super::store::writer(&s).expect("conn");
        conn.execute(
            "UPDATE chat_runs SET launch_tracked=0, prelaunch_reason='route_unavailable' WHERE run_id='legacy'",
            [],
        )
        .expect("update");
    }
    assert_eq!(
        s.chat_run_get(&t, "legacy").expect("get").cost_state,
        ChatCostState::Unknown,
        "an untracked row never reads as not_launched"
    );
}

fn claim_finish_plain(s: &SqliteStore, run: &str, secs: i64) {
    s.chat_run_finish(run, ChatRunState::Failed, None, Some("boom"), at(secs))
        .expect("finish");
}

#[test]
fn launched_run_cannot_end_with_prelaunch_reason_and_nothing_is_written() {
    let s = store();
    let t = thread(&s, "launched");
    claim(&s, &t, "r1", 1);
    s.chat_run_mark_launched("r1", at(2)).expect("launch");
    let err = s
        .chat_run_finish_prelaunch(
            "r1",
            ChatRunState::Failed,
            ChatPrelaunchReason::RouteUnavailable,
            None,
            at(3),
        )
        .expect_err("rejected");
    assert!(matches!(err, ChatError::Conflict(_)));
    let run = s.chat_run_get(&t, "r1").expect("get");
    assert_eq!(run.state, ChatRunState::Running);
    assert_eq!(run.prelaunch_reason, None);
    // A run that never started cannot complete.
    let t2 = thread(&s, "launched-2");
    claim(&s, &t2, "r2", 10);
    assert!(matches!(
        s.chat_run_finish_prelaunch(
            "r2",
            ChatRunState::Completed,
            ChatPrelaunchReason::RouteUnavailable,
            None,
            at(11)
        ),
        Err(ChatError::Invalid(_))
    ));
}

#[test]
fn terminal_rows_reject_late_launch_and_resend_is_idempotent_or_conflict() {
    let s = store();
    let t = thread(&s, "terminal");
    claim(&s, &t, "r1", 1);
    let done = s
        .chat_run_finish_prelaunch(
            "r1",
            ChatRunState::Failed,
            ChatPrelaunchReason::SlotWaitExpired,
            Some("slot"),
            at(2),
        )
        .expect("finish");
    let events = run_events(&s, &t, "r1");
    let same = s
        .chat_run_finish_prelaunch(
            "r1",
            ChatRunState::Failed,
            ChatPrelaunchReason::SlotWaitExpired,
            Some("slot"),
            at(9),
        )
        .expect("resend");
    assert_eq!(same.finished_at, done.finished_at);
    assert_eq!(run_events(&s, &t, "r1"), events);
    assert!(matches!(
        s.chat_run_finish_prelaunch(
            "r1",
            ChatRunState::Failed,
            ChatPrelaunchReason::RouteUnavailable,
            None,
            at(9)
        ),
        Err(ChatError::Conflict(_))
    ));
    assert!(matches!(
        s.chat_run_mark_launched("r1", at(9)),
        Err(ChatError::Conflict(_))
    ));
    assert_eq!(s.chat_run_get(&t, "r1").expect("get").launched_at, None);
}

#[test]
fn stopping_run_must_not_launch() {
    let s = store();
    let t = thread(&s, "stopping");
    claim(&s, &t, "r1", 1);
    s.chat_run_stop(&t, "r1", at(2)).expect("stop");
    assert!(matches!(
        s.chat_run_mark_launched("r1", at(3)),
        Err(ChatError::Conflict(_))
    ));
}

#[test]
fn recovery_branches_split_new_rows_from_launched_and_legacy_rows() {
    for (cause, state, prelaunch) in [
        (
            ChatRecoveryCause::DaemonRestart,
            ChatRunState::Interrupted,
            ChatPrelaunchReason::DaemonRestartBeforeLaunch,
        ),
        (
            ChatRecoveryCause::HumanStop,
            ChatRunState::Stopped,
            ChatPrelaunchReason::StoppedBeforeLaunch,
        ),
    ] {
        let s = store();
        let t = thread(&s, "recover");
        // New row, never launched → closed prelaunch, not_launched.
        claim(&s, &t, "new", 1);
        let r = s
            .chat_run_recover("new", cause, "recovered", at(5))
            .expect("recover");
        assert_eq!(
            (r.state, r.prelaunch_reason, r.cost_state),
            (state, Some(prelaunch), ChatCostState::NotLaunched)
        );
        // Launched row → plain close, usage missing stays unknown.
        claim(&s, &t, "launched", 10);
        s.chat_run_mark_launched("launched", at(11))
            .expect("launch");
        let r = s
            .chat_run_recover("launched", cause, "recovered", at(15))
            .expect("recover");
        assert_eq!(
            (r.state, r.prelaunch_reason, r.cost_state),
            (state, None, ChatCostState::Unknown)
        );
        assert!(r.launched_at.is_some());
        // Legacy row (launch_tracked=0, launched_at NULL) → not misread as prelaunch.
        claim(&s, &t, "legacy", 20);
        super::store::writer(&s)
            .expect("conn")
            .execute(
                "UPDATE chat_runs SET launch_tracked=0 WHERE run_id='legacy'",
                [],
            )
            .expect("update");
        let r = s
            .chat_run_recover("legacy", cause, "recovered", at(25))
            .expect("recover");
        assert_eq!(
            (r.state, r.prelaunch_reason, r.cost_state),
            (state, None, ChatCostState::Unknown)
        );
        // Recovery of a finished run changes nothing.
        let again = s
            .chat_run_recover("legacy", cause, "again", at(99))
            .expect("again");
        assert_eq!(again.finished_at, r.finished_at);
    }
}

#[test]
fn attempt_usage_known_then_missing_keeps_known_and_marks_unknown() {
    let s = store();
    let t = thread(&s, "k2m");
    claim(&s, &t, "r1", 1);
    s.chat_run_mark_launched("r1", at(2)).expect("launch");
    s.chat_run_record_attempt_usage("r1", Some(&usage(100, Some(0.5))))
        .expect("a1");
    s.chat_run_record_attempt_usage("r1", None).expect("a2");
    // Final telemetry carrying a (partial) usage or none must neither erase the mark nor the known usage.
    s.chat_run_finish_with_telemetry(
        "r1",
        ChatRunState::Completed,
        Some("ok"),
        None,
        at(9),
        None,
        0,
        None,
    )
    .expect("finish");
    let run = s.chat_run_get(&t, "r1").expect("get");
    assert_eq!(run.usage.as_deref().and_then(|u| u.input_tokens), Some(100));
    assert_eq!(run.cost_state, ChatCostState::Unknown);
}

#[test]
fn attempt_usage_missing_then_known_and_multiple_known_sum() {
    let s = store();
    let t = thread(&s, "m2k");
    claim(&s, &t, "r1", 1);
    s.chat_run_mark_launched("r1", at(2)).expect("launch");
    s.chat_run_record_attempt_usage("r1", None).expect("a1");
    s.chat_run_record_attempt_usage("r1", Some(&usage(7, Some(0.25))))
        .expect("a2");
    s.chat_run_finish_with_telemetry(
        "r1",
        ChatRunState::Completed,
        None,
        None,
        at(9),
        Some(&usage(999, Some(9.0))),
        0,
        None,
    )
    .expect("finish");
    let run = s.chat_run_get(&t, "r1").expect("get");
    assert_eq!(
        run.usage.as_deref().and_then(|u| u.input_tokens),
        Some(7),
        "attempt records stay authoritative"
    );
    assert_eq!(run.cost_state, ChatCostState::Unknown);

    claim(&s, &t, "r2", 20);
    s.chat_run_mark_launched("r2", at(21)).expect("launch");
    s.chat_run_record_attempt_usage("r2", Some(&usage(10, Some(0.5))))
        .expect("a1");
    s.chat_run_record_attempt_usage("r2", Some(&usage(20, Some(0.25))))
        .expect("a2");
    s.chat_run_finish_with_telemetry(
        "r2",
        ChatRunState::Completed,
        None,
        None,
        at(30),
        None,
        0,
        None,
    )
    .expect("finish");
    let run = s.chat_run_get(&t, "r2").expect("get");
    let u = run.usage.as_deref().expect("usage");
    assert_eq!((u.input_tokens, u.cost_usd), (Some(30), Some(0.75)));
    assert_eq!(run.cost_state, ChatCostState::Known);
}

#[test]
fn attempt_without_launch_is_rejected_and_telemetry_without_attempts_is_unchanged() {
    let s = store();
    let t = thread(&s, "nolaunch");
    claim(&s, &t, "r1", 1);
    assert!(matches!(
        s.chat_run_record_attempt_usage("r1", None),
        Err(ChatError::Conflict(_))
    ));
    // Legacy path (no attempts recorded): final telemetry still writes the passed usage.
    s.chat_run_mark_launched("r1", at(2)).expect("launch");
    s.chat_run_finish_with_telemetry(
        "r1",
        ChatRunState::Completed,
        None,
        None,
        at(5),
        Some(&usage(3, Some(0.1))),
        0,
        None,
    )
    .expect("finish");
    let run = s.chat_run_get(&t, "r1").expect("get");
    assert_eq!(run.cost_state, ChatCostState::Known);
}

#[test]
fn launch_state_survives_reopen_and_old_run_events_read_as_unknown() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("celeris.db");
    let t;
    {
        let s = SqliteStore::open(&path).expect("open");
        t = thread(&s, "reopen");
        claim(&s, &t, "r1", 1);
        s.chat_run_mark_launched("r1", at(2)).expect("launch");
        s.chat_run_record_attempt_usage("r1", Some(&usage(5, Some(0.1))))
            .expect("a1");
        s.chat_run_record_attempt_usage("r1", None).expect("a2");
        s.chat_run_finish("r1", ChatRunState::Completed, None, None, at(4))
            .expect("finish");
    }
    let s = SqliteStore::open(&path).expect("reopen");
    let run = s.chat_run_get(&t, "r1").expect("get");
    assert_eq!(run.launched_at.as_deref(), Some("2026-10-05T00:00:02.000Z"));
    assert_eq!(run.cost_state, ChatCostState::Unknown);
    assert_eq!(run.usage.as_deref().and_then(|u| u.input_tokens), Some(5));
    // A run event written before the fields existed deserialises with cost_state = unknown.
    let mut value = serde_json::to_value(&run).expect("value");
    let obj = value.as_object_mut().expect("object");
    for k in ["launched_at", "prelaunch_reason", "cost_state"] {
        obj.remove(k);
    }
    let old: ChatRun = serde_json::from_value(value).expect("old event");
    assert_eq!(old.cost_state, ChatCostState::Unknown);
    assert_eq!(old.launched_at, None);
}

#[test]
fn migration_0120_keeps_rows_usage_credentials_and_events_without_guessing_a_boundary() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("celeris.db");
    let (t, events_before, credentials_before);
    {
        let s = SqliteStore::open(&path).expect("open");
        t = thread(&s, "mig");
        claim(&s, &t, "old", 1);
        s.chat_run_finish_with_telemetry(
            "old",
            ChatRunState::Failed,
            None,
            Some("boom"),
            at(3),
            Some(&usage(42, Some(1.5))),
            0,
            None,
        )
        .expect("finish");
        claim(&s, &t, "old-active", 10);
        s.cos_run_credential_issue_at(&t, "old-active", time::Duration::hours(1), at(10))
            .expect("credential");
        let conn = super::store::writer(&s).expect("conn");
        events_before = count(&conn, "SELECT COUNT(*) FROM chat_events");
        credentials_before = count(&conn, "SELECT COUNT(*) FROM cos_run_credentials");
        conn.execute_batch(
            "ALTER TABLE chat_runs DROP COLUMN launched_at; \
             ALTER TABLE chat_runs DROP COLUMN prelaunch_reason; \
             ALTER TABLE chat_runs DROP COLUMN launch_tracked; \
             ALTER TABLE chat_runs DROP COLUMN attempts_recorded; \
             ALTER TABLE chat_runs DROP COLUMN attempts_usage_missing; \
             DELETE FROM schema_migrations WHERE version = 120;",
        )
        .expect("downgrade fixture");
    }
    let s = SqliteStore::open(&path).expect("migrate");
    let conn = super::store::writer(&s).expect("conn");
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM chat_events"),
        events_before
    );
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM cos_run_credentials"),
        credentials_before
    );
    assert!(credentials_before >= 1);
    let (launched, reason, tracked): (Option<String>, Option<String>, i64) = conn
        .query_row(
            "SELECT launched_at, prelaunch_reason, launch_tracked FROM chat_runs WHERE run_id='old-active'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("row");
    assert_eq!((launched, reason, tracked), (None, None, 0));
    drop(conn);
    let old = s.chat_run_get(&t, "old").expect("get");
    assert_eq!(old.usage.as_deref().and_then(|u| u.input_tokens), Some(42));
    assert_eq!(old.cost_state, ChatCostState::Known);
    let active = s.chat_run_get(&t, "old-active").expect("get");
    assert_eq!(active.cost_state, ChatCostState::Unknown);
    // The old active row is recovered as boundary-unknown, never as a prelaunch close.
    let r = s
        .chat_run_recover(
            "old-active",
            ChatRecoveryCause::DaemonRestart,
            "restart",
            at(50),
        )
        .expect("recover");
    assert_eq!(
        (r.prelaunch_reason, r.cost_state),
        (None, ChatCostState::Unknown)
    );
}

fn count(conn: &rusqlite::Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).expect("count")
}
