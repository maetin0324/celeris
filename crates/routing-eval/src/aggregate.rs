//! Read evaluation trials and produce versioned, interval-based evidence.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
};

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use task_core::{
    routing_evidence::{EvidenceKey, EvidenceRow, harness_version_major},
    store::SqliteStore,
};

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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Aggregate {
    pub schema: String,
    pub manifest_sha256: String,
    pub evidence_version: String,
    pub rows: Vec<EvidenceRow>,
    pub pareto: Vec<String>,
    pub paired: Vec<Value>,
}

pub fn collect(db: &str, manifest: &Manifest) -> Result<Vec<Trial>, String> {
    let uri = format!("file:{}?mode=ro", db);
    let conn = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| e.to_string())?;
    let mut stmt = conn.prepare("SELECT d.decision_id,d.subject_id,d.input_json,d.output_json,o.labels_json,r.status,r.started_at,r.finished_at,r.usage_json,r.metrics_json FROM route_decisions d LEFT JOIN route_outcomes o ON o.decision_id=d.decision_id LEFT JOIN runs r ON r.run_id=d.run_id WHERE d.stage='eval' AND d.subject_kind='eval_problem' ORDER BY d.subject_id,d.decision_id").map_err(|e| e.to_string())?;
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
        });
    }
    Ok(out)
}

pub fn build(manifest: &Manifest, trials: &[Trial]) -> (Aggregate, Vec<Value>) {
    let mut groups: BTreeMap<EvidenceKey, Vec<&Trial>> = BTreeMap::new();
    for t in trials {
        groups.entry(t.key.clone()).or_default().push(t);
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
            key: key.clone(),
            evidence_version: version.clone(),
            n: outcomes.len() as u32,
            pass: pass as u32,
            pass_lower: interval.lower,
            pass_upper: interval.upper,
            wall_median_s: median(values.iter().filter_map(|t| t.wall_secs).collect()),
            usd_list_median: median(values.iter().filter_map(|t| t.usd_list).collect()),
            quota_per_run,
            timeout_rate: values.iter().filter(|t| t.timed_out).count() as f64
                / values.len().max(1) as f64,
            low_confidence,
        };
        detail.push(json!({"key":key,"metrics":{"pass_rate":if row.n==0 {0.0}else{row.pass as f64/row.n as f64},"usd":row.usd_list_median,"wall_seconds":row.wall_median_s,"quota":row.quota_per_run},"evidence":row}));
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
            usd: r.usd_list_median.unwrap_or(f64::INFINITY),
            wall_seconds: r.wall_median_s.unwrap_or(f64::INFINITY),
            quota: r.quota_per_run.values().copied().fold(0.0, f64::max),
        })
        .collect();
    let pareto = stats::pareto_frontier(&metrics);
    let paired = paired_comparisons(manifest, trials);
    (
        Aggregate {
            schema: "celeris.routing-evidence/1".into(),
            manifest_sha256: manifest.sha256.clone(),
            evidence_version: version,
            rows,
            pareto,
            paired,
        },
        detail,
    )
}

fn paired_comparisons(manifest: &Manifest, trials: &[Trial]) -> Vec<Value> {
    let mut candidates: BTreeMap<String, BTreeMap<String, bool>> = BTreeMap::new();
    for t in trials {
        candidates
            .entry(format!(
                "{}:{}:{}",
                t.key.source_id, t.key.model_id, t.key.harness_id
            ))
            .or_default()
            .entry(t.problem_id.clone())
            .or_insert(t.passed);
    }
    let entries: Vec<_> = candidates.into_iter().collect();
    let mut out = Vec::new();
    for i in 0..entries.len() {
        for j in i + 1..entries.len() {
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
                out.push(json!({"left":entries[i].0,"right":entries[j].0,"n":ids.len(),"mcnemar_p":p,"difference_left_minus_right":diff}));
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
        "# Routing evaluation evidence\n\nManifest: `{}`  \nEvidence version: `{}`\n\n| Genre | Harness | Version | Source | Model | n | Pass | Wilson 95% | Timeout | USD median | Wall median (s) | Low confidence |\n|---|---|---|---|---|---:|---:|---|---:|---:|---:|---|\n{}\n\nPareto set: {}\n\nPaired comparisons: {}\n",
        result.manifest_sha256,
        result.evidence_version,
        result
            .rows
            .iter()
            .map(|r| format!(
                "| {} | {} | {} | {} | {} | {} | {} | {:.3}–{:.3} | {:.3} | {} | {} | {} |",
                r.key.genre,
                r.key.harness_id,
                r.key.harness_version,
                r.key.source_id,
                r.key.model_id,
                r.n,
                r.pass,
                r.pass_lower,
                r.pass_upper,
                r.timeout_rate,
                r.usd_list_median.map_or("—".into(), |v| format!("{v:.4}")),
                r.wall_median_s.map_or("—".into(), |v| format!("{v:.2}")),
                r.low_confidence
            ))
            .collect::<Vec<_>>()
            .join("\n"),
        result.pareto.join(", "),
        serde_json::to_string(&result.paired).map_err(|e| e.to_string())?
    );
    fs::write(out.join("evidence.md"), md).map_err(|e| e.to_string())?;
    fs::write(
        out.join("trials.json"),
        serde_json::to_vec_pretty(&detail).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if args.iter().any(|x| x == "--write-evidence") {
        let (store, _) =
            SqliteStore::open_client(std::path::Path::new(&db)).map_err(|e| e.to_string())?;
        store
            .insert_routing_evidence(&result.rows)
            .map_err(|e| e.to_string())?;
    }
    println!(
        "trials={} cells={} evidence_version={}",
        trials.len(),
        result.rows.len(),
        result.evidence_version
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
            split: "dev".into(),
            stratum: "coding:task".into(),
            subset_12: true,
            head_commit: None,
            base_commit: None,
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
            sha256: "manifest-hash".into(),
        };
        let trial = Trial {
            decision_id: "d".into(),
            problem_id: "p1".into(),
            trial_no: 1,
            key: EvidenceKey {
                genre: "coding".into(),
                harness_id: "claude-code".into(),
                harness_version: "1".into(),
                source_id: "claude-oauth".into(),
                model_id: "m".into(),
            },
            passed: true,
            timed_out: false,
            wall_secs: Some(3.0),
            usd_list: Some(0.4),
            quota: BTreeMap::from([("7d".into(), 0.02)]),
            status: "passed".into(),
        };
        let (agg, detail) = build(&m, &[trial]);
        assert_eq!(agg.rows.len(), 1);
        let row = &agg.rows[0];
        assert_eq!(row.n, 1);
        assert_eq!(row.pass, 1);
        assert!(row.pass_lower < 1.0 && row.pass_upper == 1.0);
        assert!(row.low_confidence);
        assert_eq!(row.usd_list_median, Some(0.4));
        assert_eq!(row.quota_per_run["7d"], 0.02);
        assert_eq!(detail.len(), 1);
        assert_eq!(agg.pareto.len(), 1);
    }

    #[test]
    fn collect_reads_eval_decision_outcome_and_run_from_fixture_db() {
        let dir = tempfile::tempdir().expect("temp dir");
        let db = dir.path().join("eval.sqlite");
        let conn = Connection::open(&db).expect("db");
        conn.execute_batch("CREATE TABLE route_decisions(decision_id TEXT,subject_id TEXT,subject_kind TEXT,run_id TEXT,input_json TEXT,output_json TEXT,stage TEXT); CREATE TABLE route_outcomes(decision_id TEXT,labels_json TEXT); CREATE TABLE runs(run_id TEXT,status TEXT,started_at TEXT,finished_at TEXT,usage_json TEXT,metrics_json TEXT);")
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
            split: "dev".into(),
            stratum: "coding:task".into(),
            subset_12: false,
            head_commit: None,
            base_commit: None,
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
}
