//! 公開 case → celeris.grade-case/1 の変換と、公開テスト改変／隠しテスト挿入の区別の試験。
//! 偽 workspace と temp dir だけを使う（ネットワーク・モデル無し）。評価 command は `sh` の script。
#[path = "support_cases/mod.rs"]
#[allow(dead_code)]
mod support_cases;
use std::{
    fs,
    path::{Path, PathBuf},
};

use celeris_campaign::{
    cases::{CaseSet, LoadedCase},
    grade_case_gen::{GenSource, generate},
    grade_runner::ProcessRunner,
    grader::{
        CommandRunner, GradeCase, GradeOptions, GradeReport, NoRunner, Status, Verdict, grade,
    },
};

const RUN_SH: &str = "test -f tests/hidden.txt || { echo hidden missing; exit 9; }\n\
if grep -q fixed src/impl.txt; then echo 'test t_pub ... ok'; echo 'test t_hidden ... ok'; \
else echo 'test t_hidden ... FAILED'; exit 1; fi\n";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn w(p: &Path, s: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, s).unwrap();
}

struct Fx {
    _d: tempfile::TempDir,
    grade_dir: PathBuf,
    work: PathBuf,
    case: GradeCase,
}

fn fixture() -> Fx {
    let set = CaseSet::load(
        &support_cases::cases_root(),
        &root().join("fixtures/grader-private"),
    )
    .expect("load");
    let LoadedCase { case, private } = set.get("k1-bf-fsync-campaign").expect("k1");
    let mut private = private.clone();
    private.hidden_command = "sh tests/run.sh -- t_pub t_hidden".into();
    let d = tempfile::tempdir().unwrap();
    let files = d.path().join("files");
    w(&files.join("public/tests/run.sh"), RUN_SH);
    w(&files.join("hidden/tests/hidden.txt"), "secret\n");
    let grade_dir = d.path().join("grade");
    let src = GenSource {
        repo: ".".into(),
        public_tests: vec!["tests/run.sh".into()],
        hidden_tests: vec!["tests/hidden.txt".into()],
        wrapper: None,
        files_dir: files,
    };
    let g = generate(case, &private, &src, &grade_dir).expect("generate");
    assert_eq!(g.definition_sha256.len(), 64);
    Fx {
        grade_dir,
        work: d.path().join("work"),
        case: g.case,
        _d: d,
    }
}

/// 成果物 workspace（公開テストは pristine のまま、隠しテストは無い）。
fn workspace(fx: &Fx, imp: &str) {
    w(&fx.work.join("tests/run.sh"), RUN_SH);
    w(&fx.work.join("src/impl.txt"), imp);
}

fn run(fx: &Fx, runner: &dyn CommandRunner) -> GradeReport {
    let opts = GradeOptions {
        case_dir: fx.grade_dir.clone(),
        work_root: fx.work.parent().unwrap().join("scratch"),
        runner,
        observed_jobs: None,
        initial_tree: None,
    };
    fs::create_dir_all(&opts.work_root).unwrap();
    grade(&fx.case, &fx.work, &opts)
}

fn impl_status(r: &GradeReport) -> Status {
    r.checks.implementation.status
}

#[test]
fn generated_case_is_grade_case_v1_and_separates_public_and_hidden() {
    let fx = fixture();
    let imp = fx.case.implementation.as_ref().unwrap();
    assert_eq!(fx.case.schema, "celeris.grade-case/1");
    assert_eq!(imp.tests.len(), 1);
    assert_eq!(imp.hidden_tests.len(), 1);
    assert_eq!(imp.must_pass, ["t_pub", "t_hidden"]);
    // 生成物は file からも読め、private schema とは別。
    let loaded = GradeCase::load(&fx.grade_dir.join("k1-bf-fsync-campaign.json")).expect("load");
    assert_eq!(loaded.id, "k1-bf-fsync-campaign");
}

#[test]
fn hidden_absent_in_workspace_is_not_deletion_and_fixed_passes() {
    let fx = fixture();
    workspace(&fx, "fixed\n");
    let r = run(&fx, &ProcessRunner::default());
    assert_eq!(
        impl_status(&r),
        Status::Pass,
        "{:?}",
        r.checks.implementation
    );
    assert_eq!(r.checks.implementation.details["hidden_inserted_count"], 1);
    assert_no_hidden_name(&r);
    // k1 は実験工程も持つので、証拠が無い試験全体は不合格のまま（実装検査だけが通る）。
    assert_eq!(r.verdict, Verdict::Fail);
}

#[test]
fn command_exit_code_decides_the_grade() {
    let fx = fixture();
    workspace(&fx, "buggy\n");
    let r = run(&fx, &ProcessRunner::default());
    assert_eq!(impl_status(&r), Status::Fail);
    assert_eq!(r.checks.implementation.details["exit"], 1);
}

#[test]
fn hidden_file_in_workspace_is_overwritten_not_judged() {
    let fx = fixture();
    workspace(&fx, "fixed\n");
    w(
        &fx.work.join("tests/hidden.txt"),
        "whatever the agent wrote\n",
    );
    let r = run(&fx, &ProcessRunner::default());
    assert_eq!(
        impl_status(&r),
        Status::Pass,
        "{:?}",
        r.checks.implementation
    );
}

#[test]
fn modified_public_test_fails_without_running_command() {
    let fx = fixture();
    workspace(&fx, "fixed\n");
    w(&fx.work.join("tests/run.sh"), "echo 'test t_pub ... ok'\n");
    let r = run(&fx, &ProcessRunner::default());
    assert_eq!(impl_status(&r), Status::Fail);
    assert_eq!(r.checks.implementation.details["ran"], false);
    assert!(r.checks.implementation.reasons[0].contains("differs from pristine"));
}

#[test]
fn deleted_public_test_fails() {
    let fx = fixture();
    workspace(&fx, "fixed\n");
    fs::remove_file(fx.work.join("tests/run.sh")).unwrap();
    let r = run(&fx, &ProcessRunner::default());
    assert_eq!(impl_status(&r), Status::Fail);
    assert!(r.checks.implementation.reasons[0].contains("deleted"));
}

#[test]
fn no_runner_is_fail_closed_even_for_a_fixed_workspace() {
    let fx = fixture();
    workspace(&fx, "fixed\n");
    let r = run(&fx, &NoRunner);
    assert_eq!(impl_status(&r), Status::Fail);
    assert!(r.checks.implementation.reasons[0].contains("could not run"));
}

#[test]
fn altered_grader_side_hidden_copy_fails() {
    let fx = fixture();
    workspace(&fx, "fixed\n");
    w(&fx.grade_dir.join("hidden/tests/hidden.txt"), "tampered\n");
    let r = run(&fx, &ProcessRunner::default());
    assert_eq!(impl_status(&r), Status::Fail);
    assert!(r.checks.implementation.reasons[0].contains("hidden test"));
}

#[test]
fn hidden_path_overlapping_a_public_test_is_rejected() {
    let mut fx = fixture();
    let imp = fx.case.implementation.as_mut().unwrap();
    imp.hidden_tests = imp.tests.clone();
    assert!(
        fx.case
            .validate()
            .unwrap_err()
            .contains("also a public test")
    );
}

#[test]
fn shell_command_is_rejected_by_the_converter() {
    let set = CaseSet::load(
        &support_cases::cases_root(),
        &root().join("fixtures/grader-private"),
    )
    .expect("load");
    let l = set.get("k1-bf-fsync-campaign").unwrap();
    let mut p = l.private.clone();
    p.hidden_command = "cargo test && rm -rf /".into();
    let d = tempfile::tempdir().unwrap();
    let src = GenSource {
        repo: ".".into(),
        public_tests: vec![],
        hidden_tests: vec![],
        wrapper: None,
        files_dir: d.path().to_path_buf(),
    };
    assert!(generate(&l.case, &p, &src, &d.path().join("o")).is_err());
}

/// 判定 JSON 全体に隠しテストの相対 path・file 名が出ていない（ADR 付記 2026-10-31「判定 JSON に隠しテストの
/// path を出さない」）。
fn assert_no_hidden_name(r: &GradeReport) {
    let j = serde_json::to_string(r).unwrap();
    assert!(!j.contains("tests/hidden.txt"), "{j}");
    assert!(!j.contains("hidden.txt"), "{j}");
}

/// 隠しテストを重ねた tree が compile error になり、出力が隠しテストの path（相対・絶対・file 名）を含んでも、
/// tail では `<hidden[0]>` に置き換わる。
#[test]
fn compile_error_output_naming_hidden_test_is_redacted() {
    let mut fx = fixture();
    let sh = "echo \"error[E0425]: cannot find value in $(pwd)/tests/hidden.txt\" >&2; \
              echo ' --> tests/hidden.txt:3:5' >&2; echo 'see hidden.txt' ; \
              echo 'error: could not compile' >&2; exit 101";
    fx.case.implementation.as_mut().unwrap().command = vec!["sh".into(), "-c".into(), sh.into()];
    workspace(&fx, "fixed\n");
    let r = run(&fx, &ProcessRunner::default());
    assert_eq!(
        impl_status(&r),
        Status::Fail,
        "{:?}",
        r.checks.implementation
    );
    assert_no_hidden_name(&r);
    let d = &r.checks.implementation.details;
    let err = d["stderr_tail"].as_str().unwrap();
    assert!(
        err.contains("/scratch/") && err.contains("/<hidden[0]>\n"),
        "{err}"
    );
    assert!(err.contains(" --> <hidden[0]>:3:5"), "{err}");
    assert!(
        d["stdout_tail"]
            .as_str()
            .unwrap()
            .contains("see <hidden[0]>")
    );
}
