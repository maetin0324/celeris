//! Jev（typesafe decision model）による要求分類 adapter。
//!
//! 設計は `agent-docs/adr/2026-10-15-mlsys-min-decision-assisted.md`（D1/D2/D4）。
//! dispatcher・store からは呼ばない。分類段が使い、失敗時は `FallbackToFixed` で fixed 方策へ戻る。
//! 401/403 だけは credential 待ち（D4）へ接続する型付き `JevError::Auth` で返す。

mod breaker;
mod cache;
mod client;
mod config;
mod error;
mod input;
mod question;
mod time;

pub use breaker::CircuitBreaker;
pub use client::{ApiKey, ApiKeyProvider, ClassifyOutcome, FallbackReason, JevClassifier};
pub use config::JevConfig;
pub use error::JevError;
pub use input::{ClassifyInput, redact_secrets};
pub use question::{Answer, Classification, FailureKind, Specificity, WorkNature};
pub use time::{Clock, Sleeper, SystemClock, TokioSleeper};
