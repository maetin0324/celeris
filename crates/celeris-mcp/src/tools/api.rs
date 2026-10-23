//! ADR 2026-10-23-mcp-read-only-db-and-api D5: `api_get`（scope `api:read`）。
//!
//! daemon の中から `[api] listen` の loopback へ、`[api] token_file` の token で **GET だけ**を代行する
//! （llm-proxy と同じ経路。MCP の客に token は渡らない）。path は `/api/v1/` の下に限り、秘密を返し得る
//! endpoint は拒否リスト（`DENIED_PATHS` / `DENIED_SEGMENTS`）で止める。

use std::sync::Arc;
use std::time::Duration;

use schemars::JsonSchema;
use serde::Deserialize;
use task_core::McpScope;

use super::{ToolCallFuture, ToolDef, ToolError, ToolOutput, schema};
use crate::auth::AuthedClient;
use crate::state::McpState;

/// 前置（`task_api::BASE_PATH` と同じ）。
pub const API_PREFIX: &str = "/api/v1/";
/// 応答本文の上限。
pub const MAX_RESPONSE_BYTES: usize = 1 << 20;
/// 応答を待つ上限。
pub const API_TIMEOUT: Duration = Duration::from_secs(30);

/// 拒否する path の型（段ごとに比べる。`*` は 1 段、末尾の `**` は 0 段以上）と理由。
/// ADR 2026-10-23-mcp-read-only-db-and-api D5 の表の正本。
pub const DENIED_PATHS: &[(&str, &str)] = &[
    ("/api/v1/secrets/**", "secret の目録と値"),
    ("/api/v1/browser/credentials/**", "保存 credential"),
    (
        "/api/v1/browser/identities/**",
        "保存した login 状態（cookie）",
    ),
    (
        "/api/v1/tasks/*/browser/live/**",
        "Live View の映像・出来事・読み取り",
    ),
    (
        "/api/v1/tasks/*/browser/waits/*/credential",
        "credential の受け渡し",
    ),
    ("/api/v1/clusters/*/connect/**", "TOTP の接続 prompt"),
    (
        "/api/v1/llm/sources/*/accounts/*/login/**",
        "アカウントの login 手続き",
    ),
    ("/api/v1/chat/attachments/*/content", "添付の生 bytes"),
    ("/api/v1/chat/attachments/*/preview", "添付の生 bytes"),
    ("/api/v1/stream", "SSE（終わらない応答）"),
    ("/api/v1/console/stream", "SSE（終わらない応答）"),
    ("/api/v1/chat/threads/*/stream", "SSE（終わらない応答）"),
];

/// どこに現れても拒否する段（将来の秘密系 endpoint の既定拒否）。
pub const DENIED_SEGMENTS: &[&str] = &[
    "secret",
    "secrets",
    "credential",
    "credentials",
    "token",
    "tokens",
    "cookie",
    "cookies",
    "password",
    "passwords",
    "login",
];

/// API への GET の宛先（daemon が `[api]` から組み立てて `McpState::set_api_target` で渡す）。
#[derive(Clone)]
pub struct ApiTarget {
    /// `http://127.0.0.1:<port>`（末尾の `/` なし）。
    pub base_url: String,
    pub token: Option<String>,
    pub(crate) client: reqwest::Client,
}

impl std::fmt::Debug for ApiTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiTarget")
            .field("base_url", &self.base_url)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl ApiTarget {
    pub fn new(base_url: impl Into<String>, token: Option<String>) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(API_TIMEOUT)
            .build()
            .map_err(|e| format!("could not build the HTTP client: {e}"))?;
        Ok(Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token,
            client,
        })
    }

    /// `[api] listen` から作る（`0.0.0.0` / `::` は loopback に直す）。
    pub fn from_listen(
        listen: std::net::SocketAddr,
        token: Option<String>,
    ) -> Result<Self, String> {
        let ip = if listen.ip().is_unspecified() {
            match listen {
                std::net::SocketAddr::V4(_) => std::net::IpAddr::from([127, 0, 0, 1]),
                std::net::SocketAddr::V6(_) => {
                    std::net::IpAddr::from(std::net::Ipv6Addr::LOCALHOST)
                }
            }
        } else {
            listen.ip()
        };
        Self::new(
            format!("http://{}", std::net::SocketAddr::new(ip, listen.port())),
            token,
        )
    }
}

fn segment_matches(pattern: &[&str], segments: &[&str]) -> bool {
    match pattern.split_first() {
        None => segments.is_empty(),
        Some((&"**", [])) => true,
        Some((p, rest)) => match segments.split_first() {
            Some((s, srest)) => {
                (*p == "*" || p.eq_ignore_ascii_case(s)) && segment_matches(rest, srest)
            }
            None => false,
        },
    }
}

/// 拒否されるなら理由を返す。`path` は `validate_path` を通ったもの。
pub fn denied_reason(path: &str) -> Option<&'static str> {
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    if segments
        .iter()
        .any(|s| DENIED_SEGMENTS.iter().any(|d| d.eq_ignore_ascii_case(s)))
    {
        return Some(
            "秘密を返し得る endpoint（secret・credential・token・cookie・password・login）",
        );
    }
    DENIED_PATHS.iter().find_map(|(pattern, why)| {
        let p: Vec<&str> = pattern.trim_start_matches('/').split('/').collect();
        segment_matches(&p, &segments).then_some(*why)
    })
}

/// path の形を確かめる（`/api/v1/` の下・traversal なし・query は `query` で渡す）。
pub fn validate_path(path: &str) -> Result<(), ToolError> {
    let Some(rest) = path.strip_prefix(API_PREFIX) else {
        return Err(ToolError::rejected(format!(
            "path は {API_PREFIX} で始めてください（例: /api/v1/tasks）: {path:?}"
        )));
    };
    if rest.is_empty() {
        return Err(ToolError::rejected("path に endpoint がありません"));
    }
    if path
        .chars()
        .any(|c| c.is_control() || matches!(c, '?' | '#' | '%' | '\\') || c.is_whitespace())
    {
        return Err(ToolError::rejected(
            "path に ? # % \\ 空白・制御文字は使えません（query は query 引数に object で渡す）",
        ));
    }
    for seg in rest.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return Err(ToolError::rejected(format!(
                "path に空の段・. ・.. は使えません: {path:?}"
            )));
        }
    }
    Ok(())
}

fn query_pairs(
    query: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Result<Vec<(String, String)>, ToolError> {
    let Some(query) = query else {
        return Ok(Vec::new());
    };
    query
        .iter()
        .map(|(k, v)| {
            let v = match v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                other => {
                    return Err(ToolError::invalid_params(format!(
                        "query の値は文字列・数・真偽だけ: {k}={other}"
                    )));
                }
            };
            Ok((k.clone(), v))
        })
        .collect()
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApiGetArgs {
    /// `/api/v1/` で始まる path（例: `/api/v1/tasks/01M…/events`）。query は含めない。
    pub path: String,
    /// query の組（値は文字列・数・真偽）。例: `{"limit": 50}`。
    #[serde(default)]
    pub query: Option<serde_json::Map<String, serde_json::Value>>,
    /// 省略するか `GET`。他の method は拒否する。
    #[serde(default)]
    pub method: Option<String>,
}

pub fn get_def() -> ToolDef {
    ToolDef {
        name: "api_get",
        description: "Celeris の HTTP API（/api/v1）に GET だけを代行する。返り値は {status, content_type, body}（JSON は値、他の text は文字列）。応答 1 MiB・30 秒まで。secret・credential・browser の live/identity・TOTP・SSE の path は拒否。endpoint の一覧は GET /api/v1/schema。",
        scope: McpScope::ApiRead,
        input_schema: schema::<ApiGetArgs>,
        call: call_get,
    }
}

/// 要求の検査（method・path・拒否リスト・query）。通れば query の組を返す。
pub fn check_request(args: &ApiGetArgs) -> Result<Vec<(String, String)>, ToolError> {
    if let Some(m) = &args.method
        && !m.eq_ignore_ascii_case("GET")
    {
        return Err(ToolError::rejected(format!(
            "api_get は GET だけです（{m} は使えません。変更は scope で許された専用の道具で行う）"
        )));
    }
    validate_path(&args.path)?;
    if let Some(why) = denied_reason(&args.path) {
        return Err(ToolError::rejected(format!(
            "この path は api_get で読めません（{why}）: {}",
            args.path
        )));
    }
    query_pairs(args.query.as_ref())
}

fn call_get<'a>(
    state: &'a Arc<McpState>,
    _client: &'a AuthedClient,
    args: serde_json::Value,
) -> ToolCallFuture<'a> {
    Box::pin(async move {
        let args: ApiGetArgs = serde_json::from_value(args)
            .map_err(|e| ToolError::invalid_params(format!("引数が不正です: {e}")))?;
        let pairs = check_request(&args)?;
        let Some(target) = state.api_target.get() else {
            return Err(ToolError::internal(
                "この MCP サーバーには API の宛先が設定されていません（[api] listen が無い）",
            ));
        };
        let mut url = reqwest::Url::parse(&format!("{}{}", target.base_url, args.path))
            .map_err(|e| ToolError::invalid_params(format!("path を URL にできません: {e}")))?;
        if !pairs.is_empty() {
            url.query_pairs_mut().extend_pairs(&pairs);
        }
        let mut req = target.client.get(url);
        if let Some(token) = &target.token {
            req = req.bearer_auth(token);
        }
        let mut resp = req
            .send()
            .await
            .map_err(|e| ToolError::internal(format!("API に繋がりません: {e}")))?;
        let status = resp.status().as_u16();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let mut body = Vec::new();
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| ToolError::internal(format!("API の応答を読めません: {e}")))?
        {
            if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(ToolError::rejected(format!(
                    "応答が {MAX_RESPONSE_BYTES} bytes を超えました。limit 等の query で絞ってください"
                )));
            }
            body.extend_from_slice(&chunk);
        }
        let body = if content_type.contains("json") {
            serde_json::from_slice::<serde_json::Value>(&body).unwrap_or_else(|_| {
                serde_json::Value::String(String::from_utf8_lossy(&body).into_owned())
            })
        } else {
            match String::from_utf8(body) {
                Ok(text) => serde_json::Value::String(text),
                Err(_) => {
                    return Err(ToolError::rejected(format!(
                        "text でない応答（{content_type}）は返せません"
                    )));
                }
            }
        };
        Ok(ToolOutput::from_value(serde_json::json!({
            "status": status,
            "content_type": content_type,
            "body": body,
        })))
    })
}

/// `mcp_calls.request` に残す要求（path と query）。
pub fn audit_request(args: &serde_json::Value) -> Option<String> {
    let path = args.get("path")?.as_str()?;
    let method = args.get("method").and_then(|m| m.as_str()).unwrap_or("GET");
    match args.get("query") {
        Some(q) if q.as_object().is_some_and(|o| !o.is_empty()) => {
            Some(format!("{method} {path} {q}"))
        }
        _ => Some(format!("{method} {path}")),
    }
}

#[cfg(test)]
#[path = "api_tests.rs"]
mod tests;
