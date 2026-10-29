//! Shared fixture reader regression: probes must not consume a one-shot reply.
#[allow(clippy::duplicate_mod)] // 同じ支援 file を試験 crate ごとに読む
#[path = "../../../tests/support/http_async.rs"]
mod asynchronous;
#[allow(clippy::duplicate_mod)] // 同じ支援 file を試験 crate ごとに読む
#[path = "../../../tests/support/http_request.rs"]
mod framing;
#[allow(clippy::duplicate_mod)] // 同じ支援 file を試験 crate ごとに読む
#[path = "../../../tests/support/http_sync.rs"]
mod synchronous;

const REQUEST: &str = "POST /decisions HTTP/1.1\r\nContent-Length: 6\r\n\r\néabcd";

#[test]
fn framing_requires_every_header_and_body_byte() {
    for end in 0..REQUEST.len() {
        assert_eq!(framing::complete(&REQUEST.as_bytes()[..end]), Ok(None));
    }
    assert_eq!(
        framing::complete(REQUEST.as_bytes()),
        Ok(Some(REQUEST.len()))
    );
    for bad in [
        "POST /decisions HTTP/1.1\r\nContent-Length: invalid\r\n\r\n",
        "POST /decisions HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nxx",
        "POST /decisions HTTP/1.1\r\nContent-Length: 999999999\r\n\r\n",
        "POST /decisions HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n",
        "not HTTP\r\n\r\n",
    ] {
        assert_eq!(framing::complete(bad.as_bytes()), Err(()));
    }
}

#[tokio::test]
async fn async_one_shot_survives_noise_and_silent_connection() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, raw) = asynchronous::accept(&listener, "POST", "/decisions").await;
        socket.write_all(b"ok").await.unwrap();
        raw
    });
    let _silent = tokio::net::TcpStream::connect(addr).await.unwrap();
    asynchronous::inject_noise(&format!("http://{addr}/decisions")).await;
    let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
    for part in REQUEST.as_bytes().chunks(3) {
        client.write_all(part).await.unwrap();
    }
    let mut reply = Vec::new();
    client.read_to_end(&mut reply).await.unwrap();
    assert_eq!(reply, b"ok");
    assert_eq!(server.await.unwrap(), REQUEST);
}

#[test]
fn sync_one_shot_survives_noise_and_silent_connection() {
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpListener, TcpStream};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut socket, raw) = synchronous::accept(&listener, "POST", "/decisions");
        socket.write_all(b"ok").unwrap();
        raw
    });
    let _silent = TcpStream::connect(addr).unwrap();
    for raw in [
        "",
        "GET / HTTP/1.1\r\n\r\n",
        "POST /decisions HTTP/1.1\r\nContent-Length: 8\r\n\r\nx",
        "POST /decisions HTTP/1.1\r\nContent-Len",
    ] {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        stream.write_all(raw.as_bytes()).unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
        let mut reply = Vec::new();
        stream.read_to_end(&mut reply).unwrap();
        assert!(reply.starts_with(b"HTTP/1.1 404"));
    }
    let mut client = TcpStream::connect(addr).unwrap();
    client
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    for part in REQUEST.as_bytes().chunks(3) {
        client.write_all(part).unwrap();
    }
    let mut reply = Vec::new();
    client.read_to_end(&mut reply).unwrap();
    assert_eq!(reply, b"ok");
    assert_eq!(server.join().unwrap(), REQUEST);
}
