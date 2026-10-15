use thiserror::Error;

/// 呼び出し側が処理すべき失敗。API key の値は持たない（secret id のみ）。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum JevError {
    /// 401/403。retry も breaker 計数もしない。credential 待ちに接続する。
    #[error("jev authentication failed (status {status}, secret_id {secret_id})")]
    Auth { status: u16, secret_id: String },
    /// provider が鍵を持っていない（未登録）。credential 待ちの入口。
    #[error("jev api key not registered (secret_id {secret_id})")]
    KeyMissing { secret_id: String },
    #[error("invalid jev config: {0}")]
    Config(String),
}
