//! HTTP の道具（ADR 2026-11-19-cos-handlers-remaining-edges D1）: 本文の読み取り・JSON の応答・path の値・query の拒否。
//! route の表（`handlers`）と CoS の route（`cos`）の両方が使う。`crate::problem`・`crate::query`・crate root の定数と
//! 外部 crate だけを使い、task-api の循環の module を import しない。

use axum::body::Body;
use axum::extract::{FromRequestParts, Path};
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::MAX_BODY_BYTES;
use crate::problem::ApiProblem;
use crate::query::QueryParams;

pub(crate) type ApiResult = Result<Response, ApiProblem>;

const JSON_CONTENT_TYPE: &str = "application/json; charset=utf-8";

pub(crate) fn json_response<T: Serialize>(status: StatusCode, value: &T) -> Response {
    match serde_json::to_vec(value) {
        Ok(body) => {
            let mut response = Response::new(Body::from(body));
            *response.status_mut() = status;
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static(JSON_CONTENT_TYPE),
            );
            response
        }
        Err(e) => {
            ApiProblem::internal(format!("failed to serialize the response: {e}")).into_response()
        }
    }
}

/// path の値。解析の失敗は 400 `bad_request`（axum の既定の text 応答にしない）。
pub(crate) struct Params<T>(pub(crate) T);

impl<S, T> FromRequestParts<S> for Params<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = ApiProblem;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Path::<T>::from_request_parts(parts, state).await {
            Ok(Path(value)) => Ok(Params(value)),
            Err(rejection) => Err(ApiProblem::bad_request(rejection.body_text())),
        }
    }
}

/// 本文を 1 MiB まで読む（超えたら 413）。
pub(crate) async fn read_body(body: Body) -> Result<Vec<u8>, ApiProblem> {
    let mut stream = body.into_data_stream();
    let mut buf = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            ApiProblem::bad_request(format!("failed to read the request body: {e}"))
        })?;
        if buf.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(ApiProblem::payload_too_large());
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// JSON 本文を解析する。構文誤り・未知フィールド・型誤りは 400。`empty_is_object` なら空本体を `{}` とみなす。
pub(crate) async fn read_json<T: DeserializeOwned>(
    body: Body,
    empty_is_object: bool,
) -> Result<T, ApiProblem> {
    let bytes = read_body(body).await?;
    let text: &[u8] = if empty_is_object && bytes.iter().all(u8::is_ascii_whitespace) {
        b"{}"
    } else {
        &bytes
    };
    serde_json::from_slice(text)
        .map_err(|e| ApiProblem::bad_request(format!("invalid JSON body: {e}")))
}

pub(crate) fn no_query(raw: &Option<String>) -> Result<(), ApiProblem> {
    QueryParams::parse(raw.as_deref(), &[]).map(|_| ())
}
