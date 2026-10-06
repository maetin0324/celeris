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

use task_core::model_catalog::{CatalogEntry, CatalogOverrideRow};

use super::RoutingCatalog;

/// routing の source 名（`opencode_go` / `openai_compatible:x` / `openai-compatible:x`）を catalog の
/// source 名（ハイフン区切り）に揃える。
fn normalize_source(source_ref: &str) -> String {
    match source_ref.split_once(':') {
        Some((head, rest)) => format!("{}:{rest}", head.replace('_', "-")),
        None => source_ref.replace('_', "-"),
    }
}

/// deployment の `upstream_model` が catalog の `model_id` を指すか（`<source>/` 接頭辞は外して比べる）。
fn upstream_is(upstream: &str, source: &str, model_id: &str) -> bool {
    upstream == model_id
        || upstream
            .strip_prefix(source)
            .and_then(|rest| rest.strip_prefix('/'))
            .is_some_and(|rest| rest == model_id)
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
/// （人が config の `tiers` / `tier_models` で決める）。
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
                && upstream_is(&dep.upstream_model, &source, &o.model_id)
        });
        let reason = if disabled {
            Some("override:disabled")
        } else if entries.iter().any(|e| {
            !e.available
                && e.source.as_str() == source
                && upstream_is(&dep.upstream_model, &source, &e.model_id)
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
}
