use super::assist::*;
use super::lane::*;
use crate::model::Tier;

fn facts(f: Fact) -> LaneFacts {
    LaneFacts {
        class: WorkClass::Routine,
        procedure_clear: f,
        machine_checkable: f,
        reversible: f,
        failure_impact_small: f,
    }
}

fn ok_entry(s: SpecificityHint, conf: f64) -> ClassificationEntry {
    ClassificationEntry {
        status: ClassifierStatus::JevOk,
        hint: Some(ClassificationHint {
            specificity: s,
            confidence: Some(conf),
        }),
        model_snapshot: None,
        cost_usd: None,
    }
}

fn run(
    mode: RoutingMode,
    explicit: bool,
    base: Tier,
    facts: LaneFacts,
    entry: Option<&ClassificationEntry>,
    limits: &LaneLimits,
    avail: impl Fn(Tier) -> bool,
) -> AssistOutcome {
    assist_lane(
        &AssistInput {
            mode,
            explicit,
            base,
            facts,
            entry,
            limits,
        },
        avail,
    )
}

#[test]
fn fixed_mode_is_identity_and_writes_no_record() {
    let e = ok_entry(SpecificityHint::InsufficientInfo, 0.99);
    let o = run(
        RoutingMode::Fixed,
        false,
        Tier::Standard,
        facts(Fact::Yes),
        Some(&e),
        &LaneLimits::default(),
        |_| true,
    );
    assert_eq!(o.lane, Tier::Standard);
    assert!(o.record.is_none());
}

#[test]
fn explicit_lane_beats_classification() {
    let e = ok_entry(SpecificityHint::NeedsInvestigationDesign, 0.99);
    let o = run(
        RoutingMode::DecisionAssisted,
        true,
        Tier::Cheap,
        facts(Fact::Yes),
        Some(&e),
        &LaneLimits::default(),
        |_| true,
    );
    assert_eq!(o.lane, Tier::Cheap);
    assert_eq!(o.record.unwrap().overridden_by.as_deref(), Some("explicit"));
}

#[test]
fn classification_raises_within_limits() {
    let e = ok_entry(SpecificityHint::NeedsInvestigationDesign, 0.9);
    let o = run(
        RoutingMode::DecisionAssisted,
        false,
        Tier::Standard,
        facts(Fact::No),
        Some(&e),
        &LaneLimits::default(),
        |_| true,
    );
    assert_eq!(o.lane, Tier::Frontier);
    let r = o.record.unwrap();
    assert_eq!(r.basis, "classification_raise");
    assert_eq!(r.classifier_status, ClassifierStatus::JevOk);
    assert!(!r.real_api_verified);
}

#[test]
fn ceiling_and_allowed_tiers_beat_classification() {
    let e = ok_entry(SpecificityHint::NeedsInvestigationDesign, 0.9);
    let cap = LaneLimits {
        allowed: vec![],
        max_lane: Some(Tier::Standard),
    };
    let o = run(
        RoutingMode::DecisionAssisted,
        false,
        Tier::Standard,
        facts(Fact::No),
        Some(&e),
        &cap,
        |_| true,
    );
    assert_eq!(o.lane, Tier::Standard);
    assert_eq!(o.record.unwrap().overridden_by.as_deref(), Some("ceiling"));
}

#[test]
fn unavailable_lane_beats_classification() {
    let e = ok_entry(SpecificityHint::InsufficientInfo, 0.9);
    let o = run(
        RoutingMode::DecisionAssisted,
        false,
        Tier::Standard,
        facts(Fact::No),
        Some(&e),
        &LaneLimits::default(),
        |t| t != Tier::Frontier,
    );
    assert_eq!(o.lane, Tier::Standard);
    assert_eq!(
        o.record.unwrap().overridden_by.as_deref(),
        Some("availability")
    );
}

#[test]
fn failures_fall_back_to_fixed_and_keep_real_api_flag_false() {
    for reason in ["breaker_open", "key_missing", "timeout"] {
        let e = ClassificationEntry::fell_back(reason);
        let o = run(
            RoutingMode::DecisionAssisted,
            false,
            Tier::Standard,
            facts(Fact::No),
            Some(&e),
            &LaneLimits::default(),
            |_| true,
        );
        assert_eq!(o.lane, Tier::Standard);
        let r = o.record.unwrap();
        assert_eq!(r.basis, "fixed_fallback");
        assert_eq!(
            r.classifier_status,
            ClassifierStatus::FellBackToFixed {
                reason: reason.into()
            }
        );
        assert!(!r.real_api_verified);
    }
    let o = run(
        RoutingMode::DecisionAssisted,
        false,
        Tier::Standard,
        facts(Fact::No),
        None,
        &LaneLimits::default(),
        |_| true,
    );
    assert_eq!(
        o.record.unwrap().classifier_status,
        ClassifierStatus::NotAttempted
    );
}

#[test]
fn classification_never_lowers_and_never_makes_cheap() {
    // Specified は何も作らず、cheap 条件が不明なら cheap にもならない。
    for s in [
        SpecificityHint::Specified,
        SpecificityHint::NeedsImplementation,
    ] {
        let e = ok_entry(s, 0.99);
        for base in [Tier::Standard, Tier::Frontier] {
            let o = run(
                RoutingMode::DecisionAssisted,
                false,
                base,
                facts(Fact::Unknown),
                Some(&e),
                &LaneLimits::default(),
                |_| true,
            );
            assert_eq!(o.lane, base);
        }
    }
    // fixed 規則が cheap でも、既存 lane（standard）より下げない。
    let e = ok_entry(SpecificityHint::Specified, 0.99);
    let o = run(
        RoutingMode::DecisionAssisted,
        false,
        Tier::Standard,
        facts(Fact::Yes),
        Some(&e),
        &LaneLimits::default(),
        |_| true,
    );
    assert_eq!(o.lane, Tier::Standard);
}

#[test]
fn low_confidence_classification_is_ignored() {
    let e = ok_entry(SpecificityHint::InsufficientInfo, 0.2);
    let o = run(
        RoutingMode::DecisionAssisted,
        false,
        Tier::Standard,
        facts(Fact::No),
        Some(&e),
        &LaneLimits::default(),
        |_| true,
    );
    assert_eq!(o.lane, Tier::Standard);
    assert_eq!(o.record.unwrap().basis, "fixed_fallback");
}

fn hard_facts() -> LaneFacts {
    LaneFacts {
        class: WorkClass::HardInvestigation,
        ..facts(Fact::Yes)
    }
}

fn basis_of(base: Tier, f: LaneFacts, s: SpecificityHint) -> (Tier, String) {
    let e = ok_entry(s, 0.9);
    let o = run(
        RoutingMode::DecisionAssisted,
        false,
        base,
        f,
        Some(&e),
        &LaneLimits::default(),
        |_| true,
    );
    (o.lane, o.record.unwrap().basis)
}

#[test]
fn lane_raised_by_fixed_rule_is_fixed_rule_even_if_classification_agrees() {
    let (lane, basis) = basis_of(
        Tier::Standard,
        hard_facts(),
        SpecificityHint::NeedsInvestigationDesign,
    );
    assert_eq!(lane, Tier::Frontier);
    assert_eq!(basis, "fixed_rule");
}

#[test]
fn lane_raised_by_classification_over_fixed_rule_is_classification_raise() {
    let (lane, basis) = basis_of(
        Tier::Standard,
        facts(Fact::Yes),
        SpecificityHint::NeedsInvestigationDesign,
    );
    assert_eq!(lane, Tier::Frontier);
    assert_eq!(basis, "classification_raise");
}

#[test]
fn classification_proposal_already_covered_by_base_lane_is_not_a_raise() {
    // fixed 規則は cheap、既存 lane は standard。分類は standard を提案するが lane は変わらない。
    let (lane, basis) = basis_of(
        Tier::Standard,
        facts(Fact::Yes),
        SpecificityHint::NeedsImplementation,
    );
    assert_eq!(lane, Tier::Standard);
    assert_eq!(basis, "fixed_rule");
}

fn assisted(entry: Option<&ClassificationEntry>, limits: &LaneLimits, f: Fact) -> LaneAssistRecord {
    run(
        RoutingMode::DecisionAssisted,
        false,
        Tier::Standard,
        facts(f),
        entry,
        limits,
        |_| true,
    )
    .record
    .expect("decision_assisted writes a record")
}

#[test]
fn record_keeps_fixed_jev_and_constrained_lanes_apart() {
    // fixed 規則は Standard（事実が No）、Jev 単独は Frontier を提案、天井が Standard に抑える。
    let mut e = ok_entry(SpecificityHint::NeedsInvestigationDesign, 0.9);
    e.model_snapshot = Some("jev-resp-model".into());
    let limits = LaneLimits {
        allowed: vec![],
        max_lane: Some(Tier::Standard),
    };
    let r = assisted(Some(&e), &limits, Fact::No);
    assert_eq!(r.fixed_lane.as_deref(), Some("standard"));
    assert_eq!(r.jev_lane.as_deref(), Some("frontier"));
    assert_eq!(r.final_lane, "standard");
    assert_eq!(r.overridden_by.as_deref(), Some("ceiling"));
    assert_eq!(r.response_model.as_deref(), Some("jev-resp-model"));
    // 独立の観測経路が無いので snapshot / call_id は欠測のまま（応答 model から複製しない）。
    assert_eq!(r.observed_snapshot, None);
    assert_eq!(r.call_id, None);
}

#[test]
fn jev_lane_is_absent_without_a_usable_proposal_and_fixed_stays_visible() {
    // 低 confidence・Specified・分類なしでは Jev 単独の提案は無いが、fixed 規則の lane は残る。
    for entry in [
        Some(ok_entry(SpecificityHint::NeedsInvestigationDesign, 0.1)),
        Some(ok_entry(SpecificityHint::Specified, 0.99)),
        None,
    ] {
        let r = assisted(entry.as_ref(), &LaneLimits::default(), Fact::No);
        assert_eq!(r.jev_lane, None);
        assert_eq!(r.fixed_lane.as_deref(), Some("standard"));
        assert_eq!(r.response_model, None);
    }
}

#[test]
fn legacy_record_without_new_fields_parses_and_new_fields_are_omitted_when_missing() {
    let old = serde_json::json!({
        "mode": "decision_assisted",
        "classifier_status": {"status": "jev_ok"},
        "real_api_verified": false,
        "base_lane": "standard",
        "final_lane": "standard",
        "basis": "fixed_rule"
    });
    let r: LaneAssistRecord = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(r.fixed_lane, None);
    assert_eq!(r.jev_lane, None);
    assert_eq!(r.response_model, None);
    assert_eq!(r.observed_snapshot, None);
    assert_eq!(r.call_id, None);
    // 欠測は書き出さない（旧 event と同じ形のまま）。
    assert_eq!(serde_json::to_value(&r).unwrap(), old);
}
