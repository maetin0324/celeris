//! 検査 (3) 生データから指標を独立に再計算し、報告値と許容差で照合する。
//! 報告は `metric <name> = <value>` 行（report.rs の `result` 行と同じく、行で宣言した数値だけを照合する）。
use std::{collections::BTreeMap, fs, path::Path};

use serde_json::{Value, json};

use super::{
    CheckResult,
    case::{Agg, MetricSpec, RecomputeSpec, Source},
    read_json, safe_join, sha256_file,
};

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn split_csv(line: &str) -> Vec<String> {
    line.split(',')
        .map(|c| c.trim().trim_matches('"').to_string())
        .collect()
}

fn samples(artifact: &Path, src: &Source) -> Result<Vec<f64>, String> {
    let mut out = Vec::new();
    match src {
        Source::Json { files, pointer } => {
            for f in files {
                let v = read_json(&safe_join(artifact, f)?)?;
                let x = v
                    .pointer(pointer)
                    .and_then(number)
                    .ok_or(format!("{f}: {pointer} is missing or not a number"))?;
                out.push(x);
            }
        }
        Source::Csv {
            files,
            column,
            filter,
        } => {
            for f in files {
                let p = safe_join(artifact, f)?;
                let text = fs::read_to_string(&p).map_err(|e| format!("{f}: {e}"))?;
                let mut lines = text.lines().filter(|l| !l.trim().is_empty());
                let header = split_csv(lines.next().ok_or(format!("{f}: empty csv"))?);
                let col = |name: &str| {
                    header
                        .iter()
                        .position(|h| h == name)
                        .ok_or(format!("{f}: no column {name}"))
                };
                let ci = col(column)?;
                let fi: Vec<(usize, &String)> = filter
                    .iter()
                    .map(|(k, v)| col(k).map(|i| (i, v)))
                    .collect::<Result<_, _>>()?;
                for (n, line) in lines.enumerate() {
                    let row = split_csv(line);
                    if fi.iter().all(|(i, v)| row.get(*i) == Some(*v)) {
                        let cell = row.get(ci).map(String::as_str).unwrap_or("");
                        out.push(cell.parse().map_err(|_| {
                            format!("{f} row {}: {column} = `{cell}` is not a number", n + 2)
                        })?);
                    }
                }
            }
        }
        Source::Ratio { .. } => return Err("ratio has no raw samples".into()),
    }
    Ok(out)
}

pub fn aggregate(agg: Agg, xs: &[f64]) -> Option<f64> {
    if agg == Agg::Count {
        return Some(xs.len() as f64);
    }
    if xs.is_empty() {
        return None;
    }
    let mut s = xs.to_vec();
    s.sort_by(f64::total_cmp);
    Some(match agg {
        Agg::Mean => s.iter().sum::<f64>() / s.len() as f64,
        Agg::Median => {
            let m = s.len() / 2;
            if s.len() % 2 == 1 {
                s[m]
            } else {
                (s[m - 1] + s[m]) / 2.0
            }
        }
        Agg::Sum => s.iter().sum(),
        Agg::Min => s[0],
        Agg::Max => s[s.len() - 1],
        Agg::Count => s.len() as f64,
    })
}

fn recompute_one(
    artifact: &Path,
    m: &MetricSpec,
    done: &BTreeMap<String, f64>,
) -> Result<f64, String> {
    match &m.source {
        Source::Ratio {
            numerator,
            denominator,
        } => {
            let n = done
                .get(numerator)
                .ok_or(format!("ratio needs {numerator} defined earlier"))?;
            let d = done
                .get(denominator)
                .ok_or(format!("ratio needs {denominator} defined earlier"))?;
            if *d == 0.0 {
                return Err("ratio denominator is zero".into());
            }
            Ok(n / d)
        }
        src => aggregate(m.agg, &samples(artifact, src)?).ok_or("no samples".into()),
    }
}

/// 報告の `metric <name> = <value>`（`=` の前後の空白は任意）。同名が 2 度あれば誤り。
pub fn parse_report(text: &str) -> (BTreeMap<String, String>, Vec<String>) {
    let mut vals = BTreeMap::new();
    let mut errs = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("metric ") else {
            continue;
        };
        match rest.split_once('=') {
            Some((k, v)) => {
                let k = k.trim().to_string();
                if vals.insert(k.clone(), v.trim().to_string()).is_some() {
                    errs.push(format!("report: metric {k} reported twice"));
                }
            }
            None => errs.push(format!("report: malformed metric line `{line}`")),
        }
    }
    (vals, errs)
}

pub fn check(spec: &RecomputeSpec, artifact: &Path) -> CheckResult {
    let mut reasons = Vec::new();
    for (rel, want) in &spec.pinned_raw {
        match safe_join(artifact, rel).and_then(|p| sha256_file(&p)) {
            Ok(got) if &got == want => {}
            Ok(_) => reasons.push(format!(
                "raw input {rel} differs from the pinned snapshot (sha256 mismatch)"
            )),
            Err(_) => reasons.push(format!("raw input {rel} is missing")),
        }
    }
    let text = match safe_join(artifact, &spec.report).and_then(|p| {
        fs::read_to_string(&p).map_err(|_| format!("report {} is missing", spec.report))
    }) {
        Ok(t) => t,
        Err(e) => {
            reasons.push(e);
            String::new()
        }
    };
    let (reported, errs) = parse_report(&text);
    reasons.extend(errs);
    let mut done = BTreeMap::new();
    let mut rows = Vec::new();
    for m in &spec.metrics {
        let rc = recompute_one(artifact, m, &done);
        let rep = reported.get(&m.name);
        let mut row = json!({"name": m.name, "tolerance_rel": m.tolerance_rel});
        match &rc {
            Ok(x) => {
                done.insert(m.name.clone(), *x);
                row["recomputed"] = json!(x);
            }
            Err(e) => {
                row["recompute_error"] = json!(e);
                reasons.push(format!(
                    "metric {}: cannot recompute from raw data: {e}",
                    m.name
                ));
            }
        }
        match rep.map(|s| s.parse::<f64>()) {
            None => reasons.push(format!("metric {}: not reported", m.name)),
            Some(Err(_)) => {
                reasons.push(format!("metric {}: reported value is not a number", m.name))
            }
            Some(Ok(r)) => {
                row["reported"] = json!(r);
                if let Ok(x) = rc {
                    let tol = m.tolerance_rel * x.abs().max(1e-12);
                    let ok = (r - x).abs() <= tol;
                    row["within_tolerance"] = json!(ok);
                    if !ok {
                        reasons.push(format!(
                            "metric {}: reported {r} but recomputed {x} from raw data (tolerance {})",
                            m.name, m.tolerance_rel
                        ));
                    }
                }
            }
        }
        rows.push(row);
    }
    CheckResult::from_reasons(reasons, json!({"metrics": rows}))
}
