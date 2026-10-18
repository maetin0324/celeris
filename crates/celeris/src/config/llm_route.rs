use super::ConfigError;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

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
    /// ADR 2026-10-15-mlsys-min-decision-assisted D4: `true` なら、`credential_ref` の秘密が未登録の間、
    /// この source へ向かう work unit だけを `waiting_for_auth` で待たせる（受信箱に登録の要求を出す）。
    #[serde(default)]
    pub wait_for_credential: bool,
    /// 受信箱に出す用途の 1 行（既定は「source <id> の API キー」）。
    #[serde(default)]
    pub credential_purpose: Option<String>,
}
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessDeclarationConfig {
    pub kind: String,
    #[serde(default)]
    pub model_namespace: Option<String>,
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
    #[serde(default)]
    pub extensions: Vec<PathBuf>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub env_from_secrets: HashMap<String, String>,
    /// `kind = "fake"` only: run the fake command under the adapter environment of this declared harness
    /// (a B/C capability probe stands in for a real harness's worker without calling a model).
    #[serde(default)]
    pub env_of: Option<String>,
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
    /// execution class（`coding` / `planner` / `review`）ごとの候補。宣言した class は
    /// `candidates` の代わりに使い、宣言の無い class は `candidates` に戻る。
    #[serde(default)]
    pub classes: BTreeMap<String, RouteClassConfig>,
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
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteClassConfig {
    pub candidates: Vec<RouteCandidateConfig>,
}
/// `[route_defaults.<harness>].classes` で宣言してよい execution class の名前。
const ROUTE_CLASSES: [&str; 3] = ["coding", "planner", "review"];
/// proxy 経由が必要な harness。候補はすべて `via = "llm_proxy"` の route を要する。
const PROXY_BOUND_HARNESSES: [&str; 3] = ["local-deep-research", "paperqa", "langmem"];

impl super::Config {
    /// `wait_for_credential = true` の source を credential 待ちの宣言（dispatcher の gate）へ。
    /// `credential_ref` が無い宣言は設定エラー（待つ先の秘密 id が無い）。
    pub fn credential_gates(
        &self,
    ) -> Result<task_dispatch::dispatcher::CredentialGates, ConfigError> {
        let mut gates = task_dispatch::dispatcher::CredentialGates {
            secrets_dir: self.secrets.as_ref().map(|s| s.dir.clone()),
            by_source: Default::default(),
        };
        for (id, s) in &self.llm_sources {
            if !s.wait_for_credential {
                continue;
            }
            let Some(secret_id) = s.credential_ref.clone().filter(|v| !v.is_empty()) else {
                return Err(ConfigError::Invalid(format!(
                    "[llm_sources.{id}] wait_for_credential = true requires credential_ref"
                )));
            };
            gates.by_source.insert(
                id.as_str().into(),
                task_dispatch::dispatcher::CredentialGate {
                    secret_id,
                    provider: id.clone(),
                    purpose: s
                        .credential_purpose
                        .clone()
                        .unwrap_or_else(|| format!("source {id} の API キー")),
                },
            );
        }
        Ok(gates)
    }

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
            for (class, c) in &d.classes {
                if !ROUTE_CLASSES.contains(&class.as_str()) {
                    return Err(ConfigError::Invalid(format!(
                        "route_defaults.{h}.classes has unknown execution class {class} (expected coding, planner, or review)"
                    )));
                }
                if c.candidates.is_empty() {
                    return Err(ConfigError::Invalid(format!(
                        "route_defaults.{h}.classes.{class} has no candidates"
                    )));
                }
                r.route_class_defaults
                    .entry(h.as_str().into())
                    .or_default()
                    .insert(
                        class.as_str().into(),
                        c.candidates
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
    for (h, defaults) in &r.route_defaults {
        validate_route_candidates(r, h, defaults, None)?;
    }
    // ADR 2026-10-11-llm-source-model-routing-evaluation D2: the harness×lane columns are checked
    // like the harness-wide column.
    for ((h, _lane), defaults) in &r.lane_route_defaults {
        validate_route_candidates(r, h, defaults, None)?;
    }
    for (h, classes) in &r.route_class_defaults {
        for (class, candidates) in classes {
            validate_route_candidates(r, h, candidates, Some(class))?;
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

/// harness 既定（`class` が None）と class 候補（`class` が Some）を同じ規則で検査する:
/// source・model の宣言、harness の kind・API format（または明示の `[[llm_routes]]`）、proxy 経由の要件。
fn validate_route_candidates(
    r: &task_core::llm_route::Registry,
    h: &task_core::llm_route::HarnessId,
    candidates: &[task_core::llm_route::CandidateKey],
    class: Option<&task_core::llm_route::ExecutionClass>,
) -> Result<(), ConfigError> {
    let scope = match class {
        Some(class) => format!("route_defaults.{h}.classes.{class}"),
        None => format!("route_defaults.{h}"),
    };
    let hs = r.harnesses.get(h).ok_or_else(|| {
        ConfigError::Invalid(format!("route_defaults references undeclared harness {h}"))
    })?;
    let mut seen = BTreeSet::new();
    for c in candidates {
        if !seen.insert((c.source.clone(), c.model.clone())) {
            return Err(ConfigError::Invalid(format!(
                "duplicate route candidate in {scope}"
            )));
        }
        let s = r.sources.get(&c.source).ok_or_else(|| {
            ConfigError::Invalid(format!("{scope} references undeclared source {}", c.source))
        })?;
        if !s.models.contains(&c.model) {
            return Err(ConfigError::Invalid(format!(
                "{scope} references undeclared model {}",
                c.model
            )));
        }
        let compatible = hs.source_kinds.contains(&s.kind)
            && hs.api_formats.contains(&s.api_format)
            || r.llm_routes.iter().any(|x| x.harness == *h && x.key == *c);
        if !compatible {
            return Err(ConfigError::Invalid(format!(
                "{scope} candidate {}/{} has no compatible llm_routes entry",
                c.source, c.model
            )));
        }
        if PROXY_BOUND_HARNESSES.contains(&h.0.as_str())
            && !r.llm_routes.iter().any(|route| {
                route.harness == *h && route.key == *c && route.via.as_deref() == Some("llm_proxy")
            })
        {
            return Err(ConfigError::Invalid(format!(
                "proxy-bound harness {h} candidate {}/{} requires via = llm_proxy",
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
    fn wait_for_credential_builds_a_gate_and_requires_a_credential_ref() {
        let text = DECLS.replace(
            "models = [\"m1\"]",
            "models = [\"m1\"]\ncredential_ref = \"jev_api_key\"\nwait_for_credential = true\ncredential_purpose = \"Jev\"",
        );
        let cfg = Config::parse_with_delivery(&text).unwrap();
        let gates = cfg.credential_gates().unwrap();
        let gate = &gates.by_source[&"main".into()];
        assert_eq!(gate.secret_id, "jev_api_key");
        assert_eq!(gate.purpose, "Jev");
        // 既定（false）なら gate は空。待つ先の秘密が無い宣言は設定エラー。
        assert!(
            Config::parse_with_delivery(DECLS)
                .unwrap()
                .credential_gates()
                .unwrap()
                .by_source
                .is_empty()
        );
        let bad = DECLS.replace(
            "models = [\"m1\"]",
            "models = [\"m1\"]\nwait_for_credential = true",
        );
        assert!(
            Config::parse_with_delivery(&bad)
                .unwrap()
                .credential_gates()
                .is_err()
        );
    }

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

    #[test]
    fn proxy_bound_harness_candidates_all_need_a_via_llm_proxy_route() {
        let text = DECLS
            .replace("[harnesses.cli]", "[harnesses.paperqa]")
            .replace("[route_defaults.cli]", "[route_defaults.paperqa]")
            + "\n[[llm_routes]]\nharness = 'paperqa'\nsource = 'main'\nmodel = 'm1'\nvia = 'direct'\n";
        let cfg = Config::parse_with_delivery(&text).unwrap();
        let err = cfg.llm_route_registry().unwrap_err().to_string();
        assert!(err.contains("requires via = llm_proxy"), "{err}");
    }
}
