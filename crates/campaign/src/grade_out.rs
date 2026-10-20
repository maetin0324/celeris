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

/// grade-case の path。`<cases>/<problem>.json`、locked な out では本番の配置 `<root>/<split>/<problem>/<problem>.json`
/// （lock は split を固定している）も探す。
pub(crate) fn resolve_case_path(
    m: &CampaignManifest,
    cases: &Path,
    problem_id: &str,
    locked: bool,
) -> std::path::PathBuf {
    let flat = cases.join(format!("{problem_id}.json"));
    if locked && !flat.is_file() {
        let split = if m.split == crate::manifest::Split::Final {
            "final"
        } else {
            "dev"
        };
        return cases
            .join(split)
            .join(problem_id)
            .join(format!("{problem_id}.json"));
    }
    flat
}

/// 再採点で置き換えられた過去の grade 行（追記専用）。
pub const GRADES_HISTORY_FILE: &str = "grades.history.jsonl";

pub fn grade_out(
    m: &CampaignManifest,
    out: &Path,
    cases: &Path,
    runner: &dyn CommandRunner,
) -> Result<Value, String> {
    grade_out_expected(m, out, cases, runner, None)
}

/// 保存 lock があれば（`expected` で cases root から作り直した期待 lock とも照合し）、`cases` の grade-case と
/// 公開/隠しテスト全実体を lock の固定値に照合する。不一致は grader command を走らせる前に拒否する。
/// lock が無い legacy out（lock 導入前・mock）は従来どおり。locked と記録された out の lock 欠落は Err。
pub fn grade_out_expected(
    m: &CampaignManifest,
    out: &Path,
    cases: &Path,
    runner: &dyn CommandRunner,
    expected: Option<&crate::lock::CampaignLock>,
) -> Result<Value, String> {
    grade_out_with(m, out, cases, runner, expected, None)
}

/// `grade_out_expected` に、grader 側で組んだ初期 tree の置き場（`<dir>/<problem_id>`）を足したもの。
pub fn grade_out_with(
    m: &CampaignManifest,
    out: &Path,
    cases: &Path,
    runner: &dyn CommandRunner,
    expected: Option<&crate::lock::CampaignLock>,
    initial_trees: Option<&Path>,
) -> Result<Value, String> {
    let lock = crate::lock_exec::load_saved(m, out, expected)?;
    let hash = m.compute_sha256()?;
    let ledger = crate::ledger::Ledger::open_readonly(out)?;
    let text = fs::read_to_string(out.join(TRIALS_FILE))
        .map_err(|e| format!("{}: {e}", out.join(TRIALS_FILE).display()))?;
    let mut lines = Vec::new();
    let (mut graded, mut final_pass) = (0u64, 0u64);
    let mut by_class: std::collections::BTreeMap<String, u64> = Default::default();
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
        let case_path = resolve_case_path(m, cases, &r.problem_id, lock.is_some());
        if !case_path.is_file() {
            missing_cases.push(r.problem_id.clone());
            lines.push(json!({"trial": key, "graded": false, "why": format!("no grade-case {}", case_path.display())}));
            continue;
        }
        let case = match &lock {
            Some(l) => {
                let verified = match l.case(&r.problem_id) {
                    None => Err(format!(
                        "{}: case is not in the campaign lock",
                        r.problem_id
                    )),
                    Some(c) => crate::lock_exec::verify_grade_case(c, &case_path),
                };
                match verified {
                    Ok(g) => g,
                    Err(e) => {
                        rejected.push(format!("{tag}: {e}"));
                        lines.push(json!({"trial": key, "graded": false, "why": "grader or tests differ from the campaign lock"}));
                        continue;
                    }
                }
            }
            None => GradeCase::load(&case_path)?,
        };
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
        // 評価側の観測記録は、この trial の identity・台帳の job ID・scheduler 応答の hash に結ばれているものだけ使う。
        let observed_path = out
            .join("observed")
            .join(&r.problem_id)
            .join(format!("{:?}-rep{}.json", r.config, r.rep));
        if observed_path.is_file()
            && let Err(e) = crate::observe::verify_binding(out, &observed_path, &ledger, &r, &hash)
        {
            rejected.push(format!("{tag}: {e}"));
            lines.push(json!({"trial": key, "graded": false, "why": "evaluator observation is not bound to this trial"}));
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
            // 評価側の取得記録は out/observed/<problem>/<config>-rep<N>.json（成果物の外）。
            observed_jobs: Some(observed_path).filter(|p| p.is_file()),
            // grader 側で組んだ初期 tree は <initial_trees>/<problem_id>（compile error の出どころの判定に使う）。
            initial_tree: initial_trees.map(|d| d.join(&r.problem_id)),
        };
        let rep = grade(&case, &artifact, &opts);
        graded += 1;
        final_pass += u64::from(rep.final_pass);
        if let Some(fc) = rep.failure_class {
            *by_class
                .entry(
                    serde_json::to_value(fc)
                        .ok()
                        .and_then(|v| v.as_str().map(String::from))
                        .unwrap_or_default(),
                )
                .or_insert(0u64) += 1;
        }
        lines.push(json!({
            "schema": GRADE_LINE_SCHEMA,
            "campaign_sha256": hash,
            "split": m.split,
            "trial": key,
            "case_id": case.id,
            "grader_sha256": grader_sha,
            "lock_sha256": lock.as_ref().map(|l| l.sha256.clone()).unwrap_or_default(),
            "tests_sha256": lock
                .as_ref()
                .and_then(|l| l.case(&r.problem_id))
                .map(|c| c.tests_sha256())
                .unwrap_or_default(),
            "artifact_sha256": artifact_sha,
            "graded": true,
            "failure_class": rep.failure_class,
            "grade": rep,
        }));
    }
    // 再採点: 直前の grades.jsonl は捨てず grades.history.jsonl に追記する（履歴を失わない）。
    // 結合・集計が読むのは常に最新の grades.jsonl だけで、試行記録（trials.jsonl の消費）は触らない。
    let cur = out.join(GRADES_FILE);
    if let Ok(prev) = fs::read_to_string(&cur) {
        let mut h = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(out.join(GRADES_HISTORY_FILE))
            .map_err(|e| e.to_string())?;
        h.write_all(prev.as_bytes()).map_err(|e| e.to_string())?;
    }
    let mut f = fs::File::create(&cur).map_err(|e| e.to_string())?;
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
        // 不合格の原因別（model_failure / env_failure / undetermined）。
        "failure_classes": by_class,
        "missing_cases": missing_cases,
        "grades": GRADES_FILE,
    }))
}
