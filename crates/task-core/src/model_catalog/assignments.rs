//! ADR 2026-10-06 model-role-assignments 付記: モデルごとの役割 membership・優先度と、
//! その実効の解決（純粋関数）。
//!
//! 表は `model_role_assignments`（migration 0053 → 0054）。人が決めた割り当てで、catalog の自動更新は消さない。
//! 実効の状態（`AssignmentState`）は catalog の `available` と上書きの `disabled` から都度求める。
//! dispatcher・llm-proxy・routing catalog の 3 か所は同じ `RoleAssignmentReader` を見て、同じ
//! `apply_to_bindings` / `AssignmentView::get` で解決する。I/O も LLM 呼び出しも持たない。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{CatalogEntry, CatalogOverrideRow, CatalogSource};
use crate::Tier;
use crate::model_routing::{ModelBinding, TierModels};

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

/// 表の 1 行（D1）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RoleAssignment {
    pub source: CatalogSource,
    pub tier: Tier,
    pub model_id: String,
    /// Lower values are preferred; ties use source and model ID.
    #[serde(default)]
    pub priority: u32,
    pub note: Option<String>,
    pub updated_at: i64,
    pub updated_by: String,
}

/// One model's membership in a role. Priority is global within that role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoleMember {
    pub source: CatalogSource,
    pub model_id: String,
    pub priority: u32,
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

    /// store の 3 表から組む（純粋）。上書きの `disabled` を先に、次に catalog の `available = 0` を見る。
    /// catalog に行が無いモデルは `Assigned` のまま（書き込み時に API が未知のモデルを弾くので、
    /// 行が消えたことで黙って除外しない）。
    pub fn build(
        assignments: &[RoleAssignment],
        entries: &[CatalogEntry],
        overrides: &[CatalogOverrideRow],
    ) -> Self {
        let mut items: Vec<EffectiveAssignment> = assignments
            .iter()
            .map(|a| {
                let disabled = overrides
                    .iter()
                    .any(|o| o.source == a.source && o.model_id == a.model_id && o.value.disabled);
                let unavailable = entries
                    .iter()
                    .any(|e| e.source == a.source && e.model_id == a.model_id && !e.available);
                let state = if disabled {
                    AssignmentState::Excluded {
                        reason: AssignmentState::REASON_DISABLED,
                    }
                } else if unavailable {
                    AssignmentState::Excluded {
                        reason: AssignmentState::REASON_UNAVAILABLE,
                    }
                } else {
                    AssignmentState::Assigned
                };
                EffectiveAssignment {
                    source: a.source.clone(),
                    tier: a.tier,
                    model_id: a.model_id.clone(),
                    priority: a.priority,
                    state,
                    note: a.note.clone(),
                    updated_at: a.updated_at,
                    updated_by: a.updated_by.clone(),
                }
            })
            .collect();
        items.sort_by(|a, b| {
            (a.priority, a.source.as_str(), &a.model_id, tier_str(a.tier)).cmp(&(
                b.priority,
                b.source.as_str(),
                &b.model_id,
                tier_str(b.tier),
            ))
        });
        Self {
            items,
            managed: Vec::new(),
        }
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

/// Legacy 用に各役割の先頭の利用可能モデルを bindings に重ねる。全候補は `members` から読む。
///
/// - `lanes` のうち `source` に割り当てがある lane は binding を置き換える。`Assigned` は
///   `name = model_id`・`model_id = Some(rule.model(<lane の config の wire>, model_id))`（付記 2026-10-07
///   wire-prefix: 行の config の `<prefix>/` を引き継ぐか、source × adapter の表で決める）・
///   `unavailable_reason = None`。既存 binding の `reasoning_effort` は引き継ぐ。`Excluded` は
///   `model_id = None`・`unavailable_reason = Some("assignment:<model_id> <reason>")`。
/// - 割り当てが無い lane は従来の binding のまま（移行の互換）。
/// - `bindings` が空で割り当ても 1 つも当たらなければ空のまま返す（legacy の挙動を変えない）。
/// - 割り当てが 1 つでも当たった結果、`resolve` は「bindings が非空で lane が無い」とエラーにする。
///   そこで `lanes` のうち binding も割り当てもない lane には
///   `{ name: <lane>, model_id: None, unavailable_reason: Some("assignment:none") }` を足し、
///   その lane はこの provider へ routing できない（`resolve` が `Err`）ことを明示する。
pub fn apply_to_bindings(
    bindings: &TierModels,
    source: &str,
    lanes: &[Tier],
    rule: WireRule<'_>,
    view: &AssignmentView,
) -> TierModels {
    let mut out = bindings.clone();
    let mut applied = false;
    for &lane in lanes {
        let Some(a) = view.get(source, lane) else {
            if view.manages(source, lane) {
                applied = true;
                out.insert(
                    lane,
                    ModelBinding {
                        name: tier_str(lane).into(),
                        model_id: None,
                        unavailable_reason: Some("assignment:none".into()),
                        reasoning_effort: None,
                    },
                );
            }
            continue;
        };
        applied = true;
        let binding = match a.state {
            AssignmentState::Assigned => ModelBinding {
                name: a.model_id.clone(),
                model_id: Some(rule.model(configured_wire(bindings.get(&lane)), &a.model_id)),
                unavailable_reason: None,
                reasoning_effort: bindings.get(&lane).and_then(|b| b.reasoning_effort.clone()),
            },
            AssignmentState::Excluded { reason } => ModelBinding {
                name: a.model_id.clone(),
                model_id: None,
                unavailable_reason: Some(format!("assignment:{} {reason}", a.model_id)),
                reasoning_effort: None,
            },
        };
        out.insert(lane, binding);
    }
    if applied {
        for &lane in lanes {
            out.entry(lane).or_insert_with(|| ModelBinding {
                name: tier_str(lane).to_string(),
                model_id: None,
                unavailable_reason: Some("assignment:none".to_string()),
                reasoning_effort: None,
            });
        }
    }
    out
}

/// config の lane binding が指す wire（`model_id`、無ければ `name`）。`unavailable_reason` のある binding
/// （`assignment:none` の仮の行・config で無効にした lane）は config の wire ではないので `None`。
pub fn configured_wire(binding: Option<&ModelBinding>) -> Option<&str> {
    let b = binding?;
    if b.unavailable_reason.is_some() {
        return None;
    }
    let wire = b.model_id.as_deref().unwrap_or(b.name.as_str());
    (!wire.is_empty()).then_some(wire)
}

/// 3 か所（dispatcher・llm-proxy・routing catalog）が割り当てを読む口。`SqliteStore` が実装する。
pub trait RoleAssignmentReader: Send + Sync {
    fn assignment_view(&self) -> Result<AssignmentView, String>;
}

/// 固定の view を返す reader（他 crate の試験用）。
#[derive(Debug, Clone, Default)]
pub struct StaticAssignments(pub AssignmentView);

impl RoleAssignmentReader for StaticAssignments {
    fn assignment_view(&self) -> Result<AssignmentView, String> {
        Ok(self.0.clone())
    }
}

/// `ProviderLive.llm_source` から catalog の source 名を求める。catalog を持たない source は `None`。
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
/// から provider 行ごとの実行用モデル名（wire）を作る規則。dispatcher・llm-proxy・routing catalog・API の preview が
/// 同じ規則を使う。
///
/// 1. 行の config の wire（lane の `tier_models` binding → 行の `model`）に `<prefix>/` があればそれを引き継ぐ
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
    use crate::model_catalog::CatalogOverride;

    fn assignment(source: &str, tier: Tier, model: &str) -> RoleAssignment {
        RoleAssignment {
            priority: 0,
            source: CatalogSource::new(source),
            tier,
            model_id: model.into(),
            note: None,
            updated_at: 1,
            updated_by: "admin".into(),
        }
    }

    fn entry(source: &str, model: &str, available: bool) -> CatalogEntry {
        CatalogEntry {
            source: CatalogSource::new(source),
            model_id: model.into(),
            display_name: None,
            first_seen: 1,
            last_seen: 1,
            available,
            capabilities: serde_json::json!({}),
        }
    }

    fn disabled(source: &str, model: &str) -> CatalogOverrideRow {
        CatalogOverrideRow {
            source: CatalogSource::new(source),
            model_id: model.into(),
            value: CatalogOverride {
                disabled: true,
                ..CatalogOverride::default()
            },
            updated_at: 1,
        }
    }

    fn view_of(items: &[(&str, Tier, &str, AssignmentState)]) -> AssignmentView {
        AssignmentView {
            managed: Vec::new(),
            items: items
                .iter()
                .map(|(s, t, m, st)| EffectiveAssignment {
                    priority: 0,
                    source: CatalogSource::new(*s),
                    tier: *t,
                    model_id: (*m).into(),
                    state: *st,
                    note: None,
                    updated_at: 1,
                    updated_by: "admin".into(),
                })
                .collect(),
        }
    }

    fn binding(model: &str, effort: Option<&str>) -> ModelBinding {
        ModelBinding {
            name: "cfg".into(),
            model_id: Some(model.into()),
            unavailable_reason: None,
            reasoning_effort: effort.map(str::to_string),
        }
    }

    #[test]
    fn build_marks_unavailable_and_disabled_as_excluded() {
        let assignments = [
            assignment("opencode-go", Tier::Frontier, "glm-5"),
            assignment("opencode-go", Tier::Standard, "gone"),
            assignment("opencode-go", Tier::Cheap, "off"),
            assignment("claude-oauth", Tier::Cheap, "no-row"),
        ];
        let entries = [
            entry("opencode-go", "glm-5", true),
            entry("opencode-go", "gone", false),
            // disabled と unavailable が重なったら disabled を先に見る。
            entry("opencode-go", "off", false),
        ];
        let overrides = [disabled("opencode-go", "off")];
        let view = AssignmentView::build(&assignments, &entries, &overrides);
        assert_eq!(view.items.len(), 4);
        assert_eq!(
            view.get("opencode-go", Tier::Frontier).map(|a| a.state),
            Some(AssignmentState::Assigned)
        );
        assert_eq!(
            view.get("opencode-go", Tier::Standard).map(|a| a.state),
            Some(AssignmentState::Excluded {
                reason: "catalog:unavailable"
            })
        );
        assert_eq!(
            view.get("opencode-go", Tier::Cheap).map(|a| a.state),
            Some(AssignmentState::Excluded {
                reason: "override:disabled"
            })
        );
        // catalog に行が無いモデルは Assigned のまま。
        assert_eq!(
            view.get("claude-oauth", Tier::Cheap).map(|a| a.state),
            Some(AssignmentState::Assigned)
        );
        assert!(view.get("claude-oauth", Tier::Frontier).is_none());
        assert!(AssignmentView::empty().get("x", Tier::Cheap).is_none());
    }

    #[test]
    fn apply_replaces_assigned_lane_and_keeps_the_others() {
        let mut bindings = TierModels::new();
        bindings.insert(Tier::Frontier, binding("old-f", Some("high")));
        bindings.insert(Tier::Cheap, binding("old-c", None));
        let view = view_of(&[("s", Tier::Frontier, "new-f", AssignmentState::Assigned)]);
        let got = apply_to_bindings(
            &bindings,
            "s",
            &[Tier::Frontier, Tier::Cheap],
            WireRule::proxy("s"),
            &view,
        );
        let f = &got[&Tier::Frontier];
        assert_eq!(f.name, "new-f");
        assert_eq!(f.model_id.as_deref(), Some("new-f"));
        assert_eq!(f.unavailable_reason, None);
        assert_eq!(f.reasoning_effort.as_deref(), Some("high"));
        // 割り当ての無い lane は従来どおり。
        assert_eq!(got[&Tier::Cheap], bindings[&Tier::Cheap]);
        // 別 source の割り当ては効かない。
        let other = apply_to_bindings(
            &bindings,
            "t",
            &[Tier::Frontier],
            WireRule::proxy("t"),
            &view,
        );
        assert_eq!(other, bindings);
    }

    #[test]
    fn apply_is_a_noop_for_empty_bindings_without_assignments() {
        let got = apply_to_bindings(
            &TierModels::new(),
            "s",
            &[Tier::Frontier, Tier::Standard, Tier::Cheap],
            WireRule::proxy("s"),
            &AssignmentView::empty(),
        );
        assert!(got.is_empty());
    }

    #[test]
    fn apply_prefixes_the_wire_model_id() {
        let view = view_of(&[(
            "opencode-go",
            Tier::Cheap,
            "glm-5",
            AssignmentState::Assigned,
        )]);
        // model も tier_models も無い opencode go の acp 行: 表の `opencode-go/`。
        let got = apply_to_bindings(
            &TierModels::new(),
            "opencode-go",
            &[Tier::Cheap],
            WireRule::new("opencode-go", "acp", None),
            &view,
        );
        assert_eq!(got[&Tier::Cheap].name, "glm-5");
        assert_eq!(
            got[&Tier::Cheap].model_id.as_deref(),
            Some("opencode-go/glm-5")
        );
        // Pi 行も同じ表（Pi の provider 名も `opencode-go`）。
        let pi = apply_to_bindings(
            &TierModels::new(),
            "opencode-go",
            &[Tier::Cheap],
            WireRule::new("opencode-go", "pi", Some("opencode-go/kimi")),
            &view,
        );
        assert_eq!(
            pi[&Tier::Cheap].model_id.as_deref(),
            Some("opencode-go/glm-5")
        );
    }

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
        // config の binding: `unavailable_reason` のある行は config の wire ではない。
        let unavailable = ModelBinding {
            name: "cheap".into(),
            model_id: None,
            unavailable_reason: Some("assignment:none".into()),
            reasoning_effort: None,
        };
        assert_eq!(configured_wire(Some(&unavailable)), None);
        assert_eq!(
            configured_wire(Some(&binding("qwen-local/q", None))),
            Some("qwen-local/q")
        );
    }

    #[test]
    fn apply_excluded_lane_cannot_resolve_and_missing_lanes_are_unroutable() {
        let view = view_of(&[
            (
                "s",
                Tier::Frontier,
                "gone",
                AssignmentState::Excluded {
                    reason: "catalog:unavailable",
                },
            ),
            ("s", Tier::Cheap, "m", AssignmentState::Assigned),
        ]);
        let lanes = [Tier::Frontier, Tier::Standard, Tier::Cheap];
        let got = apply_to_bindings(&TierModels::new(), "s", &lanes, WireRule::proxy("s"), &view);
        let f = &got[&Tier::Frontier];
        assert_eq!(f.model_id, None);
        assert_eq!(
            f.unavailable_reason.as_deref(),
            Some("assignment:gone catalog:unavailable")
        );
        assert!(crate::model_routing::resolve(&got, Tier::Frontier).is_err());
        // 割り当ても binding も無い lane は Err（panic も誤 routing もしない）。
        let std_binding = &got[&Tier::Standard];
        assert_eq!(
            std_binding.unavailable_reason.as_deref(),
            Some("assignment:none")
        );
        assert!(crate::model_routing::resolve(&got, Tier::Standard).is_err());
        assert_eq!(
            crate::model_routing::resolve(&got, Tier::Cheap),
            Ok(Some("m".to_string()))
        );
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
