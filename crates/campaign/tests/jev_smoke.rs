//! Jev smoke 判定器の fixture 試験（実 API は呼ばない）。
use celeris_campaign::jev_smoke::{Attribution, ClassifierKind, Observation, check_smoke, judge};
use serde_json::{Value, json};

fn event(status: Value, base: &str, fin: &str, basis: &str) -> Value {
    json!({"type":"routing_decided","record":{"lane_assist":{
        "mode":"decision_assisted","classifier_status":status,"real_api_verified":false,
        "base_lane":base,"final_lane":fin,"basis":basis}}})
}

fn obs(event: Value) -> Observation {
    Observation {
        task_id: "t".into(),
        event,
        allowed: vec![],
        max_lane: None,
        usage_present: true,
        cost_usd: Some(0.002),
        version: Some("typesafe/jev-1.14".into()),
        fixed_lane: None,
    }
}

fn ok() -> Value {
    json!({"status":"jev_ok"})
}

#[test]
fn distinguishes_jev_ok_fallback_and_missing() {
    let a = judge(&obs(event(ok(), "standard", "standard", "fixed_rule")));
    assert_eq!(a.classifier, ClassifierKind::JevOk);
    assert!(a.chain_ok);
    let f = judge(&obs(event(
        json!({"status":"fell_back_to_fixed","reason":"timeout"}),
        "standard",
        "standard",
        "fixed_fallback",
    )));
    assert_eq!(
        f.classifier,
        ClassifierKind::Fallback {
            reason: Some("timeout".into())
        }
    );
    assert!(!f.chain_ok);
    let m = judge(&obs(json!({"type":"routing_decided","record":{}})));
    assert_eq!(m.classifier, ClassifierKind::Missing);
    assert!(!m.violations.is_empty());
    let mut bad = obs(event(
        json!({"weird":1}),
        "standard",
        "standard",
        "fixed_rule",
    ));
    bad.usage_present = false;
    assert_eq!(judge(&bad).classifier, ClassifierKind::Missing);
}

#[test]
fn real_api_verified_constant_is_not_evidence() {
    let mut e = event(
        json!({"status":"fell_back_to_fixed","reason":"timeout"}),
        "standard",
        "standard",
        "fixed_fallback",
    );
    e["record"]["lane_assist"]["real_api_verified"] = json!(true);
    let (_, r) = check_smoke(&[obs(e)]);
    assert!(
        r.is_err(),
        "fallback only must not pass even if the flag says true"
    );
}

#[test]
fn missing_cost_or_version_fails() {
    let e = event(ok(), "standard", "standard", "fixed_rule");
    let mut no_cost = obs(e.clone());
    no_cost.cost_usd = None;
    let (_, r) = check_smoke(&[no_cost]);
    assert!(r.unwrap_err().iter().any(|m| m.contains("費用")));
    let mut no_ver = obs(e.clone());
    no_ver.version = None;
    let (_, r) = check_smoke(&[no_ver]);
    assert!(r.unwrap_err().iter().any(|m| m.contains("版")));
    let mut empty_ver = obs(e.clone());
    empty_ver.version = Some(String::new());
    assert!(check_smoke(&[empty_ver]).1.is_err());
    let mut no_row = obs(e.clone());
    no_row.usage_present = false;
    assert!(check_smoke(&[no_row]).1.is_err());
    assert!(check_smoke(&[obs(e)]).1.is_ok());
}

#[test]
fn lane_must_be_within_allowed_range() {
    let mut o = obs(event(ok(), "standard", "frontier", "classification_raise"));
    o.allowed = vec!["standard".into()];
    assert!(!judge(&o).lane_in_limits);
    let mut o = obs(event(ok(), "standard", "frontier", "classification_raise"));
    o.max_lane = Some("standard".into());
    assert!(judge(&o).violations.iter().any(|v| v.contains("許可範囲")));
}

#[test]
fn raise_attributed_to_fixed_rule_is_checked_against_fixed_lane() {
    let e = |basis| event(ok(), "standard", "frontier", basis);
    // fixed 規則の lane が観測に無い: Jev との区別ができない。
    assert_eq!(
        judge(&obs(e("fixed_rule"))).attribution,
        Attribution::Unverifiable
    );
    // fixed 規則が frontier を出していた: fixed_rule で正しい。
    let mut o = obs(e("fixed_rule"));
    o.fixed_lane = Some("frontier".into());
    assert_eq!(judge(&o).attribution, Attribution::Consistent);
    // fixed は standard 止まりで Jev が上げたのに fixed_rule: 誤帰属。
    let mut o = obs(e("fixed_rule"));
    o.fixed_lane = Some("standard".into());
    assert!(matches!(judge(&o).attribution, Attribution::Wrong(_)));
    // 逆: fixed だけで届くのに classification_raise。
    let mut o = obs(e("classification_raise"));
    o.fixed_lane = Some("frontier".into());
    assert!(matches!(judge(&o).attribution, Attribution::Wrong(_)));
    // fallback なのに classification_raise。
    let o = obs(event(
        json!({"status":"fell_back_to_fixed","reason":"timeout"}),
        "standard",
        "frontier",
        "classification_raise",
    ));
    assert!(matches!(judge(&o).attribution, Attribution::Wrong(_)));
    // lane が動いていないのに classification_raise。
    let o = obs(event(ok(), "standard", "standard", "classification_raise"));
    assert!(matches!(judge(&o).attribution, Attribution::Wrong(_)));
}

// 本体の `assist_lane` の実出力（fixture の facts）を判定器へ通す。lane 引き上げの帰属が実装と整合するかの確認。
mod real_assist {
    use super::*;
    use task_core::Tier;
    use task_core::route_select::assist::{AssistInput, ClassificationEntry, assist_lane};
    use task_core::route_select::{
        ClassificationHint, ClassifierStatus, Fact, LaneFacts, LaneLimits, RoutingMode,
        SpecificityHint, WorkClass,
    };

    fn run(class: WorkClass, hint: SpecificityHint) -> Value {
        let facts = LaneFacts {
            class,
            procedure_clear: Fact::Yes,
            machine_checkable: Fact::Yes,
            reversible: Fact::Yes,
            failure_impact_small: Fact::Yes,
        };
        let entry = ClassificationEntry {
            status: ClassifierStatus::JevOk,
            hint: Some(ClassificationHint {
                specificity: hint,
                confidence: Some(0.9),
            }),
            model_snapshot: Some("v".into()),
            cost_usd: Some(0.001),
        };
        let limits = LaneLimits::default();
        let out = assist_lane(
            &AssistInput {
                mode: RoutingMode::DecisionAssisted,
                explicit: false,
                base: Tier::Standard,
                facts,
                entry: Some(&entry),
                limits: &limits,
            },
            |_| true,
        );
        json!({"record": {"lane_assist": serde_json::to_value(out.record.unwrap()).unwrap()}})
    }

    #[test]
    fn jev_raise_standard_to_frontier_is_classification_raise() {
        // fixed 規則は Routine・全条件 Yes で cheap（base より下）。Jev が frontier を提案 → Jev の引き上げ。
        let e = run(
            WorkClass::Routine,
            SpecificityHint::NeedsInvestigationDesign,
        );
        assert_eq!(e["record"]["lane_assist"]["basis"], "classification_raise");
        let mut o = obs(e);
        o.fixed_lane = Some("cheap".into());
        assert_eq!(judge(&o).attribution, Attribution::Consistent);
    }

    #[test]
    fn fixed_rule_frontier_with_jev_agreeing_is_fixed_rule_but_not_distinguishable_from_record() {
        // fixed 規則（HardInvestigation）が frontier を出し、Jev も frontier: 引き上げは fixed に帰属（反事実として正しい）。
        let e = run(
            WorkClass::HardInvestigation,
            SpecificityHint::NeedsInvestigationDesign,
        );
        let la = &e["record"]["lane_assist"];
        assert_eq!(
            (
                la["base_lane"].clone(),
                la["final_lane"].clone(),
                la["basis"].clone()
            ),
            (json!("standard"), json!("frontier"), json!("fixed_rule"))
        );
        // 記録には fixed 規則の lane が無いので、記録だけでは「fixed が上げた」か「Jev が上げたのに誤帰属」かを区別できない。
        assert_eq!(
            judge(&obs(e.clone())).attribution,
            Attribution::Unverifiable
        );
        let mut o = obs(e);
        o.fixed_lane = Some("frontier".into());
        assert_eq!(judge(&o).attribution, Attribution::Consistent);
    }
}
