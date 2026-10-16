//! `grade-out`: out の全試行（実行したもの）を独立 grader で採点し、`grades.jsonl` に書く
//! （ADR 2026-10-16-mlsys-pilot-wiring-fixes D9）。grade-case は `<cases>/<problem_id>.json`。
//! 試行記録の outcome は読まない（grader は成果物と case 定義だけを見る）。
use std::{fs, io::Write, path::Path};

use serde_json::{Value, json};

use crate::{
    grader::{CommandRunner, GradeCase, GradeOptions, grade},
    manifest::CampaignManifest,
    record::{TRIALS_FILE, TrialRecord},
};

pub const GRADES_FILE: &str = "grades.jsonl";

pub fn grade_out(
    m: &CampaignManifest,
    out: &Path,
    cases: &Path,
    runner: &dyn CommandRunner,
) -> Result<Value, String> {
    let hash = m.compute_sha256()?;
    let text = fs::read_to_string(out.join(TRIALS_FILE))
        .map_err(|e| format!("{}: {e}", out.join(TRIALS_FILE).display()))?;
    let mut lines = Vec::new();
    let (mut graded, mut final_pass) = (0u64, 0u64);
    let mut missing_cases = Vec::new();
    for (i, l) in text.lines().enumerate() {
        let r: TrialRecord =
            serde_json::from_str(l).map_err(|e| format!("trials.jsonl line {}: {e}", i + 1))?;
        if r.campaign_sha256 != hash {
            return Err(format!(
                "trials.jsonl line {}: record of another campaign",
                i + 1
            ));
        }
        let key = json!({"config": r.config, "problem_id": r.problem_id, "rep": r.rep});
        if !r.ran() || r.workspace.is_empty() {
            lines.push(json!({"trial": key, "graded": false, "why": "not run"}));
            continue;
        }
        let case_path = cases.join(format!("{}.json", r.problem_id));
        if !case_path.is_file() {
            missing_cases.push(r.problem_id.clone());
            lines.push(json!({"trial": key, "graded": false, "why": format!("no grade-case {}", case_path.display())}));
            continue;
        }
        let case = GradeCase::load(&case_path)?;
        let opts = GradeOptions {
            case_dir: case_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default(),
            work_root: std::env::temp_dir(),
            runner,
        };
        let rep = grade(&case, &out.join(&r.workspace), &opts);
        graded += 1;
        final_pass += u64::from(rep.final_pass);
        lines.push(json!({"trial": key, "graded": true, "grade": rep}));
    }
    let mut f = fs::File::create(out.join(GRADES_FILE)).map_err(|e| e.to_string())?;
    for l in &lines {
        writeln!(f, "{l}").map_err(|e| e.to_string())?;
    }
    missing_cases.sort();
    missing_cases.dedup();
    Ok(json!({
        "ok": missing_cases.is_empty(),
        "graded": graded,
        "final_pass": final_pass,
        "missing_cases": missing_cases,
        "grades": GRADES_FILE,
    }))
}
