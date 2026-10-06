use celeris::notify::{self, NotifyConfig, SendOutcome};
use task_core::chat::triage::CosTriageSource;
use task_core::{NotificationKind, NotificationStore, SqliteStore};
use time::OffsetDateTime;

fn at() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("clock")
}

fn config() -> NotifyConfig {
    NotifyConfig {
        gui_base_url: Some("http://127.0.0.1:7700/".into()),
        ..Default::default()
    }
}

fn outbox(store: &SqliteStore, kind: NotificationKind) -> task_core::Notification {
    store.notification_upsert_pending(kind, "item-1", &serde_json::json!({
        "summary": "公開前の確認", "options": [{"key":"publish","label":"公開する"},{"key":"hold","label":"保留"}],
        "recommended": "hold", "recommendation_reason": "公開先が未確認",
        "blocking": "案件 A の公開", "web_path": "/tasks/task-id"
    }).to_string(), None, at()).expect("insert").expect("new")
}

#[test]
fn cos_chat_triage_only_escalation_and_fallback_are_sendable() {
    let store = SqliteStore::open_in_memory().expect("db");
    let legacy = store
        .notification_upsert_pending(NotificationKind::Digest, "digest", "old", None, at())
        .expect("insert")
        .expect("new");
    assert!(notify::select_routes_batch(std::slice::from_ref(&legacy)).is_none());
    let escalation = outbox(&store, NotificationKind::CosEscalation);
    let selected = notify::select_routes_batch(&[legacy, escalation.clone()]).expect("route");
    assert_eq!(selected.ids, vec![escalation.id]);
}

#[test]
fn cos_chat_triage_escalation_packet_has_bounded_text_and_absolute_link() {
    let store = SqliteStore::open_in_memory().expect("db");
    let row = outbox(&store, NotificationKind::CosEscalation);
    let body = notify::triage::render(&row, &config()).expect("render");
    for required in [
        "CoS から判断のお願い",
        "公開前の確認",
        "公開する",
        "保留",
        "公開先が未確認",
        "案件 A の公開",
        "http://127.0.0.1:7700/tasks/task-id",
        "回答はリンク先で",
    ] {
        assert!(body.contains(required), "missing {required}: {body}");
    }
    assert!(body.chars().count() <= 1900);
    assert_eq!(
        notify::triage::render(&row, &NotifyConfig::default()),
        Err(notify::triage::NO_GUI_URL)
    );
}

#[test]
fn cos_chat_triage_long_packet_keeps_link_within_1900_characters() {
    let store = SqliteStore::open_in_memory().expect("db");
    let row = store
        .notification_upsert_pending(
            NotificationKind::CosFallback,
            "item-long",
            &serde_json::json!({
                "summary": "あ".repeat(4000), "blocking": "い".repeat(4000),
                "reason": "quota", "web_path": "/inbox", "options": [],
                "secret": "PRIVATE_TOKEN_DO_NOT_SEND", "attachment_body": "PRIVATE_ATTACHMENT_DO_NOT_SEND"
            })
            .to_string(),
            None,
            at(),
        )
        .expect("insert")
        .expect("new");
    let body = notify::triage::render(&row, &config()).expect("render");
    assert!(body.starts_with("CoS 不在のため直接通知"));
    assert!(body.ends_with("http://127.0.0.1:7700/inbox\n回答はリンク先で"));
    assert!(body.chars().count() <= 1900);
    assert!(!body.contains("PRIVATE_TOKEN_DO_NOT_SEND"));
    assert!(!body.contains("PRIVATE_ATTACHMENT_DO_NOT_SEND"));
}

#[tokio::test]
async fn cos_chat_triage_webhook_disables_mentions_and_obeys_retry_after() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept");
        let mut bytes = vec![0; 8192];
        let n = stream.read(&mut bytes).await.expect("read");
        let request = String::from_utf8_lossy(&bytes[..n]).to_string();
        stream
            .write_all(
                b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 2\r\nContent-Length: 0\r\n\r\n",
            )
            .await
            .expect("reply");
        request
    });
    let client = notify::client().expect("client");
    let outcome = notify::post_webhook(&client, &format!("http://{addr}/hook"), "@everyone").await;
    assert_eq!(
        outcome,
        SendOutcome::RateLimited(std::time::Duration::from_secs(2))
    );
    let request = server.await.expect("server");
    assert!(
        request.contains("\"allowed_mentions\":{\"parse\":[]}"),
        "{request}"
    );
}

#[test]
fn cos_chat_triage_cutover_supersedes_legacy_pending_and_is_idempotent() {
    let dir = tempfile::tempdir().expect("dir");
    let db = dir.path().join("test.db");
    let store = SqliteStore::open(&db).expect("db");
    store
        .notification_upsert_pending(NotificationKind::BadNews, "old", "old notice", None, at())
        .expect("insert");
    let view = task_ops::view::ViewContext {
        workspace_root: dir.path().into(),
        retry_backoff_base: std::time::Duration::from_secs(1),
        retry_backoff_max: std::time::Duration::from_secs(60),
        max_requeues: 3,
        clusters: Default::default(),
    };
    notify::triage::cutover(&store, &view, at()).expect("cutover");
    assert_eq!(
        store.cos_triage_route_version().expect("route").as_deref(),
        Some("cos_v1")
    );
    assert!(store.notification_pending().expect("pending").is_empty());
    notify::triage::cutover(&store, &view, at()).expect("idempotent");
}

#[test]
fn cos_chat_triage_resolved_source_is_withdrawn_before_post() {
    let dir = tempfile::tempdir().expect("dir");
    let db = dir.path().join("test.db");
    let store = SqliteStore::open(&db).expect("db");
    let source = CosTriageSource {
        source_kind: "decision".into(),
        source_key: "decision-1".into(),
        source_revision: "r1".into(),
        source_event_id: None,
        operation_id: None,
        summary: "wait".into(),
        policy_version: "1".into(),
    };
    store
        .cos_triage_ingest_batch("events", "0", &[source], at())
        .expect("ingest");
    let conn = rusqlite::Connection::open(&db).expect("connection");
    let id: String = conn
        .query_row("SELECT id FROM cos_inbox_items", [], |r| r.get(0))
        .expect("item");
    store
        .cos_triage_outbox_claim(&id, "escalation", "{}", at())
        .expect("claim");
    let row = store.notification_pending().expect("pending").remove(0);
    let view = task_ops::view::ViewContext {
        workspace_root: dir.path().into(),
        retry_backoff_base: std::time::Duration::from_secs(1),
        retry_backoff_max: std::time::Duration::from_secs(60),
        max_requeues: 3,
        clusters: Default::default(),
    };
    assert!(!notify::triage::withdraw_if_resolved(&store, &db, &view, &row, at()).expect("check"));
    assert!(store.notification_pending().expect("pending").is_empty());
}

#[test]
fn cos_chat_triage_three_send_failures_keep_the_unresolved_item() {
    let dir = tempfile::tempdir().expect("dir");
    let db = dir.path().join("test.db");
    let store = SqliteStore::open(&db).expect("db");
    let source = CosTriageSource {
        source_kind: "decision".into(),
        source_key: "decision-1".into(),
        source_revision: "r1".into(),
        source_event_id: None,
        operation_id: None,
        summary: "wait".into(),
        policy_version: "1".into(),
    };
    store
        .cos_triage_ingest_batch("events", "0", &[source], at())
        .expect("ingest");
    let row = store
        .cos_triage_claim("run-1", at())
        .expect("claim")
        .expect("item");
    let item_id = &row.item_ids[0];
    store
        .cos_triage_outbox_claim(item_id, "escalation", "{}", at())
        .expect("outbox");
    for _ in 0..3 {
        let pending = store.notification_pending().expect("pending");
        let result = notify::SendResult {
            ids: vec![pending[0].id],
            outcome: SendOutcome::Failed("http status 500".into()),
        };
        notify::record(&store, &pending, &result, at()).expect("record");
    }
    assert!(store.notification_pending().expect("pending").is_empty());
    let conn = rusqlite::Connection::open(&db).expect("connection");
    let state: String = conn
        .query_row(
            "SELECT state FROM cos_inbox_items WHERE id=?1",
            [item_id],
            |r| r.get(0),
        )
        .expect("item");
    assert_eq!(state, "running");
}

#[test]
fn cos_chat_triage_notice_revision_is_checked_before_post() {
    use task_core::feed::{NoticeEvent, NoticeKind, NoticeQuery, NoticeStore};
    let dir = tempfile::tempdir().expect("dir");
    let db = dir.path().join("test.db");
    let store = SqliteStore::open(&db).expect("db");
    store
        .notice_record(&NoticeEvent {
            source_key: "completion-1".into(),
            kind: NoticeKind::TaskDone,
            group_key: "task_done:project:none".into(),
            title: "done".into(),
            summary: "done".into(),
            project_id: None,
            task_id: None,
            target: None,
            links: Vec::new(),
            at: at(),
        })
        .expect("notice");
    let notice = store
        .notice_list(&NoticeQuery::default())
        .expect("list")
        .items
        .remove(0);
    let source = CosTriageSource {
        source_kind: "notice".into(),
        source_key: notice.id.to_string(),
        source_revision: notice.count.to_string(),
        source_event_id: None,
        operation_id: None,
        summary: notice.title.clone(),
        policy_version: "1".into(),
    };
    store
        .cos_triage_ingest_batch("notices", "cursor", &[source], at())
        .expect("ingest");
    let conn = rusqlite::Connection::open(&db).expect("connection");
    let id: String = conn
        .query_row("SELECT id FROM cos_inbox_items", [], |r| r.get(0))
        .expect("item");
    store
        .cos_triage_outbox_claim(&id, "escalation", "{}", at())
        .expect("outbox");
    let row = store.notification_pending().expect("pending").remove(0);
    let view = task_ops::view::ViewContext {
        workspace_root: dir.path().into(),
        retry_backoff_base: std::time::Duration::from_secs(1),
        retry_backoff_max: std::time::Duration::from_secs(60),
        max_requeues: 3,
        clusters: Default::default(),
    };
    assert!(notify::triage::withdraw_if_resolved(&store, &db, &view, &row, at()).expect("live"));
    store.notice_mark_read(notice.id, at()).expect("read");
    assert!(
        !notify::triage::withdraw_if_resolved(&store, &db, &view, &row, at()).expect("withdraw")
    );
    assert!(store.notification_pending().expect("pending").is_empty());
}
