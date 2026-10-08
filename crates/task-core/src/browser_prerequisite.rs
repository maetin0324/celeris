//! ADR 2026-10-08-browser-prod-enablement D1.4 / D2: browser 実行の前提（適合台帳）の判定結果。
//!
//! 判定そのもの（file を読む・版を比べる）は `task_worker::browser::ledger_status` にある。ここは
//! dispatcher・event・受信箱が共有する固定の code と人向けの文だけを持つ（path・秘密は載せない）。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `Trigger::BrowserPrereqBlock` の `Transitioned.reason`。
pub const REASON_BLOCKED: &str = "browser_prerequisite";
/// `Trigger::BrowserPrereqResume` の `Transitioned.reason`。
pub const REASON_RESOLVED: &str = "browser_prerequisite_resolved";

/// 台帳の状態（D1.4 の表）。`Ok` 以外は browser task を dispatch の前で止める。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserPrerequisiteCode {
    Ok,
    Missing,
    Invalid,
    StaleRelease,
    StaleAgentBrowser,
    AgentBrowserMissing,
    NoConformantBackend,
    LedgerLacksCredential,
    BrowserPolicyMissing,
}

impl BrowserPrerequisiteCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Missing => "missing",
            Self::Invalid => "invalid",
            Self::StaleRelease => "stale_release",
            Self::StaleAgentBrowser => "stale_agent_browser",
            Self::AgentBrowserMissing => "agent_browser_missing",
            Self::NoConformantBackend => "no_conformant_backend",
            Self::LedgerLacksCredential => "ledger_lacks_credential",
            Self::BrowserPolicyMissing => "browser_policy_missing",
        }
    }

    /// 人に出す文（D1.4 の表の右列）。受信箱・task 詳細・event の `message` に入る。
    pub fn message(self) -> &'static str {
        match self {
            Self::Ok => "browser の適合台帳: 有効",
            Self::Missing => {
                "browser の適合台帳が未配置。新しい release を作るか、docs/ops/browser-prod.md の手順で台帳を作る"
            }
            Self::Invalid => {
                "browser の適合台帳が壊れている。docs/ops/browser-prod.md の手順で台帳を作り直す"
            }
            Self::StaleRelease => {
                "browser の適合台帳が古い（別の release 用）。新しい release を作るか台帳を作り直す"
            }
            Self::StaleAgentBrowser => {
                "browser の適合台帳が古い（agent-browser の版が違う）。台帳を作り直す"
            }
            Self::AgentBrowserMissing => "agent-browser が見つからない",
            Self::NoConformantBackend => "browser の適合台帳に使える backend が無い",
            Self::LedgerLacksCredential => "browser の適合台帳に credential 注入の証拠が無い",
            Self::BrowserPolicyMissing => {
                "browser の task policy が無い。task 詳細で接続先（origin）を入れる"
            }
        }
    }

    pub fn is_ok(self) -> bool {
        self == Self::Ok
    }

    /// worker が返した adapter の失敗文が台帳由来なら、その code（D2: infra_requeue に回さない）。
    pub fn from_worker_error(message: &str) -> Option<Self> {
        if message.contains("browser conformance record unavailable") {
            Some(Self::Missing)
        } else if message.contains("browser conformance record invalid") {
            Some(Self::Invalid)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_ledger_gate_codes_round_trip_and_map_worker_errors() {
        for code in [
            BrowserPrerequisiteCode::Ok,
            BrowserPrerequisiteCode::Missing,
            BrowserPrerequisiteCode::Invalid,
            BrowserPrerequisiteCode::StaleRelease,
            BrowserPrerequisiteCode::StaleAgentBrowser,
            BrowserPrerequisiteCode::AgentBrowserMissing,
            BrowserPrerequisiteCode::NoConformantBackend,
            BrowserPrerequisiteCode::LedgerLacksCredential,
            BrowserPrerequisiteCode::BrowserPolicyMissing,
        ] {
            let json = serde_json::to_string(&code).unwrap();
            assert_eq!(json, format!("\"{}\"", code.as_str()));
            assert!(!code.message().is_empty());
        }
        assert_eq!(
            BrowserPrerequisiteCode::from_worker_error(
                "adapter: browser conformance record unavailable"
            ),
            Some(BrowserPrerequisiteCode::Missing)
        );
        assert_eq!(
            BrowserPrerequisiteCode::from_worker_error("browser conformance record invalid"),
            Some(BrowserPrerequisiteCode::Invalid)
        );
        assert_eq!(BrowserPrerequisiteCode::from_worker_error("io error"), None);
    }
}
