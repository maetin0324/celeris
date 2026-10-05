//! CoS chat SSE and run events (ADR 2026-10-05 cos-chat-home D2).
//!
//! `GET /chat/threads/{t}/stream` replays `chat_events` after the cursor (`after` or
//! `Last-Event-ID`) and then keeps reading from the same cursor, so the replay/live boundary is
//! the event id itself: no gap and no duplicate. Live delivery starts from the store: REST writes
//! wake the loop through `ChatState::events`; writes from other connections (cos-run) are picked
//! up by the `poll_interval` re-read, the same way as `crate::sse`. Heartbeats are SSE comments
//! and never move the cursor. A disconnect only ends this loop; it does not stop any run.

use std::convert::Infallible;

use axum::body::{Body, Bytes};
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::get;
use task_core::chat::{
    CHAT_EVENT_PAGE_DEFAULT, CHAT_EVENT_PAGE_MAX, ChatError, ChatEvent, ChatEventQuery,
};
use tokio::sync::mpsc;
use tokio::time::{Instant, MissedTickBehavior};

use super::{authorize, chat_problem};
use crate::handlers::{ApiResult, Params, json_response};
use crate::problem::ApiProblem;
use crate::query::QueryParams;
use crate::sse::{CHANNEL_CAPACITY, X_ACCEL_BUFFERING, closed, send};
use crate::state::{ApiState, StreamSlot};

const LAST_EVENT_ID: HeaderName = HeaderName::from_static("last-event-id");
/// Replay/live page size per store read.
const STREAM_PAGE: u32 = CHAT_EVENT_PAGE_MAX;
/// SSE comment; it carries no `id:` so the client's Last-Event-ID stays put.
const HEARTBEAT: &[u8] = b": heartbeat\n\n";

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new()
        .route("/api/v1/chat/threads/{t}/stream", get(stream))
        .route("/api/v1/chat/threads/{t}/runs/{r}/events", get(run_events))
}

/// D2: the cursor errors use their own codes (`chat-cursor-expired` → 410, client refetches the
/// history snapshot); everything else keeps the chat REST mapping.
fn stream_problem(error: ChatError) -> ApiProblem {
    match error {
        ChatError::CursorExpired(cursor) => ApiProblem::new(
            StatusCode::GONE,
            "chat-cursor-expired",
            format!("event cursor {cursor} has been removed by retention; reload the history"),
        ),
        other => chat_problem(other),
    }
}

/// The resume cursor: `after` and `Last-Event-ID` must agree when both are present.
fn requested_cursor(query: Option<&str>, headers: &HeaderMap) -> Result<String, ApiProblem> {
    let header = match headers.get(LAST_EVENT_ID) {
        Some(value) => Some(
            value
                .to_str()
                .map(|v| v.trim().to_string())
                .map_err(|_| ApiProblem::bad_request("Last-Event-ID must be an event id"))?,
        ),
        None => None,
    };
    match (query, header) {
        (Some(q), Some(h)) if q != h => Err(ApiProblem::bad_request(format!(
            "`after` ({q}) and Last-Event-ID ({h}) disagree"
        ))),
        (_, Some(h)) => Ok(h),
        (Some(q), None) => Ok(q.to_string()),
        // D2: `after=0` (and no cursor) replays every event still kept.
        (None, None) => Ok("0".to_string()),
    }
}

async fn stream(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params((t,)): Params<(String,)>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiProblem> {
    authorize(&state, &headers)?;
    let params = QueryParams::parse(raw.as_deref(), &["after"])?;
    let after = requested_cursor(params.single("after")?, &headers)?;
    let thread = t.clone();
    // 404 / 400 (malformed or future) / 410 are decided before the 200 is sent.
    let cursor = state
        .blocking(move |store| {
            store
                .chat_event_cursor_check(&thread, &after)
                .map_err(stream_problem)
        })
        .await?;
    let slot = state
        .try_open_stream()
        .ok_or_else(ApiProblem::too_many_streams)?;
    let (tx, rx) = mpsc::channel::<Bytes>(CHANNEL_CAPACITY);
    tokio::spawn(run(state, slot, tx, t, cursor));

    let body = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv()
            .await
            .map(|bytes| (Ok::<Bytes, Infallible>(bytes), rx))
    });
    let mut response = Response::new(Body::from_stream(body));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream; charset=utf-8"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(X_ACCEL_BUFFERING, HeaderValue::from_static("no"));
    Ok(response)
}

async fn run(
    state: ApiState,
    _slot: StreamSlot,
    tx: mpsc::Sender<Bytes>,
    thread_id: String,
    mut cursor: i64,
) {
    let tuning = state.tuning;
    let events = state.chat.events.clone();
    let mut shutdown = state.inner.shutdown.subscribe();
    let mut poll =
        tokio::time::interval_at(Instant::now() + tuning.poll_interval, tuning.poll_interval);
    poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut heartbeat = tokio::time::interval_at(
        Instant::now() + tuning.heartbeat_interval,
        tuning.heartbeat_interval,
    );
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        // Register for the wake-up before reading so a write landing between the read and the
        // wait is not missed.
        let notified = events.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        match drain(&state, &tx, &thread_id, &mut cursor).await {
            Drain::CaughtUp => {}
            Drain::Stop => return,
        }
        let keep_going = tokio::select! {
            biased;
            _ = tx.closed() => false,
            _ = closed(&mut shutdown) => false,
            _ = &mut notified => true,
            _ = heartbeat.tick() => send(&state, &tx, Some(Bytes::from_static(HEARTBEAT))).await,
            _ = poll.tick() => true,
        };
        if !keep_going {
            return;
        }
    }
}

enum Drain {
    CaughtUp,
    Stop,
}

/// Sends every event after `cursor` (page by page) and advances it to the last sent id.
async fn drain(
    state: &ApiState,
    tx: &mpsc::Sender<Bytes>,
    thread_id: &str,
    cursor: &mut i64,
) -> Drain {
    loop {
        let thread = thread_id.to_string();
        let query = ChatEventQuery {
            after: Some(cursor.to_string()),
            run_id: None,
            limit: Some(STREAM_PAGE),
        };
        let page = match state
            .blocking(move |store| Ok(store.chat_events_page(&thread, &query)))
            .await
        {
            Ok(Ok(page)) => page,
            Ok(Err(ChatError::CursorExpired(_) | ChatError::NotFound { .. })) => {
                // Retention passed the cursor (or the thread is gone): end the stream; the
                // reconnect gets the 410/404 and the client reloads its snapshot.
                return Drain::Stop;
            }
            Ok(Err(error)) => {
                tracing::warn!(
                    error = %error,
                    "chat SSE read failed; retrying on the next tick"
                );
                return Drain::CaughtUp;
            }
            Err(problem) => {
                tracing::warn!(
                    code = problem.code(),
                    "chat SSE read failed; retrying on the next tick"
                );
                return Drain::CaughtUp;
            }
        };
        for event in &page.items {
            let Ok(id) = event.id.parse::<i64>() else {
                tracing::warn!(id = %event.id, "chat event id is not numeric");
                return Drain::Stop;
            };
            if !send(state, tx, event_frame(event)).await {
                return Drain::Stop;
            }
            *cursor = id;
        }
        if page.next_cursor.is_none() {
            return Drain::CaughtUp;
        }
    }
}

/// `event: <type>\nid: <chat_events.id>\ndata: <E>\n\n`.
fn event_frame(event: &ChatEvent) -> Option<Bytes> {
    let name = serde_json::to_value(event.event_type)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))?;
    let data = match serde_json::to_string(event) {
        Ok(data) => data,
        Err(e) => {
            tracing::warn!(error = %e, "failed to serialize a chat SSE frame");
            return None;
        }
    };
    Some(Bytes::from(format!(
        "event: {name}\nid: {}\ndata: {data}\n\n",
        event.id
    )))
}

/// `GET /chat/threads/{t}/runs/{r}/events?after&limit` (limit default 100, max 500).
async fn run_events(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params((t, r)): Params<(String, String)>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    authorize(&state, &headers)?;
    let params = QueryParams::parse(raw.as_deref(), &["after", "limit"])?;
    let limit = params.limit(
        "limit",
        CHAT_EVENT_PAGE_DEFAULT as usize,
        CHAT_EVENT_PAGE_MAX as usize,
    )?;
    let query = ChatEventQuery {
        after: params.single("after")?.map(str::to_owned),
        run_id: Some(r),
        limit: Some(u32::try_from(limit).unwrap_or(CHAT_EVENT_PAGE_MAX)),
    };
    let result = state
        .blocking(move |store| store.chat_events_page(&t, &query).map_err(stream_problem))
        .await?;
    Ok(json_response(StatusCode::OK, &result))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_header(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            LAST_EVENT_ID,
            HeaderValue::from_str(value).unwrap_or(HeaderValue::from_static("x")),
        );
        headers
    }

    #[test]
    fn chat_stream_cursor_prefers_agreeing_sources() {
        let none = HeaderMap::new();
        assert_eq!(requested_cursor(None, &none).ok().as_deref(), Some("0"));
        assert_eq!(
            requested_cursor(Some("5"), &none).ok().as_deref(),
            Some("5")
        );
        let h = with_header("7");
        assert_eq!(requested_cursor(None, &h).ok().as_deref(), Some("7"));
        assert_eq!(requested_cursor(Some("7"), &h).ok().as_deref(), Some("7"));
        assert!(requested_cursor(Some("6"), &h).is_err());
    }
}
