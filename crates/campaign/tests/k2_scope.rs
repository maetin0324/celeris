//! K2 の判定範囲（ADR 2026-10-18-k2-judged-scope）: 実装と実験だけを判定し、分析・レビューは判定せず主張もさせない。
#[path = "support_cases/mod.rs"]
#[allow(dead_code)]
mod support_cases;
use std::{
    fs,
    path::{Path, PathBuf},
};

use celeris_campaign::{
    cases::Case,
    grader::{GradeCase, GradeOptions, NoRunner, grade},
    lock::{GraderPin, LockedCase},
    lock_exec::verify_grade_case,
    manifest::Split,
    report::{check_scope_claims, scope_line},
};
use sha2::{Digest, Sha256};

const K2: &str = "k2-bf-xattr-restart-campaign";
/// この変更の前の grade-case の hash（旧 lock が持っていた値）。
const OLD_GRADE_CASE_SHA: &str = "bbaca17a0b6aa8b7fbc9934da498b85b9d0f95ab434ede1e25b8ca21b1f0fb54";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn case() -> Case {
    serde_json::from_slice(
        &fs::read(
            support_cases::cases_root()
                .join("final")
                .join(K2)
                .join("case.json"),
        )
        .expect("read"),
    )
    .expect("case")
}

fn grade_path() -> PathBuf {
    root()
        .join("fixtures/grade-cases/final")
        .join(K2)
        .join(format!("{K2}.json"))
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| (*x).to_string()).collect()
}

#[test]
fn k2_case_declares_what_is_and_is_not_judged() {
    let c = case();
    assert_eq!(c.judged_scope(), s(&["implementation", "experiment"]));
    assert_eq!(c.not_judged(), s(&["analysis", "review"]));
    let g = GradeCase::load(&grade_path()).expect("grade-case");
    assert_eq!(g.judged_scope, Some(s(&["implementation", "experiment"])));
    assert!(g.recompute.is_none() && g.review.is_none());
}

#[test]
fn judged_scope_outside_stages_is_rejected() {
    let mut v: serde_json::Value = serde_json::from_slice(
        &fs::read(
            support_cases::cases_root()
                .join("final")
                .join(K2)
                .join("case.json"),
        )
        .expect("r"),
    )
    .expect("j");
    v["judged_scope"] = serde_json::json!(["implementation", "deploy"]);
    let c: Case = serde_json::from_value(v).expect("shape");
    assert!(c.validate().is_err());
}

#[test]
fn grade_case_cannot_judge_analysis_without_a_recompute_or_hide_a_present_one() {
    let mut v: serde_json::Value =
        serde_json::from_slice(&fs::read(grade_path()).expect("r")).expect("j");
    v["judged_scope"] = serde_json::json!(["implementation", "experiment", "analysis"]);
    let g: GradeCase = serde_json::from_value(v.clone()).expect("shape");
    let e = g
        .validate()
        .expect_err("analysis judged but recompute null");
    assert!(e.contains("analysis"), "{e}");
    v["judged_scope"] = serde_json::json!(["implementation"]);
    let g: GradeCase = serde_json::from_value(v).expect("shape");
    assert!(
        g.validate()
            .expect_err("experiment present but not in scope")
            .contains("experiment")
    );
}

#[test]
fn grade_report_carries_the_scope_even_when_failing() {
    let g = GradeCase::load(&grade_path()).expect("grade-case");
    let t = tempfile::tempdir().expect("tmp");
    let opts = GradeOptions {
        case_dir: grade_path().parent().expect("p").to_path_buf(),
        work_root: t.path().to_path_buf(),
        runner: &NoRunner,
        observed_jobs: None,
        initial_tree: None,
    };
    let rep = grade(&g, t.path(), &opts);
    assert_eq!(rep.judged_scope, s(&["implementation", "experiment"]));
    assert_eq!(rep.not_judged, s(&["analysis", "review"]));
    let j = serde_json::to_value(&rep).expect("json");
    assert_eq!(j["not_judged"], serde_json::json!(["analysis", "review"]));
    assert_eq!(j["checks"]["recompute"]["status"], "not_applicable");
}

#[test]
fn report_states_not_judged_and_cannot_claim_analysis_or_review() {
    let (j, n) = (
        s(&["implementation", "experiment"]),
        s(&["analysis", "review"]),
    );
    let line = scope_line(&j, &n);
    assert_eq!(
        line,
        "judged_scope stages=implementation,experiment not_judged=analysis,review"
    );
    assert!(
        check_scope_claims(
            &j,
            &n,
            &format!("{line}\nresult A passed=1 n=1 split=final\n")
        )
        .is_empty()
    );
    // 宣言が無い／食い違う。
    assert!(!check_scope_claims(&j, &n, "result A passed=1 n=1 split=final\n").is_empty());
    let wrong = "judged_scope stages=implementation,experiment,analysis not_judged=review";
    assert!(!check_scope_claims(&j, &n, wrong).is_empty());
    // 主張文。
    for claim in [
        "verified stage=analysis",
        "verified stage=review",
        "分析・レビューまで検証済み",
        "analysis and review verified",
    ] {
        let v = check_scope_claims(&j, &n, &format!("{line}\n{claim}\n"));
        assert_eq!(v.len(), 1, "{claim}: {v:?}");
    }
    assert!(
        check_scope_claims(&j, &n, &format!("{line}\nverified stage=implementation\n")).is_empty()
    );
    // 全工程を判定する campaign には制限しない。
    assert!(check_scope_claims(&s(&["analysis"]), &[], "verified stage=analysis").is_empty());
}

#[test]
fn updated_grade_case_matches_the_new_pin_and_not_the_old_lock() {
    let cj = case();
    let pin = cj.grade_case_ref.expect("ref").sha256;
    let now = format!("{:x}", Sha256::digest(fs::read(grade_path()).expect("r")));
    assert_eq!(
        pin, now,
        "case.json grade_case_ref must be updated with the grade-case"
    );
    assert_ne!(pin, OLD_GRADE_CASE_SHA);
    let locked = |sha: &str| LockedCase {
        id: K2.into(),
        split: Split::Final,
        implementation: true,
        initial: None,
        inputs: vec![],
        grader: GraderPin {
            private_sha256: "0".repeat(64),
            grade_case_sha256: sha.into(),
        },
        tests: vec![],
    };
    // 旧 lock は新 grade-case を採点に使えない（旧試行と混ぜない）。
    let e = verify_grade_case(&locked(OLD_GRADE_CASE_SHA), &grade_path()).expect_err("old lock");
    assert!(e.contains("differs from the locked"), "{e}");
}
