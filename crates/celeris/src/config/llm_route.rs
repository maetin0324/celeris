use super::ConfigError;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LlmSourceConfig {
    pub kind: String,
    #[serde(default)]
    pub api_format: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub credential_ref: Option<String>,
    #[serde(default)]
    pub accounts: Vec<String>,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub max_runs: Option<u32>,
    #[serde(default)]
    pub quota: Option<f64>,
}
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessDeclarationConfig {
    pub kind: String,
    #[serde(default)]
    pub api_formats: BTreeSet<String>,
    #[serde(default)]
    pub source_kinds: BTreeSet<String>,
    #[serde(default)]
    pub capabilities: BTreeSet<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteCandidateConfig {
    pub source: String,
    pub model: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LlmRouteConfig {
    pub harness: String,
    pub source: String,
    pub model: String,
    #[serde(default)]
    pub via: Option<String>,
    #[serde(default)]
    pub from_api: Option<String>,
    #[serde(default)]
    pub to_api: Option<String>,
}
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteDefaultConfig {
    #[serde(default)]
    pub candidates: Vec<RouteCandidateConfig>,
    #[serde(default)]
    pub lanes: BTreeMap<String, RouteLaneDefaultConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteLaneDefaultConfig {
    #[serde(default)]
    pub candidates: Vec<RouteCandidateConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteSelectionConfig {
    #[serde(default = "default_epsilon")]
    pub exploration_epsilon: f64,
    #[serde(default = "default_window")]
    pub guardrail_window: u64,
    #[serde(default = "default_quota_floor")]
    pub guardrail_quota_floor: f64,
    #[serde(default)]
    pub evaluation_reservations: BTreeMap<String, u32>,
}
fn default_epsilon() -> f64 {
    0.1
}
fn default_window() -> u64 {
    50
}
fn default_quota_floor() -> f64 {
    0.1
}
impl Default for RouteSelectionConfig {
    fn default() -> Self {
        Self {
            exploration_epsilon: default_epsilon(),
            guardrail_window: default_window(),
            guardrail_quota_floor: default_quota_floor(),
            evaluation_reservations: BTreeMap::new(),
        }
    }
}

impl super::Config {
    /// `[llm_sources.<id>]` に `kind` がこの source の行があるか。
    pub fn declares_source_kind(&self, kind: task_core::LlmSourceRef) -> bool {
        self.llm_sources.values().any(|s| s.kind == kind.as_str())
    }

    pub fn llm_route_registry(&self) -> Result<task_core::llm_route::Registry, ConfigError> {
        use task_core::llm_route::*;
        let mut r = Registry {
            harness_selection: self.harness_selection.clone(),
            route_selection: task_core::llm_route::RouteSelectionSettings {
                exploration_epsilon: self.route_selection.exploration_epsilon,
                guardrail_window: self.route_selection.guardrail_window,
                guardrail_quota_floor: self.route_selection.guardrail_quota_floor,
                evaluation_reservations: self
                    .route_selection
                    .evaluation_reservations
                    .iter()
                    .map(|(s, n)| (s.as_str().into(), *n))
                    .collect(),
            },
            ..Default::default()
        };
        for (id, s) in &self.llm_sources {
            let format = s.api_format.clone().unwrap_or_else(|| s.kind.clone());
            r.sources.insert(
                id.as_str().into(),
                SourceSpec {
                    kind: s.kind.as_str().into(),
                    api_format: format.as_str().into(),
                    accounts: s.accounts.iter().cloned().map(Into::into).collect(),
                    models: s.models.iter().cloned().map(Into::into).collect(),
                    max_runs: s.max_runs,
                    account_max_runs: BTreeMap::new(),
                    quota: s.quota,
                    endpoint: s.endpoint.clone(),
                    credential_ref: s.credential_ref.clone(),
                },
            );
        }
        for (id, h) in &self.harness_declarations {
            r.harnesses.insert(
                id.as_str().into(),
                HarnessSpec {
                    kind: h.kind.clone(),
                    api_formats: h.api_formats.iter().cloned().map(Into::into).collect(),
                    source_kinds: h.source_kinds.iter().cloned().map(Into::into).collect(),
                    capabilities: h.capabilities.clone(),
                    command: h.command.clone(),
                    args: h.args.clone(),
                },
            );
        }
        for x in &self.llm_routes {
            r.llm_routes.push(LlmRoute {
                harness: x.harness.as_str().into(),
                key: CandidateKey {
                    source: x.source.as_str().into(),
                    model: x.model.as_str().into(),
                },
                via: x.via.clone(),
                from_api: x.from_api.as_deref().map(Into::into),
                to_api: x.to_api.as_deref().map(Into::into),
            });
        }
        for (h, d) in &self.route_defaults {
            r.route_defaults.insert(
                h.as_str().into(),
                d.candidates
                    .iter()
                    .map(|c| CandidateKey {
                        source: c.source.as_str().into(),
                        model: c.model.as_str().into(),
                    })
                    .collect(),
            );
            for (lane, defaults) in &d.lanes {
                r.lane_route_defaults.insert(
                    (h.as_str().into(), lane.clone()),
                    defaults
                        .candidates
                        .iter()
                        .map(|c| CandidateKey {
                            source: c.source.as_str().into(),
                            model: c.model.as_str().into(),
                        })
                        .collect(),
                );
            }
        }
        r.cos_route_candidates = self
            .cos
            .route_candidates
            .iter()
            .map(|c| CandidateKey {
                source: c.source.as_str().into(),
                model: c.model.as_str().into(),
            })
            .collect();
        validate_registry(&r)?;
        Ok(r)
    }
}
fn validate_registry(r: &task_core::llm_route::Registry) -> Result<(), ConfigError> {
    if !(0.0..=1.0).contains(&r.route_selection.exploration_epsilon)
        || !r.route_selection.exploration_epsilon.is_finite()
    {
        return Err(ConfigError::Invalid(
            "route_selection.exploration_epsilon must be between 0 and 1".into(),
        ));
    }
    if r.route_selection.guardrail_window == 0
        || !(0.0..=1.0).contains(&r.route_selection.guardrail_quota_floor)
    {
        return Err(ConfigError::Invalid(
            "route_selection guardrail window must be positive and quota floor between 0 and 1"
                .into(),
        ));
    }
    for source in r.route_selection.evaluation_reservations.keys() {
        if !r.sources.contains_key(source) {
            return Err(ConfigError::Invalid(format!(
                "route_selection evaluation_reservations references undeclared source {source}"
            )));
        }
    }
    for rule in &r.harness_selection.rules {
        if !r.harnesses.contains_key(&rule.harness) {
            return Err(ConfigError::Invalid(format!(
                "harness_selection references undeclared harness {}",
                rule.harness
            )));
        }
    }
    for (class, ids) in &r.harness_selection.execution_class_defaults {
        let mut seen = BTreeSet::new();
        for id in ids {
            if !r.harnesses.contains_key(id) {
                return Err(ConfigError::Invalid(format!(
                    "harness_selection default references undeclared harness {id}"
                )));
            }
            if !r.route_defaults.contains_key(id) {
                return Err(ConfigError::Invalid(format!(
                    "harness_selection execution class {class} references harness {id} without route_defaults"
                )));
            }
            if !seen.insert(id) {
                return Err(ConfigError::Invalid(format!(
                    "duplicate harness {id} in execution class {class} defaults"
                )));
            }
        }
    }
    for (h, spec) in &r.harnesses {
        let columns = r.route_defaults.get(h).into_iter().chain(
            r.lane_route_defaults
                .iter()
                .filter(|((id, _), _)| id == h)
                .map(|(_, candidates)| candidates),
        );
        for column in columns {
            let mut seen = BTreeSet::new();
            for candidate in column {
                if !seen.insert((candidate.source.clone(), candidate.model.clone())) {
                    return Err(ConfigError::Invalid(format!(
                        "duplicate route candidate for harness {h}"
                    )));
                }
            }
        }
        let _ = spec;
    }
    for route in &r.llm_routes {
        let h = r.harnesses.get(&route.harness).ok_or_else(|| {
            ConfigError::Invalid(format!(
                "llm_routes references undeclared harness {}",
                route.harness
            ))
        })?;
        let s = r.sources.get(&route.key.source).ok_or_else(|| {
            ConfigError::Invalid(format!(
                "llm_routes references undeclared source {}",
                route.key.source
            ))
        })?;
        if !s.models.contains(&route.key.model) {
            return Err(ConfigError::Invalid(format!(
                "llm_routes references undeclared model {}",
                route.key.model
            )));
        }
        let from = route.from_api.as_ref().unwrap_or(&s.api_format);
        let to = route.to_api.as_ref().unwrap_or(from);
        if !h.source_kinds.contains(&s.kind)
            || (!h.api_formats.contains(from)
                && !(route.via.is_some() && h.api_formats.contains(to)))
        {
            return Err(ConfigError::Invalid(format!(
                "llm_routes harness {} is incompatible with source {} kind/API format",
                route.harness, route.key.source
            )));
        }
    }
    for (h, defaults) in r
        .route_defaults
        .iter()
        .chain(r.lane_route_defaults.iter().map(|((h, _), d)| (h, d)))
    {
        let hs = r.harnesses.get(h).ok_or_else(|| {
            ConfigError::Invalid(format!("route_defaults references undeclared harness {h}"))
        })?;
        for c in defaults {
            let s = r.sources.get(&c.source).ok_or_else(|| {
                ConfigError::Invalid(format!(
                    "route_defaults references undeclared source {}",
                    c.source
                ))
            })?;
            if !s.models.contains(&c.model) {
                return Err(ConfigError::Invalid(format!(
                    "route_defaults references undeclared model {}",
                    c.model
                )));
            }
            let compatible = hs.source_kinds.contains(&s.kind)
                && hs.api_formats.contains(&s.api_format)
                || r.llm_routes.iter().any(|x| x.harness == *h && x.key == *c);
            if !compatible {
                return Err(ConfigError::Invalid(format!(
                    "route_defaults candidate {}/{} has no compatible llm_routes entry",
                    c.source, c.model
                )));
            }
        }
    }
    let cos_harness = task_core::llm_route::HarnessId::from("claude_code");
    for c in &r.cos_route_candidates {
        let s = r.sources.get(&c.source).ok_or_else(|| {
            ConfigError::Invalid(format!(
                "cos.route_candidates references undeclared source {}",
                c.source
            ))
        })?;
        if !s.models.contains(&c.model) {
            return Err(ConfigError::Invalid(format!(
                "cos.route_candidates references undeclared model {}",
                c.model
            )));
        }
        let compatible = r.harnesses.get(&cos_harness).is_some_and(|h| {
            (h.source_kinds.contains(&s.kind) && h.api_formats.contains(&s.api_format))
                || r.llm_routes
                    .iter()
                    .any(|route| route.harness == cos_harness && route.key == *c)
        });
        if !compatible {
            return Err(ConfigError::Invalid(format!(
                "cos.route_candidates candidate {}/{} is incompatible with claude_code",
                c.source, c.model
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::config::Config;

    const DECLS: &str = r#"
[llm_sources.main]
kind = "openai_compatible"
models = ["m1"]
[harnesses.cli]
kind = "acp"
api_formats = ["openai_compatible"]
source_kinds = ["openai_compatible"]
command = "opencode"
args = ["acp"]
[route_defaults.cli]
candidates = [{ source = "main", model = "m1" }]
"#;

    #[test]
    fn llm_sources_harness_decl_builds_registry_from_adr_tables() {
        let cfg = Config::parse_with_delivery(DECLS).unwrap();
        let registry = cfg.llm_route_registry().unwrap();
        assert!(registry.sources.contains_key(&"main".into()));
        assert_eq!(
            registry.harnesses[&"cli".into()].command.as_deref(),
            Some("opencode")
        );
        assert_eq!(
            registry.route_defaults[&"cli".into()][0].model.as_ref(),
            "m1"
        );
    }

    #[test]
    fn lane_route_defaults_override_harness_default_and_settings_load() {
        let text = DECLS.replace("candidates = [{ source = \"main\", model = \"m1\" }]", "candidates = [{ source = \"main\", model = \"m1\" }]\n[route_defaults.cli.lanes.cheap]\ncandidates = [{ source = \"main\", model = \"m1\" }]\n[route_selection]\nexploration_epsilon = 0.2\nguardrail_window = 70\nguardrail_quota_floor = 0.15\nevaluation_reservations = { main = 2 }");
        let cfg = Config::parse_with_delivery(&text).unwrap();
        let registry = cfg.llm_route_registry().unwrap();
        assert_eq!(registry.route_defaults_for(&"cli".into(), "cheap").len(), 1);
        assert_eq!(
            registry.route_defaults_for(&"cli".into(), "frontier").len(),
            1
        );
        assert_eq!(registry.route_selection.exploration_epsilon, 0.2);
        assert_eq!(registry.route_selection.guardrail_window, 70);
        assert_eq!(registry.route_selection.guardrail_quota_floor, 0.15);
        assert_eq!(
            registry.route_selection.evaluation_reservations[&"main".into()],
            2
        );
    }

    #[test]
    fn harness_decl_rejects_unknown_route_source() {
        let cfg = Config::parse_with_delivery(
            &DECLS.replace("source = \"main\"", "source = \"missing\""),
        )
        .unwrap();
        assert!(
            cfg.llm_route_registry()
                .unwrap_err()
                .to_string()
                .contains("undeclared source")
        );
    }

    #[test]
    fn lane_route_defaults_reject_unknown_source_and_settings_reject_bad_values() {
        let text = DECLS.replace("candidates = [{ source = \"main\", model = \"m1\" }]", "candidates = [{ source = \"main\", model = \"m1\" }]\n[route_defaults.cli.lanes.cheap]\ncandidates = [{ source = \"missing\", model = \"m1\" }]");
        let cfg = Config::parse_with_delivery(&text).unwrap();
        assert!(
            cfg.llm_route_registry()
                .unwrap_err()
                .to_string()
                .contains("undeclared source")
        );
        let text = DECLS.replace(
            "[route_defaults.cli]",
            "[route_selection]\nexploration_epsilon = 2.0\n[route_defaults.cli]",
        );
        let cfg = Config::parse_with_delivery(&text).unwrap();
        assert!(
            cfg.llm_route_registry()
                .unwrap_err()
                .to_string()
                .contains("exploration_epsilon")
        );
    }

    #[test]
    fn harness_decl_rejects_kind_or_format_mismatch() {
        let text = DECLS.replace(
            "source_kinds = [\"openai_compatible\"]",
            "source_kinds = [\"claude_oauth\"]",
        );
        let cfg = Config::parse_with_delivery(&text).unwrap();
        assert!(
            cfg.llm_route_registry()
                .unwrap_err()
                .to_string()
                .contains("no compatible")
        );
    }

    #[test]
    fn execution_class_harness_requires_route_defaults() {
        let text = DECLS.replace("[route_defaults.cli]\ncandidates = [{ source = \"main\", model = \"m1\" }]", "[harness_selection]\nrules = []\n[harness_selection.execution_class_defaults]\nplanner = [\"cli\"]");
        let cfg = Config::parse_with_delivery(&text).unwrap();
        assert!(
            cfg.llm_route_registry()
                .unwrap_err()
                .to_string()
                .contains("without route_defaults")
        );
    }
}
