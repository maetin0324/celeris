//! Legacy model routing helpers retained for old event interpretation and wire-name normalization.

use schemars::JsonSchema;
use serde::Serialize;

use super::CatalogSource;
use crate::Tier;

/// `Tier` の表・API 上の文字列（`frontier` / `standard` / `cheap`）。
pub fn tier_str(tier: Tier) -> &'static str {
    match tier {
        Tier::Frontier => "frontier",
        Tier::Standard => "standard",
        Tier::Cheap => "cheap",
    }
}

/// `tier_str` の逆。知らない語は `None`。
pub fn tier_from_str(s: &str) -> Option<Tier> {
    match s {
        "frontier" => Some(Tier::Frontier),
        "standard" => Some(Tier::Standard),
        "cheap" => Some(Tier::Cheap),
        _ => None,
    }
}

/// 割り当ての実効の状態。`Excluded` は割り当て行は残るが routing には使えない（D2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub enum AssignmentState {
    Assigned,
    /// `reason` は `override:disabled`（先に見る）か `catalog:unavailable`。
    Excluded {
        reason: &'static str,
    },
}

impl AssignmentState {
    pub const REASON_DISABLED: &'static str = "override:disabled";
    pub const REASON_UNAVAILABLE: &'static str = "catalog:unavailable";
}

/// 割り当て行に実効の状態を足したもの。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct EffectiveAssignment {
    pub source: CatalogSource,
    pub tier: Tier,
    pub model_id: String,
    /// Lower values are preferred; ties use source and model ID.
    #[serde(default)]
    pub priority: u32,
    pub state: AssignmentState,
    pub note: Option<String>,
    pub updated_at: i64,
    pub updated_by: String,
}

/// 全割り当ての実効の一覧（priority、source、model_id、tier の順）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, JsonSchema)]
pub struct AssignmentView {
    pub items: Vec<EffectiveAssignment>,
    /// Explicitly managed scopes, including roles with no remaining members.
    #[serde(default)]
    pub managed: Vec<(CatalogSource, Tier)>,
}

impl AssignmentView {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn manages(&self, source: &str, tier: Tier) -> bool {
        self.managed
            .iter()
            .any(|(s, t)| s.as_str() == source && *t == tier)
            || self
                .items
                .iter()
                .any(|a| a.source.as_str() == source && a.tier == tier)
    }

    /// All members in deterministic priority order, including excluded members for display.
    pub fn members(&self, source: &str, tier: Tier) -> Vec<&EffectiveAssignment> {
        let mut items: Vec<_> = self
            .items
            .iter()
            .filter(|a| a.source.as_str() == source && a.tier == tier)
            .collect();
        items.sort_by_key(|a| (a.priority, a.source.as_str(), &a.model_id));
        items
    }

    /// Legacy selection skips unavailable members before trying the next priority.
    pub fn get(&self, source: &str, tier: Tier) -> Option<&EffectiveAssignment> {
        let members = self.members(source, tier);
        members
            .iter()
            .copied()
            .find(|a| a.state == AssignmentState::Assigned)
            .or_else(|| members.first().copied())
    }
}

/// LLM source から catalog の source 名を求める。catalog を持たない source は `None`。
pub fn source_name_for_llm_source(source: &crate::LlmSourceRef) -> Option<String> {
    match source {
        crate::LlmSourceRef::ClaudeOauth => Some(CatalogSource::CLAUDE_OAUTH.to_string()),
        crate::LlmSourceRef::CodexOauth => Some(CatalogSource::CODEX_OAUTH.to_string()),
        crate::LlmSourceRef::OpencodeGo => Some(CatalogSource::OPENCODE_GO.to_string()),
        crate::LlmSourceRef::OpenaiCompatible(id) => Some(CatalogSource::openai_compatible(id).0),
        _ => None,
    }
}

/// llm-proxy が `WireRule.adapter` に渡す名前（proxy は上流へ接頭辞なしの model_id を送る）。
pub const LLM_PROXY_ADAPTER: &str = "llm-proxy";

/// `provider/<id>` の形のモデル名を要る harness（ACP = opencode の provider 名、Pi = Pi の provider 名）。
pub fn adapter_takes_provider_prefix(adapter: &str) -> bool {
    matches!(adapter, "acp" | "pi")
}

/// config に書かれた wire の `<prefix>/`（先頭の `/` まで。`qwen-local/qwen3.8-27b` → `qwen-local/`）。
/// `/` が無い・先頭が `/` なら `None`。
pub fn configured_prefix(wire: &str) -> Option<&str> {
    let end = wire.find('/')?;
    (end > 0).then(|| &wire[..=end])
}

/// source × adapter から決める既定の接頭辞の表（行の config に接頭辞が無いとき）。opencode go の ACP / Pi 行は
/// `opencode-go/<model>`（opencode・Pi の provider 名が `opencode-go`）。それ以外（claude-code / codex の CLI、
/// llm-proxy、self-host の ACP / Pi 行で config が接頭辞なし）は接頭辞なし。
pub fn default_wire_prefix(source: &str, adapter: &str) -> Option<&'static str> {
    (adapter_takes_provider_prefix(adapter) && source == CatalogSource::OPENCODE_GO)
        .then_some("opencode-go/")
}

/// ADR 2026-10-06 model-role-assignments 付記（2026-10-07 wire-prefix）: 割り当ての `model_id`（catalog の素の id）
/// から provider 行ごとの実行用モデル名（wire）を作る規則。llm-proxy・route candidate resolution・API preview が
/// 同じ規則を使う。
///
/// 1. 行の config の wire（lane の binding → 行の `model`）に `<prefix>/` があればそれを引き継ぐ
///    （行が既に使っている形が正。opencode の provider 名は `OPENCODE_CONFIG` の中にあり celeris は知らない）。
/// 2. 無ければ `default_wire_prefix(source, adapter)` の表。
/// 3. `model_id` が既にその接頭辞で始まっていれば重ねない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireRule<'a> {
    /// catalog の source 名（`opencode-go` / `openai-compatible:<id>` など）。
    pub source: &'a str,
    /// 行の adapter（`acp` / `pi` / `claude-code` / `codex` / `LLM_PROXY_ADAPTER`）。
    pub adapter: &'a str,
    /// 行の `model`（lane の binding が無いときの config の wire）。
    pub row_model: Option<&'a str>,
}

impl<'a> WireRule<'a> {
    pub fn new(source: &'a str, adapter: &'a str, row_model: Option<&'a str>) -> Self {
        Self {
            source,
            adapter,
            row_model,
        }
    }

    /// 接頭辞なしで解決する rule（proxy の lane・adapter が分からない deployment）。
    pub fn proxy(source: &'a str) -> Self {
        Self::new(source, LLM_PROXY_ADAPTER, None)
    }

    /// この lane の config の wire（`lane_wire` → `row_model`）。
    fn configured(&self, lane_wire: Option<&'a str>) -> Option<&'a str> {
        lane_wire
            .filter(|w| !w.is_empty())
            .or(self.row_model.filter(|w| !w.is_empty()))
    }

    /// この lane の接頭辞（`<prefix>/`）。
    pub fn prefix(&self, lane_wire: Option<&'a str>) -> Option<&'a str> {
        self.configured(lane_wire)
            .and_then(configured_prefix)
            .or_else(|| default_wire_prefix(self.source, self.adapter))
    }

    /// 割り当ての `model_id` から実行用のモデル名。
    pub fn model(&self, lane_wire: Option<&'a str>, model_id: &str) -> String {
        match self.prefix(lane_wire) {
            Some(prefix) if !model_id.starts_with(prefix) => format!("{prefix}{model_id}"),
            _ => model_id.to_string(),
        }
    }

    /// config の wire から catalog の `model_id`（接頭辞を外したもの）。`<prefix>/` の無い wire はそのまま。
    pub fn catalog_model_id(wire: &str) -> &str {
        configured_prefix(wire).map_or(wire, |p| &wire[p.len()..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wire_rule_inherits_the_row_prefix_and_falls_back_to_the_table() {
        let qwen = "openai-compatible:qwen";
        // self-host の ACP 行: config の `qwen-local/qwen3.8-27b` から `qwen-local/` を引き継ぐ。
        let acp = WireRule::new(qwen, "acp", Some("qwen-local/qwen3.8-27b"));
        assert_eq!(acp.prefix(None), Some("qwen-local/"));
        assert_eq!(acp.model(None, "qwen3.8-27b"), "qwen-local/qwen3.8-27b");
        // Pi 行（`model = provider/id`）も同じ。
        let pi = WireRule::new(qwen, "pi", Some("qwen-local/qwen3.8-27b"));
        assert_eq!(pi.model(None, "qwen3.8-27b"), "qwen-local/qwen3.8-27b");
        // lane の binding が行の model より先。
        assert_eq!(
            acp.model(Some("other/qwen-old"), "qwen3.8-27b"),
            "other/qwen3.8-27b"
        );
        // 既に接頭辞付きの model_id は重ねない。
        assert_eq!(
            acp.model(None, "qwen-local/qwen3.8-27b"),
            "qwen-local/qwen3.8-27b"
        );
        // config が接頭辞なしの self-host 行は接頭辞なし（行が使っている形が正）。
        assert_eq!(
            WireRule::new(qwen, "acp", Some("qwen3")).model(None, "qwen3.8-27b"),
            "qwen3.8-27b"
        );
        // llm-proxy は常に接頭辞なし（config の wire に `/` が無い）。
        assert_eq!(
            WireRule::proxy(qwen).model(Some("qwen3"), "qwen3.8-27b"),
            "qwen3.8-27b"
        );
        // opencode go: ACP / Pi は表、claude-code / codex の CLI は接頭辞なし。
        assert_eq!(
            WireRule::new("opencode-go", "acp", None).model(None, "glm-5"),
            "opencode-go/glm-5"
        );
        assert_eq!(
            WireRule::new("opencode-go", "pi", None).model(None, "glm-5"),
            "opencode-go/glm-5"
        );
        assert_eq!(
            WireRule::new("opencode-go", "claude-code", None).model(None, "glm-5"),
            "glm-5"
        );
        assert_eq!(
            WireRule::new("claude-oauth", "claude-code", Some("claude-x")).model(None, "claude-y"),
            "claude-y"
        );
        assert_eq!(configured_prefix("/x"), None);
        assert_eq!(configured_prefix("x"), None);
        assert_eq!(
            WireRule::catalog_model_id("qwen-local/qwen3.8-27b"),
            "qwen3.8-27b"
        );
        assert_eq!(WireRule::catalog_model_id("qwen3.8-27b"), "qwen3.8-27b");
    }

    #[test]
    fn source_names_follow_the_catalog() {
        use crate::LlmSourceRef as L;
        assert_eq!(
            source_name_for_llm_source(&L::ClaudeOauth).as_deref(),
            Some("claude-oauth")
        );
        assert_eq!(
            source_name_for_llm_source(&L::CodexOauth).as_deref(),
            Some("codex-oauth")
        );
        assert_eq!(
            source_name_for_llm_source(&L::OpencodeGo).as_deref(),
            Some("opencode-go")
        );
        assert_eq!(
            source_name_for_llm_source(&L::OpenaiCompatible("qwen".into())).as_deref(),
            Some("openai-compatible:qwen")
        );
        assert_eq!(source_name_for_llm_source(&L::Celeris), None);
        assert_eq!(source_name_for_llm_source(&L::None), None);
        assert_eq!(source_name_for_llm_source(&L::Unknown), None);
    }
}
