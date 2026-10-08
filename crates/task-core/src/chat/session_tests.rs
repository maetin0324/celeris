//! CoS chat thread sessions on `node_sessions` (ADR 2026-10-05 D1/D2). Injected clock, no sleeps.

use serde_json::json;
use time::OffsetDateTime;

use super::*;
use crate::model::TaskId;
use crate::node_session::{NodeSession, NodeSessionStore, SessionKind, WorkUnitSession};
use crate::store::SqliteStore;

fn at(secs: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_791_158_400 + secs).expect("valid ts")
}

fn thread(s: &SqliteStore, key: &str) -> String {
    s.chat_thread_create(
        "admin",
        &ChatCreateThreadRequest {
            title: key.into(),
            project_id: None,
            client_thread_id: key.into(),
        },
        at(0),
    )
    .expect("thread")
    .thread
    .id
}

fn post_and_claim(s: &SqliteStore, t: &str, run: &str, now: i64) -> ChatRun {
    s.chat_message_post(
        t,
        &ChatPostMessageRequest {
            client_message_id: format!("m-{run}"),
            text: "hi".into(),
            attachment_ids: vec![],
            reply_to_id: None,
            mode: ChatSendMode::Queue,
            resume_queue: false,
        },
        at(now),
    )
    .expect("post");
    s.chat_run_claim_next(
        t,
        run,
        &json!({"harness":"claude-code","model":"m1"}),
        at(now),
    )
    .expect("claim")
    .expect("claimed")
}

fn key(t: &str) -> ChatSessionKey {
    ChatSessionKey {
        thread_id: t.into(),
        harness: "claude-code".into(),
        provider: Some("claude-pool".into()),
        llm_source: Some("claude_oauth".into()),
        account_id: Some("a1".into()),
        cwd: Some(format!("/data/cos/threads/{t}/workspace")),
        model: Some("m1".into()),
    }
}

#[test]
fn cos_chat_run_session_rotate_keeps_one_live_row_per_thread() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = thread(&s, "t1");
    let other = thread(&s, "t2");
    assert_eq!(s.chat_session_active(&t).expect("active"), None);

    let first = ChatSession::new(key(&t), "11111111-1111-4111-8111-111111111111", at(1));
    assert!(!s.chat_session_rotate(&first, at(1)).expect("create"));
    let got = s.chat_session_active(&t).expect("active").expect("live");
    assert_eq!(got.key, key(&t));
    assert_eq!(got.session_id, first.session_id);

    let other_row = ChatSession::new(key(&other), "", at(1));
    s.chat_session_rotate(&other_row, at(1))
        .expect("other thread");

    let mut k2 = key(&t);
    k2.model = Some("m2".into());
    let second = ChatSession::new(k2.clone(), "22222222-2222-4222-8222-222222222222", at(2));
    assert!(s.chat_session_rotate(&second, at(2)).expect("rotate"));
    let live = s.chat_session_active(&t).expect("active").expect("live");
    assert_eq!(live.id, second.id);
    assert_eq!(live.key, k2);
    let old = s.chat_session_get(&first.id).expect("get").expect("row");
    assert_eq!(old.retired_at, Some(at(2)));
    // The other thread's session is untouched.
    let o = s
        .chat_session_active(&other)
        .expect("active")
        .expect("live");
    assert_eq!(o.id, other_row.id);

    // Rotating into a missing thread fails without retiring anything.
    let ghost = ChatSession::new(key("nope"), "", at(3));
    assert!(s.chat_session_rotate(&ghost, at(3)).is_err());

    assert!(s.chat_session_retire(&t, at(4)).expect("retire"));
    assert!(!s.chat_session_retire(&t, at(4)).expect("retire again"));
    assert_eq!(s.chat_session_active(&t).expect("active"), None);
}

#[test]
fn cos_chat_run_session_touch_set_id_and_summary_watermark() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = thread(&s, "t1");
    let mut k = key(&t);
    k.harness = "codex".into();
    let row = ChatSession::new(k, "", at(1));
    s.chat_session_rotate(&row, at(1)).expect("create");
    assert!(
        s.chat_session_set_id(&row.id, "thread-abc")
            .expect("set id")
    );
    assert!(
        s.chat_session_touch(
            &row.id,
            ChatSessionUsage {
                add_tokens: 1200,
                ..Default::default()
            },
            at(5)
        )
        .expect("touch")
    );
    assert!(
        s.chat_session_touch(
            &row.id,
            ChatSessionUsage {
                add_tokens: -5,
                ..Default::default()
            },
            at(6)
        )
        .expect("touch")
    );
    assert!(s.chat_session_set_summary_through(&row.id, 7).expect("wm"));
    assert!(s.chat_session_set_summary_through(&row.id, 3).expect("wm"));
    let got = s.chat_session_active(&t).expect("active").expect("live");
    assert_eq!(got.session_id, "thread-abc");
    assert_eq!(got.turns, 2);
    assert_eq!(got.approx_tokens, 1200);
    assert_eq!(got.last_used_at, at(6));
    assert_eq!(got.summary_through_seq, 7);

    s.chat_session_retire(&t, at(7)).expect("retire");
    assert!(
        !s.chat_session_touch(
            &row.id,
            ChatSessionUsage {
                add_tokens: 1,
                ..Default::default()
            },
            at(8)
        )
        .expect("touch retired")
    );
    assert!(!s.chat_session_set_id(&row.id, "x").expect("set id retired"));
}

#[test]
fn cos_chat_run_session_mode_is_recorded_and_shown_in_run() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = thread(&s, "t1");
    let run = post_and_claim(&s, &t, "r1", 1);
    assert_eq!(run.session_mode, None);
    let row = ChatSession::new(key(&t), "", at(1));
    s.chat_session_rotate(&row, at(1)).expect("create");

    let recorded = s
        .chat_run_record_session(
            "r1",
            ChatRunSessionMode::FreshAfterRefusal,
            &row.id,
            Some("resume_refused"),
            at(2),
        )
        .expect("record");
    assert_eq!(recorded.session_mode, Some(ChatSessionMode::Fresh));
    // Resolved config written at claim time is kept.
    assert_eq!(recorded.harness.as_deref(), Some("claude-code"));
    assert_eq!(recorded.model.as_deref(), Some("m1"));

    let got = s.chat_run_get(&t, "r1").expect("get");
    assert_eq!(got.session_mode, Some(ChatSessionMode::Fresh));
    let wire = serde_json::to_value(&got).expect("json");
    assert_eq!(wire["session_mode"], "fresh");
    assert_eq!(
        s.chat_run_session_record("r1").expect("record"),
        ChatRunSessionRecord {
            session_row_id: Some(row.id.clone()),
            detail: Some("fresh_after_refusal".into()),
            reason: Some("resume_refused".into()),
        }
    );

    // The run event carries the mode for SSE.
    let events = s
        .chat_events_page(&t, &ChatEventQuery::default())
        .expect("events");
    let last = events.items.last().expect("event");
    match &last.data {
        ChatEventData::Run(d) => assert_eq!(d.run.session_mode, Some(ChatSessionMode::Fresh)),
        other => panic!("unexpected {other:?}"),
    }

    // Each detailed mode maps to its wire value.
    for (mode, wire) in [
        (ChatRunSessionMode::New, "new"),
        (ChatRunSessionMode::Resumed, "resumed"),
        (ChatRunSessionMode::Fresh, "fresh"),
    ] {
        s.chat_run_record_session("r1", mode, &row.id, None, at(3))
            .expect("record");
        let v = serde_json::to_value(s.chat_run_get(&t, "r1").expect("get")).expect("json");
        assert_eq!(v["session_mode"], wire);
    }
}

#[test]
fn cos_chat_run_session_record_rejects_foreign_row_and_finished_run() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = thread(&s, "t1");
    let other = thread(&s, "t2");
    post_and_claim(&s, &t, "r1", 1);
    let foreign = ChatSession::new(key(&other), "", at(1));
    s.chat_session_rotate(&foreign, at(1)).expect("create");
    assert!(
        s.chat_run_record_session("r1", ChatRunSessionMode::New, &foreign.id, None, at(2))
            .is_err()
    );
    let err = s
        .chat_run_record_session("r1", ChatRunSessionMode::New, "missing", None, at(2))
        .expect_err("missing row");
    assert_eq!(err.http_status(), 404);

    let mine = ChatSession::new(key(&t), "", at(1));
    s.chat_session_rotate(&mine, at(1)).expect("create");
    s.chat_run_finish("r1", ChatRunState::Completed, Some("ok"), None, at(3))
        .expect("finish");
    let err = s
        .chat_run_record_session("r1", ChatRunSessionMode::New, &mine.id, None, at(4))
        .expect_err("finished");
    assert_eq!(err.http_status(), 409);
    assert_eq!(s.chat_run_get(&t, "r1").expect("get").session_mode, None);
}

/// Criterion 2: cos_chat rows do not leak into the existing conversation / continuation APIs,
/// and those APIs do not touch cos_chat rows.
#[test]
fn cos_chat_run_session_does_not_disturb_conversation_and_continuation_rows() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = thread(&s, "t1");
    let conv = NodeSession::new(
        "cos",
        SessionKind::Conversation,
        None,
        "claude-code",
        Some("a1".into()),
        "33333333-3333-4333-8333-333333333333",
        at(1),
    );
    s.node_session_create(&conv).expect("conv");
    let task = TaskId::new();
    let wu = WorkUnitSession::new(
        "engineering",
        task,
        Some("wu".into()),
        "claude-code",
        Some("a1".into()),
        None,
        None,
        "44444444-4444-4444-8444-444444444444",
        at(1),
    );
    s.work_unit_session_create(&wu).expect("wu");
    let chat = ChatSession::new(key(&t), "55555555-5555-4555-8555-555555555555", at(2));
    s.chat_session_rotate(&chat, at(2)).expect("chat");

    let active = s
        .node_session_active("cos", SessionKind::Conversation, None)
        .expect("active")
        .expect("conv live");
    assert_eq!(active.id, conv.id);
    assert!(
        s.node_session_retire("cos", SessionKind::Conversation, None, at(3))
            .expect("retire")
    );
    assert!(s.chat_session_active(&t).expect("chat").is_some());
    assert!(
        s.work_unit_session_current(task, Some("wu"))
            .expect("wu")
            .is_some()
    );
    s.chat_session_retire(&t, at(4)).expect("retire chat");
    assert!(
        s.work_unit_session_current(task, Some("wu"))
            .expect("wu")
            .is_some()
    );
}

#[test]
fn cos_chat_usage_store_unknown_metrics_and_idempotent_finish() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("usage.db");
    let s = SqliteStore::open(&db).expect("store");
    let t = thread(&s, "usage");
    let claimed = post_and_claim(&s, &t, "run", 0);
    assert!(
        serde_json::to_value(&claimed)
            .expect("json")
            .get("usage")
            .is_none()
    );
    let usage = crate::Usage {
        input_tokens: Some(4),
        output_tokens: Some(2),
        ..Default::default()
    };
    let run = s
        .chat_run_finish_with_telemetry(
            "run",
            ChatRunState::Failed,
            None,
            Some("failed"),
            at(3),
            Some(&usage),
            0,
            None,
        )
        .expect("finish");
    assert_eq!(run.usage.as_deref(), Some(&usage));
    assert_eq!(run.latency_ms, Some(3000));
    assert_eq!(run.time_to_first_output_ms, None);
    let wire = serde_json::to_value(&run).expect("wire");
    assert!(wire["usage"].get("cache_read_tokens").is_none());
    assert!(wire["usage"].get("cache_creation_tokens").is_none());
    assert!(wire["usage"].get("cost_usd").is_none());
    assert!(wire["usage"].get("session_resumed").is_none());
    let replay = s
        .chat_run_finish_with_telemetry(
            "run",
            ChatRunState::Failed,
            None,
            None,
            at(9),
            None,
            99,
            Some(at(2)),
        )
        .expect("replay");
    assert_eq!(replay, run);
    let reopened = SqliteStore::open(&db).expect("reopen");
    assert_eq!(reopened.chat_run_get(&t, "run").expect("persisted"), run);
    let other = thread(&s, "other");
    assert!(s.chat_run_get(&other, "run").is_err());
    assert!(s.chat_run_list(&other, Some("run"), Some(1)).is_err());
}

#[test]
fn cos_chat_run_session_touch_separates_occupancy_from_cumulative() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = thread(&s, "t1");
    let row = ChatSession::new(key(&t), "", at(1));
    s.chat_session_rotate(&row, at(1)).expect("create");
    let usage = |add, billed, ctx| ChatSessionUsage {
        add_tokens: add,
        billed_input: billed,
        context_tokens: ctx,
    };
    s.chat_session_touch(&row.id, usage(15, 600, Some(600)), at(2))
        .expect("touch");
    s.chat_session_touch(&row.id, usage(15, 400, Some(400)), at(3))
        .expect("touch");
    let got = s.chat_session_active(&t).expect("active").expect("live");
    assert_eq!(got.approx_tokens, 30);
    assert_eq!(got.billed_input_tokens, 1000);
    // Occupancy is replaced by the latest observation, not summed.
    assert_eq!(got.last_context_tokens, Some(400));
    // A run that reports no occupancy keeps the last known one.
    s.chat_session_touch(&row.id, usage(5, 50, None), at(4))
        .expect("touch");
    let got = s.chat_session_active(&t).expect("active").expect("live");
    assert_eq!(got.last_context_tokens, Some(400));
    assert_eq!(got.approx_tokens, 35);
}

#[test]
fn cos_chat_resume_delta_cursor_rises_only_on_live_row_and_is_independent_of_summary() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = thread(&s, "t1");
    let row = ChatSession::new(key(&t), "11111111-1111-4111-8111-111111111111", at(1));
    s.chat_session_rotate(&row, at(1)).expect("create");
    let live = s.chat_session_active(&t).expect("active").expect("live");
    assert_eq!(live.delivered_through_seq, 0, "a new row knows nothing");

    assert!(
        s.chat_session_set_delivered_through(&row.id, 6)
            .expect("cursor")
    );
    assert!(
        s.chat_session_set_delivered_through(&row.id, 4)
            .expect("cursor")
    );
    let live = s.chat_session_active(&t).expect("active").expect("live");
    assert_eq!(live.delivered_through_seq, 6, "the cursor never goes back");
    assert_eq!(live.summary_through_seq, 0, "the cursor is not the summary");

    // The checkpoint watermark moves alone.
    assert!(s.chat_session_set_summary_through(&row.id, 9).expect("wm"));
    let live = s.chat_session_active(&t).expect("active").expect("live");
    assert_eq!(
        (live.summary_through_seq, live.delivered_through_seq),
        (9, 6)
    );

    // A fresh row starts at 0 and a retired row is not advanced.
    let fresh = ChatSession::new(key(&t), "22222222-2222-4222-8222-222222222222", at(2));
    s.chat_session_rotate(&fresh, at(2)).expect("rotate");
    assert!(
        !s.chat_session_set_delivered_through(&row.id, 20)
            .expect("retired")
    );
    let old = s.chat_session_get(&row.id).expect("get").expect("row");
    assert_eq!(old.delivered_through_seq, 6);
    let live = s.chat_session_active(&t).expect("active").expect("live");
    assert_eq!(live.delivered_through_seq, 0);
}
