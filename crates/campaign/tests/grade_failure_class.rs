//! 独立採点の不合格の原因（model_failure / env_failure / undetermined）が bind → checker → export まで保たれ、
//! final_pass の bool に潰れないことと、再採点で分母・消費・履歴が不当に増減しないことの試験。
mod common;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use celeris_campaign::{
    checker::check,
    executor::MockExecutor,
    export::export,
    grade_out::{GRADES_FILE, GRADES_HISTORY_FILE, grade_out},
    grader::{CommandOutput, CommandRunner},
    manifest::CampaignManifest,
    runner::{RunOptions, run},
};
use serde_json::{Value, json};

fn fx() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn pilot_out(dir: &Path) -> (CampaignManifest, PathBuf) {
    pilot_out_with_faults(dir, false)
}

fn pilot_out_with_faults(dir: &Path, faults: bool) -> (CampaignManifest, PathBuf) {
    let mp = fx().join("pilot.json");
    let mut m = CampaignManifest::load(&mp).expect("pilot");
    if faults {
        m.problems = m.secondary.subset.clone();
        m.fault_scenarios
            .insert(m.problems[0].clone(), "worker-stop/v1".into());
    }
    m.sha256 = m.compute_sha256().unwrap();
    let i = m.load_input(&mp, None).expect("input");
    let out = dir.join("out");
    let o = RunOptions {
        public_cases: Default::default(),
        global_ledger: None,
        out: out.clone(),
        repo_src: Some(fx().join("repo")),
        materials_src: Some(fx().join("materials")),
        kb_src: Some(fx().join("kb")),
        inputs_src: None,
        case_inputs: Default::default(),
        lock: None,
    };
    run(&m, &i, &MockExecutor::seeded(), &o).expect("run");
    (m, out)
}

fn totals(m: &CampaignManifest, out: &Path) -> Value {
    serde_json::to_value(check(m, out).recomputed.expect("rc").totals).expect("json")
}

fn sum(t: &Value, k: &str) -> u64 {
    t.as_object()
        .unwrap()
        .values()
        .map(|c| c[k].as_u64().unwrap())
        .sum()
}

#[test]
fn failure_classes_survive_bind_checker_and_export() {
    for (class, key) in [
        ("model_failure", "grader_failed"),
        ("env_failure", "grader_env_failure"),
        ("undetermined", "grader_undetermined"),
    ] {
        let d = tempfile::tempdir().expect("dir");
        let (m, out) = pilot_out(d.path());
        common::write_grades_classed(&m, &out, |_| Some(class));
        let rep = check(&m, &out);
        assert!(rep.ok, "{class}: {:?}", rep.violations);
        let t = totals(&m, &out);
        assert_eq!(sum(&t, "final_passed"), 0, "{class}");
        assert_eq!(sum(&t, key), sum(&t, "passed"), "{class}: {t}");
        for other in ["grader_failed", "grader_env_failure", "grader_undetermined"] {
            if other != key {
                assert_eq!(sum(&t, other), 0, "{class}/{other}");
            }
        }
        let unresolved = class != "model_failure";
        assert_eq!(
            rep.complete, !unresolved,
            "{class}: {:?}",
            rep.incomplete_reasons
        );
        let table = export(&m, &out, &out.join("ex")).expect("export");
        assert_eq!(table["complete"], !unresolved, "{class}");
        assert_eq!(out.join("ex/table.json").exists(), !unresolved);
        for row in table["configs"].as_array().unwrap() {
            // 固定分母: n は変わらず、原因別内訳が別に出る。
            let bk = if key == "grader_failed" {
                "model_failure"
            } else {
                key
            };
            assert_eq!(row["failure_breakdown"][bk], row[key], "{class}");
            assert_eq!(row["failures"], row["n"], "{class}");
        }
    }
}

#[test]
fn a_failed_grade_without_failure_class_is_rejected_not_defaulted_to_model_failure() {
    let d = tempfile::tempdir().expect("dir");
    let (m, out) = pilot_out(d.path());
    common::write_grades_classed(&m, &out, |_| Some("model_failure"));
    let text = fs::read_to_string(out.join(GRADES_FILE)).unwrap();
    let mut lines: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    lines[0]["grade"]["failure_class"] = Value::Null;
    let s: String = lines.iter().map(|l| format!("{l}\n")).collect();
    fs::write(out.join(GRADES_FILE), s).unwrap();
    let rep = check(&m, &out);
    assert!(!rep.ok);
    assert!(
        rep.violations.iter().any(|v| v.contains("failure_class")),
        "{:?}",
        rep.violations
    );
}

/// 実 grader（grade_out → grade）を通す。runner の出力だけを変える。
struct Canned(Mutex<u32>, CommandOutput);
impl CommandRunner for Canned {
    fn run(&self, _argv: &[String], _cwd: &Path) -> Result<CommandOutput, String> {
        *self.0.lock().unwrap() += 1;
        Ok(self.1.clone())
    }
}

fn canned(exit: i32, stdout: &str, stderr: &str) -> Canned {
    Canned(
        Mutex::new(0),
        CommandOutput {
            exit: Some(exit),
            stdout: stdout.into(),
            stderr: stderr.into(),
        },
    )
}

const OK: &str =
    "test chunk_zero ... ok\ntest chunk_last_in_first ... ok\ntest chunk_second ... ok\n";

fn impl_total(t: &Value, k: &str) -> u64 {
    sum(t, k)
}

#[test]
fn real_grader_env_failure_is_not_a_model_failure_and_regrade_keeps_denominator_cost_and_history() {
    let d = tempfile::tempdir().expect("dir");
    let (m, out) = pilot_out(d.path());
    let cases = fx().join("grade-cases/pilot");
    let trials_before = fs::read(out.join("trials.jsonl")).unwrap();
    let base = totals(&m, &out);

    // 1) 採点環境の不良（libclang 無し）: env_failure。成功にも成果物の失敗にも数えない。
    let env = canned(
        101,
        "",
        "error: failed to run custom build command for `x`\n  Unable to find libclang\n",
    );
    let v = grade_out(&m, &out, &cases, &env).expect("grade-out");
    assert_eq!(v["failure_classes"]["env_failure"], 3, "{v}");
    let t1 = totals(&m, &out);
    assert_eq!(impl_total(&t1, "grader_env_failure"), 3);
    // pilot の review case（3 試行）は runner を使わず mock 成果物が実際に不合格。環境不良の 3 件はここに混ざらない。
    assert_eq!(
        impl_total(&t1, "grader_failed"),
        3,
        "環境不良を成果物の失敗に混ぜない"
    );
    let rep = check(&m, &out);
    assert!(rep.ok && !rep.complete);
    assert!(
        rep.incomplete_reasons
            .iter()
            .any(|r| r.contains("env_failure")),
        "{:?}",
        rep.incomplete_reasons
    );
    assert!(export(&m, &out, &out.join("ex")).expect("export")["complete"] == false);
    assert!(!out.join("ex/table.json").exists());

    // 2) 環境を直して再採点: 成果物の失敗（FAILED）。
    let model = canned(101, "test chunk_zero ... FAILED\n", "");
    let v = grade_out(&m, &out, &cases, &model).expect("regrade");
    assert_eq!(v["failure_classes"]["model_failure"], 6, "{v}");
    let t2 = totals(&m, &out);
    assert_eq!(impl_total(&t2, "grader_failed"), 6);
    assert_eq!(impl_total(&t2, "grader_env_failure"), 0);

    // 3) 合格で再採点。
    let ok = canned(0, OK, "");
    grade_out(&m, &out, &cases, &ok).expect("regrade");
    let t3 = totals(&m, &out);
    assert_eq!(impl_total(&t3, "final_passed"), 3);
    assert_eq!(impl_total(&t3, "grader_failed"), 3);

    // 再採点は分母・消費・実行記録を動かさない。
    for t in [&t1, &t2, &t3] {
        for k in [
            "n",
            "passed",
            "tokens_sum",
            "cost_micro_usd_sum",
            "ran_trials",
            "wall_secs_sum",
            "missing",
        ] {
            assert_eq!(sum(t, k), sum(&base, k), "{k} changed by re-grading");
        }
    }
    assert_eq!(fs::read(out.join("trials.jsonl")).unwrap(), trials_before);
    // 最新の grades.jsonl は 1 試行 1 行のまま（二重に数えない）、置き換えた行は履歴に残る。
    let n = fs::read_to_string(out.join(GRADES_FILE))
        .unwrap()
        .lines()
        .count();
    assert_eq!(
        n,
        fs::read_to_string(out.join("trials.jsonl"))
            .unwrap()
            .lines()
            .count()
    );
    let hist = fs::read_to_string(out.join(GRADES_HISTORY_FILE)).unwrap();
    assert_eq!(hist.lines().count(), 2 * n, "2 回分の置き換え前の行");
    assert!(hist.contains("\"env_failure\"") && hist.contains("\"model_failure\""));
    let _ = json!(null);
}

fn fault_records(m: &CampaignManifest, out: &Path, status: &str) -> Vec<Value> {
    use sha2::{Digest, Sha256};
    use std::os::unix::process::ExitStatusExt;
    let status_log = if status == "injected" {
        let exit = std::process::Command::new("/bin/sh")
            .args(["-c", "kill -TERM $$"])
            .status()
            .unwrap();
        assert_eq!(exit.signal(), Some(15));
        format!(
            "test injector: worker stopped; observed exit signal {:?}\n",
            exit.signal()
        )
    } else {
        "test injector: trigger not reached; no signal sent\n".into()
    };
    let log = status_log.as_bytes();
    fs::write(out.join("injector.log"), log).unwrap();
    fs::read_to_string(out.join("trials.jsonl")).unwrap().lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|r| m.fault_scenarios.contains_key(r["problem_id"].as_str().unwrap()))
        .map(|r| json!({
            "schema": "celeris.campaign-fault/1", "campaign_sha256": m.compute_sha256().unwrap(),
            "config": r["config"], "problem_id": r["problem_id"], "rep": r["rep"], "trial_no": r["trial_no"],
            "scenario": "worker-stop/v1", "status": status,
            "evidence": "injector.log", "evidence_sha256": format!("{:x}", Sha256::digest(log)),
        })).collect()
}
fn write_faults(out: &Path, rows: &[Value]) {
    fs::write(
        out.join("faults.jsonl"),
        rows.iter().map(|v| format!("{v}\n")).collect::<String>(),
    )
    .unwrap();
}

#[test]
fn injected_fault_is_an_independent_axis_through_real_grader_check_and_export() {
    let d = tempfile::tempdir().unwrap();
    let (m, out) = pilot_out_with_faults(d.path(), true);
    let observations = fault_records(&m, &out, "injected");
    write_faults(&out, &observations);
    let trials = fs::read(out.join("trials.jsonl")).unwrap();
    let faults = fs::read(out.join("faults.jsonl")).unwrap();
    let base = totals(&m, &out);
    for (runner, complete, result) in [
        (
            canned(
                101,
                "",
                "error: failed to run custom build command for `x`\nUnable to find libclang\n",
            ),
            false,
            "grader_env_failure",
        ),
        (
            canned(101, "test chunk_zero ... FAILED\n", ""),
            true,
            "model_failure",
        ),
        (canned(0, OK, ""), true, "final_passed"),
    ] {
        grade_out(&m, &out, &fx().join("grade-cases/pilot"), &runner).unwrap();
        let rep = check(&m, &out);
        assert!(rep.ok, "{:?}", rep.violations);
        assert_eq!(rep.complete, complete, "{:?}", rep.incomplete_reasons);
        let table = export(&m, &out, &out.join("ex")).unwrap();
        assert_eq!(table["fault_observations"], json!(observations));
        for row in table["configs"].as_array().unwrap() {
            assert_eq!(row["fault_breakdown"]["injected"][result], 1);
            let counts = row["fault_breakdown"].as_object().unwrap();
            let sum: u64 = counts
                .values()
                .flat_map(|c| c.as_object().unwrap().values())
                .map(|n| n.as_u64().unwrap())
                .sum();
            assert_eq!(sum, row["n"].as_u64().unwrap());
            assert_eq!(
                counts["injected"]
                    .as_object()
                    .unwrap()
                    .values()
                    .map(|n| n.as_u64().unwrap())
                    .sum::<u64>(),
                1
            );
            assert_eq!(
                counts["not_planned"]
                    .as_object()
                    .unwrap()
                    .values()
                    .map(|n| n.as_u64().unwrap())
                    .sum::<u64>(),
                row["n"].as_u64().unwrap() - 1
            );
        }
        assert_eq!(fs::read(out.join("trials.jsonl")).unwrap(), trials);
        assert_eq!(fs::read(out.join("faults.jsonl")).unwrap(), faults);
        let now = totals(&m, &out);
        for key in [
            "n",
            "tokens_sum",
            "cost_micro_usd_sum",
            "ran_trials",
            "wall_secs_sum",
        ] {
            assert_eq!(sum(&now, key), sum(&base, key));
        }
    }
    let csv = fs::read_to_string(out.join("ex/table.csv")).unwrap();
    assert!(csv.contains("fault_breakdown") && csv.contains("injected"));
}

#[test]
fn fault_declaration_without_confirmed_injection_is_incomplete_and_bad_binding_is_rejected() {
    let d = tempfile::tempdir().unwrap();
    let (m, out) = pilot_out_with_faults(d.path(), true);
    common::write_grades_classed(&m, &out, |_| None);
    let rep = check(&m, &out);
    assert!(rep.ok && !rep.complete);
    assert!(
        rep.incomplete_reasons
            .iter()
            .any(|r| r.contains("unobserved"))
    );
    let rows = fault_records(&m, &out, "not_injected");
    write_faults(&out, &rows);
    let table = export(&m, &out, &out.join("ex")).unwrap();
    assert_eq!(table["complete"], false);
    assert!(!out.join("ex/table.json").exists());
    for (field, value) in [
        ("campaign_sha256", json!("wrong")),
        ("scenario", json!("undeclared")),
        ("trial_no", json!(999)),
        ("status", json!("unknown")),
        ("evidence_sha256", json!("wrong")),
        ("evidence", json!("../outside")),
    ] {
        let mut bad = rows.clone();
        bad[0][field] = value;
        write_faults(&out, &bad);
        let rep = check(&m, &out);
        assert!(!rep.ok, "{field}");
        assert!(export(&m, &out, &out.join("ex")).is_err(), "{field}");
    }
    let mut duplicate = rows.clone();
    duplicate.push(rows[0].clone());
    write_faults(&out, &duplicate);
    assert!(!check(&m, &out).ok);
    write_faults(&out, &rows);
    fs::write(out.join("injector.log"), "changed").unwrap();
    assert!(!check(&m, &out).ok);
    let mut invalid = m.clone();
    invalid
        .fault_scenarios
        .insert("unknown-problem".into(), "fault/v1".into());
    assert!(invalid.validate().is_err());
    assert_ne!(m.compute_sha256().unwrap(), {
        let mut normal = m.clone();
        normal.fault_scenarios.clear();
        normal.compute_sha256().unwrap()
    });
}
