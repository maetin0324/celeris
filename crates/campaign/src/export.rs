//! export: checker が再計算した値から設定 A/B/C 別の表（JSON / CSV）を書く。
//! checker が違反を報告した記録は export しない。
use std::{fs, path::Path};

use serde_json::{Value, json};

use crate::{
    checker::{Totals, check},
    manifest::CampaignManifest,
};

pub const MISSING_LABEL: &str = "欠測";

fn status(sum_missing: u64, ran: u64) -> &'static str {
    if ran == 0 || sum_missing == ran {
        "missing"
    } else if sum_missing == 0 {
        "observed"
    } else {
        "partial"
    }
}

fn row(m: &CampaignManifest, c: crate::manifest::ConfigId, t: &Totals) -> Value {
    let cfg = m.configs.iter().find(|x| x.id == c);
    let ts = status(t.tokens_missing_trials, t.ran_trials);
    let cs = status(t.cost_missing_trials, t.ran_trials);
    // 表の harness / model は実測（全 run の記録）。宣言名は declared_* に分けて残し、実測の代わりに使わない。
    let seen = |s: &std::collections::BTreeSet<String>| {
        if s.is_empty() {
            Value::Null
        } else {
            json!(s.iter().cloned().collect::<Vec<_>>().join("|"))
        }
    };
    json!({
        "config": format!("{c:?}"),
        "harness": seen(&t.observed_harnesses),
        "model": seen(&t.observed_models),
        "source": seen(&t.observed_sources),
        "routing": seen(&t.observed_routing),
        "observed_runs": t.observed_runs,
        "unobserved_trials": t.unobserved_trials,
        "declared_harness": cfg.map(|x| x.harness.as_str()),
        "declared_model": cfg.map(|x| x.model.as_str()),
        "model_version": cfg.map(|x| x.model_version.as_str()),
        "policy_version": cfg.map(|x| x.policy_version.as_str()),
        "n": t.n, "passed": t.passed, "final_passed": t.final_passed,
        "ungraded": t.ungraded, "grader_failed": t.grader_failed, "failed": t.failed, "error": t.error,
        "timeout": t.timeout, "cancelled": t.cancelled, "blocked": t.blocked, "missing": t.missing,
        "failures": t.n - t.final_passed,
        "pass_rate": if t.n == 0 { Value::Null } else { json!(t.final_passed as f64 / t.n as f64) },
        "ran_trials": t.ran_trials,
        "tokens_status": ts,
        "tokens_sum": if ts == "missing" { Value::Null } else { json!(t.tokens_sum) },
        "tokens_missing_trials": t.tokens_missing_trials,
        "token_basis": m.budget.token_basis,
        "input_tokens_sum": t.input_tokens_sum,
        "output_tokens_sum": t.output_tokens_sum,
        "cache_read_tokens_sum": t.cache_read_tokens_sum,
        "cache_creation_tokens_sum": t.cache_creation_tokens_sum,
        "token_usage_missing_trials": t.token_usage_missing_trials,
        "cache_missing_trials": t.cache_missing_trials,
        "cost_status": cs,
        "cost_micro_usd_sum": if cs == "missing" { Value::Null } else { json!(t.cost_micro_usd_sum) },
        "cost_missing_trials": t.cost_missing_trials,
    })
}

const COLUMNS: [&str; 37] = [
    "config",
    "harness",
    "model",
    "source",
    "routing",
    "observed_runs",
    "unobserved_trials",
    "declared_harness",
    "declared_model",
    "model_version",
    "policy_version",
    "n",
    "passed",
    "final_passed",
    "ungraded",
    "grader_failed",
    "failed",
    "error",
    "timeout",
    "cancelled",
    "blocked",
    "missing",
    "failures",
    "pass_rate",
    "ran_trials",
    "tokens_status",
    "tokens_sum",
    "tokens_missing_trials",
    "token_basis",
    "input_tokens_sum",
    "output_tokens_sum",
    "cache_read_tokens_sum",
    "cache_creation_tokens_sum",
    "token_usage_missing_trials",
    "cache_missing_trials",
    "cost_micro_usd_sum",
    "cost_status",
];

fn cell(v: &Value, col: &str) -> String {
    match (&v[col], col) {
        (Value::Null, "pass_rate") => MISSING_LABEL.into(),
        (Value::Null, _) => MISSING_LABEL.into(),
        (Value::Number(n), "pass_rate") => format!("{:.4}", n.as_f64().unwrap_or(0.0)),
        (Value::Number(n), _) => n.to_string(),
        (Value::String(s), _) => s.clone(),
        (o, _) => o.to_string(),
    }
}

/// 表を `dest` に書く。戻り値は table の内容。
/// 成功 = 実行器 passed かつ独立採点 final_pass。未完了（記録欠落・未採点）の集計は `table.incomplete.*` に
/// `complete: false` を付けて書き、最終版の `table.json` / `table.csv` は書かない（古い最終版も消す）。
pub fn export(m: &CampaignManifest, out: &Path, dest: &Path) -> Result<Value, String> {
    let rep = check(m, out);
    let (complete, reasons) = (rep.complete, rep.incomplete_reasons.clone());
    let rc = match (rep.ok, rep.recomputed) {
        (true, Some(rc)) => rc,
        _ => {
            return Err(format!(
                "checker rejected the records: {}",
                rep.violations.join("; ")
            ));
        }
    };
    let rows: Vec<Value> = rc.totals.iter().map(|(c, t)| row(m, *c, t)).collect();
    // 印は run-meta（runner が書く）から引く。無い・読めないときは実測とみなさず mock 扱いにする。
    let kind = fs::read(out.join(crate::record::META_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v["evidence_kind"].as_str().map(String::from))
        .unwrap_or_else(|| "unknown".into());
    let real = kind == "real";
    let table = json!({
        "schema": "celeris.campaign-table/1",
        "campaign_sha256": m.compute_sha256()?,
        "split": m.split,
        "evidence_kind": kind,
        "complete": complete,
        "incomplete_reasons": reasons,
        "success_definition": "executor outcome passed AND independent grade final_pass (bound by campaign hash, case id, split, grader hash, artifact hash)",
        "real_performance": real && complete,
        "notice": if real { Value::Null } else {
            json!("mock/unknown evidence: these numbers are not real performance and must not be reported as measurements") },
        "configs": rows,
        "comparisons": rc.pairs,
    });
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let stem = if complete {
        "table"
    } else {
        "table.incomplete"
    };
    let stale = if complete {
        "table.incomplete"
    } else {
        "table"
    };
    for ext in ["json", "csv"] {
        let p = dest.join(format!("{stale}.{ext}"));
        if p.exists() {
            fs::remove_file(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        }
    }
    let mut s = serde_json::to_string_pretty(&table).map_err(|e| e.to_string())?;
    s.push('\n');
    fs::write(dest.join(format!("{stem}.json")), s).map_err(|e| e.to_string())?;
    let mut csv = COLUMNS.join(",") + ",evidence_kind,complete\n";
    for r in &rows {
        let line: Vec<String> = COLUMNS.iter().map(|c| cell(r, c)).collect();
        csv += &(line.join(",") + &format!(",{kind},{complete}\n"));
    }
    fs::write(dest.join(format!("{stem}.csv")), csv).map_err(|e| e.to_string())?;
    Ok(table)
}
