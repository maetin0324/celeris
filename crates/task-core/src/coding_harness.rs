//! ADR 2026-10-07（coding harness の既定）: coding タスクの既定 adapter を model family で決める。
//!
//! 明示 adapter があればそれを返す。無ければ Claude 系 → `claude-code`、それ以外（未知を含む）→ `pi`。
//! adapter id 文字列の正本はここと `AccountAdapter` に置く（task-worker の `PiAdapter::ID` は
//! [`CodingHarness::Pi`] の [`CodingHarness::adapter_id`] と同じ値であることを試験で固定する）。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::accounts::AccountAdapter;
use crate::model_family::ModelFamily;

/// coding の既定ハーネス。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CodingHarness {
    ClaudeCode,
    /// Pi（pi coding agent）+ Hashline の軽量 worker。
    Pi,
}

impl CodingHarness {
    /// Pi adapter の id。
    pub const PI_ADAPTER_ID: &'static str = "pi";

    /// family の既定ハーネス。Claude だけが Claude Code、他はすべて Pi。
    pub fn for_family(family: ModelFamily) -> CodingHarness {
        if family.is_claude() {
            CodingHarness::ClaudeCode
        } else {
            CodingHarness::Pi
        }
    }

    /// worker の adapter id（設定の `adapter` と同じ文字列）。
    pub fn adapter_id(self) -> &'static str {
        match self {
            CodingHarness::ClaudeCode => AccountAdapter::ClaudeCode.as_str(),
            CodingHarness::Pi => Self::PI_ADAPTER_ID,
        }
    }
}

/// harness の adapter の選び方（`[[harnesses]] adapter_policy`）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdapterPolicy {
    /// 選ばれた provider 行の adapter（従来の挙動）。
    #[default]
    ProviderOrder,
    /// model family の既定ハーネスに一致する行を優先する（この ADR）。
    ModelFamily,
}

impl AdapterPolicy {
    /// 既定（従来どおり）か。serde の `skip_serializing_if` 用。
    pub fn is_provider_order(&self) -> bool {
        *self == AdapterPolicy::ProviderOrder
    }
}

/// adapter をどう決めたか（routing audit に残す）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum AdapterChoice {
    /// task・役割で明示された adapter。
    Explicit,
    /// family の既定ハーネスに一致する行。
    Preferred,
    /// 既定ハーネスの行が使えず、絞る前の候補へ倒れた。
    Fallback { reason: String },
    /// `AdapterPolicy::ProviderOrder`（従来どおり）。
    ProviderOrder,
}

/// coding タスクの adapter を決める純関数。明示 adapter があればそのまま返し、
/// 無ければ family の既定ハーネスの adapter id を返す。
pub fn coding_harness_default_adapter(explicit: Option<&str>, family: ModelFamily) -> &str {
    match explicit {
        Some(adapter) => adapter,
        None => CodingHarness::for_family(family).adapter_id(),
    }
}

/// provider 行の adapter `row_adapter` が、その行の family の既定ハーネスに一致するか。
pub fn prefers(row_adapter: &str, family: ModelFamily) -> bool {
    row_adapter == CodingHarness::for_family(family).adapter_id()
}

#[cfg(test)]
mod tests;
