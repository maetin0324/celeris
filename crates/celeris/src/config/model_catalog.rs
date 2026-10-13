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

use task_core::llm_route::{CandidateKey, Registry};
use task_core::model_catalog::assignments::WireRule;
use task_core::model_catalog::{CatalogEntry, CatalogOverrideRow};

/// Catalog source identifiers use hyphens while route declarations preserve their configured spelling.
fn normalize_source(source_ref: &str) -> String {
    match source_ref.split_once(':') {
        Some((head, rest)) => format!("{}:{rest}", head.replace('_', "-")),
        None => source_ref.replace('_', "-"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DroppedCandidate {
    pub source: String,
    pub model: String,
    pub reason: &'static str,
}

/// Exclude declared source/model candidates explicitly marked unavailable or disabled.
/// Missing catalog rows remain eligible because discovery is not authoritative for unknown models.
pub fn apply_model_catalog(
    registry: &mut Registry,
    entries: &[CatalogEntry],
    overrides: &[CatalogOverrideRow],
) -> Vec<DroppedCandidate> {
    fn filter(
        candidates: &mut Vec<CandidateKey>,
        entries: &[CatalogEntry],
        overrides: &[CatalogOverrideRow],
        dropped: &mut Vec<DroppedCandidate>,
    ) {
        candidates.retain(|candidate| {
            let source = normalize_source(candidate.source.as_ref());
            let model = candidate.model.as_ref();
            let matches = |catalog_model: &str| {
                catalog_model == model || WireRule::catalog_model_id(model) == catalog_model
            };
            let disabled = overrides
                .iter()
                .any(|o| o.value.disabled && o.source.as_str() == source && matches(&o.model_id));
            let unavailable = entries
                .iter()
                .any(|e| !e.available && e.source.as_str() == source && matches(&e.model_id));
            let reason = if disabled {
                Some("override:disabled")
            } else if unavailable {
                Some("catalog:unavailable")
            } else {
                None
            };
            if let Some(reason) = reason {
                dropped.push(DroppedCandidate {
                    source,
                    model: model.to_owned(),
                    reason,
                });
                false
            } else {
                true
            }
        });
    }
    let mut dropped = Vec::new();
    for candidates in registry.route_defaults.values_mut() {
        filter(candidates, entries, overrides, &mut dropped);
    }
    for candidates in registry.lane_route_defaults.values_mut() {
        filter(candidates, entries, overrides, &mut dropped);
    }
    filter(
        &mut registry.cos_route_candidates,
        entries,
        overrides,
        &mut dropped,
    );
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;
    use task_core::llm_route::{HarnessId, ModelId, SourceId};
    use task_core::model_catalog::{CatalogOverride, CatalogSource};

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
    fn registry() -> Registry {
        let candidates = [
            ("opencode_go", "opencode-go/glm-5"),
            ("opencode_go", "opencode-go/kimi"),
            ("openai_compatible:qwen", "qwen3"),
            ("claude_oauth", "claude-x"),
        ]
        .into_iter()
        .map(|(source, model)| CandidateKey {
            source: SourceId(source.into()),
            model: ModelId(model.into()),
        })
        .collect();
        Registry {
            route_defaults: [(HarnessId("h".into()), candidates)].into(),
            ..Registry::default()
        }
    }
    #[test]
    fn unavailable_and_disabled_models_leave_the_candidates() {
        let mut registry = registry();
        let overrides = vec![
            CatalogOverrideRow {
                source: CatalogSource::new("openai-compatible:qwen"),
                model_id: "qwen3".into(),
                value: CatalogOverride {
                    disabled: true,
                    ..Default::default()
                },
                updated_at: 0,
            },
            CatalogOverrideRow {
                source: CatalogSource::new("claude-oauth"),
                model_id: "claude-x".into(),
                value: CatalogOverride {
                    tier: Some(task_core::Tier::Cheap),
                    ..Default::default()
                },
                updated_at: 0,
            },
        ];
        let dropped = apply_model_catalog(
            &mut registry,
            &[
                entry("opencode-go", "glm-5", false),
                entry("opencode-go", "kimi", true),
                entry("openai-compatible:qwen", "qwen3", true),
            ],
            &overrides,
        );
        assert_eq!(
            dropped.iter().map(|d| d.reason).collect::<Vec<_>>(),
            ["catalog:unavailable", "override:disabled"]
        );
        assert_eq!(
            registry
                .route_defaults
                .values()
                .next()
                .unwrap()
                .iter()
                .map(|c| c.model.as_ref())
                .collect::<Vec<_>>(),
            ["opencode-go/kimi", "claude-x"]
        );
    }
    #[test]
    fn models_missing_from_the_catalog_are_left_alone() {
        let mut registry = registry();
        assert!(apply_model_catalog(&mut registry, &[], &[]).is_empty());
        assert_eq!(registry.route_defaults.values().next().unwrap().len(), 4);
    }
}
