//! The env every CoS chat/triage run gets (`cos_run_env`, passed to `with_env` at launch).

use super::{COS_API_URL_ENV, COS_RUN_CREDENTIAL_ENV, cos_run_env, path_with_first};
use std::path::{Path, PathBuf};

fn value<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
    env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

#[test]
fn cos_chat_run_launch_sets_api_url_env_and_path() {
    let env = cos_run_env("http://127.0.0.1:7700/api/v1", "celeris-cos-run.x");
    assert_eq!(
        value(&env, COS_API_URL_ENV),
        Some("http://127.0.0.1:7700/api/v1")
    );
    assert_eq!(
        value(&env, COS_RUN_CREDENTIAL_ENV),
        Some("celeris-cos-run.x")
    );

    // The daemon's own executable dir (where its release's celerisctl lives) comes first, once.
    let exe_dir: PathBuf = std::env::current_exe()
        .expect("exe")
        .parent()
        .expect("exe dir")
        .to_path_buf();
    let path = value(&env, "PATH").expect("PATH set");
    let entries: Vec<_> = std::env::split_paths(path).collect();
    assert_eq!(entries.first(), Some(&exe_dir), "PATH={path}");
    assert_eq!(entries.iter().filter(|p| **p == exe_dir).count(), 1);
    // The inherited PATH stays behind it, so `sh` and the harness CLIs still resolve.
    if let Some(inherited) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&inherited).filter(|p| *p != exe_dir) {
            assert!(entries.contains(&dir), "{dir:?} dropped from PATH={path}");
        }
    }

    // No `[api] listen`: no CELERIS_API_URL; the prompt's own explanation applies.
    let env = cos_run_env("", "celeris-cos-run.x");
    assert!(value(&env, COS_API_URL_ENV).is_none());
    assert_eq!(
        value(&env, COS_RUN_CREDENTIAL_ENV),
        Some("celeris-cos-run.x")
    );

    let already = path_with_first(
        Some(Path::new("/rel/bin")),
        Some("/rel/bin:/usr/bin:/rel/bin".into()),
    );
    assert_eq!(already.as_deref(), Some("/rel/bin:/usr/bin"));
    assert_eq!(path_with_first(None, Some("/usr/bin".into())), None);
}

mod resume_delta {
    use super::super::advance_delivery_cursor;
    use serde_json::json;
    use task_core::SqliteStore;
    use task_core::chat::{
        ChatCreateThreadRequest, ChatPostMessageRequest, ChatRunSessionMode, ChatRunState,
        ChatSendMode, ChatSession, ChatSessionKey,
    };
    use time::OffsetDateTime;

    fn now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_791_158_400).expect("clock")
    }

    fn post(s: &SqliteStore, t: &str, key: &str) {
        s.chat_message_post(
            t,
            &ChatPostMessageRequest {
                client_message_id: key.into(),
                text: key.into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
            },
            now(),
        )
        .expect("post");
    }

    /// Thread with a live session; returns (thread, session row id).
    fn setup(s: &SqliteStore) -> (String, String) {
        let t = s
            .chat_thread_create(
                "admin",
                &ChatCreateThreadRequest {
                    title: "t".into(),
                    project_id: None,
                    client_thread_id: "t".into(),
                },
                now(),
            )
            .expect("thread")
            .thread
            .id;
        let row = ChatSession::new(
            ChatSessionKey {
                thread_id: t.clone(),
                harness: "fake".into(),
                ..Default::default()
            },
            "sid",
            now(),
        );
        s.chat_session_rotate(&row, now()).expect("session");
        (t, row.id)
    }

    fn claim(s: &SqliteStore, t: &str, run: &str, row: &str) {
        s.chat_run_claim_next(t, run, &json!({}), now())
            .expect("claim")
            .expect("claimed");
        s.chat_run_record_session(run, ChatRunSessionMode::Resumed, row, None, now())
            .expect("record");
    }

    fn cursor(s: &SqliteStore, t: &str) -> i64 {
        s.chat_session_active(t)
            .expect("session")
            .expect("live")
            .delivered_through_seq
    }

    #[test]
    fn cos_chat_resume_delta_cursor_covers_the_reply_only_over_queued_user_messages() {
        let s = SqliteStore::open_in_memory().expect("store");
        let (t, row) = setup(&s);
        post(&s, &t, "one"); // seq 1
        post(&s, &t, "two"); // seq 2, waits in the queue
        claim(&s, &t, "r1", &row); // reply seq 3
        // Not completed yet: nothing moves.
        assert_eq!(
            advance_delivery_cursor(&s, &t, "r1").expect("advance"),
            None
        );
        assert_eq!(cursor(&s, &t), 0);
        s.chat_run_finish("r1", ChatRunState::Completed, Some("ok"), None, now())
            .expect("finish");
        // Only a queued user message lies between the input and the reply.
        assert_eq!(
            advance_delivery_cursor(&s, &t, "r1").expect("advance"),
            Some(3)
        );
        assert_eq!(cursor(&s, &t), 3);
    }

    #[test]
    fn cos_chat_resume_delta_cursor_stops_at_the_input_before_an_unseen_message() {
        let s = SqliteStore::open_in_memory().expect("store");
        let (t, row) = setup(&s);
        post(&s, &t, "one"); // seq 1
        s.chat_system_message_add(&t, "card", &[], now())
            .expect("system"); // seq 2: not in this run's prompt
        claim(&s, &t, "r1", &row); // reply seq 3
        s.chat_run_finish("r1", ChatRunState::Completed, Some("ok"), None, now())
            .expect("finish");
        assert_eq!(
            advance_delivery_cursor(&s, &t, "r1").expect("advance"),
            Some(1)
        );
        assert_eq!(cursor(&s, &t), 1, "seq 2 is redelivered by the next resume");

        // A failed run leaves the cursor.
        post(&s, &t, "two"); // seq 4
        claim(&s, &t, "r2", &row);
        s.chat_run_finish("r2", ChatRunState::Failed, None, Some("boom"), now())
            .expect("finish");
        assert_eq!(
            advance_delivery_cursor(&s, &t, "r2").expect("advance"),
            None
        );
        assert_eq!(cursor(&s, &t), 1);
    }
}
