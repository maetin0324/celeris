//! case × grader × isolation の統合試験（実 cluster・実 LLM は使わない。制御用 mock のみ）。
//! dev case を 3 config の分離 workspace に展開し、mock 実行器の成果を grader に通す。
#[path = "support_cases/mod.rs"]
#[allow(dead_code)]
mod support_cases;
use std::{
    fs,
    path::{Path, PathBuf},
};

use celeris_campaign::{
    cases::{CaseKind, CaseSet, LoadedCase},
    grader::{
        CaseSplit, CommandOutput, CommandRunner, GradeCase, GradeOptions, GradeReport, Status,
        Verdict, grade,
    },
    isolation::{
        InitialSources, TrialView, check_same_inputs, finish_trial, prepare_trial, verify_isolation,
    },
    manifest::ConfigId,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const PRISTINE: &str =
    "#[test]\nfn hidden_fsync() {\n    assert_eq!(f(1), 1);\n    assert_eq!(f(2), 2);\n}\n";
const OK_OUT: &str = "test hidden_fsync ... ok\ntest result: ok. 1 passed; 0 failed\n";

fn sha(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}

fn write(p: &Path, s: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, s).unwrap();
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn load_set() -> CaseSet {
    CaseSet::load(
        &support_cases::cases_root(),
        &root().join("fixtures/grader-private"),
    )
    .unwrap()
}

struct Stub;
impl CommandRunner for Stub {
    fn run(&self, _argv: &[String], cwd: &Path) -> Result<CommandOutput, String> {
        // 作業 dir に重なった評価テストが pristine のときだけ全試験 ok を返す疑似 cargo。
        let t = fs::read_to_string(cwd.join("tests/hidden.rs")).unwrap_or_default();
        let ok = t == PRISTINE;
        Ok(CommandOutput {
            exit: Some(if ok { 0 } else { 1 }),
            stdout: if ok { OK_OUT.into() } else { String::new() },
            stderr: String::new(),
        })
    }
}

/// 読み込んだ case の既知欠陥から grader 側の case 定義を組む。
fn grade_case(c: &LoadedCase, split: CaseSplit) -> GradeCase {
    let defects: Vec<Value> = c
        .private
        .known_defects
        .iter()
        .enumerate()
        .map(|(i, d)| {
            json!({"id": d.id, "file": format!("src/m{i}.rs"), "keywords": [format!("defect-{i}")]})
        })
        .collect();
    let review = if defects.is_empty() {
        Value::Null
    } else {
        json!({"known_defects": defects, "min_detection_rate": 1.0, "max_false_positives": 0})
    };
    let gc: GradeCase = serde_json::from_value(json!({
        "schema": "celeris.grade-case/1",
        "id": c.case.id,
        "split": split,
        "implementation": {"pristine_dir": "pristine", "repo": "repo",
            "tests": [{"path": "tests/hidden.rs", "sha256": sha(PRISTINE.as_bytes())}],
            "command": ["cargo", "test"], "must_pass": ["hidden_fsync"]},
        "experiment": {"min_jobs": 2, "conditions": {"nodes": 3}},
        "recompute": {"report": "report.md", "metrics": [
            {"name": "bw_mean", "agg": "mean", "tolerance_rel": 0.01,
             "source": {"json": {"files": ["raw/a.json", "raw/b.json"], "pointer": "/bw"}}}]},
        "review": review,
    }))
    .unwrap();
    gc.validate().unwrap();
    gc
}

struct Fx {
    _t: tempfile::TempDir,
    case_dir: PathBuf,
    art: PathBuf,
    work: PathBuf,
    observed: PathBuf,
}

/// 制御用 mock 実行器の成果。kind は証拠の種別（real / mock）、Passed の自己申告つき。
fn mock_artifact(c: &LoadedCase, kind: &str) -> Fx {
    let t = tempfile::tempdir().unwrap();
    let (case_dir, art, work) = (
        t.path().join("case"),
        t.path().join("art"),
        t.path().join("work"),
    );
    fs::create_dir_all(&work).unwrap();
    write(&case_dir.join("pristine/tests/hidden.rs"), PRISTINE);
    write(
        &art.join("repo/src/lib.rs"),
        "pub fn f(x: u32) -> u32 { x }\n",
    );
    write(&art.join("repo/tests/hidden.rs"), PRISTINE);
    let mut jobs = vec![];
    let mut seen = vec![];
    for (i, (n, bw)) in [("a", 100.0), ("b", 120.0)].iter().enumerate() {
        let body = json!({"bw": bw}).to_string();
        write(&art.join(format!("raw/{n}.json")), &body);
        let job = format!("{}.sirius", 42588 + i);
        write(
            &art.join(format!("evidence/{job}.qstat")),
            &format!("Job Id: {job}\n Exit_status = 0\n"),
        );
        write(&art.join(format!("evidence/{job}.log")), "done\n");
        seen.push(
            json!({"job_id": job, "exit_status": 0, "conditions": {"nodes": 3},
            "raw": [{"path": format!("raw/{n}.json"), "sha256": sha(body.as_bytes())}]}),
        );
        jobs.push(
            json!({"job_id": job, "scheduler_record": format!("evidence/{job}.qstat"),
            "log": format!("evidence/{job}.log"),
            "raw": [{"path": format!("raw/{n}.json"), "sha256": sha(body.as_bytes())}],
            "conditions": {"nodes": 3}, "evidence_kind": kind}),
        );
    }
    write(
        &art.join("evidence/jobs.json"),
        &json!({"jobs": jobs}).to_string(),
    );
    let observed = t.path().join("observed.json");
    write(
        &observed,
        &json!({"schema": "celeris.observed-jobs/1", "jobs": seen}).to_string(),
    );
    write(&art.join("report.md"), "metric bw_mean = 110.0\n");
    let findings: Vec<Value> = c
        .private
        .known_defects
        .iter()
        .enumerate()
        .map(|(i, _)| json!({"file": format!("src/m{i}.rs"), "line": 1, "summary": format!("defect-{i} found")}))
        .collect();
    write(
        &art.join("review/findings.json"),
        &json!({ "findings": findings }).to_string(),
    );
    // mock の Passed 自己申告と trial 記録（grader は読まない）
    write(&art.join("result.json"), r#"{"outcome":"passed"}"#);
    write(&art.join("trials.jsonl"), "{\"outcome\":\"passed\"}\n");
    Fx {
        _t: t,
        case_dir,
        art,
        work,
        observed,
    }
}

fn run(fx: &Fx, gc: &GradeCase) -> GradeReport {
    grade(
        gc,
        &fx.art,
        &GradeOptions {
            case_dir: fx.case_dir.clone(),
            work_root: fx.work.clone(),
            runner: &Stub,
            observed_jobs: Some(fx.observed.clone()),
            initial_tree: None,
        },
    )
}

fn why(r: &GradeReport) -> String {
    serde_json::to_string(&r.checks).unwrap()
}

#[test]
fn dev_case_expands_into_three_identical_isolated_workspaces() {
    let set = load_set();
    let k1 = set.get("k1-bf-fsync-campaign").unwrap();
    let d = tempfile::tempdir().unwrap();
    // 初期 repo の代わりの stand-in（隠し command・欠陥 id を含む file が混ざっていても除かれる）
    let repo = d.path().join("src/repo");
    write(&repo.join("lib.rs"), "fn chunk() {}");
    write(
        &repo.join("grader/pristine/hidden.rs"),
        &k1.private.hidden_command,
    );
    let cdir = root().join("cases/dev/k1-bf-fsync-campaign");
    let src = InitialSources {
        repo,
        materials: cdir.join("materials"),
        kb: cdir.join("kb"),
        budget: k1.case.budget.clone(),
        withheld_names: vec!["grader".into(), "private.json".into()],
    };
    let out = d.path().join("out");
    let ts: Vec<_> = [ConfigId::A, ConfigId::B, ConfigId::C]
        .into_iter()
        .map(|c| prepare_trial(&src, &out, c, &k1.case.id, 1).unwrap())
        .collect();
    let recs: Vec<_> = ts.iter().map(|t| t.record.clone()).collect();
    check_same_inputs(&recs).unwrap();
    // 初期 snapshot は case に固定した hash と一致する（KB・資料）
    assert_eq!(recs[0].kb_sha256, k1.case.snapshot.kb_snapshot);
    assert_eq!(recs[0].materials_sha256, k1.case.snapshot.materials_sha256);
    // 隔離: A の KB 書き込みは B・C に出ない
    fs::write(ts[0].kb.join("learned.md"), "A's answer").unwrap();
    assert!(!ts[1].kb.join("learned.md").exists() && !ts[2].kb.join("learned.md").exists());
    assert_ne!(finish_trial(&ts[0]).unwrap(), recs[0].kb_sha256);
    // dev 経路に final の期待値は無い
    let fin = set.get("k2-bf-xattr-restart-campaign").unwrap();
    let mut secrets = vec![fin.case.id.clone(), fin.private.hidden_command.clone()];
    secrets.extend(fin.private.known_defects.iter().map(|x| x.id.clone()));
    secrets.push(fin.case.initial.as_ref().unwrap().commit.clone());
    let views: Vec<_> = ts
        .iter()
        .map(|t| TrialView {
            id: t.id.clone(),
            root: t.root.clone(),
            owned_hashes: vec![],
            final_values: secrets.clone(),
        })
        .collect();
    assert_eq!(verify_isolation(&views).unwrap(), vec![]);
    assert!(!ts[1].workspace.join("grader").exists());
    // 試行に渡す dev 側の木（case と grader-private/dev）にも final 値が無い
    for base in [
        root().join("cases/dev"),
        root().join("fixtures/grader-private/dev"),
    ] {
        let mut st = vec![base];
        while let Some(p) = st.pop() {
            for e in fs::read_dir(p).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    st.push(p);
                } else {
                    let t = fs::read_to_string(&p).unwrap_or_default();
                    for s in &secrets {
                        assert!(!t.contains(s.as_str()), "{} leaks {s}", p.display());
                    }
                }
            }
        }
    }
}

#[test]
fn honest_real_evidence_passes_but_mock_passed_does_not_outside_control() {
    let set = load_set();
    let k1 = set.get("k1-bf-fsync-campaign").unwrap();
    let gc = grade_case(k1, CaseSplit::Dev);
    // 実証拠つきの成果は 4 検査すべて通る（対照）
    let real = mock_artifact(k1, "real");
    let r = run(&real, &gc);
    assert_eq!(r.verdict, Verdict::Pass, "{}", why(&r));
    // mock 実行器の Passed（証拠種別 mock）は control 以外では不合格
    let mock = mock_artifact(k1, "mock");
    let r = run(&mock, &gc);
    assert_eq!(r.checks.evidence.status, Status::Fail);
    assert!(!r.final_pass, "{}", why(&r));
    // control case は制御試験としてのみ合格扱いで最終合格にならない
    let k0 = set.get("k0-fixture-control").unwrap();
    assert_eq!(k0.case.kind, CaseKind::Control);
    assert!(
        set.select_for_measurement(std::slice::from_ref(&k0.case.id))
            .is_err()
    );
    let r = run(
        &mock_artifact(k0, "mock"),
        &grade_case(k0, CaseSplit::Control),
    );
    assert_eq!(r.verdict, Verdict::ControlOnly, "{}", why(&r));
    assert!(!r.final_pass);
}

#[test]
fn weakened_deleted_unexecuted_and_fabricated_results_fail() {
    let set = load_set();
    let k1 = set.get("k1-bf-fsync-campaign").unwrap();
    let gc = grade_case(k1, CaseSplit::Dev);
    // 評価テスト弱体化（assert 削除）→ 失格
    let fx = mock_artifact(k1, "real");
    write(
        &fx.art.join("repo/tests/hidden.rs"),
        "#[test]\nfn hidden_fsync() {}\n",
    );
    let r = run(&fx, &gc);
    assert_eq!(r.checks.implementation.status, Status::Fail, "{}", why(&r));
    assert!(!r.final_pass);
    // 評価テスト削除
    let fx = mock_artifact(k1, "real");
    fs::remove_file(fx.art.join("repo/tests/hidden.rs")).unwrap();
    assert!(!run(&fx, &gc).final_pass);
    // 未実行なのにもっともらしい Markdown（証拠・生データなし）
    let fx = mock_artifact(k1, "real");
    fs::remove_dir_all(fx.art.join("evidence")).unwrap();
    fs::remove_dir_all(fx.art.join("raw")).unwrap();
    let r = run(&fx, &gc);
    assert_eq!(r.checks.evidence.status, Status::Fail);
    assert_eq!(r.checks.recompute.status, Status::Fail);
    // 数値捏造（生データの平均は 110）
    let fx = mock_artifact(k1, "real");
    write(&fx.art.join("report.md"), "metric bw_mean = 500.0\n");
    let r = run(&fx, &gc);
    assert_eq!(r.checks.recompute.status, Status::Fail);
    assert!(!r.final_pass);
    // 成功宣言だけ
    let fx = mock_artifact(k1, "real");
    for d in ["repo", "evidence", "raw", "review"] {
        fs::remove_dir_all(fx.art.join(d)).unwrap();
    }
    write(&fx.art.join("report.md"), "全部成功した\n");
    assert_eq!(run(&fx, &gc).verdict, Verdict::Fail);
}

#[test]
fn final_case_is_graded_separately_and_its_expectations_are_private() {
    let set = load_set();
    let k2 = set.get("k2-bf-xattr-restart-campaign").unwrap();
    assert_eq!(k2.case.split, celeris_campaign::manifest::Split::Final);
    // final と dev は同時に本測定へ選べない
    assert!(
        set.select_for_measurement(&["k1-bf-fsync-campaign".into(), k2.case.id.clone()])
            .is_err()
    );
    // final の public 部（case.json・materials・kb）に隠し command・欠陥 id・参照値が無い
    let mut st = vec![root().join("cases/final/k2-bf-xattr-restart-campaign")];
    while let Some(p) = st.pop() {
        for e in fs::read_dir(p).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                st.push(p);
                continue;
            }
            let t = fs::read_to_string(&p).unwrap_or_default();
            assert!(!t.contains(&k2.private.hidden_command), "{}", p.display());
            for d in &k2.private.known_defects {
                assert!(!t.contains(&d.id), "{} leaks {}", p.display(), d.id);
            }
        }
    }
    let gc = grade_case(k2, CaseSplit::Final);
    assert_eq!(run(&mock_artifact(k2, "real"), &gc).verdict, Verdict::Pass);
    assert!(!run(&mock_artifact(k2, "mock"), &gc).final_pass);
}
