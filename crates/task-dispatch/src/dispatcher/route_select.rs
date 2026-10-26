//! Harness selection and LLM source×model routing at dispatch time
//! (ADR 2026-10-11 llm-source-harness-selection).
//!
//! The order is fixed: the task's `harness` field (a human's explicit choice), otherwise the
//! declared `[harness_selection]` rules; then the candidates in their declared order, which are the
//! `route_class_defaults.<harness>.<class>` list when one is declared for the execution class (it
//! replaces, and never falls back to) otherwise `route_defaults.<harness>`, narrowed by an explicit
//! task model; harness/source compatibility (direct API
//! format or a registered `[[llm_routes]]` entry); `(source, account)` admission; and finally the
//! route policy. Every step fails closed: nothing falls back to an undeclared source.

use super::*;
use std::collections::BTreeMap;
use task_core::llm_route::{
    AccountId, AccountSnapshot, Admission, Candidate, CandidateKey, CandidateRejection,
    ExclusionReason, HarnessId, HarnessRequest, HarnessSelectionError, ModelId, QuotaSnapshot,
    Registry, ReservationSnapshot, RouteRequest, RunSelection, SourceId, admit_source,
    constrain_for_harness, enumerate_candidates, harness_candidates, route_priority,
    select_harness, wire_model_id,
};
use task_core::model_policy::RouteFeaturesRef;
use task_core::route_select::policy::FRONTIER_LANE;
use task_core::route_select::{
    HarnessOrderRecord, PolicyInput, PolicyOutput, RouteDecision, RouteDecisionInput, RoutingStage,
    build_route_decision, order_harnesses, select as select_by_evidence,
};
use task_core::routing_evidence::EvidenceRow;

/// Version of the `selection_metadata_json` / `llm_route_selected.metadata` shape.
pub const ROUTE_SELECTION_VERSION: u32 = 2;

/// Capability a task needs when it carries browser requirements.
pub const BROWSER_CAPABILITY: &str = "browser_context";

/// Mutable state the selection and admission steps read, captured once per decision.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RouteState {
    pub accounts: AccountSnapshot,
    pub quota: QuotaSnapshot,
    pub reservations: ReservationSnapshot,
    /// Per `(source, account)` run limit (stored `account_limits` or the default).
    pub account_max_runs: BTreeMap<(SourceId, AccountId), u32>,
    /// Evidence table snapshot per harness (ADR 2026-10-11 D2). Empty = no evidence yet, so the
    /// `route_defaults` column decides.
    pub evidence: BTreeMap<HarnessId, Vec<EvidenceRow>>,
    /// Stage per harness (ADR D3). A harness without a row is `shadow`.
    pub stages: BTreeMap<HarnessId, RoutingStage>,
    /// Live harness versions; evidence is usable only while the major version agrees (ADR D2).
    pub harness_versions: BTreeMap<HarnessId, String>,
    /// Seed of the deterministic ε exploration (the run id; the task id when there is no run yet).
    pub run_seed: Option<String>,
    /// The one task features snapshot of this dispatch (ADR-task-routing-features D6).
    pub features: Option<RouteFeaturesRef>,
    /// The work unit this dispatch runs, when it runs one (the decision subject).
    pub work_unit_id: Option<String>,
    /// guardrail が止めた実行クラス（`harness_balance_trips`。ADR 2026-10-22 D2）。止まったクラスは宣言順。
    pub balance_tripped: std::collections::BTreeSet<String>,
}

/// One routing request: the harness inputs plus the source/model inputs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteInput {
    pub task_id: String,
    pub harness: HarnessRequest,
    pub model_hint: Option<ModelId>,
    /// Lane label (frontier/standard/cheap): picks the harness×lane `route_defaults` column.
    pub lane: Option<String>,
    /// CoS chat: priority comes from `cos.route_candidates`.
    pub cos: bool,
}

/// Per-dispatch context that is neither registry nor admission state: the run the decision is for
/// (the ε exploration seed), the one task features snapshot of this dispatch, and the work unit
/// (the decision subject). Absent for dry runs and for CoS chat.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RouteContext {
    pub run_id: Option<String>,
    pub features: Option<RouteFeaturesRef>,
    pub work_unit_id: Option<String>,
    /// task 欄 `routing_override.evidence_mode`: `none` は根拠表を一切読まず、`pinned` は
    /// `evidence_version` の版だけ、`latest`（既定）は最新版を読む。
    pub evidence_mode: task_core::route_select::EvidenceMode,
    /// `pinned` のときだけ Some（`routing_override.evidence_table_id`）。
    pub evidence_version: Option<String>,
}

/// The admitted route.
#[derive(Debug, Clone, PartialEq)]
pub struct RoutePick {
    pub harness: HarnessId,
    pub candidate: Candidate,
    /// Model identifier passed to the harness adapter; candidate.model stays canonical.
    pub wire_model_id: ModelId,
    pub account: AccountId,
}

/// Why no route was chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteFailure {
    Harness(HarnessSelectionError),
    /// No `route_defaults.<harness>` (or CoS) candidates are declared.
    NoDeclaredDefault(HarnessId),
    /// The task names a model that no declared candidate offers.
    ModelHintUnmatched(ModelId),
    /// Every declared candidate was rejected (see the trace).
    NoEligible(HarnessId),
}

impl RouteFailure {
    pub fn reason(&self) -> String {
        match self {
            RouteFailure::Harness(e) => format!("harness selection failed: {e}"),
            RouteFailure::NoDeclaredDefault(h) => {
                format!("no route_defaults candidates are declared for harness {h}")
            }
            RouteFailure::ModelHintUnmatched(m) => {
                format!("no declared route candidate offers the requested model {m}")
            }
            RouteFailure::NoEligible(h) => {
                format!("no declared route candidate for harness {h} is eligible")
            }
        }
    }
}

/// What was considered: declared candidates in priority order and why each was dropped.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RouteTrace {
    pub harness: Option<HarnessId>,
    pub candidates: Vec<CandidateKey>,
    pub rejections: Vec<CandidateRejection>,
    pub harness_rejections: Vec<HarnessRejection>,
    /// The `celeris.route-decision/1` record of the harness that decided (ADR 2026-10-11 D2/D5):
    /// stage, rule, equivalent set, exclusion reasons, propensity and evidence version.
    pub decision: Option<RouteDecision>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HarnessRejection {
    pub harness: HarnessId,
    pub reason: String,
    pub rejections: Vec<CandidateRejection>,
}

impl RouteTrace {
    /// Versioned payload for `llm_route_selected.metadata` and `runs.selection_metadata_json`.
    pub fn metadata(
        &self,
        input: &RouteInput,
        decision: Option<&RoutePick>,
        failure: Option<&RouteFailure>,
    ) -> serde_json::Value {
        serde_json::json!({
            "version": ROUTE_SELECTION_VERSION,
            "harness": self.harness,
            "harness_explicit": input.harness.harness.is_some(),
            "cos": input.cos,
            "model_hint": input.model_hint,
            "lane": input.lane,
            // The selector's own record (ADR 2026-10-11 D2/D3): which rule decided, at which
            // stage, with which evidence version and propensity. The full `celeris.route-decision/1`
            // record is on `routing_decided` and in `route_decisions`.
            "route_decision": self.decision.as_ref().map(|d| serde_json::json!({
                "schema": d.schema,
                "decision_id": d.decision_id,
                "stage": d.stage,
                "rule": d.rule,
                "action": d.action,
                "propensity": d.propensity,
                "evidence_version": d.evidence_version,
                "options_hash": d.options_hash,
                "features_hash": d.features_hash,
                "features_schema": d.features_schema,
                "equivalent": d.equivalent,
                // task 欄で固定した routing の解決結果（無ければ null）。campaign の照合が読む。
                "routing_resolved": d.routing_resolved,
                // 根拠表の扱い。none は table 版なし（evidence_version=null）と明示する。
                "evidence_mode": d.routing_resolved.as_ref().map_or("latest", |r| r.evidence_mode.as_str()),
                "evidence_table": d.routing_resolved.as_ref().and_then(|r| (r.evidence_mode == task_core::route_select::EvidenceMode::None).then_some("none")),
            })),
            "candidates": self.candidates,
            "rejections": self.rejections.iter().map(|r| serde_json::json!({
                "source": r.candidate.source,
                "model": r.candidate.model,
                "reason": r.reason,
            })).collect::<Vec<_>>(),
            "harness_rejections": self.harness_rejections.iter().map(|h| serde_json::json!({
                "harness": h.harness,
                "reason": h.reason,
                "rejections": h.rejections.iter().map(|r| serde_json::json!({
                    "source": r.candidate.source,
                    "model": r.candidate.model,
                    "reason": r.reason,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "decision": decision.map(|p| serde_json::json!({
                "source": p.candidate.source,
                "model": p.candidate.model,
                "account": p.account,
            })),
            "failure": failure.map(RouteFailure::reason),
        })
    }
}

/// Pure decision: no I/O, no clock. Same inputs give the same pick.
pub fn route_select(
    input: &RouteInput,
    registry: &Registry,
    state: &RouteState,
) -> (Result<RoutePick, RouteFailure>, RouteTrace) {
    let harnesses = match harness_candidates(&input.harness, registry) {
        Ok(ids) => ids,
        Err(error) => return (Err(RouteFailure::Harness(error)), RouteTrace::default()),
    };
    let (harnesses, harness_order) = order_by_headroom(input, registry, state, harnesses);
    let mut rejected = Vec::new();
    let mut last = None;
    let mut saw_ineligible = None;
    let mut aggregate = RouteTrace::default();
    for harness in harnesses {
        let mut one = input.clone();
        one.harness.harness = Some(harness.clone());
        let (result, current_trace) = route_select_one(&one, registry, state);
        match result {
            Ok(pick) => {
                aggregate.candidates = current_trace.candidates;
                aggregate.rejections.extend(current_trace.rejections);
                aggregate.harness_rejections = rejected;
                aggregate.harness = Some(pick.harness.clone());
                aggregate.decision = current_trace.decision;
                attach_harness_order(&mut aggregate, &harness_order);
                return (Ok(pick), aggregate);
            }
            Err(error) => {
                aggregate
                    .rejections
                    .extend(current_trace.rejections.clone());
                aggregate.decision = current_trace.decision.clone();
                rejected.push(HarnessRejection {
                    harness,
                    reason: error.reason(),
                    rejections: current_trace.rejections,
                });
                if matches!(error, RouteFailure::NoEligible(_)) {
                    saw_ineligible = Some(error.clone());
                }
                last = Some(error);
            }
        }
    }
    aggregate.harness_rejections = rejected;
    attach_harness_order(&mut aggregate, &harness_order);
    let failure = saw_ineligible
        .or(last)
        .unwrap_or(RouteFailure::Harness(HarnessSelectionError::NoneEligible));
    aggregate.harness = aggregate
        .harness_rejections
        .last()
        .map(|r| r.harness.clone());
    (Err(failure), aggregate)
}

/// harness の試行順を残量で並べ替える（ADR 2026-10-22 D1）。対象は `harness_balance_classes` にあるクラスで、
/// guardrail が止めていないときだけ。並べ替えの入力と結果は [`HarnessOrderRecord`] に残す（対象クラスなら
/// 並べ替えが起きなくても残す）。
fn order_by_headroom(
    input: &RouteInput,
    registry: &Registry,
    state: &RouteState,
    declared: Vec<HarnessId>,
) -> (Vec<HarnessId>, Option<HarnessOrderRecord>) {
    let Some(class) = input.harness.execution_class.as_ref() else {
        return (declared, None);
    };
    let settings = &registry.route_selection;
    if input.cos || declared.len() < 2 || !settings.harness_balance_classes.contains(class.as_ref())
    {
        return (declared, None);
    }
    let tripped = state.balance_tripped.contains(class.as_ref());
    let mut scores = BTreeMap::new();
    for harness in &declared {
        let known: Vec<f64> =
            route_priority(registry, harness, input.lane.as_deref(), false, Some(class))
                .iter()
                .filter_map(|key| state.quota.source_headroom.get(&key.source).copied())
                .collect();
        if !known.is_empty() {
            scores.insert(harness.to_string(), known.into_iter().fold(0.0, f64::max));
        }
    }
    let names: Vec<String> = declared.iter().map(ToString::to_string).collect();
    let ordered_names = if tripped {
        names.clone()
    } else {
        order_harnesses(&names, &scores, settings.harness_balance_tolerance)
    };
    let ordered: Vec<HarnessId> = ordered_names
        .iter()
        .filter_map(|n| declared.iter().find(|h| h.as_ref() == n.as_str()).cloned())
        .collect();
    let record = HarnessOrderRecord {
        execution_class: class.to_string(),
        enabled: true,
        tripped,
        tolerance: settings.harness_balance_tolerance,
        switched: ordered_names.first() != names.first(),
        declared: names,
        scores,
        ordered: ordered_names,
    };
    (ordered, Some(record))
}

fn attach_harness_order(trace: &mut RouteTrace, record: &Option<HarnessOrderRecord>) {
    if let (Some(decision), Some(record)) = (trace.decision.as_mut(), record) {
        decision.harness_order = Some(record.clone());
    }
}

/// Decide one harness: the declared priority column (the execution-class list, else harness×lane
/// `route_defaults`, or the CoS
/// list) narrowed by an explicit task model and by harness/source compatibility; then the stage's
/// rule (ADR 2026-10-11 D2: the evidence table in `enforce`, the same declared column in `shadow`);
/// then `(source, account)` admission walking the rule's fallback order. Every step fails closed.
fn route_select_one(
    input: &RouteInput,
    registry: &Registry,
    state: &RouteState,
) -> (Result<RoutePick, RouteFailure>, RouteTrace) {
    let mut trace = RouteTrace::default();
    let harness = match select_harness(&input.harness, registry) {
        Ok(h) => h,
        Err(e) => return (Err(RouteFailure::Harness(e)), trace),
    };
    trace.harness = Some(harness.clone());
    let mut priority = route_priority(
        registry,
        &harness,
        input.lane.as_deref(),
        input.cos,
        input.harness.execution_class.as_ref(),
    )
    .to_vec();
    // ADR 2026-10-22 D3: Jev / fixed 規則が frontier とした lane は、その lane の `route_defaults` を先頭に置き、
    // 根拠表の同等判定・クラスの候補列に勝たせる。後ろに従来の候補列が続く（admission の fallback と ε の対象）。
    let frontier_column: Vec<CandidateKey> =
        if !input.cos && input.lane.as_deref() == Some(FRONTIER_LANE) {
            registry
                .lane_route_defaults
                .get(&(harness.clone(), FRONTIER_LANE.to_owned()))
                .cloned()
                .unwrap_or_default()
        } else {
            Vec::new()
        };
    if !frontier_column.is_empty() {
        // 後ろに続くのは lane を問わない従来の列（クラスの候補列、無ければ harness 全体の列）。
        let rest: Vec<CandidateKey> = route_priority(
            registry,
            &harness,
            None,
            false,
            input.harness.execution_class.as_ref(),
        )
        .iter()
        .filter(|key| !frontier_column.contains(key))
        .cloned()
        .collect();
        priority = frontier_column.iter().cloned().chain(rest).collect();
    }
    if priority.is_empty() {
        return (Err(RouteFailure::NoDeclaredDefault(harness)), trace);
    }
    trace.candidates = priority.clone();
    let request = RouteRequest {
        task_id: input.task_id.clone(),
        harness: harness.clone(),
        model_hint: input.model_hint.clone(),
        lane: input.lane.clone(),
        execution_class: input.harness.execution_class.clone(),
    };
    let declared = enumerate_candidates(&request, registry);
    // Declared priority entries in order; drop (and record) the ones the source does not offer
    // or the explicit model hint excludes.
    let mut ordered = Vec::new();
    for key in &priority {
        let Some(source) = registry.sources.get(&key.source) else {
            trace
                .rejections
                .push(rejection(key, "", ExclusionReason::UnknownSource));
            continue;
        };
        if let Some(c) = declared.iter().find(|c| c.key() == *key) {
            ordered.push(c.clone());
        } else {
            let reason = if source.models.contains(&key.model) {
                ExclusionReason::ModelHintMismatch
            } else {
                ExclusionReason::ModelNotDeclared
            };
            trace
                .rejections
                .push(rejection(key, source.api_format.as_ref(), reason));
        }
    }
    if ordered.is_empty()
        && let Some(hint) = &input.model_hint
    {
        trace.decision = Some(refusal_record(
            input,
            state,
            &harness,
            &priority,
            &[],
            &trace,
        ));
        return (Err(RouteFailure::ModelHintUnmatched(hint.clone())), trace);
    }
    let Some(spec) = registry.harnesses.get(&harness) else {
        return (
            Err(RouteFailure::Harness(HarnessSelectionError::Unknown(
                harness,
            ))),
            trace,
        );
    };
    let (eligible, incompatible) = constrain_for_harness(ordered, spec, registry, &harness);
    trace.rejections.extend(incompatible);
    let limited = with_account_limits(registry, state);
    let evidence = state
        .evidence
        .get(&harness)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    // No row for this genre×harness yet: the stage is `shadow` and the declared column decides.
    let stage = state
        .stages
        .get(&harness)
        .copied()
        .unwrap_or(RoutingStage::Shadow);
    let genre = input.harness.genre.as_deref().unwrap_or_default();
    let version = harness_version(state, &harness, evidence);
    let pressure = quota_pressure(evidence, &state.quota);
    // The exploration seed is the run id; a dry run without one falls back to the task id, so the
    // choice stays deterministic and reproducible (ADR D2 探索).
    let seed = state
        .run_seed
        .clone()
        .unwrap_or_else(|| input.task_id.clone());
    let output = select_by_evidence(&PolicyInput {
        eligible: &eligible,
        evidence,
        genre,
        harness_id: harness.as_ref(),
        harness_version: &version,
        lane: input.lane.as_deref().unwrap_or_default(),
        pinned: &frontier_column,
        route_defaults: &priority,
        quota_remaining: &pressure,
        stage,
        epsilon: registry.route_selection.exploration_epsilon,
        run_id: &seed,
    })
    .ok();
    let outcome = match &output {
        Some(output) => {
            // Admission walks the rule's order: the chosen candidate, then its ranked fallbacks
            // (ADR D2 fallback). Nothing outside that order is ever tried.
            let mut order = vec![output.selected.clone()];
            order.extend(output.ranked_fallbacks.iter().cloned());
            let mut admitted = None;
            for candidate in order {
                match admit_source(
                    &candidate,
                    &state.accounts,
                    &state.quota,
                    &state.reservations,
                    &limited,
                ) {
                    Admission::Admitted(owner) => {
                        admitted = Some((candidate, owner.account_id));
                        break;
                    }
                    Admission::Rejected(reason) => {
                        trace
                            .rejections
                            .push(CandidateRejection { candidate, reason });
                    }
                }
            }
            match admitted {
                Some((candidate, account)) => Ok(RoutePick {
                    wire_model_id: wire_model_id(registry, &harness, &candidate.key()),
                    harness: harness.clone(),
                    candidate,
                    account,
                }),
                None => Err(RouteFailure::NoEligible(harness.clone())),
            }
        }
        // No candidate satisfies the stage's rule: fail closed (no lexicographic fallback).
        None => Err(RouteFailure::NoEligible(harness.clone())),
    };
    trace.decision = Some(decision_record(
        input, state, &harness, &priority, &eligible, &trace, genre, &version, stage, evidence,
        &output,
    ));
    (outcome, trace)
}

/// The `celeris.route-decision/1` record of this decision (ADR-task-routing-features D5).
#[allow(clippy::too_many_arguments)]
fn decision_record(
    input: &RouteInput,
    state: &RouteState,
    harness: &HarnessId,
    priority: &[CandidateKey],
    eligible: &[Candidate],
    trace: &RouteTrace,
    genre: &str,
    harness_version: &str,
    stage: RoutingStage,
    evidence: &[EvidenceRow],
    output: &Option<PolicyOutput>,
) -> RouteDecision {
    // `trace.rejections` cannot be borrowed while `trace.decision` is assigned by the caller.
    let exclusions = trace.rejections.clone();
    build_route_decision(&RouteDecisionInput {
        decision_id: &decision_id(state),
        run_id: state.run_seed.as_deref(),
        subject_kind: if state.work_unit_id.is_some() {
            "work_unit"
        } else {
            "task"
        },
        subject_id: state.work_unit_id.as_deref().unwrap_or(&input.task_id),
        features: state.features.as_ref(),
        genre,
        harness_id: harness.as_ref(),
        harness_version,
        stage,
        lane: input.lane.as_deref(),
        model_pin: input.model_hint.as_ref().map(ModelId::as_ref),
        quota_remaining: &source_quota(&state.quota),
        considered: eligible,
        exclusions: &exclusions,
        evidence,
        route_defaults: priority,
        output: output.as_ref(),
    })
}

/// The record of a decision that never reached the rule (an unmatched explicit model).
fn refusal_record(
    input: &RouteInput,
    state: &RouteState,
    harness: &HarnessId,
    priority: &[CandidateKey],
    eligible: &[Candidate],
    trace: &RouteTrace,
) -> RouteDecision {
    let genre = input.harness.genre.as_deref().unwrap_or_default();
    let evidence = state
        .evidence
        .get(harness)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let stage = state
        .stages
        .get(harness)
        .copied()
        .unwrap_or(RoutingStage::Shadow);
    let version = harness_version(state, harness, evidence);
    decision_record(
        input, state, harness, priority, eligible, trace, genre, &version, stage, evidence, &None,
    )
}

/// `routing_decided` and `route_decisions` share one id: `run:<run_id>` when the run is already
/// known. A refusal has no run, so the recorder assigns a fresh id (see `record_route_selection`).
fn decision_id(state: &RouteState) -> String {
    state
        .run_seed
        .as_ref()
        .map(|run| format!("run:{run}"))
        .unwrap_or_default()
}

/// The version the evidence must agree with (major) to be usable. The registry and the adapters
/// carry no harness version, so a caller that knows it fills `RouteState.harness_versions`;
/// otherwise the version the evidence itself was recorded against is used.
fn harness_version(state: &RouteState, harness: &HarnessId, evidence: &[EvidenceRow]) -> String {
    state
        .harness_versions
        .get(harness)
        .cloned()
        .or_else(|| evidence.first().map(|row| row.key.harness_version.clone()))
        .unwrap_or_default()
}

/// Remaining units per `(source, model, quota window)`: the rule's quota pressure comparison
/// (ADR D2: within the equivalent set, the least quota pressure wins). Windows come from the
/// evidence rows; the remaining amount is the source's remaining quota.
fn quota_pressure(
    evidence: &[EvidenceRow],
    quota: &QuotaSnapshot,
) -> BTreeMap<(String, String, String), f64> {
    let mut out = BTreeMap::new();
    for row in evidence {
        let Some(remaining) = quota
            .source_remaining
            .get(&SourceId::from(row.key.source_id.as_str()))
            .copied()
        else {
            continue;
        };
        for window in row.quota_per_run.keys() {
            out.insert(
                (
                    row.key.source_id.clone(),
                    row.key.model_id.clone(),
                    window.clone(),
                ),
                remaining,
            );
        }
    }
    out
}

/// Remaining units per source, as recorded in the decision's constraints.
fn source_quota(quota: &QuotaSnapshot) -> BTreeMap<String, f64> {
    quota
        .source_remaining
        .iter()
        .map(|(source, remaining)| (source.to_string(), *remaining))
        .collect()
}

fn rejection(key: &CandidateKey, api_format: &str, reason: ExclusionReason) -> CandidateRejection {
    CandidateRejection {
        candidate: Candidate {
            source: key.source.clone(),
            model: key.model.clone(),
            api_format: api_format.into(),
        },
        reason,
    }
}

/// The registry with the per-account limits of `state` applied to its sources.
pub(super) fn with_account_limits(registry: &Registry, state: &RouteState) -> Registry {
    let mut out = registry.clone();
    for ((source, account), max) in &state.account_max_runs {
        if let Some(spec) = out.sources.get_mut(source) {
            spec.account_max_runs.insert(account.clone(), *max);
        }
    }
    out
}

/// The account pool that backs an OAuth source kind (login dirs, usage observations, cooldowns).
pub fn account_adapter_for_source_kind(kind: &str) -> Option<AccountAdapter> {
    match kind {
        "claude_oauth" => Some(AccountAdapter::ClaudeCode),
        "codex_oauth" => Some(AccountAdapter::Codex),
        "opencode_go" => Some(AccountAdapter::OpencodeGo),
        _ => None,
    }
}

/// The account pool `source_id` offers to route selection. `logged_in` is the logged-in account
/// directories when the source kind has an account adapter (OAuth pools), `None` otherwise.
///
/// - OAuth pool: the declared accounts narrowed to the logged-in ones, or every logged-in one when
///   none are declared.
/// - No account adapter: the declared accounts, or the source id itself as its one implicit account
///   when none are declared (ADR 2026-10-08 account-concurrency-limits §(e),
///   ADR 2026-10-26-ldr-proxy-and-route-fixes C1).
///
/// The dispatcher and `celerisctl worker` both build their pools here.
pub fn source_account_pool(
    source_id: &SourceId,
    declared: &[AccountId],
    logged_in: Option<&[AccountId]>,
) -> Vec<AccountId> {
    match logged_in {
        Some(logged_in) if declared.is_empty() => logged_in.to_vec(),
        Some(logged_in) => declared
            .iter()
            .filter(|a| logged_in.contains(a))
            .cloned()
            .collect(),
        None if declared.is_empty() => vec![AccountId::from(source_id.as_ref())],
        None => declared.to_vec(),
    }
}

/// Harness inputs taken from the task's structured fields only (never its prose).
pub fn harness_request_for(task: &Task) -> HarnessRequest {
    let mut required_capabilities = std::collections::BTreeSet::new();
    if task.requirements.browser.is_some() {
        required_capabilities.insert(BROWSER_CAPABILITY.to_string());
    }
    HarnessRequest {
        harness: task.harness_id.clone(),
        genre: task.genre.clone(),
        role: task.role.clone(),
        required_capabilities,
        has_repo: !task.repos.is_empty(),
        execution_class: (task.genre.as_deref() == Some("coding")).then(|| "coding".into()),
    }
}

impl Dispatcher {
    /// Daemon wiring: the declared sources, harnesses, routes and defaults.
    pub fn set_llm_route_registry(&mut self, registry: Registry) {
        self.llm_route_registry = Arc::new(registry);
    }

    pub fn llm_route_registry(&self) -> &Registry {
        &self.llm_route_registry
    }

    /// Capture the admission state of every declared source: logged-in accounts of OAuth
    /// pools (or the declared accounts of other sources), cooldowns and exhausted windows from
    /// the account books, stored per-account limits, and runs holding a `(source, account)`.
    pub(super) fn route_state(&mut self, cos: bool) -> RouteState {
        let registry = Arc::clone(&self.llm_route_registry);
        let now = (self.now_unix_fn)();
        let mut state = RouteState::default();
        for entry in self.running.values() {
            if let Some((source, account)) = &entry.route {
                // A leaf run's admission does not count CoS chat runs (they hold their own slot).
                if !cos && entry.cos {
                    continue;
                }
                *state
                    .reservations
                    .source_in_use
                    .entry(source.clone())
                    .or_default() += 1;
                *state
                    .reservations
                    .account_in_use
                    .entry((source.clone(), account.clone()))
                    .or_default() += 1;
            }
        }
        // Reviewer runs hold their `(source, account)` the same way.
        for (source, account) in self.reviewing.values().filter_map(|e| e.route.as_ref()) {
            *state
                .reservations
                .source_in_use
                .entry(source.clone())
                .or_default() += 1;
            *state
                .reservations
                .account_in_use
                .entry((source.clone(), account.clone()))
                .or_default() += 1;
        }
        for (source_id, spec) in &registry.sources {
            if cos && let Some(launch) = self.cos_chat_launch.as_ref() {
                for (source, account) in launch.route_reservations.values() {
                    if source == source_id {
                        *state
                            .reservations
                            .source_in_use
                            .entry(source.clone())
                            .or_default() += 1;
                        *state
                            .reservations
                            .account_in_use
                            .entry((source.clone(), account.clone()))
                            .or_default() += 1;
                    }
                }
            }
            let adapter = account_adapter_for_source_kind(spec.kind.as_ref());
            // OAuth pools are filled from the logged-in directories below.
            let mut accounts = match adapter {
                Some(_) => Vec::new(),
                None => source_account_pool(source_id, &spec.accounts, None),
            };
            if let Some(adapter) = adapter {
                let scanned = match (
                    self.config.accounts.clone(),
                    self.accounts_scan_cache.get(&adapter),
                ) {
                    (_, Some(dirs)) => dirs.clone(),
                    (Some(cfg), None) => match cfg.root_for(adapter) {
                        Some(root) => {
                            let dirs = scan_accounts(root, adapter);
                            self.accounts_scan_cache.insert(adapter, dirs.clone());
                            dirs
                        }
                        None => Vec::new(),
                    },
                    (None, None) => Vec::new(),
                };
                let logged_in: Vec<AccountId> = scanned
                    .iter()
                    .filter(|d| d.logged_in)
                    .map(|d| AccountId::from(d.id.as_str()))
                    .collect();
                accounts = source_account_pool(source_id, &spec.accounts, Some(&logged_in));
                if let Some(book) = self.account_book(adapter) {
                    let book = book.lock().unwrap_or_else(|e| e.into_inner());
                    // Preserve the account pool's headroom policy when adapting it to
                    // source×model routing. Admission chooses the first eligible account.
                    accounts.sort_by(|left, right| {
                        let in_use = |account: &AccountId| {
                            state
                                .reservations
                                .account_in_use
                                .get(&(source_id.clone(), account.clone()))
                                .copied()
                                .unwrap_or(0)
                        };
                        let score = |account: &AccountId| {
                            evaluate(
                                &AccountCandidate {
                                    id: account.as_ref(),
                                    logged_in: true,
                                    in_use: in_use(account) as usize,
                                },
                                book.state(account.as_ref()),
                                usize::MAX,
                                now,
                            )
                            .score
                        };
                        score(right)
                            .partial_cmp(&score(left))
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then_with(|| left.cmp(right))
                    });
                    // ADR 2026-10-22 D1: source の残量。最良 account の ADR-0024 D3 スコア（席の減点なし）と、
                    // 最も逼迫した窓の残り割合。除外された account は 0。
                    let (mut best_score, mut best_remaining) = (0.0_f64, 0.0_f64);
                    for account in &accounts {
                        let key = (source_id.clone(), account.clone());
                        let candidate = AccountCandidate {
                            id: account.as_ref(),
                            logged_in: true,
                            in_use: 0,
                        };
                        let evaluation =
                            evaluate(&candidate, book.state(account.as_ref()), usize::MAX, now);
                        match evaluation.excluded {
                            Some(ExcludedReason::Cooldown) => {
                                state.quota.cooling_down.insert(key);
                            }
                            Some(_) => {
                                state.quota.account_remaining.insert(key, 0.0);
                            }
                            None => {
                                best_score = best_score.max(evaluation.score.unwrap_or(0.0));
                                best_remaining =
                                    best_remaining.max(crate::accounts::remaining_fraction(
                                        book.state(account.as_ref()),
                                        now,
                                    ));
                            }
                        }
                    }
                    state
                        .quota
                        .source_headroom
                        .insert(source_id.clone(), best_score);
                    // 全 account が除外のときは account 単位の理由（cooldown・枠切れ）を残すため、
                    // source 単位の 0 は書かない。
                    if best_remaining > 0.0 {
                        state
                            .quota
                            .source_remaining
                            .insert(source_id.clone(), best_remaining);
                    }
                }
            }
            // ADR 2026-10-08 §(e): a source without an account pool cools down as one implicit account.
            if account_adapter_for_source_kind(spec.kind.as_ref()).is_none()
                && self
                    .source_cooldowns
                    .get(source_id)
                    .is_some_and(|c| c.until > now)
            {
                for account in &accounts {
                    state
                        .quota
                        .cooling_down
                        .insert((source_id.clone(), account.clone()));
                }
            }
            if self.account_limits.is_some() {
                for account in &accounts {
                    let key = (source_id.clone(), account.clone());
                    let stored = self
                        .account_limits
                        .as_ref()
                        .and_then(|l| l.get(&key).copied());
                    let (effective, _) =
                        crate::capacity::effective_account_limit(stored, self.default_max_runs());
                    let limit = crate::capacity::account_run_limit(effective, cos);
                    state
                        .account_max_runs
                        .insert(key, u32::try_from(limit).unwrap_or(u32::MAX));
                }
            }
            if !accounts.is_empty() {
                state.accounts.available.insert(source_id.clone(), accounts);
            }
        }
        state
    }

    /// Decide the route for `input` against the current registry, admission state and the stored
    /// stage/evidence snapshot.
    pub fn select_route(
        &mut self,
        input: &RouteInput,
    ) -> (Result<RoutePick, RouteFailure>, RouteTrace) {
        self.select_route_with(input, &RouteContext::default())
    }

    /// Same, with the per-dispatch context (run id, features snapshot, work unit) that the decision
    /// record carries. The stage and evidence table are read from the store here — one snapshot per
    /// decision, no LLM call, no clock beyond the store read.
    pub fn select_route_with(
        &mut self,
        input: &RouteInput,
        context: &RouteContext,
    ) -> (Result<RoutePick, RouteFailure>, RouteTrace) {
        let mut state = self.route_state(input.cos);
        state.run_seed = context.run_id.clone();
        state.features = context.features.clone();
        state.work_unit_id = context.work_unit_id.clone();
        match self.store.harness_balance_trips() {
            Ok(trips) => state.balance_tripped = trips.into_keys().collect(),
            Err(error) => {
                tracing::warn!(%error, "harness balance trips unavailable; keeping the declared harness order")
            }
        }
        self.fill_route_stage(
            input,
            context.evidence_mode,
            context.evidence_version.as_deref(),
            &mut state,
        );
        route_select(input, &self.llm_route_registry, &state)
    }

    /// Read the genre×harness stage and the latest evidence rows of every harness this request can
    /// select. A store error leaves the harness without a row, which means `shadow` and no evidence:
    /// the declared `route_defaults` column decides (fail closed, never an undeclared source).
    fn fill_route_stage(
        &self,
        input: &RouteInput,
        evidence_mode: task_core::route_select::EvidenceMode,
        evidence_version: Option<&str>,
        state: &mut RouteState,
    ) {
        let genre = input.harness.genre.as_deref().unwrap_or_default();
        let Ok(harnesses) = harness_candidates(&input.harness, &self.llm_route_registry) else {
            return;
        };
        for harness in harnesses {
            use task_core::route_select::EvidenceMode;
            // none は根拠表を読まない（固定候補順・割当規則と利用可否だけで決める）。
            let rows = match (evidence_mode, evidence_version) {
                (EvidenceMode::None, _) => None,
                (EvidenceMode::Pinned, Some(version)) => Some(self.store.routing_evidence_at(
                    genre,
                    harness.as_ref(),
                    version,
                )),
                // pinned なのに版が無い組合せは検証で弾かれる。来たら最新版へ落とさず読まない。
                (EvidenceMode::Pinned, None) => None,
                (EvidenceMode::Latest, _) => {
                    Some(self.store.latest_routing_evidence(genre, harness.as_ref()))
                }
            };
            match rows {
                Some(Ok(rows)) => {
                    state.evidence.insert(harness.clone(), rows);
                }
                Some(Err(error)) => {
                    tracing::warn!(%genre, harness = %harness, %error, "routing evidence unavailable; selecting without evidence")
                }
                None => {}
            }
            match self.store.routing_stage(genre, harness.as_ref()) {
                Ok(stage) => {
                    state.stages.insert(harness.clone(), stage);
                }
                Err(error) => {
                    tracing::warn!(%genre, harness = %harness, %error, "routing stage unavailable; treating the genre as shadow")
                }
            }
        }
    }

    /// Select from the remaining CoS candidates after earlier attempts failed.
    pub(super) fn select_cos_route_from(
        &mut self,
        input: &RouteInput,
        start: usize,
    ) -> (Result<RoutePick, RouteFailure>, RouteTrace) {
        let state = self.route_state(true);
        let mut registry = (*self.llm_route_registry).clone();
        registry.cos_route_candidates = registry
            .cos_route_candidates
            .into_iter()
            .skip(start)
            .collect();
        route_select(input, &registry, &state)
    }

    /// Append `harness_selected` and `llm_route_selected` for an admitted route and write the
    /// selection onto the run row. A refused route appends the refusal as worker progress.
    pub fn record_route_selection(
        &self,
        task_id: TaskId,
        run_id: Option<&str>,
        input: &RouteInput,
        outcome: &Result<RoutePick, RouteFailure>,
        trace: &RouteTrace,
    ) -> Result<(), DispatchError> {
        let (pick, failure) = match outcome {
            Ok(p) => (Some(p), None),
            Err(f) => (None, Some(f)),
        };
        let metadata = trace.metadata(input, pick, failure);
        if let Some(harness) = &trace.harness {
            self.store.append_event(
                task_id,
                &Event::HarnessSelected {
                    harness_id: harness.to_string(),
                },
            )?;
        }
        match pick {
            Some(pick) => {
                self.store.append_event(
                    task_id,
                    &Event::LlmRouteSelected {
                        source_id: pick.candidate.source.to_string(),
                        model_id: pick.candidate.model.to_string(),
                        account_id: pick.account.to_string(),
                        metadata: metadata.clone(),
                    },
                )?;
                if let Some(run_id) = run_id {
                    self.store.run_record_selection(
                        run_id,
                        &RunSelection {
                            harness: pick.harness.clone(),
                            source: pick.candidate.source.clone(),
                            model: pick.candidate.model.clone(),
                            account: pick.account.clone(),
                            metadata,
                        },
                    )?;
                }
            }
            None => {
                self.store.append_event(
                    task_id,
                    &Event::worker_progress(
                        "routing",
                        format!(
                            "route refused: {}; selection: {metadata}",
                            failure.map(RouteFailure::reason).unwrap_or_default()
                        ),
                    ),
                )?;
            }
        }
        Ok(())
    }
}
