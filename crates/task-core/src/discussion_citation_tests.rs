//! ADR 2026-11-21-chat-discussion-mode D6・D8.1 (WU summary-cite): `chat_discussion_summary_*`.
//!
//! 守る不変条件: 要約・案の引用は同じ thread か統括の thread の、今の revision と本文の sha256 の発言だけを引ける。
//! 引用の状態（current・edited・retracted・missing）は保存せず、引いた message の今の revision・本文・撤回から毎回導く。
//! 編集・撤回はその場で引用を失効させる（T5）。別の案件の thread の発言は引けない（T4）。出典の無い要約は拒否する。
//! 既存の試験では検出できない: base には引用の型・検査・失効の導出が無い（message は revision を持つが、引く側が無い）。

use std::collections::BTreeSet;

use serde_json::{Value, json};
use time::OffsetDateTime;

use crate::chat::*;
use crate::chat_citation::*;
use crate::project_proposal::sha256_hex;
use crate::store::SqliteStore;

const ME: &str = "human:alice";
const PROJECT: &str = "p-cite";

fn at(secs: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_791_158_400 + secs).expect("valid ts")
}

fn project_thread(s: &SqliteStore, project: &str) -> String {
    crate::chat::test_project_thread(s, project, at(0)).id
}

fn general_thread(s: &SqliteStore) -> String {
    s.cos_thread_open(&ChatOpenThreadRequest { project_id: None }, at(0))
        .expect("general thread")
        .thread
        .id
}

/// A delivered (`completed`) message of `ME` (a queued one is cancelled, not retracted).
fn post(s: &SqliteStore, t: &str, key: &str, text: &str, now: i64) -> ChatMessage {
    let m = queued(s, t, key, text, now);
    let run = format!("run-{key}");
    s.chat_run_claim_next(t, &run, &json!({"harness": "claude-code"}), at(now))
        .expect("claim")
        .expect("claimed");
    s.chat_run_finish(&run, ChatRunState::Completed, Some("ok"), None, at(now))
        .expect("finish");
    s.chat_message_get(t, &m.id).expect("get")
}

fn queued(s: &SqliteStore, t: &str, key: &str, text: &str, now: i64) -> ChatMessage {
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
        ME,
        at(now),
    )
    .expect("post")
    .response
    .message
}

fn facts(s: &SqliteStore, t: &str, m: &str) -> Option<CitedMessage> {
    s.chat_cited_message(t, m).expect("read cited message")
}

/// The citation a summary written now would carry.
fn cite_now(s: &SqliteStore, t: &str, m: &str) -> ChatCitation {
    let f = facts(s, t, m).expect("message exists");
    ChatCitation {
        thread_id: f.thread_id,
        message_id: f.message_id,
        revision: f.revision,
        sha256: f.sha256,
    }
}

fn summary(thread: &str, project: Option<&str>, citations: Vec<ChatCitation>) -> DiscussionSummary {
    DiscussionSummary {
        schema: DISCUSSION_SUMMARY_SCHEMA.into(),
        project_id: project.map(str::to_string),
        thread_id: thread.into(),
        through_seq: 100,
        body: "## 要約\n- 最小の辺の集合を先に測る案が出た".into(),
        citations,
    }
}

/// The summary check as the attach will run it: the cited messages are read now.
fn summary_codes(
    s: &SqliteStore,
    thread: &str,
    project: Option<&str>,
    doc: &DiscussionSummary,
) -> BTreeSet<&'static str> {
    let cited: Vec<CitedMessage> = doc
        .citations
        .iter()
        .filter_map(|c| facts(s, &c.thread_id, &c.message_id))
        .collect();
    let ctx = SummaryContext {
        thread_id: thread,
        project_id: project,
        cited: &cited,
    };
    check_discussion_summary(doc, &ctx)
        .into_iter()
        .map(|v| v.code)
        .collect()
}

fn edit(s: &SqliteStore, t: &str, m: &str, text: &str, expected_revision: u64, now: i64) {
    s.chat_message_edit(
        t,
        m,
        ME,
        &ChatMessageEditRequest {
            text: text.into(),
            expected_revision,
        },
        at(now),
    )
    .expect("edit");
}

#[test]
fn chat_discussion_summary_citation_current() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = project_thread(&s, PROJECT);
    let m = post(&s, &t, "c1", "最小の辺の集合を先に測りたい", 1);
    let f = facts(&s, &t, &m.id).expect("facts");
    assert_eq!(f.revision, 1);
    assert_eq!(f.sha256, sha256_hex("最小の辺の集合を先に測りたい"));
    assert_eq!(f.project_id.as_deref(), Some(PROJECT));
    assert!(!f.retracted && !f.general_thread);

    let c = cite_now(&s, &t, &m.id);
    assert_eq!(citation_state(&c, Some(&f)), ChatCitationState::Current);
    assert_eq!(verify_citation(&t, &c, Some(&f)), Ok(()));
    let doc = summary(&t, Some(PROJECT), vec![c]);
    assert!(summary_codes(&s, &t, Some(PROJECT), &doc).is_empty());
    // The file round-trips through its wire form.
    let bytes = serde_json::to_vec(&doc).expect("json");
    assert_eq!(DiscussionSummary::parse(&bytes).expect("parse"), doc);
}

#[test]
fn chat_discussion_summary_stale_after_edit() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = project_thread(&s, PROJECT);
    let m = post(&s, &t, "e1", "仮説 A を先に試す", 1);
    let c = cite_now(&s, &t, &m.id);
    let doc = summary(&t, Some(PROJECT), vec![c.clone()]);
    assert!(summary_codes(&s, &t, Some(PROJECT), &doc).is_empty());

    edit(&s, &t, &m.id, "仮説 B を先に試す", 1, 2);
    let f = facts(&s, &t, &m.id).expect("facts");
    assert_eq!(f.revision, 2);
    assert_eq!(citation_state(&c, Some(&f)), ChatCitationState::Edited);
    assert_eq!(
        verify_citation(&t, &c, Some(&f)),
        Err(ChatCitationRefusal::Edited)
    );
    assert_eq!(
        summary_codes(&s, &t, Some(PROJECT), &doc),
        BTreeSet::from(["chat_citation_edited"])
    );
    // A citation of the new revision is current again.
    let again = cite_now(&s, &t, &m.id);
    assert_eq!(again.revision, 2);
    assert_eq!(citation_state(&again, Some(&f)), ChatCitationState::Current);
}

#[test]
fn chat_discussion_summary_stale_after_retract() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = project_thread(&s, PROJECT);
    let m = post(&s, &t, "r1", "この前提は誤りだった", 1);
    let c = cite_now(&s, &t, &m.id);
    s.chat_message_retract(
        &t,
        &m.id,
        ME,
        &ChatMessageRetractRequest {
            expected_revision: 1,
            reason: "誤り".into(),
        },
        at(2),
    )
    .expect("retract");
    let f = facts(&s, &t, &m.id).expect("facts");
    assert!(f.retracted);
    assert_eq!(citation_state(&c, Some(&f)), ChatCitationState::Retracted);
    // Citing the retracted message at its new revision is refused too.
    let now = cite_now(&s, &t, &m.id);
    assert_eq!(
        verify_citation(&t, &now, Some(&f)),
        Err(ChatCitationRefusal::Retracted)
    );
    let doc = summary(&t, Some(PROJECT), vec![c]);
    assert_eq!(
        summary_codes(&s, &t, Some(PROJECT), &doc),
        BTreeSet::from(["chat_citation_retracted"])
    );
}

#[test]
fn chat_discussion_summary_citation_other_thread_refused() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = project_thread(&s, PROJECT);
    let other = project_thread(&s, "p-other");
    let general = general_thread(&s);
    let mine = post(&s, &t, "o1", "この案件の発言", 1);
    let foreign = post(&s, &other, "o2", "別の案件の発言", 2);
    let global = post(&s, &general, "o3", "統括の thread の発言", 3);

    let foreign_c = cite_now(&s, &other, &foreign.id);
    let foreign_f = facts(&s, &other, &foreign.id);
    assert_eq!(
        verify_citation(&t, &foreign_c, foreign_f.as_ref()),
        Err(ChatCitationRefusal::OtherThread)
    );
    let global_c = cite_now(&s, &general, &global.id);
    let global_f = facts(&s, &general, &global.id).expect("facts");
    assert!(global_f.general_thread && global_f.project_id.is_none());
    assert_eq!(verify_citation(&t, &global_c, Some(&global_f)), Ok(()));

    let mine_c = cite_now(&s, &t, &mine.id);
    let doc = summary(&t, Some(PROJECT), vec![mine_c.clone(), global_c, foreign_c]);
    assert_eq!(
        summary_codes(&s, &t, Some(PROJECT), &doc),
        BTreeSet::from(["chat_citation_other_thread"])
    );
    // A summary of another thread or project is not attached here.
    let elsewhere = summary(&other, Some("p-other"), vec![mine_c]);
    assert_eq!(
        summary_codes(&s, &t, Some(PROJECT), &elsewhere),
        BTreeSet::from(["discussion_summary_other_thread"])
    );
}

#[test]
fn chat_discussion_summary_citation_wrong_revision_refused() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = project_thread(&s, PROJECT);
    let other = project_thread(&s, "p-other");
    let m = post(&s, &t, "w1", "引用される発言", 1);
    let f = facts(&s, &t, &m.id).expect("facts");
    let good = cite_now(&s, &t, &m.id);

    let ahead = ChatCitation {
        revision: 2,
        ..good.clone()
    };
    let other_body = ChatCitation {
        sha256: sha256_hex("書き換えた本文"),
        ..good.clone()
    };
    for c in [&ahead, &other_body] {
        assert_eq!(citation_state(c, Some(&f)), ChatCitationState::Missing);
        assert_eq!(
            verify_citation(&t, c, Some(&f)),
            Err(ChatCitationRefusal::Missing)
        );
    }
    // The message id under another thread id does not resolve.
    let moved = ChatCitation {
        thread_id: other.clone(),
        ..good.clone()
    };
    assert_eq!(facts(&s, &other, &m.id), None);
    assert_eq!(
        verify_citation(&t, &moved, None),
        Err(ChatCitationRefusal::Missing)
    );
    for bad in [
        ChatCitation {
            revision: 0,
            ..good.clone()
        },
        ChatCitation {
            sha256: good.sha256.to_uppercase(),
            ..good.clone()
        },
        ChatCitation {
            message_id: " ".into(),
            ..good.clone()
        },
    ] {
        assert_eq!(
            verify_citation(&t, &bad, Some(&f)),
            Err(ChatCitationRefusal::Shape)
        );
    }
    let doc = summary(&t, Some(PROJECT), vec![good, ahead]);
    assert_eq!(
        summary_codes(&s, &t, Some(PROJECT), &doc),
        BTreeSet::from(["chat_citation_missing"])
    );
}

#[test]
fn chat_discussion_summary_missing_source_refused() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = project_thread(&s, PROJECT);
    let uncited = summary(&t, Some(PROJECT), vec![]);
    assert_eq!(
        summary_codes(&s, &t, Some(PROJECT), &uncited),
        BTreeSet::from(["discussion_summary_source_missing"])
    );
    let ghost = summary(
        &t,
        Some(PROJECT),
        vec![ChatCitation {
            thread_id: t.clone(),
            message_id: "01M0NOSUCHMESSAGE000000000".into(),
            revision: 1,
            sha256: sha256_hex("無い発言"),
        }],
    );
    assert_eq!(
        summary_codes(&s, &t, Some(PROJECT), &ghost),
        BTreeSet::from(["chat_citation_missing"])
    );
    // A file without `citations` is not a summary.
    let mut v = serde_json::to_value(&uncited).expect("json");
    v.as_object_mut().expect("object").remove("citations");
    let err =
        DiscussionSummary::parse(&serde_json::to_vec(&v).expect("json")).expect_err("refused");
    assert_eq!(err.code, SUMMARY_INVALID);
}

#[test]
fn chat_discussion_summary_state_is_not_stored() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = project_thread(&s, PROJECT);
    let m = post(&s, &t, "n1", "状態は保存しない", 1);
    let c = cite_now(&s, &t, &m.id);
    // The citation's wire form has exactly the four fields; a stored state is refused.
    let v = serde_json::to_value(&c).expect("json");
    let keys: BTreeSet<&str> = v
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from(["message_id", "revision", "sha256", "thread_id"])
    );
    let mut with_state = v.clone();
    with_state["state"] = json!("current");
    assert!(serde_json::from_value::<ChatCitation>(with_state).is_err());
    // The same unchanged citation value reads `edited` once the message is edited: the state
    // follows the message's revision, nothing about the citation was written.
    let before = c.clone();
    edit(&s, &t, &m.id, "状態は導く", 1, 2);
    assert_eq!(c, before);
    let f = facts(&s, &t, &m.id).expect("facts");
    assert_eq!(citation_state(&c, Some(&f)), ChatCitationState::Edited);
    let meta: Value = s
        .lock()
        .expect("lock")
        .query_row(
            "SELECT metadata_json FROM chat_messages WHERE id=?1",
            [&m.id],
            |r| r.get::<_, String>(0),
        )
        .map(|j| serde_json::from_str(&j).expect("metadata"))
        .expect("row");
    assert!(meta.get("citation_state").is_none() && meta.get("citations").is_none());
}

#[test]
fn chat_discussion_summary_limits_and_shape() {
    let s = SqliteStore::open_in_memory().expect("store");
    let t = project_thread(&s, PROJECT);
    let m = post(&s, &t, "l1", "上限の試験", 1);
    let c = cite_now(&s, &t, &m.id);
    let ok = summary(&t, Some(PROJECT), vec![c.clone()]);

    let mut at_max = ok.clone();
    at_max.body = "a".repeat(DISCUSSION_SUMMARY_BODY_MAX_BYTES);
    assert!(summary_codes(&s, &t, Some(PROJECT), &at_max).is_empty());
    let mut long = ok.clone();
    long.body = "a".repeat(DISCUSSION_SUMMARY_BODY_MAX_BYTES + 1);
    assert_eq!(
        summary_codes(&s, &t, Some(PROJECT), &long),
        BTreeSet::from([SUMMARY_LIMIT])
    );
    let mut many = ok.clone();
    many.citations = vec![c.clone(); DISCUSSION_SUMMARY_CITATIONS_MAX + 1];
    assert_eq!(
        summary_codes(&s, &t, Some(PROJECT), &many),
        BTreeSet::from([SUMMARY_LIMIT])
    );
    let mut schema = ok.clone();
    schema.schema = "celeris.discussion-summary/2".into();
    let mut before_cited = ok.clone();
    before_cited.through_seq = 0;
    let mut empty = ok.clone();
    empty.body = " ".into();
    for doc in [schema, before_cited, empty] {
        assert_eq!(
            summary_codes(&s, &t, Some(PROJECT), &doc),
            BTreeSet::from([SUMMARY_INVALID]),
            "{doc:?}"
        );
    }
    // A summary through seq N cannot cite a later message of its own thread.
    let later = post(&s, &t, "l2", "要約の後の発言", 2);
    let mut past = ok.clone();
    past.through_seq = facts(&s, &t, &m.id).expect("facts").seq;
    past.citations = vec![c, cite_now(&s, &t, &later.id)];
    assert_eq!(
        summary_codes(&s, &t, Some(PROJECT), &past),
        BTreeSet::from([SUMMARY_INVALID])
    );
    let mut extra = serde_json::to_value(&ok).expect("json");
    extra["approved"] = json!(true);
    let err =
        DiscussionSummary::parse(&serde_json::to_vec(&extra).expect("json")).expect_err("refused");
    assert_eq!(err.code, SUMMARY_INVALID);
}
