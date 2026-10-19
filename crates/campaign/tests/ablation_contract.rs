//! 専門分担 variant の契約（型・hash・B0 との同値）。外部 LLM・ネットワークなし。
//! 固定 selection は変更しない。manifest / lock / POST への配線は後続 WU。
use std::collections::BTreeMap;

use celeris_campaign::{
    lock::GraderPin,
    manifest::{Budget, Cap, Config, ConfigId, Termination, TokenBasis},
    variant::*,
};
use task_core::{model::Tier, profile::EffectiveProfile};

fn profile(node: &str) -> EffectiveProfile {
    EffectiveProfile {
        node_id: node.into(),
        chain: vec!["cos".into(), node.into()],
        harnesses_allowed: vec!["claude_code".into()],
        review_harness: Some("claude_code".into()),
        review_tier: Some(Tier::Standard),
        max_lane: Some(Tier::Standard),
        max_attempts: Some(3),
        deny_tools: vec!["rm".into()],
        approvals: vec!["merge".into()],
        ..Default::default()
    }
}

fn config(id: ConfigId) -> Config {
    Config {
        id,
        harness: "claude_code".into(),
        harness_version: "1".into(),
        model: "sonnet".into(),
        model_version: "5.5".into(),
        policy_version: "p1".into(),
        routing: None,
    }
}

fn non_target() -> NonTargetPin {
    let (review, safety) = NonTargetPin::profile_pins(&profile("coding")).unwrap();
    NonTargetPin {
        configs: vec![
            config(ConfigId::A),
            config(ConfigId::B),
            config(ConfigId::C),
        ],
        role_models: REQUIRED_ROLES
            .iter()
            .map(|r| (r.to_string(), "sonnet".to_string()))
            .collect(),
        review,
        safety,
        materials_sha256: "m".repeat(8),
        kb_snapshot: "kb1".into(),
        budget: Budget {
            token_basis: TokenBasis::InputOutput,
            max_tokens: 1000,
            max_wall_secs: 100,
            max_disk_bytes: 1 << 20,
            max_parallel: 1,
            per_case: Cap {
                max_tokens: 100,
                max_wall_secs: 10,
            },
            per_arm: Cap {
                max_tokens: 500,
                max_wall_secs: 50,
            },
        },
        termination: Termination {
            trial_timeout_secs: 60,
            max_attempts_per_trial: 2,
            stop_on_budget: true,
        },
        graders: BTreeMap::from([(
            "c1".to_string(),
            GraderPin {
                private_sha256: "p".into(),
                grade_case_sha256: "g".into(),
            },
        )]),
    }
}

fn baseline() -> BaselinePin {
    BaselinePin {
        org: OrgPin {
            nodes: vec!["cos".into(), "coding".into(), "research".into()],
            coding_node: "coding".into(),
            effective_profile_sha256: effective_profile_sha256(&profile("coding")).unwrap(),
        },
        non_target: non_target(),
    }
}

fn spec_on() -> VariantSpec {
    let b = baseline();
    VariantSpec::new(VariantId::SpecOn, b.org, b.non_target)
        .seal()
        .unwrap()
}

fn spec_off() -> VariantSpec {
    let b = baseline();
    let org = OrgPin {
        nodes: vec!["cos".into(), "worker".into()],
        coding_node: "worker".into(),
        effective_profile_sha256: effective_profile_sha256(&profile("worker")).unwrap(),
    };
    VariantSpec::new(VariantId::SpecOff, org, b.non_target)
        .seal()
        .unwrap()
}

#[test]
fn selection_is_fixed_and_matches_its_hash() {
    assert_eq!(selection_json_sha256(), selection_sha256());
    assert_eq!(
        selection_sha256(),
        "b6132a4456d00be83fa03bf3a54deaaec5b9874fa7bc0397c0d8bdd8937964bc"
    );
    let v: serde_json::Value = serde_json::from_str(SELECTION_JSON).unwrap();
    assert_eq!(v["comparison_set"]["id"], COMPARISON_ID);
    assert_eq!(v["mechanism"], "specialization");
}

#[test]
fn both_variants_verify_against_b0_and_profile_equivalence_ignores_node_and_chain() {
    let b = baseline();
    spec_on().verify(&b).unwrap();
    spec_off().verify(&b).unwrap();
    assert_eq!(
        effective_profile_sha256(&profile("coding")).unwrap(),
        effective_profile_sha256(&profile("worker")).unwrap()
    );
}

#[test]
fn content_hash_is_stable_and_ignores_node_order() {
    let a = spec_off();
    assert_eq!(
        a.content_sha256().unwrap(),
        spec_off().content_sha256().unwrap()
    );
    let mut r = a.clone();
    r.org.nodes.reverse();
    assert_eq!(r.content_sha256().unwrap(), a.sha256);
    assert_ne!(spec_on().sha256, spec_off().sha256);
    let r = a.to_ref().unwrap();
    assert_eq!(
        (r.comparison_id.as_str(), r.id),
        (COMPARISON_ID, VariantId::SpecOff)
    );
    assert_eq!(serde_json::to_value(r.id).unwrap(), "M1-spec-off");
}

#[test]
fn content_tamper_is_detected_without_resealing() {
    let mut s = spec_on();
    s.non_target.kb_snapshot = "kb2".into();
    assert!(matches!(
        s.verify(&baseline()),
        Err(VariantError::ContentHash(_))
    ));
    let mut u = spec_on();
    u.sha256.clear();
    assert!(matches!(
        u.verify(&baseline()),
        Err(VariantError::ContentHash(_))
    ));
    assert!(u.to_ref().is_err());
}

#[test]
fn selection_hash_tamper_is_detected_even_when_resealed() {
    let mut s = spec_on();
    s.selection_sha256 = "0".repeat(64);
    assert!(matches!(
        s.clone().verify(&baseline()),
        Err(VariantError::ContentHash(_)) | Err(VariantError::SelectionMismatch(_))
    ));
    let s = s.seal().unwrap();
    assert!(matches!(
        s.verify(&baseline()),
        Err(VariantError::SelectionMismatch(_))
    ));
}

#[test]
fn resealed_non_target_differences_are_rejected_against_the_fixed_baseline() {
    type Edit = fn(&mut NonTargetPin);
    let edits: [(&str, Edit); 9] = [
        ("configs", |n| n.configs[1].model = "opus".into()),
        ("role_models", |n| {
            n.role_models.insert("reviewer".into(), "haiku".into());
        }),
        ("review", |n| n.review.tier = "Cheap".into()),
        ("safety", |n| n.safety.deny_tools.clear()),
        ("materials_sha256", |n| n.materials_sha256 = "other".into()),
        ("kb_snapshot", |n| n.kb_snapshot = "kb2".into()),
        ("budget", |n| n.budget.per_case.max_tokens += 1),
        ("termination", |n| n.termination.max_attempts_per_trial += 1),
        ("graders", |n| {
            n.graders.get_mut("c1").unwrap().private_sha256 = "x".into();
        }),
    ];
    for (name, edit) in edits {
        for mut s in [spec_on(), spec_off()] {
            edit(&mut s.non_target);
            let s = s.seal().unwrap(); // 自己 hash を作り直しても
            match s.verify(&baseline()) {
                Err(VariantError::NonTargetDiff(m)) => assert!(m.contains(name), "{name}: {m}"),
                o => panic!("{name}: {o:?}"),
            }
        }
    }
}

#[test]
fn missing_or_unknown_inputs_are_rejected_not_defaulted() {
    let mut s = spec_on();
    s.non_target.role_models.remove("planner");
    let s = s.seal().unwrap();
    assert!(
        matches!(s.verify(&baseline()), Err(VariantError::Missing(m)) if m.contains("planner"))
    );

    let mut s = spec_on();
    s.non_target.kb_snapshot = "unknown".into();
    assert!(matches!(
        s.seal().unwrap().verify(&baseline()),
        Err(VariantError::Missing(_))
    ));

    let mut s = spec_on();
    s.non_target.graders.clear();
    assert!(matches!(
        s.seal().unwrap().verify(&baseline()),
        Err(VariantError::Missing(_))
    ));

    // 基準側の欠落も成功にしない。
    let mut b = baseline();
    b.non_target.configs.clear();
    assert!(matches!(
        spec_on().verify(&b),
        Err(VariantError::Missing(_))
    ));

    // profile に review / lane 上限が無ければ pin を作れない。
    let mut p = profile("coding");
    p.max_lane = None;
    assert!(matches!(
        NonTargetPin::profile_pins(&p),
        Err(VariantError::Missing(_))
    ));
}

#[test]
fn org_shape_and_profile_equivalence_are_enforced() {
    let b = baseline();
    let mut off = spec_off();
    off.org.nodes.push("extra".into());
    assert!(matches!(
        off.seal().unwrap().verify(&b),
        Err(VariantError::Org(_))
    ));

    let mut off = spec_off();
    off.org.coding_node = "cos".into();
    off.org.nodes = vec!["cos".into()];
    assert!(matches!(
        off.seal().unwrap().verify(&b),
        Err(VariantError::Org(_))
    ));

    let mut off = spec_off();
    off.org.effective_profile_sha256 = "f".repeat(64); // 再 seal しても B0 の基準と合わない
    assert!(matches!(
        off.seal().unwrap().verify(&b),
        Err(VariantError::Org(_))
    ));

    let mut on = spec_on();
    on.org.nodes.pop();
    assert!(matches!(
        on.seal().unwrap().verify(&b),
        Err(VariantError::Org(_))
    ));

    let mut s = spec_on();
    s.comparison_id = "other".into();
    assert!(matches!(
        s.seal().unwrap().verify(&b),
        Err(VariantError::Schema(_))
    ));
}
