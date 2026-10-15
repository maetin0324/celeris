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
    json!({
        "config": format!("{c:?}"),
        "harness": cfg.map(|x| x.harness.as_str()),
        "model": cfg.map(|x| x.model.as_str()),
        "model_version": cfg.map(|x| x.model_version.as_str()),
        "policy_version": cfg.map(|x| x.policy_version.as_str()),
        "n": t.n, "passed": t.passed, "failed": t.failed, "error": t.error,
        "timeout": t.timeout, "cancelled": t.cancelled, "missing": t.missing,
        "failures": t.n - t.passed,
        "pass_rate": if t.n == 0 { Value::Null } else { json!(t.passed as f64 / t.n as f64) },
        "ran_trials": t.ran_trials,
        "tokens_status": ts,
        "tokens_sum": if ts == "missing" { Value::Null } else { json!(t.tokens_sum) },
        "tokens_missing_trials": t.tokens_missing_trials,
        "cost_status": cs,
        "cost_micro_usd_sum": if cs == "missing" { Value::Null } else { json!(t.cost_micro_usd_sum) },
        "cost_missing_trials": t.cost_missing_trials,
    })
}

const COLUMNS: [&str; 20] = [
    "config",
    "harness",
    "model",
    "model_version",
    "policy_version",
    "n",
    "passed",
    "failed",
    "error",
    "timeout",
    "cancelled",
    "missing",
    "failures",
    "pass_rate",
    "ran_trials",
    "tokens_status",
    "tokens_sum",
    "tokens_missing_trials",
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

/// 表を `dest` に書く。戻り値は table.json の内容。
pub fn export(m: &CampaignManifest, out: &Path, dest: &Path) -> Result<Value, String> {
    let rep = check(m, out);
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
    let table = json!({
        "schema": "celeris.campaign-table/1",
        "campaign_sha256": m.compute_sha256()?,
        "split": m.split,
        "configs": rows,
        "comparisons": rc.pairs,
    });
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let mut s = serde_json::to_string_pretty(&table).map_err(|e| e.to_string())?;
    s.push('\n');
    fs::write(dest.join("table.json"), s).map_err(|e| e.to_string())?;
    let mut csv = COLUMNS.join(",") + "\n";
    for r in &rows {
        let line: Vec<String> = COLUMNS.iter().map(|c| cell(r, c)).collect();
        csv += &(line.join(",") + "\n");
    }
    fs::write(dest.join("table.csv"), csv).map_err(|e| e.to_string())?;
    Ok(table)
}
