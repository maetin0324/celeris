//! 実行器の passed と独立採点を別に持ち、論文用の成功は両方が揃ったときだけにする試験。
//! 結合キー（campaign hash・case id・split・arm・problem・rep・trial_no）と成果物 hash の不一致・重複は拒否する。
mod common;
use std::{
    fs,
    path::{Path, PathBuf},
};

use celeris_campaign::{
    checker::check,
    executor::MockExecutor,
    export::export,
    grades::GRADES_FILE,
    manifest::CampaignManifest,
    runner::{RunOptions, run},
};
use serde_json::{Value, json};

fn fx() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn pilot_out(dir: &Path) -> (CampaignManifest, PathBuf) {
    let mp = fx().join("pilot.json");
    let m = CampaignManifest::load(&mp).expect("pilot");
    let i = m.load_input(&mp, None).expect("input");
    let out = dir.join("out");
    let o = RunOptions {
        out: out.clone(),
        repo_src: Some(fx().join("repo")),
        materials_src: Some(fx().join("materials")),
        kb_src: Some(fx().join("kb")),
        inputs_src: None,
        case_inputs: Default::default(),
        lock: None,
        series: None,
    };
    run(&m, &i, &MockExecutor::seeded(), &o).expect("run");
    (m, out)
}

fn totals(m: &CampaignManifest, out: &Path) -> Value {
    serde_json::to_value(check(m, out).recomputed.expect("rc").totals).expect("json")
}

fn grade_lines(out: &Path) -> Vec<Value> {
    fs::read_to_string(out.join(GRADES_FILE))
        .expect("grades")
        .lines()
        .map(|l| serde_json::from_str(l).expect("json"))
        .collect()
}

fn write_lines(out: &Path, v: &[Value]) {
    let s: String = v.iter().map(|l| format!("{l}\n")).collect();
    fs::write(out.join(GRADES_FILE), s).expect("write");
}

#[test]
fn executor_pass_with_a_failing_grade_is_not_a_success_in_the_final_json_and_csv() {
    let d = tempfile::tempdir().expect("dir");
    let (m, out) = pilot_out(d.path());
    common::write_grades(&m, &out, |_| false);
    let c = check(&m, &out);
    assert!(
        c.ok && c.complete,
        "{:?} {:?}",
        c.violations,
        c.incomplete_reasons
    );
    let t = export(&m, &out, &out.join("ex")).expect("export");
    assert_eq!(t["complete"], true);
    for row in t["configs"].as_array().expect("rows") {
        assert!(
            row["passed"].as_u64().unwrap() > 0,
            "executor still says passed"
        );
        assert_eq!(row["final_passed"], 0, "{row}");
        assert_eq!(row["grader_failed"], row["passed"]);
        assert_eq!(row["pass_rate"].as_f64(), Some(0.0));
        assert_eq!(row["failures"], row["n"], "denominator is kept");
    }
    let csv = fs::read_to_string(out.join("ex/table.csv")).expect("csv");
    assert!(csv.lines().skip(1).all(|l| l.contains(",0.0000,")), "{csv}");
    // 比較表の成功も採点結果から数える。
    assert_eq!(t["comparisons"][0]["both_pass"], 0);
}

#[test]
fn ungraded_is_neither_success_nor_excluded_and_blocks_the_final_table() {
    let d = tempfile::tempdir().expect("dir");
    let (m, out) = pilot_out(d.path());
    // grades.jsonl が無い: 実行器 passed でも成功 0、分母は残り、最終 table は出ない。
    let t0 = totals(&m, &out);
    for (_, c) in t0.as_object().unwrap() {
        assert_eq!(c["final_passed"], 0);
        assert_eq!(c["ungraded"], c["passed"]);
    }
    let t = export(&m, &out, &out.join("ex")).expect("export");
    assert_eq!(t["complete"], false);
    assert!(!t["incomplete_reasons"].as_array().unwrap().is_empty());
    assert!(out.join("ex/table.incomplete.json").exists());
    assert!(out.join("ex/table.incomplete.csv").exists());
    assert!(!out.join("ex/table.json").exists() && !out.join("ex/table.csv").exists());
    // 一部だけ採点: 残りは未採点のまま（成功にならない）。
    common::write_grades(&m, &out, |_| true);
    let mut v = grade_lines(&out);
    v.truncate(1);
    write_lines(&out, &v);
    let rep = check(&m, &out);
    assert!(rep.ok && !rep.complete);
    let all: u64 = totals(&m, &out)
        .as_object()
        .unwrap()
        .values()
        .map(|c| c["final_passed"].as_u64().unwrap())
        .sum();
    assert_eq!(all, 1);
    // 採点が揃うと最終版が出て、古い incomplete は消える。
    common::write_grades(&m, &out, |_| true);
    let t = export(&m, &out, &out.join("ex")).expect("export");
    assert_eq!(t["complete"], true);
    assert!(out.join("ex/table.json").exists() && !out.join("ex/table.incomplete.json").exists());
}

fn corrupt(name: &str, f: impl Fn(&mut Vec<Value>)) {
    let d = tempfile::tempdir().expect("dir");
    let (m, out) = pilot_out(d.path());
    common::write_grades(&m, &out, |_| true);
    let mut v = grade_lines(&out);
    f(&mut v);
    write_lines(&out, &v);
    let c = check(&m, &out);
    assert!(!c.ok, "{name}: bad grade must be a violation");
    assert!(
        c.violations.iter().any(|x| x.contains("grades.jsonl")),
        "{name}: {:?}",
        c.violations
    );
    assert!(
        export(&m, &out, &out.join("ex")).is_err(),
        "{name}: export must refuse"
    );
    // 拒否した grade は成功に数えない。
    let before: u64 = totals(&m, &out)
        .as_object()
        .unwrap()
        .values()
        .map(|c| c["final_passed"].as_u64().unwrap())
        .sum();
    let n_ran = grade_lines(&out).len() as u64;
    assert!(before < n_ran, "{name}: rejected grade counted as success");
}

#[test]
fn stale_foreign_or_duplicate_grades_are_rejected() {
    corrupt("stale artifact", |v| {
        v[0]["artifact_sha256"] = json!("deadbeef");
    });
    corrupt("other case id", |v| {
        v[0]["case_id"] = json!("another-case");
    });
    corrupt("other grade case id", |v| {
        v[0]["grade"]["case_id"] = json!("another-case");
    });
    corrupt("other split", |v| {
        v[0]["grade"]["split"] = json!("final");
        v[0]["split"] = json!("final");
    });
    corrupt("other campaign", |v| {
        v[0]["campaign_sha256"] = json!("0".repeat(64));
    });
    corrupt("unknown trial", |v| {
        v[0]["trial"]["rep"] = json!(99);
    });
    corrupt("duplicate", |v| {
        let x = v[0].clone();
        v.push(x);
    });
    corrupt("missing grader hash", |v| {
        v[0]["grader_sha256"] = json!("");
    });
}

#[test]
fn failed_timeout_cancelled_and_ungraded_stay_in_the_denominator() {
    let d = tempfile::tempdir().expect("dir");
    let (m, out) = pilot_out(d.path());
    common::write_grades(&m, &out, |_| true);
    let t = totals(&m, &out);
    for (cfg, c) in t.as_object().unwrap() {
        let g = |k: &str| c[k].as_u64().unwrap();
        // 分母 n は常に全 cell。成功以外の全状態が残りを占める。
        assert_eq!(
            g("n"),
            g("final_passed")
                + g("grader_failed")
                + g("ungraded")
                + g("failed")
                + g("error")
                + g("timeout")
                + g("cancelled")
                + g("blocked")
                + g("missing"),
            "{cfg}: {c}"
        );
    }
}
