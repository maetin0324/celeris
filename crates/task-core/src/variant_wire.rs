//! variant の下位 wire 型（task-core 所有。campaign への逆依存なし）。
//!
//! campaign の `VariantRef` / `OrgPin` と JSON 形が同一（`id` は文字列）。task-core は意味を知らず、
//! 形（非空・64 桁小文字 hex・node 集合）だけを検証する。hash は再計算せず、保存した値の完全一致で比べる。
//! 既存の [`crate::decision_log::CampaignIdentity`] にはまだ欄を足さない（加算的な公開のみ）。
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

use crate::profile::EffectiveProfile;

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
        let mut a = EffectiveProfile::default();
        a.node_id = "x".into();
        a.chain = vec!["cos".into(), "x".into()];
        let mut b = EffectiveProfile::default();
        b.node_id = "y".into();
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
}
