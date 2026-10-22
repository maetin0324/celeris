//! The routing decision record (`celeris.route-decision/1`).
//!
//! ADR 2026-10-11-task-routing-features D5 fixes the shape: `input` (features snapshot reference,
//! context, constraints, candidates with their evidence values), `output` (selected, action,
//! scores, rule, propensity) and `judge` (the deterministic rule selector). The same value is
//! written to `route_decisions` and carried on the `routing_decided` event, so a decision has one
//! record and one identity (`decision_id`).
use super::policy::PolicyOutput;
use super::stage::RoutingStage;
use crate::llm_route::{Candidate, CandidateKey, CandidateRejection, ExclusionReason};
use crate::model_policy::RouteFeaturesRef;
use crate::route_record::{
    Candidate as RecordCandidate, ROUTE_DECISION_SCHEMA, RouteOptions, candidates_one_shot,
    option_id, options_hash,
};
use crate::routing_evidence::EvidenceRow;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// ADR D5 `judge.id` / `judge.version` of the deterministic evidence selector.
pub const ROUTE_JUDGE_ID: &str = "evidence-policy";
pub const ROUTE_JUDGE_VERSION: &str = "routing-policy/1";
/// ADR D5: this selector only picks a source×model, so the action is always `execute`.
pub const ROUTE_ACTION_EXECUTE: &str = "execute";

/// How the selection was made: which rule the stage armed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RouteAction {
    /// `shadow`: `route_defaults` decided; the evidence pick is recorded but not executed.
    Shadow,
    /// `enforce`: the evidence table decided (with ε exploration).
    Enforce,
    /// The task named the model explicitly; neither table decided.
    Explicit,
}

/// ADR D5 `output.rule`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RouteRule {
    Evidence,
    Explore,
    RouteDefaults,
    ModelPin,
}

/// One considered candidate with the evidence values the decision read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RouteCandidate {
    pub source_id: String,
    pub model_id: String,
    pub harness_id: String,
    pub label: String,
    /// 1-based rank in the harness×lane `route_defaults` list; `None` when not declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_default_rank: Option<u64>,
    /// Why the candidate never reached the selection rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excluded: Option<ExclusionReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<EvidenceSnapshot>,
}

/// The evidence row values a candidate was judged on (ADR D5 `candidates[].evidence`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EvidenceSnapshot {
    pub evidence_version: String,
    pub n: u64,
    pub pass: u64,
    pub pass_lower: f64,
    pub pass_upper: f64,
    pub wall_p90_s: Option<f64>,
    pub usd_per_pass: Option<f64>,
    #[serde(default)]
    pub quota_cost_per_pass: Option<f64>,
    pub quota_per_run: BTreeMap<String, f64>,
    pub timeout_rate: f64,
}

impl From<&EvidenceRow> for EvidenceSnapshot {
    fn from(row: &EvidenceRow) -> Self {
        Self {
            evidence_version: row.evidence_version.clone(),
            n: row.n,
            pass: row.pass,
            pass_lower: row.pass_lower,
            pass_upper: row.pass_upper,
            wall_p90_s: row.wall_p90_s,
            usd_per_pass: row.usd_per_pass,
            quota_cost_per_pass: row.quota_cost_per_pass,
            quota_per_run: row.quota_per_run.clone(),
            timeout_rate: row.timeout_rate,
        }
    }
}

/// A candidate the evidence rule dropped, with the reason (ADR D2 trace).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RouteExclusion {
    pub candidate: CandidateKey,
    pub reason: ExclusionReason,
}

/// ADR D5 `output.scores[]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RouteScore {
    pub option_id: String,
    pub p_success: f64,
    /// 根拠表に定価が無い候補は `None`。
    pub expected_cost_usd: Option<f64>,
    pub rank: u64,
}

/// ADR D5 `judge`: the rule selector is deterministic and has no model of its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RouteJudge {
    pub kind: String,
    pub id: String,
    pub version: String,
    pub deterministic: bool,
}

impl Default for RouteJudge {
    fn default() -> Self {
        Self {
            kind: "rule".into(),
            id: ROUTE_JUDGE_ID.into(),
            version: ROUTE_JUDGE_VERSION.into(),
            deterministic: true,
        }
    }
}

/// harness の試行順の並べ替え（ADR 2026-10-22 D1）の入力と結果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessOrderRecord {
    pub execution_class: String,
    /// クラスが `harness_balance_classes` にあるか。
    pub enabled: bool,
    /// guardrail が止めているか（止まっていれば宣言順）。
    pub tripped: bool,
    pub tolerance: f64,
    /// 宣言順。
    pub declared: Vec<String>,
    /// harness → 残量スコア（ADR-0024 D3 の最良 account）。不明は欠ける。
    pub scores: BTreeMap<String, f64>,
    /// 並べ替えの結果。
    pub ordered: Vec<String>,
    /// 結果の先頭が宣言順の先頭と違う。
    pub switched: bool,
}

/// One routing decision: the record written to `route_decisions` and carried on `routing_decided`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RouteDecision {
    pub schema: String,
    pub decision_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub subject_kind: String,
    pub subject_id: String,
    /// Identity of the content-addressed task features snapshot (`celeris.task-features/1`) this
    /// decision was made on. Flat (not a `RouteFeaturesRef`) so the ref can carry the decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features_schema: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features_template: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features_hash: Option<String>,
    pub genre: String,
    pub harness_id: String,
    pub harness_version: String,
    pub stage: RoutingStage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_pin: Option<String>,
    /// Remaining quota per source at the decision point (absolute units; `0` = exhausted).
    pub quota_remaining: BTreeMap<String, f64>,
    pub candidates: Vec<RouteCandidate>,
    /// Candidates the evidence table could not separate (ADR D2 同等集合).
    pub equivalent: Vec<CandidateKey>,
    pub exclusions: Vec<RouteExclusion>,
    /// `None` when every candidate was refused (fail closed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<CandidateKey>,
    pub action: RouteAction,
    pub rule: RouteRule,
    /// Probability that the selected candidate was chosen (includes ε exploration; 1.0 without it).
    pub propensity: f64,
    pub scores: Vec<RouteScore>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_version: Option<String>,
    /// Content address of the `celeris.route-options/1` record of this decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options_hash: Option<String>,
    pub judge: RouteJudge,
    /// decision-assisted の lane 層の記録（ADR 2026-10-15-mlsys-min-routing-policy。`fixed` では書かない）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane_assist: Option<super::assist::LaneAssistRecord>,
    /// harness の試行順を残量で並べ替えた記録（ADR 2026-10-22 D1。並べ替えの対象クラスのときだけ）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_order: Option<HarnessOrderRecord>,
    /// task 欄（`routing_override`）で固定した routing の解決結果。task 欄の無い task には書かない（従来と同一）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing_resolved: Option<super::task_override::ResolvedRouting>,
}

impl RouteDecision {
    /// The `celeris.route-options/1` options of this decision (one option per candidate).
    pub fn route_options(&self) -> RouteOptions {
        candidates_one_shot(
            &format!(
                "{}/{} の source×model（{}）",
                self.genre, self.harness_id, self.stage
            ),
            &self
                .candidates
                .iter()
                .map(|c| RecordCandidate {
                    source_id: c.source_id.clone(),
                    model_id: c.model_id.clone(),
                    harness_id: c.harness_id.clone(),
                    label: c.label.clone(),
                })
                .collect::<Vec<_>>(),
        )
    }

    /// ADR D5 `input`: what the rule saw (features reference, context, constraints, candidates).
    pub fn input_json(&self) -> serde_json::Value {
        let options = self.route_options();
        serde_json::json!({
            "features": self.features_hash.as_ref().map(|hash| serde_json::json!({
                "schema": self.features_schema,
                "template": self.features_template,
                "hash": hash,
            })),
            "context": {
                "harness_id": self.harness_id,
                "harness_version": self.harness_version,
                "genre": self.genre,
                "stage": self.stage,
            },
            "constraints": {
                "lane": self.lane,
                "model_pin": self.model_pin,
                "quota_remaining": self.quota_remaining,
            },
            "candidates": self.candidates,
            "options": {
                "schema": options.schema,
                "builder": options.builder,
                "hash": self.options_hash,
            },
        })
    }

    /// ADR D5 `output`: what the rule chose.
    pub fn output_json(&self) -> serde_json::Value {
        serde_json::json!({
            "selected": self.selected.as_ref().map(|key| serde_json::json!({
                "source_id": key.source,
                "model_id": key.model,
            })),
            "action": ROUTE_ACTION_EXECUTE,
            "scores": self.scores,
            "rule": self.rule,
            "propensity": self.propensity,
        })
    }

    /// ADR D5 `judge`.
    pub fn judge_json(&self) -> serde_json::Value {
        serde_json::to_value(&self.judge).unwrap_or_else(|_| serde_json::json!({"kind": "rule"}))
    }
}

/// Everything the record needs, captured at the decision point (no I/O, no clock).
#[derive(Debug, Clone)]
pub struct RouteDecisionInput<'a> {
    pub decision_id: &'a str,
    pub run_id: Option<&'a str>,
    pub subject_kind: &'a str,
    pub subject_id: &'a str,
    pub features: Option<&'a RouteFeaturesRef>,
    pub genre: &'a str,
    pub harness_id: &'a str,
    pub harness_version: &'a str,
    pub stage: RoutingStage,
    pub lane: Option<&'a str>,
    pub model_pin: Option<&'a str>,
    pub quota_remaining: &'a BTreeMap<String, f64>,
    /// Candidates that reached the selection rule, in the order the rule considered them.
    pub considered: &'a [Candidate],
    /// Candidates dropped before the rule (unknown source, incompatible, quota, cooldown, …).
    pub exclusions: &'a [CandidateRejection],
    pub evidence: &'a [EvidenceRow],
    pub route_defaults: &'a [CandidateKey],
    /// `None` when nothing was chosen (fail closed).
    pub output: Option<&'a PolicyOutput>,
}

/// Build the one record of a routing decision. Pure: same inputs give the same bytes.
pub fn build_route_decision(input: &RouteDecisionInput<'_>) -> RouteDecision {
    let harness_id = input.harness_id.to_owned();
    let excluded = |key: &CandidateKey| {
        input
            .exclusions
            .iter()
            .find(|r| &r.candidate.key() == key)
            .map(|r| r.reason.clone())
    };
    let evidence_for = |key: &CandidateKey| {
        input
            .evidence
            .iter()
            .find(|row| {
                row.key.lane == input.lane.unwrap_or_default()
                    && row.key.source_id == key.source.as_ref()
                    && row.key.model_id == key.model.as_ref()
            })
            .map(EvidenceSnapshot::from)
    };
    let mut considered: Vec<CandidateKey> = input
        .considered
        .iter()
        .map(Candidate::key)
        .filter(|key| excluded(key).is_none())
        .collect();
    // Refused candidates are part of the record too (their exclusion reason is the evidence).
    for rejection in input.exclusions {
        let key = rejection.candidate.key();
        if !considered.contains(&key) {
            considered.push(key);
        }
    }
    let candidates = considered
        .iter()
        .map(|key| {
            let label = format!("{}/{}", key.source, key.model);
            RouteCandidate {
                source_id: key.source.to_string(),
                model_id: key.model.to_string(),
                harness_id: harness_id.clone(),
                label,
                route_default_rank: input
                    .route_defaults
                    .iter()
                    .position(|k| k == key)
                    .map(|i| (i + 1) as u64),
                excluded: excluded(key),
                evidence: evidence_for(key),
            }
        })
        .collect::<Vec<_>>();
    let exclusions = input
        .exclusions
        .iter()
        .map(|r| RouteExclusion {
            candidate: r.candidate.key(),
            reason: r.reason.clone(),
        })
        .collect::<Vec<_>>();
    let selected = input.output.map(|o| o.selected.key());
    let pinned = input.model_pin.is_some();
    let explored = input
        .output
        .is_some_and(|o| o.equivalent.len() > 1 && Some(&o.selected.key()) != o.equivalent.first());
    let rule = if pinned {
        RouteRule::ModelPin
    } else {
        match input.output.map(|o| o.source) {
            Some(super::policy::SelectionSource::Evidence) if explored => RouteRule::Explore,
            Some(super::policy::SelectionSource::FrontierLane) if explored => RouteRule::Explore,
            Some(super::policy::SelectionSource::Evidence) => RouteRule::Evidence,
            _ => RouteRule::RouteDefaults,
        }
    };
    let action = if pinned {
        RouteAction::Explicit
    } else {
        match input.stage {
            RoutingStage::Shadow => RouteAction::Shadow,
            RoutingStage::Enforce => RouteAction::Enforce,
        }
    };
    // ADR D5: `route_defaults` decisions carry no scores, evidence decisions carry the table's
    // point estimate, cost and rank of every candidate the rule ordered.
    let scores = match (input.output, rule) {
        (Some(output), RouteRule::Evidence | RouteRule::Explore) => {
            let mut order: Vec<CandidateKey> = vec![output.selected.key()];
            order.extend(output.ranked_fallbacks.iter().map(Candidate::key));
            order
                .into_iter()
                .enumerate()
                .filter_map(|(rank, key)| {
                    let row = input.evidence.iter().find(|row| {
                        row.key.lane == input.lane.unwrap_or_default()
                            && row.key.source_id == key.source.as_ref()
                            && row.key.model_id == key.model.as_ref()
                    })?;
                    Some(RouteScore {
                        option_id: option_id(&format!("{}/{}", key.source, key.model)),
                        p_success: if row.n == 0 {
                            0.0
                        } else {
                            row.pass as f64 / row.n as f64
                        },
                        expected_cost_usd: row.usd_per_pass,
                        rank: (rank + 1) as u64,
                    })
                })
                .collect()
        }
        _ => Vec::new(),
    };
    let evidence_version = input.output.and_then(|o| o.evidence_version.clone());
    let propensity = input.output.map_or(1.0, |o| o.propensity);
    let equivalent = input
        .output
        .map(|o| o.equivalent.clone())
        .unwrap_or_default();
    let mut decision = RouteDecision {
        schema: ROUTE_DECISION_SCHEMA.into(),
        decision_id: input.decision_id.to_owned(),
        run_id: input.run_id.map(str::to_owned),
        subject_kind: input.subject_kind.to_owned(),
        subject_id: input.subject_id.to_owned(),
        features_schema: input.features.map(|f| f.schema.clone()),
        features_template: input.features.map(|f| f.template.clone()),
        features_hash: input.features.map(|f| f.hash.clone()),
        genre: input.genre.to_owned(),
        harness_id,
        harness_version: input.harness_version.to_owned(),
        stage: input.stage,
        lane: input.lane.map(str::to_owned),
        model_pin: input.model_pin.map(str::to_owned),
        quota_remaining: input.quota_remaining.clone(),
        candidates,
        equivalent,
        exclusions,
        selected,
        action,
        rule,
        propensity,
        scores,
        evidence_version,
        options_hash: None,
        judge: RouteJudge::default(),
        lane_assist: None,
        harness_order: None,
        routing_resolved: None,
    };
    decision.options_hash = Some(options_hash(&decision.route_options()));
    decision
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_route::{ApiFormat, ModelId, SourceId};
    use crate::routing_evidence::EvidenceKey;

    fn candidate(source: &str, model: &str) -> Candidate {
        Candidate {
            source: SourceId::from(source),
            model: ModelId::from(model),
            api_format: ApiFormat::from("openai"),
        }
    }

    fn row(source: &str, model: &str, pass: u64, n: u64, usd: f64) -> EvidenceRow {
        EvidenceRow {
            quota_cost_per_pass: None,
            key: EvidenceKey {
                lane: "standard".into(),
                genre: "coding".into(),
                harness_id: "claude_code".into(),
                harness_version: "2.4.0".into(),
                source_id: source.into(),
                model_id: model.into(),
            },
            evidence_version: "evidence/2026-10-11".into(),
            n,
            pass,
            pass_lower: pass as f64 / n as f64,
            pass_upper: 1.0,
            pass_boot_lower: pass as f64 / n as f64,
            pass_boot_upper: 1.0,
            wall_p90_s: Some(600.0),
            usd_per_pass: Some(usd),
            quota_per_run: [("week".to_string(), 0.01)].into(),
            timeout_rate: 0.0,
            low_confidence: false,
        }
    }

    static QUOTA: BTreeMap<String, f64> = BTreeMap::new();

    fn key(source: &str, model: &str) -> CandidateKey {
        CandidateKey {
            source: source.into(),
            model: model.into(),
        }
    }

    fn base<'a>(
        considered: &'a [Candidate],
        evidence: &'a [EvidenceRow],
        output: Option<&'a PolicyOutput>,
        defaults: &'a [CandidateKey],
    ) -> RouteDecisionInput<'a> {
        RouteDecisionInput {
            decision_id: "run:r1",
            run_id: Some("r1"),
            subject_kind: "task",
            subject_id: "t1",
            features: None,
            genre: "coding",
            harness_id: "claude_code",
            harness_version: "2.1.0",
            stage: RoutingStage::Enforce,
            lane: Some("standard"),
            model_pin: None,
            quota_remaining: &QUOTA,
            considered,
            exclusions: &[],
            evidence,
            route_defaults: defaults,
            output,
        }
    }

    #[test]
    fn evidence_decision_carries_scores_rule_and_options_hash() {
        let a = candidate("a", "m");
        let b = candidate("b", "m");
        let considered = vec![a.clone(), b.clone()];
        let evidence = vec![row("a", "m", 21, 30, 1.9), row("b", "m", 18, 30, 0.4)];
        let output = PolicyOutput {
            selected: b.clone(),
            ranked_fallbacks: vec![a.clone()],
            equivalent: vec![b.key(), a.key()],
            source: super::super::policy::SelectionSource::Evidence,
            propensity: 0.9,
            evidence_version: Some("evidence/2026-10-11".into()),
        };
        let defaults = vec![key("a", "m"), key("b", "m")];
        let decision =
            build_route_decision(&base(&considered, &evidence, Some(&output), &defaults));
        assert_eq!(decision.schema, ROUTE_DECISION_SCHEMA);
        assert_eq!(decision.rule, RouteRule::Evidence);
        assert_eq!(decision.action, RouteAction::Enforce);
        assert_eq!(decision.selected, Some(b.key()));
        assert_eq!(decision.propensity, 0.9);
        assert_eq!(
            decision.evidence_version.as_deref(),
            Some("evidence/2026-10-11")
        );
        // The chosen candidate ranks first; the point estimate and cost come from the table.
        assert_eq!(decision.scores[0].rank, 1);
        assert!((decision.scores[0].p_success - 0.6).abs() < 1e-9);
        assert_eq!(decision.scores[0].expected_cost_usd, Some(0.4));
        assert_eq!(decision.scores[1].option_id, option_id("a/m"));
        assert_eq!(decision.candidates.len(), 2);
        assert_eq!(decision.candidates[0].route_default_rank, Some(1));
        assert!(
            decision
                .options_hash
                .as_deref()
                .is_some_and(|h| h.starts_with("sha256:"))
        );
        assert_eq!(
            decision.options_hash,
            Some(options_hash(&decision.route_options()))
        );
        let value = decision.input_json();
        assert_eq!(value["context"]["stage"], "enforce");
        assert_eq!(value["constraints"]["lane"], "standard");
        assert_eq!(
            value["candidates"][0]["evidence"]["evidence_version"],
            "evidence/2026-10-11"
        );
        let output_value = decision.output_json();
        assert_eq!(output_value["action"], ROUTE_ACTION_EXECUTE);
        assert_eq!(output_value["rule"], "evidence");
        assert_eq!(output_value["selected"]["source_id"], "b");
        assert_eq!(decision.judge_json()["deterministic"], true);
    }

    #[test]
    fn exploration_and_route_defaults_records_and_refusals() {
        let a = candidate("a", "m");
        let considered = vec![a.clone()];
        // ε exploration picked a candidate that is not the rule's first choice.
        let explored = PolicyOutput {
            selected: a.clone(),
            ranked_fallbacks: vec![],
            equivalent: vec![
                CandidateKey {
                    source: "b".into(),
                    model: "m".into(),
                },
                a.key(),
            ],
            source: super::super::policy::SelectionSource::Evidence,
            propensity: 0.05,
            evidence_version: Some("evidence/1".into()),
        };
        let defaults = vec![key("a", "m"), key("b", "m")];
        let decision = build_route_decision(&base(&considered, &[], Some(&explored), &defaults));
        assert_eq!(decision.rule, RouteRule::Explore);

        let defaults_out = PolicyOutput {
            selected: a.clone(),
            ranked_fallbacks: vec![],
            equivalent: vec![],
            source: super::super::policy::SelectionSource::RouteDefaults,
            propensity: 1.0,
            evidence_version: None,
        };
        let mut input = base(&considered, &[], Some(&defaults_out), &defaults);
        input.stage = RoutingStage::Shadow;
        let shadow = build_route_decision(&input);
        assert_eq!(shadow.rule, RouteRule::RouteDefaults);
        assert_eq!(shadow.action, RouteAction::Shadow);
        assert!(shadow.scores.is_empty());
        assert_eq!(shadow.propensity, 1.0);
        assert_eq!(shadow.input_json()["context"]["stage"], "shadow");

        // A pin overrides both tables; a refusal keeps the exclusion reasons and selects nothing.
        input.model_pin = Some("m");
        let pinned = build_route_decision(&input);
        assert_eq!(pinned.rule, RouteRule::ModelPin);
        assert_eq!(pinned.action, RouteAction::Explicit);

        input.model_pin = None;
        input.output = None;
        let refusals = [CandidateRejection {
            candidate: a.clone(),
            reason: ExclusionReason::Cooldown,
        }];
        input.exclusions = &refusals;
        let refused = build_route_decision(&input);
        assert_eq!(refused.selected, None);
        assert_eq!(refused.exclusions[0].reason, ExclusionReason::Cooldown);
        assert_eq!(
            refused.candidates[0].excluded,
            Some(ExclusionReason::Cooldown)
        );
        assert_eq!(refused.propensity, 1.0);
        assert!(refused.output_json()["selected"].is_null());
    }
}
