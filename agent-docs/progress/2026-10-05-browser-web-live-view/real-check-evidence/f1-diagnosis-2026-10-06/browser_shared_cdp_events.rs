//! F1: the shared CDP relay delivers browser events that arrive while the agent sends
//! nothing (agent-browser waits for `Page.loadEventFired` after the `Page.navigate` reply).
//! A scripted browser on a socket pair stands in for Chrome; no browser, no network.
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use task_worker::browser_cdp_sink::CdpController;
use task_worker::browser_shared_cdp::SharedCdp;

const TOKEN: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

fn read_command(browser: &mut UnixStream) -> Value {
    let mut bytes = Vec::new();
    loop {
        let mut byte = [0];
        browser.read_exact(&mut byte).expect("CDP command byte");
        if byte[0] == 0 {
            return serde_json::from_slice(&bytes).expect("CDP command");
        }
        bytes.push(byte[0]);
    }
}

fn write_message(browser: &mut UnixStream, value: Value) {
    let mut bytes = serde_json::to_vec(&value).expect("CDP message");
    bytes.push(0);
    browser.write_all(&bytes).expect("CDP write");
}

fn ws_send(stream: &mut UnixStream, value: Value) {
    let payload = serde_json::to_vec(&value).expect("frame");
    assert!(payload.len() < 126);
    let mask = [1u8, 2, 3, 4];
    let mut frame = vec![0x81, 0x80 | payload.len() as u8];
    frame.extend_from_slice(&mask);
    frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    stream.write_all(&frame).expect("ws send");
}

fn ws_recv(stream: &mut UnixStream) -> Value {
    let mut h = [0u8; 2];
    stream
        .read_exact(&mut h)
        .expect("ws frame header (event not delivered?)");
    let mut n = (h[1] & 0x7f) as usize;
    if n == 126 {
        let mut ext = [0u8; 2];
        stream.read_exact(&mut ext).expect("ws len");
        n = u16::from_be_bytes(ext) as usize;
    }
    let mut payload = vec![0u8; n];
    stream.read_exact(&mut payload).expect("ws payload");
    serde_json::from_slice(&payload).expect("ws json")
}

#[test]
fn relay_delivers_events_emitted_after_the_reply() {
    let dir = tempfile::tempdir().expect("dir");
    let (controller_end, mut browser) = UnixStream::pair().expect("pair");
    let read = std::fs::File::from(std::os::fd::OwnedFd::from(
        controller_end.try_clone().expect("clone"),
    ));
    let write = std::fs::File::from(std::os::fd::OwnedFd::from(controller_end));
    let (replied_tx, replied_rx) = mpsc::channel::<()>();
    let script = thread::spawn(move || {
        let attach = read_command(&mut browser);
        assert_eq!(attach["method"], "Target.attachToTarget");
        write_message(
            &mut browser,
            json!({"id":attach["id"],"result":{"sessionId":"S1"}}),
        );
        let navigate = read_command(&mut browser);
        assert_eq!(navigate["method"], "Page.navigate");
        write_message(
            &mut browser,
            json!({"id":navigate["id"],"result":{"frameId":"F1","loaderId":"L1"}}),
        );
        // Only after the agent holds the navigate reply does the page finish loading.
        replied_rx.recv().expect("agent got navigate reply");
        write_message(
            &mut browser,
            json!({"method":"Page.loadEventFired","params":{"timestamp":1.0},"sessionId":"OTHER"}),
        );
        write_message(
            &mut browser,
            json!({"method":"Page.loadEventFired","params":{"timestamp":2.0},"sessionId":"S1"}),
        );
        browser
    });
    let socket = dir.path().join("cdp.sock");
    let _relay = SharedCdp::start(
        CdpController::new(write, read),
        &socket,
        TOKEN.into(),
        vec!["http://127.0.0.1:17730".into()],
    )
    .expect("relay");
    let mut agent = UnixStream::connect(&socket).expect("connect");
    agent
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    write!(agent, "GET /{TOKEN} HTTP/1.1\r\nHost: 127.0.0.1:9223\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n").expect("upgrade");
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut b = [0];
        agent.read_exact(&mut b).expect("upgrade reply");
        head.push(b[0]);
    }
    assert!(head.starts_with(b"HTTP/1.1 101"));
    ws_send(
        &mut agent,
        json!({"id":1,"method":"Target.attachToTarget","params":{"targetId":"T","flatten":true}}),
    );
    assert_eq!(ws_recv(&mut agent)["result"]["sessionId"], "S1");
    ws_send(
        &mut agent,
        json!({"id":2,"method":"Page.navigate","params":{"url":"http://127.0.0.1:17730/"},"sessionId":"S1"}),
    );
    assert_eq!(ws_recv(&mut agent)["id"], 2);
    replied_tx.send(()).expect("signal");
    // No further command: the load event must still arrive, and only for our session.
    let event = ws_recv(&mut agent);
    assert_eq!(event["method"], "Page.loadEventFired");
    assert_eq!(event["sessionId"], "S1");
    drop(script.join().expect("script"));
}
