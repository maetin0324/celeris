//! 独立 grader の試験（ADR 2026-10-16-mlsys-independent-grader）。
//! 合成した成果物 dir と case 定義で、成功宣言だけ・評価テスト削除/弱体化・mock Passed・生データ改竄・
//! 数値捏造・誤指摘がそれぞれ不合格になることを確かめる。評価 command は stub runner で置き換える。
use std::{
    cell::RefCell,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use celeris_campaign::grader::{
    CaseSplit, CommandOutput, CommandRunner, GradeCase, GradeOptions, GradeReport, Status, Verdict,
    grade,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const PRISTINE_TEST: &str = r#"use demo::add;

#[test]
fn adds_small_numbers() {
    assert_eq!(add(1, 2), 3);
    assert_eq!(add(0, 0), 0);
}

#[test]
fn adds_large_numbers() {
    assert_eq!(add(1 << 30, 1 << 30), 1 << 31);
}
"#;

const OK_STDOUT: &str = "running 2 tests\n\
test adds_small_numbers ... ok\n\
test adds_large_numbers ... ok\n\
test result: ok. 2 passed; 0 failed\n";

fn sha(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}

fn write(p: &Path, s: &str) {
    if let Some(d) = p.parent() {
        fs::create_dir_all(d).unwrap();
    }
    fs::write(p, s).unwrap();
}

/// stub runner: 作業 dir の評価テストが pristine と同じであることを記録し、決めた出力を返す。
struct Stub {
    exit: i32,
    stdout: String,
    calls: RefCell<Vec<(Vec<String>, String)>>,
}

impl Stub {
    fn ok() -> Self {
        Stub {
            exit: 0,
            stdout: OK_STDOUT.into(),
            calls: RefCell::new(Vec::new()),
        }
    }
}

impl CommandRunner for Stub {
    fn run(&self, argv: &[String], cwd: &Path) -> Result<CommandOutput, String> {
        let t = fs::read_to_string(cwd.join("tests/add.rs")).unwrap_or_default();
        self.calls
            .borrow_mut()
            .push((argv.to_vec(), sha(t.as_bytes())));
        Ok(CommandOutput {
            exit: Some(self.exit),
            stdout: self.stdout.clone(),
            stderr: String::new(),
        })
    }
}

struct Fx {
    _tmp: tempfile::TempDir,
    case_dir: PathBuf,
    art: PathBuf,
    work: PathBuf,
    /// 評価側が取得した job 記録（成果物の外）。
    observed: PathBuf,
}

const RAW: [(&str, f64); 3] = [("r1", 100.0), ("r2", 110.0), ("r3", 120.0)];

/// 合格する成果物と case を作る。
fn fixture(split: CaseSplit) -> (Fx, GradeCase) {
    let tmp = tempfile::tempdir().unwrap();
    let case_dir = tmp.path().join("case");
    let art = tmp.path().join("artifact");
    let work = tmp.path().join("work");
    fs::create_dir_all(&work).unwrap();
    write(&case_dir.join("pristine/tests/add.rs"), PRISTINE_TEST);
    // 成果物 repo（実装と評価テストの写し）
    write(
        &art.join("repo/src/lib.rs"),
        "pub fn add(a: u64, b: u64) -> u64 { a + b }\n",
    );
    write(&art.join("repo/tests/add.rs"), PRISTINE_TEST);
    // 実行証拠と生データ
    let mut raw = Vec::new();
    let mut seen = Vec::new();
    for (i, (name, bw)) in RAW.iter().enumerate() {
        let rel = format!("raw/{name}/ior_result.json");
        let body = json!({"tests":[{"Results":[{"access":"write","bwMiB": bw}]}]}).to_string();
        write(&art.join(&rel), &body);
        let job = format!("{}.sirius", 42588 + i);
        write(
            &art.join(format!("evidence/{job}.qstat.txt")),
            &format!("Job Id: {job}\n    job_state = F\n    Exit_status = 0\n"),
        );
        write(&art.join(format!("evidence/{job}.log")), "IOR finished\n");
        seen.push(json!({
            "job_id": job, "exit_status": 0, "conditions": {"nodes": 3, "transport": "ucx"},
            "raw": [{"path": rel, "sha256": sha(body.as_bytes())}],
        }));
        raw.push(json!({
            "job_id": job,
            "scheduler_record": format!("evidence/{job}.qstat.txt"),
            "log": format!("evidence/{job}.log"),
            "raw": [{"path": rel, "sha256": sha(body.as_bytes())}],
            "conditions": {"nodes": 3, "transport": "ucx"},
            "evidence_kind": "real",
        }));
    }
    write(
        &art.join("evidence/jobs.json"),
        &json!({"jobs": raw}).to_string(),
    );
    let observed = tmp.path().join("observed.json");
    write(
        &observed,
        &json!({"schema": "celeris.observed-jobs/1", "jobs": seen}).to_string(),
    );
    write(
        &art.join("raw/baseline.csv"),
        "cond,xfer,bw\nbase,4k,50\nbase,4k,60\nbase,1m,999\n",
    );
    write(
        &art.join("report.md"),
        "# 結果\n\nwrite 帯域は改善した。\n\nmetric write_bw_mean = 110.0\nmetric write_bw_n = 3\n\
metric base_bw_median = 55\nmetric speedup = 2.0\n",
    );
    write(
        &art.join("review/findings.json"),
        &json!({"findings":[
            {"file":"src/rpc/data_ops.rs","line":1540,"summary":"FsyncChunk reply uses a different reply id"},
            {"file":"src/storage/chunk_store.rs","line":900,"summary":"POSIX fsync_chunk keeps data only in memory; lost after SIGKILL"},
            {"file":"jobs/p0.sh","line":12,"summary":"benchfsd log level is too verbose"},
        ]})
        .to_string(),
    );
    // 自己申告（grader は読まない）
    write(
        &art.join("result.json"),
        r#"{"summary":"all tests passed","outcome":"passed"}"#,
    );
    let case: GradeCase = serde_json::from_value(json!({
        "schema": "celeris.grade-case/1",
        "id": "T1-synthetic",
        "split": split,
        "implementation": {
            "pristine_dir": "pristine",
            "repo": "repo",
            "tests": [{"path": "tests/add.rs", "sha256": sha(PRISTINE_TEST.as_bytes())}],
            "command": ["cargo", "test", "--test", "add"],
            "must_pass": ["adds_small_numbers", "adds_large_numbers"],
        },
        "experiment": {"min_jobs": 3, "conditions": {"nodes": 3, "transport": "ucx"}},
        "recompute": {
            "report": "report.md",
            "pinned_raw": {"raw/baseline.csv": sha(b"cond,xfer,bw\nbase,4k,50\nbase,4k,60\nbase,1m,999\n")},
            "metrics": [
                {"name": "write_bw_mean", "agg": "mean", "tolerance_rel": 0.01,
                 "source": {"json": {"files": ["raw/r1/ior_result.json","raw/r2/ior_result.json","raw/r3/ior_result.json"],
                                     "pointer": "/tests/0/Results/0/bwMiB"}}},
                {"name": "write_bw_n", "agg": "count", "tolerance_rel": 0.0,
                 "source": {"json": {"files": ["raw/r1/ior_result.json","raw/r2/ior_result.json","raw/r3/ior_result.json"],
                                     "pointer": "/tests/0/Results/0/bwMiB"}}},
                {"name": "base_bw_median", "agg": "median", "tolerance_rel": 0.01,
                 "source": {"csv": {"files": ["raw/baseline.csv"], "column": "bw", "filter": {"xfer": "4k"}}}},
                {"name": "speedup", "tolerance_rel": 0.01,
                 "source": {"ratio": {"numerator": "write_bw_mean", "denominator": "base_bw_median"}}},
            ],
        },
        "review": {
            "known_defects": [
                {"id": "fsync-reply-id", "file": "src/rpc/data_ops.rs", "lines": [1500, 1600], "keywords": ["reply id", "reply stream"]},
                {"id": "posix-fsync-memory", "file": "src/storage/chunk_store.rs", "keywords": ["memory", "sigkill"]},
            ],
            "also_valid": [
                {"id": "verbose-log", "file": "jobs/p0.sh", "keywords": ["log level"]},
            ],
            "min_detection_rate": 1.0,
            "max_false_positives": 0,
        },
    }))
    .unwrap();
    case.validate().unwrap();
    (
        Fx {
            _tmp: tmp,
            case_dir,
            art,
            work,
            observed,
        },
        case,
    )
}

fn run(fx: &Fx, case: &GradeCase, stub: &Stub) -> GradeReport {
    let opts = GradeOptions {
        case_dir: fx.case_dir.clone(),
        work_root: fx.work.clone(),
        runner: stub,
        observed_jobs: Some(fx.observed.clone()),
        initial_tree: None,
    };
    grade(case, &fx.art, &opts)
}

fn reasons(r: &GradeReport) -> String {
    serde_json::to_string(&r.checks).unwrap()
}

#[test]
fn honest_artifact_passes_all_four_checks() {
    let (fx, case) = fixture(CaseSplit::Dev);
    let stub = Stub::ok();
    let r = run(&fx, &case, &stub);
    assert_eq!(r.verdict, Verdict::Pass, "{}", reasons(&r));
    assert!(r.final_pass);
    for c in [
        &r.checks.implementation,
        &r.checks.evidence,
        &r.checks.recompute,
        &r.checks.review,
    ] {
        assert_eq!(c.status, Status::Pass);
    }
    // 評価 command は pristine を重ねた作業 dir で 1 回だけ走り、作業 dir は消える
    let calls = stub.calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1, sha(PRISTINE_TEST.as_bytes()));
    assert_eq!(fs::read_dir(&fx.work).unwrap().count(), 0);
    let v: Value = serde_json::to_value(&r).unwrap();
    assert_eq!(v["schema"], "celeris.grade/1");
    assert_eq!(v["checks"]["review"]["details"]["detection_rate"], 1.0);
    assert_eq!(
        v["checks"]["recompute"]["details"]["metrics"][3]["recomputed"],
        2.0
    );
}

#[test]
fn success_claim_only_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let (fx, case) = fixture(CaseSplit::Dev);
    let art = tmp.path().join("claim");
    write(
        &art.join("report.md"),
        "# 完了\n\n全テスト合格。実験も成功し、帯域は 2 倍になった。\n",
    );
    write(
        &art.join("result.json"),
        r#"{"summary":"done","outcome":"passed","passed":true}"#,
    );
    let stub = Stub::ok();
    let opts = GradeOptions {
        case_dir: fx.case_dir.clone(),
        work_root: fx.work.clone(),
        runner: &stub,
        observed_jobs: None,
        initial_tree: None,
    };
    let r = grade(&case, &art, &opts);
    assert_eq!(r.verdict, Verdict::Fail);
    for c in [
        &r.checks.implementation,
        &r.checks.evidence,
        &r.checks.recompute,
        &r.checks.review,
    ] {
        assert_eq!(c.status, Status::Fail, "{}", reasons(&r));
    }
    assert!(stub.calls.borrow().is_empty());
}

#[test]
fn plausible_report_with_metric_lines_but_no_raw_data_fails() {
    let (fx, case) = fixture(CaseSplit::Dev);
    fs::remove_dir_all(fx.art.join("raw")).unwrap();
    let r = run(&fx, &case, &Stub::ok());
    assert_eq!(r.verdict, Verdict::Fail);
    assert_eq!(r.checks.evidence.status, Status::Fail);
    assert_eq!(r.checks.recompute.status, Status::Fail);
    assert!(reasons(&r).contains("cannot recompute from raw data"));
}

#[test]
fn deleted_evaluation_test_disqualifies_without_running() {
    let (fx, case) = fixture(CaseSplit::Dev);
    fs::remove_file(fx.art.join("repo/tests/add.rs")).unwrap();
    let stub = Stub::ok();
    let r = run(&fx, &case, &stub);
    assert_eq!(r.checks.implementation.status, Status::Fail);
    assert!(reasons(&r).contains("was deleted"));
    assert!(stub.calls.borrow().is_empty());
    assert!(!r.final_pass);
}

#[test]
fn weakened_evaluation_test_is_detected() {
    for (label, edit) in [
        (
            "ignore",
            PRISTINE_TEST.replacen(
                "#[test]\nfn adds_large",
                "#[test]\n#[ignore]\nfn adds_large",
                1,
            ),
        ),
        (
            "assert",
            PRISTINE_TEST.replace("    assert_eq!(add(0, 0), 0);\n", ""),
        ),
        (
            "cfg-skip",
            PRISTINE_TEST.replacen("#[test]", "#[cfg(any())]\n#[test]", 1),
        ),
    ] {
        let (fx, case) = fixture(CaseSplit::Dev);
        write(&fx.art.join("repo/tests/add.rs"), &edit);
        let stub = Stub::ok();
        let r = run(&fx, &case, &stub);
        assert_eq!(r.checks.implementation.status, Status::Fail, "{label}");
        let s = reasons(&r);
        assert!(
            s.contains("sha256 mismatch") && s.contains("weakened"),
            "{label}: {s}"
        );
        assert!(stub.calls.borrow().is_empty(), "{label}");
    }
}

#[test]
fn whitespace_only_change_is_still_a_mismatch() {
    let (fx, case) = fixture(CaseSplit::Dev);
    write(
        &fx.art.join("repo/tests/add.rs"),
        &format!("{PRISTINE_TEST}\n"),
    );
    let r = run(&fx, &case, &Stub::ok());
    assert_eq!(r.checks.implementation.status, Status::Fail);
    assert!(reasons(&r).contains("modified"));
}

#[test]
fn altered_grader_pristine_is_refused() {
    let (fx, case) = fixture(CaseSplit::Dev);
    write(
        &fx.case_dir.join("pristine/tests/add.rs"),
        "#[test]\nfn nothing() {}\n",
    );
    let r = run(&fx, &case, &Stub::ok());
    assert_eq!(r.checks.implementation.status, Status::Fail);
    assert!(reasons(&r).contains("grader-side copy altered"));
}

#[test]
fn failing_or_empty_test_run_fails() {
    let (fx, case) = fixture(CaseSplit::Dev);
    let failing = Stub {
        exit: 101,
        ..Stub::ok()
    };
    assert_eq!(
        run(&fx, &case, &failing).checks.implementation.status,
        Status::Fail
    );
    // exit 0 でも必須試験が ok を出していなければ不合格（0 件実行の抜け道）
    let empty = Stub {
        stdout: "running 0 tests\ntest result: ok. 0 passed\n".into(),
        ..Stub::ok()
    };
    let r = run(&fx, &case, &empty);
    assert_eq!(r.checks.implementation.status, Status::Fail);
    assert!(reasons(&r).contains("did not report ok"));
}

#[test]
fn mock_evidence_with_passed_record_fails_outside_control() {
    let (fx, case) = fixture(CaseSplit::Dev);
    let p = fx.art.join("evidence/jobs.json");
    let mut doc: Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
    for j in doc["jobs"].as_array_mut().unwrap() {
        j["evidence_kind"] = json!("mock");
    }
    fs::write(&p, doc.to_string()).unwrap();
    // mock runner の trial 記録（outcome passed）を置いても採点に使われない
    write(
        &fx.art.join("trials.jsonl"),
        "{\"outcome\":\"passed\",\"passed\":true}\n",
    );
    let r = run(&fx, &case, &Stub::ok());
    assert_eq!(r.checks.evidence.status, Status::Fail);
    assert!(reasons(&r).contains("evidence_kind=mock"));
    assert!(!r.final_pass);
}

#[test]
fn control_case_never_final_passes() {
    let (fx, mut case) = fixture(CaseSplit::Control);
    let p = fx.art.join("evidence/jobs.json");
    let mut doc: Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
    for j in doc["jobs"].as_array_mut().unwrap() {
        j["evidence_kind"] = json!("mock");
    }
    fs::write(&p, doc.to_string()).unwrap();
    let r = run(&fx, &case, &Stub::ok());
    assert_eq!(r.checks.evidence.status, Status::Pass, "{}", reasons(&r));
    assert_eq!(r.verdict, Verdict::ControlOnly);
    assert!(!r.final_pass);
    // 全検査が not_applicable の case も合格にしない
    case.split = CaseSplit::Dev;
    case.implementation = None;
    case.experiment = None;
    case.recompute = None;
    case.review = None;
    assert_eq!(run(&fx, &case, &Stub::ok()).verdict, Verdict::Fail);
}

#[test]
fn tampered_raw_data_fails_evidence_and_recompute() {
    let (fx, case) = fixture(CaseSplit::Dev);
    write(
        &fx.art.join("raw/r1/ior_result.json"),
        &json!({"tests":[{"Results":[{"access":"write","bwMiB": 400.0}]}]}).to_string(),
    );
    let r = run(&fx, &case, &Stub::ok());
    assert_eq!(r.checks.evidence.status, Status::Fail);
    assert!(reasons(&r).contains("sha256 differs from the recorded value"));
    assert_eq!(r.checks.recompute.status, Status::Fail);
    assert!(reasons(&r).contains("reported 110 but recomputed"));
}

#[test]
fn consistently_forged_pinned_input_is_caught_by_the_pin() {
    let (fx, case) = fixture(CaseSplit::Dev);
    // 固定入力を書き換え、報告もそれに合わせた（内部では辻褄が合う）捏造
    write(
        &fx.art.join("raw/baseline.csv"),
        "cond,xfer,bw\nbase,4k,5\nbase,4k,6\n",
    );
    let rep = fs::read_to_string(fx.art.join("report.md")).unwrap();
    let rep = rep
        .replace("metric base_bw_median = 55", "metric base_bw_median = 5.5")
        .replace("metric speedup = 2.0", "metric speedup = 20.0");
    write(&fx.art.join("report.md"), &rep);
    let r = run(&fx, &case, &Stub::ok());
    assert_eq!(r.checks.recompute.status, Status::Fail);
    let s = reasons(&r);
    assert!(s.contains("pinned snapshot"), "{s}");
    assert!(
        !s.contains("reported 20"),
        "numbers themselves are consistent: {s}"
    );
}

#[test]
fn fabricated_or_missing_reported_numbers_fail() {
    for (from, to, expect) in [
        (
            "metric speedup = 2.0",
            "metric speedup = 2.5",
            "reported 2.5 but recomputed 2",
        ),
        (
            "metric write_bw_n = 3",
            "metric write_bw_n = 4",
            "write_bw_n: reported 4",
        ),
        (
            "metric write_bw_mean = 110.0\n",
            "",
            "write_bw_mean: not reported",
        ),
        (
            "metric speedup = 2.0",
            "metric speedup = about 2x",
            "not a number",
        ),
    ] {
        let (fx, case) = fixture(CaseSplit::Dev);
        let rep = fs::read_to_string(fx.art.join("report.md")).unwrap();
        write(&fx.art.join("report.md"), &rep.replace(from, to));
        let r = run(&fx, &case, &Stub::ok());
        assert_eq!(r.checks.recompute.status, Status::Fail, "{to}");
        assert!(reasons(&r).contains(expect), "{to}: {}", reasons(&r));
    }
}

#[test]
fn experiment_condition_and_scheduler_mismatch_fail() {
    let (fx, case) = fixture(CaseSplit::Dev);
    let p = fx.art.join("evidence/jobs.json");
    let mut doc: Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
    doc["jobs"][0]["conditions"]["nodes"] = json!(2);
    doc["jobs"][1]["raw"][0]["path"] = json!("../../case/pristine/tests/add.rs");
    fs::write(&p, doc.to_string()).unwrap();
    write(
        &fx.art.join("evidence/42590.sirius.qstat.txt"),
        "qstat: Unknown Job Id 42590.sirius\n",
    );
    let r = run(&fx, &case, &Stub::ok());
    assert_eq!(r.checks.evidence.status, Status::Fail);
    let s = reasons(&r);
    assert!(
        s.contains("condition nodes = 2 but the case requires 3"),
        "{s}"
    );
    assert!(s.contains("must be relative and stay inside"), "{s}");
    assert!(s.contains("successful exit status"), "{s}");
    // job が足りない
    let mut doc: Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
    doc["jobs"].as_array_mut().unwrap().truncate(1);
    fs::write(&p, doc.to_string()).unwrap();
    assert!(reasons(&run(&fx, &case, &Stub::ok())).contains("requires at least 3"));
}

#[test]
fn false_findings_and_missed_defects_fail_review() {
    let (fx, case) = fixture(CaseSplit::Dev);
    let p = fx.art.join("review/findings.json");
    let mut doc: Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
    // list にも実在もしない指摘を 1 件足す
    doc["findings"]
        .as_array_mut()
        .unwrap()
        .push(json!({"file":"src/lib.rs","line":3,"summary":"possible race in the allocator"}));
    fs::write(&p, doc.to_string()).unwrap();
    let r = run(&fx, &case, &Stub::ok());
    assert_eq!(r.checks.review.status, Status::Fail);
    let v = serde_json::to_value(&r.checks.review.details).unwrap();
    assert_eq!(v["false_positives"].as_array().unwrap().len(), 1);
    assert!(reasons(&r).contains("false finding"));

    // 場所は合うが内容が違う指摘・defect id の自己申告だけでは検出に数えない
    let (fx, case) = fixture(CaseSplit::Dev);
    write(
        &p_of(&fx),
        &json!({"findings":[
            {"file":"src/rpc/data_ops.rs","line":1540,"summary":"FsyncChunk reply uses a different reply id"},
            {"file":"src/storage/chunk_store.rs","line":900,"summary":"naming is unclear","defect_id":"posix-fsync-memory"},
        ]})
        .to_string(),
    );
    let r = run(&fx, &case, &Stub::ok());
    assert_eq!(r.checks.review.status, Status::Fail);
    let v = serde_json::to_value(&r.checks.review.details).unwrap();
    assert_eq!(v["missed"], json!(["posix-fsync-memory"]));
    assert_eq!(v["detection_rate"], 0.5);
}

fn p_of(fx: &Fx) -> PathBuf {
    fx.art.join("review/findings.json")
}

fn bin() -> Command {
    Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("binary path"))
}

#[test]
fn cli_grade_outputs_json_and_exit_code() {
    let (fx, mut case) = fixture(CaseSplit::Dev);
    let case_path = fx.case_dir.join("case.json");
    // CLI は評価 command を実際に走らせる（ProcessRunner）。ここでは cargo の代わりに失敗する command。
    case.implementation.as_mut().unwrap().command = [
        "sh",
        "-c",
        "echo 'test adds_small_numbers ... FAILED'; exit 101",
    ]
    .map(String::from)
    .to_vec();
    fs::write(&case_path, serde_json::to_string(&case).unwrap()).unwrap();
    // 実装テストは exit 101 で fail、他の 3 検査は成果物から独立に pass し、最終判定は fail（exit 1）。
    let out = bin()
        .args(["grade", "--case"])
        .arg(&case_path)
        .arg("--artifact")
        .arg(&fx.art)
        .arg("--work")
        .arg(&fx.work)
        .arg("--observed")
        .arg(&fx.observed)
        .output()
        .unwrap();
    // 不合格の判定 JSON は stdout に出て exit 1。stderr に判定 JSON は出ない（ADR ops-r2 付記 H7）。
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("\"verdict\""), "{err}");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["verdict"], "fail");
    assert_eq!(v["final_pass"], false);
    assert_eq!(v["checks"]["implementation"]["status"], "fail");
    let reasons = v["checks"]["implementation"]["reasons"].to_string();
    assert!(reasons.contains("exited with Some(101)"), "{reasons}");
    assert_eq!(v["checks"]["implementation"]["details"]["ran"], true);
    for k in ["evidence", "recompute", "review"] {
        assert_eq!(v["checks"][k]["status"], "pass", "{k}: {}", v["checks"][k]);
    }

    // 評価 command が作業 dir（pristine を重ねた複製）で実際に走り、必要な試験が ok なら合格（exit 0）。
    case.implementation.as_mut().unwrap().command = [
        "sh",
        "-c",
        "test -f tests/add.rs && printf 'test adds_small_numbers ... ok\\ntest adds_large_numbers ... ok\\n'",
    ]
    .map(String::from)
    .to_vec();
    fs::write(&case_path, serde_json::to_string(&case).unwrap()).unwrap();
    let ok = bin()
        .args(["grade", "--case"])
        .arg(&case_path)
        .arg("--artifact")
        .arg(&fx.art)
        .arg("--work")
        .arg(&fx.work)
        .arg("--observed")
        .arg(&fx.observed)
        .output()
        .unwrap();
    let so = String::from_utf8_lossy(&ok.stdout);
    assert_eq!(
        ok.status.code(),
        Some(0),
        "{so} {}",
        String::from_utf8_lossy(&ok.stderr)
    );
    let v: Value = serde_json::from_str(&so).unwrap();
    assert_eq!(v["final_pass"], true);
    assert_eq!(v["checks"]["implementation"]["status"], "pass");

    // 成功宣言だけの成果物は exit 1、判定 JSON は stdout に出る
    let claim = fx.work.join("claim");
    write(&claim.join("report.md"), "# 完了\n全て成功\n");
    let out = bin()
        .args(["grade", "--case"])
        .arg(&case_path)
        .arg("--artifact")
        .arg(&claim)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("\"verdict\""));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["verdict"], "fail");
    for k in ["implementation", "evidence", "recompute", "review"] {
        assert_eq!(v["checks"][k]["status"], "fail", "{k}");
    }
}

#[test]
fn cli_grade_error_is_exit_2_with_empty_stdout() {
    // 判定 JSON を作れないエラー（grade-case が無い）は stdout 空・stderr に理由・exit 2（不合格の 1 と別）。
    let (fx, _case) = fixture(CaseSplit::Dev);
    let out = bin()
        .args(["grade", "--case"])
        .arg(fx.case_dir.join("no-such-case.json"))
        .arg("--artifact")
        .arg(&fx.art)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty(), "{}", String::from_utf8_lossy(&out.stdout));
    assert!(!out.stderr.is_empty());
}

fn observed_doc(fx: &Fx) -> Value {
    serde_json::from_str(&fs::read_to_string(&fx.observed).unwrap()).unwrap()
}

fn run_with(fx: &Fx, case: &GradeCase, observed: Option<PathBuf>) -> GradeReport {
    let stub = Stub::ok();
    grade(
        case,
        &fx.art,
        &GradeOptions {
            case_dir: fx.case_dir.clone(),
            work_root: fx.work.clone(),
            runner: &stub,
            observed_jobs: observed,
            initial_tree: None,
        },
    )
}

/// エージェントが書いた整合的な自己申告（job id・終了記録・hash・ログが互いに辻褄が合う）だけでは通らない。
#[test]
fn self_reported_experiment_without_evaluator_records_fails() {
    let (fx, case) = fixture(CaseSplit::Dev);
    assert!(run_with(&fx, &case, Some(fx.observed.clone())).final_pass);
    let r = run_with(&fx, &case, None);
    assert_eq!(r.checks.evidence.status, Status::Fail);
    assert!(reasons(&r).contains("no evaluator-collected job records"));
    assert!(!r.final_pass);
}

#[test]
fn job_ids_absent_from_the_evaluator_records_fail() {
    let (fx, case) = fixture(CaseSplit::Dev);
    let mut o = observed_doc(&fx);
    o["jobs"].as_array_mut().unwrap().remove(1);
    write(&fx.observed, &o.to_string());
    let r = run_with(&fx, &case, Some(fx.observed.clone()));
    assert_eq!(r.checks.evidence.status, Status::Fail);
    assert!(reasons(&r).contains("42589.sirius: job id was not recorded by the evaluator"));
    assert!(!r.final_pass);
}

#[test]
fn evaluator_exit_condition_and_raw_hash_mismatch_fail() {
    let (fx, case) = fixture(CaseSplit::Dev);
    let mut o = observed_doc(&fx);
    o["jobs"][0]["exit_status"] = json!(271);
    o["jobs"][1]["conditions"]["nodes"] = json!(1);
    o["jobs"][2]["raw"][0]["sha256"] = json!("0".repeat(64));
    write(&fx.observed, &o.to_string());
    let r = run_with(&fx, &case, Some(fx.observed.clone()));
    assert_eq!(r.checks.evidence.status, Status::Fail);
    let s = reasons(&r);
    assert!(s.contains("evaluator-recorded exit status is 271"), "{s}");
    assert!(
        s.contains("evaluator-recorded condition nodes = 1 but the case requires 3"),
        "{s}"
    );
    assert!(
        s.contains("does not match the evaluator-collected hash"),
        "{s}"
    );
}

/// 成果物と生データを自己申告どおりに作り直して hash を合わせても、評価側が取得した hash とは一致しない。
#[test]
fn consistently_rewritten_raw_data_does_not_match_collected_hash() {
    let (fx, case) = fixture(CaseSplit::Dev);
    let rel = "raw/r1/ior_result.json";
    let body = json!({"tests":[{"Results":[{"access":"write","bwMiB": 400.0}]}]}).to_string();
    write(&fx.art.join(rel), &body);
    let p = fx.art.join("evidence/jobs.json");
    let mut doc: Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
    doc["jobs"][0]["raw"][0]["sha256"] = json!(sha(body.as_bytes()));
    write(&p, &doc.to_string());
    let r = run_with(&fx, &case, Some(fx.observed.clone()));
    assert_eq!(r.checks.evidence.status, Status::Fail);
    assert!(reasons(&r).contains("does not match the evaluator-collected hash"));
}
