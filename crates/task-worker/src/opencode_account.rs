//! opencode go の利用枠の確認（ADR 2026-10-06 D2）。
//!
//! 推論を使わず、`<account dir>/opencode/auth.json` の鍵で `GET <usage_url>`（`Authorization: Bearer`）を
//! 呼ぶだけの決定的な HTTP。応答の解析は `task_core::accounts::parse_opencode_go_usage`（純粋関数）。
//! 鍵は要求にだけ使い、ログ・戻り値・`Debug` に値を出さない。
//!
//! 結果の写し方（codex の確認と同じ語彙）:
//! - `auth.json` が無い・鍵が読めない・401/403 → `AuthFailed`（ログインが要る）
//! - 429 → `Throttled`
//! - 通信失敗・timeout・解析できない応答 → `SpawnFailed`（観測は更新しない。前回値を消さない）

use std::path::Path;
use std::time::Duration;

use task_core::accounts::parse_opencode_go_usage;

use crate::claude_account::{AccountCheck, AccountCheckResult};

/// `auth.json` から読んだ opencode go の鍵。`Debug` は値を出さない。
struct GoKey(String);

impl std::fmt::Debug for GoKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GoKey(<redacted>)")
    }
}

/// `<account_dir>/opencode/auth.json` の `["opencode-go"]`（`type` は `api`）から鍵を取る。
fn read_go_key(account_dir: &Path) -> Option<GoKey> {
    let text = std::fs::read_to_string(account_dir.join("opencode").join("auth.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let entry = value.get("opencode-go")?;
    if entry.get("type").and_then(|t| t.as_str()) != Some("api") {
        return None;
    }
    let key = entry.get("key")?.as_str()?.trim();
    (!key.is_empty()).then(|| GoKey(key.to_owned()))
}

fn failed(result: AccountCheckResult, detail: &str) -> AccountCheck {
    AccountCheck {
        result,
        detail: Some(detail.into()),
        observation: None,
    }
}

/// opencode go の認証と利用枠を確認する。`timeout` は要求全体（接続・応答）の上限。
pub async fn check_account_opencode_go(
    account_dir: &Path,
    usage_url: &str,
    timeout: Duration,
) -> AccountCheck {
    let Some(key) = read_go_key(account_dir) else {
        return failed(
            AccountCheckResult::AuthFailed,
            "opencode/auth.json has no opencode-go api key",
        );
    };
    let client = match reqwest::Client::builder().timeout(timeout).build() {
        Ok(client) => client,
        Err(_) => {
            return failed(
                AccountCheckResult::SpawnFailed,
                "could not build http client",
            );
        }
    };
    let response = match client
        .get(usage_url)
        .bearer_auth(&key.0)
        .header("accept", "application/json")
        .send()
        .await
    {
        Ok(response) => response,
        // reqwest のエラー文面は URL を含みうるが鍵は含まない（鍵は header にしか載せない）。種別だけを出す。
        Err(e) if e.is_timeout() => return failed(AccountCheckResult::SpawnFailed, "timeout"),
        Err(_) => {
            return failed(
                AccountCheckResult::SpawnFailed,
                "could not reach the opencode go usage endpoint",
            );
        }
    };
    let status = response.status();
    match status.as_u16() {
        401 | 403 => {
            return failed(
                AccountCheckResult::AuthFailed,
                &format!("opencode go rejected the key (HTTP {})", status.as_u16()),
            );
        }
        429 => {
            return failed(
                AccountCheckResult::Throttled,
                "opencode go usage endpoint: HTTP 429",
            );
        }
        s if !(200..300).contains(&s) => {
            return failed(
                AccountCheckResult::SpawnFailed,
                &format!("opencode go usage endpoint: HTTP {s}"),
            );
        }
        _ => {}
    }
    let Ok(body) = response.text().await else {
        return failed(
            AccountCheckResult::SpawnFailed,
            "could not read usage response",
        );
    };
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    match parse_opencode_go_usage(&body, now) {
        Ok(observation) => AccountCheck {
            result: AccountCheckResult::Ok,
            detail: None,
            observation: Some(observation),
        },
        Err(message) => failed(AccountCheckResult::SpawnFailed, &message),
    }
}

#[cfg(test)]
mod tests;
