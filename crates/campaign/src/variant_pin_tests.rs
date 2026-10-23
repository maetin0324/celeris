use super::*;
use crate::{
    lock::{GraderPin, LockedCase, check_successor},
    manifest::{
        Split,
        fixtures::{campaign, input},
    },
    record::DeliveredInput,
    variant::{REQUIRED_ROLES, VariantId, VariantRef},
};
use serde_json::json;
use task_core::model::Tier;

fn org_node(id: &str, attempts: u32) -> OrgNode {
    use task_core::{org::OrgKind, profile::Profile};
    let mut profile = Profile::default();
    profile.harnesses.allowed = vec!["claude_code".into()];
    profile.review.harness = Some("claude_code".into());
    profile.review.tier = Some(Tier::Standard);
    profile.budget.max_lane = Some(Tier::Standard);
    profile.budget.max_attempts = Some(attempts);
    profile.deny_tools = vec!["rm".into()];
    profile.permissions.approvals = vec!["merge".into()];
    OrgNode {
        id: id.into(),
        parent_id: (id != "cos").then(|| "cos".to_string()),
        name: id.into(),
        kind: if id == "cos" {
            OrgKind::Secretary
        } else {
            OrgKind::Department
        },
        genre: None,
        brief: String::new(),
        profile,
        position: 0,
        created_at: time::OffsetDateTime::UNIX_EPOCH,
        updated_at: time::OffsetDateTime::UNIX_EPOCH,
    }
}

fn profile(node: &str) -> EffectiveProfile {
    resolve(&[org_node("cos", 3), org_node(node, 3)], node)
}

pub(crate) fn org_with(nodes: &[&str], tweak: Option<(&str, u32)>) -> Value {
    let all: Vec<OrgNode> = nodes
        .iter()
        .map(|n| match tweak {
            Some((t, v)) if t == *n => org_node(n, v),
            _ => org_node(n, 3),
        })
        .collect();
    json!({
        "items": all,
        "effective_profiles": all.iter().map(|n| resolve(&all, &n.id)).collect::<Vec<_>>(),
        "lead_sessions": []
    })
}

pub(crate) fn org_json(nodes: &[&str]) -> Value {
    org_with(nodes, None)
}

pub(crate) fn b0() -> B0Inputs {
    B0Inputs {
        coding_node: "coding".into(),
        role_models: REQUIRED_ROLES
            .iter()
            .map(|r| (r.to_string(), "m".to_string()))
            .collect(),
        org: org_json(&["cos", "coding", "research"]),
    }
}

fn case(id: &str) -> LockedCase {
    LockedCase {
        id: id.into(),
        split: Split::Dev,
        implementation: false,
        initial: None,
        inputs: Vec::<DeliveredInput>::new(),
        grader: GraderPin {
            private_sha256: "5".repeat(64),
            grade_case_sha256: "6".repeat(64),
        },
        tests: vec![],
    }
}

fn build(m: &CampaignManifest, mid: &str) -> CampaignLock {
    CampaignLock::new("1", mid, m, vec![case("p0"), case("p1")])
        .expect("new")
        .seal()
        .expect("seal")
}

pub(crate) fn manifest() -> CampaignManifest {
    let mut m = campaign(&input(2, "dev"), Split::Dev, 2);
    m.snapshot.materials_sha256 = "e".repeat(64);
    m
}

/// B0（variant なし）の manifest / lock から基準を組み、spec を作って manifest / lock に結ぶ。
pub(crate) fn world(id: VariantId) -> (CampaignManifest, CampaignLock, VariantSpec) {
    let m0 = manifest();
    let base = baseline_pin(&m0, &build(&m0, "m-0"), &b0()).expect("baseline");
    let org = match id {
        VariantId::SpecOn => base.org.clone(),
        VariantId::SpecOff => OrgPin {
            nodes: vec!["cos".into(), "worker".into()],
            coding_node: "worker".into(),
            effective_profile_sha256: effective_profile_sha256(&profile("worker")).unwrap(),
        },
    };
    let spec = VariantSpec::new(id, org, base.non_target)
        .seal()
        .expect("seal");
    let mut m = m0;
    m.variant = Some(spec.to_ref().expect("ref"));
    let l = build(&m, "m-1");
    (m, l, spec)
}

pub(crate) fn live(spec: &VariantSpec) -> OrgSnapshot {
    let nodes: Vec<&str> = spec.org.nodes.iter().map(String::as_str).collect();
    OrgSnapshot::from_get_org(&org_json(&nodes)).expect("live")
}

#[test]
fn both_variants_pass_and_the_old_hash_is_unchanged() {
    for id in [VariantId::SpecOn, VariantId::SpecOff] {
        let (m, l, spec) = world(id);
        check_pin(&m, &l, &spec, &b0(), &live(&spec)).expect("pin");
        l.bind_to_manifest(&m).expect("bind");
        m.validate().ok();
    }
    // variant なしの manifest / lock は従来どおり直列化に `variant` を出さず、hash は参照の有無で変わる。
    let m0 = manifest();
    assert!(!serde_json::to_string(&m0).unwrap().contains("variant"));
    let l0 = build(&m0, "m-0");
    assert!(!serde_json::to_string(&l0).unwrap().contains("variant"));
    let (m, l, _) = world(VariantId::SpecOn);
    assert_ne!(m.compute_sha256().unwrap(), m0.compute_sha256().unwrap());
    assert_ne!(l.lock_hash().unwrap(), l0.lock_hash().unwrap());
}

#[test]
fn a_lock_for_a_different_variant_reference_does_not_bind() {
    let (m, l, spec) = world(VariantId::SpecOn);
    let mut m2 = m.clone();
    m2.variant = None;
    assert!(
        l.bind_to_manifest(&m2).is_err(),
        "lock has it, manifest not"
    );
    assert!(build(&m2, "m-1").bind_to_manifest(&m).is_err());
    let err = check_pin(&m, &build(&m2, "m-1"), &spec, &b0(), &live(&spec)).unwrap_err();
    assert!(matches!(err, VariantError::Missing(_)), "{err}");
    let err = check_pin(&m2, &build(&m2, "m-1"), &spec, &b0(), &live(&spec)).unwrap_err();
    assert!(matches!(err, VariantError::Missing(_)), "{err}");
}

#[test]
fn successor_cannot_swap_or_drop_the_variant() {
    let (_, on, _) = world(VariantId::SpecOn);
    let (m_off, _, _) = world(VariantId::SpecOff);
    let off = build(&m_off, "m-2");
    assert!(matches!(
        check_successor(&on, &off),
        Err(crate::lock::LockError::NotSuccessor(_))
    ));
    let mut m_none = manifest();
    m_none.variant = None;
    assert!(check_successor(&on, &build(&m_none, "m-3")).is_err());
    assert!(check_successor(&build(&m_none, "m-3"), &on).is_err());
}

#[test]
fn a_resealed_spec_with_non_target_drift_is_rejected_even_if_manifest_and_lock_follow() {
    let (m0, _, spec) = world(VariantId::SpecOn);
    for edit in [
        |s: &mut VariantSpec| s.non_target.budget.max_tokens += 1,
        |s: &mut VariantSpec| s.non_target.kb_snapshot = "kb@other".into(),
        |s: &mut VariantSpec| {
            s.non_target
                .role_models
                .insert("reviewer".into(), "x".into());
        },
        |s: &mut VariantSpec| s.non_target.safety.approvals.clear(),
    ] {
        let mut s = spec.clone();
        edit(&mut s);
        let s = s.seal().unwrap(); // 再 seal して自己 hash は整合させる
        let mut m = m0.clone();
        m.variant = Some(s.to_ref().unwrap());
        let l = build(&m, "m-1");
        let err = check_pin(&m, &l, &s, &b0(), &live(&s)).unwrap_err();
        assert!(matches!(err, VariantError::NonTargetDiff(_)), "{err}");
    }
}

#[test]
fn spec_swap_tamper_and_selection_drift_are_rejected() {
    let (m, l, spec) = world(VariantId::SpecOn);
    // 内容改変（未再 seal）。
    let mut t = spec.clone();
    t.non_target.budget.max_tokens += 1;
    assert!(matches!(
        check_pin(&m, &l, &t, &b0(), &live(&spec)).unwrap_err(),
        VariantError::ContentHash(_)
    ));
    // 別 variant の spec を同じ manifest に差す。
    let (_, _, off) = world(VariantId::SpecOff);
    assert!(matches!(
        check_pin(&m, &l, &off, &b0(), &live(&off)).unwrap_err(),
        VariantError::ContentHash(_)
    ));
    // 参照の selection hash がずれた manifest は構造検査で拒否。
    let mut bad = m.clone();
    bad.variant = Some(VariantRef {
        selection_sha256: "0".repeat(64),
        ..m.variant.clone().unwrap()
    });
    assert!(bad.validate().is_err());
}

#[test]
fn baseline_ignores_the_spec_and_rejects_missing_or_inconsistent_b0() {
    let m = manifest();
    let l = build(&m, "m-0");
    // B0 の org 応答の情報不足。
    for edit in [
        |v: &mut Value| {
            v.as_object_mut().unwrap().remove("effective_profiles");
        },
        |v: &mut Value| {
            v["effective_profiles"].as_array_mut().unwrap().pop();
        },
        |v: &mut Value| v["items"] = json!([]),
    ] {
        let mut b = b0();
        edit(&mut b.org);
        assert!(matches!(
            baseline_pin(&m, &l, &b).unwrap_err(),
            VariantError::Org(_)
        ));
    }
    let mut b = b0();
    b.coding_node = "cos".into();
    assert!(baseline_pin(&m, &l, &b).is_err());
    b.coding_node = "ghost".into();
    assert!(baseline_pin(&m, &l, &b).is_err());
    // 役割モデルの欠落。
    let mut b = b0();
    b.role_models.remove("planner");
    assert!(matches!(
        baseline_pin(&m, &l, &b).unwrap_err(),
        VariantError::Missing(_)
    ));
    // lock が別 manifest のもの。
    let mut other = m.clone();
    other.budget.max_tokens += 1;
    assert!(matches!(
        baseline_pin(&other, &l, &b0()).unwrap_err(),
        VariantError::Baseline(_)
    ));
}

#[test]
fn live_org_drift_is_rejected() {
    let (m, l, spec) = world(VariantId::SpecOn);
    let extra = OrgSnapshot::from_get_org(&org_json(&["cos", "coding", "research", "x"])).unwrap();
    assert!(matches!(
        check_pin(&m, &l, &spec, &b0(), &extra).unwrap_err(),
        VariantError::Org(_)
    ));
    let drift = OrgSnapshot::from_get_org(&org_with(
        &["cos", "coding", "research"],
        Some(("coding", 2)),
    ))
    .unwrap();
    assert!(matches!(
        check_pin(&m, &l, &spec, &b0(), &drift).unwrap_err(),
        VariantError::Org(_)
    ));
}

#[test]
fn b0_is_read_only_with_the_approved_sha256() {
    let text = serde_json::to_vec(&json!({
        "coding_node": "coding",
        "role_models": b0().role_models,
        "org": b0().org,
    }))
    .unwrap();
    let sha = format!("{:x}", Sha256::digest(&text));
    assert!(B0Inputs::from_sealed(&text, &sha).is_ok());
    assert!(matches!(
        B0Inputs::from_sealed(&text, &"0".repeat(64)),
        Err(VariantError::ContentHash(_))
    ));
    for bad in ["", "abc", &sha.to_uppercase()] {
        assert!(matches!(
            B0Inputs::from_sealed(&text, bad),
            Err(VariantError::Missing(_))
        ));
    }
    let mut t = text.clone();
    t.push(b' ');
    assert!(B0Inputs::from_sealed(&t, &sha).is_err());
}

#[test]
fn all_nodes_and_the_shared_digest_pin_b0_for_both_variants() {
    for id in [VariantId::SpecOn, VariantId::SpecOff] {
        let (_, _, spec) = world(id);
        let b0s = OrgSnapshot::from_get_org(&b0().org).unwrap();
        let nodes: Vec<&str> = spec.org.nodes.iter().map(String::as_str).collect();
        let live = OrgSnapshot::from_get_org(&org_json(&nodes)).unwrap();
        check_b0_org(&spec, &b0s, &live).expect("same");
        let cos = OrgSnapshot::from_get_org(&org_with(&nodes, Some(("cos", 2)))).unwrap();
        assert!(check_b0_org(&spec, &b0s, &cos).is_err(), "{id:?} cos drift");
    }
    let b0s = OrgSnapshot::from_get_org(&b0().org).unwrap();
    // 同じ digest を task-core の共有関数からも得る（二重実装なし）。
    let nodes: Vec<OrgNode> = ["cos", "coding", "research"]
        .iter()
        .map(|n| org_node(n, 3))
        .collect();
    assert_eq!(
        b0s.digest().unwrap(),
        task_core::variant_wire::org_effective_digest(&nodes).unwrap()
    );
    // 応答の実効 profile が items と食い違えば拒否。
    let mut bad = org_json(&["cos", "coding"]);
    bad["effective_profiles"][1]["max_attempts"] = json!(1);
    assert!(OrgSnapshot::from_get_org(&bad).is_err());
}
