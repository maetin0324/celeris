//! Pure source/model selection rules (ADR 2026-10-11 D2/D3).
pub mod assist;
pub mod availability;
pub mod escalation;
pub mod evidence;
pub mod lane;
pub mod policy;
pub mod stage;
pub mod trace;

pub use assist::{
    AssistInput, AssistOutcome, ClassificationEntry, ClassifierStatus, LaneAssistRecord,
    REAL_API_VERIFIED, assist_lane,
};
pub use availability::{
    Availability, AvailabilityInput, AvailabilitySplit, Remaining, UnavailableReason, availability,
    split_by_availability,
};
pub use escalation::{
    EscalationAttempts, EscalationDecision, EscalationLimits, FailureClass, ReplanReason,
    escalation_decision,
};
pub use evidence::{WilsonInterval, wilson_interval};
pub use lane::{
    ClassificationHint, DEFAULT_MIN_CONFIDENCE, DEFAULT_POLICY_VERSION, Fact, LaneBasis,
    LaneChoice, LaneDecision, LaneFacts, LaneLimits, LanePolicyTable, LaneRecord, RoutingMode,
    SpecificityHint, WorkClass, clamp_to_limits, decide_lane, fixed_lane, lane_record,
};
pub use policy::{NoRoute, PolicyInput, PolicyOutput, SelectionSource, select};
pub use stage::{
    GuardrailInput, GuardrailReason, GuardrailResult, RoutingStage, RoutingStageChange,
    evaluate_guardrail,
};
pub use trace::{
    EvidenceSnapshot, RouteAction, RouteCandidate, RouteDecision, RouteDecisionInput,
    RouteExclusion, RouteJudge, RouteRule, RouteScore, build_route_decision,
};

#[cfg(test)]
mod mlsys_policy_tests;

#[cfg(test)]
mod assist_tests;
