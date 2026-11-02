//! ScriptRuntime の誤りの型（境界 ADR 2026-11-02 (c) の値の写像「エラー」）。

use serde::Serialize;

/// actor への要求が失敗した理由。TS の `HostError { code, message }` はここへ写す。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScriptError {
    /// script が例外を投げた、または Promise が reject された。
    #[error("script exception: {message}")]
    Exception { message: String },
    /// 要求の deadline を過ぎた。呼び出し側は待つのをやめ、actor は後から来た結果を捨てる。
    #[error("script call timed out")]
    Timeout,
    /// 要求の queue が満杯。待たずに返す（backpressure）。
    #[error("script runtime is busy (request queue full)")]
    Busy,
    /// script の返した値が契約に合わない（`undefined`・JSON にできない値など）。
    #[error("invalid script output: {reason}")]
    InvalidOutput { reason: String },
    /// capability・scope の検査で拒否した。
    #[error("denied: {reason}")]
    Denied { reason: String },
    /// actor が止まった（shutdown・thread の異常終了・起動失敗）。
    #[error("script runtime terminated")]
    Terminated,
    /// 要求を固定した generation が既に破棄された。
    #[error("script generation is gone")]
    GenerationGone,
}
