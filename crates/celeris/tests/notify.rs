//! ADR-0037: Discord への送信の仕組み（判定は CoS の outbox。旧 `scan` / `schedule` の試験は経路と一緒に消した）。
//!
//! 見るもの:
//! - 送信は**偽の HTTP サーバ**（`tokio::net::TcpListener` で 1 リクエスト受けて応答を返す）へ。
//!   外部ネットワークには出ない（CLAUDE.md の禁止事項）。
//! - 3 回失敗したら諦める。429 は attempts に数えない。
//! - 失敗の文面に URL・ホスト名が出ない。

#[cfg(test)]
#[allow(clippy::duplicate_mod)] // 同じ支援 file を試験 crate ごとに読む
#[path = "../../../tests/support/http_async.rs"]
mod fake_http;
#[cfg(test)]
#[allow(clippy::duplicate_mod)] // 同じ支援 file を試験 crate ごとに読む
#[path = "../../../tests/support/http_request.rs"]
mod http_framing;

use std::path::Path;
use std::time::Duration;

use celeris::notify::{self, SendResult};

use task_core::SqliteStore;
use task_core::notify::{MAX_NOTIFY_ATTEMPTS, NotificationKind};
use time::OffsetDateTime;

fn at(secs: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_800_000_000 + secs).unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

struct Env {
    _dir: tempfile::TempDir,
    store: SqliteStore,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let store = SqliteStore::open(&dir.path().join("celeris.db"))
            .unwrap_or_else(|e| panic!("open: {e}"));
        Self { _dir: dir, store }
    }

    fn as_store(&self) -> &SqliteStore {
        &self.store
    }
}

// ---- 送信（偽の HTTP サーバ。外部ネットワークには出ない）----

/// `127.0.0.1:0` で待ち受け、`POST /hook` を `max` 件受けて `response` をそのまま返す。返るのは URL と、
/// 受け取った本文を集めるハンドル。
///
/// 接続は 1 本ずつ別の task で読み、受けた接続は必ず応答してから閉じる。`fake_http::accept` は最初に
/// 読み終えた 1 本を返して残りの接続を捨てるので、迷い込み（同じ port を一度取って捨てた別の試験の
/// 送信など）が先に終わると本物の送信が応答なしに切られ、送信側は "invalid request" になった
/// （2026-11-02 の flaky。`the_fake_webhook_answers_a_connection_even_if_another_request_finishes_first`）。
/// 読みに時間切れも置かない（負荷で送信が遅れても 404 で切らない）。
async fn fake_webhook_raw(
    max: usize,
    response: &str,
) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|e| panic!("bind: {e}"));
    let addr = listener
        .local_addr()
        .unwrap_or_else(|e| panic!("addr: {e}"));
    let response: std::sync::Arc<str> = response.into();
    let handle = tokio::spawn(async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let mut bodies = Vec::new();
        while bodies.len() < max {
            tokio::select! {
                Some(body) = rx.recv() => bodies.push(body),
                connection = listener.accept() => {
                    let (socket, _) = connection.unwrap_or_else(|e| panic!("accept: {e}"));
                    tokio::spawn(answer_hook(socket, response.clone(), tx.clone()));
                }
            }
        }
        bodies
    });
    (format!("http://{addr}/hook"), handle)
}

/// 1 本の接続から要求を読み切り、`POST /hook` なら本文を `tx` に送って `response` を、違えば 404 を返す。
async fn answer_hook(
    mut socket: tokio::net::TcpStream,
    response: std::sync::Arc<str>,
    tx: tokio::sync::mpsc::UnboundedSender<String>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut bytes = Vec::new();
    let mut buf = [0u8; 8192];
    let request = loop {
        match socket.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => bytes.extend_from_slice(&buf[..n]),
        }
        match http_framing::complete(&bytes) {
            Ok(Some(_)) => break String::from_utf8_lossy(&bytes).into_owned(),
            Ok(None) => {}
            Err(()) => break String::new(),
        }
    };
    if fake_http::matches(&request, "POST", "/hook") {
        let _ = socket.write_all(response.as_bytes()).await;
        let _ = tx.send(request);
    } else {
        let _ = socket.write_all(http_framing::NOT_FOUND).await;
    }
    let _ = socket.flush().await;
    let _ = socket.shutdown().await;
}

/// `max` 件を受けて、毎回 `status` だけの空応答を返す（`connection: close`）。
async fn fake_webhook(max: usize, status: u16) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    fake_webhook_raw(
        max,
        &format!("HTTP/1.1 {status} X\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"),
    )
    .await
}

/// 待ち受け socket の accept 待ちの列（`/proc/net/tcp` の LISTEN 行の rx_queue）が空になるまで待つ。
/// 偽サーバが接続を accept し終えたことを、時間でなく出来事で確かめる。
async fn wait_until_accepted(port: u16) {
    let local = format!("0100007F:{port:04X}");
    for _ in 0..1000 {
        let table = std::fs::read_to_string("/proc/net/tcp").unwrap_or_default();
        let backlog = table.lines().skip(1).find_map(|line| {
            let cols: Vec<&str> = line.split_whitespace().collect();
            (cols.get(1) == Some(&local.as_str()) && cols.get(3) == Some(&"0A"))
                .then(|| {
                    cols.get(4)
                        .and_then(|q| q.split(':').nth(1))
                        .map(str::to_owned)
                })
                .flatten()
        });
        if backlog.as_deref() == Some("00000000") {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the fake server did not accept the connection");
}

/// 2026-11-02 の flaky（`a_failing_webhook_is_retried_three_times_and_then_given_up` が 1 回だけ
/// "invalid request"）の再現: 偽サーバが受けた接続のうち、別の接続が先に要求を読み終えても、
/// 受けた接続を応答なしに捨てない。迷い込み（A）が先に終わる順序を接続の順序で決める。
#[tokio::test]
async fn the_fake_webhook_answers_a_connection_even_if_another_request_finishes_first() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (url, _server) = fake_webhook(4, 500).await;
    let authority = url
        .strip_prefix("http://")
        .and_then(|rest| rest.split('/').next())
        .unwrap_or_else(|| panic!("authority"))
        .to_string();
    let port: u16 = authority
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or_else(|| panic!("port"));
    let request = "POST /hook HTTP/1.1\r\nhost: x\r\ncontent-length: 2\r\n\r\n{}";
    let (head, last) = request.split_at(request.len() - 1);
    // B（本物の送信の代わり）: 接続して最後の 1 byte を残して送り、偽サーバが accept するまで待つ。
    let mut b = tokio::net::TcpStream::connect(&authority)
        .await
        .unwrap_or_else(|e| panic!("connect b: {e}"));
    b.write_all(head.as_bytes())
        .await
        .unwrap_or_else(|e| panic!("write b: {e}"));
    wait_until_accepted(port).await;
    // A（迷い込み）: 要求を全部送り、応答を読み終える。
    let mut a = tokio::net::TcpStream::connect(&authority)
        .await
        .unwrap_or_else(|e| panic!("connect a: {e}"));
    a.write_all(request.as_bytes())
        .await
        .unwrap_or_else(|e| panic!("write a: {e}"));
    let mut reply_a = Vec::new();
    let _ = a.read_to_end(&mut reply_a).await;
    assert!(reply_a.starts_with(b"HTTP/1.1 500"), "{reply_a:?}");
    // B の残りを送る。B にも応答が返る。
    let _ = b.write_all(last.as_bytes()).await;
    let mut reply_b = Vec::new();
    let read = b.read_to_end(&mut reply_b).await;
    assert!(
        reply_b.starts_with(b"HTTP/1.1 500"),
        "B got no response: {read:?} {reply_b:?}"
    );
}

#[tokio::test]
async fn a_pending_notification_is_posted_once_and_marked_sent() {
    let env = Env::new();
    let row = env
        .store
        .notification_upsert_pending(
            NotificationKind::CosEscalation,
            "r1",
            "悪い知らせ: テスト",
            None,
            at(0),
        )
        .unwrap_or_else(|e| panic!("upsert: {e}"))
        .unwrap_or_else(|| panic!("row"));
    let (url, server) = fake_webhook(1, 204).await;
    let client = notify::client().unwrap_or_else(|| panic!("client"));

    let batch = notify::select_routes_batch(std::slice::from_ref(&row))
        .unwrap_or_else(|| panic!("no batch"));
    let (tx, mut rx) = tokio::sync::mpsc::channel::<SendResult>(4);
    notify::spawn_send(client, url, &batch, tx);
    let result = rx.recv().await.unwrap_or_else(|| panic!("no result"));
    assert_eq!(result.outcome, notify::SendOutcome::Sent, "{result:?}");
    assert_eq!(result.ids, vec![row.id]);

    let bodies = server.await.unwrap_or_else(|e| panic!("server: {e}"));
    assert_eq!(bodies.len(), 1);
    assert!(bodies[0].starts_with("POST /hook "), "{}", bodies[0]);
    assert!(bodies[0].contains("悪い知らせ"), "{}", bodies[0]);
    assert!(
        bodies[0].contains("\"username\":\"Celeris\""),
        "{}",
        bodies[0]
    );

    // 次の tick で台帳に書かれる。
    let pending = env
        .store
        .notification_pending()
        .unwrap_or_else(|e| panic!("pending: {e}"));
    notify::record(env.as_store(), &pending, &result, at(1))
        .unwrap_or_else(|e| panic!("record: {e}"));
    assert!(
        env.store
            .notification_pending()
            .unwrap_or_default()
            .is_empty()
    );
    let recent = env
        .store
        .notification_recent(5)
        .unwrap_or_else(|e| panic!("recent: {e}"));
    assert_eq!(recent[0].ok, Some(true));
    assert_eq!(recent[0].sent_at, Some(at(1)));
}

#[tokio::test]
async fn a_failing_webhook_is_retried_three_times_and_then_given_up() {
    let env = Env::new();
    let row = env
        .store
        .notification_upsert_pending(NotificationKind::CosEscalation, "r1", "b", None, at(0))
        .unwrap_or_else(|e| panic!("upsert: {e}"))
        .unwrap_or_else(|| panic!("row"));
    // 500 を返す偽サーバ（4 回分受けられるが、諦めるので 3 回しか来ない）。
    let (url, server) = fake_webhook(4, 500).await;
    let client = notify::client().unwrap_or_else(|| panic!("client"));

    let mut attempts = 0;
    for tick in 0..5 {
        let pending = env
            .store
            .notification_pending()
            .unwrap_or_else(|e| panic!("pending: {e}"));
        if pending.is_empty() {
            break;
        }
        let batch = notify::select_routes_batch(&pending).unwrap_or_else(|| panic!("no batch"));
        attempts += 1;
        let (tx, mut rx) = tokio::sync::mpsc::channel::<SendResult>(4);
        notify::spawn_send(client.clone(), url.clone(), &batch, tx);
        let result = rx.recv().await.unwrap_or_else(|| panic!("no result"));
        let error = match &result.outcome {
            notify::SendOutcome::Failed(e) => e.clone(),
            other => panic!("expected Failed, got {other:?}"),
        };
        assert_eq!(error, "http status 500");
        // 失敗の文面に URL・ホスト名は出ない（ADR-0037 D3）。
        assert!(!error.contains("127.0.0.1"), "{error}");
        assert!(!error.contains("http://"), "{error}");
        notify::record(env.as_store(), &pending, &result, at(tick))
            .unwrap_or_else(|e| panic!("record: {e}"));
    }
    assert_eq!(attempts, MAX_NOTIFY_ATTEMPTS as usize, "3 回で諦める");

    let recent = env
        .store
        .notification_recent(5)
        .unwrap_or_else(|e| panic!("recent: {e}"));
    let found = recent
        .iter()
        .find(|n| n.id == row.id)
        .unwrap_or_else(|| panic!("row"));
    assert_eq!(found.ok, Some(false));
    assert_eq!(found.attempts, MAX_NOTIFY_ATTEMPTS);
    assert!(found.sent_at.is_none());
    assert!(
        found.error.as_deref().unwrap_or("").contains("gave up"),
        "{:?}",
        found.error
    );
    drop(server);
}

#[tokio::test]
async fn an_unreachable_webhook_never_reveals_the_host() {
    // 誰も待っていないポート（接続できない）。偽サーバを立ててすぐ落とす。
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|e| panic!("bind: {e}"));
    let addr = listener
        .local_addr()
        .unwrap_or_else(|e| panic!("addr: {e}"));
    drop(listener);
    let client = notify::client().unwrap_or_else(|| panic!("client"));
    let outcome = notify::post_webhook(&client, &format!("http://{addr}/hook"), "x").await;
    let error = match outcome {
        notify::SendOutcome::Failed(e) => e,
        other => panic!("expected Failed, got {other:?}"),
    };
    assert!(!error.contains("127.0.0.1"), "{error}");
    assert!(!error.contains(&addr.port().to_string()), "{error}");
    assert!(!error.contains("hook"), "{error}");
}

// ---- 送信の間隔（ADR-0037 D5。実機 2026-09-18: 8 件一斉送信で 429 が 3 件）----

#[tokio::test]
async fn a_rate_limited_response_is_not_counted_as_an_attempt() {
    let env = Env::new();
    let row = env
        .store
        .notification_upsert_pending(
            NotificationKind::CosEscalation,
            "r1",
            "悪い知らせ: b",
            None,
            at(0),
        )
        .unwrap_or_else(|e| panic!("upsert: {e}"))
        .unwrap_or_else(|| panic!("row"));
    // Discord 風の 429（`retry-after` ヘッダに秒数）。
    let (url, server) = fake_webhook_raw(
        1,
        "HTTP/1.1 429 X\r\nretry-after: 2\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
    )
    .await;
    let client = notify::client().unwrap_or_else(|| panic!("client"));
    let batch = notify::select_routes_batch(std::slice::from_ref(&row))
        .unwrap_or_else(|| panic!("no batch"));
    let (tx, mut rx) = tokio::sync::mpsc::channel::<SendResult>(4);
    notify::spawn_send(client, url, &batch, tx);
    let result = rx.recv().await.unwrap_or_else(|| panic!("no result"));
    match &result.outcome {
        notify::SendOutcome::RateLimited(wait) => {
            assert_eq!(*wait, Duration::from_secs(2), "{wait:?}")
        }
        other => panic!("expected RateLimited, got {other:?}"),
    }
    server.await.unwrap_or_else(|e| panic!("server: {e}"));

    // 429 は台帳に触れない: attempts は増えず、まだ pending のまま。
    let pending_before = env
        .store
        .notification_pending()
        .unwrap_or_else(|e| panic!("pending: {e}"));
    notify::record(env.as_store(), &pending_before, &result, at(1))
        .unwrap_or_else(|e| panic!("record: {e}"));
    let pending_after = env
        .store
        .notification_pending()
        .unwrap_or_else(|e| panic!("pending: {e}"));
    assert_eq!(pending_after.len(), 1);
    assert_eq!(pending_after[0].attempts, 0, "429 は attempts に数えない");
    assert!(pending_after[0].ok.is_none());
}

// ---- 秘密が無い間は送らない（ADR-0037 D2）----

#[test]
fn the_webhook_secret_is_read_from_the_secrets_dir() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    std::fs::write(
        dir.path().join("discord-webhook"),
        "https://example.invalid/webhooks/1/abc\n",
    )
    .unwrap_or_else(|e| panic!("write: {e}"));
    assert_eq!(
        notify::webhook_url(Some(dir.path() as &Path), "discord-webhook").as_deref(),
        Some("https://example.invalid/webhooks/1/abc")
    );
    // 別の id を指せば見つからない。
    assert_eq!(notify::webhook_url(Some(dir.path()), "other"), None);
}

// ---- 実機相当: `POST /notify/test` の中身を偽サーバ（`[secrets]` に入れた URL）へ 1 回通す ----

#[tokio::test]
async fn send_test_posts_one_message_to_the_url_in_the_secrets_dir() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let (url, server) = fake_webhook(1, 204).await;
    std::fs::write(dir.path().join("discord-webhook"), format!("{url}\n"))
        .unwrap_or_else(|e| panic!("write: {e}"));
    let client = notify::client();

    let result = notify::send_test(client.as_ref(), Some(dir.path()), "discord-webhook").await;
    assert_eq!(result, notify::TestSend::Sent, "{result:?}");
    let bodies = server.await.unwrap_or_else(|e| panic!("server: {e}"));
    assert_eq!(bodies.len(), 1);
    assert!(bodies[0].starts_with("POST /hook "), "{}", bodies[0]);
    assert!(bodies[0].contains("celeris"), "{}", bodies[0]);

    // 秘密が無ければ送らない（API はこれを 409 `notify_unavailable` にする）。
    let missing = notify::send_test(client.as_ref(), Some(dir.path()), "nope").await;
    match missing {
        notify::TestSend::NotConfigured(detail) => {
            assert!(detail.contains("nope"), "{detail}");
            assert!(!detail.contains("127.0.0.1"), "{detail}");
        }
        other => panic!("expected NotConfigured, got {other:?}"),
    }
}
