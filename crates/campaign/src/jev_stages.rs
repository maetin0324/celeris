//! Jev 段階別集計: 記録（`routing_decided` の lane_assist・Jev 消費行・run・結果）から、段ごとの件数を数える純関数。
//! 判定は既存の `jev_smoke::judge` を再利用する。実 API は呼ばない。
//!
//! 欠測は欠測のまま数える（推測で埋めない）。変化 0・効果なしは 0 のまま出す。
//! `confidence` は読まない（分類器自身の確信であり、成功確率でも propensity でもない）。
use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use crate::jev_smoke::{ClassifierKind, Observation, judge};

/// 1 試行分の記録。`jev_usage`: Jev の消費行（`cost_usd` 欠測は null）。
/// `run.proposed_model` は提案先頭、`run.exec_model` は実行した model。
pub fn observations_from(records: &Value) -> Result<Vec<Record>, String> {
    let arr = records
        .as_array()
        .or_else(|| records.get("records").and_then(Value::as_array))
        .ok_or("records は配列、または {\"records\": [...]} でなければならない")?;
    arr.iter()
        .enumerate()
        .map(|(i, v)| Record::parse(i, v))
        .collect()
}

#[derive(Clone, Debug)]
pub struct Record {
    pub arm: String,
    pub obs: Observation,
    pub jev_calls: u64,
    pub proposed_model: Option<String>,
    pub exec_model: Option<String>,
    pub run_fallback: Option<String>,
    pub budget_stopped: bool,
    pub result: Option<String>,
    pub retries: u64,
    pub total_cost_usd: Option<f64>,
}

impl Record {
    fn parse(i: usize, v: &Value) -> Result<Self, String> {
        let s = |p: &str| v.pointer(p).and_then(Value::as_str).map(str::to_owned);
        let usage = v.get("jev_usage").and_then(Value::as_array);
        let jev_calls = usage.map_or(0, |u| u.len() as u64);
        // 費用は全呼び出しで揃っているときだけ既知（1 つでも欠測なら欠測）。
        let cost = usage.filter(|u| !u.is_empty()).and_then(|u| {
            u.iter()
                .map(|r| r.get("cost_usd").and_then(Value::as_f64))
                .sum::<Option<f64>>()
        });
        let lanes = |k: &str| -> Vec<String> {
            v.get(k)
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default()
        };
        let event = v
            .get("event")
            .cloned()
            .ok_or_else(|| format!("records[{i}]: event（routing_decided）が無い"))?;
        Ok(Record {
            arm: s("/arm").unwrap_or_else(|| "?".into()),
            obs: Observation {
                task_id: s("/task_id").unwrap_or_else(|| format!("#{i}")),
                event,
                allowed: lanes("allowed"),
                max_lane: s("/max_lane"),
                usage_present: jev_calls > 0,
                cost_usd: cost,
                version: s("/version"),
                fixed_lane: s("/fixed_lane"),
            },
            jev_calls,
            proposed_model: s("/run/proposed_model"),
            exec_model: s("/run/exec_model"),
            run_fallback: s("/run/fallback_reason"),
            budget_stopped: s("/run/stopped_by").as_deref() == Some("budget"),
            result: s("/outcome/result"),
            retries: v
                .pointer("/outcome/retries")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            total_cost_usd: v.pointer("/outcome/cost_usd").and_then(Value::as_f64),
        })
    }
}

/// 抑止理由別の件数（常に全欄を出す。0 は 0）。
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Suppressed {
    /// 人・System の明示 lane が勝った。
    pub explicit: u64,
    /// 許可・天井が勝った。
    pub ceiling: u64,
    pub availability: u64,
    /// Jev の分類が fallback（理由名別は `classifier_fallback_reasons`）。
    pub classifier_fallback: u64,
    pub classifier_fallback_reasons: BTreeMap<String, u64>,
    /// 分類を試みていない（Jev 不調とは別）。
    pub not_attempted: u64,
    /// run 側の fallback（proxy が次候補へ送った）。
    pub run_fallback: u64,
    pub budget: u64,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Stages {
    pub trials: u64,
    // ① 分類試行
    pub classification_attempted: u64,
    pub not_attempted: u64,
    /// lane_assist 欄が読めない記録。
    pub missing: u64,
    // ② jev_ok
    pub jev_ok: u64,
    pub jev_fallback: u64,
    pub jev_calls: u64,
    /// jev_ok・費用・版が揃い許可内・帰属が矛盾しない件数。
    pub chain_ok: u64,
    pub jev_cost_missing: u64,
    pub jev_cost_usd: f64,
    pub version_missing: u64,
    // ③ lane 提案変更（proposed_lane が base_lane と違う）
    pub lane_proposal_changed: u64,
    pub lane_proposal_absent: u64,
    // ④ 制約適用後 lane
    pub final_lane_changed: u64,
    pub final_lane_counts: BTreeMap<String, u64>,
    pub lane_out_of_limits: u64,
    // ⑤ 実行 model 変更（提案先頭と実行が違う）
    pub model_changed: u64,
    /// どちらかの model が記録に無い。
    pub model_unknown: u64,
    /// lane が変わった試行のうち model も変わった件数。
    pub lane_changed_and_model_changed: u64,
    // ⑥ 抑止
    pub suppressed: Suppressed,
    // ⑦ 最終成果・再試行・全費用
    pub results: BTreeMap<String, u64>,
    pub result_missing: u64,
    pub retries: u64,
    pub total_cost_usd: f64,
    pub total_cost_missing: u64,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct StageReport {
    pub total: Stages,
    pub by_arm: BTreeMap<String, Stages>,
}

fn add(s: &mut Stages, r: &Record) {
    let v = judge(&r.obs);
    s.trials += 1;
    s.jev_calls += r.jev_calls;
    let la = r
        .obs
        .event
        .pointer("/record/lane_assist")
        .or_else(|| r.obs.event.get("lane_assist"))
        .filter(|x| x.is_object());
    let lane = |k: &str| la.and_then(|x| x.get(k)).and_then(Value::as_str);
    match &v.classifier {
        ClassifierKind::JevOk => {
            s.classification_attempted += 1;
            s.jev_ok += 1;
            if !v.cost_recorded {
                s.jev_cost_missing += 1;
            }
            if !v.version_recorded {
                s.version_missing += 1;
            }
        }
        ClassifierKind::Fallback { reason } => {
            s.classification_attempted += 1;
            s.jev_fallback += 1;
            s.suppressed.classifier_fallback += 1;
            *s.suppressed
                .classifier_fallback_reasons
                .entry(reason.clone().unwrap_or_else(|| "unknown".into()))
                .or_default() += 1;
        }
        ClassifierKind::NotAttempted => {
            s.not_attempted += 1;
            s.suppressed.not_attempted += 1;
        }
        ClassifierKind::Missing => s.missing += 1,
    }
    if let Some(c) = r.obs.cost_usd {
        s.jev_cost_usd += c;
    }
    if v.chain_ok {
        s.chain_ok += 1;
    }
    if la.is_some() {
        if !v.lane_in_limits {
            s.lane_out_of_limits += 1;
        }
        match (lane("proposed_lane"), lane("base_lane")) {
            (Some(p), Some(b)) if p != b => s.lane_proposal_changed += 1,
            (None, _) => s.lane_proposal_absent += 1,
            _ => {}
        }
        if let Some(f) = lane("final_lane") {
            *s.final_lane_counts.entry(f.to_owned()).or_default() += 1;
        }
    }
    let lane_changed =
        matches!((lane("base_lane"), lane("final_lane")), (Some(b), Some(f)) if b != f);
    if lane_changed {
        s.final_lane_changed += 1;
    }
    match lane("overridden_by") {
        Some("explicit") => s.suppressed.explicit += 1,
        Some("ceiling") => s.suppressed.ceiling += 1,
        Some("availability") => s.suppressed.availability += 1,
        _ => {}
    }
    match (&r.proposed_model, &r.exec_model) {
        (Some(p), Some(e)) if p != e => {
            s.model_changed += 1;
            if lane_changed {
                s.lane_changed_and_model_changed += 1;
            }
        }
        (Some(_), Some(_)) => {}
        _ => s.model_unknown += 1,
    }
    if r.run_fallback.is_some() {
        s.suppressed.run_fallback += 1;
    }
    if r.budget_stopped {
        s.suppressed.budget += 1;
    }
    match &r.result {
        Some(x) => *s.results.entry(x.clone()).or_default() += 1,
        None => s.result_missing += 1,
    }
    s.retries += r.retries;
    match r.total_cost_usd {
        Some(c) => s.total_cost_usd += c,
        None => s.total_cost_missing += 1,
    }
}

pub fn aggregate(records: &[Record]) -> StageReport {
    let mut total = Stages::default();
    let mut by_arm: BTreeMap<String, Stages> = BTreeMap::new();
    for r in records {
        add(&mut total, r);
        add(by_arm.entry(r.arm.clone()).or_default(), r);
    }
    StageReport { total, by_arm }
}

pub fn run_json(records: &Value) -> Result<StageReport, String> {
    Ok(aggregate(&observations_from(records)?))
}
