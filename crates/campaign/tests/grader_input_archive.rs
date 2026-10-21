//! 再計算検査は sha256 固定の入力 tgz を grader 自身が展開して読む（作業者の展開先に依存しない）。
//! 欠落・sha256 不一致は model_failure ではなく env_failure（grader/配線側）。
use std::{fs, path::Path, process::Command};

use celeris_campaign::grader::{
    CommandOutput, CommandRunner, FailureClass, GradeCase, GradeOptions, GradeReport, Status,
    Verdict, grade,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

struct NoRun;
impl CommandRunner for NoRun {
    fn run(&self, _argv: &[String], _cwd: &Path) -> Result<CommandOutput, String> {
        Err("not used".into())
    }
}

const CELL: &str = "{\"bw\": 10.0}";

struct Fx {
    _t: tempfile::TempDir,
    case_dir: std::path::PathBuf,
    work: std::path::PathBuf,
    art: std::path::PathBuf,
    case: GradeCase,
}

fn fixture(archive_sha: Option<String>, pin_sha: Option<String>) -> Fx {
    let t = tempfile::tempdir().expect("tmp");
    let case_dir = t.path().join("case");
    let src = t.path().join("src/runs/r1");
    fs::create_dir_all(&src).expect("src");
    fs::create_dir_all(&case_dir).expect("case");
    fs::write(src.join("cell.json"), CELL).expect("cell");
    let tgz = case_dir.join("inputs.tgz");
    let st = Command::new("tar")
        .arg("-czf")
        .arg(&tgz)
        .arg("-C")
        .arg(t.path().join("src"))
        .arg("runs")
        .status()
        .expect("tar");
    assert!(st.success());
    let real = format!("{:x}", Sha256::digest(fs::read(&tgz).expect("read")));
    let cell_sha = format!("{:x}", Sha256::digest(CELL.as_bytes()));
    let case_json: Value = json!({
        "schema": "celeris.grade-case/1", "id": "arc", "split": "final",
        "recompute": {
            "report": "report.md",
            "input_archive": {"path": "inputs.tgz", "sha256": archive_sha.unwrap_or(real)},
            "pinned_raw": {"inputs/runs/r1/cell.json": pin_sha.unwrap_or(cell_sha)},
            "metrics": [{"name": "bw", "agg": "mean", "tolerance_rel": 0.01,
                "source": {"json": {"files": ["inputs/runs/r1/cell.json"], "pointer": "/bw"}}}]
        }
    });
    let cp = case_dir.join("arc.json");
    fs::write(&cp, case_json.to_string()).expect("case json");
    let art = t.path().join("artifact");
    fs::create_dir_all(&art).expect("art");
    fs::write(art.join("report.md"), "metric bw = 10.0\n").expect("report");
    let work = t.path().join("work");
    fs::create_dir_all(&work).expect("work");
    let case = GradeCase::load(&cp).expect("load");
    Fx {
        _t: t,
        case_dir,
        work,
        art,
        case,
    }
}

fn run(fx: &Fx) -> GradeReport {
    let opts = GradeOptions {
        case_dir: fx.case_dir.clone(),
        work_root: fx.work.clone(),
        runner: &NoRun,
        observed_jobs: None,
        initial_tree: None,
    };
    grade(&fx.case, &fx.art, &opts)
}

#[test]
fn grades_from_the_pinned_archive_wherever_the_worker_extracted() {
    let fx = fixture(None, None);
    // 作業者の workspace には ex/・inputs/x/ など別の場所に展開した写し（または何も無い）だけがある。
    fs::create_dir_all(fx.art.join("ex/runs/r1")).expect("ex");
    fs::write(fx.art.join("ex/runs/r1/cell.json"), "{\"bw\": 999.0}").expect("ex cell");
    let r = run(&fx);
    assert_eq!(
        r.checks.recompute.status,
        Status::Pass,
        "{:?}",
        r.checks.recompute.reasons
    );
    assert_eq!(r.verdict, Verdict::Pass);
    // 作業者が嘘の数値を報告すれば、固定入力で再計算して model_failure になる。
    fs::write(fx.art.join("report.md"), "metric bw = 50.0\n").expect("report");
    let r = run(&fx);
    assert_eq!(r.failure_class, Some(FailureClass::ModelFailure));
    // grader の展開先は残さない。
    assert_eq!(fs::read_dir(&fx.work).expect("work").count(), 0);
}

#[test]
fn missing_or_mismatching_pinned_input_is_grader_side_not_model_failure() {
    let missing = fixture(None, None);
    fs::remove_file(missing.case_dir.join("inputs.tgz")).expect("rm");
    let r = run(&missing);
    assert_eq!(r.verdict, Verdict::Fail);
    assert_eq!(
        r.failure_class,
        Some(FailureClass::EnvFailure),
        "{:?}",
        r.checks.recompute.reasons
    );

    let bad_arc = fixture(Some("0".repeat(64)), None);
    assert_eq!(run(&bad_arc).failure_class, Some(FailureClass::EnvFailure));

    let bad_pin = fixture(None, Some("1".repeat(64)));
    assert_eq!(run(&bad_pin).failure_class, Some(FailureClass::EnvFailure));
}
