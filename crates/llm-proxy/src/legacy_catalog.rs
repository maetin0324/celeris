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

    /// 付記「モデルごとの複数役割」: 起動時の catalog に、要求時点の割り当て（`normalize_legacy_config_with`）
    /// から **無い** model profile と deployment（同じ `source_ref`・`upstream_model`・lane が無いもの）を足した
    /// 写し。policy・既存の行は `self` のまま（起動時の品質・制約を保つ）。estimator shadow が役割の全メンバーを
    /// 候補に写せるようにするためのもので、`self` は変えない。
    pub fn extended_with(&self, dynamic: &LegacyCatalog) -> LegacyCatalog {
        let mut out = self.clone();
        for d in &dynamic.deployments {
            let covered = out.deployments.iter().any(|e| {
                e.source_ref == d.source_ref
                    && e.upstream_model == d.upstream_model
                    && d.allowed_lanes.iter().all(|l| e.allowed_lanes.contains(l))
            });
            if covered {
                continue;
            }
            let mut dep = d.clone();
            if out.deployments.iter().any(|e| e.id == dep.id) {
                dep.id = format!("{}/dynamic", dep.id);
            }
            out.deployments.push(dep);
        }
        for m in &dynamic.models {
            if out.models.iter().any(|e| e.id == m.id) {
                continue;
            }
            // 起動時の同じ family の profile から context 上限・capabilities・価格を継ぐ（品質は継がない）。
            let mut model = m.clone();
            if let Some(base) = self.models.iter().find(|e| e.family == m.family) {
                model.context_limits = base.context_limits.clone();
                model.capabilities = base.capabilities.clone();
                model.pricing = base.pricing.clone();
            }
            out.models.push(model);
        }
        out
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
     -> Vec<(String, usize)> {
        if !view.manages(source_ref, tier) {
            return configured.map(|s| vec![(s.clone(), 0)]).unwrap_or_default();
        }
        view.members(source_ref, tier).into_iter().filter_map(|a| match a.state {
            AssignmentState::Assigned => Some((a.model_id.clone(), a.priority as usize)),
            AssignmentState::Excluded { reason } => {
                warnings.push(format!("model_role_assignments: deployment legacy:{source_ref}:{tier:?} excluded ({reason})"));
                None
            }
        }).collect()
    };
    let mut add = |source_ref: &str,
                   family: &str,
                   billing: Billing,
                   tier: Tier,
                   wire: &str,
                   priority: usize| {
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
        let base_id = format!("legacy:{source_ref}:{tier:?}");
        let id = if catalog.deployments.iter().any(|d| d.id == base_id) {
            format!("{base_id}/model:{wire}")
        } else {
            base_id
        };
        catalog.deployments.push(DeploymentProfile {
            id,
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
            config_order: priority,
        });
    };
    for tier in [Tier::Frontier, Tier::Standard, Tier::Cheap] {
        if config
            .sources
            .claude_oauth
            .as_ref()
            .is_some_and(|s| s.enabled)
        {
            for (wire, priority) in resolve("claude-oauth", tier, config.models.claude.get(&tier)) {
                add(
                    "claude-oauth",
                    "claude",
                    Billing::Subscription,
                    tier,
                    &wire,
                    priority,
                );
            }
        }
        if config
            .sources
            .codex_oauth
            .as_ref()
            .is_some_and(|s| s.enabled)
        {
            for (wire, priority) in resolve("codex-oauth", tier, config.models.gpt.get(&tier)) {
                add(
                    "codex-oauth",
                    "gpt",
                    Billing::Subscription,
                    tier,
                    &wire,
                    priority,
                );
            }
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
                for (wire, priority) in
                    resolve(&source_ref, tier, config.models.qwen.get(&Tier::Cheap))
                {
                    add(
                        &source_ref,
                        "qwen",
                        Billing::SelfHosted,
                        tier,
                        &wire,
                        priority,
                    );
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
            managed: Vec::new(),
            items: items
                .iter()
                .map(|(source, tier, model, state)| EffectiveAssignment {
                    priority: 0,
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
    fn extended_with_adds_missing_members_and_inherits_family_limits() {
        let c = config();
        let mut startup = normalize_legacy_config(&c);
        for m in &mut startup.models {
            if m.family == "qwen" {
                m.context_limits = ContextLimits {
                    input: Some(1000),
                    output: Some(100),
                    total: Some(1100),
                };
            }
        }
        let v = view(&[
            (
                "openai-compatible:relay-a",
                Tier::Cheap,
                "m1",
                AssignmentState::Assigned,
            ),
            (
                "openai-compatible:relay-a",
                Tier::Cheap,
                "m2",
                AssignmentState::Assigned,
            ),
        ]);
        let dynamic = normalize_legacy_config_with(&c, &v);
        let got = startup.extended_with(&dynamic);
        // 起動時の行・policy はそのまま、無いメンバーだけ足す（id の衝突は `/dynamic`）。
        assert_eq!(got.policies, startup.policies);
        assert!(got.deployments.len() == startup.deployments.len() + 2);
        let m1 = got
            .deployments
            .iter()
            .find(|d| d.source_ref == "openai-compatible:relay-a" && d.upstream_model == "m1")
            .unwrap();
        assert_eq!(m1.id, "legacy:openai-compatible:relay-a:Cheap/dynamic");
        assert!(
            got.deployments
                .iter()
                .any(|d| d.source_ref == "openai-compatible:relay-a" && d.upstream_model == "m2")
        );
        let model = got
            .models
            .iter()
            .find(|m| m.id == "legacy:qwen:m2")
            .unwrap();
        assert_eq!(model.context_limits.input, Some(1000));
        // 同じ写しをもう一度重ねても増えない。
        assert_eq!(got.extended_with(&dynamic), got);
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
