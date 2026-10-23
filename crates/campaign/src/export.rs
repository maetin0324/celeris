//! export: checker が再計算した値から設定 A/B/C 別の表（JSON / CSV）を書く。
//! checker が違反を報告した記録は export しない。
use std::{fs, path::Path};

use serde_json::{Value, json};

use crate::{
    checker::{Totals, check_expected},
    manifest::CampaignManifest,
};

pub const FINAL_ACCOUNTING_FILE: &str = "final-accounting.json";
pub const MISSING_LABEL: &str = "欠測";
/// 主比較の消費の範囲。CoS の call と共有オーバーヘッドは含まない（arm へ配分しない）。
pub const COST_SCOPE: &str = "trial runs only (tokens/cost recorded on each trial; for B the task tree runs of the trial). CoS calls and shared overhead are NOT included and are not attributed to arms. cost_micro_usd_sum is the per-trial recorded cost; list-price and cash cost are kept apart only in final-accounting.json (never added together); missing trials are counted, never zero-filled";

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
    let mut row = json!({
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
        "ungraded": t.ungraded, "grader_failed": t.grader_failed,
        "grader_env_failure": t.grader_env_failure, "grader_undetermined": t.grader_undetermined,
        "grader_control_only": t.grader_control_only, "failed": t.failed, "error": t.error,
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
    });
    // 固定分母 n の総合値（pass_rate = final_passed / n）と、不成功の原因別内訳は別に持つ。
    // grader_env_failure / grader_undetermined は再採点前の未確定で、モデルの失敗に混ぜない
    // （complete=false で最終表を止める）。
    row["fault_breakdown"] = json!(t.fault_breakdown);
    row["failure_breakdown"] = json!({
        "model_failure": t.grader_failed,
        "grader_env_failure": t.grader_env_failure,
        "grader_undetermined": t.grader_undetermined,
        "grader_control_only": t.grader_control_only,
        "ungraded": t.ungraded,
        "executor_failed": t.failed,
        "executor_error": t.error,
        "executor_timeout": t.timeout,
        "executor_cancelled": t.cancelled,
        "executor_blocked": t.blocked,
        "missing": t.missing,
    });
    row
}

pub const COLUMNS: [&str; 41] = [
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
    "grader_env_failure",
    "grader_undetermined",
    "grader_control_only",
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
    "fault_breakdown",
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

/// 表の CSV（RFC 4180。JSON の object を持つ `fault_breakdown` も 1 セルに quoting する）。
/// 列は `COLUMNS` + `evidence_kind` + `complete`。
pub fn csv_table(rows: &[Value], evidence_kind: &str, complete: bool) -> String {
    let header: Vec<&str> = COLUMNS
        .iter()
        .copied()
        .chain(["evidence_kind", "complete"])
        .collect();
    let mut csv = crate::csv::line(&header);
    for r in rows {
        let mut line: Vec<String> = COLUMNS.iter().map(|c| cell(r, c)).collect();
        line.push(evidence_kind.to_string());
        line.push(complete.to_string());
        csv += &crate::csv::line(&line);
    }
    csv
}

/// 表を `dest` に書く。戻り値は table の内容。
/// 成功 = 実行器 passed かつ独立採点 final_pass。未完了（記録欠落・未採点）の集計は `table.incomplete.*` に
/// `complete: false` を付けて書き、最終版の `table.json` / `table.csv` は書かない（古い最終版も消す）。
pub fn export(m: &CampaignManifest, out: &Path, dest: &Path) -> Result<Value, String> {
    export_full(m, out, dest, None, None)
}

/// `export` に最終集計（`final_accounting::compute` の出力）を添える。表の `consumption` に範囲と削減の可否だけを写し、
/// 全文は `final-accounting.json` に書く。欠測があれば範囲は `observed_partial`（完全な総費用ではない）。
pub fn export_with(
    m: &CampaignManifest,
    out: &Path,
    dest: &Path,
    accounting: Option<&Value>,
) -> Result<Value, String> {
    export_full(m, out, dest, None, accounting)
}

/// 最終版を有効なまま残さない: checker が拒否したら dest の既存 `table.*` も消してから Err を返す。
pub fn export_expected(
    m: &CampaignManifest,
    out: &Path,
    dest: &Path,
    expected: Option<&crate::lock::CampaignLock>,
) -> Result<Value, String> {
    export_full(m, out, dest, expected, None)
}

/// 期待 lock との照合（`export_expected`）と最終集計の添付（`export_with`）を両方行う。CLI の export はこれを使う。
pub fn export_full(
    m: &CampaignManifest,
    out: &Path,
    dest: &Path,
    expected: Option<&crate::lock::CampaignLock>,
    accounting: Option<&Value>,
) -> Result<Value, String> {
    let rep = check_expected(m, out, expected);
    let (complete, reasons) = (rep.complete, rep.incomplete_reasons.clone());
    let rc = match (rep.ok, rep.recomputed) {
        (true, Some(rc)) => rc,
        _ => {
            for stem in ["table", "table.incomplete"] {
                for ext in ["json", "csv"] {
                    let p = dest.join(format!("{stem}.{ext}"));
                    if p.exists() {
                        fs::remove_file(&p).map_err(|e| format!("{}: {e}", p.display()))?;
                    }
                }
            }
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
    let locked = crate::lock_exec::load_saved(m, out, expected)?.is_some();
    let real = kind == "real" && locked;
    let mut table = json!({
        "schema": "celeris.campaign-table/1",
        "campaign_sha256": m.compute_sha256()?,
        "split": m.split,
        "evidence_kind": kind,
        "lock_verified": locked,
        "complete": complete,
        "incomplete_reasons": reasons,
        "success_contract": crate::success::SUCCESS_CONTRACT,
        "retry_selection": crate::success::RETRY_SELECTION,
        "denominator": crate::success::DENOMINATOR,
        "cost_scope": COST_SCOPE,
        "success_definition": "executor outcome passed AND independent grade final_pass (a failed grade keeps its failure_class: model_failure | env_failure | undetermined; the last two are unresolved until re-graded) (bound by campaign hash, case id, split, grader hash, artifact hash)",
        "real_performance": real && complete,
        "notice": if real { Value::Null } else {
            json!("mock/unknown/lock-less evidence: these numbers are not real performance and must not be reported as measurements") },
        "configs": rows,
        "fault_scenarios": m.fault_scenarios,
        "fault_observations": rc.fault_observations,
        "comparisons": rc.pairs,
    });
    if let Some(a) = accounting {
        let scope = |m: &str| {
            if a["total"]["complete"][m] == true {
                "complete"
            } else {
                "observed_partial"
            }
        };
        table["consumption"] = json!({
            "file": FINAL_ACCOUNTING_FILE,
            "scope": {"tokens": scope("tokens"), "list_cost_usd": scope("list_cost_usd"), "cash_cost_usd": scope("cash_cost_usd")},
            "reduction_confirmable": a["reduction_confirmable"],
            "gaps": a["gaps"].as_array().map_or(0, Vec::len),
        });
    }
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    if let Some(a) = accounting {
        let mut s = serde_json::to_string_pretty(a).map_err(|e| e.to_string())?;
        s.push('\n');
        fs::write(dest.join(FINAL_ACCOUNTING_FILE), s).map_err(|e| e.to_string())?;
    }
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
    let csv = csv_table(&rows, &kind, complete);
    fs::write(dest.join(format!("{stem}.csv")), csv).map_err(|e| e.to_string())?;
    Ok(table)
}
