//! short-ml-campaign（dev 専用 C2 case）の固定資産の決定的検証。実 job は投入せず、LLM・network も使わない。
//! 通常 loader での読み込み・hash pin・実行前契約と環境 lock の一致・入力隔離（評価側の目的変数と参照値が trial に渡らない）・
//! 既存 case の保持を見る。python/numpy の版は要求値であり、ここでは測定済みとして扱わない。
use std::{
    fs,
    path::{Path, PathBuf},
};

use celeris_campaign::{
    cases::{CaseKind, CaseSet, verify_file_sha256},
    grader::GradeCase,
    manifest::Split,
    snapshot::{copy_tree, hash_tree},
};
use serde_json::Value;
use sha2::{Digest, Sha256};

const ID: &str = "short-ml-campaign";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}
fn case_dir() -> PathBuf {
    root().join("cases/dev").join(ID)
}
fn inputs_dir() -> PathBuf {
    root().join("fixtures/inputs").join(ID)
}
fn private_dir() -> PathBuf {
    root().join("fixtures/grader-private/dev").join(ID)
}
fn sha(p: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(p).unwrap()))
}
fn json(p: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap()
}
fn load() -> CaseSet {
    CaseSet::load(
        &root().join("cases"),
        &root().join("fixtures/grader-private"),
    )
    .unwrap()
}
fn rows(p: &Path) -> Vec<Vec<String>> {
    fs::read_to_string(p)
        .unwrap()
        .lines()
        .map(|l| l.split(',').map(str::to_string).collect())
        .collect()
}

#[test]
fn loads_through_the_normal_loader_as_dev_only() {
    let set = load();
    let c = set.get(ID).expect("short-ml-campaign loads");
    assert_eq!(c.case.split, Split::Dev);
    assert_eq!(c.case.kind, CaseKind::Real);
    assert_eq!(c.case.stages, ["experiment", "analysis"]);
    assert_eq!(c.case.judged_scope(), ["experiment", "analysis"]);
    assert!(c.private.known_defects.len() >= 3);
    assert!(!root().join("cases/final").join(ID).exists());
    assert!(!case_dir().join("private.json").exists());
}

#[test]
fn existing_cases_are_kept_with_their_pins() {
    let set = load();
    for id in [
        "k0-fixture-control",
        "k1-bf-fsync-campaign",
        "k3-locusta-ab-analysis",
        "k2-bf-xattr-restart-campaign",
        "k4-cmp4-validity-analysis",
    ] {
        let c = set.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(c.case.private_ref.sha256.len() == 64, "{id}");
    }
    assert_eq!(set.cases.len(), 6);
}

#[test]
fn grade_case_is_a_normal_experiment_plus_recompute_pinned_to_train_csv() {
    let c = load();
    let c = c.get(ID).unwrap();
    let g = c.case.grade_case_ref.as_ref().unwrap();
    let gp = root().join("fixtures/grade-cases").join(&g.path);
    verify_file_sha256(&gp, &g.sha256).unwrap();
    let gc = GradeCase::load(&gp).unwrap();
    assert!(gc.implementation.is_none() && gc.review.is_none());
    let e = gc.experiment.as_ref().unwrap();
    assert_eq!(e.min_jobs, 1);
    assert_eq!(e.conditions["queue"], "debug");
    assert_eq!(e.conditions["ncpus"], 1);
    let r = gc.recompute.as_ref().unwrap();
    assert_eq!(
        r.pinned_raw["inputs/train.csv"],
        sha(&inputs_dir().join("train.csv"))
    );
    let names: Vec<_> = r.metrics.iter().map(|m| m.name.as_str()).collect();
    let contract = json(&case_dir().join("contract.json"));
    let want: Vec<_> = contract["metrics"]["names"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(names, want);
}

#[test]
fn contract_pins_every_input_environment_and_grader_file() {
    let d = case_dir();
    let (c, k) = (json(&d.join("case.json")), json(&d.join("contract.json")));
    let p = &k["pins"];
    assert_eq!(p["case_json"], sha(&d.join("case.json")).as_str());
    assert_eq!(p["env_lock"], sha(&d.join("env.lock.json")).as_str());
    assert_eq!(
        p["materials_sha256"],
        hash_tree(&d.join("materials")).unwrap().as_str()
    );
    assert_eq!(p["kb_snapshot"], hash_tree(&d.join("kb")).unwrap().as_str());
    assert_eq!(
        p["private"],
        sha(&private_dir().join("private.json")).as_str()
    );
    assert_eq!(p["grade_case"], c["grade_case_ref"]["sha256"]);
    for i in c["inputs"].as_array().unwrap() {
        let n = i["name"].as_str().unwrap();
        assert_eq!(i["sha256"].as_str().unwrap(), sha(&inputs_dir().join(n)));
        assert_eq!(p["inputs"][n], i["sha256"]);
    }
}

#[test]
fn contract_fixes_numeric_limits_termination_and_stop_before_run() {
    let d = case_dir();
    let (c, k) = (json(&d.join("case.json")), json(&d.join("contract.json")));
    let l = &k["limits"];
    for key in [
        "cpu_cores",
        "memory_gib",
        "disk_bytes",
        "job_walltime_secs",
        "case_wall_secs",
        "jobs_max",
        "jobs_hard_cap",
    ] {
        assert!(l[key].as_u64().unwrap() > 0, "{key}");
    }
    assert_eq!(l["llm_tokens"], 0);
    assert_eq!(l["main_comparison_trials"], 0);
    assert_eq!(l["gpu"], false);
    assert!(l["jobs_max"].as_u64() <= l["jobs_hard_cap"].as_u64());
    assert!(l["jobs_hard_cap"].as_u64().unwrap() <= 2);
    assert!(l["job_walltime_secs"].as_u64() <= l["case_wall_secs"].as_u64());
    assert_eq!(c["budget"]["max_disk_bytes"], l["disk_bytes"]);
    assert_eq!(c["budget"]["max_wall_secs"], l["case_wall_secs"]);
    assert_eq!(c["experiment"]["walltime_secs"], l["job_walltime_secs"]);
    assert_eq!(c["experiment"]["params"]["job_count_max"], l["jobs_max"]);
    assert_eq!(k["no_real_job_in_preparation"], true);
    assert!(k["termination"]["raw"].as_array().unwrap().len() >= 3);
    assert!(
        k["termination"]["collection"]
            .as_str()
            .unwrap()
            .contains("observed-jobs")
    );
    assert!(!k["on_failure"].as_str().unwrap().is_empty());
}

#[test]
fn env_lock_keeps_python_and_numpy_as_unverified_requests() {
    let e = json(&case_dir().join("env.lock.json"));
    assert_eq!(e["state"], "requested_unverified");
    assert!(e["measured"].is_null());
    assert_eq!(e["requested"]["python"], "3.9.25");
    assert_eq!(e["requested"]["numpy"], "1.23.5");
    assert_eq!(e["stop_on_mismatch"], true);
    assert!(!e["measure_before_run"].as_array().unwrap().is_empty());
}

#[test]
fn data_shape_and_train_eval_separation() {
    let t = rows(&inputs_dir().join("train.csv"));
    let f = rows(&inputs_dir().join("eval-features.csv"));
    let y = rows(&private_dir().join("eval-targets.csv"));
    assert_eq!(t[0], ["x1", "x2", "x3", "x4", "y"]);
    assert_eq!((t.len() - 1, f.len() - 1, y.len() - 1), (1600, 400, 400));
    assert_eq!(
        f[0],
        ["id", "x1", "x2", "x3", "x4"],
        "eval features carry no target"
    );
    assert!(
        t[1..].iter().all(|r| r.len() == 5)
            && f[1..].iter().all(|r| r.len() == 5)
            && y[1..].iter().all(|r| r.len() == 2)
    );
    let ref_ = json(&private_dir().join("reference.json"));
    assert_eq!(ref_["seed"], 20261019);
    let p = json(&private_dir().join("private.json"));
    assert_eq!(
        p["expected"]["withheld"]["eval-targets.csv"],
        sha(&private_dir().join("eval-targets.csv")).as_str()
    );
    assert_eq!(
        p["expected"]["withheld"]["reference.json"],
        sha(&private_dir().join("reference.json")).as_str()
    );
}

/// 参照値を標準ライブラリだけで train.csv から独立に再計算して、private の値と照合する（python/numpy 不要）。
#[test]
fn reference_coefficients_match_an_independent_ridge_fit() {
    let t = rows(&inputs_dir().join("train.csv"));
    let data: Vec<Vec<f64>> = t[1..]
        .iter()
        .map(|r| r.iter().map(|v| v.parse().unwrap()).collect())
        .collect();
    let n = data.len() as f64;
    let mean = |j: usize| data.iter().map(|r| r[j]).sum::<f64>() / n;
    let m: Vec<f64> = (0..5).map(mean).collect();
    let mut a = [[0.0f64; 5]; 4];
    for r in &data {
        for i in 0..4 {
            for j in 0..4 {
                a[i][j] += (r[i] - m[i]) * (r[j] - m[j]);
            }
            a[i][4] += (r[i] - m[i]) * (r[4] - m[4]);
        }
    }
    for (i, row) in a.iter_mut().enumerate() {
        row[i] += 1.0;
    }
    for c in 0..4 {
        let p = (c..4)
            .max_by(|&x, &y| a[x][c].abs().total_cmp(&a[y][c].abs()))
            .unwrap();
        a.swap(c, p);
        for r in 0..4 {
            if r != c {
                let f = a[r][c] / a[c][c];
                let pivot = a[c];
                for (x, y) in a[r].iter_mut().zip(pivot) {
                    *x -= f * y;
                }
            }
        }
    }
    let w: Vec<f64> = (0..4).map(|i| a[i][4] / a[i][i]).collect();
    let b = m[4] - (0..4).map(|i| w[i] * m[i]).sum::<f64>();
    let r = json(&private_dir().join("reference.json"));
    for (i, wi) in w.iter().enumerate() {
        let want = r["coef"][format!("x{}", i + 1)].as_f64().unwrap();
        assert!((wi - want).abs() < 1e-5, "x{}: {wi} vs {want}", i + 1);
    }
    assert!((b - r["intercept"].as_f64().unwrap()).abs() < 1e-5);
}

/// trial が見られる物（materials・kb・配送入力・case.json）に、評価側の目的変数・参照値・private の語が出ない。
#[test]
fn nothing_evaluator_side_reaches_the_trial() {
    let c = load();
    let c = c.get(ID).unwrap();
    let visible: Vec<PathBuf> = {
        let mut v = vec![case_dir().join("case.json")];
        for sub in [case_dir().join("materials"), case_dir().join("kb")] {
            v.extend(fs::read_dir(sub).unwrap().map(|e| e.unwrap().path()));
        }
        v.extend(c.case.inputs.iter().map(|i| inputs_dir().join(&i.name)));
        v
    };
    let names: Vec<_> = c.case.inputs.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["train.csv", "eval-features.csv"]);
    let r = json(&private_dir().join("reference.json"));
    let mut needles: Vec<String> = c
        .private
        .known_defects
        .iter()
        .map(|d| d.id.clone())
        .collect();
    needles.extend(
        [
            "eval-targets",
            "reference.json",
            "grader-private",
            "reference_not_for_copying",
        ]
        .map(String::from),
    );
    needles.push(format!("{:.6}", r["eval_rmse"].as_f64().unwrap()));
    needles.push(format!("{:.6}", r["train_rmse"].as_f64().unwrap()));
    for p in &visible {
        let t = fs::read_to_string(p).unwrap();
        for n in &needles {
            assert!(!t.contains(n.as_str()), "{} leaks {n}", p.display());
        }
    }
    // 評価側の目的変数 file は配送対象のどこにも無い。
    assert!(!inputs_dir().join("eval-targets.csv").exists());
    assert!(!case_dir().join("materials/eval-targets.csv").exists());
}

fn copy() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    copy_tree(&root().join("cases"), &t.path().join("cases")).unwrap();
    copy_tree(
        &root().join("fixtures/grader-private"),
        &t.path().join("grader-private"),
    )
    .unwrap();
    let (c, g) = (t.path().join("cases"), t.path().join("grader-private"));
    (t, c, g)
}

#[test]
fn tampering_with_any_pinned_file_is_rejected() {
    for (rel, what) in [
        (
            "cases/dev/short-ml-campaign/materials/job.py",
            "materials hash mismatch",
        ),
        (
            "cases/dev/short-ml-campaign/kb/snapshot.md",
            "kb snapshot hash mismatch",
        ),
        (
            "grader-private/dev/short-ml-campaign/private.json",
            "private file hash mismatch",
        ),
    ] {
        let (t, c, g) = copy();
        let p = t.path().join(rel);
        let mut s = fs::read_to_string(&p).unwrap();
        s.push(' ');
        fs::write(&p, s).unwrap();
        let e = CaseSet::load(&c, &g).expect_err(rel);
        assert!(e.contains(what), "{rel}: {e}");
    }
    // 配送入力（loader の外）は sha256 照合で拒否される。
    let (t, _, _) = copy();
    let p = t.path().join("train.csv");
    fs::copy(inputs_dir().join("train.csv"), &p).unwrap();
    let want = sha(&p);
    verify_file_sha256(&p, &want).unwrap();
    fs::write(&p, "x1,x2,x3,x4,y\n0,0,0,0,0\n").unwrap();
    assert!(verify_file_sha256(&p, &want).is_err());
}

#[test]
fn tampered_case_json_or_env_lock_breaks_the_contract_pin() {
    let k = json(&case_dir().join("contract.json"));
    for f in ["case.json", "env.lock.json"] {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join(f);
        let mut s = fs::read_to_string(case_dir().join(f)).unwrap();
        s.push(' ');
        fs::write(&p, s).unwrap();
        let key = if f == "case.json" {
            "case_json"
        } else {
            "env_lock"
        };
        assert_ne!(k["pins"][key], sha(&p).as_str(), "{f}");
    }
}
