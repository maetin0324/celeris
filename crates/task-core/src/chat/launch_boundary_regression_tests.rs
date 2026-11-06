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
