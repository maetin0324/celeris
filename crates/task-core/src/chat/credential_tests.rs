use rusqlite::params;
use serde_json::json;
use sha2::Digest;
use time::{Duration, OffsetDateTime};

use super::*;
use crate::store::SqliteStore;

fn at(secs: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_791_158_400 + secs).expect("valid time")
}

fn live_run(store: &SqliteStore, key: &str, now: i64) -> (String, String) {
    let thread = store
        .chat_thread_create(
            "admin",
            &ChatCreateThreadRequest {
                title: "thread".into(),
                project_id: None,
                client_thread_id: key.into(),
            },
            at(now),
        )
        .expect("thread")
        .thread;
    store
        .chat_message_post(
            &thread.id,
            &ChatPostMessageRequest {
                client_message_id: format!("message-{key}"),
                text: "input".into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
            },
            at(now + 1),
        )
        .expect("message");
    let run_id = format!("run-{key}");
    store
        .chat_run_claim_next(&thread.id, &run_id, &json!({}), at(now + 2))
        .expect("claim")
        .expect("run");
    (thread.id, run_id)
}

#[test]
fn cos_chat_ops_cred_issue_verify_hash_only_and_classify() {
    let store = SqliteStore::open_in_memory().expect("store");
    let (thread_id, run_id) = live_run(&store, "a", 0);
    let token = store
        .cos_run_credential_issue_at(&thread_id, &run_id, Duration::seconds(30), at(3))
        .expect("issue");
    assert_eq!(token.len(), 64);
    assert_eq!(
        store
            .cos_run_credential_verify(&token, at(4))
            .expect("verify"),
        CosRunIdentity {
            thread_id: thread_id.clone(),
            run_id: run_id.clone()
        }
    );
    let conn = store.lock().expect("lock");
    let (hash, count): (String, i64) = conn
        .query_row(
            "SELECT token_hash, (SELECT COUNT(*) FROM cos_run_credentials WHERE token_hash=?1) \
         FROM cos_run_credentials WHERE run_id=?2",
            params![
                format!("{:x}", sha2::Sha256::digest(token.as_bytes())),
                run_id
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("stored hash");
    assert_ne!(hash, token);
    assert_eq!(count, 1);
    drop(conn);
    assert!(matches!(
        store.cos_run_credential_verify("unknown", at(4)),
        Err(CosRunCredentialError::Unknown)
    ));
    assert!(matches!(
        store.cos_run_credential_verify(&token, at(33)),
        Err(CosRunCredentialError::Expired)
    ));
    store
        .cos_run_credential_revoke(&run_id, at(5))
        .expect("revoke");
    assert!(matches!(
        store.cos_run_credential_verify(&token, at(6)),
        Err(CosRunCredentialError::Revoked)
    ));
}

#[test]
fn cos_chat_ops_cred_terminal_run_revokes_in_same_transition() {
    let store = SqliteStore::open_in_memory().expect("store");
    let (thread_id, run_id) = live_run(&store, "b", 0);
    let token = store
        .cos_run_credential_issue_at(&thread_id, &run_id, Duration::hours(1), at(3))
        .expect("issue");
    store
        .chat_run_finish(&run_id, ChatRunState::Completed, Some("done"), None, at(4))
        .expect("finish");
    assert!(matches!(
        store.cos_run_credential_verify(&token, at(5)),
        Err(CosRunCredentialError::Revoked)
    ));
    let conn = store.lock().expect("lock");
    let state: (String, Option<String>) = conn.query_row(
        "SELECT r.state,c.revoked_at FROM chat_runs r JOIN cos_run_credentials c ON c.run_id=r.run_id WHERE r.run_id=?1",
        [&run_id], |row| Ok((row.get(0)?, row.get(1)?)),
    ).expect("rows");
    assert_eq!(state.0, "completed");
    assert!(state.1.is_some());
}

#[test]
fn cos_chat_ops_cred_requires_live_run_and_positive_ttl() {
    let store = SqliteStore::open_in_memory().expect("store");
    let (thread_id, run_id) = live_run(&store, "c", 0);
    assert!(matches!(
        store.cos_run_credential_issue_at(&thread_id, &run_id, Duration::ZERO, at(3)),
        Err(store::ChatError::Invalid(_))
    ));
    let token = store
        .cos_run_credential_issue_at(&thread_id, &run_id, Duration::seconds(30), at(3))
        .expect("issue");
    assert!(matches!(
        store.cos_run_credential_issue_at(&thread_id, &run_id, Duration::seconds(30), at(3)),
        Err(store::ChatError::Conflict(_))
    ));
    store
        .chat_run_finish(&run_id, ChatRunState::Stopped, None, None, at(4))
        .expect("finish");
    assert!(matches!(
        store.cos_run_credential_verify(&token, at(5)),
        Err(CosRunCredentialError::Revoked)
    ));
    assert!(matches!(
        store.cos_run_credential_issue_at(&thread_id, &run_id, Duration::seconds(30), at(5)),
        Err(store::ChatError::Conflict(_))
    ));
}

#[test]
fn cos_chat_ops_checkpoint_enforces_cursor_and_size() {
    let store = SqliteStore::open_in_memory().expect("store");
    let (thread_id, run_id) = live_run(&store, "d", 0);
    let saved = store
        .chat_thread_checkpoint_at(&thread_id, &run_id, "要約", 1, 0, at(3))
        .expect("save");
    assert_eq!(saved.summary_through_seq, 1);
    assert!(matches!(
        store.chat_thread_checkpoint_at(&thread_id, &run_id, "stale", 1, 0, at(4)),
        Err(store::ChatError::Conflict(_))
    ));
    assert!(matches!(
        store.chat_thread_checkpoint_at(&thread_id, &run_id, "future", 2, 1, at(4)),
        Err(store::ChatError::Invalid(_))
    ));
    assert!(matches!(
        store.chat_thread_checkpoint_at(&thread_id, &run_id, &"あ".repeat(10_923), 1, 1, at(4)),
        Err(store::ChatError::TooLarge(_))
    ));
    let conn = store.lock().expect("lock");
    let summary: (String, i64) = conn
        .query_row(
            "SELECT summary,summary_through_seq FROM chat_threads WHERE id=?1",
            [&thread_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("summary");
    assert_eq!(summary, ("要約".into(), 1));
}

#[test]
fn cos_chat_ops_checkpoint_requires_current_run() {
    let store = SqliteStore::open_in_memory().expect("store");
    let (thread_id, run_id) = live_run(&store, "e", 0);
    assert!(matches!(
        store.chat_thread_checkpoint_at(&thread_id, "wrong", "x", 0, 0, at(3)),
        Err(store::ChatError::Conflict(_))
    ));
    store
        .chat_run_finish(&run_id, ChatRunState::Interrupted, None, None, at(4))
        .expect("finish");
    assert!(matches!(
        store.chat_thread_checkpoint_at(&thread_id, &run_id, "x", 1, 0, at(5)),
        Err(store::ChatError::Conflict(_))
    ));
}

#[test]
fn cos_chat_ops_checkpoint_rejects_queued_gap_before_interrupt_input() {
    let store = SqliteStore::open_in_memory().expect("store");
    let thread = store
        .chat_thread_create(
            "admin",
            &ChatCreateThreadRequest {
                title: "interrupt".into(),
                project_id: None,
                client_thread_id: "gap".into(),
            },
            at(0),
        )
        .expect("thread")
        .thread;
    for (key, mode) in [
        ("ordinary", ChatSendMode::Queue),
        ("urgent", ChatSendMode::Interrupt),
    ] {
        store
            .chat_message_post(
                &thread.id,
                &ChatPostMessageRequest {
                    client_message_id: key.into(),
                    text: key.into(),
                    attachment_ids: vec![],
                    reply_to_id: None,
                    mode,
                    resume_queue: false,
                },
                at(1),
            )
            .expect("post");
    }
    let run = store
        .chat_run_claim_next(&thread.id, "urgent-run", &json!({}), at(2))
        .expect("claim")
        .expect("run");
    assert_eq!(run.id, "urgent-run");
    assert!(matches!(
        store.chat_thread_checkpoint_at(&thread.id, &run.id, "summary", 2, 0, at(3)),
        Err(store::ChatError::Invalid(_))
    ));
}
