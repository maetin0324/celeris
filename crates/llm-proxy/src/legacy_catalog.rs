//! Legacy `[llm_proxy.models]` and `[llm_proxy.sources]` catalog normalization.
//! The old keys remain the authority in legacy mode. Missing capability and price
//! observations stay unknown; no source URL or credential is copied to the catalog.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::Tier;
use task_core::model_catalog::assignments::{AssignmentState, AssignmentView};
use task_core::model_router::policy::{RoutingMode, RoutingPolicy};
use task_core::model_router::profiles::{
    Billing, Capabilities, ContextLimits, DeploymentProfile, ModelProfile, Support,
};

use crate::config::LlmProxyConfig;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LegacyCatalog {
    pub models: Vec<ModelProfile>,
    pub deployments: Vec<DeploymentProfile>,
    pub policies: Vec<RoutingPolicy>,
    /// ADR 2026-10-06 model-role-assignments D2: 割り当てで deployment を作らなかった lane の記録
    /// （`model_role_assignments: deployment legacy:<source>:<Tier> excluded (<reason>)`）。
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl LegacyCatalog {
    pub fn deployment(&self, source_ref: &str, tier: Tier) -> Option<&DeploymentProfile> {
        self.deployments
            .iter()
            .find(|d| d.source_ref == source_ref && d.allowed_lanes.contains(&tier))
    }
}

/// Derive model, deployment and policy values from the existing proxy config.
/// Source-qualified model IDs deliberately avoid merging aliases whose identity
/// and revision cannot be established from the legacy configuration.
pub fn normalize_legacy_config(config: &LlmProxyConfig) -> LegacyCatalog {
    normalize_legacy_config_with(config, &AssignmentView::empty())
}

/// `normalize_legacy_config` に DB の割り当て（ADR 2026-10-06 model-role-assignments D2）を重ねる。
/// `claude-oauth` → `models.claude`、`codex-oauth` → `models.gpt`、`openai-compatible:<id>` → その relay の
/// cheap lane の wire model を割り当ての `model_id` に置き換える。割り当ての無い lane は config のまま。
/// `Excluded` の lane は deployment を作らず `warnings` に書く。
pub fn normalize_legacy_config_with(
    config: &LlmProxyConfig,
    view: &AssignmentView,
) -> LegacyCatalog {
    let mut catalog = LegacyCatalog {
        models: Vec::new(),
        deployments: Vec::new(),
        policies: [Tier::Frontier, Tier::Standard, Tier::Cheap]
            .into_iter()
            .map(|lane| {
                let mut policy = RoutingPolicy::defaults(lane, RoutingMode::Legacy);
                policy.prefer_free = config.prefer_free;
                policy
            })
            .collect(),
        warnings: Vec::new(),
    };
    // 割り当てがあればその wire model を、`Excluded` なら deployment を作らず警告を、無ければ config の値を返す。
    let mut warnings: Vec<String> = Vec::new();
    let mut resolve = |source_ref: &str,
                       tier: Tier,
                       configured: Option<&String>|
     -> Option<String> {
        match view.get(source_ref, tier) {
            Some(a) => match a.state {
                AssignmentState::Assigned => Some(a.model_id.clone()),
                AssignmentState::Excluded { reason } => {
                    warnings.push(format!(
                        "model_role_assignments: deployment legacy:{source_ref}:{tier:?} excluded ({reason})"
                    ));
                    None
                }
            },
            None => configured.cloned(),
        }
    };
    let mut add = |source_ref: &str, family: &str, billing: Billing, tier: Tier, wire: &str| {
        let model_id = format!("legacy:{family}:{wire}");
        if !catalog.models.iter().any(|m| m.id == model_id) {
            catalog.models.push(ModelProfile {
                id: model_id.clone(),
                revision: "unknown".into(),
                family: family.into(),
                capabilities: Capabilities {
                    tools: Support::Unknown,
                    structured_output: Support::Unknown,
                    vision: Support::Unknown,
                    streaming: Support::Unknown,
                    reasoning_efforts: vec![],
                },
                context_limits: ContextLimits {
                    input: None,
                    output: None,
                    total: None,
                },
                quality: vec![],
                pricing: None,
                provenance: "llm_proxy.models (legacy)".into(),
            });
        }
        let order = catalog.deployments.len();
        catalog.deployments.push(DeploymentProfile {
            id: format!("legacy:{source_ref}:{tier:?}"),
            source_ref: source_ref.into(),
            model_profile_id: model_id,
            upstream_model: wire.into(),
            adapter_constraints: vec![],
            billing,
            host: None,
            region: None,
            trust_zone: None,
            external_network: true,
            retains_data: None,
            allowed_lanes: vec![tier],
            resource_group_id: None,
            concurrency_limit: None,
            rpm_limit: None,
            tpm_limit: None,
            price_override: None,
            config_order: order,
        });
    };
    for tier in [Tier::Frontier, Tier::Standard, Tier::Cheap] {
        if config
            .sources
            .claude_oauth
            .as_ref()
            .is_some_and(|s| s.enabled)
            && let Some(wire) = resolve("claude-oauth", tier, config.models.claude.get(&tier))
        {
            add("claude-oauth", "claude", Billing::Subscription, tier, &wire);
        }
        if config
            .sources
            .codex_oauth
            .as_ref()
            .is_some_and(|s| s.enabled)
            && let Some(wire) = resolve("codex-oauth", tier, config.models.gpt.get(&tier))
        {
            add("codex-oauth", "gpt", Billing::Subscription, tier, &wire);
        }
        // The deployment lane is fixed here, independently of source or wire prefixes.
        if tier == Tier::Cheap {
            for source in config
                .sources
                .openai_compatible
                .iter()
                .filter(|s| s.enabled)
            {
                let source_ref = format!("openai-compatible:{}", source.id);
                if let Some(wire) = resolve(&source_ref, tier, config.models.qwen.get(&Tier::Cheap))
                {
                    add(&source_ref, "qwen", Billing::SelfHosted, tier, &wire);
                }
            }
        }
    }
    catalog.warnings = warnings;
    catalog
}

#[cfg(test)]
mod tests {
    use super::*;
    use task_core::model_catalog::CatalogSource;
    use task_core::model_catalog::assignments::EffectiveAssignment;

    fn config() -> LlmProxyConfig {
        serde_json::from_value(serde_json::json!({
            "sources": {
                "claude_oauth": { "accounts_dir": "/unused" },
                "codex_oauth": { "accounts_dir": "/unused" },
                "openai_compatible": [
                    { "id": "relay-a", "base_url": "http://localhost/a" },
                    { "id": "relay-b", "base_url": "http://localhost/b" }
                ]
            },
            "models": {
                "claude": { "frontier": "claude-f", "standard": "claude-s", "cheap": "claude-c" },
                "gpt": { "frontier": "gpt-f", "standard": "gpt-s", "cheap": "gpt-c" },
                "qwen": { "cheap": "qwen-c" }
            }
        }))
        .unwrap()
    }

    fn view(items: &[(&str, Tier, &str, AssignmentState)]) -> AssignmentView {
        AssignmentView {
            items: items
                .iter()
                .map(|(source, tier, model, state)| EffectiveAssignment {
                    source: CatalogSource::new(*source),
                    tier: *tier,
                    model_id: (*model).into(),
                    state: *state,
                    note: None,
                    updated_at: 1,
                    updated_by: "admin".into(),
                })
                .collect(),
        }
    }

    fn wire<'a>(c: &'a LegacyCatalog, source: &str, tier: Tier) -> Option<&'a str> {
        c.deployment(source, tier)
            .map(|d| d.upstream_model.as_str())
    }

    #[test]
    fn empty_view_equals_the_plain_normalization() {
        let c = config();
        assert_eq!(
            normalize_legacy_config_with(&c, &AssignmentView::empty()),
            normalize_legacy_config(&c)
        );
        assert!(normalize_legacy_config(&c).warnings.is_empty());
    }

    #[test]
    fn assignments_replace_lanes_and_leave_the_others_untouched() {
        let c = config();
        let base = normalize_legacy_config(&c);
        let v = view(&[
            (
                "claude-oauth",
                Tier::Standard,
                "claude-assigned",
                AssignmentState::Assigned,
            ),
            (
                "codex-oauth",
                Tier::Cheap,
                "gpt-assigned",
                AssignmentState::Assigned,
            ),
            (
                "openai-compatible:relay-b",
                Tier::Cheap,
                "qwen-assigned",
                AssignmentState::Assigned,
            ),
        ]);
        let got = normalize_legacy_config_with(&c, &v);
        assert_eq!(
            wire(&got, "claude-oauth", Tier::Standard),
            Some("claude-assigned")
        );
        assert_eq!(wire(&got, "codex-oauth", Tier::Cheap), Some("gpt-assigned"));
        // relay ごとの cheap lane。割り当ての無い relay は config のまま。
        assert_eq!(
            wire(&got, "openai-compatible:relay-b", Tier::Cheap),
            Some("qwen-assigned")
        );
        assert_eq!(
            wire(&got, "openai-compatible:relay-a", Tier::Cheap),
            Some("qwen-c")
        );
        // 割り当てに合わせて model profile も増える（source 修飾の identity）。
        assert!(
            got.models
                .iter()
                .any(|m| m.id == "legacy:claude:claude-assigned")
        );
        // 他の lane・source は同じ。
        for (source, tier) in [
            ("claude-oauth", Tier::Frontier),
            ("claude-oauth", Tier::Cheap),
            ("codex-oauth", Tier::Frontier),
            ("codex-oauth", Tier::Standard),
        ] {
            assert_eq!(
                got.deployment(source, tier),
                base.deployment(source, tier),
                "{source} {tier:?}"
            );
        }
        assert_eq!(got.deployments.len(), base.deployments.len());
        assert!(got.warnings.is_empty());
    }

    #[test]
    fn excluded_lane_has_no_deployment_and_a_warning() {
        let c = config();
        let v = view(&[
            (
                "codex-oauth",
                Tier::Cheap,
                "gone",
                AssignmentState::Excluded {
                    reason: "catalog:unavailable",
                },
            ),
            (
                "openai-compatible:relay-a",
                Tier::Cheap,
                "off",
                AssignmentState::Excluded {
                    reason: "override:disabled",
                },
            ),
        ]);
        let got = normalize_legacy_config_with(&c, &v);
        assert!(got.deployment("codex-oauth", Tier::Cheap).is_none());
        assert!(
            got.deployment("openai-compatible:relay-a", Tier::Cheap)
                .is_none()
        );
        assert_eq!(
            wire(&got, "openai-compatible:relay-b", Tier::Cheap),
            Some("qwen-c")
        );
        assert_eq!(wire(&got, "codex-oauth", Tier::Standard), Some("gpt-s"));
        assert_eq!(
            got.warnings,
            [
                "model_role_assignments: deployment legacy:codex-oauth:Cheap excluded (catalog:unavailable)",
                "model_role_assignments: deployment legacy:openai-compatible:relay-a:Cheap excluded (override:disabled)",
            ]
        );
    }

    #[test]
    fn assignment_for_a_disabled_source_is_ignored() {
        let mut c = config();
        c.sources.codex_oauth = None;
        let v = view(&[(
            "codex-oauth",
            Tier::Cheap,
            "gpt-assigned",
            AssignmentState::Assigned,
        )]);
        let got = normalize_legacy_config_with(&c, &v);
        assert!(
            got.deployments
                .iter()
                .all(|d| d.source_ref != "codex-oauth")
        );
    }
}
