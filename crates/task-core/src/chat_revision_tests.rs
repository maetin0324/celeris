//! ADR 2026-11-21-chat-discussion-mode D5 / D8.1 (WU edit-retract): a person edits or retracts
//! their own message; the previous body stays in `chat_events`, search forgets a retracted body,
//! and a summary or a live session that covered the message is dropped. Injected clock, no sleeps.

use serde_json::json;
use time::OffsetDateTime;

use crate::chat::*;
use crate::chat_revision::ChatRevisionError;
use crate::store::SqliteStore;

const ME: &str = "human:alice";

fn at(secs: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_791_158_400 + secs).expect("valid ts")
}

fn thread(s: &SqliteStore) -> String {
    crate::chat::test_project_thread(s, "p-revise", at(0)).id
}

fn post(s: &SqliteStore, t: &str, key: &str, text: &str, author: &str, now: i64) -> ChatMessage {
    s.chat_message_post_as(
        t,
        &ChatPostMessageRequest {
            client_message_id: key.into(),
            text: text.into(),
            attachment_ids: vec![],
            reply_to_id: None,
            mode: ChatSendMode::Queue,
            resume_queue: false,
            discussion_mode: None,
        },
        author,
        at(now),
    )
    .expect("post")
    .response
    .message
}

/// Claims the next queued input as `run` (the input becomes `running`).
fn claim(s: &SqliteStore, t: &str, run: &str, now: i64) -> ChatRun {
    s.chat_run_claim_next(
        t,
        run,
        &json!({"harness":"claude-code","model":"m1"}),
        at(now),
    )
    .expect("claim")
    .expect("claimed")
}

fn finish(s: &SqliteStore, run: &str, now: i64) {
    s.chat_run_finish(run, ChatRunState::Completed, Some("ok"), None, at(now))
        .expect("finish");
}

/// A delivered (`completed`) message of `ME`.
fn delivered(s: &SqliteStore, t: &str, key: &str, text: &str, now: i64) -> ChatMessage {
    let m = post(s, t, key, text, ME, now);
    claim(s, t, &format!("run-{key}"), now + 1);
    finish(s, &format!("run-{key}"), now + 2);
    s.chat_message_get(t, &m.id).expect("get")
}

fn edit(text: &str, expected_revision: u64) -> ChatMessageEditRequest {
    ChatMessageEditRequest {
        text: text.into(),
        expected_revision,
    }
}

fn retract(expected_revision: u64, reason: &str) -> ChatMessageRetractRequest {
    ChatMessageRetractRequest {
        expected_revision,
        reason: reason.into(),
    }
}

fn revised(s: &SqliteStore, t: &str, m: &str) -> Vec<ChatMessageRevisedData> {
    s.chat_message_revisions(t, m)
        .expect("revisions")
        .items
        .into_iter()
        .map(|e| match e.data {
            ChatEventData::Revised(d) => d,
            other => panic!("not message_revised: {other:?}"),
        })
        .collect()
}

fn search_hits(s: &SqliteStore, word: &str) -> usize {
    s.chat_thread_list(&ChatThreadQuery {
        q: Some(word.into()),
        ..ChatThreadQuery::default()
    })
    .expect("search")
    .items
    .len()
}

/// Invariant: the body before each change is kept in `chat_events` and readable in order; the
/// message carries the new revision and who edited it. Before this WU a message had no revision
/// and no way to change, so there was nothing to keep.
#[test]
fn chat_message_edit_by_author_keeps_previous_body_in_chat_events() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = thread(&s);
    let m = delivered(&s, &t, "k1", "first words", 0);
    assert_eq!(m.edit_state.revision, 1);
    let wire = serde_json::to_value(&m).expect("wire");
    assert!(wire.get("revision").is_none() && wire.get("edited").is_none());

    let e1 = s
        .chat_message_edit(&t, &m.id, ME, &edit("second words", 1), at(10))
        .expect("edit");
    assert_eq!(
        (e1.text.as_str(), e1.edit_state.revision),
        ("second words", 2)
    );
    assert_eq!(
        e1.edit_state.edited.as_ref().map(|c| c.by.as_str()),
        Some(ME)
    );
    assert_eq!(e1.discussion_mode, m.discussion_mode, "edit keeps the mode");
    let e2 = s
        .chat_message_retract(&t, &m.id, ME, &retract(2, "wrong thread"), at(11))
        .expect("retract");
    assert_eq!((e2.text.as_str(), e2.edit_state.revision), ("", 3));
    assert_eq!(
        e2.edit_state
            .retracted
            .as_ref()
            .and_then(|c| c.reason.as_deref()),
        Some("wrong thread")
    );

    let history = revised(&s, &t, &m.id);
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].kind, ChatMessageRevisionKind::Edit);
    assert_eq!(history[0].previous_text, "first words");
    assert_eq!(
        history[0].previous_sha256,
        crate::project_proposal::sha256_hex("first words")
    );
    assert_eq!((history[0].previous_revision, history[0].revision), (1, 2));
    assert_eq!(history[1].kind, ChatMessageRevisionKind::Retract);
    assert_eq!(history[1].previous_text, "second words");
    assert_eq!(history[1].by, ME);
    assert_eq!(history[1].reason.as_deref(), Some("wrong thread"));
    // The change is streamed: message_revised, then the message event with the new body.
    let page = s
        .chat_events_page(&t, &ChatEventQuery::default())
        .expect("events");
    let tail: Vec<_> = page
        .items
        .iter()
        .rev()
        .take(2)
        .map(|e| e.event_type)
        .collect();
    assert_eq!(
        tail,
        vec![ChatEventType::Message, ChatEventType::MessageRevised]
    );
}

/// Invariant: only the author (a `human:` principal) changes a message; others, a relay, and
/// CoS / system / assistant messages are refused without any write.
#[test]
fn chat_message_edit_forbidden_for_non_author_principals() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = thread(&s);
    let m = delivered(&s, &t, "k1", "mine", 0);
    for by in [
        "human:bob",
        "operator:claude-code",
        "mcp:client-1",
        &format!("cos:{t}"),
        "human:local-other",
    ] {
        let e = s
            .chat_message_edit(&t, &m.id, by, &edit("theirs", 1), at(5))
            .expect_err("edit by another principal");
        assert!(matches!(e, ChatRevisionError::Forbidden(_)), "{by}: {e:?}");
        let r = s
            .chat_message_retract(&t, &m.id, by, &retract(1, ""), at(5))
            .expect_err("retract by another principal");
        assert_eq!(r.http_status(), 403, "{by}");
    }
    // An operator's message claiming to be the person's is still not the person's.
    let op = post(&s, &t, "k2", "operator text", "operator:claude-code", 6);
    let e = s
        .chat_message_edit(&t, &op.id, "operator:claude-code", &edit("x", 1), at(7))
        .expect_err("operator cannot edit");
    assert!(matches!(e, ChatRevisionError::Forbidden(_)));
    assert!(revised(&s, &t, &m.id).is_empty());
    assert_eq!(s.chat_message_get(&t, &m.id).expect("get").text, "mine");
}

/// Invariant: a relayed message (author = the operator) is never editable by anyone.
#[test]
fn chat_message_edit_forbidden_for_relay_message() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = thread(&s);
    let relay = s
        .chat_relay_post(
            &t,
            &ChatRelayRequest {
                client_message_id: "r1".into(),
                text: "the person said this".into(),
                said_at: crate::chat::store::chat_ts(at(0)),
                task_ids: vec![],
                expires_at: None,
                discussion_mode: None,
            },
            "operator:claude-code",
            at(1),
        )
        .expect("relay")
        .response
        .message;
    for by in ["operator:claude-code", ME] {
        let e = s
            .chat_message_edit(&t, &relay.id, by, &edit("changed", 1), at(2))
            .expect_err("relay is not editable");
        assert!(matches!(e, ChatRevisionError::Forbidden(_)), "{by}");
    }
}

/// Invariant: CoS replies and daemon inputs are not a person's words and cannot be changed.
#[test]
fn chat_message_edit_assistant_or_system_is_403() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = thread(&s);
    let m = post(&s, &t, "k1", "question", ME, 0);
    claim(&s, &t, "run-1", 1);
    finish(&s, "run-1", 2);
    let reply = s
        .chat_message_list(&t, &ChatMessageQuery::default())
        .expect("list")
        .items
        .into_iter()
        .find(|x| x.role == ChatMessageRole::Assistant)
        .expect("assistant reply");
    let e = s
        .chat_message_edit(&t, &reply.id, ME, &edit("x", 1), at(3))
        .expect_err("assistant");
    assert!(matches!(e, ChatRevisionError::Forbidden(_)));
    let system = s
        .chat_system_message_add(&t, "daemon request", &[], at(4))
        .expect("system");
    let e = s
        .chat_message_retract(&t, &system.id, ME, &retract(1, ""), at(5))
        .expect_err("system");
    assert!(matches!(e, ChatRevisionError::Forbidden(_)));
    assert_eq!(
        s.chat_message_get(&t, &m.id)
            .expect("get")
            .edit_state
            .revision,
        1
    );
}

/// Invariant: an input a run is reading (`running`) is not changed; a stale revision is a
/// conflict; a queued input can be edited but only cancelled (not retracted); a cancelled or
/// retracted message cannot change again.
#[test]
fn chat_message_edit_running_is_409() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = thread(&s);
    let m = post(&s, &t, "k1", "queued words", ME, 0);
    let queued = s
        .chat_message_edit(&t, &m.id, ME, &edit("queued, edited", 1), at(1))
        .expect("queued input can be edited");
    assert_eq!(queued.state, ChatMessageState::Queued);
    let e = s
        .chat_message_retract(&t, &m.id, ME, &retract(2, ""), at(2))
        .expect_err("queued retract");
    assert!(
        matches!(e, ChatRevisionError::Chat(ChatError::Conflict(_))),
        "{e:?}"
    );
    let run = claim(&s, &t, "run-1", 3);
    assert_eq!(run.input_message_id, m.id);
    for result in [
        s.chat_message_edit(&t, &m.id, ME, &edit("too late", 2), at(4)),
        s.chat_message_retract(&t, &m.id, ME, &retract(2, ""), at(4)),
    ] {
        assert!(
            matches!(result, Err(ChatRevisionError::Chat(ChatError::Conflict(_)))),
            "{result:?}"
        );
    }
    let c = post(&s, &t, "k2", "to cancel", ME, 5);
    s.chat_message_cancel(&t, &c.id, at(6)).expect("cancel");
    let e = s
        .chat_message_edit(&t, &c.id, ME, &edit("x", 1), at(7))
        .expect_err("cancelled");
    assert!(matches!(e, ChatRevisionError::Chat(ChatError::Conflict(_))));
}

#[test]
fn chat_message_edit_stale_revision_is_409() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = thread(&s);
    let m = delivered(&s, &t, "k1", "v1", 0);
    s.chat_message_edit(&t, &m.id, ME, &edit("v2", 1), at(5))
        .expect("edit");
    let e = s
        .chat_message_edit(&t, &m.id, ME, &edit("v3", 1), at(6))
        .expect_err("stale");
    assert!(
        matches!(e, ChatRevisionError::Chat(ChatError::Conflict(_))),
        "{e:?}"
    );
    s.chat_message_retract(&t, &m.id, ME, &retract(2, ""), at(7))
        .expect("retract");
    let e = s
        .chat_message_edit(&t, &m.id, ME, &edit("v4", 3), at(8))
        .expect_err("retracted");
    assert!(
        matches!(e, ChatRevisionError::Chat(ChatError::Conflict(_))),
        "{e:?}"
    );
    assert_eq!(revised(&s, &t, &m.id).len(), 2);
}

/// Invariant: a retracted body leaves the thread search (FTS5 follows `text` through the
/// existing update trigger) and the message text; the previous body is only in `chat_events`.
#[test]
fn chat_message_retract_empties_text_and_search() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = thread(&s);
    let m = delivered(&s, &t, "k1", "zebrafinch secret plan", 0);
    assert_eq!(search_hits(&s, "zebrafinch"), 1);
    let r = s
        .chat_message_retract(&t, &m.id, ME, &retract(1, "said too much"), at(5))
        .expect("retract");
    assert_eq!(r.text, "");
    assert_eq!(search_hits(&s, "zebrafinch"), 0);
    let fts: i64 = s
        .lock()
        .expect("lock")
        .query_row(
            "SELECT COUNT(*) FROM chat_search WHERE chat_search MATCH 'zebrafinch'",
            [],
            |row| row.get(0),
        )
        .expect("fts");
    assert_eq!(fts, 0);
    assert_eq!(
        revised(&s, &t, &m.id)[0].previous_text,
        "zebrafinch secret plan"
    );
    // An edit also replaces what search finds.
    let m2 = delivered(&s, &t, "k2", "pelican idea", 10);
    s.chat_message_edit(&t, &m2.id, ME, &edit("heron idea", 1), at(20))
        .expect("edit");
    assert_eq!(search_hits(&s, "pelican"), 0);
    assert_eq!(search_hits(&s, "heron"), 1);
}

/// Invariant (D5.4): a summary whose cursor covers the changed message is cleared in the same
/// transaction, the cleared summary is kept in the event, and the run that was going to
/// checkpoint on the old cursor gets 409. A summary that does not cover it stays.
#[test]
fn chat_message_edit_resets_checkpoint_covering_seq() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = thread(&s);
    let m1 = delivered(&s, &t, "k1", "covered", 0);
    post(&s, &t, "k2", "next", ME, 10);
    claim(&s, &t, "run-k2", 11);
    s.chat_thread_checkpoint_at(&t, "run-k2", "summary of m1", m1.seq, 0, at(12))
        .expect("checkpoint");
    assert_eq!(
        s.chat_thread_summary(&t).expect("summary"),
        ("summary of m1".to_string(), m1.seq)
    );
    s.chat_message_edit(&t, &m1.id, ME, &edit("covered, edited", 1), at(13))
        .expect("edit");
    assert_eq!(
        s.chat_thread_summary(&t).expect("summary"),
        (String::new(), 0)
    );
    let reset = revised(&s, &t, &m1.id)[0]
        .checkpoint_reset
        .clone()
        .expect("reset recorded");
    assert_eq!(reset.previous_through_seq, m1.seq);
    assert_eq!(reset.previous_summary, "summary of m1");
    let stale = s
        .chat_thread_checkpoint_at(&t, "run-k2", "late summary", m1.seq, m1.seq, at(14))
        .expect_err("old cursor");
    assert!(matches!(stale, ChatError::Conflict(_)), "{stale:?}");
    finish(&s, "run-k2", 15);

    // A message after the summary's cursor does not touch the summary.
    post(&s, &t, "k3", "later", ME, 20);
    claim(&s, &t, "run-k3", 21);
    s.chat_thread_checkpoint_at(&t, "run-k3", "summary 2", m1.seq, 0, at(22))
        .expect("checkpoint 2");
    finish(&s, "run-k3", 23);
    let after = s
        .chat_message_list(&t, &ChatMessageQuery::default())
        .expect("list")
        .items
        .into_iter()
        .find(|x| x.client_message_id.as_deref() == Some("k3"))
        .expect("k3");
    s.chat_message_edit(&t, &after.id, ME, &edit("later, edited", 1), at(24))
        .expect("edit after cursor");
    assert_eq!(
        s.chat_thread_summary(&t).expect("summary"),
        ("summary 2".to_string(), m1.seq)
    );
    assert!(revised(&s, &t, &after.id)[0].checkpoint_reset.is_none());
}

/// Invariant (D5.4): a live session that was delivered the changed message is retired, so the
/// next run starts fresh without the old words in the harness memory; a session that has not
/// seen the message stays live.
#[test]
fn chat_message_edit_retires_session_that_saw_message() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = thread(&s);
    let m1 = delivered(&s, &t, "k1", "seen", 0);
    let key = ChatSessionKey {
        thread_id: t.clone(),
        harness: "claude-code".into(),
        provider: None,
        llm_source: None,
        account_id: None,
        cwd: None,
        model: Some("m1".into()),
        policy_key: None,
    };
    let session = ChatSession::new(key.clone(), "11111111-1111-4111-8111-111111111111", at(3));
    s.chat_session_rotate(&session, at(3)).expect("session");
    s.chat_session_set_delivered_through(&session.id, i64::try_from(m1.seq).expect("seq"))
        .expect("delivered");
    let m2 = delivered(&s, &t, "k2", "unseen", 10);
    s.chat_message_edit(&t, &m2.id, ME, &edit("unseen, edited", 1), at(20))
        .expect("edit unseen");
    assert_eq!(
        s.chat_session_active(&t).expect("active").map(|x| x.id),
        Some(session.id.clone()),
        "a session that has not seen the message stays"
    );
    s.chat_message_retract(&t, &m1.id, ME, &retract(1, ""), at(21))
        .expect("retract seen");
    assert_eq!(s.chat_session_active(&t).expect("active"), None);
    let retired = s.chat_session_get(&session.id).expect("get").expect("row");
    assert!(retired.retired_at.is_some());
}

/// Invariant: chat_events retention removes `text_delta` / `tool` only, never the previous body.
#[test]
fn chat_message_edit_previous_body_survives_retention() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = thread(&s);
    let m = delivered(&s, &t, "k1", "keep me", 0);
    s.chat_message_edit(&t, &m.id, ME, &edit("new me", 1), at(5))
        .expect("edit");
    s.chat_events_retention(at(400 * 86_400), time::Duration::days(1))
        .expect("retention");
    let history = revised(&s, &t, &m.id);
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].previous_text, "keep me");
}

/// Invariant: the endpoints refuse an unknown message / another thread's message (404) and an
/// over-long reason or a no-op edit (422) without writing.
#[test]
fn chat_message_edit_validates_text_and_reason() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = thread(&s);
    let m = delivered(&s, &t, "k1", "same", 0);
    let e = s
        .chat_message_edit(&t, &m.id, ME, &edit("same", 1), at(5))
        .expect_err("no-op");
    assert!(matches!(e, ChatRevisionError::Chat(ChatError::Invalid(_))));
    let e = s
        .chat_message_edit(&t, &m.id, ME, &edit("  ", 1), at(5))
        .expect_err("blank");
    assert!(matches!(e, ChatRevisionError::Chat(ChatError::Invalid(_))));
    let long = "x".repeat(crate::chat_revision::CHAT_RETRACT_REASON_MAX_CHARS + 1);
    let e = s
        .chat_message_retract(&t, &m.id, ME, &retract(1, &long), at(5))
        .expect_err("reason");
    assert!(matches!(e, ChatRevisionError::Chat(ChatError::Invalid(_))));
    let e = s
        .chat_message_edit(&t, "missing", ME, &edit("x", 1), at(5))
        .expect_err("missing");
    assert_eq!(e.http_status(), 404);
    assert!(revised(&s, &t, &m.id).is_empty());
}
