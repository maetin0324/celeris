//! policy-close: `routing.mode`（fixed / decision_assisted）の通し試験（公開 API のみ。ADR
//! 2026-10-15-mlsys-min-routing-policy 付記「policy-close」）。分類は注入した結果で、Jev・実 API は呼ばない。

use task_core::Tier;
use task_core::route_select::{
    AssistInput, Availability, AvailabilityInput, ClassificationEntry, ClassificationHint,
    ClassifierStatus, DEFAULT_MIN_CONFIDENCE, DEFAULT_POLICY_VERSION, EscalationAttempts,
    EscalationDecision, EscalationLimits, Fact, FailureClass, LaneFacts, LaneLimits,
    REAL_API_VERIFIED, Remaining, ReplanReason, RoutingMode, SpecificityHint, UnavailableReason,
    WorkClass, assist_lane, availability, escalation_decision,
};
use task_dispatch::{
    BoardItem, ClassificationBoard, FailureOrigin, LaunchAdmission, admit_by_availability,
    classify_failure,
};
use task_worker::AdapterError;

fn routine(clear: Fact, check: Fact, rev: Fact, small: Fact) -> LaneFacts {
    LaneFacts {
        class: WorkClass::Routine,
        procedure_clear: clear,
        machine_checkable: check,
        reversible: rev,
        failure_impact_small: small,
    }
}

fn cheap_facts() -> LaneFacts {
    routine(Fact::Yes, Fact::Yes, Fact::Yes, Fact::Yes)
}

fn jev_ok(s: SpecificityHint, confidence: f64) -> ClassificationEntry {
    ClassificationEntry {
        status: ClassifierStatus::JevOk,
        hint: Some(ClassificationHint {
            specificity: s,
            confidence: Some(confidence),
        }),
        model_snapshot: None,
    }
}

fn run(
    mode: RoutingMode,
    explicit: bool,
    base: Tier,
    facts: LaneFacts,
    entry: Option<&ClassificationEntry>,
    limits: &LaneLimits,
) -> task_core::route_select::AssistOutcome {
    assist_lane(
        &AssistInput {
            mode,
            explicit,
            base,
            facts,
            entry,
            limits,
        },
        |_| true,
    )
}

#[test]
fn default_mode_is_fixed_and_fixed_ignores_any_classification() {
    assert_eq!(RoutingMode::default(), RoutingMode::Fixed);
    let limits = LaneLimits::default();
    for s in [
        SpecificityHint::Specified,
        SpecificityHint::NeedsImplementation,
        SpecificityHint::NeedsInvestigationDesign,
        SpecificityHint::InsufficientInfo,
    ] {
        let e = jev_ok(s, 0.99);
        let with = run(
            RoutingMode::Fixed,
            false,
            Tier::Standard,
            cheap_facts(),
            Some(&e),
            &limits,
        );
        let without = run(
            RoutingMode::Fixed,
            false,
            Tier::Standard,
            cheap_facts(),
            None,
            &limits,
        );
        assert_eq!(with, without, "fixed は分類を読まない");
        assert_eq!(with.lane, Tier::Standard);
        assert!(with.record.is_none(), "fixed は lane_assist を書かない");
    }
}

#[test]
fn decision_assisted_never_goes_below_the_fixed_result() {
    let limits = LaneLimits::default();
    for base in [Tier::Cheap, Tier::Standard, Tier::Frontier] {
        for s in [
            SpecificityHint::Specified,
            SpecificityHint::NeedsImplementation,
            SpecificityHint::NeedsInvestigationDesign,
            SpecificityHint::InsufficientInfo,
        ] {
            let e = jev_ok(s, 0.95);
            let fixed = run(
                RoutingMode::Fixed,
                false,
                base,
                cheap_facts(),
                None,
                &limits,
            );
            let assisted = run(
                RoutingMode::DecisionAssisted,
                false,
                base,
                cheap_facts(),
                Some(&e),
                &limits,
            );
            assert!(
                task_core::route_select::lane::tier_rank(assisted.lane)
                    >= task_core::route_select::lane::tier_rank(fixed.lane),
                "{base:?}/{s:?}: 分類は lane を下げない"
            );
        }
    }
    // cheap 条件を満たさない事実は、分類が Specified でも cheap にならない。
    let e = jev_ok(SpecificityHint::Specified, 0.99);
    let o = run(
        RoutingMode::DecisionAssisted,
        false,
        Tier::Standard,
        routine(Fact::Yes, Fact::No, Fact::Yes, Fact::Yes),
        Some(&e),
        &limits,
    );
    assert_eq!(o.lane, Tier::Standard);
}

#[test]
fn explicit_lane_and_ceiling_beat_the_classification() {
    let e = jev_ok(SpecificityHint::NeedsInvestigationDesign, 0.99);
    // 明示 lane（人・System）: 提案を採らない。
    let o = run(
        RoutingMode::DecisionAssisted,
        true,
        Tier::Cheap,
        cheap_facts(),
        Some(&e),
        &LaneLimits::default(),
    );
    assert_eq!(o.lane, Tier::Cheap);
    assert_eq!(o.record.unwrap().overridden_by.as_deref(), Some("explicit"));
    // 許可・天井の外: frontier へは上げない。
    let capped = LaneLimits {
        allowed: vec![],
        max_lane: Some(Tier::Standard),
    };
    let o = run(
        RoutingMode::DecisionAssisted,
        false,
        Tier::Standard,
        cheap_facts(),
        Some(&e),
        &capped,
    );
    assert_eq!(o.lane, Tier::Standard);
    assert_eq!(o.record.unwrap().overridden_by.as_deref(), Some("ceiling"));
    // 利用可否: 引き上げ先が使えなければ採らない。
    let o = assist_lane(
        &AssistInput {
            mode: RoutingMode::DecisionAssisted,
            explicit: false,
            base: Tier::Standard,
            facts: cheap_facts(),
            entry: Some(&e),
            limits: &LaneLimits::default(),
        },
        |t| t != Tier::Frontier,
    );
    assert_eq!(o.lane, Tier::Standard);
    assert_eq!(
        o.record.unwrap().overridden_by.as_deref(),
        Some("availability")
    );
}

#[test]
fn fallback_and_real_api_verified_are_recorded_separately() {
    let limits = LaneLimits::default();
    let fixed = run(
        RoutingMode::Fixed,
        false,
        Tier::Standard,
        cheap_facts(),
        None,
        &limits,
    );
    for reason in ["breaker_open", "timeout", "key_missing", "auth"] {
        let e = ClassificationEntry::fell_back(reason);
        let o = run(
            RoutingMode::DecisionAssisted,
            false,
            Tier::Standard,
            cheap_facts(),
            Some(&e),
            &limits,
        );
        let r = o.record.unwrap();
        assert_eq!(o.lane, fixed.lane, "fallback は fixed と同じ lane");
        assert_eq!(r.basis, "fixed_fallback");
        assert_eq!(
            r.classifier_status,
            ClassifierStatus::FellBackToFixed {
                reason: reason.into()
            }
        );
        assert!(!r.real_api_verified);
    }
    // 低 confidence の成功も fixed に戻る（status は jev_ok のまま、実 API 検証済みは別欄で false）。
    let low = jev_ok(
        SpecificityHint::InsufficientInfo,
        DEFAULT_MIN_CONFIDENCE - 0.1,
    );
    let o = run(
        RoutingMode::DecisionAssisted,
        false,
        Tier::Standard,
        cheap_facts(),
        Some(&low),
        &limits,
    );
    let r = o.record.unwrap();
    assert_eq!(r.classifier_status, ClassifierStatus::JevOk);
    assert_eq!(r.basis, "fixed_fallback");
    assert_eq!(o.lane, Tier::Standard);
    assert!(!REAL_API_VERIFIED && !r.real_api_verified);
    // 分類段が走っていない: not_attempted。
    let o = run(
        RoutingMode::DecisionAssisted,
        false,
        Tier::Standard,
        cheap_facts(),
        None,
        &limits,
    );
    assert_eq!(
        o.record.unwrap().classifier_status,
        ClassifierStatus::NotAttempted
    );
}

#[test]
fn infra_failures_wait_and_never_escalate_while_content_escalates_once() {
    let limits = EscalationLimits::default();
    let at = |lane, esc, retries| EscalationAttempts {
        lane,
        escalations_done: esc,
        infra_retries: retries,
    };
    let infra = [
        classify_failure(FailureOrigin::Adapter(&AdapterError::Other("boom".into()))),
        classify_failure(FailureOrigin::Adapter(&AdapterError::Exhausted(
            "quota".into(),
        ))),
        classify_failure(FailureOrigin::WorkerError {
            message: "HTTP 429 rate limit",
            retryable: false,
        }),
    ];
    for class in infra {
        assert_eq!(class, FailureClass::TransientInfra);
        for retries in 0..limits.max_infra_retries {
            assert_eq!(
                escalation_decision(class, &at(Tier::Cheap, 0, retries), &limits),
                EscalationDecision::WaitRetry { lane: Tier::Cheap },
                "一時障害は同じ lane で待つ（上位へ送らない）"
            );
        }
        assert_eq!(
            escalation_decision(
                class,
                &at(Tier::Cheap, 0, limits.max_infra_retries),
                &limits
            ),
            EscalationDecision::Replan(ReplanReason::InfraRetriesExhausted)
        );
    }
    let content = classify_failure(FailureOrigin::ContentRejected);
    assert_eq!(content, FailureClass::Content);
    assert_eq!(
        escalation_decision(content, &at(Tier::Cheap, 0, 0), &limits),
        EscalationDecision::Escalate { to: Tier::Standard }
    );
    assert_eq!(
        escalation_decision(content, &at(Tier::Standard, 1, 0), &limits),
        EscalationDecision::Replan(ReplanReason::EscalationLimit)
    );
    assert!(matches!(
        escalation_decision(content, &at(Tier::Frontier, 0, 0), &limits),
        EscalationDecision::Replan(_)
    ));
    // 天井が昇格先を禁じるなら昇格せず replan。
    let capped = EscalationLimits {
        lane_limits: LaneLimits {
            allowed: vec![],
            max_lane: Some(Tier::Cheap),
        },
        ..Default::default()
    };
    assert!(matches!(
        escalation_decision(content, &at(Tier::Cheap, 0, 0), &capped),
        EscalationDecision::Replan(_)
    ));
}

#[test]
fn unknown_quota_or_cost_is_not_treated_as_zero_or_available() {
    let base = AvailabilityInput {
        compatible: true,
        cooling_down: false,
        in_use: 0,
        max_concurrent: Some(2),
        quota_remaining: Remaining::Known(10.0),
        estimated_cost: Remaining::Known(1.0),
        budget_remaining: Remaining::Known(5.0),
    };
    assert_eq!(availability(&base), Availability::Available);
    for unknown in [
        AvailabilityInput {
            quota_remaining: Remaining::Unknown,
            ..base
        },
        AvailabilityInput {
            estimated_cost: Remaining::Unknown,
            ..base
        },
        AvailabilityInput {
            budget_remaining: Remaining::Unknown,
            ..base
        },
    ] {
        assert_eq!(availability(&unknown), Availability::Unknown);
    }
    assert_eq!(
        availability(&AvailabilityInput {
            quota_remaining: Remaining::Known(0.0),
            ..base
        }),
        Availability::Unavailable(UnavailableReason::QuotaExhausted)
    );
    assert_eq!(Remaining::Unknown.pessimistic_cost(), f64::INFINITY);
    // Unknown だけなら起動せず Hold。Available が後ろにあればそちらが勝つ。
    assert_eq!(
        admit_by_availability(vec![("u", Availability::Unknown)]),
        LaunchAdmission::Hold { unconfirmed: 1 }
    );
    assert_eq!(
        admit_by_availability(vec![
            ("u", Availability::Unknown),
            ("a", Availability::Available)
        ]),
        LaunchAdmission::Launch("a")
    );
}

#[test]
fn policy_version_is_pinned_and_policy_path_does_not_use_evidence_router() {
    assert_eq!(DEFAULT_POLICY_VERSION, "mlsys-min-1");
    let board = ClassificationBoard::default();
    board.put(
        "s",
        BoardItem {
            entry: ClassificationEntry::fell_back("timeout"),
            credential: None,
        },
    );
    assert!(board.get("s").is_some());
    // 方策の経路（純粋部 4 本と dispatcher の lane 層）は実測根拠表ルータに依存しない。
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for rel in [
        "../task-core/src/route_select/lane.rs",
        "../task-core/src/route_select/availability.rs",
        "../task-core/src/route_select/escalation.rs",
        "../task-core/src/route_select/assist.rs",
        "src/dispatcher/lane_assist.rs",
        "src/dispatcher/route_failure.rs",
    ] {
        let src = std::fs::read_to_string(root.join(rel)).expect(rel);
        let code: String = src
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for banned in ["routing_evidence", "model_router", "feedback::"] {
            assert!(!code.contains(banned), "{rel} が {banned} に依存している");
        }
    }
    // 既存の根拠表ルータは残っている。
    assert!(root.join("../task-core/src/model_router").exists());
    assert!(
        root.join("../task-core/src/route_select/evidence.rs")
            .exists()
    );
}
