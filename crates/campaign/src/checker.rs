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
    /// 記録はあるが実行されなかった cell（task 作成失敗 `executor_error`、予算打ち切りで未投入）。
    /// 1 つでもあれば `complete=false`（依存で blocked の cell は分母に残る結果で、ここには数えない）。
    #[serde(default)]
    pub unexecuted: u64,
    /// 成功（論文用）: 実行器が passed を返し、かつ有効な独立採点が final_pass の cell。分母は n のまま。
    pub final_passed: u64,
    /// 実行器は passed だが有効な採点が無い cell（未採点・結合検査で拒否）。成功にも分母除外にもしない。
    pub ungraded: u64,
    /// 実行器は passed だが独立採点が不合格の cell（成果物が誤り）。
    pub grader_failed: u64,
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
    /// 実測（全 run の記録）から集めた値。宣言ではない。executor が観測できなかった試行は
    /// `unobserved_trials` に数え、集合には入れない。
    pub observed_runs: u64,
    pub unobserved_trials: u64,
    pub observed_harnesses: BTreeSet<String>,
    pub observed_sources: BTreeSet<String>,
    pub observed_models: BTreeSet<String>,
    /// `mode/source/online_update=…`（run の判断 record に残った実効 routing）。
    pub observed_routing: BTreeSet<String>,
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
    /// 全 cell に記録があり、実行器 passed の cell すべてに有効な採点がある。false の集計は最終版にしない。
    pub complete: bool,
    pub incomplete_reasons: Vec<String>,
    pub records: usize,
    pub recomputed: Option<Recomputed>,
}

fn read_json(path: &Path) -> Result<Value, String> {
    let b = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", path.display()))
}

/// 期待する (config → 問題集合)。有効 arm だけ（`planned_configs`）。未使用 arm は含めない。
fn expected(m: &CampaignManifest) -> BTreeMap<ConfigId, BTreeSet<String>> {
    let mut out: BTreeMap<ConfigId, BTreeSet<String>> = m
        .active_arms()
        .into_iter()
        .map(|c| (c, BTreeSet::new()))
        .collect();
    for p in &m.problems {
        for c in m.planned_configs(p) {
            out.entry(c).or_default().insert(p.clone());
        }
    }
    out
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

/// 試行ごとの観測 run（非 A arm の、実際に動いた試行だけ）。
fn observed_by_trial(recs: &[TrialRecord]) -> Vec<(ConfigId, String, &TrialRecord)> {
    recs.iter()
        .filter(|r| {
            r.config != ConfigId::A && r.ran() && r.reason.as_deref() != Some(REASON_EXECUTOR_ERROR)
        })
        .map(|r| {
            (
                r.config,
                format!(
                    "{:?}/{}/rep{}#{}",
                    r.config, r.problem_id, r.rep, r.trial_no
                ),
                r,
            )
        })
        .collect()
}

/// 1 つの set の固定欄のうち run の実測と比べられるもの。宣言から観測値を補わない（None は違反）。
fn reconcile_set_fixed(
    f: &crate::manifest::SetFixed,
    arms: &[ConfigId],
    trials: &[(ConfigId, String, &TrialRecord)],
    out: &mut BTreeSet<String>,
) {
    let checks_runs = f.policy.is_some()
        || f.models.is_some()
        || f.harnesses.is_some()
        || !f.role_models.is_empty();
    if !checks_runs {
        return;
    }
    let mut run_ids: BTreeMap<&str, &str> = BTreeMap::new();
    for (arm, tag, r) in trials.iter().filter(|(a, _, _)| arms.contains(a)) {
        let _ = arm;
        if r.observed.is_empty() {
            out.insert(format!("{tag}: set fixed declared but no run was observed"));
            continue;
        }
        if !r.observed.iter().any(|o| o.role == "worker") {
            out.insert(format!("{tag}: no worker run observed"));
        }
        for o in &r.observed {
            let who = format!("{tag}: run {:?} ({:?})", o.run_id, o.role);
            if o.run_id.is_empty() {
                out.insert(format!("{who}: run id not observed"));
            } else if let Some(prev) = run_ids.insert(&o.run_id, tag)
                && prev != tag.as_str()
            {
                out.insert(format!("{who}: run id also claimed by trial {prev}"));
            }
            if o.role.is_empty() {
                out.insert(format!("{who}: role not observed"));
            }
            if let Some(models) = &f.models {
                match &o.model {
                    None => out.insert(format!("{who}: model not observed")),
                    Some(x) if !models.contains(x) => out.insert(format!(
                        "{who}: model {x:?} is outside the set's fixed models"
                    )),
                    _ => false,
                };
            }
            if let Some(hs) = &f.harnesses {
                match &o.harness {
                    None => out.insert(format!("{who}: harness not observed")),
                    Some(x) if !hs.contains(x) => out.insert(format!(
                        "{who}: harness {x:?} is outside the set's fixed harnesses"
                    )),
                    _ => false,
                };
            }
            if let Some(p) = &f.policy {
                match &o.lower_policy {
                    None => out.insert(format!("{who}: lower_policy not observed")),
                    Some(x) if x != p => out.insert(format!(
                        "{who}: lower_policy {x:?} differs from the set's fixed policy {p:?}"
                    )),
                    _ => false,
                };
            }
            if !f.role_models.is_empty() {
                match (f.role_models.get(&o.role), &o.model) {
                    (None, _) => out.insert(format!(
                        "{who}: role has no declared model in the set's role_models"
                    )),
                    (Some(_), None) => out.insert(format!("{who}: model not observed")),
                    (Some(want), Some(got)) if want != got => out.insert(format!(
                        "{who}: ran model {got:?} but role_models declares {want:?}"
                    )),
                    _ => false,
                };
            }
        }
    }
}

/// SetFixed.initial_info / success を材料・grader の実 hash と照合する（宣言が無い欄は見ない）。
/// 実 hash は snapshot.json（実際に hash した材料 tree）と封印済み lock（材料 hash・全 case の grader pin）から読み、
/// 読めない証拠は違反（宣言で補わない）。
pub fn reconcile_set_evidence(
    m: &CampaignManifest,
    snap: Option<&Value>,
    lock: Option<&crate::lock::CampaignLock>,
    v: &mut Vec<String>,
) {
    let Ok(Some(sets)) = m.selected() else { return };
    for s in sets {
        if let Some(want) = &s.fixed.initial_info {
            match (snap.and_then(|x| x["materials_sha256"].as_str()), lock) {
                (Some(got), Some(l)) if got == want && l.materials_sha256 == *want => {}
                (None, _) | (_, None) => v.push(
                    "set fixed initial_info: materials hash evidence (snapshot/lock) is missing"
                        .into(),
                ),
                (Some(got), Some(l)) => v.push(format!(
                    "set fixed initial_info {want} differs from observed materials (snapshot {got}, lock {})",
                    l.materials_sha256
                )),
            }
        }
        if let Some(want) = &s.fixed.success {
            match lock {
                None => v.push("set fixed success: grader pin evidence (lock) is missing".into()),
                Some(l) if l.cases.is_empty() => {
                    v.push("set fixed success: lock pins no grader".into());
                }
                Some(l) if l.grader_set_sha256() != *want => v.push(format!(
                    "set fixed success {want} differs from the locked grader hash {}",
                    l.grader_set_sha256()
                )),
                _ => {}
            }
        }
    }
}

/// 実験セット（宣言があるときだけ）の固定構成と、実際に動いた全 run の照合。B0 と B1/C1 は別々に検査する。
/// 設定名が同じだけでは同条件とみなさない: run が実際に使った model/harness を見る。
fn reconcile_sets(m: &CampaignManifest, recs: &[TrialRecord], v: &mut Vec<String>) {
    if m.experiment_sets.is_empty() {
        return;
    }
    let observed = |c: ConfigId| {
        recs.iter().filter(move |r| r.config == c).flat_map(|r| {
            r.observed.iter().map(move |o| {
                (
                    format!(
                        "{:?}/{}/rep{}#{}",
                        r.config, r.problem_id, r.rep, r.trial_no
                    ),
                    o,
                )
            })
        })
    };
    let pairs = m.compare_pairs();
    let mut seen = BTreeSet::new();
    // B0（A/B 比較）: B の全役割（worker・reviewer・planner・cos）が実際に Sonnet で動いた。
    if pairs.contains(&(ConfigId::A, ConfigId::B)) {
        for (tag, o) in observed(ConfigId::B) {
            let who = format!("{tag}: run {} ({})", o.run_id, o.role);
            match &o.model {
                None => seen.insert(format!("{who}: B0 model not observed")),
                Some(model) => {
                    match crate::manifest::check_b0_observed(&[(o.role.clone(), model.clone())]) {
                        Ok(()) => false,
                        Err(e) => seen.insert(format!("{who}: {e}")),
                    }
                }
            };
        }
    }
    // SetFixed 宣言（policy/models/harnesses/role_models）と実測の照合。新形式（set 宣言あり）では
    // 証拠の欠落（役割・run id・model/harness/lower_policy が None、実行した試行に worker run が無い）を違反にする。
    if let Ok(Some(sets)) = m.selected() {
        for s in sets {
            reconcile_set_fixed(&s.fixed, &s.arms, &observed_by_trial(recs), &mut seen);
        }
    }
    // B1/C1（B/C 比較）: 実際に動いた run の model は共通候補集合の内側、harness は共通 run_harnesses の内側。
    if pairs.contains(&(ConfigId::B, ConfigId::C)) {
        let rt = |c: ConfigId| {
            m.configs
                .iter()
                .find(|x| x.id == c)
                .and_then(|x| x.routing.as_ref())
        };
        if let (Some(b), Some(c)) = (rt(ConfigId::B), rt(ConfigId::C)) {
            fn set(v: &Option<Vec<String>>) -> Option<BTreeSet<&String>> {
                v.as_ref().map(|v| v.iter().collect())
            }
            let common_models = match (set(&b.models), set(&c.models)) {
                (Some(x), Some(y)) => Some(x.intersection(&y).copied().collect::<BTreeSet<_>>()),
                _ => None,
            };
            let common_h = match (set(&b.run_harnesses), set(&c.run_harnesses)) {
                (Some(x), Some(y)) => Some(x.intersection(&y).copied().collect::<BTreeSet<_>>()),
                _ => None,
            };
            // 宣言（共通候補集合・harness・tier 対応・根拠表不使用）を実測の前に検査する。
            let cfg = |c: ConfigId| m.configs.iter().find(|x| x.id == c);
            if let (Some(bc), Some(cc)) = (cfg(ConfigId::B), cfg(ConfigId::C))
                && let Err(e) = crate::manifest::check_b1_c1_common(bc, cc)
            {
                seen.insert(e);
            }
            // 実測: fixed-core の照合を共通条件から作った SetFixed で再利用する（model/harness/lower_policy の
            // None・集合外・worker 欠落・観測 run 欠落・run id の重複を違反にする）。
            let common = crate::manifest::SetFixed {
                policy: b.lower_policy.clone(),
                models: common_models
                    .as_ref()
                    .map(|s| s.iter().map(|x| (*x).clone()).collect()),
                harnesses: common_h
                    .as_ref()
                    .map(|s| s.iter().map(|x| (*x).clone()).collect()),
                ..Default::default()
            };
            let trials = observed_by_trial(recs);
            if common.models.is_none() || common.harnesses.is_none() || common.policy.is_none() {
                seen.insert(
                    "B1/C1: common candidate models/run_harnesses/lower_policy are not declared"
                        .into(),
                );
            }
            reconcile_set_fixed(&common, &[ConfigId::B, ConfigId::C], &trials, &mut seen);
            // B と C は同じ (problem, rep) 集合を実行している（片側だけの試行は同条件の比較にならない）。
            let keys = |c: ConfigId| {
                trials
                    .iter()
                    .filter(|(a, _, _)| *a == c)
                    .map(|(_, _, r)| (r.problem_id.clone(), r.rep))
                    .collect::<BTreeSet<_>>()
            };
            let (kb, kc) = (keys(ConfigId::B), keys(ConfigId::C));
            for (p, rep) in kb.symmetric_difference(&kc) {
                seen.insert(format!(
                    "B1/C1: {p}/rep{rep} was run by only one of B and C"
                ));
            }
            for arm in [ConfigId::B, ConfigId::C] {
                for (tag, o) in observed(arm) {
                    // 許す差は routing mode だけ（B=fixed / C=decision_assisted）。観測欠落は違反。
                    match (&o.routing_mode, arm) {
                        (None, _) => {
                            seen.insert(format!(
                                "{tag}: run {} ({}): B1/C1 routing mode not observed",
                                o.run_id, o.role
                            ));
                        }
                        (Some(g), ConfigId::B) if g != "fixed" => {
                            seen.insert(format!("{tag}: B1/C1 B ran routing mode {g}"));
                        }
                        (Some(g), ConfigId::C) if g != "decision_assisted" => {
                            seen.insert(format!("{tag}: B1/C1 C ran routing mode {g}"));
                        }
                        _ => {}
                    }
                    if o.evidence_table_id.is_some() || o.evidence_mode.as_deref() != Some("none") {
                        seen.insert(format!(
                            "{tag}: run {} ({}): B1/C1 used an evidence table (evidence_mode {:?}, table {:?})",
                            o.run_id, o.role, o.evidence_mode, o.evidence_table_id
                        ));
                    }
                }
                for (tag, o) in observed(arm) {
                    let who = format!("{tag}: run {} ({})", o.run_id, o.role);
                    if let (Some(cm), Some(model)) = (&common_models, &o.model)
                        && !cm.contains(model)
                    {
                        seen.insert(format!(
                            "{who}: B1/C1 ran model {model} outside the common candidate set"
                        ));
                    }
                    if let (Some(ch), Some(h)) = (&common_h, &o.harness)
                        && !ch.contains(h)
                    {
                        seen.insert(format!(
                            "{who}: B1/C1 ran harness {h} outside the common run_harnesses"
                        ));
                    }
                }
            }
        }
    }
    v.extend(seen);
}

/// 宣言した構成（manifest）と、実際に走った全 run の実測（`TrialRecord.observed`）の照合。
/// 宣言から導いた値は実測として扱わない。違反は文言に理由を残す。
/// - A: 実際に渡した `--model` は宣言した model と一致する（harness の実行ファイル名は表示するだけ）。
/// - B/C: worker run の model/harness は arm の `models` / `run_harnesses` の内側、判断 record の routing は
///   arm の mode・出自 task・online_update・下段・根拠表と一致する（全体 config へ落ちた run を検出する）。
fn reconcile_observed(
    m: &CampaignManifest,
    r: &TrialRecord,
    tag: &str,
    celeris_run: bool,
    v: &mut Vec<String>,
) {
    let Some(cfg) = m.configs.iter().find(|c| c.id == r.config) else {
        return;
    };
    for o in &r.observed {
        let who = format!("{tag}: run {} ({})", o.run_id, o.role);
        if r.config == ConfigId::A {
            if let Some(model) = &o.model
                && *model != cfg.model
            {
                v.push(format!(
                    "{who}: ran model {model:?} but the manifest declares {:?}",
                    cfg.model
                ));
            }
            continue;
        }
        let Some(rt) = &cfg.routing else { continue };
        let mode = if r.config == ConfigId::B {
            "fixed"
        } else {
            "decision_assisted"
        };
        // routing は reviewer run を含む全 run が、task 欄の固定から解決されていなければならない。
        if celeris_run && o.harness.is_some() {
            match (&o.routing_mode, &o.routing_source) {
                (Some(got), Some(src)) => {
                    if got != mode {
                        v.push(format!(
                            "{who}: routing mode {got} but arm {:?} requires {mode}",
                            r.config
                        ));
                    }
                    if src != "task" {
                        v.push(format!(
                            "{who}: routing resolved from {src}, not the pinned task field"
                        ));
                    }
                }
                _ => v.push(format!(
                    "{who}: no resolved routing recorded (fell back to the global config?)"
                )),
            }
        }
        if let Some(got) = o.online_update
            && got != rt.online_update.unwrap_or(false)
        {
            v.push(format!(
                "{who}: online_update {got} differs from the manifest"
            ));
        }
        if let (Some(got), Some(want)) = (&o.lower_policy, &rt.lower_policy)
            && got != want
        {
            v.push(format!(
                "{who}: lower_policy {got} differs from the manifest {want}"
            ));
        }
        if o.routing_mode.is_some() {
            match (&o.evidence_mode, rt.effective_evidence_mode(r.config)) {
                (Some(got), Ok(want)) if got == want => {}
                (got, want) => v.push(format!(
                    "{who}: evidence_mode {got:?} differs from the manifest {want:?}"
                )),
            }
        }
        if o.routing_mode.is_some() && o.evidence_table_id != rt.evidence_table_id {
            v.push(format!(
                "{who}: evidence_table_id {:?} differs from the manifest {:?}",
                o.evidence_table_id, rt.evidence_table_id
            ));
        }
        if o.role == "worker" {
            if let (Some(model), Some(allowed)) = (&o.model, &rt.models)
                && !allowed.contains(model)
            {
                v.push(format!(
                    "{who}: model {model:?} is outside the arm's models set"
                ));
            }
            if let (Some(h), Some(allowed)) = (&o.harness, &rt.run_harnesses)
                && !allowed.contains(h)
            {
                v.push(format!(
                    "{who}: harness {h:?} is outside the arm's run_harnesses set"
                ));
            }
        }
    }
}

/// 新規投入の gate（`budget::admit`）が閉じる条件: 確定消費の残りが 1 試行の予約（per_case）を下回る。
/// 残りが 0 になるまで待たない（予約を下回った時点で、in-flight が無ければ打ち切る）。
fn budget_gate_closed(b: &crate::manifest::Budget, tokens: u64, wall_secs: u64) -> bool {
    b.max_tokens.saturating_sub(tokens) < b.per_case.max_tokens
        || b.max_wall_secs.saturating_sub(wall_secs) < b.per_case.max_wall_secs
}

pub fn check(m: &CampaignManifest, out: &Path) -> CheckReport {
    check_expected(m, out, None)
}

/// `expected` は cases root から作り直した期待 lock（CLI）。保存 lock と食い違えば違反。
pub fn check_expected(
    m: &CampaignManifest,
    out: &Path,
    want_lock: Option<&crate::lock::CampaignLock>,
) -> CheckReport {
    let mut v: Vec<String> = Vec::new();
    let mut recs: Vec<TrialRecord> = Vec::new();
    let result = (|| -> Result<(), String> {
        m.validate()?;
        let hash = m.compute_sha256()?;
        // 初期木を宣言しない case は repo なしの seed で始まる（lock が固定。信用できない lock なら空集合 = 通常 seed と比べて失敗）。
        let repo_free = crate::lock_exec::load_saved(m, out, want_lock)
            .ok()
            .flatten()
            .map(|l| crate::lock_exec::repo_free_cases(&l))
            .unwrap_or_default();
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
            reconcile_observed(m, r, &tag, celeris_run, &mut v);
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
            // 「変わっていない」は runner と同じ規則: 配った入力（`inputs/`）は成果物に数えない。初期 hash は入力を除いた値、
            // 最終 hash は入力込みの値なので、入力のある試行は workspace の現物を入力抜きで数え直して比べる。
            let unchanged = || {
                if r.inputs.is_empty() {
                    return r.workspace_sha256_final == r.workspace_sha256_initial;
                }
                let ws = out.join(&r.workspace);
                crate::snapshot::hash_tree(&ws).ok().as_ref() == Some(&r.workspace_sha256_final)
                    && crate::snapshot::hash_tree_excluding(&ws, &[crate::inputs::INPUTS_DIR])
                        .ok()
                        .as_ref()
                        == Some(&r.workspace_sha256_initial)
            };
            if r.reason.as_deref() == Some(REASON_NO_ARTIFACT)
                && (r.config != ConfigId::A || r.outcome != Outcome::Failed || !unchanged())
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
                let seed_key = if repo_free.contains(&r.problem_id) {
                    "workspace_seed_empty_sha256"
                } else {
                    "workspace_seed_sha256"
                };
                if r.workspace_sha256_initial != snap[seed_key]
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
            } else if !budget_gate_closed(&m.budget, cum_tokens, cum_wall) {
                v.push(format!(
                    "{tag}: cancelled for budget although budget remained"
                ));
            }
        }
        reconcile_sets(m, &recs, &mut v);
        Ok(())
    })();
    if let Err(e) = result {
        v.push(e);
        return CheckReport {
            ok: false,
            violations: v,
            complete: false,
            incomplete_reasons: vec!["records could not be verified".into()],
            records: recs.len(),
            recomputed: None,
        };
    }
    let saved_lock = match crate::lock_exec::load_saved(m, out, want_lock) {
        Ok(l) => l,
        Err(e) => {
            v.push(format!("lock: {e}"));
            None
        }
    };
    reconcile_set_evidence(
        m,
        read_json(&out.join(SNAPSHOT_FILE)).ok().as_ref(),
        saved_lock.as_ref(),
        &mut v,
    );
    let mut binding = crate::grades::bind(m, &recs, out, saved_lock.as_ref());
    if v.iter().any(|x| x.starts_with("lock: ")) {
        // lock を信用できないときは grade を 1 件も有効にしない（未採点として分母に残る）。
        binding.valid.clear();
        if out.join(crate::grades::GRADES_FILE).exists() {
            v.push(
                "lock: grades.jsonl is not bound because the campaign lock is not trusted".into(),
            );
        }
    }
    v.extend(binding.violations.iter().cloned());
    let rc = recompute_graded(m, &recs, &binding.valid);
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
    let mut reasons = Vec::new();
    for (c, t) in &rc.totals {
        if t.missing > 0 {
            reasons.push(format!("{c:?}: {} trials have no record", t.missing));
        }
        if t.unexecuted > 0 {
            reasons.push(format!(
                "{c:?}: {} trials were never executed (task creation failed or budget stop)",
                t.unexecuted
            ));
        }
        if t.ungraded > 0 {
            reasons.push(format!(
                "{c:?}: {} executor-passed trials have no valid independent grade",
                t.ungraded
            ));
        }
    }
    CheckReport {
        ok: v.is_empty(),
        complete: reasons.is_empty(),
        incomplete_reasons: reasons,
        violations: v,
        records: recs.len(),
        recomputed: Some(rc),
    }
}

/// 生記録から再計算する。各 (config, 問題) は trial_no 最小の記録で判定し、無ければ missing。
/// 分母は期待された問題数で、failed/error/timeout/cancelled/missing のどれも除かない。
pub fn recompute(m: &CampaignManifest, recs: &[TrialRecord]) -> Recomputed {
    recompute_graded(m, recs, &BTreeMap::new())
}

/// `grades`: 結合検査を通った独立採点（試行 → final_pass）。無い試行は未採点で、成功に数えない。
pub fn recompute_graded(
    m: &CampaignManifest,
    recs: &[TrialRecord],
    grades: &BTreeMap<crate::grades::TrialKey, bool>,
) -> Recomputed {
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
            let cell = first.get(&(*c, p.as_str(), rep));
            if cell.is_some_and(|r| {
                r.reason.as_deref() == Some(REASON_EXECUTOR_ERROR)
                    || (!r.ran() && r.reason.as_deref() != Some(REASON_BLOCKED))
            }) {
                t.unexecuted += 1;
            }
            match cell.map(|r| r.outcome) {
                None => t.missing += 1,
                Some(o) => match o {
                    crate::aggregate::Outcome::Passed => {
                        t.passed += 1;
                        match cell.and_then(|r| grades.get(&crate::grades::key_of(r))) {
                            Some(true) => t.final_passed += 1,
                            Some(false) => t.grader_failed += 1,
                            None => t.ungraded += 1,
                        }
                    }
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
            if r.observed.is_empty() {
                t.unobserved_trials += 1;
            }
            for o in &r.observed {
                t.observed_runs += 1;
                t.observed_harnesses.extend(o.harness.clone());
                t.observed_sources.extend(o.source.clone());
                t.observed_models.extend(o.model.clone());
                if let (Some(mode), Some(src)) = (&o.routing_mode, &o.routing_source) {
                    t.observed_routing.insert(format!(
                        "{mode}/{src}/online_update={}",
                        o.online_update.map_or("欠測".into(), |b| b.to_string())
                    ));
                }
            }
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
        first.get(&(c, p, rep)).is_some_and(|r| {
            r.outcome == crate::aggregate::Outcome::Passed
                && grades.get(&crate::grades::key_of(r)) == Some(&true)
        })
    };
    let mut pairs = Vec::new();
    // 選択した比較組だけ（選んでいない比較表は出さない）。名前は従来の primary(A,B) / secondary(B,C)。
    for (a, b) in m.compare_pairs() {
        let name = if (a, b) == (ConfigId::B, ConfigId::C) {
            "secondary"
        } else {
            "primary"
        };
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
