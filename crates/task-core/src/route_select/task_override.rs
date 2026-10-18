//! task 単位の routing 固定（ADR 2026-10-16-mlsys-measure-task-routing）。
//!
//! 本番の全体 config（`[routing] mode`）を切り替えずに、試行ごとの routing 設定を task に固定する。
//! 解決は「task 欄があれば config より優先、無ければ従来（config）」。純粋関数のみ。

use super::lane::{DEFAULT_POLICY_VERSION, RoutingMode};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `task_routing_overrides` の 1 行（作成時に固定・以後不変。`POST /tasks` の `routing_override`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingOverride {
    /// この task の lane 層の mode（config の `[routing] mode` より優先）。
    pub mode: RoutingMode,
    /// 下段（fixed 方策・lane→model 対応表）の版。省略時は `DEFAULT_POLICY_VERSION`。現行の版以外は受理しない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lower_policy: Option<String>,
    /// 根拠表の扱い（ADR 2026-10-17-evidence-mode-none-pinned-latest）。欄なしの旧 JSON は
    /// `evidence_table_id` あり→`pinned`、なし→`latest` と読む。
    #[serde(default)]
    pub evidence_mode: EvidenceMode,
    /// 根拠表 `routing_evidence` の版（`evidence_version`）。`pinned` のとき必須で、この版だけを読む。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_table_id: Option<String>,
    /// run の結果を `routing_outcome_recorded`（オンライン更新の根拠）へ追記するか。既定 true（従来）。
    #[serde(default = "default_true")]
    pub online_update: bool,
}

fn default_true() -> bool {
    true
}

/// 根拠表の扱い。`none` は参照しない・`pinned` は指定版のみ・`latest` は通常運用の最新版。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceMode {
    None,
    Pinned,
    #[default]
    Latest,
}

impl EvidenceMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Pinned => "pinned",
            Self::Latest => "latest",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRoutingOverride {
    mode: RoutingMode,
    #[serde(default)]
    lower_policy: Option<String>,
    #[serde(default)]
    evidence_mode: Option<EvidenceMode>,
    #[serde(default)]
    evidence_table_id: Option<String>,
    #[serde(default = "default_true")]
    online_update: bool,
}

impl<'de> Deserialize<'de> for RoutingOverride {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let r = RawRoutingOverride::deserialize(d)?;
        // 旧データ・旧設定: 欄なしは id の有無で決める（id あり→pinned、なし→latest）。
        let evidence_mode = r.evidence_mode.unwrap_or(if r.evidence_table_id.is_some() {
            EvidenceMode::Pinned
        } else {
            EvidenceMode::Latest
        });
        Ok(Self {
            mode: r.mode,
            lower_policy: r.lower_policy,
            evidence_mode,
            evidence_table_id: r.evidence_table_id,
            online_update: r.online_update,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RoutingOverrideError {
    #[error(
        "routing_override.lower_policy {0:?} is not a known policy version (known: {DEFAULT_POLICY_VERSION})"
    )]
    UnknownLowerPolicy(String),
    #[error("routing_override.evidence_table_id must not be empty")]
    EmptyEvidenceTable,
    #[error("routing_override.evidence_mode=none must not set evidence_table_id")]
    NoneWithEvidenceTable,
    #[error("routing_override.evidence_mode=pinned requires a non-empty evidence_table_id")]
    PinnedWithoutEvidenceTable,
    #[error("routing_override.evidence_mode=latest must not set evidence_table_id")]
    LatestWithEvidenceTable,
}

impl RoutingOverride {
    /// 作成時の検査。未知の下段版・空の根拠表 id は拒否する（黙って最新・既定へ倒さない）。
    pub fn validate(&self) -> Result<(), RoutingOverrideError> {
        if let Some(p) = &self.lower_policy
            && p != DEFAULT_POLICY_VERSION
        {
            return Err(RoutingOverrideError::UnknownLowerPolicy(p.clone()));
        }
        let id = self.evidence_table_id.as_deref();
        if id.is_some_and(|id| id.trim().is_empty()) && self.evidence_mode != EvidenceMode::Pinned {
            return Err(RoutingOverrideError::EmptyEvidenceTable);
        }
        match (self.evidence_mode, id) {
            (EvidenceMode::None, Some(_)) => Err(RoutingOverrideError::NoneWithEvidenceTable),
            (EvidenceMode::Latest, Some(_)) => Err(RoutingOverrideError::LatestWithEvidenceTable),
            (EvidenceMode::Pinned, None) => Err(RoutingOverrideError::PinnedWithoutEvidenceTable),
            (EvidenceMode::Pinned, Some(i)) if i.trim().is_empty() => {
                Err(RoutingOverrideError::PinnedWithoutEvidenceTable)
            }
            _ => Ok(()),
        }
    }
}

/// 解決の出自。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModeSource {
    Task,
    Config,
}

// 解決結果（`celeris.route-decision/1` の `routing_resolved` に載る）。
//
// `evidence_mode` の無い旧 event（9c4af7f9 より前に追記された `routing_resolved`）は、
// `RoutingOverride` と同じ規則で `evidence_table_id` あり→`pinned`、なし→`latest` と読む
// （ADR 2026-10-18-resolved-routing-evidence-mode-default）。events は書き換えない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ResolvedRouting {
    pub mode: RoutingMode,
    pub source: ModeSource,
    pub lower_policy: String,
    pub evidence_mode: EvidenceMode,
    /// `pinned` のときだけ Some。`None` かつ `evidence_mode=latest` は「最新版を読む」（従来）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_table_id: Option<String>,
    pub online_update: bool,
}

#[derive(Deserialize)]
struct RawResolvedRouting {
    mode: RoutingMode,
    source: ModeSource,
    lower_policy: String,
    #[serde(default)]
    evidence_mode: Option<EvidenceMode>,
    #[serde(default)]
    evidence_table_id: Option<String>,
    online_update: bool,
}

impl<'de> Deserialize<'de> for ResolvedRouting {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let r = RawResolvedRouting::deserialize(d)?;
        let evidence_mode = r.evidence_mode.unwrap_or(if r.evidence_table_id.is_some() {
            EvidenceMode::Pinned
        } else {
            EvidenceMode::Latest
        });
        Ok(Self {
            mode: r.mode,
            source: r.source,
            lower_policy: r.lower_policy,
            evidence_mode,
            evidence_table_id: r.evidence_table_id,
            online_update: r.online_update,
        })
    }
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
            evidence_mode: o.evidence_mode,
            evidence_table_id: o.evidence_table_id.clone(),
            online_update: o.online_update,
        },
        None => ResolvedRouting {
            mode: config_mode,
            source: ModeSource::Config,
            lower_policy: DEFAULT_POLICY_VERSION.to_string(),
            evidence_mode: EvidenceMode::Latest,
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
            evidence_mode: EvidenceMode::Latest,
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

    #[test]
    fn legacy_json_infers_mode_and_round_trips() {
        let o: RoutingOverride = serde_json::from_str(r#"{"mode":"fixed"}"#).unwrap();
        assert_eq!(o.evidence_mode, EvidenceMode::Latest);
        let o: RoutingOverride =
            serde_json::from_str(r#"{"mode":"fixed","evidence_table_id":"ev-1"}"#).unwrap();
        assert_eq!(o.evidence_mode, EvidenceMode::Pinned);
        assert!(o.validate().is_ok());
        let n: RoutingOverride =
            serde_json::from_str(r#"{"mode":"fixed","evidence_mode":"none"}"#).unwrap();
        assert_eq!(n.evidence_mode, EvidenceMode::None);
        let back: RoutingOverride =
            serde_json::from_str(&serde_json::to_string(&n).unwrap()).unwrap();
        assert_eq!(back, n);
        assert_eq!(
            resolve_routing(RoutingMode::Fixed, Some(&n)).evidence_mode,
            EvidenceMode::None
        );
        assert_eq!(
            resolve_routing(RoutingMode::Fixed, None).evidence_mode,
            EvidenceMode::Latest
        );
    }

    #[test]
    fn contradictions_are_rejected() {
        let mk = |m, id: Option<&str>| RoutingOverride {
            evidence_mode: m,
            evidence_table_id: id.map(String::from),
            ..ov(RoutingMode::Fixed)
        };
        assert_eq!(
            mk(EvidenceMode::None, Some("e")).validate(),
            Err(RoutingOverrideError::NoneWithEvidenceTable)
        );
        assert_eq!(
            mk(EvidenceMode::Latest, Some("e")).validate(),
            Err(RoutingOverrideError::LatestWithEvidenceTable)
        );
        assert_eq!(
            mk(EvidenceMode::Pinned, None).validate(),
            Err(RoutingOverrideError::PinnedWithoutEvidenceTable)
        );
        assert_eq!(
            mk(EvidenceMode::Pinned, Some("")).validate(),
            Err(RoutingOverrideError::PinnedWithoutEvidenceTable)
        );
        assert!(mk(EvidenceMode::None, None).validate().is_ok());
        assert!(mk(EvidenceMode::Pinned, Some("e")).validate().is_ok());
        assert!(mk(EvidenceMode::Latest, None).validate().is_ok());
    }

    #[test]
    fn resolved_routing_without_evidence_mode_follows_the_override_rule() {
        let base = serde_json::json!({
            "mode": "fixed", "source": "task", "lower_policy": "lane-policy/1", "online_update": true
        });
        let latest: ResolvedRouting = serde_json::from_value(base.clone()).unwrap();
        assert_eq!(latest.evidence_mode, EvidenceMode::Latest);
        let mut pinned = base.clone();
        pinned["evidence_table_id"] = "ev-1".into();
        let pinned: ResolvedRouting = serde_json::from_value(pinned).unwrap();
        assert_eq!(pinned.evidence_mode, EvidenceMode::Pinned);
        let mut none = base;
        none["evidence_mode"] = "none".into();
        let none: ResolvedRouting = serde_json::from_value(none).unwrap();
        assert_eq!(none.evidence_mode, EvidenceMode::None);
    }
}
