use celeris_campaign::{dagbench::graded::Grading, grader::GradeCase};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const PRISTINE_TEST: &str =
    "use demo::add;\n\n#[test]\nfn adds_small_numbers() {\n    assert_eq!(add(1, 2), 3);\n}\n";
const BASE_CSV: &str = "cond,xfer,bw\nbase,4k,50\nbase,4k,60\nbase,1m,999\n";
const GOOD_REPORT: &str = "# 結果\n\nmetric write_bw_mean = 110.0\nmetric write_bw_n = 3\nmetric base_bw_median = 55\nmetric speedup = 2.0\n";
/// 捏造: 生データから再計算できない speedup。
pub const FABRICATED_REPORT: &str = "# 結果\n\nmetric write_bw_mean = 110.0\nmetric write_bw_n = 3\nmetric base_bw_median = 55\nmetric speedup = 9.0\n";

fn sha(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}

pub fn write(p: &Path, s: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, s).unwrap();
}

/// 共有 target の `celeris-campaign`（cargo が試験の途中で作り直しうる）。
fn shared_grader() -> PathBuf {
    std::env::var("CARGO_BIN_EXE_celeris-campaign")
        .expect("binary path")
        .into()
}

/// grader の写しを `dir` に固定する。grading の pin は grader の sha256 を含み、graded run は RunHeader を
/// 何度も書いて pin の一致を確かめる。共有 target の binary は同じ nextest の別の試験（例
/// `crates/celeris/tests/ops_r1_pipeline.rs` の `cargo build -p celeris-campaign`）が別の feature 集合で
/// 作り直すので、それを直に pin すると run の途中で「graded run pins changed」になる。
/// 写しは別 process（`cp`）で書く（自 process で書いた fd が fork に漏れて exec が ETXTBSY になるのを避ける）。
pub fn freeze_grader(dir: &Path, source: &Path) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let frozen = dir.join("grader");
    assert!(
        Command::new("cp")
            .arg(source)
            .arg(&frozen)
            .status()
            .unwrap()
            .success(),
        "copy grader {}",
        source.display()
    );
    frozen
}

/// 固定入力から作る成果物・case・評価側の scheduler 記録。
pub fn fixture(root: &Path, overrides: Vec<(String, String)>) -> Grading {
    fixture_with_grader(root, overrides, &shared_grader())
}

/// [`fixture`] の grader の出所を指定する版（grader は `root` の下に固定した写しを使う）。
pub fn fixture_with_grader(
    root: &Path,
    overrides: Vec<(String, String)>,
    grader: &Path,
) -> Grading {
    let (case_dir, art, obs) = (
        root.join("case"),
        root.join("good"),
        root.join("observed.json"),
    );
    write(&case_dir.join("pristine/tests/add.rs"), PRISTINE_TEST);
    write(
        &art.join("repo/src/lib.rs"),
        "pub fn add(a: u64, b: u64) -> u64 { a + b }\n",
    );
    write(&art.join("repo/tests/add.rs"), PRISTINE_TEST);
    let (mut raw, mut seen) = (vec![], vec![]);
    for (i, bw) in [100.0, 110.0, 120.0].iter().enumerate() {
        let rel = format!("raw/r{}/ior_result.json", i + 1);
        let body = json!({"tests":[{"Results":[{"access":"write","bwMiB": bw}]}]}).to_string();
        write(&art.join(&rel), &body);
        let job = format!("{}.sirius", 42588 + i);
        write(
            &art.join(format!("evidence/{job}.qstat.txt")),
            &format!("Job Id: {job}\n    job_state = F\n    Exit_status = 0\n"),
        );
        write(&art.join(format!("evidence/{job}.log")), "IOR finished\n");
        let rawref = json!([{"path": rel, "sha256": sha(body.as_bytes())}]);
        seen.push(
            json!({"job_id": job, "exit_status": 0, "conditions": {"nodes": 3}, "raw": rawref}),
        );
        raw.push(json!({"job_id": job, "scheduler_record": format!("evidence/{job}.qstat.txt"),
            "log": format!("evidence/{job}.log"), "raw": rawref, "conditions": {"nodes": 3}, "evidence_kind": "real"}));
    }
    write(
        &art.join("evidence/jobs.json"),
        &json!({"jobs": raw}).to_string(),
    );
    write(
        &obs,
        &json!({"schema": "celeris.observed-jobs/1", "jobs": seen}).to_string(),
    );
    write(&art.join("raw/baseline.csv"), BASE_CSV);
    write(&art.join("report.md"), GOOD_REPORT);
    write(
        &art.join("review/findings.json"),
        &json!({"findings": []}).to_string(),
    );
    let files = [
        "raw/r1/ior_result.json",
        "raw/r2/ior_result.json",
        "raw/r3/ior_result.json",
    ];
    let json_src = json!({"json": {"files": files, "pointer": "/tests/0/Results/0/bwMiB"}});
    let case: GradeCase = serde_json::from_value(json!({
        "schema": "celeris.grade-case/1", "id": "dagbench-analyze", "split": "dev",
        "implementation": {
            "pristine_dir": "pristine", "repo": "repo",
            "tests": [{"path": "tests/add.rs", "sha256": sha(PRISTINE_TEST.as_bytes())}],
            "command": ["sh", "-c", "test -f tests/add.rs && echo 'test adds_small_numbers ... ok'"],
            "must_pass": ["adds_small_numbers"],
        },
        "experiment": {"min_jobs": 3, "conditions": {"nodes": 3}},
        "recompute": {
            "report": "report.md",
            "pinned_raw": {"raw/baseline.csv": sha(BASE_CSV.as_bytes())},
            "metrics": [
                {"name": "write_bw_mean", "agg": "mean", "tolerance_rel": 0.01, "source": json_src},
                {"name": "write_bw_n", "agg": "count", "tolerance_rel": 0.0, "source": json_src},
                {"name": "base_bw_median", "agg": "median", "tolerance_rel": 0.01,
                 "source": {"csv": {"files": ["raw/baseline.csv"], "column": "bw", "filter": {"xfer": "4k"}}}},
                {"name": "speedup", "tolerance_rel": 0.01,
                 "source": {"ratio": {"numerator": "write_bw_mean", "denominator": "base_bw_median"}}},
            ],
        },
            }))
    .unwrap();
    case.validate().unwrap();
    write(
        &case_dir.join("case.json"),
        &serde_json::to_string(&case).unwrap(),
    );
    Grading {
        node: "analyze".into(),
        grader_bin: freeze_grader(root, grader),
        case: case_dir.join("case.json"),
        observed: Some(obs),
        work: root.join("grade-work"),
        stage_root: root.join("stage"),
        good_artifact: art,
        first_attempt_overrides: overrides,
        max_attempts: 3,
    }
}

pub fn k3_grading(root: &Path) -> Option<Grading> {
    use celeris_campaign::grader::case::{Agg, Source};
    let tgz = std::env::var("CAMPAIGN_K3_TGZ")
        .map(std::path::PathBuf::from)
        .ok()
        .into_iter()
        .chain([
            std::path::PathBuf::from("/local/celeris/data/scratch/mlsys-pilot-2/inputs/main-43223.tgz"),
            std::path::PathBuf::from("/local/celeris/data/workspaces/01M45ACZQWTW9BP8WVY8KAW9TC/artifacts/locusta-latency/raw/main-43223.tgz"),
        ])
        .find(|p| p.is_file());
    let tgz = tgz?;
    assert_eq!(
        sha(&fs::read(&tgz).unwrap()),
        "bdb155d4c1690ae5743621d129c270fcc53c919751d62c56d0c4395b74d2e90f",
        "原票の hash が case.json の固定とずれた"
    );
    // grader 側の写し: 固定 tgz は grade-case の隣（grader が自分で展開する。repo には置かない）。
    let case_dir = root.join("grade-case");
    fs::create_dir_all(&case_dir).unwrap();
    let case = case_dir.join("k3-locusta-ab-analysis.json");
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/grade-cases/dev/k3-locusta-ab-analysis/k3-locusta-ab-analysis.json"),
        &case,
    )
    .unwrap();
    fs::copy(&tgz, case_dir.join("main-43223.tgz")).unwrap();
    let g = GradeCase::load(&case).unwrap();
    let good = root.join("good");
    fs::create_dir_all(good.join("inputs")).unwrap();
    assert!(
        Command::new("tar")
            .arg("-xzf")
            .arg(&tgz)
            .arg("-C")
            .arg(good.join("inputs"))
            .status()
            .unwrap()
            .success()
    );
    // 報告は raw から素朴に（平均・件数だけで）計算する。捏造は最初の指標だけ 6% ずらす。
    let (mut ok, mut bad) = (String::new(), String::new());
    for (i, m) in g.recompute.as_ref().unwrap().metrics.iter().enumerate() {
        let Source::Json { files, pointer } = &m.source else {
            panic!("K3 は json 指標だけ")
        };
        let xs: Vec<f64> = files
            .iter()
            .map(|f| {
                let v: Value = serde_json::from_slice(&fs::read(good.join(f)).unwrap()).unwrap();
                v.pointer(pointer).and_then(Value::as_f64).unwrap()
            })
            .collect();
        let x = match m.agg {
            Agg::Mean => xs.iter().sum::<f64>() / xs.len() as f64,
            Agg::Count => xs.len() as f64,
            a => panic!("未対応 {a:?}"),
        };
        ok.push_str(&format!("metric {} = {x}\n", m.name));
        let y = if i == 0 { x * 1.06 } else { x };
        bad.push_str(&format!("metric {} = {y}\n", m.name));
    }
    write(&good.join("report.md"), &ok);
    Some(Grading {
        node: "analyze".into(),
        grader_bin: freeze_grader(root, &shared_grader()),
        case,
        observed: None,
        work: root.join("grade-work"),
        stage_root: root.join("stage"),
        good_artifact: good,
        first_attempt_overrides: vec![("report.md".into(), bad)],
        max_attempts: 3,
    })
}
