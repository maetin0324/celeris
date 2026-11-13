#[path = "../../../../tests/support/http_async.rs"]
mod asynchronous;
#[path = "../../../../tests/support/http_request.rs"]
mod framing;
#[path = "../../../../tests/support/http_sync.rs"]
mod synchronous;

mod browser_startup_reap;
mod fake_http;
mod workspace_roots;
