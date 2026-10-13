//! Pure source/model selection rules (ADR 2026-10-11 D2/D3).
pub mod evidence;
pub mod policy;
pub mod stage;
pub mod trace;

pub use evidence::{EvidenceKey, EvidenceRow, WilsonInterval, wilson_interval};
pub use policy::{PolicyInput, PolicyOutput, SelectionSource, select};
pub use stage::{
    GuardrailInput, GuardrailReason, GuardrailResult, RoutingStage, RoutingStageChange,
    evaluate_guardrail,
};
pub use trace::{RouteAction, RouteDecision};
