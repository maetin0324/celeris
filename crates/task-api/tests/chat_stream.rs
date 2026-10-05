//! D2 chat SSE and run events against a temporary SQLite database and a fake run driven through
//! the store (cos-run's role). Time is paused: the poll/heartbeat timers advance only when the
//! runtime is idle, and every wait is an event wait with a long safety bound (ADR-0125).

mod common;

use std::time::Duration;

use common::*;
use serde_json::{Value, json};
use task_core::chat::{ChatRunState, ChatToolData, ChatToolState};
use time::OffsetDateTime;

const BASE: &str = "/api/v1/chat/threads";
const WAIT: Duration = Duration::from_secs(120);

async fn thread_with_message(app: &axum::Router, key: &str) -> String {
    let body = json!({"title":"Stream","project_id":null,"client_thread_id":key});
    let created = send(app, post_admin(BASE, &body)).await;
    assert_eq!(created.status.as_u16(), 201, "{}", created.text());
    let id = created.json()["thread"]["id"]
        .as_str()
        .expect("thread id")
        .to_string();
    let message = json!({"client_message_id":"m1","text":"hello","attachment_ids":[],
        "reply_to_id":null,"mode":"queue","resume_queue":false});
    let posted = send(app, post_admin(&format!("{BASE}/{id}/messages"), &message)).await;
    assert_eq!(posted.status.as_u16(), 202, "{}", posted.text());
    id
}

async fn snapshot_event_id(app: &axum::Router, thread: &str) -> u64 {
    let list = send(app, get_admin(&format!("{BASE}/{thread}/messages")))
        .await
        .json();
    list["snapshot_event_id"]
        .as_str()
        .expect("snapshot_event_id")
        .parse()
        .expect("numeric snapshot id")
}

fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

fn claim(env: &TestEnv, thread: &str, run: &str) {
    env.store
        .chat_run_claim_next(thread, run, &json!({"harness":"claude-code"}), now())
        .expect("claim")
        .expect("claimed run");
}

fn tool(state: ChatToolState) -> ChatToolData {
    ChatToolData {
        call_id: "call-1".to_string(),
        name: "bash".to_string(),
        state,
        summary: "ls".to_string(),
        detail: None,
        error: false,
        truncated: false,
    }
}

/// Runs the fake run to completion: two text deltas, a tool call and the terminal run event.
fn finish_fake_run(env: &TestEnv, run: &str) {
    env.store
        .chat_run_append_text(run, "Hel", now())
        .expect("delta 1");
    env.store
        .chat_run_tool(run, &tool(ChatToolState::Running), now())
        .expect("tool running");
    env.store
        .chat_run_tool(run, &tool(ChatToolState::Completed), now())
        .expect("tool done");
    env.store
        .chat_run_append_text(run, "lo", now())
        .expect("delta 2");
    env.store
        .chat_run_finish(run, ChatRunState::Completed, None, None, now())
        .expect("finish");
}

/// Every event id of `run` after `after`, read through the run events page (not the SSE).
async fn stored_ids(app: &axum::Router, thread: &str, run: &str, after: u64) -> Vec<u64> {
    let page = send(
        app,
        get_admin(&format!(
            "{BASE}/{thread}/runs/{run}/events?after={after}&limit=500"
        )),
    )
    .await;
    assert_eq!(page.status.as_u16(), 200, "{}", page.text());
    page.json()["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|e| e["id"].as_str().expect("id").parse().expect("numeric"))
        .collect()
}

fn is_terminal_run(frame: &Frame) -> bool {
    frame.event == "run"
        && matches!(
            frame.data["data"]["run"]["state"].as_str(),
            Some("completed" | "stopped" | "failed" | "interrupted")
        )
}

/// Reads event frames (skipping heartbeats) until `stop` holds for one of them.
async fn collect_until(sse: &mut Sse, stop: impl Fn(&Frame) -> bool) -> Vec<Frame> {
    let mut frames = Vec::new();
    loop {
        let frame = sse
            .next_frame(WAIT)
            .await
            .expect("frame before the safety bound");
        if frame.event.is_empty() {
            continue;
        }
        let done = stop(&frame);
        frames.push(frame);
        if done {
            return frames;
        }
    }
}

fn ids(frames: &[Frame]) -> Vec<u64> {
    frames.iter().map(|f| f.id.expect("event id")).collect()
}

fn assert_envelope(frame: &Frame, thread: &str) {
    let id = frame.id.expect("id line");
    assert_eq!(frame.data["id"], id.to_string());
    assert_eq!(frame.data["type"], frame.event);
    assert_eq!(frame.data["thread_id"], thread);
    assert!(frame.data["at"].is_string());
    assert!(frame.data.get("run_id").is_some() && frame.data.get("message_id").is_some());
}

fn stream_get(
    thread: &str,
    query: &str,
    last_event_id: Option<&str>,
) -> axum::http::Request<axum::body::Body> {
    let path = format!("{BASE}/{thread}/stream{query}");
    match last_event_id {
        Some(id) => get_with(&path, &[admin_headers()[0], ("last-event-id", id)]),
        None => get_admin(&path),
    }
}

#[tokio::test(start_paused = true)]
async fn chat_stream_snapshot_then_live_has_no_gap() {
    let env = admin_env();
    let app = env.router();
    let thread = thread_with_message(&app, "snap").await;
    let snapshot = snapshot_event_id(&app, &thread).await;
    // Written after the snapshot but before connecting: must come from the replay.
    claim(&env, &thread, "run-1");
    let mut sse = open_stream(
        &app,
        stream_get(&thread, &format!("?after={snapshot}"), None),
    )
    .await;
    assert_eq!(sse.status, 200);
    assert_eq!(
        sse.headers
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("text/event-stream; charset=utf-8")
    );
    // The replay reaches the claim's running `run` event first...
    let mut frames = collect_until(&mut sse, |f| f.event == "run").await;
    // ...then everything written after that must come live.
    finish_fake_run(&env, "run-1");
    frames.extend(collect_until(&mut sse, is_terminal_run).await);
    for frame in &frames {
        assert_envelope(frame, &thread);
    }
    let got = ids(&frames);
    assert!(got.windows(2).all(|w| w[0] < w[1]), "ascending: {got:?}");
    assert!(got[0] > snapshot);
    // Every run event after the snapshot was delivered, and nothing else from the run.
    let expected = stored_ids(&app, &thread, "run-1", snapshot).await;
    let delivered_run: Vec<u64> = frames
        .iter()
        .filter(|f| f.data["run_id"] == "run-1")
        .map(|f| f.id.expect("id"))
        .collect();
    assert_eq!(delivered_run, expected);
    // The fake run's order: deltas, tool running/completed, delta, terminal run.
    let order: Vec<&str> = frames
        .iter()
        .filter(|f| matches!(f.event.as_str(), "text_delta" | "tool"))
        .map(|f| f.event.as_str())
        .collect();
    assert_eq!(order, ["text_delta", "tool", "tool", "text_delta"]);
    let deltas: Vec<(u64, &str)> = frames
        .iter()
        .filter(|f| f.event == "text_delta")
        .map(|f| {
            (
                f.data["data"]["offset"].as_u64().expect("offset"),
                f.data["data"]["text"].as_str().expect("text"),
            )
        })
        .collect();
    assert_eq!(deltas, [(0, "Hel"), (3, "lo")]);
    assert_eq!(frames.last().map(|f| f.event.as_str()), Some("run"));
}

#[tokio::test(start_paused = true)]
async fn chat_stream_last_event_id_reconnect_has_no_duplicate() {
    let env = admin_env();
    let app = env.router();
    let thread = thread_with_message(&app, "reconnect").await;
    let snapshot = snapshot_event_id(&app, &thread).await;
    claim(&env, &thread, "run-1");
    env.store
        .chat_run_append_text("run-1", "a", now())
        .expect("delta");
    let mut first = open_stream(
        &app,
        stream_get(&thread, &format!("?after={snapshot}"), None),
    )
    .await;
    let before = collect_until(&mut first, |f| f.event == "text_delta").await;
    let last = before.last().and_then(|f| f.id).expect("last id");
    // Disconnect, the run goes on (a disconnect is not a stop).
    drop(first);
    finish_fake_run(&env, "run-1");
    let mut second = open_stream(&app, stream_get(&thread, "", Some(&last.to_string()))).await;
    assert_eq!(second.status, 200);
    let after = collect_until(&mut second, is_terminal_run).await;
    assert_eq!(
        after.last().map(|f| f.data["data"]["run"]["state"].clone()),
        Some(json!("completed")),
        "the run completed although the first stream was dropped"
    );
    let mut all = ids(&before);
    all.extend(ids(&after));
    assert!(all.windows(2).all(|w| w[0] < w[1]), "no duplicate: {all:?}");
    let expected = stored_ids(&app, &thread, "run-1", snapshot).await;
    let run_ids: Vec<u64> = before
        .iter()
        .chain(after.iter())
        .filter(|f| f.data["run_id"] == "run-1")
        .map(|f| f.id.expect("id"))
        .collect();
    assert_eq!(run_ids, expected);
    // The same cursor in both places is fine.
    let both = open_stream(
        &app,
        stream_get(&thread, &format!("?after={last}"), Some(&last.to_string())),
    )
    .await;
    assert_eq!(both.status, 200);
}

#[tokio::test(start_paused = true)]
async fn chat_stream_heartbeat_does_not_advance_the_id() {
    let env = admin_env();
    let app = env.router();
    let thread = thread_with_message(&app, "beat").await;
    let snapshot = snapshot_event_id(&app, &thread).await;
    let mut sse = open_stream(
        &app,
        stream_get(&thread, &format!("?after={snapshot}"), None),
    )
    .await;
    assert_eq!(sse.status, 200);
    // Nothing to send: the next frame is the 15 s heartbeat comment, without id/event/data.
    let beat = sse.next_frame(WAIT).await.expect("heartbeat");
    assert!(
        beat.event.is_empty() && beat.id.is_none() && beat.data == Value::Null,
        "{beat:?}"
    );
    claim(&env, &thread, "run-1");
    let next = collect_until(&mut sse, |_| true).await;
    let first_after = next[0].id.expect("id");
    assert!(first_after > snapshot);
    // The heartbeat consumed no event id: replaying from the snapshot yields the same first id.
    let mut replay = open_stream(
        &app,
        stream_get(&thread, &format!("?after={snapshot}"), None),
    )
    .await;
    let again = collect_until(&mut replay, |_| true).await;
    assert_eq!(again[0].id, Some(first_after));
}

#[tokio::test(start_paused = true)]
async fn chat_stream_expired_cursor_is_410_and_future_is_400() {
    let env = admin_env();
    let app = env.router();
    let thread = thread_with_message(&app, "expire").await;
    let snapshot = snapshot_event_id(&app, &thread).await;
    claim(&env, &thread, "run-1");
    finish_fake_run(&env, "run-1");
    let removed = env
        .store
        .chat_events_retention(now() + time::Duration::days(31), time::Duration::days(30))
        .expect("retention");
    assert!(removed > 0);
    let expired = send(
        &app,
        stream_get(&thread, &format!("?after={snapshot}"), None),
    )
    .await;
    assert_problem(&expired, 410, "chat-cursor-expired");
    let header_expired = send(&app, stream_get(&thread, "", Some(&snapshot.to_string()))).await;
    assert_problem(&header_expired, 410, "chat-cursor-expired");
    let run_page = send(
        &app,
        get_admin(&format!(
            "{BASE}/{thread}/runs/run-1/events?after={snapshot}"
        )),
    )
    .await;
    assert_problem(&run_page, 410, "chat-cursor-expired");
    // A cursor from the start (0) is still served: the remaining events are replayed.
    let mut from_start = open_stream(&app, stream_get(&thread, "?after=0", None)).await;
    assert_eq!(from_start.status, 200);
    let frames = collect_until(&mut from_start, is_terminal_run).await;
    assert!(
        frames
            .iter()
            .all(|f| !matches!(f.event.as_str(), "text_delta" | "tool"))
    );

    assert_problem(
        &send(&app, stream_get(&thread, "?after=999999", None)).await,
        400,
        "bad_request",
    );
    assert_problem(
        &send(&app, stream_get(&thread, "?after=1", Some("2"))).await,
        400,
        "bad_request",
    );
    assert_problem(
        &send(&app, stream_get(&thread, "?after=abc", None)).await,
        400,
        "bad_request",
    );
    assert_problem(
        &send(&app, stream_get("no-such-thread", "?after=0", None)).await,
        404,
        "chat_not_found",
    );
}

#[tokio::test(start_paused = true)]
async fn chat_stream_run_events_page() {
    let env = admin_env();
    let app = env.router();
    let thread = thread_with_message(&app, "pages").await;
    claim(&env, &thread, "run-1");
    finish_fake_run(&env, "run-1");
    let all = stored_ids(&app, &thread, "run-1", 0).await;
    assert!(all.len() >= 5, "{all:?}");
    let mut seen = Vec::new();
    let mut after = "0".to_string();
    loop {
        let page = send(
            &app,
            get_admin(&format!(
                "{BASE}/{thread}/runs/run-1/events?after={after}&limit=2"
            )),
        )
        .await;
        assert_eq!(page.status.as_u16(), 200, "{}", page.text());
        let body = page.json();
        let items = body["items"].as_array().expect("items");
        assert!(items.len() <= 2);
        for item in items {
            assert_eq!(item["run_id"], "run-1");
            seen.push(
                item["id"]
                    .as_str()
                    .expect("id")
                    .parse::<u64>()
                    .expect("numeric"),
            );
        }
        match body["next_cursor"].as_str() {
            Some(next) => after = next.to_string(),
            None => break,
        }
    }
    assert_eq!(seen, all);
    // The page size is capped at 500; 0 is rejected; unknown runs are 404.
    let capped = send(
        &app,
        get_admin(&format!("{BASE}/{thread}/runs/run-1/events?limit=100000")),
    )
    .await;
    assert_eq!(capped.status.as_u16(), 200);
    assert_problem(
        &send(
            &app,
            get_admin(&format!("{BASE}/{thread}/runs/run-1/events?limit=0")),
        )
        .await,
        400,
        "bad_request",
    );
    assert_problem(
        &send(
            &app,
            get_admin(&format!("{BASE}/{thread}/runs/nope/events")),
        )
        .await,
        404,
        "chat_not_found",
    );
    assert_problem(
        &send(
            &app,
            get_admin(&format!("{BASE}/{thread}/runs/run-1/events?bogus=1")),
        )
        .await,
        400,
        "bad_request",
    );
}

#[tokio::test(start_paused = true)]
async fn chat_stream_replay_crosses_store_page_boundary() {
    let env = admin_env();
    let app = env.router();
    let thread = thread_with_message(&app, "large-replay").await;
    let snapshot = snapshot_event_id(&app, &thread).await;
    claim(&env, &thread, "run-large");
    // The stream reads at most 500 rows per store call. The terminal event must
    // arrive after it advances the cursor and reads another page.
    for _ in 0..501 {
        env.store
            .chat_run_append_text("run-large", "x", now())
            .expect("append text");
    }
    env.store
        .chat_run_finish("run-large", ChatRunState::Completed, None, None, now())
        .expect("finish");
    let mut sse = open_stream(
        &app,
        stream_get(&thread, &format!("?after={snapshot}"), None),
    )
    .await;
    let frames = collect_until(&mut sse, is_terminal_run).await;
    let received = ids(&frames);
    assert!(received.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(received.len() > 500, "replay must cross a store page");
    let mut expected = Vec::new();
    let mut after = snapshot.to_string();
    loop {
        let page = send(
            &app,
            get_admin(&format!(
                "{BASE}/{thread}/runs/run-large/events?after={after}&limit=500"
            )),
        )
        .await;
        assert_eq!(page.status.as_u16(), 200, "{}", page.text());
        let body = page.json();
        expected.extend(body["items"].as_array().expect("items").iter().map(|item| {
            item["id"]
                .as_str()
                .expect("id")
                .parse::<u64>()
                .expect("numeric id")
        }));
        match body["next_cursor"].as_str() {
            Some(next) => after = next.to_string(),
            None => break,
        }
    }
    let received_run: Vec<u64> = frames
        .iter()
        .filter(|frame| frame.data["run_id"] == "run-large")
        .map(|frame| frame.id.expect("id"))
        .collect();
    assert_eq!(received_run, expected);
    assert_eq!(
        frames.iter().filter(|f| f.event == "text_delta").count(),
        501
    );
}

#[tokio::test(start_paused = true)]
async fn chat_stream_does_not_mix_threads() {
    let env = admin_env();
    let app = env.router();
    let thread = thread_with_message(&app, "first-thread").await;
    let other = thread_with_message(&app, "second-thread").await;
    let snapshot = snapshot_event_id(&app, &thread).await;
    claim(&env, &other, "other-run");
    finish_fake_run(&env, "other-run");
    claim(&env, &thread, "own-run");
    finish_fake_run(&env, "own-run");
    let mut sse = open_stream(
        &app,
        stream_get(&thread, &format!("?after={snapshot}"), None),
    )
    .await;
    let frames = collect_until(&mut sse, is_terminal_run).await;
    assert!(frames.iter().all(|f| f.data["thread_id"] == thread));
    assert!(frames.iter().all(|f| f.data["run_id"] != "other-run"));
    assert_eq!(
        frames
            .iter()
            .filter(|f| f.data["run_id"] == "own-run")
            .map(|f| f.id.expect("id"))
            .collect::<Vec<_>>(),
        stored_ids(&app, &thread, "own-run", snapshot).await
    );
}
