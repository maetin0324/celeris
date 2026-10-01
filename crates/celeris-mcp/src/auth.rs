//! ADR-0056 D1 / D4（Phase 78）: 口ごとの認証。決定的（LLM も I/O 以外の判断も無い。DESIGN 原則 1）。

use sha2::{Digest, Sha256};
use task_core::{McpScope, TaskStore};
use time::OffsetDateTime;

use crate::config::ListenerAuth;

/// トークンの値から DB に残す形（SHA-256 の 16 進）を作る。**値そのものはここにしか通らない**
/// （呼び出し側はこの結果だけを保存・比較する）。
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// `celerisctl mcp client add` が発行するトークン（新しいクレートを足さないため、乱数は
/// `ulid`（OS の乱数源を使う thread-local RNG）に頼る。3 本つなげて十分な当てずっぽう耐性を持たせる）。
pub fn generate_token() -> String {
    format!(
        "{}{}{}",
        ulid::Ulid::new(),
        ulid::Ulid::new(),
        ulid::Ulid::new()
    )
    .to_lowercase()
}

/// 認証を通ったクライアント（スコープ込み。トークンの値は持たない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthedClient {
    pub id: String,
    pub name: String,
    pub scopes: Vec<McpScope>,
}

impl AuthedClient {
    pub fn has_scope(&self, scope: McpScope) -> bool {
        self.scopes.contains(&scope)
    }
}

/// 認証の失敗（HTTP 401 に写す。理由は応答に出さない — トークンの手掛かりを与えないため）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// `auth = "token"` の口で `Authorization: Bearer <token>` が無い・形が違う。
    MissingToken,
    /// トークンがどのクライアントとも一致しない。
    InvalidToken,
    /// クライアントが失効している。
    Revoked,
    /// `auth = "none"` の口が指す `client` が存在しない（設定ミス）。
    UnknownFixedClient,
}

/// 口の認証方式にしたがってクライアントを決める。成功したら `last_used_at` を更新する。
pub fn authenticate(
    store: &dyn TaskStore,
    listener_auth: &ListenerAuth,
    bearer: Option<&str>,
    now: OffsetDateTime,
) -> Result<AuthedClient, AuthError> {
    let client = match listener_auth {
        ListenerAuth::Token => {
            let token = bearer
                .filter(|t| !t.is_empty())
                .ok_or(AuthError::MissingToken)?;
            let hash = hash_token(token);
            store
                .mcp_client_by_token_hash(&hash)
                .ok()
                .flatten()
                .ok_or(AuthError::InvalidToken)?
        }
        ListenerAuth::Fixed(client_id) => store
            .mcp_client_get(client_id)
            .ok()
            .flatten()
            .ok_or(AuthError::UnknownFixedClient)?,
    };
    if client.is_revoked() {
        return Err(AuthError::Revoked);
    }
    let _ = store.mcp_client_touch_last_used(&client.id, now);
    Ok(AuthedClient {
        id: client.id,
        name: client.name,
        scopes: client.scopes,
    })
}

/// `Authorization: Bearer <token>` からトークンの値だけを取り出す。
pub fn bearer_token(value: Option<&str>) -> Option<&str> {
    value?
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

#[cfg(test)]
#[path = "auth/tests.rs"]
mod tests;
