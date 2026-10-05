use std::sync::Mutex;

use serde_json::json;
use task_core::chat::{
    ChatCreateThreadRequest, ChatPostMessageRequest, ChatRunState, ChatSendMode,
};

use super::*;

fn at(secs: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_791_158_400 + secs).expect("valid ts")
}

const HOUR: i64 = 3600;
const DAY: i64 = 24 * HOUR;

struct Fixture {
    _dir: tempfile::TempDir,
    settings: ChatGcSettings,
    store: SqliteStore,
    thread_id: String,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("celeris.db");
    let store = SqliteStore::open(&db_path).expect("store");
    let thread_id = store
        .chat_thread_create(
            "admin",
            &ChatCreateThreadRequest {
                title: "gc".into(),
                project_id: None,
                client_thread_id: "gc".into(),
            },
            at(0),
        )
        .expect("thread")
        .thread
        .id;
    let settings = ChatGcSettings {
        db_path,
        data_dir: dir.path().to_path_buf(),
        limits: ChatAttachmentLimits::default(),
        stream_retention: time::Duration::days(30),
        busy_timeout: Duration::from_secs(5),
    };
    Fixture {
        _dir: dir,
        settings,
        store,
        thread_id,
    }
}

fn upload(f: &Fixture, key: &str) -> String {
    let attachments =
        ChatAttachmentStore::open(&f.settings.data_dir, &f.settings.db_path, f.settings.limits)
            .expect("attachments");
    attachments
        .upload(&f.thread_id, key, "a.txt", Some(3), &b"abc"[..], at(0))
        .expect("upload")
        .id
}

fn blob(f: &Fixture, id: &str) -> PathBuf {
    f.settings
        .data_dir
        .join("chat/attachments")
        .join(id)
        .join("blob")
}

fn finished_run_with_events(f: &Fixture) {
    f.store
        .chat_message_post(
            &f.thread_id,
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
    f.store
        .chat_run_claim_next(&f.thread_id, "r1", &json!({}), at(2))
        .expect("claim")
        .expect("run");
    f.store
        .chat_run_append_text("r1", "partial", at(3))
        .expect("delta");
    f.store
        .chat_run_finish("r1", ChatRunState::Completed, Some("done"), None, at(4))
        .expect("finish");
}

fn delta_events(f: &Fixture) -> i64 {
    let conn = rusqlite::Connection::open(&f.settings.db_path).expect("conn");
    conn.query_row(
        "SELECT COUNT(*) FROM chat_events WHERE type='text_delta'",
        [],
        |r| r.get(0),
    )
    .expect("count")
}

#[test]
fn chat_gc_removes_unsent_upload_after_orphan_ttl() {
    let f = fixture();
    let id = upload(&f, "u1");
    assert!(blob(&f, &id).is_file());
    let gc = ChatGc::open(&f.settings).expect("gc");
    let early = gc.run_once(at(23 * HOUR)).expect("early pass");
    assert_eq!(early.attachments_deleted, 0);
    assert!(blob(&f, &id).is_file());
    let late = gc.run_once(at(25 * HOUR)).expect("late pass");
    assert_eq!(late.attachments_deleted, 1);
    assert!(!blob(&f, &id).exists());
}

#[test]
fn chat_gc_keeps_referenced_blob_and_deletes_after_unreferenced_retention() {
    let f = fixture();
    let id = upload(&f, "u1");
    let attachments =
        ChatAttachmentStore::open(&f.settings.data_dir, &f.settings.db_path, f.settings.limits)
            .expect("attachments");
    attachments
        .add_ref(&id, "task", "t1", at(HOUR))
        .expect("pin");
    let gc = ChatGc::open(&f.settings).expect("gc");
    assert_eq!(
        gc.run_once(at(40 * DAY)).expect("pass").attachments_deleted,
        0
    );
    assert!(
        blob(&f, &id).is_file(),
        "referenced blobs are never removed"
    );
    attachments
        .remove_ref(&id, "task", "t1", at(41 * DAY))
        .expect("unpin");
    assert_eq!(
        gc.run_once(at(70 * DAY)).expect("pass").attachments_deleted,
        0
    );
    assert_eq!(
        gc.run_once(at(72 * DAY)).expect("pass").attachments_deleted,
        1
    );
    assert!(!blob(&f, &id).exists());
}

#[test]
fn chat_gc_expires_upload_reservations() {
    let f = fixture();
    let conn = rusqlite::Connection::open(&f.settings.db_path).expect("conn");
    conn.execute(
        "INSERT INTO chat_upload_reservations(id,thread_id,client_upload_id,reserved_bytes,\
         lease_expires_at,created_at) VALUES('01M4ZZZZZZZZZZZZZZZZZZZZZZ',?1,'stale',10,\
         '2026-10-05T01:00:00.000Z','2026-10-05T00:00:00.000Z')",
        [&f.thread_id],
    )
    .expect("reservation");
    let gc = ChatGc::open(&f.settings).expect("gc");
    gc.run_once(at(2 * HOUR)).expect("pass");
    let left: i64 = conn
        .query_row("SELECT COUNT(*) FROM chat_upload_reservations", [], |r| {
            r.get(0)
        })
        .expect("count");
    assert_eq!(left, 0, "an expired lease releases its reservation");
}

#[test]
fn chat_gc_applies_stream_retention_after_run_end() {
    let f = fixture();
    finished_run_with_events(&f);
    assert!(delta_events(&f) > 0);
    let gc = ChatGc::open(&f.settings).expect("gc");
    assert_eq!(gc.run_once(at(29 * DAY)).expect("pass").events_removed, 0);
    let report = gc.run_once(at(31 * DAY)).expect("pass");
    assert!(report.events_removed > 0);
    assert_eq!(delta_events(&f), 0);
    // Messages are kept: retention only removes the stream detail.
    let messages = f
        .store
        .chat_thread_get(&f.thread_id)
        .expect("get")
        .expect("thread");
    assert_eq!(messages.id, f.thread_id);
}

#[tokio::test(start_paused = true)]
async fn chat_gc_loop_runs_each_interval_with_injected_clock() {
    let f = fixture();
    let id = upload(&f, "u1");
    finished_run_with_events(&f);
    let now = Arc::new(Mutex::new(at(HOUR)));
    let clock_now = Arc::clone(&now);
    let clock: ChatGcClock = Arc::new(move || *clock_now.lock().expect("clock"));
    let (report_tx, mut report_rx) = tokio::sync::mpsc::unbounded_channel();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(chat_gc_loop(
        f.settings.clone(),
        CHAT_GC_INTERVAL,
        clock,
        Arc::new(|| true),
        stop_rx,
        move |report| {
            let _ = report_tx.send(report);
        },
    ));
    // The first pass runs at start-up.
    let first = report_rx.recv().await.expect("first pass");
    assert_eq!(first, ChatGcReport::default());
    *now.lock().expect("clock") = at(25 * HOUR);
    let second = report_rx.recv().await.expect("second pass");
    assert_eq!(second.attachments_deleted, 1);
    assert_eq!(second.events_removed, 0);
    assert!(!blob(&f, &id).exists());
    *now.lock().expect("clock") = at(31 * DAY);
    let third = report_rx.recv().await.expect("third pass");
    assert!(third.events_removed > 0);
    stop_tx.send(()).expect("stop");
    handle.await.expect("loop ends");
}

#[tokio::test(start_paused = true)]
async fn chat_gc_loop_skips_passes_while_gate_is_closed() {
    let f = fixture();
    let id = upload(&f, "u1");
    let open = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let gate_open = Arc::clone(&open);
    let (report_tx, mut report_rx) = tokio::sync::mpsc::unbounded_channel();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(chat_gc_loop(
        f.settings.clone(),
        CHAT_GC_INTERVAL,
        Arc::new(|| at(25 * HOUR)),
        Arc::new(move || gate_open.load(std::sync::atomic::Ordering::SeqCst)),
        stop_rx,
        move |report| {
            let _ = report_tx.send(report);
        },
    ));
    // Several intervals elapse while the gate (standby) is closed: no pass runs.
    tokio::time::sleep(CHAT_GC_INTERVAL * 3).await;
    assert!(report_rx.try_recv().is_err());
    assert!(blob(&f, &id).is_file());
    open.store(true, std::sync::atomic::Ordering::SeqCst);
    let report = report_rx.recv().await.expect("pass after activation");
    assert_eq!(report.attachments_deleted, 1);
    stop_tx.send(()).expect("stop");
    handle.await.expect("loop ends");
}
