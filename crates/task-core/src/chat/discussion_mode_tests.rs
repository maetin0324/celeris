//! ADR 2026-11-21-chat-discussion-mode D1・D8.1: the message record of the mode and the run's mode
//! derived from its inputs. No earlier test can see these: before the ADR a message had no mode.

use rusqlite::params;
use serde_json::{Value, json};
use time::OffsetDateTime;

use super::*;
use crate::discussion_mode::{ChatDiscussionDecider, RunDiscussionMode};
use crate::store::SqliteStore;

fn at(secs: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_791_158_400 + secs).expect("valid ts")
}

fn req(key: &str, mode: Option<ChatDiscussionMode>) -> ChatPostMessageRequest {
    ChatPostMessageRequest {
        client_message_id: key.into(),
        text: format!("text {key}"),
        attachment_ids: vec![],
        reply_to_id: None,
        mode: ChatSendMode::Queue,
        resume_queue: false,
        discussion_mode: mode,
    }
}

fn post(
    s: &SqliteStore,
    thread: &str,
    key: &str,
    mode: Option<ChatDiscussionMode>,
    t: i64,
) -> String {
    s.chat_message_post_as(thread, &req(key, mode), "human:rmaeda", at(t))
        .expect("post")
        .response
        .message
        .id
}

fn metadata(s: &SqliteStore, id: &str) -> Value {
    let raw: String = s
        .lock()
        .expect("lock")
        .query_row(
            "SELECT metadata_json FROM chat_messages WHERE id=?1",
            params![id],
            |r| r.get(0),
        )
        .expect("metadata");
    serde_json::from_str(&raw).expect("json")
}

fn claim(s: &SqliteStore, thread: &str, run: &str, t: i64) -> ChatRun {
    s.chat_run_claim_next(thread, run, &json!({}), at(t))
        .expect("claim")
        .expect("a queued input")
}

fn mode(s: &SqliteStore, thread: &str, run: &str) -> RunDiscussionMode {
    s.chat_run_discussion_mode(thread, run).expect("mode")
}

/// D1.1: only discuss and propose are written; an absent field and `execute` leave the
/// `metadata_json` exactly as before the ADR, and the wire message omits the field.
#[test]
fn chat_discussion_mode_recorded_only_for_discuss_and_propose() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = test_project_thread(&s, "record", at(0));
    let absent = post(&s, &t.id, "absent", None, 1);
    let execute = post(&s, &t.id, "execute", Some(ChatDiscussionMode::Execute), 2);
    let discuss = post(&s, &t.id, "discuss", Some(ChatDiscussionMode::Discuss), 3);
    let propose = post(&s, &t.id, "propose", Some(ChatDiscussionMode::Propose), 4);
    let base = json!({"mode": "queue", "interrupt": false, "attachment_ids": [],
                      "author": "human:rmaeda"});
    assert_eq!(metadata(&s, &absent), base);
    assert_eq!(metadata(&s, &execute), base);
    assert_eq!(metadata(&s, &discuss)["discussion_mode"], "discuss");
    assert_eq!(metadata(&s, &propose)["discussion_mode"], "propose");
    let wire = |id: &str| {
        serde_json::to_value(s.chat_message_get(&t.id, id).expect("message")).expect("wire")
    };
    assert!(wire(&absent).get("discussion_mode").is_none());
    assert!(wire(&execute).get("discussion_mode").is_none());
    assert_eq!(wire(&discuss)["discussion_mode"], "discuss");
    // The same key with another mode is a different message (409), the same mode is the same.
    let again = s.chat_message_post_as(
        &t.id,
        &req("discuss", Some(ChatDiscussionMode::Discuss)),
        "human:rmaeda",
        at(5),
    );
    assert_eq!(again.expect("same").response.message.id, discuss);
    let other = s.chat_message_post_as(&t.id, &req("discuss", None), "human:rmaeda", at(6));
    assert_eq!(other.expect_err("other mode").http_status(), 409);
    assert_eq!(
        s.chat_message_post_as(
            &t.id,
            &req("absent", Some(ChatDiscussionMode::Execute)),
            "human:rmaeda",
            at(7)
        )
        .expect("execute is absent")
        .response
        .message
        .id,
        absent
    );
}

/// D1.2: the run takes the strictest mode of its inputs — its own input and the earlier messages
/// no completed run answered — and names the messages that carry it.
#[test]
fn chat_discussion_mode_run_takes_strictest_input() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = test_project_thread(&s, "strict", at(0));
    let first = post(&s, &t.id, "first", None, 1);
    claim(&s, &t.id, "r1", 2);
    assert_eq!(mode(&s, &t.id, "r1").mode, ChatDiscussionMode::Execute);
    // r1 fails: its input is carried over to the next run's window.
    s.chat_run_finish("r1", ChatRunState::Failed, None, Some("boom"), at(3))
        .expect("fail");
    let _ = first;
    let discuss = post(&s, &t.id, "discuss", Some(ChatDiscussionMode::Discuss), 4);
    let propose = post(&s, &t.id, "propose", Some(ChatDiscussionMode::Propose), 5);
    let execute = post(&s, &t.id, "execute", Some(ChatDiscussionMode::Execute), 6);
    // Interrupt: the execute message is claimed first, the earlier discuss/propose still queued.
    s.lock()
        .expect("lock")
        .execute(
            "UPDATE chat_messages SET metadata_json=json_set(metadata_json,'$.interrupt',1) WHERE id=?1",
            params![execute],
        )
        .expect("interrupt");
    let run = claim(&s, &t.id, "r2", 7);
    assert_eq!(run.input_message_id, execute);
    let derived = mode(&s, &t.id, "r2");
    assert_eq!(derived.mode, ChatDiscussionMode::Discuss);
    assert_eq!(
        derived.decided_by,
        vec![ChatDiscussionDecider {
            message_id: discuss.clone(),
            author: Some("human:rmaeda".into()),
        }]
    );
    s.chat_run_finish("r2", ChatRunState::Completed, Some("ok"), None, at(8))
        .expect("complete");
    // After r2 completed, the next run's window starts after r2's input (seq of `execute`);
    // discuss (queued, earlier seq) is the input of r3 and is discuss on its own.
    let r3 = claim(&s, &t.id, "r3", 9);
    assert_eq!(r3.input_message_id, discuss);
    assert_eq!(mode(&s, &t.id, "r3").mode, ChatDiscussionMode::Discuss);
    s.chat_run_finish("r3", ChatRunState::Completed, Some("ok"), None, at(10))
        .expect("complete");
    let r4 = claim(&s, &t.id, "r4", 11);
    assert_eq!(r4.input_message_id, propose);
    assert_eq!(mode(&s, &t.id, "r4").mode, ChatDiscussionMode::Propose);
    s.chat_run_finish("r4", ChatRunState::Completed, Some("ok"), None, at(12))
        .expect("complete");
    // A fresh execute message after the completed runs is execute again.
    let fresh = post(&s, &t.id, "fresh", None, 13);
    let r5 = claim(&s, &t.id, "r5", 14);
    assert_eq!(r5.input_message_id, fresh);
    let derived = mode(&s, &t.id, "r5");
    assert_eq!(derived.mode, ChatDiscussionMode::Execute);
    // The value is not stored anywhere but the messages.
    let stored: i64 = s
        .lock()
        .expect("lock")
        .query_row(
            "SELECT COUNT(*) FROM chat_runs WHERE COALESCE(resolved_config_json,'') LIKE '%discussion_mode%'",
            [],
            |r| r.get(0),
        )
        .expect("count");
    assert_eq!(stored, 0);
}

/// D1.2: a daemon request (`role=system`, triage・review) run is execute, as today.
#[test]
fn chat_discussion_mode_system_input_run_is_execute() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = test_project_thread(&s, "system", at(0));
    post(&s, &t.id, "human", None, 1);
    claim(&s, &t.id, "r1", 2);
    s.chat_run_finish("r1", ChatRunState::Completed, Some("ok"), None, at(3))
        .expect("complete");
    let request = s
        .chat_system_queue_once(&t.id, "cos-triage:1", "triage", at(4))
        .expect("system");
    let run = claim(&s, &t.id, "r2", 5);
    assert_eq!(run.input_message_id, request.id);
    let derived = mode(&s, &t.id, "r2");
    assert_eq!(derived.mode, ChatDiscussionMode::Execute);
    assert_eq!(
        derived.decided_by,
        vec![ChatDiscussionDecider {
            message_id: request.id,
            author: None,
        }]
    );
}

/// D1.1: an unreadable recorded value is read as the strictest mode (fail closed).
#[test]
fn chat_discussion_mode_unreadable_record_is_discuss() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = test_project_thread(&s, "unreadable", at(0));
    let id = post(&s, &t.id, "odd", None, 1);
    s.lock()
        .expect("lock")
        .execute(
            "UPDATE chat_messages SET metadata_json=json_set(metadata_json,'$.discussion_mode','debate') WHERE id=?1",
            params![id],
        )
        .expect("odd value");
    claim(&s, &t.id, "r1", 2);
    assert_eq!(mode(&s, &t.id, "r1").mode, ChatDiscussionMode::Discuss);
}

/// D1.1・D7: a relay and an MCP `console_instruct` input carry the mode; the run's deciders keep
/// the MCP client's author apart from a person's (an MCP input never counts as the person).
#[test]
fn discussion_mode_relay_and_mcp_instruct_carry_mode() {
    let s = SqliteStore::open_in_memory().expect("store");
    let posted = s
        .chat_mcp_instruct(
            "chatgpt",
            None,
            None,
            "議論",
            None,
            Some(ChatDiscussionMode::Discuss),
            at(0),
        )
        .expect("mcp");
    let message = posted.response.message;
    assert_eq!(message.discussion_mode, Some(ChatDiscussionMode::Discuss));
    assert_eq!(message.author.as_deref(), Some("mcp:chatgpt"));
    let run = claim(&s, &message.thread_id, "r-mcp", 1);
    assert_eq!(run.input_message_id, message.id);
    let derived = mode(&s, &message.thread_id, "r-mcp");
    assert_eq!(derived.mode, ChatDiscussionMode::Discuss);
    assert_eq!(derived.decided_by[0].author.as_deref(), Some("mcp:chatgpt"));
    let plain = s
        .chat_mcp_instruct(
            "chatgpt",
            Some(&message.thread_id),
            None,
            "続き",
            None,
            None,
            at(2),
        )
        .expect("mcp");
    assert_eq!(plain.response.message.discussion_mode, None);
    let t = test_project_thread(&s, "relay", at(3));
    let relay = ChatRelayRequest {
        client_message_id: "relay-1".into(),
        text: "人の原文".into(),
        said_at: "2026-10-05T00:00:00Z".into(),
        task_ids: vec![],
        expires_at: None,
        discussion_mode: Some(ChatDiscussionMode::Propose),
    };
    let relayed = s
        .chat_relay_post(&t.id, &relay, "operator:claude-code", at(4))
        .expect("relay")
        .response
        .message;
    assert_eq!(relayed.discussion_mode, Some(ChatDiscussionMode::Propose));
    assert_eq!(relayed.author.as_deref(), Some("operator:claude-code"));
    assert_eq!(metadata(&s, &relayed.id)["discussion_mode"], "propose");
}
