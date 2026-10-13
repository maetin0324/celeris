//! 供給側の結果（ADR-0010 D5、ADR 2026-10-11-llm-source-harness-selection）。失敗は AccountBook の
//! `(source_id, account_id)` cooldown に記録する。

use std::time::Duration;

pub type AdapterId = String;
pub type ProviderId = String;

/// run の供給側の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderOutcome {
    Ok,
    Throttled { retry_after: Duration },
    AuthFailed,
    Exhausted,
}
