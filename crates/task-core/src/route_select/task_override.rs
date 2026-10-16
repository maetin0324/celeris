//! task 単位の routing 固定（ADR 2026-10-16-mlsys-measure-task-routing）。
//!
//! 本番の全体 config（`[routing] mode`）を切り替えずに、試行ごとの routing 設定を task に固定する。
//! 解決は「task 欄があれば config より優先、無ければ従来（config）」。純粋関数のみ。

use super::lane::{DEFAULT_POLICY_VERSION, RoutingMode};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `Task.routing.routing_override`（作成時に固定。`POST /tasks` の `routing_override`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingOverride {
    /// この task の lane 層の mode（config の `[routing] mode` より優先）。
    pub mode: RoutingMode,
    /// 下段（fixed 方策・lane→model 対応表）の版。省略時は `DEFAULT_POLICY_VERSION`。現行の版以外は受理しない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lower_policy: Option<String>,
    /// 根拠表 `routing_evidence` の版（`evidence_version`）。指定すると最新版ではなくこの版だけを読む。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_table_id: Option<String>,
    /// run の結果を `routing_outcome_recorded`（オンライン更新の根拠）へ追記するか。既定 true（従来）。
    #[serde(default = "default_true")]
    pub online_update: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RoutingOverrideError {
    #[error(
        "routing_override.lower_policy {0:?} is not a known policy version (known: {DEFAULT_POLICY_VERSION})"
    )]
    UnknownLowerPolicy(String),
    #[error("routing_override.evidence_table_id must not be empty")]
    EmptyEvidenceTable,
}

impl RoutingOverride {
    /// 作成時の検査。未知の下段版・空の根拠表 id は拒否する（黙って最新・既定へ倒さない）。
    pub fn validate(&self) -> Result<(), RoutingOverrideError> {
        if let Some(p) = &self.lower_policy
            && p != DEFAULT_POLICY_VERSION
        {
            return Err(RoutingOverrideError::UnknownLowerPolicy(p.clone()));
        }
        if self
            .evidence_table_id
            .as_deref()
            .is_some_and(|id| id.trim().is_empty())
        {
            return Err(RoutingOverrideError::EmptyEvidenceTable);
        }
        Ok(())
    }
}

/// 解決の出自。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModeSource {
    Task,
    Config,
}

/// 解決結果（`celeris.route-decision/1` の `routing_resolved` に載る）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResolvedRouting {
    pub mode: RoutingMode,
    pub source: ModeSource,
    pub lower_policy: String,
    /// `None` は「最新版を読む」（従来）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_table_id: Option<String>,
    pub online_update: bool,
}

/// task 欄があれば config より優先、無ければ従来（config の mode・既定の下段・最新の根拠表・オンライン更新あり）。
pub fn resolve_routing(
    config_mode: RoutingMode,
    task: Option<&RoutingOverride>,
) -> ResolvedRouting {
    match task {
        Some(o) => ResolvedRouting {
            mode: o.mode,
            source: ModeSource::Task,
            lower_policy: o
                .lower_policy
                .clone()
                .unwrap_or_else(|| DEFAULT_POLICY_VERSION.to_string()),
            evidence_table_id: o.evidence_table_id.clone(),
            online_update: o.online_update,
        },
        None => ResolvedRouting {
            mode: config_mode,
            source: ModeSource::Config,
            lower_policy: DEFAULT_POLICY_VERSION.to_string(),
            evidence_table_id: None,
            online_update: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ov(mode: RoutingMode) -> RoutingOverride {
        RoutingOverride {
            mode,
            lower_policy: None,
            evidence_table_id: None,
            online_update: true,
        }
    }

    #[test]
    fn task_field_beats_config_and_absence_keeps_config() {
        for (config, task) in [
            (RoutingMode::DecisionAssisted, RoutingMode::Fixed),
            (RoutingMode::Fixed, RoutingMode::DecisionAssisted),
        ] {
            let r = resolve_routing(config, Some(&ov(task)));
            assert_eq!((r.mode, r.source), (task, ModeSource::Task));
            let r = resolve_routing(config, None);
            assert_eq!((r.mode, r.source), (config, ModeSource::Config));
            assert!(r.online_update);
            assert_eq!(r.evidence_table_id, None);
            assert_eq!(r.lower_policy, DEFAULT_POLICY_VERSION);
        }
    }

    #[test]
    fn json_defaults_and_validation() {
        let o: RoutingOverride = serde_json::from_str(r#"{"mode":"fixed"}"#).unwrap();
        assert!(o.online_update);
        assert!(o.validate().is_ok());
        assert!(serde_json::from_str::<RoutingOverride>(r#"{"mode":"auto"}"#).is_err());
        assert!(serde_json::from_str::<RoutingOverride>(r#"{"mode":"fixed","x":1}"#).is_err());
        let mut bad = ov(RoutingMode::Fixed);
        bad.lower_policy = Some("mlsys-min-0".into());
        assert!(matches!(
            bad.validate(),
            Err(RoutingOverrideError::UnknownLowerPolicy(_))
        ));
        bad.lower_policy = Some(DEFAULT_POLICY_VERSION.into());
        bad.evidence_table_id = Some(" ".into());
        assert_eq!(
            bad.validate(),
            Err(RoutingOverrideError::EmptyEvidenceTable)
        );
    }
}
