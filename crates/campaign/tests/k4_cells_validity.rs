//! K4 のセル有効性採点（ADR 2026-10-23-k4-cells-validity-grading）。合成 raw を sha256 固定の tgz にして
//! grader 自身に展開させ、正しい解は通り、既知の誤答（全セル有効・実効 fsync=0 を有効・失敗 job を有効・
//! 欠測を分母から除外・表と報告の不一致・有効セルの数値誤り）は落ちることを無 LLM で確かめる。
//! 実 K4 は公開入力 v2（`main-v3-n2-43706-public2.tgz`、ADR 2026-10-25-k4-public-input-v2）を `CAMPAIGN_K4_TGZ` で渡す。
use std::{collections::BTreeMap, fs, path::Path, process::Command};

use celeris_campaign::{
    cells_validity::{
        self, Expected, JobState, parse_qstat,
        synth::{self, Cell, Defect, Plan},
    },
    grader::{FailureClass, GradeCase, GradeOptions, NoRunner, Status, grade},
};
use serde_json::json;
use sha2::{Digest, Sha256};

struct Fx {
    _t: tempfile::TempDir,
    case_dir: std::path::PathBuf,
    art: std::path::PathBuf,
    work: std::path::PathBuf,
    case: GradeCase,
    expected: Expected,
}

fn sha(p: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(p).expect("read")))
}

fn fixture(plan: &Plan) -> Fx {
    let t = tempfile::tempdir().expect("tmp");
    let raw = t.path().join("raw");
    synth::write(&raw.join("inputs"), plan).expect("synth");
    let case_dir = t.path().join("case");
    fs::create_dir_all(&case_dir).expect("case");
    let tgz = case_dir.join("inputs.tgz");
    let st = Command::new("tar")
        .arg("-czf")
        .arg(&tgz)
        .arg("-C")
        .arg(raw.join("inputs"))
        .arg(".")
        .status()
        .expect("tar");
    assert!(st.success());
    let spec = synth::spec(plan, "inputs");
    let planned_rel = format!("{}/planned-cells.json", spec.run);
    let case_json = json!({
        "schema": "celeris.grade-case/1", "id": "k4-synth", "split": "final",
        "recompute": {
            "report": "report.md",
            "input_archive": {"path": "inputs.tgz", "sha256": sha(&tgz)},
            "pinned_raw": {planned_rel.clone(): sha(&raw.join(&planned_rel))},
            "metrics": [],
            "cells": spec,
        }
    });
    let cp = case_dir.join("k4-synth.json");
    fs::write(&cp, case_json.to_string()).expect("case json");
    let case = GradeCase::load(&cp).expect("load");
    let expected = cells_validity::compute(&raw, &spec).expect("expected");
    let art = t.path().join("artifact");
    fs::create_dir_all(art.join("results")).expect("art");
    fs::write(art.join("report.md"), expected.metric_lines()).expect("report");
    fs::write(art.join("results/cells.csv"), expected.csv()).expect("csv");
    let work = t.path().join("work");
    fs::create_dir_all(&work).expect("work");
    Fx {
        _t: t,
        case_dir,
        art,
        work,
        case,
        expected,
    }
}

fn run(fx: &Fx) -> celeris_campaign::grader::GradeReport {
    grade(
        &fx.case,
        &fx.art,
        &GradeOptions {
            case_dir: fx.case_dir.clone(),
            work_root: fx.work.clone(),
            runner: &NoRunner,
            observed_jobs: None,
            initial_tree: None,
        },
    )
}

fn recompute(fx: &Fx) -> (Status, Vec<String>) {
    let r = run(fx);
    (r.checks.recompute.status, r.checks.recompute.reasons)
}

/// 表の valid 列を書き換え、報告の件数も表に合わせる（「表どおりの報告」を作る）。
fn rewrite(fx: &Fx, f: impl Fn(&celeris_campaign::cells_validity::CellRow) -> Option<bool>) {
    let mut exp = fx.expected.clone();
    for r in &mut exp.rows {
        if let Some(v) = f(r) {
            r.valid = v;
        }
    }
    fs::write(fx.art.join("results/cells.csv"), exp.csv()).expect("csv");
    fs::write(fx.art.join("report.md"), exp.metric_lines()).expect("report");
}

fn like_real() -> Plan {
    // 実 43706 と同じ形: BenchFS W2 rep1 は途中で切れ、rep2 は未実行、job は F / Exit -20。
    let mut p = synth::healthy();
    p.cells.push(Cell {
        fs: "benchfs",
        rep: 2,
        workload: "w2",
        job: "1001",
        defect: Defect::Absent,
    });
    for c in &mut p.cells {
        if c.fs == "benchfs" && c.workload == "w2" && c.rep == 1 {
            c.defect = Defect::Truncated;
        }
    }
    p.jobs
        .insert("1001", Some(synth::qstat("1001", "F", Some(-20))));
    p
}

#[test]
fn correct_solution_passes_and_missing_cells_stay_in_the_denominator() {
    let fx = fixture(&like_real());
    let c = fx.expected.counts();
    assert_eq!(c["cells_total"], 13.0);
    assert_eq!(c["cells_valid"], 11.0);
    assert_eq!(c["cells_valid_benchfs"], 2.0);
    assert_eq!(c["cells_total_benchfs"], 4.0);
    let bad: Vec<_> = fx.expected.rows.iter().filter(|r| !r.valid).collect();
    assert_eq!(bad.len(), 2);
    assert!(bad.iter().all(|r| r.reasons[0] == "missing"));
    assert!(
        bad.iter()
            .all(|r| r.reasons.iter().any(|x| x == "job_failed(exit=-20)"))
    );
    let r = run(&fx);
    assert_eq!(
        r.checks.recompute.status,
        Status::Pass,
        "{:?}",
        r.checks.recompute.reasons
    );
    assert!(r.final_pass);
    assert_eq!(
        fs::read_dir(&fx.work).expect("work").count(),
        0,
        "grader の展開先は残さない"
    );
}

#[test]
fn all_valid_claim_with_correct_file_counts_is_rejected() {
    let fx = fixture(&like_real());
    // 件数（旧 5 metric 相当）が正しくても、全セル有効の表と報告は落ちる。
    rewrite(&fx, |_| Some(true));
    let (st, reasons) = recompute(&fx);
    assert_eq!(st, Status::Fail);
    assert!(
        reasons
            .iter()
            .any(|r| r.contains("benchfs/rep1/w2") && r.contains("marked valid")),
        "{reasons:?}"
    );
    assert_eq!(run(&fx).failure_class, Some(FailureClass::ModelFailure));
}

#[test]
fn dropping_missing_cells_from_the_denominator_is_rejected() {
    let fx = fixture(&like_real());
    let mut exp = fx.expected.clone();
    exp.rows.retain(|r| r.valid);
    fs::write(fx.art.join("results/cells.csv"), exp.csv()).expect("csv");
    fs::write(fx.art.join("report.md"), exp.metric_lines()).expect("report");
    let (st, reasons) = recompute(&fx);
    assert_eq!(st, Status::Fail);
    assert!(
        reasons
            .iter()
            .any(|r| r.contains("not in the table") && r.contains("denominator")),
        "{reasons:?}"
    );
    assert!(
        reasons.iter().any(|r| r.contains("metric cells_total")),
        "{reasons:?}"
    );
}

#[test]
fn effective_fsync_zero_counted_valid_is_rejected() {
    let mut p = synth::healthy();
    for c in &mut p.cells {
        if c.fs == "chfs" && c.workload == "w1a" {
            c.defect = Defect::FsyncZero;
        }
        if c.fs == "chfs" && c.workload == "w2" {
            c.defect = Defect::FsyncFallback;
        }
    }
    let fx = fixture(&p);
    let bad: Vec<_> = fx.expected.rows.iter().filter(|r| !r.valid).collect();
    assert_eq!(bad.len(), 2, "{:?}", fx.expected.rows);
    assert!(
        bad.iter().all(|r| r.reasons[0].starts_with("fsync")),
        "{bad:?}"
    );
    assert_eq!(recompute(&fx).0, Status::Pass, "正しい解");
    rewrite(&fx, |r| (r.fs == "chfs").then_some(true));
    let (st, reasons) = recompute(&fx);
    assert_eq!(st, Status::Fail);
    assert!(
        reasons
            .iter()
            .any(|r| r.contains("chfs/rep1/w1a") && r.contains("fsync=0")),
        "{reasons:?}"
    );
    assert!(
        reasons
            .iter()
            .any(|r| r.contains("chfs/rep1/w2") && r.contains("fallback")),
        "{reasons:?}"
    );
}

#[test]
fn failed_or_unconfirmed_jobs_are_rejected_but_f_with_exit_zero_passes() {
    let mut p = synth::healthy();
    // 1002: F / Exit 271（killed）で cell も exit≠0。1003: 記録なし（Unknown Job Id 相当）。1004: まだ R。
    for c in &mut p.cells {
        match (c.fs, c.workload) {
            ("gekkofs", "w1a") => {
                c.job = "1002";
                c.defect = Defect::ExitNonZero;
            }
            ("gekkofs", "w2") => c.job = "1003",
            ("unifyfs", "w1a") => c.job = "1004",
            _ => {}
        }
    }
    p.jobs
        .insert("1002", Some(synth::qstat("1002", "F", Some(271))));
    p.jobs.insert("1003", None);
    p.jobs.insert("1004", Some(synth::qstat("1004", "R", None)));
    let fx = fixture(&p);
    let by = |fs: &str, wl: &str| {
        fx.expected
            .rows
            .iter()
            .find(|r| r.fs == fs && r.workload == wl)
            .expect("row")
            .clone()
    };
    assert_eq!(
        by("gekkofs", "w1a").reasons,
        vec!["exit=255", "job_failed(exit=271)"]
    );
    assert_eq!(by("gekkofs", "w2").reasons, vec!["job_unknown"]);
    assert_eq!(by("unifyfs", "w1a").reasons, vec!["job_not_finished(R)"]);
    // job 1001 は F かつ Exit_status=0: state F だけで失敗にしない。
    assert!(by("benchfs", "w1a").valid);
    assert_eq!(recompute(&fx).0, Status::Pass);
    rewrite(&fx, |r| (!r.valid).then_some(true));
    let (st, reasons) = recompute(&fx);
    assert_eq!(st, Status::Fail);
    for tag in ["gekkofs/rep1/w1a", "gekkofs/rep1/w2", "unifyfs/rep1/w1a"] {
        assert!(
            reasons
                .iter()
                .any(|r| r.contains(tag) && r.contains("marked valid")),
            "{tag}: {reasons:?}"
        );
    }
    // 逆に、F/Exit 0 の正常 job のセルを「F だから失敗」と無効にするのも落ちる。
    rewrite(&fx, |r| (r.job.as_deref() == Some("1001")).then_some(false));
    let (st, reasons) = recompute(&fx);
    assert_eq!(st, Status::Fail);
    assert!(
        reasons
            .iter()
            .any(|r| r.contains("marked invalid but the raw shows a valid cell")),
        "{reasons:?}"
    );
}

#[test]
fn table_and_report_disagreement_is_rejected() {
    let fx = fixture(&like_real());
    let mut lines = fx.expected.metric_lines();
    lines = lines.replace("metric cells_valid = 11", "metric cells_valid = 13");
    fs::write(fx.art.join("report.md"), lines).expect("report");
    let (st, reasons) = recompute(&fx);
    assert_eq!(st, Status::Fail);
    assert!(
        reasons
            .iter()
            .any(|r| r.contains("table and report disagree")),
        "{reasons:?}"
    );
    // 表に書いた有効セルの数値が raw とずれていても落ちる。
    let mut exp = fx.expected.clone();
    for r in &mut exp.rows {
        if let Some(v) = r.values.get_mut("write_MiB_s") {
            *v *= 1.05;
        }
    }
    fs::write(fx.art.join("results/cells.csv"), exp.csv()).expect("csv");
    fs::write(fx.art.join("report.md"), exp.metric_lines()).expect("report");
    let (st, reasons) = recompute(&fx);
    assert_eq!(st, Status::Fail);
    assert!(
        reasons
            .iter()
            .any(|r| r.contains("write_MiB_s") && r.contains("tolerance")),
        "{reasons:?}"
    );
}

#[test]
fn invalid_rows_must_name_the_primary_cause_and_unknown_valid_values_are_rejected() {
    let fx = fixture(&like_real());
    let text = fs::read_to_string(fx.art.join("results/cells.csv")).expect("csv");
    let edited = text.replace("\"missing;job_failed(exit=-20)\"", "\"because\"");
    assert_ne!(edited, text);
    fs::write(fx.art.join("results/cells.csv"), edited).expect("csv");
    let (st, reasons) = recompute(&fx);
    assert_eq!(st, Status::Fail);
    assert!(
        reasons
            .iter()
            .any(|r| r.contains("do not mention the primary cause missing")),
        "{reasons:?}"
    );
    let text = fs::read_to_string(fx.art.join("results/cells.csv")).expect("csv");
    let edited: String = text
        .lines()
        .map(|l| if l.starts_with("fs,") { l.to_string() } else { l.replace(",valid,", ",ok,") } + "\n")
        .collect();
    fs::write(fx.art.join("results/cells.csv"), edited).expect("csv");
    let (st, reasons) = recompute(&fx);
    assert_eq!(st, Status::Fail);
    assert!(
        reasons.iter().any(|r| r.contains("valid must be")),
        "{reasons:?}"
    );
}

#[test]
fn broken_grader_raw_is_env_failure_not_model_failure() {
    let p = like_real();
    let fx = fixture(&p);
    // planned-cells.json を pin から変えた archive を置き直す → grader 資産の不整合。
    let raw = fx._t.path().join("raw/inputs");
    fs::write(
        raw.join("runs").join(&p.run).join("planned-cells.json"),
        "[]",
    )
    .expect("w");
    let tgz = fx.case_dir.join("inputs.tgz");
    assert!(
        Command::new("tar")
            .arg("-czf")
            .arg(&tgz)
            .arg("-C")
            .arg(&raw)
            .arg(".")
            .status()
            .expect("tar")
            .success()
    );
    let mut v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(fx.case_dir.join("k4-synth.json")).expect("r"))
            .expect("j");
    v["recompute"]["input_archive"]["sha256"] = json!(sha(&tgz));
    fs::write(fx.case_dir.join("k4-synth.json"), v.to_string()).expect("w");
    let case = GradeCase::load(&fx.case_dir.join("k4-synth.json")).expect("load");
    let r = grade(
        &case,
        &fx.art,
        &GradeOptions {
            case_dir: fx.case_dir.clone(),
            work_root: fx.work.clone(),
            runner: &NoRunner,
            observed_jobs: None,
            initial_tree: None,
        },
    );
    assert_eq!(
        r.failure_class,
        Some(FailureClass::EnvFailure),
        "{:?}",
        r.checks.recompute.reasons
    );
}

#[test]
fn qstat_records_are_read_by_state_and_exit_status() {
    assert_eq!(
        parse_qstat(&synth::qstat("1", "F", Some(0))),
        JobState::Finished
    );
    assert_eq!(
        parse_qstat(&synth::qstat("1", "F", Some(-20))),
        JobState::Failed { exit_status: -20 }
    );
    assert_eq!(
        parse_qstat(&synth::qstat("1", "R", None)),
        JobState::NotFinished { state: "R".into() }
    );
    assert_eq!(
        parse_qstat("qstat: Unknown Job Id 1.pbs\n"),
        JobState::Unknown
    );
    assert_eq!(parse_qstat(""), JobState::Unknown);
    let m: BTreeMap<String, f64> = synth::healthy()
        .jobs
        .keys()
        .map(|k| (k.to_string(), 0.0))
        .collect();
    assert_eq!(m.len(), 1);
}

/// 件数・valid 列・報告がすべて正しく、有効セル 1 つの数値 1 つだけが許容差（相対 0.1%）を超えてずれた表は落ちる。
/// 表と報告の不一致とは独立に、数値誤りだけで拒否されることを確かめる。
#[test]
fn numeric_error_in_one_valid_cell_is_rejected_on_its_own() {
    let fx = fixture(&like_real());
    for (key, factor) in [("read_ops", 1.02), ("md_stat", 0.97)] {
        let mut exp = fx.expected.clone();
        let row = exp
            .rows
            .iter_mut()
            .find(|r| r.valid && r.values.contains_key(key))
            .expect("valid row with the value");
        let cell = format!("{}/rep{}/{}", row.fs, row.rep, row.workload);
        *row.values.get_mut(key).expect("value") *= factor;
        assert_eq!(exp.counts(), fx.expected.counts(), "件数は正しいまま");
        fs::write(fx.art.join("results/cells.csv"), exp.csv()).expect("csv");
        fs::write(fx.art.join("report.md"), exp.metric_lines()).expect("report");
        let (st, reasons) = recompute(&fx);
        assert_eq!(st, Status::Fail, "{key}");
        assert!(
            reasons
                .iter()
                .any(|r| r.contains(&cell) && r.contains(key) && r.contains("tolerance")),
            "{key}: {reasons:?}"
        );
        assert!(
            !reasons.iter().any(|r| r.contains("metric cells_")),
            "数値誤りだけで落ちる: {reasons:?}"
        );
    }
    // 許容差の内側（相対 0.05%）は通る。
    let mut exp = fx.expected.clone();
    for r in &mut exp.rows {
        if let Some(v) = r.values.get_mut("write_MiB_s") {
            *v *= 1.0005;
        }
    }
    fs::write(fx.art.join("results/cells.csv"), exp.csv()).expect("csv");
    fs::write(fx.art.join("report.md"), exp.metric_lines()).expect("report");
    assert_eq!(recompute(&fx).0, Status::Pass);
}

/// 実 K4 原票（job 43706、公開入力 v2）: 人が承認した final-v3 と同じ 30/32（BenchFS W2 の 2 セルは欠測・job F/-20）。
/// A 腕相当の 30/32 は通り、B 腕相当（UnifyFS W3 を無効にした 28/32）・全セル有効・失敗 job のセルだけを有効・
/// 欠測を分母から除外・有効セルの数値誤りは落ちる。
#[test]
fn real_k4_archive_grades_thirty_of_thirty_two() {
    let tgz = std::env::var("CAMPAIGN_K4_TGZ")
        .map(std::path::PathBuf::from)
        .ok()
        .filter(|p| p.is_file())
        .or_else(|| {
            let p = std::path::PathBuf::from(
                "/local/celeris/data/workspaces/01M5SRJMA9CFNG2C9KCCS5VS4B/artifacts/k4-inputs-v2/main-v3-n2-43706-public2.tgz",
            );
            p.is_file().then_some(p)
        });
    let Some(tgz) = tgz else {
        eprintln!("SKIPPED(実物検証): 原票 tgz が無い");
        return;
    };
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/grade-cases/final/k4-cmp4-validity-analysis");
    let t = tempfile::tempdir().expect("tmp");
    let cdir = t.path().join("case");
    fs::create_dir_all(&cdir).expect("case");
    fs::copy(
        src.join("k4-cmp4-validity-analysis.json"),
        cdir.join("k4-cmp4-validity-analysis.json"),
    )
    .expect("copy");
    let case = GradeCase::load(&cdir.join("k4-cmp4-validity-analysis.json")).expect("load");
    let rec = case.recompute.as_ref().expect("recompute");
    let arc = rec.input_archive.as_ref().expect("input_archive");
    assert_eq!(sha(&tgz), arc.sha256);
    fs::copy(&tgz, cdir.join(&arc.path)).expect("tgz");
    let raw = t.path().join("raw");
    fs::create_dir_all(raw.join("inputs")).expect("raw");
    assert!(
        Command::new("tar")
            .arg("-xzf")
            .arg(&tgz)
            .arg("-C")
            .arg(raw.join("inputs"))
            .status()
            .expect("tar")
            .success()
    );
    let spec = rec.cells.as_ref().expect("cells");
    let exp = cells_validity::compute(&raw, spec).expect("expected");
    let c = exp.counts();
    assert_eq!(c["cells_total"], 32.0);
    assert_eq!(c["cells_valid"], 30.0);
    assert_eq!(
        (
            c["cells_valid_benchfs"],
            c["cells_valid_chfs"],
            c["cells_valid_gekkofs"],
            c["cells_valid_unifyfs"]
        ),
        (6.0, 8.0, 8.0, 8.0)
    );
    let bad: Vec<_> = exp.rows.iter().filter(|r| !r.valid).collect();
    assert_eq!(
        bad.iter()
            .map(|r| format!("{}/rep{}/{}", r.fs, r.rep, r.workload))
            .collect::<Vec<_>>(),
        ["benchfs/rep1/w2", "benchfs/rep2/w2"]
    );
    assert!(
        bad.iter()
            .all(|r| r.reasons == ["missing", "job_failed(exit=-20)"]),
        "{bad:?}"
    );
    // 旧 5 件数 metric も report に要る。
    let mut five = String::new();
    for m in &rec.metrics {
        let celeris_campaign::grader::case::Source::Json { files, .. } = &m.source else {
            panic!("json")
        };
        five.push_str(&format!("metric {} = {}\n", m.name, files.len()));
    }
    let art = t.path().join("artifact");
    fs::create_dir_all(art.join("results")).expect("art");
    let work = t.path().join("work");
    fs::create_dir_all(&work).expect("work");
    let run = |exp: &Expected| {
        fs::write(art.join("results/cells.csv"), exp.csv()).expect("csv");
        fs::write(art.join("report.md"), five.clone() + &exp.metric_lines()).expect("report");
        let r = grade(
            &case,
            &art,
            &GradeOptions {
                case_dir: cdir.clone(),
                work_root: work.clone(),
                runner: &NoRunner,
                observed_jobs: None,
                initial_tree: None,
            },
        );
        (
            r.checks.recompute.status,
            r.failure_class,
            r.checks.recompute.reasons,
        )
    };
    let (st, fc, reasons) = run(&exp);
    assert_eq!((st, fc), (Status::Pass, None), "{reasons:?}");
    let mut b_like = exp.clone();
    for r in &mut b_like.rows {
        if r.fs == "unifyfs" && r.workload == "w3" {
            r.valid = false;
            r.reasons = vec!["mdtest unlink warnings".into()];
        }
    }
    let (st, fc, reasons) = run(&b_like);
    assert_eq!((st, fc), (Status::Fail, Some(FailureClass::ModelFailure)));
    assert!(
        reasons
            .iter()
            .any(|r| r.contains("unifyfs/rep1/w3") && r.contains("marked invalid")),
        "{reasons:?}"
    );
    let mut all_valid = exp.clone();
    for r in &mut all_valid.rows {
        r.valid = true;
    }
    assert_eq!(run(&all_valid).0, Status::Fail);
    // 失敗 job（F/-20）の BenchFS W2 だけを有効にした報告（32/32）。
    let mut w2_valid = exp.clone();
    for r in &mut w2_valid.rows {
        if r.fs == "benchfs" && r.workload == "w2" {
            r.valid = true;
            r.reasons.clear();
        }
    }
    let (st, fc, reasons) = run(&w2_valid);
    assert_eq!((st, fc), (Status::Fail, Some(FailureClass::ModelFailure)));
    assert!(
        reasons
            .iter()
            .any(|r| r.contains("benchfs/rep1/w2") && r.contains("marked valid")),
        "{reasons:?}"
    );
    // 欠測の 2 セルを表と分母から落とした報告（30/30）。
    let mut dropped = exp.clone();
    dropped.rows.retain(|r| r.valid);
    assert_eq!(dropped.counts()["cells_total"], 30.0);
    let (st, _, reasons) = run(&dropped);
    assert_eq!(st, Status::Fail);
    assert!(
        reasons.iter().any(|r| r.contains("denominator")),
        "{reasons:?}"
    );
    // 件数は正しく、有効セル 1 つの帯域だけを 2% 誤った報告。
    let mut wrong_value = exp.clone();
    let row = wrong_value
        .rows
        .iter_mut()
        .find(|r| r.valid && r.values.contains_key("write_MiB_s"))
        .expect("valid ior row");
    let cell = format!("{}/rep{}/{}", row.fs, row.rep, row.workload);
    *row.values.get_mut("write_MiB_s").expect("value") *= 1.02;
    let (st, fc, reasons) = run(&wrong_value);
    assert_eq!((st, fc), (Status::Fail, Some(FailureClass::ModelFailure)));
    assert!(
        reasons
            .iter()
            .any(|r| r.contains(&cell) && r.contains("write_MiB_s") && r.contains("tolerance")),
        "{reasons:?}"
    );
}
