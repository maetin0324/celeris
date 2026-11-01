//! ops-r1 の condition 宣言（manifest の `contract.conditions` 欄）。A=直接 Claude Code、B=CoS 受付。
//! 形と理由は `docs/ops/ops-r1-pipeline.md` §3・§5 と ADR `agent-docs/adr/2026-10-30-ops-r1-pipeline-manifest.md`。
//!
//! - 欄が無い manifest（旧系列）は検査を 1 つも足さない。直列化・sha256 も変わらない。
//! - `contract` の下に置くので、condition は `contract.revision = main-4-ops-r1` の manifest だけが持てる。
//! - `conditions` のある manifest は task API に投入する道具（run・dry-run・preflight・cap-probe・config-check・env-check）が
//!   拒否する。B は CoS が受け付けた試行で、campaign が `POST /tasks` で投入するものではない。
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::manifest::{
    CONTRACT_REVISION_OPS_R1, CONTRACT_REVISION_OPS_R2, CampaignManifest, ConfigId,
};

/// 試行の受付経路。A は人が Claude Code を直接起動し、B は案件の CoS thread が受け付ける。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intake {
    DirectClaudeCode,
    CosThread,
}

/// arm 1 つ分の condition。`trial_ids[rep - 1]` が `rep` 番目の試行の trial_id。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConditionDecl {
    pub condition_id: String,
    pub intake: Intake,
    pub trial_ids: Vec<String>,
    /// B の CoS（案件 CoS・統括 CoS）が動く model。`ops-r1-assemble` が CoS run の実測と照合する。A は書かない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cos_model: Option<String>,
}

/// 投入系の道具が ops-r1 の manifest を拒否するときの文。
pub const OPS_R1_SUBMIT_REJECTED: &str = "the ops-r1 manifest (with conditions) is only for ops-r1-assemble and the grading/aggregation stages; B is a CoS-intake trial, not a POST /tasks submission";

static NO_CONDITIONS: BTreeMap<ConfigId, ConditionDecl> = BTreeMap::new();

impl CampaignManifest {
    /// `contract.conditions`（無ければ空）。
    pub fn conditions(&self) -> &BTreeMap<ConfigId, ConditionDecl> {
        self.contract
            .as_ref()
            .map_or(&NO_CONDITIONS, |c| &c.conditions)
    }

    /// `conditions` を持つ ops-r1 の manifest か。
    pub fn is_ops_r1(&self) -> bool {
        !self.conditions().is_empty()
    }

    /// B が CoS 受付（campaign は B の task に routing を固定しない）。
    pub fn b_is_cos_thread(&self) -> bool {
        self.conditions()
            .get(&ConfigId::B)
            .is_some_and(|c| c.intake == Intake::CosThread)
    }

    /// `main-4-ops-r2`（K2 pilot）の manifest か。preflight だけがこれを受ける（`reject_ops_r1_submission`）。
    pub fn is_ops_r2(&self) -> bool {
        self.is_ops_r1()
            && self.contract.as_ref().map(|c| c.revision.as_str()) == Some(CONTRACT_REVISION_OPS_R2)
    }

    /// 投入系の subcommand（`cmd`）の入口で呼ぶ。ops-r1 の manifest なら拒否する。
    /// ops-r2 の manifest は `preflight` だけを通す（開始前チェック。有料実行・task 作成はしない）。他の投入系は ops-r2 でも拒否する。
    pub fn reject_ops_r1_submission(&self, cmd: &str) -> Result<(), String> {
        if self.is_ops_r2() && cmd == "preflight" {
            return Ok(());
        }
        if self.is_ops_r1() {
            return Err(format!("{cmd}: {OPS_R1_SUBMIT_REJECTED}"));
        }
        Ok(())
    }

    /// condition_id から arm と宣言を引く。
    pub fn condition_arm(&self, condition_id: &str) -> Result<(ConfigId, &ConditionDecl), String> {
        self.conditions()
            .iter()
            .find(|(_, c)| c.condition_id == condition_id)
            .map(|(id, c)| (*id, c))
            .ok_or_else(|| {
                let known: Vec<&str> = self
                    .conditions()
                    .values()
                    .map(|c| c.condition_id.as_str())
                    .collect();
                format!("condition {condition_id:?} is not declared in the manifest (declared: {known:?})")
            })
    }

    /// `conditions` の形の検査。欄が無ければ何もしない（旧系列の manifest に検査を足さない）。
    pub(crate) fn validate_conditions(&self, active: &[ConfigId]) -> Result<(), String> {
        let conditions = self.conditions();
        if conditions.is_empty() {
            return Ok(());
        }
        if !matches!(
            self.contract.as_ref().map(|c| c.revision.as_str()),
            Some(CONTRACT_REVISION_OPS_R1 | CONTRACT_REVISION_OPS_R2)
        ) {
            return Err(format!(
                "contract.conditions: contract.revision must be {CONTRACT_REVISION_OPS_R1} or {CONTRACT_REVISION_OPS_R2}"
            ));
        }
        let keys: Vec<ConfigId> = conditions.keys().copied().collect();
        if keys != active {
            return Err(format!(
                "contract.conditions must cover exactly the active arms {active:?} (got {keys:?})"
            ));
        }
        let mut ids = BTreeSet::new();
        let mut trials = BTreeSet::new();
        for (arm, c) in conditions {
            let want = match arm {
                ConfigId::A => Intake::DirectClaudeCode,
                ConfigId::B => Intake::CosThread,
                ConfigId::C => return Err("contract.conditions: ops-r1 has no arm C".into()),
            };
            if c.intake != want {
                return Err(format!(
                    "contract.conditions.{arm:?}.intake must be {}",
                    serde_json::to_value(want)
                        .ok()
                        .and_then(|v| v.as_str().map(String::from))
                        .unwrap_or_default()
                ));
            }
            if c.condition_id.trim().is_empty() || !ids.insert(c.condition_id.as_str()) {
                return Err(format!(
                    "contract.conditions.{arm:?}.condition_id must be non-empty and unique"
                ));
            }
            if *arm != ConfigId::B && c.cos_model.is_some() {
                return Err(format!(
                    "contract.conditions.{arm:?}.cos_model is only for B"
                ));
            }
            if c.cos_model.as_deref().is_some_and(|m| m.trim().is_empty()) {
                return Err("contract.conditions.B.cos_model must be non-empty".into());
            }
            if c.trial_ids.len() != self.repeats as usize {
                return Err(format!(
                    "contract.conditions.{arm:?}.trial_ids must have repeats ({}) entries",
                    self.repeats
                ));
            }
            for t in &c.trial_ids {
                if t.trim().is_empty() || !trials.insert(t.as_str()) {
                    return Err(format!(
                        "contract.conditions.{arm:?}.trial_ids must be non-empty and unique across arms"
                    ));
                }
            }
        }
        Ok(())
    }
}
