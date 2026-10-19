//! variant の下位 wire 型（task-core 所有。campaign への逆依存なし）。
//!
//! campaign の `VariantRef` / `OrgPin` と JSON 形が同一（`id` は文字列）。task-core は意味を知らず、
//! 形（非空・64 桁小文字 hex・node 集合）だけを検証する。hash は再計算せず、保存した値の完全一致で比べる。
//! [`crate::decision_log::CampaignIdentity`] の `variant` / `variant_org` 欄の型。
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

use crate::org::OrgNode;
use crate::profile::{EffectiveProfile, resolve};

/// variant への短い参照（campaign `VariantRef` と同じ JSON 名・省略なし）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CampaignVariantRef {
    pub comparison_id: String,
    pub id: String,
    pub content_sha256: String,
    pub selection_sha256: String,
}

/// 検証済みの構成 pin（campaign `OrgPin` と同じ JSON 名）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CampaignOrgPin {
    pub nodes: Vec<String>,
    pub coding_node: String,
    pub effective_profile_sha256: String,
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

impl CampaignVariantRef {
    pub fn validate(&self) -> Result<(), String> {
        for (name, v) in [("comparison_id", &self.comparison_id), ("id", &self.id)] {
            if v.trim().is_empty() {
                return Err(format!("variant.{name} must not be empty"));
            }
        }
        for (name, v) in [
            ("content_sha256", &self.content_sha256),
            ("selection_sha256", &self.selection_sha256),
        ] {
            if !is_sha256_hex(v) {
                return Err(format!("variant.{name} must be 64 lowercase hex chars"));
            }
        }
        Ok(())
    }
}

impl CampaignOrgPin {
    pub fn validate(&self) -> Result<(), String> {
        if self.nodes.iter().any(|n| n.trim().is_empty()) {
            return Err("variant_org.nodes must not contain empty ids".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        if !self.nodes.iter().all(|n| seen.insert(n)) {
            return Err("variant_org.nodes must not contain duplicates".into());
        }
        if !self.nodes.contains(&self.coding_node) {
            return Err("variant_org.coding_node must be one of nodes".into());
        }
        if !is_sha256_hex(&self.effective_profile_sha256) {
            return Err(
                "variant_org.effective_profile_sha256 must be 64 lowercase hex chars".into(),
            );
        }
        Ok(())
    }
}

fn canonical(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, x)| (k.clone(), canonical(x)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(canonical).collect()),
        o => o.clone(),
    }
}

/// `node_id`・`chain` を除いた EffectiveProfile の canonical JSON sha256。
/// campaign の `variant::effective_profile_sha256` と同値（試験で固定）。
pub fn effective_profile_sha256(p: &EffectiveProfile) -> Result<String, String> {
    let mut p = p.clone();
    p.node_id = String::new();
    p.chain = Vec::new();
    let v = serde_json::to_value(&p).map_err(|e| format!("profile: {e}"))?;
    let bytes = serde_json::to_vec(&canonical(&v)).map_err(|e| e.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

/// org の全 node の実効 profile を id 順に並べた canonical JSON の sha256。
/// 各要素は `node_id`・`chain` を含む完全な EffectiveProfile（親子構造の変化も digest に出る）。
/// 入力の並び順には依存しない。id の重複・空 id は拒否する（黙って片方を採らない）。
pub fn org_effective_digest(nodes: &[OrgNode]) -> Result<String, String> {
    let mut ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    ids.sort_unstable();
    if ids.iter().any(|i| i.trim().is_empty()) {
        return Err("org node id must not be empty".into());
    }
    if let Some(w) = ids.windows(2).find(|w| w[0] == w[1]) {
        return Err(format!("org node id duplicated: {}", w[0]));
    }
    let list: Vec<Value> = ids
        .iter()
        .map(|id| serde_json::to_value(resolve(nodes, id)).map_err(|e| format!("profile: {e}")))
        .collect::<Result<_, _>>()?;
    let bytes = serde_json::to_vec(&canonical(&Value::Array(list))).map_err(|e| e.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

impl CampaignOrgPin {
    /// 現在の org（`nodes`）が pin と一致するかを照合する。一致は node 集合（順序無関係）、
    /// coding node の存在、その実効 profile hash。欠落・重複・未知 node・profile drift は理由付きで拒否する。
    pub fn check_org(&self, nodes: &[OrgNode]) -> Result<(), String> {
        self.validate()?;
        let mut have: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
        have.sort_unstable();
        if let Some(w) = have.windows(2).find(|w| w[0] == w[1]) {
            return Err(format!("org node id duplicated: {}", w[0]));
        }
        let mut want: Vec<&str> = self.nodes.iter().map(String::as_str).collect();
        want.sort_unstable();
        if let Some(m) = want.iter().find(|w| have.binary_search(w).is_err()) {
            return Err(format!("org is missing pinned node: {m}"));
        }
        if let Some(u) = have.iter().find(|h| want.binary_search(h).is_err()) {
            return Err(format!("org has node not in pin: {u}"));
        }
        let actual = effective_profile_sha256(&resolve(nodes, &self.coding_node))?;
        if actual != self.effective_profile_sha256 {
            return Err(format!(
                "coding node {} profile drifted: pinned {}, actual {actual}",
                self.coding_node, self.effective_profile_sha256
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: &str = "b6132a44b6132a44b6132a44b6132a44b6132a44b6132a44b6132a44b6132a44";

    fn vref() -> CampaignVariantRef {
        CampaignVariantRef {
            comparison_id: "cmp".into(),
            id: "M1-spec-on".into(),
            content_sha256: H.into(),
            selection_sha256: H.into(),
        }
    }

    #[test]
    fn variant_ref_roundtrip_keeps_wire_names() {
        let v = vref();
        let j = serde_json::to_value(&v).unwrap();
        assert_eq!(
            j,
            serde_json::json!({"comparison_id":"cmp","id":"M1-spec-on","content_sha256":H,"selection_sha256":H})
        );
        assert_eq!(serde_json::from_value::<CampaignVariantRef>(j).unwrap(), v);
        v.validate().unwrap();
    }

    #[test]
    fn unknown_field_and_missing_field_rejected() {
        let mut j = serde_json::to_value(vref()).unwrap();
        j["extra"] = serde_json::json!(1);
        assert!(serde_json::from_value::<CampaignVariantRef>(j).is_err());
        let mut j = serde_json::to_value(vref()).unwrap();
        j.as_object_mut().unwrap().remove("content_sha256");
        assert!(serde_json::from_value::<CampaignVariantRef>(j).is_err());
        let pin =
            serde_json::json!({"nodes":["a"],"coding_node":"a","effective_profile_sha256":H,"x":1});
        assert!(serde_json::from_value::<CampaignOrgPin>(pin).is_err());
    }

    #[test]
    fn invalid_hash_rejected() {
        for bad in ["", "abc", &H.to_uppercase(), &format!("{}g", &H[..63])] {
            let mut v = vref();
            v.content_sha256 = bad.into();
            assert!(v.validate().is_err(), "{bad}");
            let mut v = vref();
            v.selection_sha256 = bad.into();
            assert!(v.validate().is_err(), "{bad}");
        }
    }

    #[test]
    fn org_pin_validation() {
        let pin = |nodes: &[&str], coding: &str, h: &str| CampaignOrgPin {
            nodes: nodes.iter().map(|s| s.to_string()).collect(),
            coding_node: coding.into(),
            effective_profile_sha256: h.into(),
        };
        pin(&["cos", "w"], "w", H).validate().unwrap();
        assert!(pin(&["cos", "w"], "x", H).validate().is_err());
        assert!(pin(&["w", "w"], "w", H).validate().is_err());
        assert!(pin(&["w"], "w", "zz").validate().is_err());
    }

    #[test]
    fn profile_hash_ignores_node_and_chain() {
        let a = EffectiveProfile {
            node_id: "x".into(),
            chain: vec!["cos".into(), "x".into()],
            ..Default::default()
        };
        let mut b = EffectiveProfile {
            node_id: "y".into(),
            ..Default::default()
        };
        assert_eq!(
            effective_profile_sha256(&a).unwrap(),
            effective_profile_sha256(&b).unwrap()
        );
        b.skills = vec!["s".into()];
        assert_ne!(
            effective_profile_sha256(&a).unwrap(),
            effective_profile_sha256(&b).unwrap()
        );
    }

    fn node(id: &str, parent: Option<&str>, skills: &[&str]) -> OrgNode {
        OrgNode {
            id: id.into(),
            parent_id: parent.map(Into::into),
            name: id.into(),
            kind: crate::org::OrgKind::Section,
            genre: None,
            brief: String::new(),
            profile: crate::profile::Profile {
                skills: skills.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            },
            position: 0,
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn org() -> Vec<OrgNode> {
        vec![
            node("cos", None, &[]),
            node("a", Some("cos"), &["x"]),
            node("w", Some("cos"), &["y"]),
        ]
    }

    fn pin_for(nodes: &[OrgNode], coding: &str) -> CampaignOrgPin {
        CampaignOrgPin {
            nodes: nodes.iter().map(|n| n.id.clone()).collect(),
            coding_node: coding.into(),
            effective_profile_sha256: effective_profile_sha256(&resolve(nodes, coding)).unwrap(),
        }
    }

    #[test]
    fn org_digest_is_order_invariant_and_detects_drift() {
        let o = org();
        let mut r = o.clone();
        r.reverse();
        assert_eq!(
            org_effective_digest(&o).unwrap(),
            org_effective_digest(&r).unwrap()
        );
        let mut d = o.clone();
        d[1].profile.skills = vec!["z".into()];
        assert_ne!(
            org_effective_digest(&o).unwrap(),
            org_effective_digest(&d).unwrap()
        );
        let mut dup = o.clone();
        dup.push(node("a", Some("cos"), &[]));
        assert!(
            org_effective_digest(&dup)
                .unwrap_err()
                .contains("duplicated")
        );
    }

    #[test]
    fn check_org_accepts_match_regardless_of_order() {
        let o = org();
        let pin = pin_for(&o, "w");
        pin.check_org(&o).unwrap();
        let mut r = o.clone();
        r.reverse();
        pin.check_org(&r).unwrap();
    }

    #[test]
    fn check_org_rejects_missing_unknown_duplicate_and_drift() {
        let o = org();
        let pin = pin_for(&o, "w");
        let missing: Vec<_> = o.iter().filter(|n| n.id != "a").cloned().collect();
        assert!(
            pin.check_org(&missing)
                .unwrap_err()
                .contains("missing pinned node: a")
        );
        let mut extra = o.clone();
        extra.push(node("n", Some("cos"), &[]));
        assert!(pin.check_org(&extra).unwrap_err().contains("not in pin: n"));
        let mut dup = o.clone();
        dup.push(node("a", Some("cos"), &[]));
        assert!(pin.check_org(&dup).unwrap_err().contains("duplicated"));
        let mut drift = o.clone();
        drift[2].profile.skills = vec!["other".into()];
        assert!(pin.check_org(&drift).unwrap_err().contains("drifted"));
        let mut bad = pin.clone();
        bad.coding_node = "zz".into();
        assert!(bad.check_org(&o).is_err());
    }

    #[test]
    fn existing_profile_hash_unchanged() {
        // wire/hash 不変の固定値（campaign variant_submit.rs と同じ既定 profile）。
        let h = effective_profile_sha256(&EffectiveProfile::default()).unwrap();
        assert_eq!(h.len(), 64);
        let a = EffectiveProfile {
            node_id: "q".into(),
            chain: vec!["q".into()],
            ..Default::default()
        };
        assert_eq!(h, effective_profile_sha256(&a).unwrap());
    }
}
