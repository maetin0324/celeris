//! 成功契約: 同じ out dir（runner の生記録 + 独立採点）から、checker・export・human-op join/report が同じ結論を出す。
//! export の CSV は正規の quoting で、JSON セル（fault_breakdown 複数 key）を含めて読み戻せる。
mod common;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use celeris_campaign::{
    checker::check,
    csv,
    executor::MockExecutor,
    export::{COLUMNS, csv_table, export},
    manifest::CampaignManifest,
    runner::{RunOptions, run},
    success::{self, SuccessClass},
};
use serde_json::{Value, json};

fn fx() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn pilot_out(dir: &Path) -> (CampaignManifest, PathBuf) {
    let mp = fx().join("pilot.json");
    let mut m = CampaignManifest::load(&mp).expect("pilot");
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

fn report(human_ops: &Path, events: &Path, out: &Path) -> Value {
    let o = Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("bin"))
        .args(["human-op", "report", "--file"])
        .arg(human_ops)
        .arg("--events")
        .arg(events)
        .arg("--out")
        .arg(out)
        .output()
        .expect("spawn");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    serde_json::from_str(&fs::read_to_string(out.join("report.json")).expect("r")).expect("json")
}

#[test]
fn checker_export_and_human_ops_reach_the_same_conclusion_per_cell() {
    let d = tempfile::tempdir().expect("dir");
    let (m, out) = pilot_out(d.path());
    // 採点: A は合格、B は成果物の不合格、C は採点基盤の不良（再採点まで未確定）。
    common::write_grades_classed(&m, &out, |r| match r["config"].as_str() {
        Some("A") => None,
        Some("B") => Some("model_failure"),
        _ => Some("env_failure"),
    });
    let rep = check(&m, &out);
    assert!(rep.ok, "{:?}", rep.violations);
    let totals = rep.recomputed.expect("rc").totals;
    let table = export(&m, &out, &out.join("ex")).expect("export");
    let canon = success::load(&out).expect("load").expect("manifest");
    assert!(
        canon.grade_violations.is_empty(),
        "{:?}",
        canon.grade_violations
    );
    // 評価者記録は shared scope の 1 件だけ（試行の結論に影響しない）。
    let hop = d.path().join("human-ops.jsonl");
    let o = Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("bin"))
        .args(["human-op", "record", "--file"])
        .arg(&hop)
        .args([
            "--campaign",
            "c",
            "--manifest-sha256",
            &canon.manifest_sha256,
        ])
        .args([
            "--scope",
            "shared",
            "--op-kind",
            "status_check",
            "--intent",
            "monitoring",
        ])
        .args([
            "--actor",
            "eval",
            "--mode",
            "manual",
            "--started-at",
            "2026-10-22T00:00:00Z",
        ])
        .args(["--ended-at", "2026-10-22T00:01:00Z"])
        .output()
        .expect("spawn");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let r = report(&hop, &out, &d.path().join("rep"));

    for (cfg, t) in &totals {
        let label = format!("{cfg:?}");
        let cells: Vec<_> = canon.cells.values().filter(|c| c.config == *cfg).collect();
        // 分母: manifest の計画 cell 数が checker の n・report の試行数と一致。
        assert_eq!(cells.len() as u64, t.n, "{label}");
        let arm = &r["arms"][&label];
        assert_eq!(arm["trials"], t.n, "{label}: {arm}");
        // validated success: checker・export・report が同じ数。
        let vs = cells.iter().filter(|c| c.class.is_success()).count() as u64;
        assert_eq!(vs, t.final_passed, "{label}");
        assert_eq!(
            arm["validated_success"].as_u64().unwrap_or(0),
            t.final_passed,
            "{label}"
        );
        let row = table["configs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["config"] == label)
            .expect("row");
        assert_eq!(row["final_passed"], t.final_passed, "{label}");
        // executor passed と validated success は別欄。
        assert_eq!(
            arm["executor_passed"].as_u64().unwrap_or(0),
            t.passed,
            "{label}"
        );
        let mut by_class: BTreeMap<String, u64> = BTreeMap::new();
        for c in &cells {
            *by_class.entry(c.class.as_str().to_string()).or_default() += 1;
        }
        assert_eq!(arm["success_classes"], json!(by_class), "{label}");
        for (class, key) in [
            (SuccessClass::ModelFailure, "grader_failed"),
            (SuccessClass::GraderEnvFailure, "grader_env_failure"),
            (SuccessClass::Ungraded, "ungraded"),
            (SuccessClass::Missing, "missing"),
        ] {
            let n = cells.iter().filter(|c| c.class == class).count() as u64;
            assert_eq!(row[key], n, "{label}/{key}");
        }
    }
    assert!(totals[&celeris_campaign::manifest::ConfigId::A].final_passed > 0);
    // 試行ごと: join の success_class が canonical と同じ。
    for t in r["trials"].as_array().expect("trials") {
        let cfg = t["arm"].as_str().unwrap();
        let cell = canon
            .cells
            .values()
            .find(|c| {
                format!("{:?}", c.config) == cfg
                    && t["case_id"] == c.problem_id.as_str()
                    && t["rep"] == c.rep
            })
            .expect("cell");
        assert_eq!(t["success_class"]["value"], cell.class.as_str(), "{t}");
        assert_eq!(t["completion"]["value"], cell.class.is_success());
        assert_eq!(
            t["success_class"]["needs_regrade"],
            cell.class.needs_regrade()
        );
    }
}

#[test]
fn csv_round_trips_json_cells_with_multiple_fault_keys() {
    let row = json!({
        "config": "B",
        "harness": "claude-code, \"quoted\"\nnext",
        "n": 2,
        "pass_rate": null,
        "fault_breakdown": {"exposed": {"passed": 1, "failed": 0}, "not_exposed": {"failed": 1}},
    });
    let text = csv_table(std::slice::from_ref(&row), "real", true);
    let rows = csv::parse(&text).expect("parse");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].len(), COLUMNS.len() + 2);
    assert_eq!(rows[1].len(), rows[0].len(), "列数が崩れない");
    let col = |n: &str| rows[0].iter().position(|h| h == n).expect("col");
    assert_eq!(rows[1][col("harness")], row["harness"].as_str().unwrap());
    let fb: Value = serde_json::from_str(&rows[1][col("fault_breakdown")]).expect("json cell");
    assert_eq!(fb, row["fault_breakdown"]);
    assert_eq!(rows[1][col("complete")], "true");
    // 旧実装（comma join）では列がずれた: JSON セルの comma で分割すると列が増える。
    let naive = rows[1]
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(",");
    assert!(naive.split(',').count() > rows[0].len());
    // quoting の規則そのもの
    assert_eq!(csv::field("a,b"), "\"a,b\"");
    assert_eq!(csv::field("say \"hi\""), "\"say \"\"hi\"\"\"");
    assert!(csv::parse("\"open").is_err());
}

/// 実行されなかった cell（arm 予算切れで未投入）の消費は、checker の欠測数（分母 = 実行した試行）と
/// report の欠測数が一致し、report は未実行として別に数える（値は null のまま。観測済み 0 にしない）。
#[test]
fn unexecuted_cell_consumption_is_not_run_in_report_and_checker_alike() {
    let d = tempfile::tempdir().expect("dir");
    let (m, out) = pilot_out(d.path());
    common::write_grades_classed(&m, &out, |_| None);
    // B の最初の記録を「arm の残りが無く投入しなかった」試行に書き換える（runner が書く形と同じ）。
    let tp = out.join("trials.jsonl");
    let mut rewritten = false;
    let lines: Vec<String> = fs::read_to_string(&tp)
        .expect("trials")
        .lines()
        .map(|l| {
            let mut v: Value = serde_json::from_str(l).expect("json");
            if !rewritten && v["config"] == "B" {
                rewritten = true;
                for (k, x) in [
                    ("outcome", json!("cancelled")),
                    ("passed", json!(false)),
                    ("reason", json!("arm_budget")),
                    ("tokens", Value::Null),
                    ("token_usage", Value::Null),
                    ("cost_micro_usd", Value::Null),
                    ("wall_secs", json!(0)),
                    ("workspace", json!("")),
                    ("kb", json!("")),
                ] {
                    v[k] = x;
                }
            }
            v.to_string()
        })
        .collect();
    assert!(rewritten, "pilot に B の記録がある");
    fs::write(&tp, lines.join("\n") + "\n").expect("write");

    let totals = check(&m, &out).recomputed.expect("rc").totals;
    let b = &totals[&celeris_campaign::manifest::ConfigId::B];
    assert!(b.ran_trials < b.n, "B に未実行の cell がある");
    let hop = d.path().join("human-ops.jsonl");
    fs::write(&hop, "").expect("hop");
    let r = report(&hop, &out, &d.path().join("rep"));
    let arm = &r["arms"]["B"];
    let n = |k: &str| arm[k].as_u64().unwrap_or(0);
    assert_eq!(n("tokens_missing_trials"), b.tokens_missing_trials, "{arm}");
    assert_eq!(
        n("cost_micro_usd_missing_trials"),
        b.cost_missing_trials,
        "{arm}"
    );
    assert_eq!(n("not_run_trials"), b.n - b.ran_trials, "{arm}");
    assert_eq!(n("tokens_not_run_trials"), b.n - b.ran_trials, "{arm}");
    // 未実行の試行の値は null（0 にしない）、join の success_class は ran=false。
    let t = r["trials"]
        .as_array()
        .expect("trials")
        .iter()
        .find(|t| t["arm"] == "B" && t["success_class"]["ran"] == false)
        .expect("not-run trial");
    assert_eq!(t["success_class"]["value"], "executor_cancelled", "{t}");
    assert!(t["join"]["tokens"]["value"].is_null(), "{t}");
}

/// decision-log の records を渡さない export は総消費を欠測と明記し、前回の final-accounting.json を残さない。
#[test]
fn export_without_records_marks_consumption_missing() {
    let d = tempfile::tempdir().expect("dir");
    let (m, out) = pilot_out(d.path());
    common::write_grades_classed(&m, &out, |_| None);
    let dest = out.join("ex");
    fs::create_dir_all(&dest).expect("dest");
    let stale = dest.join(celeris_campaign::export::FINAL_ACCOUNTING_FILE);
    fs::write(&stale, "{}\n").expect("stale");
    let table = export(&m, &out, &dest).expect("export");
    let c = &table["consumption"];
    assert_eq!(c["scope"]["tokens"], "missing", "{c}");
    assert_eq!(c["reduction_confirmable"], false, "{c}");
    assert_eq!(c["reason"], celeris_campaign::export::NO_RECORDS_REASON);
    assert!(!stale.exists(), "前回の最終集計が残っている");
}
