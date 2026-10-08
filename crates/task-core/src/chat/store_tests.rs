//! ADR 2026-10-05 D1/D2: chat store operations (threads, messages, runs, events).
//! Every clock is injected; no sleeps.

use std::sync::{Arc, Barrier};

use rusqlite::params;
use serde_json::json;
use time::{Duration, OffsetDateTime};

use super::*;
use crate::store::SqliteStore;

fn at(secs: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_791_158_400 + secs).expect("valid ts")
}

fn store() -> SqliteStore {
    SqliteStore::open_in_memory().expect("store")
}

#[test]
fn cos_chat_run_control_system_review_card_is_idempotent() {
    let s = store();
    let t = create(&s, "review-card", "Review", 0);
    let card = ChatCard {
        kind: ChatCardKind::Operation,
        id: "run-1".into(),
        title: "Check operation".into(),
        state: "pending".into(),
        href: "/chat/threads/review-card/runs/run-1".into(),
        actor: ChatActor::System,
        reason: Some("outcome unknown".into()),
        operation_id: None,
    };
    let first = s
        .chat_system_message_add_once(
            &t.id,
            "cos-review:run-1",
            "Review",
            std::slice::from_ref(&card),
            at(1),
        )
        .expect("first review card");
    let second = s
        .chat_system_message_add_once(&t.id, "cos-review:run-1", "Review", &[card], at(2))
        .expect("same review card");
    assert_eq!(first.id, second.id);
    assert_eq!(
        status(
            s.chat_system_message_add_once(&t.id, "cos-review:run-1", "different", &[], at(3))
                .expect_err("key cannot be reused")
        ),
        409
    );
}

fn create(s: &SqliteStore, key: &str, title: &str, now: i64) -> ChatThread {
    s.chat_thread_create(
        "admin",
        &ChatCreateThreadRequest {
            title: title.into(),
            project_id: None,
            client_thread_id: key.into(),
        },
        at(now),
    )
    .expect("create thread")
    .thread
}

fn req(key: &str, text: &str) -> ChatPostMessageRequest {
    ChatPostMessageRequest {
        client_message_id: key.into(),
        text: text.into(),
        attachment_ids: vec![],
        reply_to_id: None,
        mode: ChatSendMode::Queue,
        resume_queue: false,
    }
}

fn post(s: &SqliteStore, t: &str, key: &str, text: &str, now: i64) -> ChatMessage {
    s.chat_message_post(t, &req(key, text), at(now))
        .expect("post")
        .response
        .message
}

fn add_attachment(s: &SqliteStore, id: &str, thread: &str) {
    let conn = s.lock().expect("lock");
    conn.execute(
        "INSERT INTO chat_attachments(id,thread_id,original_name,media_type,size_bytes,sha256,\
         relative_path,state,created_at) VALUES(?1,?2,'a.png','image/png',1,'00','x','ready',\
         '2026-10-05T00:00:00.000Z')",
        params![id, thread],
    )
    .expect("attachment");
}

fn claim(s: &SqliteStore, t: &str, run: &str, now: i64) -> Option<ChatRun> {
    s.chat_run_claim_next(t, run, &json!({"harness":"claude-code"}), at(now))
        .expect("claim")
}

fn status(e: ChatError) -> u16 {
    e.http_status()
}

#[test]
fn chat_store_thread_create_is_idempotent_per_client_key() {
    let s = store();
    let first = s
        .chat_thread_create(
            "admin",
            &ChatCreateThreadRequest {
                title: "画面".into(),
                project_id: None,
                client_thread_id: "k1".into(),
            },
            at(0),
        )
        .expect("create");
    assert!(first.created);
    assert_eq!(first.thread.kind, ChatThreadKind::Human);
    assert_eq!(first.thread.revision, 1);
    let again = s
        .chat_thread_create(
            "admin",
            &ChatCreateThreadRequest {
                title: "画面".into(),
                project_id: None,
                client_thread_id: "k1".into(),
            },
            at(5),
        )
        .expect("replay");
    assert!(!again.created);
    assert_eq!(again.thread.id, first.thread.id);
    let conflict = s
        .chat_thread_create(
            "admin",
            &ChatCreateThreadRequest {
                title: "別".into(),
                project_id: None,
                client_thread_id: "k1".into(),
            },
            at(6),
        )
        .expect_err("different content");
    assert_eq!(status(conflict), 409);
    // Another scope may reuse the key.
    let other = s
        .chat_thread_create(
            "other",
            &ChatCreateThreadRequest {
                title: "別".into(),
                project_id: None,
                client_thread_id: "k1".into(),
            },
            at(7),
        )
        .expect("other scope");
    assert_ne!(other.thread.id, first.thread.id);
    let missing_project = s
        .chat_thread_create(
            "admin",
            &ChatCreateThreadRequest {
                title: "x".into(),
                project_id: Some("nope".into()),
                client_thread_id: "k2".into(),
            },
            at(8),
        )
        .expect_err("project reference");
    assert_eq!(status(missing_project), 422);
    let blank = s
        .chat_thread_create(
            "admin",
            &ChatCreateThreadRequest {
                title: "  ".into(),
                project_id: None,
                client_thread_id: "k3".into(),
            },
            at(9),
        )
        .expect_err("blank title");
    assert_eq!(status(blank), 422);
}

#[test]
fn chat_store_thread_list_pages_by_updated_desc_and_status() {
    let s = store();
    let a = create(&s, "a", "A", 0);
    let b = create(&s, "b", "B", 10);
    let c = create(&s, "c", "C", 20);
    post(&s, &a.id, "m", "touch a", 30); // a becomes newest
    let page = s
        .chat_thread_list(&ChatThreadQuery {
            limit: Some(2),
            ..Default::default()
        })
        .expect("page 1");
    let ids: Vec<_> = page.items.iter().map(|t| t.id.clone()).collect();
    assert_eq!(ids, vec![a.id.clone(), c.id.clone()]);
    assert_eq!(page.items[0].queued_count, 1);
    let next = s
        .chat_thread_list(&ChatThreadQuery {
            limit: Some(2),
            before: page.next_cursor.clone(),
            ..Default::default()
        })
        .expect("page 2");
    assert_eq!(next.items.len(), 1);
    assert_eq!(next.items[0].id, b.id);
    assert_eq!(next.next_cursor, None);
    s.chat_thread_patch(
        &b.id,
        &ChatPatchThreadRequest {
            title: None,
            status: Some(ChatThreadStatus::Archived),
            expected_revision: 1,
        },
        at(40),
    )
    .expect("archive");
    let open = s
        .chat_thread_list(&ChatThreadQuery {
            status: Some(ChatThreadStatus::Open),
            ..Default::default()
        })
        .expect("open");
    assert!(open.items.iter().all(|t| t.id != b.id));
    assert_eq!(open.items.len(), 2);
    for bad in [Some(0), Some(101)] {
        let e = s
            .chat_thread_list(&ChatThreadQuery {
                limit: bad,
                ..Default::default()
            })
            .expect_err("limit");
        assert_eq!(status(e), 400);
    }
    let e = s
        .chat_thread_list(&ChatThreadQuery {
            before: Some("garbage".into()),
            ..Default::default()
        })
        .expect_err("cursor");
    assert_eq!(status(e), 400);
    assert_eq!(
        s.chat_thread_list(&ChatThreadQuery {
            limit: Some(100),
            ..Default::default()
        })
        .expect("max")
        .items
        .len(),
        3
    );
}

#[test]
fn chat_store_search_takes_fts_syntax_literally_and_dedups_threads() {
    let s = store();
    let a = create(&s, "a", "alpha report", 0);
    let b = create(&s, "b", "other", 1);
    post(&s, &a.id, "a1", "alpha beta", 2);
    post(&s, &a.id, "a2", "alpha again", 3);
    post(&s, &b.id, "b1", "gamma", 4);
    post(&s, &b.id, "b2", "say \"quoted\" text:gamma", 5);
    let search = |q: &str| -> Vec<String> {
        s.chat_thread_list(&ChatThreadQuery {
            q: Some(q.into()),
            ..Default::default()
        })
        .expect("search must not raise FTS syntax errors")
        .items
        .into_iter()
        .map(|t| t.id)
        .collect()
    };
    // Title and two messages hit thread a: one row.
    assert_eq!(search("alpha"), vec![a.id.clone()]);
    // FTS operators are words: no OR, no column filter, no NEAR, no prefix, no negation.
    assert!(search("alpha OR gamma").is_empty());
    assert!(search("title:alpha").is_empty());
    assert_eq!(
        search("text:gamma"),
        vec![b.id.clone()],
        "phrase 'text gamma' is in b2"
    );
    assert!(search("NEAR(alpha beta)").is_empty());
    assert!(search("alph*").is_empty());
    assert!(search("-alpha").len() <= 1);
    assert_eq!(search("\"quoted\""), vec![b.id.clone()]);
    assert!(search("\"").is_empty());
    assert!(search("(").is_empty());
    assert!(search("^gamma").len() <= 1);
    assert_eq!(
        chat_fts_literal("a OR \"b"),
        Some("\"a\" \"OR\" \"\"\"b\"".into())
    );
    // Blank q is the plain list.
    assert_eq!(search("   ").len(), 2);
}

#[test]
fn chat_store_thread_patch_revision_and_archive_rules() {
    let s = store();
    let t = create(&s, "a", "A", 0);
    let e = s
        .chat_thread_patch(
            &t.id,
            &ChatPatchThreadRequest {
                title: Some("B".into()),
                status: None,
                expected_revision: 9,
            },
            at(1),
        )
        .expect_err("stale revision");
    assert_eq!(status(e), 409);
    let renamed = s
        .chat_thread_patch(
            &t.id,
            &ChatPatchThreadRequest {
                title: Some("B".into()),
                status: None,
                expected_revision: 1,
            },
            at(2),
        )
        .expect("rename");
    assert_eq!((renamed.title.as_str(), renamed.revision), ("B", 2));
    let m = post(&s, &t.id, "m", "hi", 3);
    let archive = ChatPatchThreadRequest {
        title: None,
        status: Some(ChatThreadStatus::Archived),
        expected_revision: 2,
    };
    assert_eq!(
        status(
            s.chat_thread_patch(&t.id, &archive, at(4))
                .expect_err("queued")
        ),
        409
    );
    claim(&s, &t.id, "r1", 5).expect("claimed");
    assert_eq!(
        status(
            s.chat_thread_patch(&t.id, &archive, at(6))
                .expect_err("run")
        ),
        409
    );
    s.chat_run_finish("r1", ChatRunState::Completed, Some("ok"), None, at(7))
        .expect("finish");
    let archived = s
        .chat_thread_patch(&t.id, &archive, at(8))
        .expect("archive");
    assert_eq!(archived.status, ChatThreadStatus::Archived);
    let e = s
        .chat_message_post(&t.id, &req("m2", "x"), at(9))
        .expect_err("archived");
    assert_eq!(status(e), 409);
    // The original message replays even after archive.
    assert_eq!(
        s.chat_message_post(&t.id, &req("m", "hi"), at(10))
            .expect("replay")
            .response
            .message
            .id,
        m.id
    );
    let empty = ChatPatchThreadRequest {
        title: None,
        status: None,
        expected_revision: 3,
    };
    assert_eq!(
        status(
            s.chat_thread_patch(&t.id, &empty, at(11))
                .expect_err("empty")
        ),
        422
    );
    {
        let conn = s.lock().expect("lock");
        conn.execute(
            "INSERT INTO chat_threads(id,kind,title,status,created_at,updated_at) \
             VALUES('inbox','inbox','受信箱','open','x','x')",
            [],
        )
        .expect("inbox");
    }
    let e = s
        .chat_thread_patch(
            "inbox",
            &ChatPatchThreadRequest {
                title: None,
                status: Some(ChatThreadStatus::Archived),
                expected_revision: 1,
            },
            at(12),
        )
        .expect_err("inbox archive");
    assert_eq!(status(e), 409);
    assert_eq!(
        status(
            s.chat_thread_patch("missing", &empty, at(13))
                .expect_err("404")
        ),
        422,
        "an empty patch is rejected before the lookup"
    );
    let e = s
        .chat_thread_patch(
            "missing",
            &ChatPatchThreadRequest {
                title: Some("x".into()),
                status: None,
                expected_revision: 1,
            },
            at(13),
        )
        .expect_err("404");
    assert_eq!(status(e), 404);
}

#[test]
fn chat_store_message_limits() {
    let s = store();
    let t = create(&s, "a", "A", 0);
    let other = create(&s, "b", "B", 0);
    let max = "あ".repeat(CHAT_MESSAGE_TEXT_MAX_BYTES / 3) + "x";
    assert_eq!(max.len(), CHAT_MESSAGE_TEXT_MAX_BYTES);
    post(&s, &t.id, "max", &max, 1);
    let e = s
        .chat_message_post(&t.id, &req("over", &format!("{max}y")), at(2))
        .expect_err("over");
    assert_eq!(status(e), 413);
    let e = s
        .chat_message_post(&t.id, &req("blank", " \n\t"), at(3))
        .expect_err("blank");
    assert_eq!(status(e), 422);
    add_attachment(&s, "a1", &t.id);
    add_attachment(&s, "ax", &other.id);
    let mut with = req("blank-att", "  ");
    with.attachment_ids = vec!["a1".into()];
    let m = s
        .chat_message_post(&t.id, &with, at(4))
        .expect("blank with attachment");
    assert_eq!(m.response.message.attachment_ids, vec!["a1".to_string()]);
    let mut foreign = req("foreign", "x");
    foreign.attachment_ids = vec!["ax".into()];
    assert_eq!(
        status(
            s.chat_message_post(&t.id, &foreign, at(5))
                .expect_err("scope")
        ),
        422
    );
    let mut many = req("many", "x");
    many.attachment_ids = (0..11).map(|i| format!("z{i}")).collect();
    assert_eq!(
        status(s.chat_message_post(&t.id, &many, at(6)).expect_err("count")),
        413
    );
    let mut dup = req("dup", "x");
    dup.attachment_ids = vec!["a1".into(), "a1".into()];
    assert_eq!(
        status(s.chat_message_post(&t.id, &dup, at(6)).expect_err("dup")),
        422
    );
    let mut reply = req("reply", "x");
    reply.reply_to_id = Some("nope".into());
    assert_eq!(
        status(
            s.chat_message_post(&t.id, &reply, at(6))
                .expect_err("reply")
        ),
        422
    );
    assert_eq!(
        status(
            s.chat_message_post("missing", &req("k", "x"), at(6))
                .expect_err("thread")
        ),
        404
    );
    // Queue limit: 100 waiting messages (2 already queued).
    for i in 0..(CHAT_QUEUE_MAX - 2) {
        post(&s, &t.id, &format!("q{i}"), "q", 10);
    }
    let e = s
        .chat_message_post(&t.id, &req("q-over", "q"), at(11))
        .expect_err("queue full");
    assert_eq!(status(e), 429);
    let thread = s.chat_thread_get(&t.id).expect("get").expect("thread");
    assert_eq!(thread.queued_count as usize, CHAT_QUEUE_MAX);
}

#[test]
fn chat_store_message_replay_returns_same_message_and_conflicts_on_change() {
    let s = store();
    let t = create(&s, "a", "A", 0);
    add_attachment(&s, "a1", &t.id);
    let mut r = req("c1", "直して");
    r.attachment_ids = vec!["a1".into()];
    let first = s.chat_message_post(&t.id, &r, at(1)).expect("post");
    assert!(first.created);
    assert_eq!(first.response.queue_position, 1);
    assert_eq!(first.response.run_id, None);
    assert_eq!(first.response.message.seq, 1);
    let again = s.chat_message_post(&t.id, &r, at(2)).expect("replay");
    assert!(!again.created);
    assert_eq!(again.response, first.response);
    let mut text = r.clone();
    text.text = "別".into();
    let mut att = r.clone();
    att.attachment_ids = vec![];
    let mut mode = r.clone();
    mode.mode = ChatSendMode::Interrupt;
    for changed in [text, att, mode] {
        assert_eq!(
            status(
                s.chat_message_post(&t.id, &changed, at(3))
                    .expect_err("conflict")
            ),
            409
        );
    }
    let next = post(&s, &t.id, "c2", "次", 4);
    assert_eq!(next.seq, 2, "a replay does not consume seq");
    let events = s
        .chat_events_page(&t.id, &ChatEventQuery::default())
        .expect("events");
    let message_events = events
        .items
        .iter()
        .filter(|e| e.event_type == ChatEventType::Message)
        .count();
    assert_eq!(message_events, 2);
}

#[test]
fn chat_store_message_pages_with_snapshot_event_id() {
    let s = store();
    let t = create(&s, "a", "A", 0);
    for i in 1..=5 {
        post(&s, &t.id, &format!("m{i}"), &format!("text {i}"), i);
    }
    let seqs = |r: &ChatMessageListResponse| r.items.iter().map(|m| m.seq).collect::<Vec<_>>();
    let latest = s
        .chat_message_list(
            &t.id,
            &ChatMessageQuery {
                limit: Some(2),
                ..Default::default()
            },
        )
        .expect("latest");
    assert_eq!(seqs(&latest), vec![4, 5]);
    assert_eq!(latest.next_before_seq, Some(4));
    let detail = s.chat_thread_detail(&t.id).expect("detail");
    assert_eq!(latest.snapshot_event_id, detail.last_event_id);
    let older = s
        .chat_message_list(
            &t.id,
            &ChatMessageQuery {
                before_seq: Some(4),
                limit: Some(2),
                ..Default::default()
            },
        )
        .expect("older");
    assert_eq!(seqs(&older), vec![2, 3]);
    assert_eq!(older.next_before_seq, Some(2));
    let first = s
        .chat_message_list(
            &t.id,
            &ChatMessageQuery {
                before_seq: Some(2),
                limit: Some(2),
                ..Default::default()
            },
        )
        .expect("first");
    assert_eq!((seqs(&first), first.next_before_seq), (vec![1], None));
    let after = s
        .chat_message_list(
            &t.id,
            &ChatMessageQuery {
                after_seq: Some(1),
                limit: Some(3),
                ..Default::default()
            },
        )
        .expect("after");
    assert_eq!(
        (seqs(&after), after.next_after_seq),
        (vec![2, 3, 4], Some(4))
    );
    let e = s
        .chat_message_list(
            &t.id,
            &ChatMessageQuery {
                before_seq: Some(3),
                after_seq: Some(1),
                limit: None,
            },
        )
        .expect_err("exclusive");
    assert_eq!(status(e), 400);
    let e = s
        .chat_message_list(
            &t.id,
            &ChatMessageQuery {
                limit: Some(201),
                ..Default::default()
            },
        )
        .expect_err("limit");
    assert_eq!(status(e), 400);
    // Events after the snapshot are exactly those written after the read.
    post(&s, &t.id, "m6", "six", 6);
    let tail = s
        .chat_events_page(
            &t.id,
            &ChatEventQuery {
                after: Some(latest.snapshot_event_id.clone()),
                ..Default::default()
            },
        )
        .expect("tail");
    assert_eq!(tail.items.len(), 2, "message + queue for m6");
    match &tail.items[0].data {
        ChatEventData::Message(m) => assert_eq!(m.message.seq, 6),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn chat_store_cancel_only_queued_user_input() {
    let s = store();
    let t = create(&s, "a", "A", 0);
    let m1 = post(&s, &t.id, "m1", "one", 1);
    let m2 = post(&s, &t.id, "m2", "two", 2);
    let run = claim(&s, &t.id, "r1", 3).expect("claim m1");
    assert_eq!(
        status(
            s.chat_message_cancel(&t.id, &m1.id, at(4))
                .expect_err("started")
        ),
        409
    );
    let output = run.output_message_id.clone().expect("output");
    assert_eq!(
        status(
            s.chat_message_cancel(&t.id, &output, at(4))
                .expect_err("assistant")
        ),
        409
    );
    let cancelled = s.chat_message_cancel(&t.id, &m2.id, at(5)).expect("cancel");
    assert_eq!(cancelled.state, ChatMessageState::Cancelled);
    let again = s
        .chat_message_cancel(&t.id, &m2.id, at(6))
        .expect("idempotent");
    assert_eq!(again, cancelled);
    assert_eq!(
        status(
            s.chat_message_cancel(&t.id, "missing", at(6))
                .expect_err("404")
        ),
        404
    );
    s.chat_run_finish("r1", ChatRunState::Completed, None, None, at(7))
        .expect("finish");
    assert!(
        claim(&s, &t.id, "r2", 8).is_none(),
        "cancelled input is not claimed"
    );
}

#[test]
fn chat_store_claim_is_fifo_with_one_live_run() {
    let s = store();
    let t = create(&s, "a", "A", 0);
    let m1 = post(&s, &t.id, "m1", "one", 1);
    let m2 = post(&s, &t.id, "m2", "two", 2);
    let r1 = claim(&s, &t.id, "r1", 3).expect("first");
    assert_eq!(r1.input_message_id, m1.id);
    assert_eq!(r1.state, ChatRunState::Running);
    assert_eq!(r1.harness.as_deref(), Some("claude-code"));
    assert!(
        claim(&s, &t.id, "r2", 4).is_none(),
        "one live run per thread"
    );
    let thread = s.chat_thread_get(&t.id).expect("get").expect("thread");
    assert_eq!(thread.active_run_id.as_deref(), Some("r1"));
    assert_eq!(thread.queued_count, 1);
    s.chat_run_finish("r1", ChatRunState::Completed, Some("done"), None, at(5))
        .expect("finish");
    let r2 = claim(&s, &t.id, "r2", 6).expect("second");
    assert_eq!(r2.input_message_id, m2.id);
}

#[test]
fn chat_store_concurrent_claim_has_one_winner() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("chat.db");
    let first = Arc::new(SqliteStore::open(&path).expect("open"));
    let t = create(&first, "a", "A", 0);
    for i in 0..3 {
        post(&first, &t.id, &format!("m{i}"), "x", 1);
    }
    // Two connections (two stores) and four threads race for the same thread.
    let second = Arc::new(SqliteStore::open(&path).expect("open second"));
    let barrier = Arc::new(Barrier::new(4));
    let handles: Vec<_> = (0..4)
        .map(|i| {
            let s = if i % 2 == 0 {
                first.clone()
            } else {
                second.clone()
            };
            let b = barrier.clone();
            let thread_id = t.id.clone();
            std::thread::spawn(move || {
                b.wait();
                s.chat_run_claim_next(&thread_id, &format!("r{i}"), &json!({}), at(2))
            })
        })
        .collect();
    let mut winners = 0;
    for h in handles {
        match h.join().expect("join") {
            Ok(Some(_)) => winners += 1,
            Ok(None) => {}
            Err(e) => panic!("claim failed: {e}"),
        }
    }
    assert_eq!(winners, 1);
    let live: i64 = first
        .lock()
        .expect("lock")
        .query_row(
            "SELECT COUNT(*) FROM chat_runs WHERE thread_id=?1 AND state IN ('running','stopping')",
            params![t.id],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(live, 1);
}

#[test]
fn chat_store_run_progress_events_and_finish() {
    let s = store();
    let t = create(&s, "a", "A", 0);
    let input = post(&s, &t.id, "m1", "one", 1);
    let run = claim(&s, &t.id, "r1", 2).expect("claim");
    let output = run.output_message_id.clone().expect("output");
    s.chat_run_status("r1", ChatStatusPhase::Thinking, "確認中", at(3))
        .expect("status");
    s.chat_run_append_text("r1", "確認", at(4))
        .expect("delta 1");
    s.chat_run_append_text("r1", "します", at(5))
        .expect("delta 2");
    let long = "é".repeat(3000); // 6000 bytes
    s.chat_run_tool(
        "r1",
        &ChatToolData {
            call_id: "c1".into(),
            name: "Bash".into(),
            state: ChatToolState::Completed,
            summary: "ls".into(),
            detail: Some(long),
            error: false,
            truncated: false,
        },
        at(6),
    )
    .expect("tool");
    let page = s
        .chat_events_page(
            &t.id,
            &ChatEventQuery {
                run_id: Some("r1".into()),
                ..Default::default()
            },
        )
        .expect("run events");
    let deltas: Vec<_> = page
        .items
        .iter()
        .filter_map(|e| match &e.data {
            ChatEventData::TextDelta(d) => Some((d.offset, d.text.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        deltas,
        vec![(0, "確認".to_string()), (6, "します".to_string())]
    );
    let tool = page
        .items
        .iter()
        .find_map(|e| match &e.data {
            ChatEventData::Tool(d) => Some(d.clone()),
            _ => None,
        })
        .expect("tool event");
    assert!(tool.truncated);
    assert!(tool.detail.as_deref().map(str::len).unwrap_or(0) <= CHAT_TOOL_DETAIL_MAX_BYTES);
    assert!(page.items.iter().all(|e| e.run_id.as_deref() == Some("r1")));
    let done = s
        .chat_run_finish(
            "r1",
            ChatRunState::Completed,
            Some("確認しました"),
            None,
            at(7),
        )
        .expect("finish");
    assert_eq!(done.state, ChatRunState::Completed);
    assert!(done.finished_at.is_some());
    let again = s
        .chat_run_finish("r1", ChatRunState::Completed, None, None, at(8))
        .expect("idempotent");
    assert_eq!(again, done);
    assert_eq!(
        status(
            s.chat_run_finish("r1", ChatRunState::Failed, None, None, at(8))
                .expect_err("other terminal")
        ),
        409
    );
    assert_eq!(
        status(
            s.chat_run_append_text("r1", "late", at(9))
                .expect_err("finished")
        ),
        409
    );
    assert_eq!(
        status(
            s.chat_run_finish("r1", ChatRunState::Running, None, None, at(9))
                .expect_err("not terminal")
        ),
        422
    );
    let list = s
        .chat_message_list(&t.id, &ChatMessageQuery::default())
        .expect("messages");
    let by_id = |id: &str| {
        list.items
            .iter()
            .find(|m| m.id == id)
            .cloned()
            .expect("msg")
    };
    assert_eq!(by_id(&input.id).state, ChatMessageState::Completed);
    let out = by_id(&output);
    assert_eq!(
        (out.text.as_str(), out.state),
        ("確認しました", ChatMessageState::Completed)
    );
    assert_eq!(out.reply_to_id.as_deref(), Some(input.id.as_str()));
    assert_eq!(s.chat_run_get(&t.id, "r1").expect("get"), done);
    assert_eq!(status(s.chat_run_get(&t.id, "zz").expect_err("404")), 404);
}

#[test]
fn chat_store_stop_pauses_queue_and_late_stop_is_noop() {
    let s = store();
    let t = create(&s, "a", "A", 0);
    post(&s, &t.id, "m1", "one", 1);
    post(&s, &t.id, "m2", "two", 2);
    post(&s, &t.id, "m3", "three", 3);
    claim(&s, &t.id, "r1", 4).expect("r1");
    let stop = s.chat_run_stop(&t.id, "r1", at(5)).expect("stop");
    assert!(stop.accepted);
    assert_eq!(stop.response.run.state, ChatRunState::Stopping);
    assert!(stop.response.queue_paused);
    let again = s.chat_run_stop(&t.id, "r1", at(6)).expect("stopping again");
    assert!(again.accepted);
    let stopped = s
        .chat_run_finish("r1", ChatRunState::Stopped, None, None, at(7))
        .expect("stopped");
    assert_eq!(stopped.state, ChatRunState::Stopped);
    // Paused: the next queued message does not start.
    assert!(claim(&s, &t.id, "r2", 8).is_none());
    // Terminal replay: 200, nothing changes.
    let replay = s
        .chat_run_stop(&t.id, "r1", at(9))
        .expect("terminal replay");
    assert!(!replay.accepted);
    assert_eq!(replay.response.run.state, ChatRunState::Stopped);
    let thread = s.chat_thread_get(&t.id).expect("get").expect("thread");
    let resumed = s
        .chat_thread_resume_queue(&t.id, thread.revision, at(10))
        .expect("resume");
    assert!(!resumed.queue_paused);
    assert_eq!(
        status(
            s.chat_thread_resume_queue(&t.id, thread.revision, at(10))
                .expect_err("stale revision")
        ),
        409
    );
    let r2 = claim(&s, &t.id, "r2", 11).expect("r2 after resume");
    // A late stop for the old run must not stop or pause the new run.
    let late = s.chat_run_stop(&t.id, "r1", at(12)).expect("late stop");
    assert!(!late.accepted);
    assert!(!late.response.queue_paused);
    assert_eq!(
        s.chat_run_get(&t.id, &r2.id).expect("r2").state,
        ChatRunState::Running
    );
    assert_eq!(
        status(s.chat_run_stop(&t.id, "nope", at(13)).expect_err("404")),
        404
    );
    let other = create(&s, "b", "B", 14);
    assert_eq!(
        status(
            s.chat_run_stop(&other.id, &r2.id, at(15))
                .expect_err("wrong thread")
        ),
        404
    );
    // The paused input of the stopped run is interrupted, not re-queued.
    let msgs = s
        .chat_message_list(&t.id, &ChatMessageQuery::default())
        .expect("msgs");
    assert_eq!(msgs.items[0].state, ChatMessageState::Interrupted);
}

#[test]
fn chat_store_send_with_resume_queue_unpauses() {
    let s = store();
    let t = create(&s, "a", "A", 0);
    post(&s, &t.id, "m1", "one", 1);
    claim(&s, &t.id, "r1", 2).expect("r1");
    s.chat_run_stop(&t.id, "r1", at(3)).expect("stop");
    s.chat_run_finish("r1", ChatRunState::Stopped, None, None, at(4))
        .expect("stopped");
    post(&s, &t.id, "m2", "no resume", 5);
    assert!(
        claim(&s, &t.id, "r2", 6).is_none(),
        "send without resume_queue keeps pause"
    );
    let mut r = req("m3", "resume");
    r.resume_queue = true;
    let posted = s.chat_message_post(&t.id, &r, at(7)).expect("post");
    assert_eq!(posted.response.queue_position, 2);
    let thread = s.chat_thread_get(&t.id).expect("get").expect("thread");
    assert!(!thread.queue_paused);
    let r2 = claim(&s, &t.id, "r2", 8).expect("fifo resumes");
    let m2 = s
        .chat_message_list(&t.id, &ChatMessageQuery::default())
        .expect("msgs")
        .items
        .into_iter()
        .find(|m| m.client_message_id.as_deref() == Some("m2"))
        .expect("m2");
    assert_eq!(r2.input_message_id, m2.id);
}

#[test]
fn chat_store_interrupt_runs_first_then_queue_keeps_pause_state() {
    let s = store();
    let t = create(&s, "a", "A", 0);
    post(&s, &t.id, "m1", "one", 1);
    let m2 = post(&s, &t.id, "m2", "two", 2);
    claim(&s, &t.id, "r1", 3).expect("r1");
    let mut i = req("int", "割り込み");
    i.mode = ChatSendMode::Interrupt;
    let posted = s.chat_message_post(&t.id, &i, at(4)).expect("interrupt");
    assert_eq!(
        posted.response.queue_position, 1,
        "interrupt goes ahead of m2"
    );
    let r1 = s.chat_run_get(&t.id, "r1").expect("r1");
    assert_eq!(r1.state, ChatRunState::Stopping);
    let thread = s.chat_thread_get(&t.id).expect("get").expect("thread");
    assert!(!thread.queue_paused, "interrupt does not pause the queue");
    // Nothing starts until the old run ends.
    assert!(claim(&s, &t.id, "r2", 5).is_none());
    s.chat_run_finish("r1", ChatRunState::Interrupted, None, None, at(6))
        .expect("old run ends");
    let r2 = claim(&s, &t.id, "r2", 7).expect("interrupt runs");
    assert_eq!(r2.input_message_id, posted.response.message.id);
    s.chat_run_finish("r2", ChatRunState::Completed, None, None, at(8))
        .expect("r2 done");
    let r3 = claim(&s, &t.id, "r3", 9).expect("queue continues (not paused)");
    assert_eq!(r3.input_message_id, m2.id);

    // When the queue is paused, only the interrupt message runs.
    let p = create(&s, "b", "B", 10);
    post(&s, &p.id, "p1", "one", 11);
    let p2 = post(&s, &p.id, "p2", "two", 12);
    claim(&s, &p.id, "p-r1", 13).expect("p-r1");
    s.chat_run_stop(&p.id, "p-r1", at(14)).expect("stop pauses");
    let mut pi = req("p-int", "割り込み");
    pi.mode = ChatSendMode::Interrupt;
    let pint = s.chat_message_post(&p.id, &pi, at(15)).expect("interrupt");
    s.chat_run_finish("p-r1", ChatRunState::Stopped, None, None, at(16))
        .expect("stopped");
    let pr2 = claim(&s, &p.id, "p-r2", 17).expect("interrupt ignores pause");
    assert_eq!(pr2.input_message_id, pint.response.message.id);
    s.chat_run_finish("p-r2", ChatRunState::Completed, None, None, at(18))
        .expect("done");
    assert!(claim(&s, &p.id, "p-r3", 19).is_none(), "rest stays paused");
    let thread = s.chat_thread_get(&p.id).expect("get").expect("thread");
    s.chat_thread_resume_queue(&p.id, thread.revision, at(20))
        .expect("resume");
    assert_eq!(
        claim(&s, &p.id, "p-r3", 21)
            .expect("resumed")
            .input_message_id,
        p2.id
    );
}

#[test]
fn chat_store_event_retention_and_cursor_expiry_use_injected_clock() {
    let s = store();
    let t = create(&s, "a", "A", 0);
    let keep_thread = create(&s, "b", "B", 0);
    post(&s, &t.id, "m1", "one", 1);
    claim(&s, &t.id, "r1", 2).expect("r1");
    let first_delta = s.chat_run_append_text("r1", "abc", at(3)).expect("delta");
    let last_delta = s.chat_run_append_text("r1", "def", at(4)).expect("delta");
    s.chat_run_finish("r1", ChatRunState::Completed, None, None, at(5))
        .expect("finish");
    // A live run's deltas are never removed.
    post(&s, &t.id, "m2", "two", 6);
    claim(&s, &t.id, "r2", 7).expect("r2");
    let live_delta = s.chat_run_append_text("r2", "live", at(8)).expect("delta");
    post(&s, &keep_thread.id, "k1", "k", 9);

    let retention = Duration::days(CHAT_EVENT_RETENTION_DAYS);
    // 1 second before the boundary: nothing is removed.
    let removed = s
        .chat_events_retention(at(5) + retention - Duration::seconds(1), retention)
        .expect("retention early");
    assert_eq!(removed, 0);
    let early = (first_delta - 1).to_string();
    s.chat_event_cursor_check(&t.id, &early)
        .expect("not yet expired");

    let removed = s
        .chat_events_retention(at(5) + retention, retention)
        .expect("retention at boundary");
    assert_eq!(removed, 2);
    let expired = s
        .chat_event_cursor_check(&t.id, &early)
        .expect_err("expired");
    assert_eq!(status(expired), 410);
    let e = s
        .chat_events_page(
            &t.id,
            &ChatEventQuery {
                after: Some(first_delta.to_string()),
                ..Default::default()
            },
        )
        .expect_err("page with expired cursor");
    assert_eq!(status(e), 410);
    // At or after the removed range the cursor is valid; 0 replays what remains.
    s.chat_event_cursor_check(&t.id, &last_delta.to_string())
        .expect("cursor at watermark");
    let all = s
        .chat_events_page(
            &t.id,
            &ChatEventQuery {
                after: Some("0".into()),
                limit: Some(500),
                ..Default::default()
            },
        )
        .expect("from 0");
    let ids: Vec<i64> = all
        .items
        .iter()
        .map(|e| e.id.parse().expect("id"))
        .collect();
    assert!(!ids.contains(&first_delta) && !ids.contains(&last_delta));
    assert!(ids.contains(&live_delta));
    assert!(
        all.items.iter().any(|e| e.event_type == ChatEventType::Run),
        "run/message events stay"
    );
    // The other thread has no watermark.
    s.chat_event_cursor_check(&keep_thread.id, "1")
        .expect("other thread");
    // Messages keep the full text.
    let msgs = s
        .chat_message_list(&t.id, &ChatMessageQuery::default())
        .expect("msgs");
    assert!(msgs.items.iter().any(|m| m.text == "abcdef"));
    // Re-running retention is a no-op.
    assert_eq!(
        s.chat_events_retention(at(5) + retention * 2, retention)
            .expect("again"),
        0
    );
}

#[test]
fn chat_store_future_and_malformed_cursors_are_bad_requests() {
    let s = store();
    let t = create(&s, "a", "A", 0);
    let last: i64 = s
        .chat_thread_detail(&t.id)
        .expect("detail")
        .last_event_id
        .parse()
        .expect("id");
    s.chat_event_cursor_check(&t.id, &last.to_string())
        .expect("current");
    for bad in [(last + 1).to_string(), "-1".into(), "abc".into(), "".into()] {
        let e = s
            .chat_event_cursor_check(&t.id, &bad)
            .expect_err("bad cursor");
        assert_eq!(status(e), 400, "{bad:?}");
    }
    assert_eq!(
        status(s.chat_event_cursor_check("missing", "0").expect_err("404")),
        404
    );
    let e = s
        .chat_events_page(
            &t.id,
            &ChatEventQuery {
                limit: Some(501),
                ..Default::default()
            },
        )
        .expect_err("limit");
    assert_eq!(status(e), 400);
}

/// ADR 2026-10-08-cos-workspace-files-in-chat D1/D2: workspace files pinned to a CoS reply appear
/// in `attachment_ids` and `workspace_files`; only assistant replies and same-thread blobs.
#[test]
fn cos_workspace_files_attach_to_assistant_reply() {
    let s = store();
    let t = create(&s, "ws-files", "Workspace", 0);
    let other = create(&s, "ws-other", "Other", 0);
    let input = post(&s, &t.id, "c1", "手順を書いて", 1);
    let run = claim(&s, &t.id, "run-ws", 2).expect("claimed");
    let output = run.output_message_id.clone().expect("output message");
    let md = "01M4CDNAZF1CDBZVHFDZ1JZJA1";
    let foreign = "01M4CDNAZF1CDBZVHFDZ1JZJA2";
    add_attachment(&s, md, &t.id);
    add_attachment(&s, foreign, &other.id);
    let files = vec![ChatWorkspaceFile {
        path: "artifacts/setup.md".into(),
        attachment_id: md.into(),
    }];

    assert_eq!(
        status(
            s.chat_message_attach_workspace_files(&t.id, &input.id, &files, at(3))
                .expect_err("user message")
        ),
        422
    );
    let wrong = vec![ChatWorkspaceFile {
        path: "x.md".into(),
        attachment_id: foreign.into(),
    }];
    assert_eq!(
        status(
            s.chat_message_attach_workspace_files(&t.id, &output, &wrong, at(3))
                .expect_err("other thread")
        ),
        409
    );

    let message = s
        .chat_message_attach_workspace_files(&t.id, &output, &files, at(3))
        .expect("attach");
    assert_eq!(message.attachment_ids, vec![md.to_string()]);
    assert_eq!(message.workspace_files, files);
    // Idempotent, and the run's terminal message event carries the files.
    s.chat_message_attach_workspace_files(&t.id, &output, &files, at(4))
        .expect("repeat");
    s.chat_run_finish(
        "run-ws",
        ChatRunState::Completed,
        Some("`artifacts/setup.md` を見て"),
        None,
        at(5),
    )
    .expect("finish");
    let listed = s
        .chat_message_list(&t.id, &ChatMessageQuery::default())
        .expect("list")
        .items
        .into_iter()
        .find(|m| m.id == output)
        .expect("reply");
    assert_eq!(listed.attachment_ids, vec![md.to_string()]);
    assert_eq!(listed.workspace_files, files);
    let json = serde_json::to_value(&listed).expect("json");
    assert_eq!(json["workspace_files"][0]["path"], "artifacts/setup.md");
    let expires: Option<String> = s
        .lock()
        .expect("lock")
        .query_row(
            "SELECT expires_at FROM chat_attachments WHERE id=?1",
            [md],
            |r| r.get(0),
        )
        .expect("row");
    assert_eq!(expires, None, "a pinned blob is not an orphan");
}

#[test]
fn cos_chat_mcp_metadata_and_atomic_validation() {
    let s = store();
    let input = s
        .chat_mcp_instruct("chatgpt-rdc", None, None, "依頼", at(0))
        .unwrap()
        .response
        .message;
    store::read_tx(&s, |conn| {
        let (author, legacy): (String, Option<String>) = conn.query_row(
            "SELECT json_extract(metadata_json,'$.author'),legacy_message_id FROM chat_messages WHERE id=?1",
            [&input.id], |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        assert_eq!(author, "mcp:chatgpt-rdc");
        assert_eq!(legacy, None);
        Ok(())
    }).unwrap();
    assert!(
        s.chat_mcp_instruct("chatgpt", None, None, "", at(1))
            .is_err()
    );
    assert!(
        s.chat_mcp_instruct("chatgpt", None, Some("missing"), "依頼", at(1))
            .is_err()
    );
    let threads = s.chat_thread_list(&ChatThreadQuery::default()).unwrap();
    assert_eq!(threads.items.len(), 1);
}

#[test]
fn cos_chat_mcp_title_truncates_unicode_without_truncating_input() {
    let s = store();
    let text = format!("{}\nsecond line", "調".repeat(80));
    let input = s
        .chat_mcp_instruct("chatgpt", None, None, &text, at(0))
        .unwrap()
        .response
        .message;
    assert_eq!(input.text, text);
    assert_eq!(
        s.chat_thread_get(&input.thread_id).unwrap().unwrap().title,
        format!("chatgpt: {}", "調".repeat(60))
    );
}
