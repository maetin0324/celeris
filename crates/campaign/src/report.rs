//! 報告本文の数値検査。報告の `result <config> passed=<x> n=<y> split=<s>` 行を、
//! checker が生記録から再計算した値と突き合わせる（Markdown だけでは合格しない）。
use std::collections::BTreeMap;

use crate::{
    checker::{Recomputed, check},
    manifest::{CampaignManifest, ConfigId},
};
use std::path::Path;

fn parse_cfg(s: &str) -> Option<ConfigId> {
    match s {
        "A" => Some(ConfigId::A),
        "B" => Some(ConfigId::B),
        "C" => Some(ConfigId::C),
        _ => None,
    }
}

/// 違反の一覧（空なら合格）。out 自体の checker 違反も含める。
pub fn check_report(m: &CampaignManifest, out: &Path, text: &str) -> Vec<String> {
    let rep = check(m, out);
    let mut v = rep.violations.clone();
    let Some(rc) = rep.recomputed else {
        return v;
    };
    for r in &rep.incomplete_reasons {
        v.push(format!(
            "report: campaign is incomplete, not a final result: {r}"
        ));
    }
    v.extend(compare(m, &rc, text));
    v
}

pub fn compare(m: &CampaignManifest, rc: &Recomputed, text: &str) -> Vec<String> {
    let mut v = Vec::new();
    let mut seen: BTreeMap<ConfigId, (u64, u64, String)> = BTreeMap::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        if it.next() != Some("result") {
            continue;
        }
        let Some(cfg) = it.next().and_then(parse_cfg) else {
            v.push(format!("report: unknown config in line `{line}`"));
            continue;
        };
        let kv: BTreeMap<&str, &str> = it.filter_map(|t| t.split_once('=')).collect();
        let num = |k: &str| kv.get(k).and_then(|x| x.parse::<u64>().ok());
        match (num("passed"), num("n"), kv.get("split")) {
            (Some(p), Some(n), Some(s)) => {
                if seen.insert(cfg, (p, n, (*s).to_string())).is_some() {
                    v.push(format!("report: {cfg:?} reported twice"));
                }
            }
            _ => v.push(format!("report: malformed result line `{line}`")),
        }
    }
    let split = match m.split {
        crate::manifest::Split::Dev => "dev",
        crate::manifest::Split::Final => "final",
    };
    for c in seen.keys().filter(|c| !rc.totals.contains_key(c)) {
        v.push(format!(
            "report: result row for {c:?}, which is not in the effective plan (unused arms are not reported)"
        ));
    }
    for (c, t) in &rc.totals {
        match seen.get(c) {
            None => v.push(format!("report: no result row for config {c:?}")),
            Some((p, n, s)) => {
                if *n != t.n {
                    v.push(format!(
                        "report: {c:?} denominator n={n} but expected {} (failures/timeouts/missing must stay in the denominator)",
                        t.n
                    ));
                }
                if *p != t.final_passed {
                    v.push(format!(
                        "report: {c:?} passed={p} but recomputed from raw records and independent grades {}",
                        t.final_passed
                    ));
                }
                if s != split {
                    v.push(format!(
                        "report: {c:?} split={s} but the campaign split is {split} (dev and final must not be mixed)"
                    ));
                }
            }
        }
    }
    v
}

/// 報告の消費の主張を最終集計（`final_accounting::compute` の出力）と突き合わせる。
/// - `consumption metric=<m> total=<n> scope=<complete|observed_partial>`: 値は観測和と一致し、
///   欠測のある指標を `complete`（完全な総費用）と呼ばない。
/// - `reduction metric=<m> confirmed=<true|false>`: 欠測のある指標の削減を確定にしない。
pub fn check_consumption_claims(acct: &serde_json::Value, text: &str) -> Vec<String> {
    let key = |m: &str| match m {
        "tokens" => Some("tokens"),
        "list_cost_usd" => Some("list_cost_micro_usd"),
        "cash_cost_usd" => Some("cash_cost_micro_usd"),
        _ => None,
    };
    let mut v = Vec::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let head = it.next();
        if !matches!(head, Some("consumption" | "reduction")) {
            continue;
        }
        let kv: BTreeMap<&str, &str> = it.filter_map(|t| t.split_once('=')).collect();
        let Some(metric) = kv.get("metric").copied().filter(|m| key(m).is_some()) else {
            v.push(format!("report: missing or unknown metric in `{line}`"));
            continue;
        };
        let complete = acct["total"]["complete"][metric] == true;
        if head == Some("reduction") {
            if kv.get("confirmed") == Some(&"true")
                && !acct["reduction_confirmable"][metric]
                    .as_bool()
                    .unwrap_or(false)
            {
                v.push(format!(
                    "report: reduction on {metric} is confirmed but the accounting has missing data; it cannot be confirmed"
                ));
            }
            continue;
        }
        let k = key(metric).unwrap_or_default();
        match kv.get("scope").copied() {
            Some("complete") if !complete => v.push(format!(
                "report: {metric} total is called complete but only an observed partial sum exists (missing data)"
            )),
            Some("complete" | "observed_partial") => {}
            _ => v.push(format!("report: scope must be complete or observed_partial in `{line}`")),
        }
        let stated = kv.get("total").and_then(|x| x.parse::<u64>().ok());
        if stated != acct["observed_sum"][k].as_u64() {
            v.push(format!(
                "report: {metric} total {stated:?} differs from the observed sum {}",
                acct["observed_sum"][k]
            ));
        }
    }
    v
}

/// 判定範囲の宣言行。報告はこの行を持たねばならない（`not_judged` がある campaign）。
pub fn scope_line(judged: &[String], not_judged: &[String]) -> String {
    format!(
        "judged_scope stages={} not_judged={}",
        judged.join(","),
        not_judged.join(",")
    )
}

/// `case.json` から campaign の判定範囲を引く（問題 id に対応する case が `cases_root/<split>/<id>/case.json` に有るものの和）。
pub fn campaign_scope(
    m: &CampaignManifest,
    cases_root: &Path,
) -> Result<(Vec<String>, Vec<String>), String> {
    let split = match m.split {
        crate::manifest::Split::Dev => "dev",
        crate::manifest::Split::Final => "final",
    };
    let (mut judged, mut not_judged) = (Vec::new(), Vec::new());
    for id in &m.problems {
        let p = cases_root.join(split).join(id).join("case.json");
        if !p.exists() {
            continue;
        }
        let c: crate::cases::Case = serde_json::from_slice(
            &std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?,
        )
        .map_err(|e| format!("{}: {e}", p.display()))?;
        for s in c.judged_scope() {
            if !judged.contains(&s) {
                judged.push(s);
            }
        }
        for s in c.not_judged() {
            if !not_judged.contains(&s) {
                not_judged.push(s);
            }
        }
    }
    // 工程は case ごとに違い得る。ある case が判定する工程でも、別の case が判定しないなら campaign 全体では not_judged に残す。
    judged.retain(|s| !not_judged.contains(s));
    Ok((judged, not_judged))
}

const OVERCLAIM_PHRASES: [&str; 3] = [
    "分析・レビューまで検証済み",
    "analysis and review verified",
    "analysis/review verified",
];

/// 判定しなかった工程を検証済みと主張する経路を塞ぐ。
/// - `judged_scope stages=.. not_judged=..` 行が `scope_line` と一致する（無い・食い違いは違反）。
/// - `verified stage=<s>` で not_judged の工程を主張する行、および定型の過大主張句は違反。
pub fn check_scope_claims(judged: &[String], not_judged: &[String], text: &str) -> Vec<String> {
    let mut v = Vec::new();
    if not_judged.is_empty() {
        return v;
    }
    let want = scope_line(judged, not_judged);
    if !text.lines().any(|l| l.trim() == want) {
        v.push(format!("report: missing scope declaration `{want}`"));
    }
    for line in text.lines() {
        let mut it = line.split_whitespace();
        if it.next() == Some("verified") {
            let kv: BTreeMap<&str, &str> = it.filter_map(|t| t.split_once('=')).collect();
            if let Some(s) = kv.get("stage")
                && not_judged.iter().any(|n| n == s)
            {
                v.push(format!(
                    "report: stage {s} is not judged in this campaign and cannot be claimed verified"
                ));
            }
        }
        if let Some(p) = OVERCLAIM_PHRASES.iter().find(|p| line.contains(**p)) {
            v.push(format!(
                "report: `{p}` overclaims; not judged: {}",
                not_judged.join(",")
            ));
        }
    }
    v
}
