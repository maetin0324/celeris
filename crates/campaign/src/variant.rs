//! 専門分担 variant（M1-specialization）の契約。ADR 2026-10-19-specialization-variant。
//! 型・内容 hash・B0 との同値検査だけを持つ。manifest / lock / POST への配線は後続 WU。
//! 比較は固定基準（B0 から作った `BaselinePin`）に対して行い、自己 hash の再 seal では差を正当化できない。
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use task_core::{
    profile::EffectiveProfile,
    variant_wire::{CampaignOrgPin, CampaignVariantRef},
};

use crate::{
    lock::GraderPin,
    manifest::{Budget, Config, Termination, canonical},
};

pub const VARIANT_SCHEMA: &str = "celeris.campaign-variant/1";
pub const COMPARISON_ID: &str = "M1-specialization";
pub const SELECTION_JSON: &str = include_str!("../fixtures/ablation/selection.json");
pub const SELECTION_SHA256_FILE: &str = include_str!("../fixtures/ablation/selection.sha256");
/// 根 node（spec-off の唯一の非根作業課の親）。
pub const ROOT_NODE: &str = "cos";
/// 全役割（planner / reviewer / cos / worker）のモデルを必ず宣言させる。
pub const REQUIRED_ROLES: [&str; 4] = ["planner", "reviewer", "cos", "worker"];

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum VariantId {
    #[serde(rename = "M1-spec-on")]
    SpecOn,
    #[serde(rename = "M1-spec-off")]
    SpecOff,
}

impl VariantId {
    pub fn as_str(self) -> &'static str {
        match self {
            VariantId::SpecOn => "M1-spec-on",
            VariantId::SpecOff => "M1-spec-off",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VariantError {
    /// 必要情報の欠落・unknown 値（既定値で成功にしない）。
    Missing(String),
    /// schema / comparison_id が違う。
    Schema(String),
    /// 同梱 selection.sha256 / selection.json と一致しない。
    SelectionMismatch(String),
    /// 未封印、または内容 hash が一致しない（改変）。
    ContentHash(String),
    /// org 構成が variant の定義に合わない。
    Org(String),
    /// 非対象の差分（欄名つき）。
    NonTargetDiff(String),
    /// B0 の実体（manifest / lock / org 応答）から基準を組めない・互いに矛盾する。
    Baseline(String),
}

impl std::fmt::Display for VariantError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (k, m) = match self {
            VariantError::Missing(m) => ("missing", m),
            VariantError::Schema(m) => ("schema", m),
            VariantError::SelectionMismatch(m) => ("selection_mismatch", m),
            VariantError::ContentHash(m) => ("content_hash", m),
            VariantError::Org(m) => ("org", m),
            VariantError::NonTargetDiff(m) => ("non_target_diff", m),
            VariantError::Baseline(m) => ("baseline", m),
        };
        write!(f, "variant {k}: {m}")
    }
}

impl std::error::Error for VariantError {}

fn sha_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// `selection.sha256` の値（空白除去）。
pub fn selection_sha256() -> String {
    SELECTION_SHA256_FILE.trim().to_string()
}

/// 同梱 `selection.json` の正規化 JSON（キー整列・compact）の sha256。`selection.sha256` はこの方式の値で、
/// `selection_sha256()` と一致しなければ固定が壊れている。
pub fn selection_json_sha256() -> String {
    match serde_json::from_str::<serde_json::Value>(SELECTION_JSON) {
        Ok(v) => canonical_sha(&v).unwrap_or_default(),
        Err(_) => String::new(),
    }
}

/// `node_id`・`chain` を除いた EffectiveProfile の canonical JSON sha256（spec-on の coding 課と spec-off の単一課の同値判定）。
/// 計算は task-core（`variant_wire`）が持つ。ここは同じ値を返す委譲で、二重実装を持たない。
pub fn effective_profile_sha256(p: &EffectiveProfile) -> Result<String, VariantError> {
    task_core::variant_wire::effective_profile_sha256(p).map_err(VariantError::Missing)
}

fn canonical_sha<T: Serialize>(v: &T) -> Result<String, VariantError> {
    let v = serde_json::to_value(v).map_err(|e| VariantError::Missing(e.to_string()))?;
    let bytes =
        serde_json::to_vec(&canonical(&v)).map_err(|e| VariantError::Missing(e.to_string()))?;
    Ok(sha_hex(&bytes))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OrgPin {
    /// org の node id 集合（整列して比較する）。
    pub nodes: Vec<String>,
    /// B0 で coding genre を受ける課（spec-off では唯一の非根作業課）。
    pub coding_node: String,
    pub effective_profile_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReviewPin {
    pub harness: String,
    pub tier: String,
    pub escalate_on_fail: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SafetyPin {
    pub max_lane: String,
    pub allowed_tiers: Vec<String>,
    pub max_attempts: u32,
    pub approvals: Vec<String>,
    pub deny_tools: Vec<String>,
    pub tools: Vec<String>,
    /// browser capability の canonical hash（無ければ "none"）。
    pub browser_sha256: String,
}

/// 非対象（variant で変えてはならない値）の写し。B0 の manifest / lock / profile から作る。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NonTargetPin {
    /// arm ごとの harness / model / 版（`ExecutionPin.configs`）。
    pub configs: Vec<Config>,
    /// 全役割のモデル（`REQUIRED_ROLES` を全て含む）。
    pub role_models: BTreeMap<String, String>,
    pub review: ReviewPin,
    pub safety: SafetyPin,
    pub materials_sha256: String,
    pub kb_snapshot: String,
    pub budget: Budget,
    pub termination: Termination,
    /// case id -> grader pin。
    pub graders: BTreeMap<String, GraderPin>,
}

fn nonempty(name: &str, v: &str) -> Result<(), VariantError> {
    let t = v.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("unknown") {
        return Err(VariantError::Missing(format!("{name} is empty or unknown")));
    }
    Ok(())
}

impl NonTargetPin {
    /// 欠落・unknown を拒否する（既定値で埋めない）。
    pub fn validate(&self) -> Result<(), VariantError> {
        if self.configs.is_empty() {
            return Err(VariantError::Missing("configs".into()));
        }
        for c in &self.configs {
            for (n, v) in [
                ("harness", &c.harness),
                ("harness_version", &c.harness_version),
                ("model", &c.model),
                ("model_version", &c.model_version),
                ("policy_version", &c.policy_version),
            ] {
                nonempty(&format!("config {:?} {n}", c.id), v)?;
            }
        }
        for r in REQUIRED_ROLES {
            match self.role_models.get(r) {
                Some(m) => nonempty(&format!("role_models.{r}"), m)?,
                None => return Err(VariantError::Missing(format!("role_models.{r}"))),
            }
        }
        nonempty("review.harness", &self.review.harness)?;
        nonempty("review.tier", &self.review.tier)?;
        nonempty("safety.max_lane", &self.safety.max_lane)?;
        nonempty("safety.browser_sha256", &self.safety.browser_sha256)?;
        if self.safety.max_attempts == 0 {
            return Err(VariantError::Missing("safety.max_attempts".into()));
        }
        nonempty("materials_sha256", &self.materials_sha256)?;
        nonempty("kb_snapshot", &self.kb_snapshot)?;
        if self.graders.is_empty() {
            return Err(VariantError::Missing("graders".into()));
        }
        for (id, g) in &self.graders {
            nonempty(&format!("graders.{id}.private_sha256"), &g.private_sha256)?;
            nonempty(
                &format!("graders.{id}.grade_case_sha256"),
                &g.grade_case_sha256,
            )?;
        }
        Ok(())
    }

    /// 非対象の profile 由来の欄（review / safety）を B0 coding 課の EffectiveProfile から作る。
    pub fn profile_pins(p: &EffectiveProfile) -> Result<(ReviewPin, SafetyPin), VariantError> {
        let tier = |t: &Option<task_core::model::Tier>, n: &str| -> Result<String, VariantError> {
            t.map(|t| format!("{t:?}"))
                .ok_or_else(|| VariantError::Missing(format!("profile.{n}")))
        };
        let review = ReviewPin {
            harness: p
                .review_harness
                .clone()
                .ok_or_else(|| VariantError::Missing("profile.review_harness".into()))?,
            tier: tier(&p.review_tier, "review_tier")?,
            escalate_on_fail: p.review_escalate_on_fail.unwrap_or(true),
        };
        let safety = SafetyPin {
            max_lane: tier(&p.max_lane, "max_lane")?,
            allowed_tiers: p.allowed_tiers.iter().map(|t| format!("{t:?}")).collect(),
            max_attempts: p
                .max_attempts
                .ok_or_else(|| VariantError::Missing("profile.max_attempts".into()))?,
            approvals: p.approvals.clone(),
            deny_tools: p.deny_tools.clone(),
            tools: p.tools.clone(),
            browser_sha256: match &p.browser {
                Some(b) => canonical_sha(b)?,
                None => "none".into(),
            },
        };
        Ok((review, safety))
    }

    /// 欄ごとの差分（名前の列）。空なら同一。
    pub fn diff(&self, other: &NonTargetPin) -> Vec<&'static str> {
        let mut d = Vec::new();
        macro_rules! cmp {
            ($($f:ident),*) => { $( if self.$f != other.$f { d.push(stringify!($f)); } )* };
        }
        cmp!(
            configs,
            role_models,
            review,
            safety,
            materials_sha256,
            kb_snapshot,
            budget,
            termination,
            graders
        );
        d
    }
}

/// B0 から作る固定基準。variant 側の自己申告ではなく、呼び出し元が B0 の実体から組み立てて渡す。
#[derive(Clone, Debug, PartialEq)]
pub struct BaselinePin {
    pub org: OrgPin,
    pub non_target: NonTargetPin,
}

impl BaselinePin {
    pub fn validate(&self) -> Result<(), VariantError> {
        self.non_target.validate()?;
        nonempty("baseline.org.coding_node", &self.org.coding_node)?;
        nonempty(
            "baseline.org.effective_profile_sha256",
            &self.org.effective_profile_sha256,
        )?;
        if !self.org.nodes.contains(&self.org.coding_node) {
            return Err(VariantError::Org(
                "baseline coding_node is not in nodes".into(),
            ));
        }
        if self.org.nodes.iter().any(|n| n == ROOT_NODE) && self.org.coding_node == ROOT_NODE {
            return Err(VariantError::Org(
                "the root cos cannot be the coding node".into(),
            ));
        }
        Ok(())
    }
}

/// manifest / lock / POST / GET が運ぶ短い参照。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VariantRef {
    pub comparison_id: String,
    pub id: VariantId,
    pub content_sha256: String,
    pub selection_sha256: String,
}

impl From<&VariantRef> for CampaignVariantRef {
    fn from(v: &VariantRef) -> Self {
        CampaignVariantRef {
            comparison_id: v.comparison_id.clone(),
            id: v.id.as_str().to_string(),
            content_sha256: v.content_sha256.clone(),
            selection_sha256: v.selection_sha256.clone(),
        }
    }
}

/// 未知の id は拒否する（既定値へ丸めない）。
impl TryFrom<&CampaignVariantRef> for VariantRef {
    type Error = VariantError;
    fn try_from(v: &CampaignVariantRef) -> Result<Self, VariantError> {
        let id = match v.id.as_str() {
            "M1-spec-on" => VariantId::SpecOn,
            "M1-spec-off" => VariantId::SpecOff,
            other => {
                return Err(VariantError::Schema(format!(
                    "unknown variant id {other:?}"
                )));
            }
        };
        Ok(VariantRef {
            comparison_id: v.comparison_id.clone(),
            id,
            content_sha256: v.content_sha256.clone(),
            selection_sha256: v.selection_sha256.clone(),
        })
    }
}

impl From<&OrgPin> for CampaignOrgPin {
    fn from(o: &OrgPin) -> Self {
        CampaignOrgPin {
            nodes: o.nodes.clone(),
            coding_node: o.coding_node.clone(),
            effective_profile_sha256: o.effective_profile_sha256.clone(),
        }
    }
}

impl From<&CampaignOrgPin> for OrgPin {
    fn from(o: &CampaignOrgPin) -> Self {
        OrgPin {
            nodes: o.nodes.clone(),
            coding_node: o.coding_node.clone(),
            effective_profile_sha256: o.effective_profile_sha256.clone(),
        }
    }
}

impl VariantRef {
    /// 固定 selection と一致するか（同梱 selection.json の再計算も確かめる）。manifest の構造検査から呼ぶ。
    pub fn check_fixed(&self) -> Result<(), VariantError> {
        if self.comparison_id != COMPARISON_ID {
            return Err(VariantError::Schema(format!(
                "variant.comparison_id must be {COMPARISON_ID}"
            )));
        }
        let fixed = selection_sha256();
        if selection_json_sha256() != fixed {
            return Err(VariantError::SelectionMismatch(
                "bundled selection.json != selection.sha256".into(),
            ));
        }
        if self.selection_sha256 != fixed {
            return Err(VariantError::SelectionMismatch(format!(
                "variant.selection_sha256 {} != {fixed}",
                self.selection_sha256
            )));
        }
        let h = &self.content_sha256;
        if h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(VariantError::ContentHash(
                "variant.content_sha256 is not a sha256".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct VariantSpec {
    pub schema: String,
    pub comparison_id: String,
    pub variant: VariantId,
    pub selection_sha256: String,
    pub org: OrgPin,
    pub non_target: NonTargetPin,
    /// 内容 hash。空なら未封印。
    #[serde(default)]
    pub sha256: String,
}

impl VariantSpec {
    /// 固定 selection と B0 基準から未封印の spec を作る。
    pub fn new(variant: VariantId, org: OrgPin, non_target: NonTargetPin) -> Self {
        VariantSpec {
            schema: VARIANT_SCHEMA.into(),
            comparison_id: COMPARISON_ID.into(),
            variant,
            selection_sha256: selection_sha256(),
            org,
            non_target,
            sha256: String::new(),
        }
    }

    /// `sha256` を空にして canonical JSON → sha256（`manifest::compute_sha256` と同じ手順）。
    pub fn content_sha256(&self) -> Result<String, VariantError> {
        let mut s = self.clone();
        s.sha256 = String::new();
        s.org.nodes.sort();
        canonical_sha(&s)
    }

    pub fn seal(mut self) -> Result<Self, VariantError> {
        self.sha256 = self.content_sha256()?;
        Ok(self)
    }

    pub fn check_seal(&self) -> Result<(), VariantError> {
        if self.sha256.is_empty() {
            return Err(VariantError::ContentHash("unsealed".into()));
        }
        let c = self.content_sha256()?;
        if c != self.sha256 {
            return Err(VariantError::ContentHash(format!(
                "recorded {} != computed {c}",
                self.sha256
            )));
        }
        Ok(())
    }

    pub fn to_ref(&self) -> Result<VariantRef, VariantError> {
        self.check_seal()?;
        Ok(VariantRef {
            comparison_id: self.comparison_id.clone(),
            id: self.variant,
            content_sha256: self.sha256.clone(),
            selection_sha256: self.selection_sha256.clone(),
        })
    }

    /// 完全検証。入力（baseline・spec）を全て検査してから B0 と比較する。再 seal しても通らない。
    pub fn verify(&self, baseline: &BaselinePin) -> Result<(), VariantError> {
        if self.schema != VARIANT_SCHEMA {
            return Err(VariantError::Schema(format!(
                "schema must be {VARIANT_SCHEMA}"
            )));
        }
        if self.comparison_id != COMPARISON_ID {
            return Err(VariantError::Schema(format!(
                "comparison_id must be {COMPARISON_ID}"
            )));
        }
        let fixed = selection_sha256();
        if selection_json_sha256() != fixed {
            return Err(VariantError::SelectionMismatch(
                "bundled selection.json != selection.sha256".into(),
            ));
        }
        if self.selection_sha256 != fixed {
            return Err(VariantError::SelectionMismatch(format!(
                "{} != {fixed}",
                self.selection_sha256
            )));
        }
        self.check_seal()?;
        self.non_target.validate()?;
        baseline.validate()?;
        nonempty("org.coding_node", &self.org.coding_node)?;
        nonempty(
            "org.effective_profile_sha256",
            &self.org.effective_profile_sha256,
        )?;
        let diff = self.non_target.diff(&baseline.non_target);
        if !diff.is_empty() {
            return Err(VariantError::NonTargetDiff(diff.join(",")));
        }
        if self.org.effective_profile_sha256 != baseline.org.effective_profile_sha256 {
            return Err(VariantError::Org(
                "effective_profile_sha256 differs from B0 coding node".into(),
            ));
        }
        let mut nodes = self.org.nodes.clone();
        nodes.sort();
        let mut base_nodes = baseline.org.nodes.clone();
        base_nodes.sort();
        match self.variant {
            VariantId::SpecOn => {
                if nodes != base_nodes || self.org.coding_node != baseline.org.coding_node {
                    return Err(VariantError::Org("spec-on must equal the B0 org".into()));
                }
            }
            VariantId::SpecOff => {
                let worker = &self.org.coding_node;
                let mut want = vec![ROOT_NODE.to_string(), worker.clone()];
                want.sort();
                if worker == ROOT_NODE || nodes != want {
                    return Err(VariantError::Org(
                        "spec-off must be the root cos plus exactly one non-root work node".into(),
                    ));
                }
            }
        }
        Ok(())
    }
}
