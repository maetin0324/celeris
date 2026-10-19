//! 専門分担 variant の固定: B0 の実体から独立に基準（`BaselinePin`）を組み、manifest / lock の参照と spec を照合する。
//! 実 API は呼ばない。GET `/api/v1/org` の応答（`items` と `effective_profiles`）を値として受ける。
//! ADR 2026-10-19-specialization-variant。
use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::Value;
use task_core::profile::EffectiveProfile;

use crate::{
    lock::CampaignLock,
    manifest::CampaignManifest,
    variant::{
        BaselinePin, NonTargetPin, OrgPin, ROOT_NODE, VariantError, VariantSpec,
        effective_profile_sha256,
    },
};

/// `GET /api/v1/org` の応答から読んだ組織。継承解決済みの EffectiveProfile を node ごとに持つ。
#[derive(Clone, Debug)]
pub struct OrgSnapshot {
    profiles: BTreeMap<String, EffectiveProfile>,
}

impl OrgSnapshot {
    /// 情報不足（items / effective_profiles の欠落・不整合・重複）は拒否する。
    pub fn from_get_org(v: &Value) -> Result<Self, VariantError> {
        let bad = |m: &str| VariantError::Org(format!("GET org: {m}"));
        let items = v
            .get("items")
            .and_then(Value::as_array)
            .ok_or_else(|| bad("items is missing"))?;
        let profs = v
            .get("effective_profiles")
            .and_then(Value::as_array)
            .ok_or_else(|| bad("effective_profiles is missing"))?;
        if items.is_empty() {
            return Err(bad("items is empty"));
        }
        let mut ids = BTreeSet::new();
        for it in items {
            let id = it
                .get("id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| bad("an item has no id"))?;
            if !ids.insert(id.to_string()) {
                return Err(bad(&format!("duplicate node {id}")));
            }
        }
        let mut profiles = BTreeMap::new();
        for p in profs {
            let p: EffectiveProfile = serde_json::from_value(p.clone())
                .map_err(|e| bad(&format!("effective profile: {e}")))?;
            if profiles.insert(p.node_id.clone(), p).is_some() {
                return Err(bad("duplicate effective profile"));
            }
        }
        let have: BTreeSet<String> = profiles.keys().cloned().collect();
        if have != ids {
            return Err(bad("effective_profiles do not cover exactly the items"));
        }
        Ok(OrgSnapshot { profiles })
    }

    pub fn nodes(&self) -> Vec<String> {
        self.profiles.keys().cloned().collect()
    }

    pub fn profile(&self, node: &str) -> Option<&EffectiveProfile> {
        self.profiles.get(node)
    }

    /// `coding_node`（根 cos ではない既存 node）の `OrgPin`。
    pub fn org_pin(&self, coding_node: &str) -> Result<OrgPin, VariantError> {
        if coding_node == ROOT_NODE {
            return Err(VariantError::Org(
                "the root cos cannot be the coding node".into(),
            ));
        }
        let p = self
            .profile(coding_node)
            .ok_or_else(|| VariantError::Org(format!("coding node {coding_node} is not in org")))?;
        Ok(OrgPin {
            nodes: self.nodes(),
            coding_node: coding_node.to_string(),
            effective_profile_sha256: effective_profile_sha256(p)?,
        })
    }
}

/// B0 の実体の写し（`variant-b0.json`）。VariantSpec とは独立に保存した、B0 daemon の `GET /api/v1/org` と運用上の宣言。
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct B0Inputs {
    /// B0 で coding genre を受ける課。
    pub coding_node: String,
    /// 全役割（`REQUIRED_ROLES`）のモデル。
    pub role_models: BTreeMap<String, String>,
    /// B0 daemon の `GET /api/v1/org` 応答。
    pub org: Value,
}

/// B0 の manifest・lock・org から基準を組む。VariantSpec の値は一切使わない。
pub fn baseline_pin(
    manifest: &CampaignManifest,
    lock: &CampaignLock,
    b0: &B0Inputs,
) -> Result<BaselinePin, VariantError> {
    let base = |m: String| VariantError::Baseline(m);
    // B0 の実体どうしが一致していること（lock は manifest に結び付いている）。
    lock.bind_to_manifest(manifest)
        .map_err(|e| base(format!("lock does not bind to the manifest: {e}")))?;
    let n = manifest.normalized();
    let l = lock.normalized();
    if n.configs != l.execution.configs {
        return Err(base("manifest configs differ from the lock".into()));
    }
    let org = OrgSnapshot::from_get_org(&b0.org)?;
    let org_pin = org.org_pin(&b0.coding_node)?;
    let profile = org
        .profile(&b0.coding_node)
        .ok_or_else(|| VariantError::Org("coding node profile".into()))?;
    let (review, safety) = NonTargetPin::profile_pins(profile)?;
    let non_target = NonTargetPin {
        configs: l.execution.configs.clone(),
        role_models: b0.role_models.clone(),
        review,
        safety,
        materials_sha256: l.materials_sha256.clone(),
        kb_snapshot: l.kb_snapshot.clone(),
        budget: l.execution.budget.clone(),
        termination: l.execution.termination.clone(),
        graders: l
            .cases
            .iter()
            .map(|c| (c.id.clone(), c.grader.clone()))
            .collect(),
    };
    let b = BaselinePin {
        org: org_pin,
        non_target,
    };
    b.validate()?;
    Ok(b)
}

/// 稼働中 org（`GET /api/v1/org`）が spec の宣言と一致するか。node 集合と coding 課の EffectiveProfile。
pub fn check_live_org(spec: &VariantSpec, live: &OrgSnapshot) -> Result<(), VariantError> {
    let mut want = spec.org.nodes.clone();
    want.sort();
    if live.nodes() != want {
        return Err(VariantError::Org(format!(
            "live org nodes {:?} != variant nodes {want:?}",
            live.nodes()
        )));
    }
    let pin = live.org_pin(&spec.org.coding_node)?;
    if pin.effective_profile_sha256 != spec.org.effective_profile_sha256 {
        return Err(VariantError::Org(
            "live EffectiveProfile of the coding node differs from the variant".into(),
        ));
    }
    Ok(())
}

/// manifest・lock・spec・B0・稼働 org を全て照合する（preflight の入口）。
/// manifest の参照が無い・spec の `VariantRef` と違う（差し替え・再 seal）・lock が結び付いていない・B0 基準と違う場合は拒否。
pub fn check_pin(
    manifest: &CampaignManifest,
    lock: &CampaignLock,
    spec: &VariantSpec,
    b0: &B0Inputs,
    live: &OrgSnapshot,
) -> Result<(), VariantError> {
    let want = manifest
        .variant
        .as_ref()
        .ok_or_else(|| VariantError::Missing("manifest.variant".into()))?;
    want.check_fixed()?;
    if &spec.to_ref()? != want {
        return Err(VariantError::ContentHash(
            "the variant spec does not match the manifest reference".into(),
        ));
    }
    if lock.execution.variant.as_ref() != Some(want) {
        return Err(VariantError::Missing(
            "lock.execution.variant differs from the manifest reference".into(),
        ));
    }
    let baseline = baseline_pin(manifest, lock, b0)?;
    spec.verify(&baseline)?;
    check_live_org(spec, live)
}

#[cfg(test)]
mod tests {
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

    fn org_json(nodes: &[&str]) -> Value {
        json!({
            "items": nodes.iter().map(|n| json!({"id": n, "parent_id": null})).collect::<Vec<_>>(),
            "effective_profiles": nodes.iter().map(|n| profile(n)).collect::<Vec<_>>(),
            "lead_sessions": []
        })
    }

    fn b0() -> B0Inputs {
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

    fn manifest() -> CampaignManifest {
        let mut m = campaign(&input(2, "dev"), Split::Dev, 2);
        m.snapshot.materials_sha256 = "e".repeat(64);
        m
    }

    /// B0（variant なし）の manifest / lock から基準を組み、spec を作って manifest / lock に結ぶ。
    fn world(id: VariantId) -> (CampaignManifest, CampaignLock, VariantSpec) {
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

    fn live(spec: &VariantSpec) -> OrgSnapshot {
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
        let extra =
            OrgSnapshot::from_get_org(&org_json(&["cos", "coding", "research", "x"])).unwrap();
        assert!(matches!(
            check_pin(&m, &l, &spec, &b0(), &extra).unwrap_err(),
            VariantError::Org(_)
        ));
        let mut drift = org_json(&["cos", "coding", "research"]);
        drift["effective_profiles"][1]["max_attempts"] = json!(9);
        let drift = OrgSnapshot::from_get_org(&drift).unwrap();
        assert!(matches!(
            check_pin(&m, &l, &spec, &b0(), &drift).unwrap_err(),
            VariantError::Org(_)
        ));
    }
}
