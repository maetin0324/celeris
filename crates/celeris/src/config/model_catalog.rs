//! `[model_catalog]`（ADR 2026-10-06 D4）: 利用可能モデルの自動発見の周期と、opencode go の取得先。

use serde::Deserialize;

/// `[model_catalog]`。発見は決定的な HTTP・コマンド実行だけで、LLM は呼ばない。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelCatalogConfig {
    /// tick loop が発見を走らせる間隔（秒）。`0` で自動発見を無効にする（手動の
    /// `POST /api/v1/llm/models/discover` は使える）。
    #[serde(default = "default_refresh_interval_seconds")]
    pub refresh_interval_seconds: u64,
    /// opencode go gateway のモデル一覧（認証不要、OpenAI 形式）。
    #[serde(default = "default_opencode_go_models_url")]
    pub opencode_go_models_url: String,
    /// gateway が引けないときの代替（`<cmd> models opencode-go`）。
    #[serde(default = "default_opencode_cli")]
    pub opencode_cli: String,
}

impl Default for ModelCatalogConfig {
    fn default() -> Self {
        Self {
            refresh_interval_seconds: default_refresh_interval_seconds(),
            opencode_go_models_url: default_opencode_go_models_url(),
            opencode_cli: default_opencode_cli(),
        }
    }
}

fn default_refresh_interval_seconds() -> u64 {
    3600
}
fn default_opencode_go_models_url() -> String {
    "https://opencode.ai/zen/go/v1/models".to_string()
}
fn default_opencode_cli() -> String {
    "opencode".to_string()
}

use task_core::Tier;
use task_core::model_catalog::assignments::{
    AssignmentState, AssignmentView, LLM_PROXY_ADAPTER, WireRule,
};
use task_core::model_catalog::{CatalogEntry, CatalogOverrideRow};
use task_core::model_router::profiles::{DeploymentProfile, ModelProfile};

use super::RoutingCatalog;
use super::model_routing::unknown_model;

/// routing の source 名（`opencode_go` / `openai_compatible:x` / `openai-compatible:x`）を catalog の
/// source 名（ハイフン区切り）に揃える。
fn normalize_source(source_ref: &str) -> String {
    match source_ref.split_once(':') {
        Some((head, rest)) => format!("{}:{rest}", head.replace('_', "-")),
        None => source_ref.replace('_', "-"),
    }
}

/// deployment の `upstream_model` が catalog の `model_id` を指すか。付記 2026-10-07 wire-prefix: 実行用の名前の
/// `<prefix>/`（`opencode-go/`・`qwen-local/` など。`WireRule::catalog_model_id`）は外して比べる。
fn upstream_is(upstream: &str, model_id: &str) -> bool {
    upstream == model_id || WireRule::catalog_model_id(upstream) == model_id
}

/// 候補から外した deployment 1 件（API の `warnings` と ログに出す）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DroppedDeployment {
    pub deployment_id: String,
    /// `catalog:unavailable`（発見で見えなくなった）か `override:disabled`（人が無効にした）。
    pub reason: &'static str,
}

/// ADR 2026-10-06 D4: catalog で `available = 0` のモデル、または上書きで `disabled` のモデルを指す deployment を
/// routing の候補（`catalog.deployments`）から外す。catalog に無いモデルは触らない（catalog は config を
/// **足さない**・**絞らない**のは見えている範囲だけ）。外した理由は `catalog.warnings` にも 1 行ずつ足す。
/// 上書きの `disabled` は catalog に行が無いモデルにも効く。`tier` / `alias` は routing には使わない
/// （人が config の `[llm_sources.<id>]`・route の候補で決める）。
pub fn apply_model_catalog(
    catalog: &mut RoutingCatalog,
    entries: &[CatalogEntry],
    overrides: &[CatalogOverrideRow],
) -> Vec<DroppedDeployment> {
    let mut dropped = Vec::new();
    catalog.deployments.retain(|dep| {
        let source = normalize_source(&dep.source_ref);
        let disabled = overrides.iter().any(|o| {
            o.value.disabled
                && o.source.as_str() == source
                && upstream_is(&dep.upstream_model, &o.model_id)
        });
        let reason = if disabled {
            Some("override:disabled")
        } else if entries.iter().any(|e| {
            !e.available
                && e.source.as_str() == source
                && upstream_is(&dep.upstream_model, &e.model_id)
        }) {
            Some("catalog:unavailable")
        } else {
            None
        };
        match reason {
            Some(reason) => {
                dropped.push(DroppedDeployment {
                    deployment_id: dep.id.clone(),
                    reason,
                });
                false
            }
            None => true,
        }
    });
    for d in &dropped {
        catalog.warnings.push(format!(
            "model_catalog: deployment {} excluded from routing ({})",
            d.deployment_id, d.reason
        ));
    }
    dropped
}

/// `apply_role_assignments` が当てた（または外した）deployment 1 件（ログ・結合試験用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedAssignment {
    /// 当てた後の deployment id。
    pub deployment_id: String,
    /// catalog の source 名（ハイフン区切り）。
    pub source: String,
    pub tier: Tier,
    /// 割り当てた `upstream_model`（`Excluded` なら `None`）。
    pub upstream_model: Option<String>,
    /// `Excluded` のときの理由（`catalog:unavailable` / `override:disabled`）。
    pub excluded_reason: Option<&'static str>,
}

/// 割り当てた model を deployment の `upstream_model` に書く形にする（付記 2026-10-07 wire-prefix: dispatcher・
/// llm-proxy と同じ `WireRule`）。元の `upstream_model` が行の config の wire なので、その `<prefix>/` を引き継ぐ
/// （`qwen-local/qwen3.8-27b` → `qwen-local/<model_id>`、`opencode-go/kimi` → `opencode-go/<model_id>`）。接頭辞が
/// 無ければ source × llm-proxy の表で決める。
fn assigned_upstream(dep: &DeploymentProfile, source: &str, model_id: &str) -> String {
    WireRule::new(source, LLM_PROXY_ADAPTER, None).model(Some(&dep.upstream_model), model_id)
}

fn ensure_model(catalog_models: &mut Vec<ModelProfile>, id: &str, family: Option<&str>) {
    if catalog_models.iter().any(|m| m.id == id) {
        return;
    }
    let mut model = unknown_model(id, "model_role_assignments");
    if let Some(family) = family {
        model.family = family.to_string();
    }
    catalog_models.push(model);
}

/// ADR 2026-10-06 model-role-assignments D2: DB の割り当て（`source × 役割 → model`）を routing catalog の
/// deployment に重ねる。`apply_model_catalog` の前に適用する（割り当て済みで使えない model はここで外すので、
/// 後段と警告は重ならない。config の model が消えていても、利用可能な model への割り当てで lane が復活する）。dispatcher の `apply_to_bindings`・llm-proxy の
/// `normalize_legacy_config_with` と同じ `(source, tier) → model` を返す。
///
/// 対象は `legacy:<source>:<Tier>`（llm-proxy 由来）の 1 lane deployment。`source_ref` を catalog の source 名に
/// 正規化したものが割り当ての source と一致し、deployment の lane に割り当てがあるときだけ動く。
///
/// - `Assigned`: `upstream_model` を割り当ての model に、`model_profile_id` を `legacy:<family>:<wire>` に書き換える。
///   model profile が無ければ `unknown_model(.., "model_role_assignments")` を足す。
/// - `Excluded`: deployment を外し、`catalog.warnings` に
///   `model_role_assignments: deployment <id> excluded (<reason>)` を足す。
/// - 上記以外の deployment（`[[model_routing.deployments]]` の手書き行）には触らない。
/// - 書き換える `upstream_model` は付記 2026-10-07 wire-prefix の `WireRule`（`assigned_upstream`）: 元の
///   `upstream_model` の `<prefix>/` を引き継ぐ（`qwen-local/qwen3.8-27b` → `qwen-local/<model_id>`）。
pub fn apply_role_assignments(
    catalog: &mut RoutingCatalog,
    view: &AssignmentView,
) -> Vec<AppliedAssignment> {
    let mut applied = Vec::new();
    if view.items.is_empty() && view.managed.is_empty() {
        return applied;
    }
    let mut out: Vec<DeploymentProfile> = Vec::with_capacity(catalog.deployments.len());
    let mut warnings = Vec::new();
    let mut new_models: Vec<(String, Option<String>)> = Vec::new();
    for dep in std::mem::take(&mut catalog.deployments) {
        if !dep.id.starts_with("legacy:") {
            out.push(dep);
            continue;
        }
        let source = normalize_source(&dep.source_ref);
        let lanes: Vec<Tier> = dep
            .allowed_lanes
            .iter()
            .copied()
            .filter(|lane| view.manages(&source, *lane))
            .collect();
        if lanes.is_empty() {
            out.push(dep);
            continue;
        }
        for lane in lanes {
            for (index, assignment) in view.members(&source, lane).into_iter().enumerate() {
                let id = if index == 0 {
                    dep.id.clone()
                } else {
                    format!("{}/model:{}", dep.id, assignment.model_id)
                };
                match assignment.state {
                    AssignmentState::Excluded { reason } => {
                        warnings.push(format!(
                            "model_role_assignments: deployment {id} excluded ({reason})"
                        ));
                        applied.push(AppliedAssignment {
                            deployment_id: id,
                            source: source.clone(),
                            tier: lane,
                            upstream_model: None,
                            excluded_reason: Some(reason),
                        });
                    }
                    AssignmentState::Assigned => {
                        let upstream = assigned_upstream(&dep, &source, &assignment.model_id);
                        let family = dep.model_profile_id.split(':').nth(1).map(str::to_string);
                        let profile_id = format!(
                            "legacy:{}:{upstream}",
                            family.as_deref().unwrap_or("unknown")
                        );
                        new_models.push((profile_id.clone(), family));
                        let mut next = dep.clone();
                        next.id = id.clone();
                        next.upstream_model = upstream.clone();
                        next.model_profile_id = profile_id;
                        next.allowed_lanes = vec![lane];
                        next.config_order = assignment.priority as usize;
                        applied.push(AppliedAssignment {
                            deployment_id: id,
                            source: source.clone(),
                            tier: lane,
                            upstream_model: Some(upstream),
                            excluded_reason: None,
                        });
                        out.push(next);
                    }
                }
            }
        }
    }
    catalog.deployments = out;
    for (id, family) in new_models {
        ensure_model(&mut catalog.models, &id, family.as_deref());
    }
    catalog.warnings.extend(warnings);
    applied
}

#[cfg(test)]
mod tests {
    use super::*;
    use task_core::Tier;
    use task_core::model_catalog::{CatalogOverride, CatalogSource};
    use task_core::model_router::policy::RoutingMode;
    use task_core::model_router::profiles::{Billing, DeploymentProfile};

    fn dep(id: &str, source_ref: &str, upstream: &str) -> DeploymentProfile {
        DeploymentProfile {
            id: id.into(),
            source_ref: source_ref.into(),
            model_profile_id: upstream.into(),
            upstream_model: upstream.into(),
            adapter_constraints: vec![],
            billing: Billing::Subscription,
            host: None,
            region: None,
            trust_zone: None,
            external_network: true,
            retains_data: None,
            allowed_lanes: vec![Tier::Cheap],
            resource_group_id: None,
            concurrency_limit: None,
            rpm_limit: None,
            tpm_limit: None,
            price_override: None,
            config_order: 0,
        }
    }

    fn entry(source: &str, id: &str, available: bool) -> CatalogEntry {
        CatalogEntry {
            source: CatalogSource::new(source),
            model_id: id.into(),
            display_name: None,
            first_seen: 0,
            last_seen: 0,
            available,
            capabilities: serde_json::json!({}),
        }
    }

    fn routing() -> RoutingCatalog {
        RoutingCatalog {
            mode: RoutingMode::Legacy,
            models: vec![],
            deployments: vec![
                dep("a", "opencode_go", "opencode-go/glm-5"),
                dep("b", "opencode_go", "opencode-go/kimi"),
                dep("c", "openai_compatible:qwen", "qwen3"),
                dep("d", "claude_oauth", "claude-x"),
            ],
            policies: vec![],
            warnings: vec![],
        }
    }

    #[test]
    fn unavailable_and_disabled_models_leave_the_candidates() {
        let mut catalog = routing();
        let entries = vec![
            entry("opencode-go", "glm-5", false),
            entry("opencode-go", "kimi", true),
            entry("openai-compatible:qwen", "qwen3", true),
        ];
        let overrides = vec![
            CatalogOverrideRow {
                source: CatalogSource::new("openai-compatible:qwen"),
                model_id: "qwen3".into(),
                value: CatalogOverride {
                    disabled: true,
                    ..CatalogOverride::default()
                },
                updated_at: 0,
            },
            // disabled でない上書きは候補を変えない。
            CatalogOverrideRow {
                source: CatalogSource::new("claude-oauth"),
                model_id: "claude-x".into(),
                value: CatalogOverride {
                    tier: Some(Tier::Cheap),
                    ..CatalogOverride::default()
                },
                updated_at: 0,
            },
        ];
        let dropped = apply_model_catalog(&mut catalog, &entries, &overrides);
        assert_eq!(
            dropped,
            vec![
                DroppedDeployment {
                    deployment_id: "a".into(),
                    reason: "catalog:unavailable"
                },
                DroppedDeployment {
                    deployment_id: "c".into(),
                    reason: "override:disabled"
                },
            ]
        );
        let left: Vec<&str> = catalog.deployments.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(left, ["b", "d"]);
        assert_eq!(catalog.warnings.len(), 2);
        assert!(catalog.warnings[0].contains("catalog:unavailable"));
    }

    #[test]
    fn models_missing_from_the_catalog_are_left_alone() {
        let mut catalog = routing();
        let dropped = apply_model_catalog(&mut catalog, &[], &[]);
        assert!(dropped.is_empty());
        assert_eq!(catalog.deployments.len(), 4);
        assert!(catalog.warnings.is_empty());
    }

    fn view(items: &[(&str, Tier, &str, AssignmentState)]) -> AssignmentView {
        AssignmentView {
            managed: Vec::new(),
            items: items
                .iter()
                .map(|(source, tier, model, state)| {
                    task_core::model_catalog::assignments::EffectiveAssignment {
                        priority: 0,
                        source: CatalogSource::new(*source),
                        tier: *tier,
                        model_id: (*model).into(),
                        state: *state,
                        note: None,
                        updated_at: 0,
                        updated_by: "admin".into(),
                    }
                })
                .collect(),
        }
    }

    fn lane_dep(
        id: &str,
        source_ref: &str,
        upstream: &str,
        profile: &str,
        lane: Tier,
    ) -> DeploymentProfile {
        let mut d = dep(id, source_ref, upstream);
        d.model_profile_id = profile.into();
        d.allowed_lanes = vec![lane];
        d
    }

    #[test]
    fn role_assignments_rewrite_lane_deployments_and_add_the_model_profile() {
        let mut catalog = routing();
        catalog.deployments = vec![
            lane_dep(
                "legacy:claude-oauth:Cheap",
                "claude-oauth",
                "claude-c",
                "legacy:claude:claude-c",
                Tier::Cheap,
            ),
            lane_dep(
                "legacy:claude-oauth:Standard",
                "claude-oauth",
                "claude-s",
                "legacy:claude:claude-s",
                Tier::Standard,
            ),
            lane_dep(
                "manual",
                "claude_oauth",
                "hand-written",
                "hand-written",
                Tier::Cheap,
            ),
        ];
        let v = view(&[(
            "claude-oauth",
            Tier::Cheap,
            "claude-new",
            AssignmentState::Assigned,
        )]);
        let applied = apply_role_assignments(&mut catalog, &v);
        assert_eq!(applied.len(), 1);
        let by_id = |id: &str| catalog.deployments.iter().find(|d| d.id == id).unwrap();
        let c = by_id("legacy:claude-oauth:Cheap");
        assert_eq!(c.upstream_model, "claude-new");
        assert_eq!(c.model_profile_id, "legacy:claude:claude-new");
        assert_eq!(
            by_id("legacy:claude-oauth:Standard").upstream_model,
            "claude-s"
        );
        // 手書きの deployment には触らない。
        assert_eq!(by_id("manual").upstream_model, "hand-written");
        let m = catalog
            .models
            .iter()
            .find(|m| m.id == "legacy:claude:claude-new")
            .expect("assigned model profile");
        assert_eq!(m.provenance, "model_role_assignments");
        assert!(catalog.warnings.is_empty());
    }

    #[test]
    fn role_assignments_remove_excluded_lanes_with_a_warning() {
        let mut catalog = routing();
        catalog.deployments = vec![lane_dep(
            "legacy:codex-oauth:Cheap",
            "codex-oauth",
            "gpt-c",
            "legacy:gpt:gpt-c",
            Tier::Cheap,
        )];
        let v = view(&[(
            "codex-oauth",
            Tier::Cheap,
            "gone",
            AssignmentState::Excluded {
                reason: "override:disabled",
            },
        )]);
        let applied = apply_role_assignments(&mut catalog, &v);
        assert!(catalog.deployments.is_empty());
        assert_eq!(applied[0].excluded_reason, Some("override:disabled"));
        assert_eq!(
            catalog.warnings,
            [
                "model_role_assignments: deployment legacy:codex-oauth:Cheap excluded (override:disabled)"
            ]
        );
    }

    #[test]
    fn empty_view_changes_nothing() {
        let mut catalog = routing();
        assert!(apply_role_assignments(&mut catalog, &AssignmentView::empty()).is_empty());
        assert_eq!(catalog.deployments.len(), 4);
    }

    /// ADR 2026-10-06 model-role-assignments 付記（2026-10-07 wire-prefix）: 元の `upstream_model` の接頭辞
    /// （`qwen-local/`）は引き継ぎ、接頭辞の無い proxy の lane はそのまま。catalog の不在は接頭辞を見通して効く。
    #[test]
    fn legacy_lanes_inherit_the_configured_prefix() {
        let mut catalog = routing();
        catalog.deployments = vec![
            lane_dep(
                "legacy:openai-compatible:qwen:Cheap",
                "openai-compatible:qwen",
                "qwen3.8-27b",
                "legacy:qwen:qwen3.8-27b",
                Tier::Cheap,
            ),
            lane_dep(
                "legacy:openai-compatible:qwen-local:Cheap",
                "openai-compatible:qwen",
                "qwen-local/qwen3.8-27b",
                "legacy:qwen:qwen-local/qwen3.8-27b",
                Tier::Cheap,
            ),
        ];
        let v = view(&[(
            "openai-compatible:qwen",
            Tier::Cheap,
            "qwen3.9-32b",
            AssignmentState::Assigned,
        )]);
        apply_role_assignments(&mut catalog, &v);
        let upstream = |id: &str| {
            catalog
                .deployments
                .iter()
                .find(|d| d.id == id)
                .map(|d| d.upstream_model.clone())
                .unwrap_or_else(|| panic!("{id} present: {:?}", catalog.deployments))
        };
        assert_eq!(
            upstream("legacy:openai-compatible:qwen:Cheap"),
            "qwen3.9-32b"
        );
        assert_eq!(
            upstream("legacy:openai-compatible:qwen-local:Cheap"),
            "qwen-local/qwen3.9-32b"
        );
        assert!(catalog.warnings.is_empty());
        let dropped = apply_model_catalog(
            &mut catalog,
            &[entry("openai-compatible:qwen", "qwen3.9-32b", false)],
            &[],
        );
        let ids: Vec<&str> = dropped.iter().map(|d| d.deployment_id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "legacy:openai-compatible:qwen:Cheap",
                "legacy:openai-compatible:qwen-local:Cheap",
            ]
        );
    }
}
