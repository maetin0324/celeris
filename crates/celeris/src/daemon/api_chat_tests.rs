//! ADR 2026-10-05-cos-chat-home D4/D1: daemon の chat 配線（添付の data dir と上限、起動時の legacy 移行）。

use std::path::Path;

use task_core::{InstanceRole, SqliteStore};

use super::*;

const TOKEN: &str = "chat-wiring-token";

fn load_config(dir: &Path, extra: &str) -> Config {
    let path = dir.join("config.toml");
    let db = dir.join("state").join("celeris.db");
    std::fs::create_dir_all(db.parent().expect("db dir")).expect("mkdir");
    let ws = dir.join("ws");
    std::fs::write(
        &path,
        format!(
            "db = {db:?}\nworkspace_root = {ws:?}\n{extra}\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n"
        ),
    )
    .expect("write config");
    Config::load(&path).expect("config")
}

fn multipart(name: &str, key: &str, file: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(format!("--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
    body.extend_from_slice(file);
    body.extend_from_slice(format!("\r\n--boundary\r\nContent-Disposition: form-data; name=\"client_upload_id\"\r\n\r\n{key}\r\n--boundary--\r\n").as_bytes());
    body
}

#[tokio::test]
async fn chat_wiring_passes_data_dir_and_limits_to_api() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = load_config(
        dir.path(),
        "[cos.attachments]\nmax_file_bytes = 8\nmax_message_bytes = 8\nmax_storage_bytes = 64\n",
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let settings = api_settings(
        &cfg,
        addr,
        Some(TOKEN.into()),
        "i".into(),
        "t".into(),
        None,
        "sha12sha12ab".into(),
        DaemonMode::Normal,
        SharedRole::new(InstanceRole::Active),
        None,
    );
    let (_tx, rx) = tokio::sync::watch::channel(None);
    let state = tokio::task::spawn_blocking(move || ApiState::new(settings, rx))
        .await
        .expect("join")
        .expect("state");
    let state = with_chat_wiring(state, &cfg);
    let (stop, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(task_api::serve_with_listener(listener, state, async move {
        let _ = stop_rx.await;
    }));
    let client = reqwest::Client::new();
    let base = format!("http://{addr}/api/v1/chat");
    let created = client
        .post(format!("{base}/threads"))
        .bearer_auth(TOKEN)
        .header("content-type", "application/json")
        .body(r#"{"title":"Wiring","project_id":null,"client_thread_id":"wiring"}"#)
        .send()
        .await
        .expect("create thread");
    assert_eq!(created.status().as_u16(), 201);
    let thread: serde_json::Value = created.json().await.expect("thread json");
    let thread_id = thread["thread"]["id"]
        .as_str()
        .expect("thread id")
        .to_owned();
    let upload = |key: &'static str, bytes: &'static [u8]| {
        client
            .post(format!("{base}/threads/{thread_id}/attachments"))
            .bearer_auth(TOKEN)
            .header("content-type", "multipart/form-data; boundary=boundary")
            .body(multipart("a.bin", key, bytes))
            .send()
    };
    let ok = upload("small", b"1234").await.expect("upload");
    assert_eq!(ok.status().as_u16(), 201);
    let body: serde_json::Value = ok.json().await.expect("upload json");
    let id = body["id"]
        .as_str()
        .or_else(|| body["attachment"]["id"].as_str())
        .expect("attachment id")
        .to_owned();
    // The blob lands under `<data_dir>/chat/attachments` where data_dir is the DB's directory.
    let blob = dir
        .path()
        .join("state/chat/attachments")
        .join(&id)
        .join("blob");
    assert_eq!(std::fs::read(&blob).expect("blob"), b"1234");
    // `[cos.attachments] max_file_bytes` is the limit the API enforces.
    let too_big = upload("big", b"123456789").await.expect("upload big");
    assert_eq!(too_big.status().as_u16(), 413);
    let _ = stop.send(());
    server.await.expect("join").expect("serve");
}

#[test]
fn chat_wiring_daemon_store_open_runs_legacy_migration() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = load_config(dir.path(), "");
    {
        // A migrated DB with an old Console CoS row that has not been projected yet.
        let store = SqliteStore::open(&cfg.db.path).expect("store");
        drop(store);
        let conn = rusqlite::Connection::open(&cfg.db.path).expect("conn");
        conn.execute(
            "INSERT INTO messages(id,node_id,project_id,role,text,created_at) \
             VALUES('old-cos-1','cos',NULL,'user','旧 Console の発言','2026-10-04T00:00:00Z')",
            [],
        )
        .expect("old message");
    }
    // The daemon start-up path opens the store (and so runs the legacy projection).
    let _dispatcher = crate::build_dispatcher(&cfg, Default::default()).expect("dispatcher");
    let conn = rusqlite::Connection::open(&cfg.db.path).expect("conn");
    let (kind, text): (String, String) = conn
        .query_row(
            "SELECT t.kind, m.text FROM chat_messages m JOIN chat_threads t ON t.id=m.thread_id \
             WHERE m.legacy_message_id='old-cos-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("legacy message");
    assert_eq!(kind, "legacy");
    assert_eq!(text, "旧 Console の発言");
}
