//! 独立 checker: out ディレクトリの生記録（trials.jsonl）だけから合否・集計を再計算する。
//! runner / aggregate の集計コードは使わない（記録の型と manifest の hash 計算だけを共有する）。
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use serde::Serialize;
use serde_json::Value;

use crate::{
    manifest::{CampaignManifest, ConfigId},
    record::*,
};

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Totals {
    pub n: u64,
    pub passed: u64,
    pub failed: u64,
    pub error: u64,
    pub timeout: u64,
    pub cancelled: u64,
    pub missing: u64,
    /// 実行した全試行（再試行を含む）の token。欠測の試行は sum に入れず `*_missing_trials` に数える。
    pub tokens_sum: u64,
    pub tokens_missing_trials: u64,
    pub cost_micro_usd_sum: u64,
    pub cost_missing_trials: u64,
    pub ran_trials: u64,
    pub wall_secs_sum: u64,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Pair {
    pub name: String,
    pub first: ConfigId,
    pub second: ConfigId,
    pub problems: u64,
    pub both_pass: u64,
    pub only_first: u64,
    pub only_second: u64,
    pub neither: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Recomputed {
    pub totals: BTreeMap<ConfigId, Totals>,
    pub pairs: Vec<Pair>,
}

#[derive(Debug, Serialize)]
pub struct CheckReport {
    pub ok: bool,
    pub violations: Vec<String>,
    pub records: usize,
    pub recomputed: Option<Recomputed>,
}

fn read_json(path: &Path) -> Result<Value, String> {
    let b = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", path.display()))
}

/// 期待する (config → 問題集合)。A/B は全問題、C は事前登録 subset。
fn expected(m: &CampaignManifest) -> BTreeMap<ConfigId, BTreeSet<String>> {
    let all: BTreeSet<String> = m.problems.iter().cloned().collect();
    let sub: BTreeSet<String> = m.secondary.subset.iter().cloned().collect();
    BTreeMap::from([
        (ConfigId::A, all.clone()),
        (ConfigId::B, all),
        (ConfigId::C, sub),
    ])
}

pub fn check(m: &CampaignManifest, out: &Path) -> CheckReport {
    let mut v: Vec<String> = Vec::new();
    let mut recs: Vec<TrialRecord> = Vec::new();
    let result = (|| -> Result<(), String> {
        m.validate()?;
        let hash = m.compute_sha256()?;
        let meta = read_json(&out.join(META_FILE))?;
        if meta["campaign_sha256"] != hash {
            v.push("run-meta campaign_sha256 differs from the manifest hash".into());
        }
        let saved: CampaignManifest = serde_json::from_value(read_json(&out.join(CAMPAIGN_FILE))?)
            .map_err(|e| e.to_string())?;
        if saved.compute_sha256()? != hash {
            v.push("saved campaign.json differs from the manifest under check".into());
        }
        let input_sha = meta["input_sha256"].as_str().unwrap_or("").to_string();
        if input_sha != m.input.sha256 {
            v.push("run-meta input_sha256 differs from the manifest's input pin".into());
        }
        let snap = read_json(&out.join(SNAPSHOT_FILE))?;
        let text = fs::read_to_string(out.join(TRIALS_FILE)).map_err(|e| e.to_string())?;
        if !text.is_empty() && !text.ends_with('\n') {
            v.push("trials.jsonl ends with an incomplete line".into());
        }
        for (i, line) in text.lines().enumerate() {
            match serde_json::from_str::<TrialRecord>(line) {
                Ok(r) => recs.push(r),
                Err(e) => v.push(format!("trials.jsonl line {}: unparsable: {e}", i + 1)),
            }
        }
        let exp = expected(m);
        let mut seen: BTreeSet<(ConfigId, String, u32)> = BTreeSet::new();
        let mut ws_dirs: BTreeSet<String> = BTreeSet::new();
        let mut cum_tokens = 0u64;
        let mut cum_wall = 0u64;
        let cfg_by_id: BTreeMap<ConfigId, &crate::manifest::Config> =
            m.configs.iter().map(|c| (c.id, c)).collect();
        for r in &recs {
            let tag = format!("{:?}/{}#{}", r.config, r.problem_id, r.trial_no);
            if r.schema != TRIAL_SCHEMA {
                v.push(format!("{tag}: unknown schema {}", r.schema));
            }
            if r.campaign_sha256 != hash {
                v.push(format!("{tag}: campaign hash mismatch"));
            }
            if r.input_sha256 != m.input.sha256 {
                v.push(format!("{tag}: input manifest hash mismatch"));
            }
            if r.split != m.split {
                v.push(format!(
                    "{tag}: split {:?} mixed into a {:?} campaign",
                    r.split, m.split
                ));
            }
            match exp.get(&r.config) {
                Some(s) if s.contains(&r.problem_id) => {}
                _ => v.push(format!("{tag}: not an expected trial for this config")),
            }
            if r.trial_no == 0 || r.trial_no > m.termination.max_attempts_per_trial {
                v.push(format!(
                    "{tag}: trial_no outside 1..={}",
                    m.termination.max_attempts_per_trial
                ));
            }
            if !seen.insert((r.config, r.problem_id.clone(), r.trial_no)) {
                v.push(format!("{tag}: duplicate record"));
            }
            if r.passed != (r.outcome == crate::aggregate::Outcome::Passed) {
                v.push(format!(
                    "{tag}: passed flag contradicts outcome {:?}",
                    r.outcome
                ));
            }
            if let Some(c) = cfg_by_id.get(&r.config)
                && (r.harness != c.harness
                    || r.harness_version != c.harness_version
                    || r.model != c.model
                    || r.model_version != c.model_version
                    || r.policy_version != c.policy_version)
            {
                v.push(format!(
                    "{tag}: harness/model/policy version differs from the manifest"
                ));
            }
            let timeout = m.termination.trial_timeout_secs;
            let is_to = r.outcome == crate::aggregate::Outcome::Timeout;
            if r.wall_secs > timeout {
                v.push(format!(
                    "{tag}: wall_secs {} exceeds trial timeout {timeout}",
                    r.wall_secs
                ));
            }
            if is_to && r.wall_secs < timeout && r.reason.as_deref() != Some("runner_timeout") {
                v.push(format!(
                    "{tag}: timeout recorded but only {}s elapsed",
                    r.wall_secs
                ));
            }
            if r.ran() {
                if r.workspace.is_empty() || !ws_dirs.insert(r.workspace.clone()) {
                    v.push(format!(
                        "{tag}: workspace is missing or shared with another trial"
                    ));
                }
                if r.workspace_sha256_initial != snap["repo_sha256"]
                    || r.kb_sha256_initial != snap["kb_sha256"]
                {
                    v.push(format!(
                        "{tag}: initial workspace/KB does not match the saved snapshot"
                    ));
                }
                if m.termination.stop_on_budget
                    && (cum_tokens >= m.budget.max_tokens || cum_wall >= m.budget.max_wall_secs)
                {
                    v.push(format!(
                        "{tag}: executed after the budget was already exhausted"
                    ));
                }
                cum_tokens += r.tokens.unwrap_or(0);
                cum_wall += r.wall_secs;
            } else if r.outcome != crate::aggregate::Outcome::Cancelled || r.tokens.is_some() {
                v.push(format!(
                    "{tag}: budget-stopped trial must be cancelled with no usage"
                ));
            } else if !(cum_tokens >= m.budget.max_tokens || cum_wall >= m.budget.max_wall_secs) {
                v.push(format!(
                    "{tag}: cancelled for budget although budget remained"
                ));
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        v.push(e);
        return CheckReport {
            ok: false,
            violations: v,
            records: recs.len(),
            recomputed: None,
        };
    }
    let rc = recompute(m, &recs);
    // runner の summary.json との突き合わせ（記録の欠落・数値の食い違いを検出する）。
    match read_json(&out.join(SUMMARY_FILE)) {
        Ok(s) => {
            if s["records"] != recs.len() {
                v.push(format!(
                    "summary.json says {} records, trials.jsonl has {}",
                    s["records"],
                    recs.len()
                ));
            }
            for (c, t) in &rc.totals {
                let cell = &s["cells"][format!("{c:?}")];
                let got = [
                    t.n,
                    t.passed,
                    t.failed,
                    t.error,
                    t.timeout,
                    t.cancelled,
                    t.missing,
                ];
                let key = [
                    "n",
                    "passed",
                    "failed",
                    "error",
                    "timeout",
                    "cancelled",
                    "missing",
                ];
                for (k, g) in key.iter().zip(got) {
                    if cell[*k] != g {
                        v.push(format!(
                            "summary.json {c:?}.{k} = {} but recomputed {g}",
                            cell[*k]
                        ));
                    }
                }
            }
        }
        Err(e) => v.push(format!("summary: {e}")),
    }
    CheckReport {
        ok: v.is_empty(),
        violations: v,
        records: recs.len(),
        recomputed: Some(rc),
    }
}

/// 生記録から再計算する。各 (config, 問題) は trial_no 最小の記録で判定し、無ければ missing。
/// 分母は期待された問題数で、failed/error/timeout/cancelled/missing のどれも除かない。
pub fn recompute(m: &CampaignManifest, recs: &[TrialRecord]) -> Recomputed {
    let exp = expected(m);
    let mut totals: BTreeMap<ConfigId, Totals> = BTreeMap::new();
    let mut first: BTreeMap<(ConfigId, &str), &TrialRecord> = BTreeMap::new();
    for r in recs {
        let e = first.entry((r.config, r.problem_id.as_str())).or_insert(r);
        if r.trial_no < e.trial_no {
            *e = r;
        }
    }
    for (c, probs) in &exp {
        let t = totals.entry(*c).or_default();
        t.n = probs.len() as u64;
        for p in probs {
            match first.get(&(*c, p.as_str())).map(|r| r.outcome) {
                None => t.missing += 1,
                Some(o) => match o {
                    crate::aggregate::Outcome::Passed => t.passed += 1,
                    crate::aggregate::Outcome::Failed => t.failed += 1,
                    crate::aggregate::Outcome::Error => t.error += 1,
                    crate::aggregate::Outcome::Timeout => t.timeout += 1,
                    crate::aggregate::Outcome::Cancelled => t.cancelled += 1,
                },
            }
        }
    }
    for r in recs.iter().filter(|r| r.ran()) {
        if let Some(t) = totals.get_mut(&r.config) {
            t.ran_trials += 1;
            t.wall_secs_sum += r.wall_secs;
            match r.tokens {
                Some(x) => t.tokens_sum += x,
                None => t.tokens_missing_trials += 1,
            }
            match r.cost_micro_usd {
                Some(x) => t.cost_micro_usd_sum += x,
                None => t.cost_missing_trials += 1,
            }
        }
    }
    let pass = |c: ConfigId, p: &str| {
        first
            .get(&(c, p))
            .is_some_and(|r| r.outcome == crate::aggregate::Outcome::Passed)
    };
    let mut pairs = Vec::new();
    for (name, a, b) in [
        ("primary", m.primary.0, m.primary.1),
        ("secondary", m.secondary.pair.0, m.secondary.pair.1),
    ] {
        let mut pr = Pair {
            name: name.into(),
            first: a,
            second: b,
            problems: 0,
            both_pass: 0,
            only_first: 0,
            only_second: 0,
            neither: 0,
        };
        for p in exp[&a].intersection(&exp[&b]) {
            pr.problems += 1;
            match (pass(a, p), pass(b, p)) {
                (true, true) => pr.both_pass += 1,
                (true, false) => pr.only_first += 1,
                (false, true) => pr.only_second += 1,
                (false, false) => pr.neither += 1,
            }
        }
        pairs.push(pr);
    }
    Recomputed { totals, pairs }
}
