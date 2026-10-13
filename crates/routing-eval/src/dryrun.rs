use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    candidates::{CANDIDATES, Candidate},
    manifest::Manifest,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MeasuredUsage {
    /// Number of measured runs used to derive the per-run quota percentages.
    pub runs: usize,
    /// Account name (`claude` or `codex`), then window name, then total percent.
    pub accounts: BTreeMap<String, BTreeMap<String, f64>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CandidateEstimate {
    pub source_id: String,
    pub account: String,
    pub harness_id: String,
    pub model_id: String,
    pub problems: usize,
    pub k: usize,
    pub calls: usize,
    pub list_usd: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountEstimate {
    pub account: String,
    pub calls: usize,
    pub quota_percent: BTreeMap<String, Option<f64>>,
    pub weekly_over_budget: Option<bool>,
    pub reductions: Vec<Reduction>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reduction {
    pub model_id: String,
    pub from_problems: usize,
    pub to_problems: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DryRunReport {
    pub schema: String,
    pub manifest_sha256: String,
    pub candidates: Vec<CandidateEstimate>,
    pub accounts: Vec<AccountEstimate>,
}

pub fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let manifest = required(&args, "--manifest")?;
    let db = required(&args, "--db")?;
    let measured = args
        .iter()
        .position(|a| a == "--measured")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);
    let out = args
        .iter()
        .position(|a| a == "--out")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);
    let m: Manifest = serde_json::from_slice(&fs::read(manifest).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let usage = measured
        .map(|p| {
            fs::read(p).map_err(|e| e.to_string()).and_then(|b| {
                serde_json::from_slice::<MeasuredUsage>(&b).map_err(|e| e.to_string())
            })
        })
        .transpose()?;
    if let Some(u) = &usage
        && u.runs != 10
    {
        return Err("--measured must contain exactly 10 measured runs".into());
    }
    let report = estimate(&m, Path::new(&db), usage.as_ref())?;
    let json = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    let markdown = to_markdown(&report);
    if let Some(path) = out {
        fs::write(path, format!("{json}\n")).map_err(|e| e.to_string())?;
    }
    println!("{markdown}\n\n```json\n{json}\n```");
    Ok(())
}

fn required(args: &[String], flag: &str) -> Result<String, String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
        .ok_or_else(|| format!("missing required option {flag}"))
}

pub fn estimate(
    manifest: &Manifest,
    db: &Path,
    measured: Option<&MeasuredUsage>,
) -> Result<DryRunReport, String> {
    let eligible = manifest
        .problems
        .iter()
        .filter(|p| p.split == "dev" || p.split == "held-out")
        .count();
    if eligible < 30 {
        return Err(format!("manifest has {eligible} problems; 30 required"));
    }
    let subset = manifest.problems.iter().filter(|p| p.subset_12).count();
    if subset < 12 {
        return Err(format!(
            "manifest has {subset} subset problems; 12 required"
        ));
    }
    let conn = open_ro(db)?;
    let measured_db = quota_rates(&conn)?;
    let mut estimates = Vec::new();
    for c in CANDIDATES {
        let problems = if c.problem_count == 12 { 12 } else { 30 };
        let calls = problems * c.k;
        estimates.push(CandidateEstimate {
            source_id: c.source_id.into(),
            account: c.account.into(),
            harness_id: c.harness_id.into(),
            model_id: c.model_id.into(),
            problems,
            k: c.k,
            calls,
            list_usd: list_cost(&conn, &c, problems, c.k)?,
        });
    }
    let mut accounts = Vec::new();
    for account in ["claude", "codex"] {
        let calls: usize = estimates
            .iter()
            .filter(|e| e.account == account)
            .map(|e| e.calls)
            .sum();
        let rates = measured
            .and_then(|u| u.accounts.get(account))
            .map(|v| {
                v.iter()
                    .map(|(k, total)| (k.clone(), total / 10.0))
                    .collect()
            })
            .or_else(|| measured_db.get(account).cloned());
        let mut quota_percent = BTreeMap::new();
        for window in ["5h", "7d", "monthly"] {
            quota_percent.insert(
                window.into(),
                rates
                    .as_ref()
                    .and_then(|r| r.get(window))
                    .map(|per_call| per_call * calls as f64),
            );
        }
        let weekly = quota_percent.get("7d").copied().flatten().map(|v| v > 15.0);
        let reductions = if weekly == Some(true) {
            shrink(
                account,
                calls,
                rates
                    .as_ref()
                    .and_then(|r| r.get("7d"))
                    .copied()
                    .unwrap_or(0.0),
                &estimates,
            )
        } else {
            vec![]
        };
        accounts.push(AccountEstimate {
            account: account.into(),
            calls,
            quota_percent,
            weekly_over_budget: weekly,
            reductions,
        });
    }
    Ok(DryRunReport {
        schema: "celeris.routing-eval-dry-run/1".into(),
        manifest_sha256: manifest.sha256.clone(),
        candidates: estimates,
        accounts,
    })
}

fn open_ro(db: &Path) -> Result<Connection, String> {
    let uri = format!("file:{}?mode=ro", db.display());
    Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| e.to_string())
}

// Only event rows explicitly marked method=measured contribute a quota rate.
fn quota_rates(conn: &Connection) -> Result<BTreeMap<String, BTreeMap<String, f64>>, String> {
    let mut result = BTreeMap::new();
    let mut stmt = conn.prepare("SELECT json FROM events WHERE json_extract(json,'$.type')='quota_estimated' ORDER BY seq DESC").map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    for row in rows {
        let raw = row.map_err(|e| e.to_string())?;
        let v: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        if v["method"].as_str() != Some("measured") {
            continue;
        }
        let account = v["account"].as_str().unwrap_or("");
        if account.is_empty() {
            continue;
        }
        let rates = result
            .entry(account.to_owned())
            .or_insert_with(BTreeMap::new);
        for window in v["windows"].as_array().into_iter().flatten() {
            let Some(pct) = window["per_run_pct"]
                .as_f64()
                .or_else(|| window["quota_per_run_pct"].as_f64())
            else {
                continue;
            };
            let key = normalize_window(window["window"].as_str().unwrap_or(""));
            if !key.is_empty() {
                rates.entry(key).or_insert(pct);
            }
        }
    }
    Ok(result)
}

fn normalize_window(s: &str) -> String {
    match s {
        "five_hour" | "5h" => "5h",
        "seven_day" | "7d" => "7d",
        "monthly" | "month" => "monthly",
        _ => "",
    }
    .into()
}

fn list_cost(
    conn: &Connection,
    candidate: &Candidate,
    problems: usize,
    k: usize,
) -> Result<Option<f64>, String> {
    let mut stmt = conn.prepare("SELECT model,usage_json FROM runs WHERE usage_json IS NOT NULL ORDER BY started_at DESC").map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?;
    let mut tokens = Vec::new();
    for row in rows {
        let (run_model, raw) = row.map_err(|e| e.to_string())?;
        let v: Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let model = run_model
            .as_deref()
            .or_else(|| v["model_id"].as_str())
            .or_else(|| v["model"].as_str());
        if model != Some(candidate.model_id) {
            continue;
        }
        let input = v["input_tokens"].as_f64().unwrap_or(0.0);
        let output = v["output_tokens"].as_f64().unwrap_or(0.0);
        if input + output > 0.0 {
            tokens.push((input, output));
        }
    }
    if tokens.is_empty() {
        return Ok(None);
    }
    tokens.sort_by(|a, b| (a.0 + a.1).total_cmp(&(b.0 + b.1)));
    let (input, output) = tokens[tokens.len() / 2];
    let (in_rate, out_rate) = match candidate.model_id {
        "claude-opus-5-5" => (4.0, 20.0),
        "claude-sonnet-5-5" => (2.0, 10.0),
        "claude-haiku-5-5" => (1.0, 5.0),
        "claude-fable-5-1" => (10.0, 50.0),
        _ => return Ok(None),
    };
    Ok(Some(
        ((input / 1_000_000.0) * in_rate + (output / 1_000_000.0) * out_rate)
            * (problems * k) as f64,
    ))
}

fn shrink(
    account: &str,
    total_calls: usize,
    pct_per_call: f64,
    candidates: &[CandidateEstimate],
) -> Vec<Reduction> {
    let allowance = (15.0 / pct_per_call).floor().max(0.0) as usize;
    if total_calls == 0 || allowance >= total_calls {
        return vec![];
    }
    let mut left = allowance;
    let mut reductions = Vec::new();
    for e in candidates.iter().filter(|e| e.account == account) {
        let full = e.calls;
        let keep = left.min(full);
        left -= keep;
        let problem_keep = (keep / e.k).min(e.problems);
        reductions.push(Reduction {
            model_id: e.model_id.clone(),
            from_problems: e.problems,
            to_problems: problem_keep,
        });
    }
    reductions
}

fn to_markdown(report: &DryRunReport) -> String {
    let mut out = format!(
        "# Evaluation dry-run\n\nManifest `{}`\n\n| Account | Harness | Model | Problems × k | Calls | List USD |\n|---|---|---|---:|---:|---:|\n",
        report.manifest_sha256
    );
    for e in &report.candidates {
        out.push_str(&format!(
            "| {} | {} | {} | {} × {} | {} | {} |\n",
            e.account,
            e.harness_id,
            e.model_id,
            e.problems,
            e.k,
            e.calls,
            e.list_usd
                .map(|v| format!("${v:.2}"))
                .unwrap_or_else(|| "未測定".into())
        ));
    }
    out.push_str(
        "\n| Account | Calls | 5h | 7d | monthly | 7d > 15% |\n|---|---:|---:|---:|---:|---|\n",
    );
    for a in &report.accounts {
        let val = |k: &str| {
            a.quota_percent
                .get(k)
                .copied()
                .flatten()
                .map(|x| format!("{x:.2}%"))
                .unwrap_or_else(|| "未測定".into())
        };
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            a.account,
            a.calls,
            val("5h"),
            val("7d"),
            val("monthly"),
            a.weekly_over_budget
                .map(|b| b.to_string())
                .unwrap_or_else(|| "未測定".into())
        ));
        if !a.reductions.is_empty() {
            out.push_str("\n縮小案:\n");
            for r in &a.reductions {
                out.push_str(&format!(
                    "- {}: {} 問 → {} 問\n",
                    r.model_id, r.from_problems, r.to_problems
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Problem;
    use tempfile::tempdir;

    fn manifest() -> Manifest {
        let problems = (0..30)
            .map(|i| Problem {
                id: format!("p{i}"),
                task_id: "t".into(),
                work_unit_id: format!("w{i}"),
                created_at: "2026-10-01".into(),
                genre: "coding".into(),
                kind: "test".into(),
                command: "true".into(),
                prompt: "Fixture problem description".into(),
                split: "dev".into(),
                stratum: "coding:test".into(),
                subset_12: i < 12,
                head_commit: None,
                base_commit: None,
                features_sha256: "x".into(),
                features: Value::Null,
                sha256: "x".into(),
            })
            .collect();
        Manifest {
            schema: crate::MANIFEST_SCHEMA.into(),
            version: "v1".into(),
            cutoff: "2026-10-05".into(),
            seed: 1,
            problems,
            sha256: "msha".into(),
        }
    }

    #[test]
    fn estimates_separate_accounts_and_leave_codex_quota_unmeasured() {
        let d = tempdir().expect("temp");
        let db = d.path().join("db");
        let c = Connection::open(&db).expect("db");
        c.execute_batch("CREATE TABLE events(seq INTEGER,json TEXT); CREATE TABLE runs(started_at TEXT,model TEXT,usage_json TEXT);").expect("schema");
        let r = estimate(&manifest(), &db, None).expect("estimate");
        assert_eq!(r.accounts[0].calls, 192);
        assert_eq!(r.accounts[1].calls, 132);
        assert_eq!(r.accounts[1].weekly_over_budget, None);
        assert!(r.accounts[1].quota_percent.values().all(Option::is_none));
    }

    #[test]
    fn measured_ten_run_quota_triggers_per_model_reduction_over_fifteen_percent() {
        let d = tempdir().expect("temp");
        let db = d.path().join("db");
        let c = Connection::open(&db).expect("db");
        c.execute_batch("CREATE TABLE events(seq INTEGER,json TEXT); CREATE TABLE runs(started_at TEXT,model TEXT,usage_json TEXT);").expect("schema");
        let usage = MeasuredUsage {
            runs: 10,
            accounts: BTreeMap::from([("claude".into(), BTreeMap::from([("7d".into(), 20.0)]))]),
        };
        let r = estimate(&manifest(), &db, Some(&usage)).expect("estimate");
        assert_eq!(r.accounts[0].weekly_over_budget, Some(true));
        assert_eq!(r.accounts[0].reductions.len(), 4);
        assert!(
            r.accounts[0]
                .reductions
                .iter()
                .any(|x| x.to_problems < x.from_problems)
        );
    }

    #[test]
    fn emits_json_and_markdown_with_unmeasured_label() {
        let d = tempdir().expect("temp");
        let db = d.path().join("db");
        let c = Connection::open(&db).expect("db");
        c.execute_batch("CREATE TABLE events(seq INTEGER,json TEXT); CREATE TABLE runs(started_at TEXT,model TEXT,usage_json TEXT);").expect("schema");
        let r = estimate(&manifest(), &db, None).expect("estimate");
        let json = serde_json::to_string(&r).expect("json");
        let md = to_markdown(&r);
        assert!(json.contains("gpt-6-astra"));
        assert!(md.contains("未測定"));
        assert!(md.contains("192"));
    }
}
