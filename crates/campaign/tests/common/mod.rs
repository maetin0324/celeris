//! 試験用: mock 実行の out に、`grade-out` と同じ形の grades.jsonl を書く（採点 command は走らせない）。
#![allow(dead_code)]
use std::{fs, path::Path};

use celeris_campaign::{
    grades::{GRADE_LINE_SCHEMA, GRADES_FILE},
    manifest::CampaignManifest,
};
use serde_json::{Value, json};

/// 実行した全試行に grade を付ける。`final_pass(record)` が採点結果。
pub fn write_grades(m: &CampaignManifest, out: &Path, final_pass: impl Fn(&Value) -> bool) {
    let hash = m.compute_sha256().expect("hash");
    let text = fs::read_to_string(out.join("trials.jsonl")).expect("trials");
    let mut s = String::new();
    for l in text.lines() {
        let r: Value = serde_json::from_str(l).expect("json");
        if r["workspace"] == "" {
            continue;
        }
        let pid = r["problem_id"].clone();
        let fp = final_pass(&r);
        let line = json!({
            "schema": GRADE_LINE_SCHEMA,
            "campaign_sha256": hash,
            "split": m.split,
            "trial": {"config": r["config"], "problem_id": pid, "rep": r["rep"], "trial_no": r["trial_no"]},
            "case_id": pid,
            "grader_sha256": "00",
            "artifact_sha256": r["workspace_sha256_final"],
            "graded": true,
            "failure_class": (!fp).then_some("model_failure"),
            "grade": {"case_id": pid, "split": m.split, "final_pass": fp, "failure_class": (!fp).then_some("model_failure")},
        });
        s += &format!("{line}\n");
    }
    fs::write(out.join(GRADES_FILE), s).expect("grades");
}

/// `class(record)` が None なら合格、Some(c) なら不合格で `failure_class = c`（model_failure / env_failure / undetermined）。
pub fn write_grades_classed(
    m: &CampaignManifest,
    out: &Path,
    class: impl Fn(&Value) -> Option<&'static str>,
) {
    let hash = m.compute_sha256().expect("hash");
    let text = fs::read_to_string(out.join("trials.jsonl")).expect("trials");
    let mut s = String::new();
    for l in text.lines() {
        let r: Value = serde_json::from_str(l).expect("json");
        if r["workspace"] == "" {
            continue;
        }
        let pid = r["problem_id"].clone();
        let c = class(&r);
        let line = json!({
            "schema": GRADE_LINE_SCHEMA,
            "campaign_sha256": hash,
            "split": m.split,
            "trial": {"config": r["config"], "problem_id": pid, "rep": r["rep"], "trial_no": r["trial_no"]},
            "case_id": pid,
            "grader_sha256": "00",
            "artifact_sha256": r["workspace_sha256_final"],
            "graded": true,
            "failure_class": c,
            "grade": {"case_id": pid, "split": m.split, "final_pass": c.is_none(), "failure_class": c},
        });
        s += &format!("{line}\n");
    }
    fs::write(out.join(GRADES_FILE), s).expect("grades");
}
