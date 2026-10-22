//! Read evaluation trials and produce versioned, interval-based evidence.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
};

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use task_core::routing_evidence::{EvidenceKey, EvidenceRow, harness_version_major};

use crate::{
    manifest::{Manifest, Problem},
    stats,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Trial {
    pub decision_id: String,
    pub problem_id: String,
    pub trial_no: u32,
    pub key: EvidenceKey,
    pub passed: bool,
    pub timed_out: bool,
    pub wall_secs: Option<f64>,
    pub usd_list: Option<f64>,
    pub quota: BTreeMap<String, f64>,
    pub status: String,
    /// `eval_trials.status`（評価 run の試行の行が無ければ None）。
    #[serde(default)]
    pub eval_status: Option<String>,
}

/// error・cancelled の試行は評価の仕組みの失敗で、問題を解けたかの証拠ではない。
/// 合否（母数・Wilson 区間・paired 比較）に数えず、除外数だけを出す。
pub fn excluded_status(trial: &Trial) -> Option<&str> {
    match trial.eval_status.as_deref() {
        Some(s @ ("error" | "cancelled")) => Some(s),
        _ => None,
    }
}

/// 根拠表の 1 cell で合否から除いた試行の数。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExcludedCount {
    pub error: u64,
    pub cancelled: u64,
}

impl ExcludedCount {
    pub fn total(&self) -> u64 {
        self.error + self.cancelled
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExcludedCell {
    pub key: EvidenceKey,
    #[serde(flatten)]
    pub count: ExcludedCount,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Aggregate {
    pub schema: String,
    pub manifest_sha256: String,
    pub evidence_version: String,
    pub rows: Vec<EvidenceRow>,
    pub pareto: Vec<String>,
    pub paired: Vec<Value>,
    /// cell ごとの除外数（error・cancelled）。除外が無い cell は載せない。
    pub excluded: Vec<ExcludedCell>,
}

impl Aggregate {
    pub fn excluded_total(&self) -> u64 {
        self.excluded.iter().map(|cell| cell.count.total()).sum()
    }

    pub fn excluded_for(&self, key: &EvidenceKey) -> ExcludedCount {
        self.excluded
            .iter()
            .find(|cell| &cell.key == key)
            .map(|cell| cell.count.clone())
            .unwrap_or_default()
    }
}

pub fn collect(db: &str, manifest: &Manifest) -> Result<Vec<Trial>, String> {
    let uri = format!("file:{}?mode=ro", db);
    let conn = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| e.to_string())?;
    let mut stmt = conn.prepare("SELECT d.decision_id,d.subject_id,d.input_json,d.output_json,o.labels_json,r.status,r.started_at,r.finished_at,r.usage_json,r.metrics_json,t.status FROM route_decisions d LEFT JOIN route_outcomes o ON o.decision_id=d.decision_id LEFT JOIN runs r ON r.run_id=d.run_id LEFT JOIN eval_trials t ON t.decision_id=d.decision_id WHERE d.stage='eval' AND d.subject_kind='eval_problem' ORDER BY d.subject_id,d.decision_id").map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<String>>(7)?,
                r.get::<_, Option<String>>(8)?,
                r.get::<_, Option<String>>(9)?,
                r.get::<_, Option<String>>(10)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let problems: BTreeMap<&str, &Problem> = manifest
        .problems
        .iter()
        .map(|p| (p.id.as_str(), p))
        .collect();
    let mut out = Vec::new();
    for row in rows {
        let (
            decision_id,
            problem_id,
            input,
            output,
            labels,
            status,
            started,
            finished,
            usage,
            metrics,
            eval_status,
        ) = row.map_err(|e| e.to_string())?;
        let Some(problem) = problems.get(problem_id.as_str()) else {
            continue;
        };
        let input: Value = serde_json::from_str(&input).map_err(|e| e.to_string())?;
        let selected: Value = serde_json::from_str(&output).map_err(|e| e.to_string())?;
        let picked = selected.get("selected").unwrap_or(&selected);
        let strval = |v: &Value, key: &str| -> String {
            v.get(key).and_then(Value::as_str).unwrap_or("").to_owned()
        };
        let harness = strval(picked, "harness_id");
        let version_raw = strval(picked, "harness_version");
        let source = strval(picked, "source_id");
        let model = strval(picked, "model_id");
        if harness.is_empty() || source.is_empty() || model.is_empty() {
            continue;
        }
        let labels: Value = labels
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| e.to_string())?
            .unwrap_or_else(|| json!({}));
        let usage: Value = usage
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| e.to_string())?
            .unwrap_or_else(|| json!({}));
        let metrics: Value = metrics
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| e.to_string())?
            .unwrap_or_else(|| json!({}));
        let field = |obj: &Value, key: &str| obj.get(key).and_then(Value::as_f64);
        let passed = labels
            .get("passed")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| status.as_deref() == Some("passed"));
        let timed_out = labels
            .get("timed_out")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let wall_secs = field(&labels, "wall_secs")
            .or_else(|| field(&metrics, "wall_secs"))
            .or_else(|| match (started.as_deref(), finished.as_deref()) {
                (Some(start), Some(finish)) => {
                    let format = time::format_description::well_known::Rfc3339;
                    let start = time::OffsetDateTime::parse(start, &format).ok()?;
                    let finish = time::OffsetDateTime::parse(finish, &format).ok()?;
                    Some((finish - start).as_seconds_f64().max(0.0))
                }
                _ => None,
            });
        let usd_list = field(&labels, "usd_list").or_else(|| field(&usage, "usd_list"));
        let quota = labels
            .get("quota")
            .or_else(|| usage.get("quota"))
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_f64().map(|n| (k.clone(), n)))
                    .collect()
            })
            .unwrap_or_default();
        out.push(Trial {
            decision_id,
            problem_id,
            trial_no: input.get("trial_no").and_then(Value::as_u64).unwrap_or(1) as u32,
            key: EvidenceKey {
                lane: String::new(),
                genre: problem.genre.clone(),
                harness_id: harness,
                harness_version: harness_version_major(&strval(picked, "harness_id"), &version_raw),
                source_id: source,
                model_id: model,
            },
            passed,
            timed_out,
            wall_secs,
            usd_list,
            quota,
            status: status.unwrap_or_else(|| "missing".into()),
            eval_status,
        });
    }
    Ok(out)
}

pub fn build(manifest: &Manifest, trials: &[Trial]) -> (Aggregate, Vec<Value>) {
    let mut groups: BTreeMap<EvidenceKey, Vec<&Trial>> = BTreeMap::new();
    let mut excluded: BTreeMap<EvidenceKey, ExcludedCount> = BTreeMap::new();
    let mut counted = Vec::new();
    for t in trials {
        match excluded_status(t) {
            Some("error") => excluded.entry(t.key.clone()).or_default().error += 1,
            Some(_) => excluded.entry(t.key.clone()).or_default().cancelled += 1,
            None => {
                groups.entry(t.key.clone()).or_default().push(t);
                counted.push(t.clone());
            }
        }
    }
    let version = format!(
        "{}-{}",
        manifest.sha256,
        time::OffsetDateTime::now_utc().date()
    );
    let mut rows = Vec::new();
    let mut detail = Vec::new();
    for (key, ts) in groups {
        let mut by_problem: BTreeMap<&str, Vec<&Trial>> = BTreeMap::new();
        for t in ts {
            by_problem.entry(&t.problem_id).or_default().push(t);
        }
        for repeated in by_problem.values_mut() {
            repeated.sort_by_key(|trial| trial.trial_no);
        }
        let outcomes: Vec<bool> = by_problem
            .values()
            .map(|v| v.first().is_some_and(|t| t.passed))
            .collect();
        let pass = outcomes.iter().filter(|&&p| p).count();
        let interval = stats::wilson_95(pass, outcomes.len());
        let bootstrap = stats::bootstrap_interval(&outcomes, 42, 2_000, 0.95);
        let values: Vec<&Trial> = by_problem
            .values()
            .flat_map(|v| v.iter().copied())
            .collect();
        let median = |mut v: Vec<f64>| -> Option<f64> {
            if v.is_empty() {
                return None;
            }
            v.sort_by(f64::total_cmp);
            Some(v[v.len() / 2])
        };
        let quotas: BTreeSet<String> = values
            .iter()
            .flat_map(|t| t.quota.keys().cloned())
            .collect();
        let quota_per_run = quotas
            .into_iter()
            .filter_map(|q| {
                let xs = values
                    .iter()
                    .filter_map(|t| t.quota.get(&q).copied())
                    .collect();
                median(xs).map(|x| (q, x))
            })
            .collect();
        let subset_problem_ids: BTreeSet<&str> = manifest
            .problems
            .iter()
            .filter(|problem| problem.subset_12)
            .map(|problem| problem.id.as_str())
            .collect();
        let low_confidence = by_problem.keys().any(|id| subset_problem_ids.contains(id))
            || by_problem.values().any(|v| v.len() < 2);
        let row = EvidenceRow {
            quota_cost_per_pass: None,
            key: key.clone(),
            evidence_version: version.clone(),
            n: outcomes.len() as u64,
            pass: pass as u64,
            pass_lower: interval.lower,
            pass_upper: interval.upper,
            pass_boot_lower: bootstrap.lower,
            pass_boot_upper: bootstrap.upper,
            wall_p90_s: percentile90(values.iter().filter_map(|t| t.wall_secs).collect()),
            usd_per_pass: median(values.iter().filter_map(|t| t.usd_list).collect()),
            quota_per_run,
            timeout_rate: values.iter().filter(|t| t.timed_out).count() as f64
                / values.len().max(1) as f64,
            low_confidence,
        };
        let excluded_here = excluded.get(&key).cloned().unwrap_or_default();
        detail.push(json!({"key":key,"excluded":excluded_here,"metrics":{"pass_rate":if row.n==0 {0.0}else{row.pass as f64/row.n as f64},"usd":row.usd_per_pass,"wall_seconds":row.wall_p90_s,"quota":row.quota_per_run},"evidence":row}));
        rows.push(row);
    }
    let metrics: Vec<stats::CandidateMetrics> = rows
        .iter()
        .map(|r| stats::CandidateMetrics {
            id: format!(
                "{}:{}:{}",
                r.key.source_id, r.key.model_id, r.key.harness_id
            ),
            pass: if r.n == 0 {
                0.0
            } else {
                r.pass as f64 / r.n as f64
            },
            usd: r.usd_per_pass.unwrap_or(f64::INFINITY),
            wall_seconds: r.wall_p90_s.unwrap_or(f64::INFINITY),
            quota: r.quota_per_run.values().copied().fold(0.0, f64::max),
        })
        .collect();
    let pareto = stats::pareto_frontier(&metrics);
    let paired = paired_comparisons(manifest, &counted);
    let excluded = excluded
        .into_iter()
        .map(|(key, count)| ExcludedCell { key, count })
        .collect();
    (
        Aggregate {
            schema: "celeris.routing-evidence/1".into(),
            manifest_sha256: manifest.sha256.clone(),
            evidence_version: version,
            rows,
            pareto,
            paired,
            excluded,
        },
        detail,
    )
}

/// 経過時間の p90（nearest-rank）。評価 trial のレポート用で、根拠表（本番の完了単位）には書かない。
fn percentile90(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let rank = ((values.len() as f64) * 0.9).ceil() as usize;
    values.get(rank.clamp(1, values.len()) - 1).copied()
}

fn paired_comparisons(manifest: &Manifest, trials: &[Trial]) -> Vec<Value> {
    let mut candidates: BTreeMap<(String, String, String), BTreeMap<String, bool>> =
        BTreeMap::new();
    for t in trials {
        candidates
            .entry((
                t.key.genre.clone(),
                t.key.harness_id.clone(),
                format!(
                    "{}:{}:{}",
                    t.key.source_id, t.key.model_id, t.key.harness_id
                ),
            ))
            .or_default()
            .entry(t.problem_id.clone())
            .or_insert(t.passed);
    }
    let entries: Vec<_> = candidates.into_iter().collect();
    let mut out = Vec::new();
    for i in 0..entries.len() {
        for j in i + 1..entries.len() {
            if entries[i].0.0 != entries[j].0.0 || entries[i].0.1 != entries[j].0.1 {
                continue;
            }
            let ids: Vec<_> = entries[i]
                .1
                .keys()
                .filter(|id| {
                    entries[j].1.contains_key(*id)
                        && manifest
                            .problems
                            .iter()
                            .any(|p| p.id == **id && p.split == "dev")
                })
                .cloned()
                .collect();
            let a: Vec<_> = ids.iter().map(|id| entries[i].1[id]).collect();
            let b: Vec<_> = ids.iter().map(|id| entries[j].1[id]).collect();
            if let (Ok(p), Ok(diff)) = (
                stats::mcnemar_exact(&a, &b),
                stats::paired_difference_interval(&a, &b, 0xA661, 2000, 0.95),
            ) {
                out.push(json!({"genre":entries[i].0.0,"harness_id":entries[i].0.1,"left":entries[i].0.2,"right":entries[j].0.2,"n":ids.len(),"mcnemar_p":p,"difference_left_minus_right":diff}));
            }
        }
    }
    out
}

pub fn run() -> Result<(), String> {
    run_args(std::env::args().skip(2).collect())
}

pub fn run_args(args: Vec<String>) -> Result<(), String> {
    let value = |flag: &str| -> Result<String, String> {
        args.iter()
            .position(|x| x == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
            .ok_or_else(|| format!("missing required option {flag}"))
    };
    let db = value("--db")?;
    let manifest_path = PathBuf::from(value("--manifest")?);
    let out = PathBuf::from(value("--out")?);
    fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let manifest: Manifest =
        serde_json::from_slice(&fs::read(manifest_path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let trials = collect(&db, &manifest)?;
    let (result, detail) = build(&manifest, &trials);
    fs::write(
        out.join("evidence.json"),
        serde_json::to_vec_pretty(&result).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let md = format!(
        "# Routing evaluation evidence\n\nManifest: `{}`  \nEvidence version: `{}`\n\n| Genre | Harness | Version | Source | Model | n | Pass | Wilson 95% | Bootstrap 95% | Timeout | USD median | Wall median (s) | Low confidence | Excluded (error/cancelled) |\n|---|---|---|---|---|---:|---:|---|---|---:|---:|---:|---|---:|\n{}\n\nExcluded trials (error/cancelled; not counted in n, Pass or intervals): {}\n\n{}\n\nPareto set: {}\n\nPaired comparisons: {}\n",
        result.manifest_sha256,
        result.evidence_version,
        result
            .rows
            .iter()
            .map(|r| format!(
                "| {} | {} | {} | {} | {} | {} | {} | {:.3}–{:.3} | {:.3}–{:.3} | {:.3} | {} | {} | {} | {}/{} |",
                r.key.genre,
                r.key.harness_id,
                r.key.harness_version,
                r.key.source_id,
                r.key.model_id,
                r.n,
                r.pass,
                r.pass_lower,
                r.pass_upper,
                r.pass_boot_lower,
                r.pass_boot_upper,
                r.timeout_rate,
                r.usd_per_pass.map_or("—".into(), |v| format!("{v:.4}")),
                r.wall_p90_s.map_or("—".into(), |v| format!("{v:.2}")),
                r.low_confidence,
                result.excluded_for(&r.key).error,
                result.excluded_for(&r.key).cancelled
            ))
            .collect::<Vec<_>>()
            .join("\n"),
        result.excluded_total(),
        excluded_table(&result.excluded),
        result.pareto.join(", "),
        serde_json::to_string(&result.paired).map_err(|e| e.to_string())?
    );
    fs::write(out.join("evidence.md"), md).map_err(|e| e.to_string())?;
    fs::write(
        out.join("trials.json"),
        serde_json::to_vec_pretty(&detail).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    println!(
        "trials={} excluded={} cells={} evidence_version={}",
        trials.len(),
        result.excluded_total(),
        result.rows.len(),
        result.evidence_version
    );
    Ok(())
}

/// 除外のあった cell の表（合否の行が無い、全試行が除外された cell も載る）。
fn excluded_table(excluded: &[ExcludedCell]) -> String {
    if excluded.is_empty() {
        return "No excluded trials.".into();
    }
    let rows = excluded
        .iter()
        .map(|cell| {
            format!(
                "| {} | {} | {} | {} | {} | {} | {} |",
                cell.key.genre,
                cell.key.harness_id,
                cell.key.harness_version,
                cell.key.source_id,
                cell.key.model_id,
                cell.count.error,
                cell.count.cancelled
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "| Genre | Harness | Version | Source | Model | Error | Cancelled |\n|---|---|---|---|---|---:|---:|\n{rows}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paired_comparisons_never_cross_harnesses() {
        let problem = Problem {
            id: "p".into(),
            task_id: "t".into(),
            work_unit_id: "w".into(),
            created_at: "".into(),
            genre: "coding".into(),
            kind: "task".into(),
            command: "check".into(),
            prompt: "Fixture problem description".into(),
            split: "dev".into(),
            stratum: "coding:task".into(),
            subset_12: true,
            head_commit: None,
            base_commit: None,
            base_commit_from_parent: false,
            features_sha256: "".into(),
            features: json!({}),
            sha256: "".into(),
        };
        let manifest = Manifest {
            schema: "x".into(),
            version: "1".into(),
            cutoff: "".into(),
            seed: 0,
            problems: vec![problem],
            refill_rejected: Vec::new(),
            sha256: "x".into(),
        };
        let make_trial = |harness: &str, source: &str| Trial {
            decision_id: source.into(),
            problem_id: "p".into(),
            trial_no: 1,
            key: EvidenceKey {
                lane: String::new(),
                genre: "coding".into(),
                harness_id: harness.into(),
                harness_version: "1".into(),
                source_id: source.into(),
                model_id: "m".into(),
            },
            passed: true,
            timed_out: false,
            wall_secs: None,
            usd_list: None,
            quota: BTreeMap::new(),
            status: "passed".into(),
            eval_status: None,
        };
        let comparisons = paired_comparisons(
            &manifest,
            &[
                make_trial("claude_code", "claude_oauth"),
                make_trial("codex", "codex_oauth"),
            ],
        );
        assert!(comparisons.is_empty());
    }

    #[test]
    fn aggregate_emits_intervals_metrics_pareto_and_k1_low_confidence() {
        let p = Problem {
            id: "p1".into(),
            task_id: "t".into(),
            work_unit_id: "w".into(),
            created_at: "".into(),
            genre: "coding".into(),
            kind: "task".into(),
            command: "check".into(),
            prompt: "Fixture problem description".into(),
            split: "dev".into(),
            stratum: "coding:task".into(),
            subset_12: true,
            head_commit: None,
            base_commit: None,
            base_commit_from_parent: false,
            features_sha256: "".into(),
            features: json!({}),
            sha256: "".into(),
        };
        let m = Manifest {
            schema: "manifest".into(),
            version: "1".into(),
            cutoff: "".into(),
            seed: 0,
            problems: vec![p],
            refill_rejected: Vec::new(),
            sha256: "manifest-hash".into(),
        };
        let trial = Trial {
            decision_id: "d".into(),
            problem_id: "p1".into(),
            trial_no: 1,
            key: EvidenceKey {
                lane: String::new(),
                genre: "coding".into(),
                harness_id: "claude_code".into(),
                harness_version: "1".into(),
                source_id: "claude_oauth".into(),
                model_id: "m".into(),
            },
            passed: true,
            timed_out: false,
            wall_secs: Some(3.0),
            usd_list: Some(0.4),
            quota: BTreeMap::from([("7d".into(), 0.02)]),
            status: "passed".into(),
            eval_status: None,
        };
        let (agg, detail) = build(&m, &[trial]);
        assert_eq!(agg.rows.len(), 1);
        let row = &agg.rows[0];
        assert_eq!(row.n, 1);
        assert_eq!(row.pass, 1);
        assert!(row.pass_lower < 1.0 && row.pass_upper == 1.0);
        assert_eq!((row.pass_boot_lower, row.pass_boot_upper), (1.0, 1.0));
        assert!(row.low_confidence);
        assert_eq!(row.usd_per_pass, Some(0.4));
        assert_eq!(row.quota_per_run["7d"], 0.02);
        assert_eq!(detail.len(), 1);
        assert_eq!(agg.pareto.len(), 1);
    }

    #[test]
    fn collect_reads_eval_decision_outcome_and_run_from_fixture_db() {
        let dir = tempfile::tempdir().expect("temp dir");
        let db = dir.path().join("eval.sqlite");
        let conn = Connection::open(&db).expect("db");
        conn.execute_batch("CREATE TABLE route_decisions(decision_id TEXT,subject_id TEXT,subject_kind TEXT,run_id TEXT,input_json TEXT,output_json TEXT,stage TEXT); CREATE TABLE route_outcomes(decision_id TEXT,labels_json TEXT); CREATE TABLE runs(run_id TEXT,status TEXT,started_at TEXT,finished_at TEXT,usage_json TEXT,metrics_json TEXT); CREATE TABLE eval_trials(decision_id TEXT,status TEXT);")
            .expect("fixture schema");
        conn.execute("INSERT INTO route_decisions VALUES('d','p1','eval_problem','r1',?1,?2,'eval')", rusqlite::params![r#"{"manifest_sha":"mh","trial_no":2}"#,r#"{"selected":{"source_id":"s","model_id":"m","harness_id":"codex","harness_version":"v2.3.1"}}"#]).expect("decision");
        conn.execute("INSERT INTO route_outcomes VALUES('d',?1)", [r#"{"passed":true,"timed_out":false,"wall_secs":4.0,"usd_list":0.3,"quota":{"7d":0.01}}"#]).expect("outcome");
        conn.execute("INSERT INTO runs VALUES('r1','passed','2026-10-01T00:00:00Z','2026-10-01T00:00:05Z',NULL,NULL)", []).expect("run");
        drop(conn);
        let problem = Problem {
            id: "p1".into(),
            task_id: "t".into(),
            work_unit_id: "w".into(),
            created_at: "".into(),
            genre: "coding".into(),
            kind: "task".into(),
            command: "check".into(),
            prompt: "Fixture problem description".into(),
            split: "dev".into(),
            stratum: "coding:task".into(),
            subset_12: false,
            head_commit: None,
            base_commit: None,
            base_commit_from_parent: false,
            features_sha256: "".into(),
            features: json!({}),
            sha256: "".into(),
        };
        let manifest = Manifest {
            schema: "manifest".into(),
            version: "1".into(),
            cutoff: "".into(),
            seed: 0,
            problems: vec![problem],
            refill_rejected: Vec::new(),
            sha256: "mh".into(),
        };
        let rows = collect(db.to_str().expect("path"), &manifest).expect("collect");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].trial_no, 2);
        assert_eq!(rows[0].key.harness_version, "2");
        assert!(rows[0].passed);
        assert_eq!(rows[0].wall_secs, Some(4.0));
        assert_eq!(rows[0].quota["7d"], 0.01);
    }

    #[test]
    fn aggregate_excludes_error_trials() {
        let problem = |id: &str| Problem {
            id: id.into(),
            task_id: "t".into(),
            work_unit_id: "w".into(),
            created_at: "".into(),
            genre: "coding".into(),
            kind: "task".into(),
            command: "check".into(),
            prompt: "Fixture problem description".into(),
            split: "dev".into(),
            stratum: "coding:task".into(),
            subset_12: false,
            head_commit: None,
            base_commit: None,
            base_commit_from_parent: false,
            features_sha256: "".into(),
            features: json!({}),
            sha256: "".into(),
        };
        let manifest = Manifest {
            schema: "manifest".into(),
            version: "1".into(),
            cutoff: "".into(),
            seed: 0,
            problems: ["p1", "p2", "p3", "p4"].map(problem).to_vec(),
            refill_rejected: Vec::new(),
            sha256: "mh".into(),
        };
        // eval_trials を持つ実 DB の形で collect から通す: done（passed / failed）2 件、
        // error 2 件（route_outcomes なし・runs は error）、cancelled 1 件。
        let dir = tempfile::tempdir().expect("temp dir");
        let db = dir.path().join("eval.sqlite");
        let conn = Connection::open(&db).expect("db");
        conn.execute_batch("CREATE TABLE route_decisions(decision_id TEXT,subject_id TEXT,subject_kind TEXT,run_id TEXT,input_json TEXT,output_json TEXT,stage TEXT); CREATE TABLE route_outcomes(decision_id TEXT,labels_json TEXT); CREATE TABLE runs(run_id TEXT,status TEXT,started_at TEXT,finished_at TEXT,usage_json TEXT,metrics_json TEXT); CREATE TABLE eval_trials(trial_id TEXT,decision_id TEXT,status TEXT,error TEXT);")
            .expect("fixture schema");
        let selected = r#"{"selected":{"source_id":"codex_oauth","model_id":"m","harness_id":"codex","harness_version":"1.0"}}"#;
        let trials = [
            ("d1", "p1", "passed", Some(true)),
            ("d2", "p2", "failed", Some(false)),
            ("d3", "p3", "error", None),
            ("d4", "p4", "error", None),
            ("d5", "p4", "cancelled", None),
        ];
        for (n, (decision, problem_id, status, passed)) in trials.iter().enumerate() {
            let run = format!("r{n}");
            conn.execute(
                "INSERT INTO route_decisions VALUES(?1,?2,'eval_problem',?3,?4,?5,'eval')",
                rusqlite::params![
                    decision,
                    problem_id,
                    run,
                    json!({"trial_no": n + 1}).to_string(),
                    selected
                ],
            )
            .expect("decision");
            let run_status = if passed.is_some() {
                "completed"
            } else {
                "error"
            };
            conn.execute(
                "INSERT INTO runs VALUES(?1,?2,NULL,NULL,NULL,NULL)",
                rusqlite::params![run, run_status],
            )
            .expect("run");
            if let Some(passed) = passed {
                conn.execute(
                    "INSERT INTO route_outcomes VALUES(?1,?2)",
                    rusqlite::params![decision, json!({"passed": passed}).to_string()],
                )
                .expect("outcome");
            }
            conn.execute(
                "INSERT INTO eval_trials VALUES(?1,?2,?3,?4)",
                rusqlite::params![
                    format!("t{n}"),
                    decision,
                    status,
                    (passed.is_none()).then_some("run ended as error")
                ],
            )
            .expect("trial");
        }
        drop(conn);

        let collected = collect(db.to_str().expect("path"), &manifest).expect("collect");
        // 古い error 行は消さずに読む（除外は集計の側で行う）。
        assert_eq!(collected.len(), 5);
        let (agg, detail) = build(&manifest, &collected);
        assert_eq!(agg.rows.len(), 1);
        let row = &agg.rows[0];
        // 母数と Wilson 区間は done の 2 件（1 合格・1 不合格）だけから出る。
        assert_eq!((row.n, row.pass), (2, 1));
        let done_only = stats::wilson_95(1, 2);
        assert_eq!(
            (row.pass_lower, row.pass_upper),
            (done_only.lower, done_only.upper)
        );
        let with_errors = stats::wilson_95(1, 4);
        assert_ne!(row.pass_lower, with_errors.lower);
        assert_eq!(
            agg.excluded_for(&row.key),
            ExcludedCount {
                error: 2,
                cancelled: 1
            }
        );
        assert_eq!(detail[0]["excluded"]["error"], 2);
        assert_eq!(detail[0]["excluded"]["cancelled"], 1);

        // 根拠表（evidence.md / evidence.json）に除外数が出る。
        let manifest_path = dir.path().join("manifest.json");
        fs::write(
            &manifest_path,
            serde_json::to_vec(&manifest).expect("manifest json"),
        )
        .expect("manifest");
        let out = dir.path().join("out");
        run_args(vec![
            "--db".into(),
            db.display().to_string(),
            "--manifest".into(),
            manifest_path.display().to_string(),
            "--out".into(),
            out.display().to_string(),
        ])
        .expect("aggregate");
        let md = fs::read_to_string(out.join("evidence.md")).expect("evidence.md");
        assert!(md.contains("| Excluded (error/cancelled) |"), "{md}");
        assert!(
            md.contains("| 2 | 1 | 0.") && md.contains("| 2/1 |"),
            "{md}"
        );
        assert!(
            md.contains(
                "Excluded trials (error/cancelled; not counted in n, Pass or intervals): 3"
            ),
            "{md}"
        );
        assert!(
            md.lines()
                .any(|line| line == "| coding | codex | 1.0 | codex_oauth | m | 2 | 1 |"),
            "{md}"
        );
        let evidence: Value =
            serde_json::from_slice(&fs::read(out.join("evidence.json")).expect("evidence.json"))
                .expect("evidence json");
        assert_eq!(evidence["excluded"][0]["error"], 2);
        assert_eq!(evidence["excluded"][0]["cancelled"], 1);
        assert_eq!(evidence["rows"][0]["n"], 2);
    }
}
