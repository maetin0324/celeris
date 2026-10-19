//! `context.campaign`: variant 付き run だけが運ぶ、run 開始時に dispatcher が照合した固定値
//! （propagation §8.5）。worker は読むだけで、判定の期待値は持たない。plain な run では `None`。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::decision_log::CampaignIdentity;

/// run が実際に使う harness・source・model（`runs.selected_*` と同じ値）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CampaignSelected {
    pub harness_id: String,
    pub source_id: String,
    pub model_id: String,
}

/// 照合済みの run pin。run 証拠（`runs/<run_id>/campaign-pin.json`）と task の保存 pin
/// （`<artifacts_dir>/campaign-pin.json`）に同じ形で残り、continuation・再開は保存 pin と全欄一致が条件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CampaignRunPin {
    /// 解決した `CampaignIdentity`（variant・variant_org を含む）。
    pub identity: CampaignIdentity,
    /// 全 node の実効 profile digest（`task_core::variant_wire::org_effective_digest`）。
    pub org_nodes_sha256: String,
    pub selected: CampaignSelected,
    pub daemon_instance_id: String,
    pub config_sha256: String,
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

impl CampaignRunPin {
    /// 形の検査（空欄・hash 形式・identity）。variant 付き identity だけを pin にできる。
    pub fn validate(&self) -> Result<(), String> {
        self.identity.validate()?;
        if self.identity.variant.is_none() || self.identity.variant_org.is_none() {
            return Err("campaign pin identity must carry variant and variant_org".into());
        }
        for (name, v) in [
            ("org_nodes_sha256", &self.org_nodes_sha256),
            ("config_sha256", &self.config_sha256),
        ] {
            if !is_sha256_hex(v) {
                return Err(format!("campaign.{name} must be 64 lowercase hex chars"));
            }
        }
        for (name, v) in [
            ("selected.harness_id", &self.selected.harness_id),
            ("selected.source_id", &self.selected.source_id),
            ("selected.model_id", &self.selected.model_id),
            ("daemon_instance_id", &self.daemon_instance_id),
        ] {
            if v.trim().is_empty() {
                return Err(format!("campaign.{name} must not be empty"));
            }
        }
        Ok(())
    }
}
