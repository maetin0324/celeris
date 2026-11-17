//! Discord への送り出し。通常の送信は CoS escalation / unavailable fallback のみ
//! （判定は CoS resolve / fallback が source revision ごとの outbox を作る）。
//!
//! 送信（`spawn_send` / `post_webhook`）— CoS の pending のみ Discord の webhook へ POST する。
//! tick をブロックしないよう `tokio::spawn` で送り、**結果は次の tick で** `notification_mark` する
//! （送信結果は `mpsc` でループへ戻る。ADR-0022 D2 の `check` と同じ形）。
//!
//! 秘密（webhook URL）の規律（ADR-0037 D3）: **URL はログにもエラー文にも応答にも出さない**。
//! `reqwest::Error` の `Display` は URL を含むので、そのまま文字列にしてはいけない（`safe_error` を使う）。
//! 送信は 1 tick に最大 1 通、429 は attempts に数えず `Retry-After` の間だけ待つ。

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use task_core::notify::{MAX_NOTIFY_ATTEMPTS, NotificationId, NotificationKind};
use task_core::{Notification, SqliteStore, StoreError};
use time::OffsetDateTime;

pub mod triage;

/// `[notify] interval_secs` の既定（ADR-0037 D3）。
pub const DEFAULT_INTERVAL_SECS: u64 = 30;
/// webhook への POST のタイムアウト（ADR-0037 D3）。
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Discord の 1 通の上限（2000 字）より少し手前で切る。
pub const CONTENT_MAX_CHARS: usize = 1900;
/// 送信の本文に付ける名前（ADR-0037 D3）。
const WEBHOOK_USERNAME: &str = "Celeris";
/// `Retry-After` が読めないときに待つ既定の秒数（ADR-0037 D3。実機 2026-09-18: 8 件一斉送信で 429）。
pub const DEFAULT_RETRY_AFTER_SECS: u64 = 5;

/// `[notify]`（ADR-0037 D2 / D3、ADR-0133 D6）。秘密の id と二経路の間隔を持つ。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NotifyConfig {
    /// `[secrets]` の中の webhook URL の id。無ければ何も送らない（エラーにしない）。
    #[serde(default = "default_webhook_secret")]
    pub discord_webhook_secret: String,
    /// 判定と送信を行う間隔（秒）。
    #[serde(default = "default_interval_secs")]
    pub interval_secs: u64,
    /// 例: `"http://192.168.1.103:7720"`。無ければ文面にリンクを入れない。
    #[serde(default)]
    pub web_base_url: Option<String>,
    /// 新しい受信箱項目を同じ一通に束ねる猶予。
    #[serde(default = "default_inbox_batch_secs")]
    pub inbox_batch_secs: u64,
    /// 未回答の項目を一度だけ再通知するまでの秒数。
    #[serde(default = "default_inbox_reminder_secs")]
    pub inbox_reminder_secs: u64,
    /// 通知の要約間隔。ゼロなら外部へ送らない。
    #[serde(default = "default_digest_interval_secs")]
    pub digest_interval_secs: u64,
    /// 要約に載せる種類の最大行数。
    #[serde(default = "default_digest_max_lines")]
    pub digest_max_lines: usize,
}

fn default_inbox_batch_secs() -> u64 {
    60
}
fn default_inbox_reminder_secs() -> u64 {
    86_400
}
fn default_digest_interval_secs() -> u64 {
    3_600
}
fn default_digest_max_lines() -> usize {
    10
}

fn default_webhook_secret() -> String {
    task_core::notify::DEFAULT_WEBHOOK_SECRET_ID.to_string()
}

fn default_interval_secs() -> u64 {
    DEFAULT_INTERVAL_SECS
}

impl Default for NotifyConfig {
    fn default() -> Self {
        Self {
            discord_webhook_secret: default_webhook_secret(),
            interval_secs: default_interval_secs(),
            web_base_url: None,
            inbox_batch_secs: default_inbox_batch_secs(),
            inbox_reminder_secs: default_inbox_reminder_secs(),
            digest_interval_secs: default_digest_interval_secs(),
            digest_max_lines: default_digest_max_lines(),
        }
    }
}

impl NotifyConfig {
    /// 末尾の `/` を落とした GUI の根（空文字列は「無い」とみなす）。
    pub fn base_url(&self) -> Option<&str> {
        self.web_base_url
            .as_deref()
            .map(|u| u.trim_end_matches('/'))
            .filter(|u| !u.is_empty())
    }
}

/// 送信の結果（spawn した先から tick ループへ戻る）。`ids` は 1 件のことも、
/// `bad_news` を束ねた複数件のこともある（ADR-0037 D4）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendResult {
    pub ids: Vec<NotificationId>,
    pub outcome: SendOutcome,
}

/// 1 回の POST の結末。**429 は失敗ではない**（ADR-0037 D3 / 実機 2026-09-18）: attempts に数えず、
/// `retry_after` の間だけ次の送信を控える。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    /// 送れた。
    Sent,
    /// Discord の 1 秒あたりの上限に当たった。この時間が経つまで次を送らない。
    RateLimited(Duration),
    /// 送れなかった（**URL・ホスト名は含まない**短い理由）。
    Failed(String),
}

// ---- 文面の部品（決定的。LLM は呼ばない）----

/// 文字数で切って `…` を付ける（バイトではない）。
fn excerpt(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let head: String = flat.chars().take(max).collect();
    format!("{head}…")
}

/// Discord の 1 通に収める。
fn clamp_content(text: &str) -> String {
    if text.chars().count() <= CONTENT_MAX_CHARS {
        return text.to_string();
    }
    text.chars().take(CONTENT_MAX_CHARS).collect()
}

// ---- 送り先の選択（CoS の outbox と root の完了の要約だけ）----

/// Only the two CoS outbox kinds and the root-done summary may reach the webhook.
pub fn select_routes_batch(pending: &[Notification]) -> Option<SendBatch> {
    pending
        .iter()
        .find(|n| {
            matches!(
                n.kind,
                NotificationKind::CosEscalation
                    | NotificationKind::CosFallback
                    | NotificationKind::RootDoneSummary
            )
        })
        .map(|n| SendBatch {
            ids: vec![n.id],
            content: n.body.clone(),
        })
}

/// 1 通に束ねた送信の材料。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendBatch {
    pub ids: Vec<NotificationId>,
    pub content: String,
}

// ---- 送信（ADR-0037 D3）----

/// ADR-0030 D1 の流儀で `[secrets] dir` から webhook の URL を読む。無ければ `None`
/// （**値もパスもログに出さない**。秘密が無いのは異常ではないので警告も出さない）。
pub fn webhook_url(secrets_dir: Option<&Path>, secret_id: &str) -> Option<String> {
    let dir = secrets_dir?;
    let text = std::fs::read_to_string(dir.join(secret_id)).ok()?;
    let value = text.trim_end_matches(['\n', '\r']).trim().to_string();
    if value.is_empty() { None } else { Some(value) }
}

/// `reqwest::Error` の `Display` は URL を含むので使わない。種別だけの短い文にする。
fn safe_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timed out".to_string()
    } else if e.is_connect() {
        "could not connect".to_string()
    } else if e.is_body() || e.is_decode() {
        "bad response body".to_string()
    } else if e.is_request() {
        "invalid request".to_string()
    } else {
        "request failed".to_string()
    }
}

/// `Retry-After` ヘッダ（秒。小数もある）か、無ければ Discord の JSON 本文の `retry_after`
/// （秒。ミリ秒ではない）を読む。どちらも無ければ `DEFAULT_RETRY_AFTER_SECS`。
async fn read_retry_after(response: reqwest::Response) -> Duration {
    let from_header = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.trim().parse::<f64>().ok());
    if let Some(secs) = from_header {
        return Duration::from_secs_f64(secs.max(0.0));
    }
    let body = response.text().await.unwrap_or_default();
    let from_body = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("retry_after").and_then(|r| r.as_f64()));
    match from_body {
        Some(secs) => Duration::from_secs_f64(secs.max(0.0)),
        None => Duration::from_secs(DEFAULT_RETRY_AFTER_SECS),
    }
}

/// webhook の URL へ 1 通 POST する。返すのは**種別だけ**の短いエラー（URL・ホスト名は入らない）。
/// 429 は `SendOutcome::RateLimited`（失敗ではない。ADR-0037 D3）。
pub async fn post_webhook(client: &reqwest::Client, url: &str, content: &str) -> SendOutcome {
    let payload = serde_json::json!({
        "content": clamp_content(content),
        "username": WEBHOOK_USERNAME,
        "allowed_mentions": { "parse": [] },
    });
    let response = match client
        .post(url)
        .json(&payload)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return SendOutcome::Failed(safe_error(&e)),
    };
    let status = response.status();
    if status.is_success() {
        SendOutcome::Sent
    } else if status.as_u16() == 429 {
        SendOutcome::RateLimited(read_retry_after(response).await)
    } else {
        SendOutcome::Failed(format!("http status {}", status.as_u16()))
    }
}

/// tick をブロックせずに 1 通送る（B1。`bad_news` の束ねなら複数 id）。結果は `tx` に流れ、
/// **次の tick** で `notification_mark` される。
pub fn spawn_send(
    client: reqwest::Client,
    url: String,
    batch: &SendBatch,
    tx: tokio::sync::mpsc::Sender<SendResult>,
) {
    let ids = batch.ids.clone();
    let content = batch.content.clone();
    tokio::spawn(async move {
        let outcome = post_webhook(&client, &url, &content).await;
        let _ = tx.send(SendResult { ids, outcome }).await;
    });
}

/// 送信の結果を台帳に書く（`tick_loop` が次の tick の先頭で呼ぶ）。3 回目の失敗で諦める（`ok = false`）。
/// `RateLimited` は台帳に触れない（**attempts に数えない**。次の tick でそのまま再挑戦する）。
pub fn record(
    store: &SqliteStore,
    pending: &[Notification],
    result: &SendResult,
    now: OffsetDateTime,
) -> Result<(), StoreError> {
    match &result.outcome {
        SendOutcome::Sent => {
            for id in &result.ids {
                store.notification_mark(*id, Some(true), None, now)?;
            }
        }
        SendOutcome::RateLimited(_) => {}
        SendOutcome::Failed(error) => {
            for id in &result.ids {
                // 直前に読んだ pending の `attempts` で「これが最後の試行か」を決める（決定的）。
                let attempts_before = pending
                    .iter()
                    .find(|n| n.id == *id)
                    .map(|n| n.attempts)
                    .unwrap_or(0);
                let give_up = attempts_before + 1 >= MAX_NOTIFY_ATTEMPTS;
                store.notification_mark(*id, None, Some(error), now)?;
                if give_up {
                    let reason = format!("gave up after {MAX_NOTIFY_ATTEMPTS} attempts: {error}");
                    store.notification_mark(*id, Some(false), Some(&reason), now)?;
                }
            }
        }
    }
    Ok(())
}

/// `POST /notify/test` で送る定型文（ADR-0037 D4）。
pub const TEST_CONTENT: &str =
    "celeris のテスト送信です。ここに「人の判断が要るとき」だけ通知が届きます。";

/// `POST /notify/test` の結果（ADR-0037 D4）。**どの枝にも URL・ホスト名は入らない**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestSend {
    /// 送れた。
    Sent,
    /// 送り先には届いたが失敗した（種別だけの短い理由）。
    Failed(String),
    /// 秘密が無い・HTTP クライアントが無い（API は 409 `notify_unavailable`）。
    NotConfigured(String),
}

/// テスト送信 1 回（`AdminRequest::NotifyTest` の中身）。秘密を読むのも POST するのも celeris 側。
pub async fn send_test(
    client: Option<&reqwest::Client>,
    secrets_dir: Option<&Path>,
    secret_id: &str,
) -> TestSend {
    let Some(client) = client else {
        return TestSend::NotConfigured(
            "the HTTP client could not be created, so nothing can be sent".to_string(),
        );
    };
    let Some(url) = webhook_url(secrets_dir, secret_id) else {
        return TestSend::NotConfigured(format!(
            "no Discord webhook is registered; add it as the secret `{secret_id}`"
        ));
    };
    match post_webhook(client, &url, TEST_CONTENT).await {
        SendOutcome::Sent => TestSend::Sent,
        SendOutcome::RateLimited(_) => TestSend::Failed("rate limited".to_string()),
        SendOutcome::Failed(detail) => TestSend::Failed(detail),
    }
}

/// 秘密が無いときに pending を畳む理由（GUI の「直近の送信」に出る。URL は含まない）。
pub const NOT_CONFIGURED: &str = "discord webhook is not configured";

/// 秘密が無い間は**送らず、pending も溜めない**（ADR-0037 D2）。判定はしたが送れなかったことを
/// 台帳に残して畳む（後から秘密を登録しても、その間の出来事は蒸し返さない）。
pub fn discard_pending(
    store: &SqliteStore,
    pending: &[Notification],
    now: OffsetDateTime,
) -> Result<usize, StoreError> {
    let mut discarded = 0;
    for row in pending {
        if store.notification_mark(row.id, Some(false), Some(NOT_CONFIGURED), now)? {
            discarded += 1;
        }
    }
    Ok(discarded)
}

/// 10 秒のタイムアウトを持つ HTTP クライアント（rustls。ADR-0037 D3）。組み立てに失敗したら
/// `None`（通知だけが止まり、celeris は動き続ける）。
pub fn client() -> Option<reqwest::Client> {
    match reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(concat!("celeris/", env!("CARGO_PKG_VERSION")))
        .build()
    {
        Ok(client) => Some(client),
        Err(e) => {
            tracing::warn!(error = %e, "notify: cannot build the HTTP client; notifications are disabled");
            None
        }
    }
}

#[cfg(test)]
#[path = "notify/tests.rs"]
mod tests;
