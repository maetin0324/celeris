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
    aggregate::Outcome,
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
    pub blocked: u64,
    pub missing: u64,
    /// 実行した全試行（再試行を含む）の token。欠測の試行は sum に入れず `*_missing_trials` に数える。
    pub tokens_sum: u64,
    pub tokens_missing_trials: u64,
    /// tokens の内訳の和（`token_usage` を持つ試行だけ）。cache は欠測の試行を足さず、件数を別に数える。
    pub input_tokens_sum: u64,
    pub output_tokens_sum: u64,
    pub cache_read_tokens_sum: u64,
    pub cache_creation_tokens_sum: u64,
    /// `token_usage` が無い（内訳ごと欠測の）試行数。
    pub token_usage_missing_trials: u64,
    /// cache_read / cache_creation が欠測の試行数（内訳はあるが cache が取れなかった）。
    pub cache_missing_trials: u64,
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

enum Dep {
    Met,
    Failed(String),
    Waiting,
}

/// `recs`（それ以前の記録）から problem の前提の成否を再計算する。runner の判定コードは使わない。
fn dep_state(m: &CampaignManifest, recs: &[TrialRecord], problem: &str, rep: u32) -> Dep {
    let mut waiting = false;
    for q in m.dependencies.get(problem).into_iter().flatten() {
        for c in m.planned_configs(q) {
            let mut best: Option<&TrialRecord> = None;
            for r in recs
                .iter()
                .filter(|r| r.config == c && r.problem_id == *q && r.rep == rep)
            {
                if best.is_none_or(|b| r.trial_no < b.trial_no) {
                    best = Some(r);
                }
            }
            match best {
                Some(r) if r.outcome == Outcome::Passed => {}
                Some(_) => return Dep::Failed(q.clone()),
                None => waiting = true,
            }
        }
    }
    if waiting { Dep::Waiting } else { Dep::Met }
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
        // Celeris 経路（B/C が task-api 経由）で走った campaign は、実行した B/C 試行すべてに routing がある。
        let celeris_run = matches!(meta["executor"].as_str(), Some("celeris" | "arm-dispatch"));
        let exp = expected(m);
        let mut seen: BTreeSet<(ConfigId, String, u32, u32)> = BTreeSet::new();
        let mut ws_dirs: BTreeSet<String> = BTreeSet::new();
        let mut cum_tokens = 0u64;
        let mut cum_wall = 0u64;
        let mut arm_use: BTreeMap<ConfigId, (u64, u64)> = BTreeMap::new();
        let cfg_by_id: BTreeMap<ConfigId, &crate::manifest::Config> =
            m.configs.iter().map(|c| (c.id, c)).collect();
        for (idx, r) in recs.iter().enumerate() {
            let earlier = &recs[..idx];
            let tag = format!(
                "{:?}/{}/rep{}#{}",
                r.config, r.problem_id, r.rep, r.trial_no
            );
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
            if r.rep == 0 || r.rep > m.repeats {
                v.push(format!("{tag}: rep outside 1..={}", m.repeats));
            }
            if !seen.insert((r.config, r.problem_id.clone(), r.rep, r.trial_no)) {
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
            // 実効 routing: 記録した値は manifest の arm 設定から導いた値と一致する（A は持たない）。
            if r.config == ConfigId::A {
                if r.routing.is_some() {
                    v.push(format!("{tag}: arm A must not carry a Celeris routing"));
                }
            } else if let Some(c) = cfg_by_id.get(&r.config) {
                let want = crate::celeris_exec::routing_override(c);
                match (&r.routing, &want) {
                    (Some(got), Ok(w)) if got != w => v.push(format!(
                        "{tag}: recorded routing {got} differs from the manifest arm setting {w}"
                    )),
                    (Some(_), Err(e)) => {
                        v.push(format!(
                            "{tag}: routing recorded but manifest is invalid: {e}"
                        ));
                    }
                    (None, Ok(_))
                        if celeris_run
                            && r.ran()
                            && r.reason.as_deref() != Some(REASON_EXECUTOR_ERROR) =>
                    {
                        v.push(format!("{tag}: ran without a recorded routing"));
                    }
                    _ => {}
                }
            }
            // 予算に数えた tokens は内訳を manifest の基準で数えた値と一致する。
            if let Some(u) = &r.token_usage
                && r.tokens != u.count(m.budget.token_basis)
            {
                v.push(format!(
                    "{tag}: tokens {:?} differ from token_usage counted by {:?}",
                    r.tokens, m.budget.token_basis
                ));
            }
            if r.token_usage.is_none() && r.tokens.is_some() {
                v.push(format!("{tag}: tokens recorded without token_usage"));
            }
            if r.reason.as_deref() == Some(REASON_NO_ARTIFACT)
                && (r.config != ConfigId::A
                    || r.outcome != Outcome::Failed
                    || r.workspace_sha256_final != r.workspace_sha256_initial)
            {
                v.push(format!(
                    "{tag}: no_artifact must be an A trial, outcome failed, with an unchanged workspace"
                ));
            }
            if r.config == ConfigId::A
                && r.outcome == Outcome::Passed
                && r.workspace_sha256_final == r.workspace_sha256_initial
            {
                v.push(format!(
                    "{tag}: A passed without any artifact (workspace unchanged)"
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
            if r.outcome == Outcome::Blocked && r.reason.as_deref() != Some(REASON_BLOCKED) {
                v.push(format!(
                    "{tag}: blocked outcome without the dependency_blocked reason"
                ));
            }
            if r.reason.as_deref() == Some(REASON_BLOCKED) {
                // blocked の前提不成立を、それ以前の記録から再計算して確かめる。
                if r.outcome != Outcome::Blocked
                    || r.tokens.is_some()
                    || r.cost_micro_usd.is_some()
                    || r.wall_secs != 0
                    || !r.workspace.is_empty()
                {
                    v.push(format!(
                        "{tag}: blocked trial must be outcome blocked with no usage or workspace"
                    ));
                }
                if !matches!(dep_state(m, earlier, &r.problem_id, r.rep), Dep::Failed(_)) {
                    v.push(format!(
                        "{tag}: blocked although no prerequisite had failed"
                    ));
                }
            } else if r.ran() {
                match dep_state(m, earlier, &r.problem_id, r.rep) {
                    Dep::Met => {}
                    Dep::Failed(q) => v.push(format!(
                        "{tag}: executed although prerequisite {q} did not pass (should be blocked)"
                    )),
                    Dep::Waiting => v.push(format!(
                        "{tag}: executed before its prerequisites had finished"
                    )),
                }
                if r.workspace.is_empty() || !ws_dirs.insert(r.workspace.clone()) {
                    v.push(format!(
                        "{tag}: workspace is missing or shared with another trial"
                    ));
                }
                if r.workspace_sha256_initial != snap["workspace_seed_sha256"]
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
                let arm = arm_use.get(&r.config).copied().unwrap_or_default();
                let case = m.budget.per_case;
                let over_case = r.tokens.is_some_and(|t| t > case.max_tokens)
                    || r.wall_secs > case.max_wall_secs;
                if over_case
                    && r.outcome != crate::aggregate::Outcome::Timeout
                    && r.reason.as_deref() != Some(REASON_CASE_BUDGET)
                {
                    v.push(format!(
                        "{tag}: exceeded the per-case budget but was not cancelled as case_budget"
                    ));
                }
                if matches!(
                    r.reason.as_deref(),
                    Some(REASON_CASE_BUDGET | REASON_ARM_BUDGET)
                ) && r.outcome != crate::aggregate::Outcome::Cancelled
                {
                    v.push(format!("{tag}: budget reason requires outcome cancelled"));
                }
                if r.reason.as_deref() == Some(REASON_CASE_BUDGET) && !over_case {
                    v.push(format!(
                        "{tag}: case_budget although within the per-case cap"
                    ));
                }
                if arm.0 >= m.budget.per_arm.max_tokens || arm.1 >= m.budget.per_arm.max_wall_secs {
                    v.push(format!(
                        "{tag}: executed after its arm budget was already exhausted"
                    ));
                }
                let e = arm_use.entry(r.config).or_default();
                e.0 += r.charged_tokens(&m.budget);
                e.1 += r.wall_secs;
                cum_tokens += r.charged_tokens(&m.budget);
                cum_wall += r.wall_secs;
            } else if r.outcome != crate::aggregate::Outcome::Cancelled || r.tokens.is_some() {
                v.push(format!(
                    "{tag}: budget-stopped trial must be cancelled with no usage"
                ));
            } else if r.reason.as_deref() == Some(REASON_ARM_BUDGET) {
                let (t, w) = arm_use.get(&r.config).copied().unwrap_or_default();
                if t < m.budget.per_arm.max_tokens && w < m.budget.per_arm.max_wall_secs {
                    v.push(format!(
                        "{tag}: cancelled for arm budget although the arm budget remained"
                    ));
                }
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
                    t.blocked,
                    t.missing,
                ];
                let key = [
                    "n",
                    "passed",
                    "failed",
                    "error",
                    "timeout",
                    "cancelled",
                    "blocked",
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
    let mut first: BTreeMap<(ConfigId, &str, u32), &TrialRecord> = BTreeMap::new();
    for r in recs {
        let e = first
            .entry((r.config, r.problem_id.as_str(), r.rep))
            .or_insert(r);
        if r.trial_no < e.trial_no {
            *e = r;
        }
    }
    for (c, probs) in &exp {
        let t = totals.entry(*c).or_default();
        // 分母 = 計画された試行総数（問題数 × repeats）。
        t.n = (probs.len() as u64) * u64::from(m.repeats);
        for (p, rep) in probs
            .iter()
            .flat_map(|p| (1..=m.repeats).map(move |k| (p, k)))
        {
            match first.get(&(*c, p.as_str(), rep)).map(|r| r.outcome) {
                None => t.missing += 1,
                Some(o) => match o {
                    crate::aggregate::Outcome::Passed => t.passed += 1,
                    crate::aggregate::Outcome::Failed => t.failed += 1,
                    crate::aggregate::Outcome::Error => t.error += 1,
                    crate::aggregate::Outcome::Timeout => t.timeout += 1,
                    crate::aggregate::Outcome::Cancelled => t.cancelled += 1,
                    crate::aggregate::Outcome::Blocked => t.blocked += 1,
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
            match r.token_usage {
                Some(u) => {
                    t.input_tokens_sum += u.input;
                    t.output_tokens_sum += u.output;
                    t.cache_read_tokens_sum += u.cache_read.unwrap_or(0);
                    t.cache_creation_tokens_sum += u.cache_creation.unwrap_or(0);
                    t.cache_missing_trials +=
                        u64::from(u.cache_read.is_none() || u.cache_creation.is_none());
                }
                None => t.token_usage_missing_trials += 1,
            }
            match r.cost_micro_usd {
                Some(x) => t.cost_micro_usd_sum += x,
                None => t.cost_missing_trials += 1,
            }
        }
    }
    let pass = |c: ConfigId, p: &str, rep: u32| {
        first
            .get(&(c, p, rep))
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
        for (p, rep) in exp[&a]
            .intersection(&exp[&b])
            .flat_map(|p| (1..=m.repeats).map(move |k| (p, k)))
        {
            pr.problems += 1;
            match (pass(a, p, rep), pass(b, p, rep)) {
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
