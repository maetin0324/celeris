//! `daemon_port::wait_api_ready` の試験（routing_eval_run は PATH を書き換えるので別の test binary に置く）。
//!
//! 本物の daemon は起動しない。JoinHandle と loopback の listener で、daemon の終了・答えない接続・listener 無しの
//! 3 つの場合を作る。CPU 負荷はかけない。

use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

mod daemon_port;

/// 終わった JoinHandle は上限（300 秒）を待たずにエラーになり、run の結果が文面に出る。
#[tokio::test]
async fn finished_daemon_fails_without_waiting_for_the_limit() {
    // listen しない予約 port: 接続は拒否され続ける。
    let reservation = daemon_port::PortReservation::new();
    let url = format!("http://{}/api/v1/ping", reservation.addr());
    let client = reqwest::Client::new();

    let mut exited = tokio::spawn(async { Err::<(), String>("config load failed: boom".into()) });
    let started = Instant::now();
    let err = daemon_port::wait_api_ready(
        &client,
        &url,
        "t",
        &mut exited,
        Duration::from_secs(5),
        Duration::from_secs(300),
    )
    .await
    .expect_err("an exited daemon is not ready");
    assert!(started.elapsed() < Duration::from_secs(60), "{err}");
    assert!(err.contains("daemon exited"), "{err}");
    assert!(err.contains("config load failed: boom"), "{err}");

    let mut panicked = tokio::spawn(async {
        if true {
            panic!("daemon start panicked: bind");
        }
    });
    let err = daemon_port::wait_api_ready(
        &client,
        &url,
        "t",
        &mut panicked,
        Duration::from_secs(5),
        Duration::from_secs(300),
    )
    .await
    .expect_err("a panicked daemon is not ready");
    assert!(
        err.contains("run panicked: daemon start panicked: bind"),
        "{err}"
    );
}

/// 最初の接続は受け付けても答えない。要求の timeout（200ms）の後の接続は答えるので成功する。
#[tokio::test]
async fn silent_connection_does_not_use_up_the_limit() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        let (silent, _) = listener.accept().await.expect("accept silent");
        let mut held = vec![silent];
        loop {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf).await;
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                .await;
            held.push(stream);
        }
    });
    let client = reqwest::Client::new();
    let mut daemon = tokio::spawn(std::future::pending::<Result<(), String>>());
    let result = daemon_port::wait_api_ready(
        &client,
        &format!("http://{addr}/api/v1/ping"),
        "t",
        &mut daemon,
        Duration::from_millis(200),
        Duration::from_secs(60),
    )
    .await;
    server.abort();
    daemon.abort();
    result.expect("the answering connection after the silent one is ready");
}

/// listener が無ければ上限でエラーになり、文面に経過秒と最後の接続エラーがある。
#[tokio::test]
async fn no_listener_fails_at_the_limit_with_elapsed_time() {
    let reservation = daemon_port::PortReservation::new();
    let client = reqwest::Client::new();
    let mut daemon = tokio::spawn(std::future::pending::<Result<(), String>>());
    let started = Instant::now();
    let err = daemon_port::wait_api_ready(
        &client,
        &format!("http://{}/api/v1/ping", reservation.addr()),
        "t",
        &mut daemon,
        Duration::from_millis(200),
        Duration::from_secs(1),
    )
    .await
    .expect_err("nobody listens");
    daemon.abort();
    assert!(started.elapsed() >= Duration::from_secs(1), "{err}");
    assert!(err.contains("not ready after"), "{err}");
    assert!(err.contains("limit 1s"), "{err}");
    assert!(err.contains("last error: "), "{err}");
    assert!(!err.contains("no request finished yet"), "{err}");
}
