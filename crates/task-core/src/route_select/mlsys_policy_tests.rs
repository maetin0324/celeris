use super::*;
use crate::model::Tier;

const Y: Fact = Fact::Yes;
fn facts() -> LaneFacts {
    LaneFacts {
        class: WorkClass::Routine,
        procedure_clear: Y,
        machine_checkable: Y,
        reversible: Y,
        failure_impact_small: Y,
    }
}
fn hint(s: SpecificityHint, c: f64) -> ClassificationHint {
    ClassificationHint {
        specificity: s,
        confidence: Some(c),
    }
}
const STD: Option<LaneChoice> = Some(LaneChoice::Lane(Tier::Standard));

#[test]
fn all_four_conditions_give_cheap() {
    assert_eq!(fixed_lane(&facts()), Some(LaneChoice::Lane(Tier::Cheap)));
}

#[test]
fn each_missing_cheap_condition_gives_standard() {
    let mut cases = vec![];
    let mut f = facts();
    f.procedure_clear = Fact::No;
    cases.push(f);
    let mut f = facts();
    f.machine_checkable = Fact::No;
    cases.push(f);
    let mut f = facts();
    f.reversible = Fact::No;
    cases.push(f);
    let mut f = facts();
    f.failure_impact_small = Fact::No;
    cases.push(f);
    for f in cases {
        assert_eq!(fixed_lane(&f), STD, "{f:?}");
    }
}

#[test]
fn unknown_condition_defers_and_never_cheap() {
    let mut f = facts();
    f.reversible = Fact::Unknown;
    assert_eq!(fixed_lane(&f), None);
    f.class = WorkClass::Unspecified;
    assert_eq!(fixed_lane(&f), None);
}

#[test]
fn hard_work_goes_frontier_or_replan() {
    let mut f = facts();
    f.class = WorkClass::HardInvestigation;
    assert_eq!(fixed_lane(&f), Some(LaneChoice::Lane(Tier::Frontier)));
    f.class = WorkClass::HighCostReview;
    assert_eq!(fixed_lane(&f), Some(LaneChoice::Lane(Tier::Frontier)));
    f.class = WorkClass::UndecidedDesign;
    assert_eq!(fixed_lane(&f), Some(LaneChoice::Replan));
}

#[test]
fn classification_never_lowers_or_creates_cheap() {
    let classes = [
        WorkClass::Routine,
        WorkClass::UndecidedDesign,
        WorkClass::HardInvestigation,
        WorkClass::HighCostReview,
        WorkClass::Unspecified,
    ];
    let vals = [Fact::Yes, Fact::No, Fact::Unknown];
    let specs = [
        SpecificityHint::Specified,
        SpecificityHint::NeedsImplementation,
        SpecificityHint::NeedsInvestigationDesign,
        SpecificityHint::InsufficientInfo,
    ];
    let rank = |c: Option<LaneChoice>| match c {
        None => -1,
        Some(LaneChoice::Lane(t)) => lane::tier_rank(t) as i32,
        Some(LaneChoice::Replan) => 3,
    };
    for class in classes {
        for a in vals {
            for b in vals {
                let f = LaneFacts {
                    class,
                    procedure_clear: a,
                    machine_checkable: b,
                    reversible: Y,
                    failure_impact_small: Y,
                };
                let base = fixed_lane(&f);
                for s in specs {
                    let d =
                        decide_lane(RoutingMode::DecisionAssisted, &f, Some(&hint(s, 0.99)), 0.6);
                    assert!(rank(d.choice) >= rank(base), "{f:?} {s:?}");
                    if base != Some(LaneChoice::Lane(Tier::Cheap)) {
                        assert_ne!(d.choice, Some(LaneChoice::Lane(Tier::Cheap)), "{f:?} {s:?}");
                    }
                }
            }
        }
    }
}

#[test]
fn classification_raises_and_unknown_stays_none() {
    let d = decide_lane(
        RoutingMode::DecisionAssisted,
        &facts(),
        Some(&hint(SpecificityHint::NeedsImplementation, 0.9)),
        0.6,
    );
    assert_eq!(d.choice, STD);
    assert_eq!(d.basis, LaneBasis::ClassificationRaise);
    let mut f = facts();
    f.reversible = Fact::Unknown;
    // Specified / NeedsImplementation cannot invent a lane from an unknown base.
    for s in [
        SpecificityHint::Specified,
        SpecificityHint::NeedsImplementation,
    ] {
        assert_eq!(
            decide_lane(RoutingMode::DecisionAssisted, &f, Some(&hint(s, 0.9)), 0.6).choice,
            None
        );
    }
    let d = decide_lane(
        RoutingMode::DecisionAssisted,
        &f,
        Some(&hint(SpecificityHint::InsufficientInfo, 0.9)),
        0.6,
    );
    assert_eq!(d.choice, Some(LaneChoice::Replan));
}

#[test]
fn fixed_mode_ignores_classification_and_low_confidence_falls_back() {
    let h = hint(SpecificityHint::InsufficientInfo, 0.99);
    let d = decide_lane(RoutingMode::Fixed, &facts(), Some(&h), 0.6);
    assert_eq!(d.choice, Some(LaneChoice::Lane(Tier::Cheap)));
    assert_eq!(RoutingMode::default(), RoutingMode::Fixed);
    let low = hint(SpecificityHint::InsufficientInfo, 0.1);
    let d = decide_lane(RoutingMode::DecisionAssisted, &facts(), Some(&low), 0.6);
    assert_eq!(
        (d.choice, d.basis),
        (
            Some(LaneChoice::Lane(Tier::Cheap)),
            LaneBasis::FixedFallback
        )
    );
    let d = decide_lane(RoutingMode::DecisionAssisted, &facts(), None, 0.6);
    assert_eq!(d.basis, LaneBasis::FixedFallback);
}

#[test]
fn limits_are_never_exceeded() {
    let lim = LaneLimits {
        allowed: vec![Tier::Cheap, Tier::Standard],
        max_lane: Some(Tier::Standard),
    };
    assert_eq!(
        clamp_to_limits(LaneChoice::Lane(Tier::Frontier), &lim),
        LaneChoice::Replan
    );
    assert_eq!(
        clamp_to_limits(LaneChoice::Lane(Tier::Cheap), &lim),
        LaneChoice::Lane(Tier::Cheap)
    );
    let only_std = LaneLimits {
        allowed: vec![Tier::Standard],
        max_lane: None,
    };
    assert_eq!(
        clamp_to_limits(LaneChoice::Lane(Tier::Cheap), &only_std),
        LaneChoice::Lane(Tier::Standard)
    );
}

#[test]
fn lane_table_copies_registry_and_records_version() {
    use crate::llm_route::{CandidateKey, HarnessId, Registry};
    let h = HarnessId::from("claude_code");
    let key = CandidateKey {
        source: "s".into(),
        model: "m".into(),
    };
    let mut reg = Registry::default();
    reg.lane_route_defaults
        .insert((h.clone(), "cheap".into()), vec![key.clone()]);
    let t = LanePolicyTable::from_registry(DEFAULT_POLICY_VERSION, &reg, &h);
    assert_eq!(t.candidates(Tier::Cheap), &[key]);
    assert!(t.candidates(Tier::Frontier).is_empty());
    let d = decide_lane(RoutingMode::Fixed, &facts(), None, 0.6);
    let r = lane_record(RoutingMode::Fixed, &t, &d);
    assert_eq!(r.policy_version, "mlsys-min-1");
}

fn avail() -> AvailabilityInput {
    AvailabilityInput {
        compatible: true,
        cooling_down: false,
        in_use: 0,
        max_concurrent: Some(2),
        quota_remaining: Remaining::Known(10.0),
        estimated_cost: Remaining::Known(1.0),
        budget_remaining: Remaining::Known(5.0),
    }
}

#[test]
fn unknown_quota_or_cost_is_not_zero_and_not_available() {
    assert_eq!(availability(&avail()), Availability::Available);
    let mut i = avail();
    i.quota_remaining = Remaining::Unknown;
    assert_eq!(availability(&i), Availability::Unknown);
    let mut i = avail();
    i.estimated_cost = Remaining::Unknown;
    assert_eq!(availability(&i), Availability::Unknown);
    let mut i = avail();
    i.budget_remaining = Remaining::Unknown;
    assert_eq!(availability(&i), Availability::Unknown);
    assert!(Remaining::Unknown.pessimistic_cost() > Remaining::Known(0.0).pessimistic_cost());
    assert!(Remaining::Unknown.pessimistic_cost().is_infinite());
}

#[test]
fn unavailable_reasons_win_over_unknown_and_unknown_is_not_extended() {
    let mut i = avail();
    i.quota_remaining = Remaining::Unknown;
    i.in_use = 2;
    assert_eq!(
        availability(&i),
        Availability::Unavailable(UnavailableReason::ConcurrencyFull)
    );
    let mut i = avail();
    i.estimated_cost = Remaining::Known(6.0);
    assert_eq!(
        availability(&i),
        Availability::Unavailable(UnavailableReason::BudgetExhausted)
    );
    let mut i = avail();
    i.compatible = false;
    assert_eq!(
        availability(&i),
        Availability::Unavailable(UnavailableReason::Incompatible)
    );
    let s = split_by_availability(vec![
        ("a", Availability::Unknown),
        ("b", Availability::Available),
        (
            "c",
            Availability::Unavailable(UnavailableReason::CoolingDown),
        ),
    ]);
    assert_eq!((s.available, s.unconfirmed), (vec!["b"], vec!["a"]));
}

fn att(lane: Tier, esc: u32, infra: u32) -> EscalationAttempts {
    EscalationAttempts {
        lane,
        escalations_done: esc,
        infra_retries: infra,
    }
}

#[test]
fn transient_infra_waits_and_never_escalates() {
    let l = EscalationLimits::default();
    for lane in [Tier::Cheap, Tier::Standard, Tier::Frontier] {
        assert_eq!(
            escalation_decision(FailureClass::TransientInfra, &att(lane, 0, 0), &l),
            EscalationDecision::WaitRetry { lane }
        );
    }
    assert_eq!(
        escalation_decision(FailureClass::TransientInfra, &att(Tier::Cheap, 0, 3), &l),
        EscalationDecision::Replan(ReplanReason::InfraRetriesExhausted)
    );
}

#[test]
fn content_escalates_one_stage_then_replans() {
    let l = EscalationLimits::default();
    assert_eq!(
        escalation_decision(FailureClass::Content, &att(Tier::Cheap, 0, 0), &l),
        EscalationDecision::Escalate { to: Tier::Standard }
    );
    assert_eq!(
        escalation_decision(FailureClass::Content, &att(Tier::Standard, 1, 0), &l),
        EscalationDecision::Replan(ReplanReason::EscalationLimit)
    );
    assert_eq!(
        escalation_decision(FailureClass::Content, &att(Tier::Frontier, 0, 0), &l),
        EscalationDecision::Replan(ReplanReason::NoHigherLane)
    );
}

#[test]
fn escalation_respects_allowed_and_ceiling() {
    let l = EscalationLimits {
        lane_limits: LaneLimits {
            allowed: vec![Tier::Cheap, Tier::Frontier],
            max_lane: None,
        },
        ..Default::default()
    };
    // Standard is not permitted; do not skip to frontier.
    assert_eq!(
        escalation_decision(FailureClass::Content, &att(Tier::Cheap, 0, 0), &l),
        EscalationDecision::Replan(ReplanReason::NotPermitted)
    );
    let l = EscalationLimits {
        lane_limits: LaneLimits {
            allowed: vec![],
            max_lane: Some(Tier::Standard),
        },
        ..Default::default()
    };
    assert_eq!(
        escalation_decision(FailureClass::Content, &att(Tier::Standard, 0, 0), &l),
        EscalationDecision::Replan(ReplanReason::NotPermitted)
    );
}
