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
    /// Window -> quota percent for all calls of this candidate (measured per-run mean × calls).
    pub quota_percent: BTreeMap<String, Option<f64>>,
    /// Window -> number of measured production runs behind the per-run mean.
    pub quota_samples: BTreeMap<String, usize>,
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
        let rates = measured_db.get(&(c.account.to_owned(), c.model_id.to_owned()));
        let mut quota_percent = BTreeMap::new();
        let mut quota_samples = BTreeMap::new();
        for window in WINDOWS {
            let rate = rates.and_then(|r| r.get(window));
            quota_percent.insert(window.into(), rate.map(|r| r.mean * calls as f64));
            quota_samples.insert(window.into(), rate.map_or(0, |r| r.samples));
        }
        estimates.push(CandidateEstimate {
            source_id: c.source_id.into(),
            account: c.account.into(),
            harness_id: c.harness_id.into(),
            model_id: c.model_id.into(),
            problems,
            k: c.k,
            calls,
            list_usd: list_cost(&conn, &c, problems, c.k)?,
            quota_percent,
            quota_samples,
        });
    }
    let mut accounts = Vec::new();
    for account in ["claude", "codex"] {
        let mine: Vec<&CandidateEstimate> =
            estimates.iter().filter(|e| e.account == account).collect();
        let calls: usize = mine.iter().map(|e| e.calls).sum();
        let override_rates = measured.and_then(|u| u.accounts.get(account));
        let mut quota_percent = BTreeMap::new();
        for window in WINDOWS {
            let total = match override_rates {
                // A 10-run measurement gives one per-call rate for the whole account.
                Some(r) => r.get(window).map(|total| total / 10.0 * calls as f64),
                // Production measurements are per model; every model needs one.
                None => mine
                    .iter()
                    .map(|e| e.quota_percent.get(window).copied().flatten())
                    .sum::<Option<f64>>(),
            };
            quota_percent.insert(window.into(), total);
        }
        let weekly_total = quota_percent.get("7d").copied().flatten();
        let weekly = weekly_total.map(|v| v > 15.0);
        let reductions = match weekly_total {
            Some(total) if total > 15.0 && calls > 0 => {
                shrink(account, calls, total / calls as f64, &estimates)
            }
            _ => vec![],
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

const WINDOWS: [&str; 3] = ["5h", "7d", "monthly"];

/// Measured production runs averaged per (account, model, window).
const QUOTA_SAMPLE_LIMIT: usize = 100;

#[derive(Clone, Copy, Debug, Default)]
struct QuotaRate {
    mean: f64,
    samples: usize,
}

/// `celeris.routing-eval` account names by the source that wrote the quota event.
fn eval_account(source: &str) -> Option<&'static str> {
    match source {
        "claude-oauth" => Some("claude"),
        "codex-oauth" => Some("codex"),
        _ => None,
    }
}

// Only windows explicitly marked method=measured contribute. Each event is one run's
// before/after difference (`used_pct`); the rate is the mean over the latest
// QUOTA_SAMPLE_LIMIT runs of the same model on the same source.
type RateKey = (String, String);

fn quota_rates(
    conn: &Connection,
) -> Result<BTreeMap<RateKey, BTreeMap<String, QuotaRate>>, String> {
    let mut sums: BTreeMap<RateKey, BTreeMap<String, (f64, usize)>> = BTreeMap::new();
    let mut stmt = conn
        .prepare(
            "SELECT e.json, r.model FROM events e \
             LEFT JOIN runs r ON r.run_id = json_extract(e.json,'$.run_id') \
             WHERE json_extract(e.json,'$.type')='quota_estimated' ORDER BY e.id DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (raw, model) = row.map_err(|e| e.to_string())?;
        let Some(model) = model else { continue };
        let v: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        let Some(account) = v["source"].as_str().and_then(eval_account) else {
            continue;
        };
        let windows = sums.entry((account.to_owned(), model)).or_default();
        for window in v["windows"].as_array().into_iter().flatten() {
            if window["method"].as_str() != Some("measured") {
                continue;
            }
            let Some(pct) = window["used_pct"].as_f64() else {
                continue;
            };
            let key = normalize_window(window["window"].as_str().unwrap_or(""));
            if key.is_empty() {
                continue;
            }
            let slot = windows.entry(key).or_insert((0.0, 0));
            if slot.1 < QUOTA_SAMPLE_LIMIT {
                slot.0 += pct;
                slot.1 += 1;
            }
        }
    }
    Ok(sums
        .into_iter()
        .map(|(k, windows)| {
            let rates = windows
                .into_iter()
                .filter(|(_, (_, n))| *n > 0)
                .map(|(w, (sum, n))| {
                    (
                        w,
                        QuotaRate {
                            mean: sum / n as f64,
                            samples: n,
                        },
                    )
                })
                .collect();
            (k, rates)
        })
        .collect())
}

fn normalize_window(s: &str) -> String {
    match s {
        "five_hour" | "5h" => "5h",
        "seven_day" | "7d" => "7d",
        "monthly" | "month" | "one_month" => "monthly",
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
    let Some((in_rate, out_rate)) = list_rates(candidate.model_id) else {
        return Ok(None);
    };
    let mut per_run = Vec::new();
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
        // The harness-reported list price covers cache reads and writes; the token
        // fallback prices them as 0.1× (read) and 1.25× (write) of the input rate.
        let usd = match v["cost_usd"].as_f64() {
            Some(usd) => usd,
            None => {
                let tokens = |k: &str| v[k].as_f64().unwrap_or(0.0) / 1_000_000.0;
                tokens("input_tokens") * in_rate
                    + tokens("output_tokens") * out_rate
                    + tokens("cache_read_tokens") * in_rate * 0.1
                    + tokens("cache_creation_tokens") * in_rate * 1.25
            }
        };
        if usd > 0.0 {
            per_run.push(usd);
        }
    }
    if per_run.is_empty() {
        return Ok(None);
    }
    per_run.sort_by(f64::total_cmp);
    Ok(Some(per_run[per_run.len() / 2] * (problems * k) as f64))
}

/// List prices (USD per million input / output tokens). Codex models have none.
fn list_rates(model_id: &str) -> Option<(f64, f64)> {
    match model_id {
        "claude-opus-5-5" => Some((4.0, 20.0)),
        "claude-sonnet-5-5" => Some((2.0, 10.0)),
        "claude-haiku-5-5" => Some((1.0, 5.0)),
        "claude-fable-5-1" => Some((10.0, 50.0)),
        _ => None,
    }
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
        "# Evaluation dry-run\n\nManifest `{}`\n\n| Account | Harness | Model | Problems × k | Calls | List USD | 5h | 7d | measured runs (5h/7d) |\n|---|---|---|---:|---:|---:|---:|---:|---:|\n",
        report.manifest_sha256
    );
    for e in &report.candidates {
        let pct = |k: &str| {
            e.quota_percent
                .get(k)
                .copied()
                .flatten()
                .map(|x| format!("{x:.2}%"))
                .unwrap_or_else(|| "未測定".into())
        };
        let n = |k: &str| e.quota_samples.get(k).copied().unwrap_or(0);
        out.push_str(&format!(
            "| {} | {} | {} | {} × {} | {} | {} | {} | {} | {}/{} |\n",
            e.account,
            e.harness_id,
            e.model_id,
            e.problems,
            e.k,
            e.calls,
            e.list_usd
                .map(|v| format!("${v:.2}"))
                .unwrap_or_else(|| "未測定".into()),
            pct("5h"),
            pct("7d"),
            n("5h"),
            n("7d")
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
                base_commit_from_parent: false,
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
            refill_rejected: Vec::new(),
            sha256: "msha".into(),
        }
    }

    #[test]
    fn estimates_separate_accounts_and_leave_codex_quota_unmeasured() {
        let d = tempdir().expect("temp");
        let db = d.path().join("db");
        let c = Connection::open(&db).expect("db");
        c.execute_batch("CREATE TABLE events(id INTEGER PRIMARY KEY,json TEXT); CREATE TABLE runs(run_id TEXT,started_at TEXT,model TEXT,usage_json TEXT);").expect("schema");
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
        c.execute_batch("CREATE TABLE events(id INTEGER PRIMARY KEY,json TEXT); CREATE TABLE runs(run_id TEXT,started_at TEXT,model TEXT,usage_json TEXT);").expect("schema");
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
        c.execute_batch("CREATE TABLE events(id INTEGER PRIMARY KEY,json TEXT); CREATE TABLE runs(run_id TEXT,started_at TEXT,model TEXT,usage_json TEXT);").expect("schema");
        let r = estimate(&manifest(), &db, None).expect("estimate");
        let json = serde_json::to_string(&r).expect("json");
        let md = to_markdown(&r);
        assert!(json.contains("gpt-6-astra"));
        assert!(md.contains("未測定"));
        assert!(md.contains("192"));
    }

    #[test]
    fn production_measured_quota_is_averaged_per_model_and_mapped_from_source() {
        let d = tempdir().expect("temp");
        let db = d.path().join("db");
        let c = Connection::open(&db).expect("db");
        c.execute_batch("CREATE TABLE events(id INTEGER PRIMARY KEY,json TEXT); CREATE TABLE runs(run_id TEXT,started_at TEXT,model TEXT,usage_json TEXT);").expect("schema");
        let models = [
            "claude-opus-5-5",
            "claude-sonnet-5-5",
            "claude-haiku-5-5",
            "claude-fable-5-1",
        ];
        for (i, model) in models.iter().enumerate() {
            for (j, pct) in [0.0, 1.0].iter().enumerate() {
                let run = format!("r{i}{j}");
                c.execute(
                    "INSERT INTO runs(run_id,started_at,model) VALUES (?1,'t',?2)",
                    [&run, *model],
                )
                .expect("run");
                let ev = serde_json::json!({
                    "type": "quota_estimated", "run_id": run, "source": "claude-oauth",
                    "account": "claude_max_personal", "method": "measured",
                    "windows": [
                        {"window": "seven_day", "used_pct": pct, "method": "measured"},
                        {"window": "five_hour", "used_pct": 50.0, "method": "unknown"}
                    ]
                });
                c.execute("INSERT INTO events(json) VALUES (?1)", [ev.to_string()])
                    .expect("event");
            }
        }
        // A codex event without a measured window stays unmeasured.
        c.execute(
            "INSERT INTO runs(run_id,started_at,model) VALUES ('x','t','gpt-6-luna')",
            [],
        )
        .expect("run");
        c.execute(
            "INSERT INTO events(json) VALUES (?1)",
            [serde_json::json!({"type":"quota_estimated","run_id":"x","source":"codex-oauth","account":"chatgpt_plus_personal","method":"unknown","windows":[{"window":"seven_day","method":"unknown"}]}).to_string()],
        )
        .expect("event");
        let r = estimate(&manifest(), &db, None).expect("estimate");
        let opus = &r.candidates[0];
        assert_eq!(opus.quota_percent["7d"], Some(30.0));
        assert_eq!(opus.quota_samples["7d"], 2);
        assert_eq!(opus.quota_percent["5h"], None);
        // 192 calls × 0.5% per run.
        assert_eq!(r.accounts[0].quota_percent["7d"], Some(96.0));
        assert_eq!(r.accounts[0].weekly_over_budget, Some(true));
        assert!(!r.accounts[0].reductions.is_empty());
        assert_eq!(r.accounts[1].quota_percent["7d"], None);
    }

    #[test]
    fn list_cost_prefers_reported_cost_and_prices_cache_tokens() {
        let d = tempdir().expect("temp");
        let db = d.path().join("db");
        let c = Connection::open(&db).expect("db");
        c.execute_batch("CREATE TABLE events(id INTEGER PRIMARY KEY,json TEXT); CREATE TABLE runs(run_id TEXT,started_at TEXT,model TEXT,usage_json TEXT);").expect("schema");
        c.execute(
            "INSERT INTO runs VALUES ('a','1','claude-opus-5-5',?1)",
            [r#"{"input_tokens":10,"output_tokens":10,"cost_usd":0.5}"#],
        )
        .expect("run");
        c.execute(
            "INSERT INTO runs VALUES ('b','2','claude-haiku-5-5',?1)",
            [r#"{"input_tokens":0,"output_tokens":0,"cache_read_tokens":10000000,"cache_creation_tokens":0}"#],
        )
        .expect("run");
        let r = estimate(&manifest(), &db, None).expect("estimate");
        assert_eq!(r.candidates[0].list_usd, Some(30.0));
        // 10M cache reads × $1/M × 0.1 = $1 per run, 60 calls.
        let haiku = r.candidates[2].list_usd.expect("haiku usd");
        assert!((haiku - 60.0).abs() < 1e-9);
        assert_eq!(r.candidates[4].list_usd, None);
    }
}
