use super::*;
use std::net::TcpListener;
use std::sync::mpsc;

/// 127.0.0.1 の空きポートに束ねる（**外部ネットワークには出ない**）。
fn listener() -> TcpListener {
    TcpListener::bind("127.0.0.1:0").expect("bind")
}

#[test]
fn provider_kind_proxy_is_reachable_without_a_qwen_model() {
    let server = listener();
    let addr = server.local_addr().expect("addr");
    let (tx, rx) = mpsc::channel::<String>();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = server.accept().expect("accept");
        let mut buf = [0u8; 1024];
        let n = stream.read(&mut buf).unwrap_or(0);
        let _ = tx.send(String::from_utf8_lossy(&buf[..n]).to_string());
        let _ = stream.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 15\r\n\r\n{\"data\": []}\r\n\r\n",
        );
    });
    let base = format!("http://127.0.0.1:{}/v1", addr.port());
    assert_eq!(probe_models(&base, PROBE_TIMEOUT, None), Reachability::Ok);
    let request = rx.recv().expect("request");
    assert!(request.starts_with("GET /v1/models HTTP/1.1"), "{request}");
    assert!(!request.to_ascii_lowercase().contains("qwen"), "{request}");
    handle.join().expect("join");
}

#[test]
fn a_non_2xx_answer_is_unreachable() {
    let server = listener();
    let addr = server.local_addr().expect("addr");
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = server.accept().expect("accept");
        let mut buf = [0u8; 1024];
        let _ = stream.read(&mut buf);
        let _ = stream.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n");
    });
    let base = format!("http://127.0.0.1:{}/v1", addr.port());
    let outcome = probe_models(&base, PROBE_TIMEOUT, None);
    assert_eq!(outcome.should_fall_back(), Some("HTTP 502"), "{outcome:?}");
    handle.join().expect("join");
}

/// ADR-0132 D4: 接続不可（proxy が listen していないポート）。
#[test]
fn a_refused_connection_is_unreachable() {
    let server = listener();
    let port = server.local_addr().expect("addr").port();
    drop(server); // ここで誰も listen していないポートになる。
    let base = format!("http://127.0.0.1:{port}/v1");
    let outcome = probe_models(&base, PROBE_TIMEOUT, None);
    let reason = outcome.should_fall_back().expect("unreachable");
    // Phase 97b: 手放した直後の一時ポートを別のプロセス（並走するテストや GUI の偽 celeris）が
    // 取ることがあり、その場合は「接続できない」ではなく接続後の reset で「応答を読めない」になる。
    // どちらも到達不可（フォールバック対象）なので両方を受け入れる（release ゲートで 1 回起きた）。
    assert!(
        reason.contains("接続できない") || reason.contains("応答を読めない"),
        "{reason}"
    );
}

/// ADR-0052 D1: タイムアウト（受けるだけで何も返さないサーバ）。
#[test]
fn a_server_that_never_answers_times_out() {
    let server = listener();
    let addr = server.local_addr().expect("addr");
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let handle = std::thread::spawn(move || {
        let (stream, _) = server.accept().expect("accept");
        // 何も書かずに、検査が終わるまで開けたままにする。
        let _ = done_rx.recv();
        drop(stream);
    });
    let base = format!("http://127.0.0.1:{}/v1", addr.port());
    let started = Instant::now();
    let outcome = probe_models(&base, Duration::from_millis(300), None);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "制限時間で打ち切る: {:?}",
        started.elapsed()
    );
    let reason = outcome.should_fall_back().expect("unreachable");
    assert!(reason.contains("応答"), "{reason}");
    let _ = done_tx.send(());
    handle.join().expect("join");
}

/// 検査できない書き方は `Unknown`（= 従来どおり `langmem` で走らせる）。
#[test]
fn unprobeable_base_urls_are_unknown() {
    for base in [
        "https://api.example.com/v1",
        "ftp://nope",
        "",
        "http://",
        "http://host:notaport/v1",
    ] {
        assert!(
            matches!(
                probe_models(base, PROBE_TIMEOUT, None),
                Reachability::Unknown { .. }
            ),
            "{base}"
        );
    }
}

/// Phase 65b: `bearer_token` を渡すと `Authorization: Bearer <token>` が送られ、それが無いと
/// 401 を返す上流（celeris の `llm-proxy` の `/v1/models` と同じ挙動）でも到達できる。
#[test]
fn a_bearer_token_is_sent_as_an_authorization_header() {
    let server = listener();
    let addr = server.local_addr().expect("addr");
    let (tx, rx) = mpsc::channel::<String>();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = server.accept().expect("accept");
        let mut buf = [0u8; 1024];
        let n = stream.read(&mut buf).unwrap_or(0);
        let req = String::from_utf8_lossy(&buf[..n]).to_string();
        let _ = tx.send(req.clone());
        if req.contains("Authorization: Bearer secret-proxy-token") {
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        } else {
            let _ = stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n");
        }
    });
    let base = format!("http://127.0.0.1:{}/v1", addr.port());
    assert_eq!(
        probe_models(&base, PROBE_TIMEOUT, Some("secret-proxy-token")),
        Reachability::Ok
    );
    let request = rx.recv().expect("request");
    assert!(
        request.contains("Authorization: Bearer secret-proxy-token"),
        "{request}"
    );
    handle.join().expect("join");
}

#[test]
fn without_a_bearer_token_the_same_upstream_answers_401() {
    let server = listener();
    let addr = server.local_addr().expect("addr");
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = server.accept().expect("accept");
        let mut buf = [0u8; 1024];
        let n = stream.read(&mut buf).unwrap_or(0);
        let req = String::from_utf8_lossy(&buf[..n]).to_string();
        assert!(!req.contains("Authorization"), "{req}");
        let _ = stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n");
    });
    let base = format!("http://127.0.0.1:{}/v1", addr.port());
    let outcome = probe_models(&base, PROBE_TIMEOUT, None);
    assert!(
        matches!(outcome, Reachability::Unknown { .. }),
        "{outcome:?}"
    );
    handle.join().expect("join");
}

/// Phase 65b: 401/403 は「到達性が無い」ではない（＝ 認証エラーで `Unreachable` にしない。
/// フォールバックさせず、従来どおり `langmem` で走らせる）。
#[test]
fn a_401_or_403_from_the_probe_is_unknown_not_unreachable() {
    for status in ["401 Unauthorized", "403 Forbidden"] {
        let server = listener();
        let addr = server.local_addr().expect("addr");
        let response = format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\n\r\n");
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = server.accept().expect("accept");
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(response.as_bytes());
        });
        let base = format!("http://127.0.0.1:{}/v1", addr.port());
        let outcome = probe_models(&base, PROBE_TIMEOUT, None);
        assert!(
            matches!(outcome, Reachability::Unknown { .. }),
            "{status}: {outcome:?}"
        );
        assert!(
            outcome.should_fall_back().is_none(),
            "{status}: フォールバックしない"
        );
        handle.join().expect("join");
    }
}

#[test]
fn urls_split_into_host_port_and_path() {
    assert_eq!(
        split_http_url("http://127.0.0.1:18000/v1").expect("split"),
        ("127.0.0.1".to_string(), 18000, "/v1".to_string())
    );
    assert_eq!(
        split_http_url("http://bnode150/v1/").expect("split"),
        ("bnode150".to_string(), 80, "/v1".to_string())
    );
    assert_eq!(
        split_http_url("http://[::1]:8000").expect("split"),
        ("::1".to_string(), 8000, String::new())
    );
}
