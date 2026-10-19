//! campaign 側の集計。error/timeout/missing を分母から除かず、hash・split の違う結果を混ぜない。
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::manifest::{CampaignManifest, ConfigId, Split};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Passed,
    Failed,
    Error,
    Timeout,
    Cancelled,
    /// 前提 problem が成立せず実行しなかった（失敗扱いで分母に残る）。
    Blocked,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrialResult {
    pub campaign_sha256: String,
    pub split: Split,
    pub config: ConfigId,
    pub problem_id: String,
    pub trial_no: u32,
    /// 独立反復の番号（1 始まり）。trial_no（内部再試行）とは別 ID。
    #[serde(default = "one")]
    pub rep: u32,
    pub outcome: Outcome,
}

fn one() -> u32 {
    1
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct Cell {
    /// 分母 = 計画された試行総数（manifest が宣言した問題数 × repeats）。実行されなかった問題も失敗側に数える。
    pub n: u64,
    pub passed: u64,
    pub failed: u64,
    pub error: u64,
    pub timeout: u64,
    pub cancelled: u64,
    pub blocked: u64,
    pub missing: u64,
}

/// 各 (問題, rep) の最初の試行（trial_no 最小）で判定する。manifest の campaign_sha256 / split が違う結果は拒否する。
pub fn aggregate(
    manifest: &CampaignManifest,
    results: &[TrialResult],
) -> Result<BTreeMap<ConfigId, Cell>, String> {
    let hash = manifest.compute_sha256()?;
    let declared: BTreeSet<&str> = manifest.problems.iter().map(String::as_str).collect();
    let mut first: BTreeMap<(ConfigId, &str, u32), &TrialResult> = BTreeMap::new();
    for r in results {
        if r.campaign_sha256 != hash {
            return Err(format!(
                "trial for {} belongs to a different campaign hash",
                r.problem_id
            ));
        }
        if r.split != manifest.split {
            return Err(format!(
                "trial for {} has split {:?}, expected {:?}",
                r.problem_id, r.split, manifest.split
            ));
        }
        if !declared.contains(r.problem_id.as_str()) {
            return Err(format!("trial for undeclared problem {}", r.problem_id));
        }
        if !manifest.is_active(r.config) {
            return Err(format!(
                "trial for {:?} but that arm is not in the effective plan",
                r.config
            ));
        }
        if r.rep == 0 || r.rep > manifest.repeats {
            return Err(format!(
                "trial for {} has rep {}, expected 1..={}",
                r.problem_id, r.rep, manifest.repeats
            ));
        }
        let slot = first
            .entry((r.config, r.problem_id.as_str(), r.rep))
            .or_insert(r);
        if r.trial_no < slot.trial_no {
            *slot = r;
        }
    }
    let mut out = BTreeMap::new();
    for cid in manifest.active_arms() {
        // 分母は有効 arm の計画試行だけ（未使用 arm・計画外の問題を missing として数えない）。
        let planned: Vec<&str> = declared
            .iter()
            .copied()
            .filter(|p| manifest.planned_configs(p).contains(&cid))
            .collect();
        let mut cell = Cell {
            n: (planned.len() as u64) * u64::from(manifest.repeats),
            ..Cell::default()
        };
        for (id, rep) in planned
            .iter()
            .flat_map(|id| (1..=manifest.repeats).map(move |k| (id, k)))
        {
            match first.get(&(cid, *id, rep)).map(|r| r.outcome) {
                Some(Outcome::Passed) => cell.passed += 1,
                Some(Outcome::Failed) => cell.failed += 1,
                Some(Outcome::Error) => cell.error += 1,
                Some(Outcome::Timeout) => cell.timeout += 1,
                Some(Outcome::Cancelled) => cell.cancelled += 1,
                Some(Outcome::Blocked) => cell.blocked += 1,
                None => cell.missing += 1,
            }
        }
        out.insert(cid, cell);
    }
    Ok(out)
}

/// routing-eval の根拠表集計へ渡す trial を、その manifest の dev 問題のものだけに絞る。
/// routing-eval の `build` は問題の所属を見ないので、別 manifest・held-out の trial が同じ cell に混ざらないよう呼び出し側で絞る。
pub fn dev_trials<'a>(
    manifest: &routing_eval::manifest::Manifest,
    trials: &'a [routing_eval::aggregate::Trial],
) -> Vec<&'a routing_eval::aggregate::Trial> {
    let dev: BTreeSet<&str> = manifest
        .problems
        .iter()
        .filter(|p| p.split == "dev")
        .map(|p| p.id.as_str())
        .collect();
    trials
        .iter()
        .filter(|t| dev.contains(t.problem_id.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::fixtures::*;

    fn res(m: &CampaignManifest, cfg: ConfigId, p: &str, n: u32, o: Outcome) -> TrialResult {
        TrialResult {
            campaign_sha256: m.compute_sha256().unwrap(),
            split: m.split,
            config: cfg,
            problem_id: p.into(),
            trial_no: n,
            rep: 1,
            outcome: o,
        }
    }

    #[test]
    fn errors_timeouts_and_missing_stay_in_denominator() {
        let i = input(4, "dev");
        let m = campaign(&i, Split::Dev, 4);
        let rs = vec![
            res(&m, ConfigId::A, "p0", 1, Outcome::Passed),
            res(&m, ConfigId::A, "p1", 1, Outcome::Error),
            res(&m, ConfigId::A, "p2", 1, Outcome::Timeout),
            // p3 missing
        ];
        let cells = aggregate(&m, &rs).unwrap();
        let a = &cells[&ConfigId::A];
        assert_eq!(
            (a.n, a.passed, a.error, a.timeout, a.missing),
            (4, 1, 1, 1, 1)
        );
        assert_eq!(cells[&ConfigId::B].missing, 4);
    }

    #[test]
    fn first_trial_decides() {
        let i = input(4, "dev");
        let m = campaign(&i, Split::Dev, 4);
        let rs = vec![
            res(&m, ConfigId::A, "p0", 2, Outcome::Passed),
            res(&m, ConfigId::A, "p0", 1, Outcome::Failed),
        ];
        assert_eq!(aggregate(&m, &rs).unwrap()[&ConfigId::A].failed, 1);
    }

    #[test]
    fn other_hash_split_or_problem_is_rejected() {
        let i = input(4, "dev");
        let m = campaign(&i, Split::Dev, 4);
        let mut r = res(&m, ConfigId::A, "p0", 1, Outcome::Passed);
        r.campaign_sha256 = "other".into();
        assert!(aggregate(&m, &[r]).is_err());
        let mut r = res(&m, ConfigId::A, "p0", 1, Outcome::Passed);
        r.split = Split::Final;
        assert!(aggregate(&m, &[r]).is_err());
        assert!(aggregate(&m, &[res(&m, ConfigId::A, "zz", 1, Outcome::Passed)]).is_err());
    }
}
