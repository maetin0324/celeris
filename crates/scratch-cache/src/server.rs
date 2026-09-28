//! loopback の WebDAV サブセット（ADR-0075 D5 (b)、U5 の実測で sccache 0.18 が発行したメソッドだけ）。
//!
//! - `GET` / `HEAD` `/<prefix>/<k0>/<k1>/<k2>/<key>`: L1 → L2 → 404。
//! - `PUT` 同じ path: L1 に書いて 201（L2 へは flusher）。
//! - `PROPFIND`（Depth 0）: collection（`/` で終わる path）には常に 207（ディレクトリは仮想。opendal は PUT の前に
//!   親の collection を PROPFIND し、`getlastmodified` が無いと失敗する）。ファイルは在れば 207、無ければ 404。
//! - `MKCOL`: 201（通常は呼ばれない）。
//! - `/<prefix>/.sccache_check`: sccache の storage check。メモリだけで扱う。
//! - `/healthz`（認証なし）、`/stats`（認証なし、`celeris.scratch-cache-stats/1`）。
//!
//! 認証は `Authorization: Bearer <token>`（`SCCACHE_WEBDAV_TOKEN`）。token を設定しなければ認証しない（loopback だけに bind）。

use std::future::Future;
use std::sync::Arc;
use std::time::SystemTime;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

use crate::key::{self, CHECK_KEY};
use crate::store::{Lookup, TieredStore};

/// `serve` の設定。
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// DAV のメソッドに要求する Bearer token（`None` = 認証しない）。
    pub token: Option<String>,
    /// PUT の本文の上限（byte）。
    pub max_body_bytes: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            token: None,
            max_body_bytes: 1024 * 1024 * 1024,
        }
    }
}

struct App {
    store: TieredStore,
    token: Option<String>,
}

/// router を組む（テストは `tokio::net::TcpListener` の port 0 に `serve` する）。
pub fn router(store: TieredStore, cfg: ServerConfig) -> Router {
    let app = Arc::new(App {
        store,
        token: cfg.token.filter(|t| !t.is_empty()),
    });
    Router::new()
        .fallback(handle)
        .layer(DefaultBodyLimit::max(cfg.max_body_bytes))
        .with_state(app)
}

/// `listener` で `shutdown` が終わるまで応答する。
pub async fn serve(
    listener: tokio::net::TcpListener,
    router: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await
}

fn status(code: StatusCode) -> Response {
    (code, Body::empty()).into_response()
}

/// HTTP-date（`getlastmodified`。opendal は RFC 2822 として読む）。
fn http_date(t: SystemTime) -> String {
    let d = time::OffsetDateTime::from(t);
    let wd = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    let mo = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "{}, {:02} {} {} {:02}:{:02}:{:02} GMT",
        wd[d.weekday().number_days_from_monday() as usize],
        d.day(),
        mo[(u8::from(d.month()) as usize).saturating_sub(1) % 12],
        d.year(),
        d.hour(),
        d.minute(),
        d.second()
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn multistatus(href: &str, collection: bool, len: Option<u64>) -> Response {
    let prop = if collection {
        "<D:resourcetype><D:collection/></D:resourcetype>".to_string()
    } else {
        format!(
            "<D:resourcetype/><D:getcontentlength>{}</D:getcontentlength>",
            len.unwrap_or(0)
        )
    };
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <D:multistatus xmlns:D=\"DAV:\"><D:response><D:href>{}</D:href><D:propstat><D:prop>{prop}\
         <D:getlastmodified>{}</D:getlastmodified></D:prop><D:status>HTTP/1.1 200 OK</D:status>\
         </D:propstat></D:response></D:multistatus>",
        xml_escape(href),
        http_date(SystemTime::now())
    );
    let mut r = (StatusCode::MULTI_STATUS, body).into_response();
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/xml; charset=utf-8"),
    );
    r
}

fn authorized(app: &App, headers: &HeaderMap) -> bool {
    let Some(token) = app.token.as_deref() else {
        return true;
    };
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| t.trim() == token)
}

/// 本文つきの 200。HEAD でも本文を渡す（hyper は HEAD の本文を送らず、`Content-Length` は本文の長さになる）。
fn octets(bytes: Vec<u8>) -> Response {
    let mut r = (StatusCode::OK, bytes).into_response();
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    r
}

async fn handle(
    State(app): State<Arc<App>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri.path();
    match (method.as_str(), path) {
        ("GET" | "HEAD", "/healthz") => return (StatusCode::OK, "ok\n").into_response(),
        ("GET", "/stats") => {
            let store = app.store.clone();
            return match tokio::task::spawn_blocking(move || store.stats()).await {
                Ok(s) => match serde_json::to_vec(&s) {
                    Ok(json) => {
                        let mut r = (StatusCode::OK, json).into_response();
                        r.headers_mut().insert(
                            header::CONTENT_TYPE,
                            HeaderValue::from_static("application/json"),
                        );
                        r
                    }
                    Err(_) => status(StatusCode::INTERNAL_SERVER_ERROR),
                },
                Err(_) => status(StatusCode::INTERNAL_SERVER_ERROR),
            };
        }
        _ => {}
    }
    if !authorized(&app, &headers) {
        let mut r = status(StatusCode::UNAUTHORIZED);
        r.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Bearer realm=\"celeris-scratch-cache\""),
        );
        return r;
    }
    let collection = path.ends_with('/');
    let last = path.rsplit('/').next().unwrap_or("");
    match method.as_str() {
        "PROPFIND" => {
            if collection {
                return multistatus(path, true, None);
            }
            if last == CHECK_KEY {
                return match app.store.check_object() {
                    Some(b) => multistatus(path, false, Some(b.len() as u64)),
                    None => status(StatusCode::NOT_FOUND),
                };
            }
            if !key::valid_key(last) {
                return status(StatusCode::BAD_REQUEST);
            }
            let store = app.store.clone();
            let k = last.to_string();
            match tokio::task::spawn_blocking(move || store.contains(&k)).await {
                Ok(true) => multistatus(path, false, None),
                Ok(false) => status(StatusCode::NOT_FOUND),
                Err(_) => status(StatusCode::INTERNAL_SERVER_ERROR),
            }
        }
        "MKCOL" => status(StatusCode::CREATED),
        "GET" | "HEAD" => {
            if collection {
                return status(StatusCode::NOT_FOUND);
            }
            if last == CHECK_KEY {
                return match app.store.check_object() {
                    Some(b) => octets(b),
                    None => status(StatusCode::NOT_FOUND),
                };
            }
            if !key::valid_key(last) {
                return status(StatusCode::BAD_REQUEST);
            }
            let store = app.store.clone();
            let k = last.to_string();
            match tokio::task::spawn_blocking(move || store.get(&k)).await {
                Ok(Ok(Lookup::L1(b) | Lookup::L2(b))) => octets(b),
                Ok(Ok(Lookup::Miss)) => status(StatusCode::NOT_FOUND),
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, "scratch-cache: GET failed");
                    status(StatusCode::NOT_FOUND)
                }
                Err(_) => status(StatusCode::INTERNAL_SERVER_ERROR),
            }
        }
        "PUT" => {
            if collection {
                return status(StatusCode::METHOD_NOT_ALLOWED);
            }
            if last == CHECK_KEY {
                app.store.set_check_object(body.to_vec());
                return status(StatusCode::CREATED);
            }
            if !key::valid_key(last) {
                return status(StatusCode::BAD_REQUEST);
            }
            let store = app.store.clone();
            let k = last.to_string();
            match tokio::task::spawn_blocking(move || store.put(&k, &body)).await {
                Ok(Ok(())) => status(StatusCode::CREATED),
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, "scratch-cache: PUT failed to write L1");
                    status(StatusCode::INSUFFICIENT_STORAGE)
                }
                Err(_) => status(StatusCode::INTERNAL_SERVER_ERROR),
            }
        }
        _ => status(StatusCode::METHOD_NOT_ALLOWED),
    }
}
