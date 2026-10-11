//! ADR 2026-10-07（coding harness の既定）: `adapter_policy = "model_family"` のハーネスで、
//! `worker_hint.adapter` の無い run の provider 行を「行の model family の既定ハーネス」
//! （Claude → `claude-code`、それ以外・未知 → `pi`）に一致するものへ絞って選ぶ。
//!
//! 絞った候補（preferred）の中の順序（sticky → cheap local → pool score → 設定順）は
//! `select_provider_excluding` のまま変えない。preferred が無い・全滅なら、絞る前の候補で従来の
//! 選択をやり直す（fallback。理由は `LaneResolution.adapter_choice` に残す）。明示 adapter・
//! CoS・planner の run は対象外。

use std::collections::HashMap;

use task_core::LlmSourceRef;

use super::*;

/// 既定解決の設定（daemon が config から作り、`Dispatcher::set_coding_harness_default` で渡す）。
#[derive(Debug, Clone, Default)]
pub struct CodingHarnessDefault {
    /// `adapter_policy = "model_family"` のハーネス id（task の `genre`）。空なら既定解決はしない。
    pub harnesses: Vec<String>,
    /// provider id → 実効の LLM source（`Config::provider_llm_source`）。family 導出の 1 番目の材料。
    pub sources: HashMap<ProviderId, LlmSourceRef>,
    /// routing catalog の model id → `ModelProfile.family`（自由文字列。読む側が `ModelFamily::parse`）。
    pub model_families: HashMap<String, String>,
}

impl Dispatcher {
    /// ADR 2026-10-07: 既定解決の設定を差し替える（起動時と設定の再読込で呼ぶ）。
    pub fn set_coding_harness_default(&mut self, default: CodingHarnessDefault) {
        self.coding_default = default;
    }

    /// catalog の family（無ければ空文字）。`legacy_provider_profiles` の `ModelProfile.family` に入れる。
    pub(super) fn catalog_family(&self, model_id: &str) -> String {
        self.coding_default
            .model_families
            .get(model_id)
            .cloned()
            .unwrap_or_default()
    }

}
