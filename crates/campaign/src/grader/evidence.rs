//! 検査 (2) 実験条件と実行証拠: job ID・scheduler 記録・ログ・生データの存在と、case の条件との一致。
//! `evidence_kind=mock` は control case 以外で不合格。
use std::{collections::BTreeSet, fs, path::Path};

use serde_json::{Value, json};

use super::{CheckResult, case::ExperimentSpec, read_json, safe_join, sha256_file};

fn valid_job_id(id: &str) -> bool {
    let (num, host) = id.split_once('.').unwrap_or((id, ""));
    !num.is_empty()
        && num.bytes().all(|b| b.is_ascii_digit())
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
}

/// scheduler 記録が正常終了を示すか（PBS `qstat -x -f` / Slurm `sacct` / 汎用 `exit_status=0`）。
fn scheduler_exit_ok(text: &str) -> bool {
    if text.contains("Unknown Job Id") {
        return false;
    }
    text.lines().any(|l| {
        let n: String = l.chars().filter(|c| !c.is_whitespace()).collect();
        let n = n.to_ascii_lowercase();
        n == "exit_status=0" || n.ends_with(":exit_status=0") || n.contains("exitcode=0:0")
    })
}

fn file_text(artifact: &Path, rel: Option<&str>, what: &str, tag: &str) -> Result<String, String> {
    let rel = rel.ok_or(format!("{tag}: {what} path is missing"))?;
    let p = safe_join(artifact, rel).map_err(|e| format!("{tag}: {e}"))?;
    fs::read_to_string(&p).map_err(|_| format!("{tag}: {what} {rel} is missing or unreadable"))
}

pub fn check(spec: &ExperimentSpec, artifact: &Path, control: bool) -> CheckResult {
    let jobs_path = match safe_join(artifact, &spec.jobs_file) {
        Ok(p) => p,
        Err(e) => return CheckResult::from_reasons(vec![e], Value::Null),
    };
    let doc = match read_json(&jobs_path) {
        Ok(v) => v,
        Err(_) => {
            return CheckResult::from_reasons(
                vec![format!("execution evidence {} is missing", spec.jobs_file)],
                Value::Null,
            );
        }
    };
    let empty = Vec::new();
    let jobs = doc["jobs"].as_array().unwrap_or(&empty);
    let mut reasons = Vec::new();
    let mut ids = BTreeSet::new();
    let mut seen_raw = BTreeSet::new();
    let mut rows = Vec::new();
    for (i, j) in jobs.iter().enumerate() {
        let id = j["job_id"].as_str().unwrap_or("");
        let tag = format!("job[{i}] {id}");
        let before = reasons.len();
        match j["evidence_kind"].as_str() {
            Some("real") => {}
            Some("mock") if control => {}
            Some("mock") => reasons.push(format!(
                "{tag}: evidence_kind=mock is not accepted outside control cases"
            )),
            other => reasons.push(format!("{tag}: unknown evidence_kind {other:?}")),
        }
        if !valid_job_id(id) {
            reasons.push(format!(
                "{tag}: job_id is missing or not a scheduler job id"
            ));
        } else if !ids.insert(id.to_string()) {
            reasons.push(format!("{tag}: job_id recorded twice"));
        }
        let num = id.split('.').next().unwrap_or("");
        match file_text(
            artifact,
            j["scheduler_record"].as_str(),
            "scheduler record",
            &tag,
        ) {
            Ok(t) => {
                if num.is_empty() || !t.contains(num) {
                    reasons.push(format!(
                        "{tag}: scheduler record does not mention the job id"
                    ));
                }
                if !scheduler_exit_ok(&t) {
                    reasons.push(format!(
                        "{tag}: scheduler record does not show a successful exit status"
                    ));
                }
            }
            Err(e) => reasons.push(e),
        }
        match file_text(artifact, j["log"].as_str(), "log", &tag) {
            Ok(t) if t.trim().is_empty() => reasons.push(format!("{tag}: log is empty")),
            Ok(_) => {}
            Err(e) => reasons.push(e),
        }
        let raw = j["raw"].as_array().unwrap_or(&empty);
        if raw.is_empty() {
            reasons.push(format!("{tag}: no raw data recorded"));
        }
        for r in raw {
            let rel = r["path"].as_str().unwrap_or("");
            match safe_join(artifact, rel) {
                Ok(p) if p.is_file() => {
                    let got = sha256_file(&p).unwrap_or_default();
                    if r["sha256"].as_str() != Some(got.as_str()) {
                        reasons.push(format!(
                            "{tag}: raw data {rel} sha256 differs from the recorded value"
                        ));
                    }
                    if !seen_raw.insert(rel.to_string()) {
                        reasons.push(format!("{tag}: raw data {rel} is claimed by two jobs"));
                    }
                }
                Ok(_) => reasons.push(format!("{tag}: raw data {rel} is missing")),
                Err(e) => reasons.push(format!("{tag}: {e}")),
            }
        }
        for (k, want) in &spec.conditions {
            let got = &j["conditions"][k];
            if got != want {
                reasons.push(format!(
                    "{tag}: condition {k} = {got} but the case requires {want}"
                ));
            }
        }
        rows.push(json!({"job_id": id, "ok": reasons.len() == before}));
    }
    if jobs.len() < spec.min_jobs {
        reasons.push(format!(
            "{} job(s) recorded but the case requires at least {}",
            jobs.len(),
            spec.min_jobs
        ));
    }
    CheckResult::from_reasons(reasons, json!({"jobs": rows, "required": spec.min_jobs}))
}
