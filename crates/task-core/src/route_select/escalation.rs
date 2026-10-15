//! 失敗分類と限定エスカレーション（ADR 2026-10-15-mlsys-min-routing-policy D7）。
use super::lane::{LaneChoice, LaneLimits, clamp_to_limits, tier_rank};
use crate::model::Tier;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    TransientInfra,
    Content,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EscalationLimits {
    /// 昇格の上限回数（1 回 = 1 段）。
    pub max_escalations: u32,
    pub max_infra_retries: u32,
    pub lane_limits: LaneLimits,
}

impl Default for EscalationLimits {
    fn default() -> Self {
        Self {
            max_escalations: 1,
            max_infra_retries: 3,
            lane_limits: LaneLimits::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EscalationAttempts {
    pub lane: Tier,
    pub escalations_done: u32,
    pub infra_retries: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplanReason {
    InfraRetriesExhausted,
    EscalationLimit,
    NoHigherLane,
    NotPermitted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EscalationDecision {
    /// 同じ lane で待機してから再試行（上位 model へ送らない）。
    WaitRetry {
        lane: Tier,
    },
    Escalate {
        to: Tier,
    },
    Replan(ReplanReason),
}

pub fn escalation_decision(
    class: FailureClass,
    attempts: &EscalationAttempts,
    limits: &EscalationLimits,
) -> EscalationDecision {
    match class {
        FailureClass::TransientInfra => {
            if attempts.infra_retries < limits.max_infra_retries {
                EscalationDecision::WaitRetry {
                    lane: attempts.lane,
                }
            } else {
                EscalationDecision::Replan(ReplanReason::InfraRetriesExhausted)
            }
        }
        FailureClass::Content => {
            if attempts.escalations_done >= limits.max_escalations {
                return EscalationDecision::Replan(ReplanReason::EscalationLimit);
            }
            let next = match attempts.lane {
                Tier::Cheap => Tier::Standard,
                Tier::Standard => Tier::Frontier,
                Tier::Frontier => return EscalationDecision::Replan(ReplanReason::NoHigherLane),
            };
            match clamp_to_limits(LaneChoice::Lane(next), &limits.lane_limits) {
                LaneChoice::Lane(t) if tier_rank(t) == tier_rank(next) => {
                    EscalationDecision::Escalate { to: t }
                }
                // 許可・天井で 1 段上が使えない（さらに上へ飛ばさない）。
                _ => EscalationDecision::Replan(ReplanReason::NotPermitted),
            }
        }
    }
}
