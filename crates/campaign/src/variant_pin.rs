//! 専門分担 variant の固定: B0 の実体から独立に基準（`BaselinePin`）を組み、manifest / lock の参照と spec を照合する。
//! 実 API は呼ばない。GET `/api/v1/org` の応答（`items` と `effective_profiles`）を値として受ける。
//! ADR 2026-10-19-specialization-variant。
use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use task_core::{
    org::OrgNode,
    profile::{EffectiveProfile, resolve},
};

use crate::{
    lock::CampaignLock,
    manifest::CampaignManifest,
    variant::{
        BaselinePin, NonTargetPin, OrgPin, ROOT_NODE, VariantError, VariantId, VariantSpec,
        effective_profile_sha256,
    },
};

/// `GET /api/v1/org` の応答から読んだ組織。継承解決済みの EffectiveProfile を node ごとに持つ。
#[derive(Clone, Debug)]
pub struct OrgSnapshot {
    nodes: Vec<OrgNode>,
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
        let mut nodes = Vec::new();
        for it in items {
            let n: OrgNode = serde_json::from_value(it.clone())
                .map_err(|e| bad(&format!("item is not an org node: {e}")))?;
            if n.id.is_empty() {
                return Err(bad("an item has no id"));
            }
            if !ids.insert(n.id.clone()) {
                return Err(bad(&format!("duplicate node {}", n.id)));
            }
            nodes.push(n);
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
        // 応答の実効 profile が items から解いた値と同じこと（全 node digest の入力と食い違わせない）。
        for n in &nodes {
            if profiles.get(&n.id) != Some(&resolve(&nodes, &n.id)) {
                return Err(bad(&format!(
                    "effective profile of {} differs from the one resolved from items",
                    n.id
                )));
            }
        }
        Ok(OrgSnapshot { nodes, profiles })
    }

    /// 全 node の実効 profile digest。計算は task-core の共有関数（dispatcher / API と同じ）。
    pub fn digest(&self) -> Result<String, VariantError> {
        task_core::variant_wire::org_effective_digest(&self.nodes).map_err(VariantError::Org)
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

impl B0Inputs {
    /// 承認済み sha256（呼び出し側が明示。file の隣に置いた値は使わない）に一致する bytes だけを読む。
    /// sha256 が不正・不一致なら、中身を解釈する前に拒否する（欠落・改変は preflight の失敗）。
    pub fn from_sealed(bytes: &[u8], approved_sha256: &str) -> Result<Self, VariantError> {
        let ok = approved_sha256.len() == 64
            && approved_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !ok {
            return Err(VariantError::Missing(
                "approved sha256 of variant-b0.json must be 64 lowercase hex".into(),
            ));
        }
        let got = format!("{:x}", Sha256::digest(bytes));
        if got != approved_sha256 {
            return Err(VariantError::ContentHash(format!(
                "variant-b0.json sha256 {got} != approved {approved_sha256}"
            )));
        }
        serde_json::from_slice(bytes)
            .map_err(|e| VariantError::Baseline(format!("variant-b0.json: {e}")))
    }
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

/// B0 の全 node（根 cos を含む）と稼働 org の照合。非対象の node（coding 課以外・根 cos）が B0 から動いていないことを見る。
/// spec-on は全 node の digest（共有関数）が一致、spec-off は B0 と共通の node（根 cos 必須）の profile が一致。
pub fn check_b0_org(
    spec: &VariantSpec,
    b0: &OrgSnapshot,
    live: &OrgSnapshot,
) -> Result<(), VariantError> {
    match spec.variant {
        VariantId::SpecOn => {
            if b0.digest()? != live.digest()? {
                return Err(VariantError::Org(
                    "live org differs from B0 in some node's effective profile".into(),
                ));
            }
        }
        VariantId::SpecOff => {
            let mut seen_root = false;
            for id in live.nodes() {
                let (Some(l), Some(b)) = (live.profile(&id), b0.profile(&id)) else {
                    continue;
                };
                if id == ROOT_NODE {
                    seen_root = true;
                }
                if id != spec.org.coding_node
                    && effective_profile_sha256(l)? != effective_profile_sha256(b)?
                {
                    return Err(VariantError::Org(format!("live node {id} differs from B0")));
                }
            }
            if !seen_root {
                return Err(VariantError::Org(
                    "root cos is missing from B0 or live".into(),
                ));
            }
        }
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
    check_live_org(spec, live)?;
    check_b0_org(spec, &OrgSnapshot::from_get_org(&b0.org)?, live)
}

#[cfg(test)]
#[path = "variant_pin_tests.rs"]
pub(crate) mod tests;
