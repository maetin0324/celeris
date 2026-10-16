//! `grade-out`: out の全試行（実行したもの）を独立 grader で採点し、`grades.jsonl` に書く
//! （ADR 2026-10-16-mlsys-pilot-wiring-fixes D9）。grade-case は `<cases>/<problem_id>.json`。
//! 試行記録の outcome は読まない（grader は成果物と case 定義だけを見る）。
use std::{fs, io::Write, path::Path};

use serde_json::{Value, json};

use crate::{
    grader::sha256_file,
    grader::{CommandRunner, GradeCase, GradeOptions, grade},
    grades::GRADE_LINE_SCHEMA,
    manifest::CampaignManifest,
    record::{TRIALS_FILE, TrialRecord},
    snapshot::hash_tree,
};

pub use crate::grades::GRADES_FILE;

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
    let mut rejected: Vec<String> = Vec::new();
    for (i, l) in text.lines().enumerate() {
        let r: TrialRecord =
            serde_json::from_str(l).map_err(|e| format!("trials.jsonl line {}: {e}", i + 1))?;
        if r.campaign_sha256 != hash {
            return Err(format!(
                "trials.jsonl line {}: record of another campaign",
                i + 1
            ));
        }
        let key = json!({"config": r.config, "problem_id": r.problem_id, "rep": r.rep, "trial_no": r.trial_no});
        let tag = format!(
            "{:?}/{}/rep{}#{}",
            r.config, r.problem_id, r.rep, r.trial_no
        );
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
        if case.id != r.problem_id {
            rejected.push(format!(
                "{tag}: grade-case id {} differs from the problem id",
                case.id
            ));
            lines.push(json!({"trial": key, "graded": false, "why": "grade-case id differs from the problem id"}));
            continue;
        }
        // 採点する成果物が、実行終了時に記録した workspace と同じものか（古い・改変された成果物は採点しない）。
        let artifact = out.join(&r.workspace);
        let artifact_sha = hash_tree(&artifact)?;
        if artifact_sha != r.workspace_sha256_final {
            rejected.push(format!(
                "{tag}: artifact hash differs from the trial's recorded final workspace"
            ));
            lines.push(json!({"trial": key, "graded": false, "why": "artifact hash differs from the recorded final workspace"}));
            continue;
        }
        let grader_sha = sha256_file(&case_path)?;
        let opts = GradeOptions {
            case_dir: case_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default(),
            work_root: std::env::temp_dir(),
            runner,
        };
        let rep = grade(&case, &artifact, &opts);
        graded += 1;
        final_pass += u64::from(rep.final_pass);
        lines.push(json!({
            "schema": GRADE_LINE_SCHEMA,
            "campaign_sha256": hash,
            "split": m.split,
            "trial": key,
            "case_id": case.id,
            "grader_sha256": grader_sha,
            "artifact_sha256": artifact_sha,
            "graded": true,
            "grade": rep,
        }));
    }
    let mut f = fs::File::create(out.join(GRADES_FILE)).map_err(|e| e.to_string())?;
    for l in &lines {
        writeln!(f, "{l}").map_err(|e| e.to_string())?;
    }
    missing_cases.sort();
    missing_cases.dedup();
    // ok は「処理が完了したか」。個々の grade の合否（final_pass）ではない。
    Ok(json!({
        "ok": missing_cases.is_empty() && rejected.is_empty(),
        "rejected": rejected,
        "graded": graded,
        "final_pass": final_pass,
        "missing_cases": missing_cases,
        "grades": GRADES_FILE,
    }))
}
