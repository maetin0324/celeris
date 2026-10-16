//! 検査 (4) review case: 既知欠陥 list に対する検出率と誤指摘を数える。
//! 指摘は file・行・keyword で決定的に照合する（指摘側の defect id の自己申告は使わない）。
use std::path::Path;

use serde_json::{Value, json};

use super::{
    CheckResult,
    case::{DefectSpec, ReviewSpec},
    read_json, safe_join,
};

fn norm_path(p: &str) -> &str {
    p.trim().trim_start_matches("./")
}

fn matches(d: &DefectSpec, f: &Value) -> bool {
    let file = f["file"].as_str().unwrap_or("");
    if norm_path(file) != norm_path(&d.file) {
        return false;
    }
    if let Some((a, b)) = d.lines {
        match f["line"].as_u64() {
            Some(l) if (a..=b).contains(&l) => {}
            _ => return false,
        }
    }
    let body = format!(
        "{} {}",
        f["summary"].as_str().unwrap_or(""),
        f["detail"].as_str().unwrap_or("")
    )
    .to_lowercase();
    d.keywords.iter().any(|k| body.contains(&k.to_lowercase()))
}

pub fn check(spec: &ReviewSpec, artifact: &Path) -> CheckResult {
    let doc = match safe_join(artifact, &spec.findings).and_then(|p| read_json(&p)) {
        Ok(v) => v,
        Err(_) => {
            return CheckResult::from_reasons(
                vec![format!("review findings {} are missing", spec.findings)],
                Value::Null,
            );
        }
    };
    let empty = Vec::new();
    let findings = doc["findings"].as_array().unwrap_or(&empty);
    let mut detected = Vec::new();
    let mut missed = Vec::new();
    for d in &spec.known_defects {
        if findings.iter().any(|f| matches(d, f)) {
            detected.push(d.id.clone());
        } else {
            missed.push(d.id.clone());
        }
    }
    let mut false_pos = Vec::new();
    for (i, f) in findings.iter().enumerate() {
        let hit = spec
            .known_defects
            .iter()
            .chain(&spec.also_valid)
            .any(|d| matches(d, f));
        if !hit {
            false_pos.push(json!({
                "index": i,
                "file": f["file"],
                "line": f["line"],
            }));
        }
    }
    let rate = detected.len() as f64 / spec.known_defects.len() as f64;
    let mut reasons = Vec::new();
    if rate < spec.min_detection_rate {
        reasons.push(format!(
            "detected {}/{} known defects (rate {rate:.3} < required {})",
            detected.len(),
            spec.known_defects.len(),
            spec.min_detection_rate
        ));
    }
    if false_pos.len() > spec.max_false_positives {
        reasons.push(format!(
            "{} false finding(s) (neither a known defect nor a confirmed real issue) > allowed {}",
            false_pos.len(),
            spec.max_false_positives
        ));
    }
    CheckResult::from_reasons(
        reasons,
        json!({
            "known": spec.known_defects.len(),
            "detected": detected,
            "missed": missed,
            "detection_rate": rate,
            "findings": findings.len(),
            "false_positives": false_pos,
        }),
    )
}
