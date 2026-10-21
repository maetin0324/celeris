//! `campaign export --verify-connected`: mock 実行の campaign を一時 DB に作り、結合済み export が
//! 全欄を持てば成功、空・必須欄の欠測・消費未計上なら非 0 で失敗することを確かめる（実 API・実クラスタなし）。
mod common;

use serde_json::Value;
use std::process::{Command, Output};
use task_core::SqliteStore;
use task_core::decision_log::{
    CandidateRef, ClassifierRecord, DecisionLogExecution, DecisionLogOutcome, DecisionLogProposal,
    DecisionLogUsage, RunEndLabel, UsageKey, UsageRole,
};
use time::OffsetDateTime;

const CAMPAIGN: &str = "camp-1";

fn cand(m: &str) -> CandidateRef {
    CandidateRef {
        source_id: "s".into(),
        model_id: m.into(),
        harness_id: "h".into(),
    }
}

/// 独立 grader の出力（`grades.jsonl`）。`mh` は trial の manifest hash。
fn grades_file(dir: &std::path::Path, mh: &str, pass: bool) -> std::path::PathBuf {
    grades_file_for(dir, mh, "case-7", pass)
}

fn grades_file_for(dir: &std::path::Path, mh: &str, case: &str, pass: bool) -> std::path::PathBuf {
    let p = dir.join("grades.jsonl");
    let row = serde_json::json!({
        "schema": "celeris.campaign-grade/1",
        "campaign_sha256": mh,
        "case_id": case,
        "trial": {"config": "assisted", "problem_id": case, "rep": 2, "trial_no": 1},
        "grade": {"final_pass": pass},
    });
    std::fs::write(&p, format!("{row}\n")).unwrap();
    p
}

fn verify(db: &std::path::Path, campaign: &str, out: &std::path::Path) -> Output {
    let grades = grades_file(db.parent().unwrap(), "mh-abc", true);
    verify_with(db, campaign, out, Some(&grades))
}

fn verify_with(
    db: &std::path::Path,
    campaign: &str,
    out: &std::path::Path,
    grades: Option<&std::path::Path>,
) -> Output {
    let mut cmd = Command::new(common::celerisctl_path());
    cmd.args(["--db", db.to_str().unwrap(), "campaign", "export", campaign])
        .args(["--out", out.to_str().unwrap(), "--verify-connected"]);
    if let Some(g) = grades {
        cmd.args(["--grades", g.to_str().unwrap()]);
    }
    cmd.env_remove("CELERIS_DB")
        .env_remove("CELERIS_CONFIG")
        .env_remove("CELERIS_RUN_DB")
        .output()
        .expect("spawn celerisctl")
}

#[derive(Clone, Copy, Default)]
struct Gap {
    no_case: bool,
    no_grader: bool,
    no_usage: bool,
}

fn seed(store: &SqliteStore, gap: Gap) {
    let now = OffsetDateTime::UNIX_EPOCH;
    let p = DecisionLogProposal {
        run_id: "r1".into(),
        decision_id: Some("d1".into()),
        campaign_id: Some(CAMPAIGN.into()),
        arm: Some("assisted".into()),
        case_id: (!gap.no_case).then(|| "case-7".into()),
        repetition: Some(2),
        manifest_hash: Some("mh-abc".into()),
        routing_setting: Some("decision_assisted".into()),
        task_id: Some("t1".into()),
        proposed_first: Some(cand("first")),
        proposed_candidates: vec![cand("first"), cand("second")],
        classifier: Some(ClassifierRecord {
            kind: "jev".into(),
            model: Some("jev-m".into()),
            snapshot: Some("snap-1".into()),
            confidence: Some(0.8),
            probs: None,
            fallback: None,
        }),
        prompt_version: Some("pv1".into()),
        question_schema_version: Some("qs1".into()),
        provider_model_version: Some("pm-2026".into()),
        ..Default::default()
    };
    store.decision_log_record_proposal(&p, now).unwrap();
    let e = DecisionLogExecution {
        run_id: "r1".into(),
        exec_source_id: Some("s".into()),
        exec_model: Some("second".into()),
        fallback_reason: Some("rate_limit".into()),
        ..Default::default()
    };
    store.decision_log_record_execution(&e, now).unwrap();
    let o = DecisionLogOutcome {
        run_id: "r1".into(),
        run_end: Some(RunEndLabel::Completed),
        wall_ms: Some(1000),
        ..Default::default()
    };
    store.decision_log_record_outcome(&o, now).unwrap();
    if gap.no_usage {
        // 行はあるが何も観測できていない（全欄が欠測）。
        let mut u = DecisionLogUsage::new(
            UsageKey::Run {
                run_id: "r1".into(),
                usage_source: "proxy".into(),
            },
            UsageRole::Worker,
        );
        u.run_id = Some("r1".into());
        u.campaign_id = Some(CAMPAIGN.into());
        store.decision_log_record_usage(&u, now).unwrap();
        return;
    }
    let mut u = DecisionLogUsage::new(
        UsageKey::Run {
            run_id: "r1".into(),
            usage_source: "proxy".into(),
        },
        UsageRole::Worker,
    );
    u.run_id = Some("r1".into());
    u.campaign_id = Some(CAMPAIGN.into());
    u.input_tokens = Some(100);
    u.list_cost_usd = Some(0.5);
    u.wall_ms = Some(1000);
    store.decision_log_record_usage(&u, now).unwrap();
    let mut jev = DecisionLogUsage::jev_call("wu-1", 1, "t1", Some(CAMPAIGN), Some(0.01));
    jev.task_id = Some("t1".into());
    store.decision_log_record_usage(&jev, now).unwrap();
    let mut cos = DecisionLogUsage::cos_turn("sess", 1, Some(CAMPAIGN));
    cos.list_cost_usd = Some(0.25);
    store.decision_log_record_usage(&cos, now).unwrap();
}

fn db_with(gap: Option<Gap>) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("fixture.db");
    let store = SqliteStore::open(&db).unwrap();
    if let Some(g) = gap {
        seed(&store, g);
    }
    (dir, db)
}

#[test]
fn connected_campaign_exports_every_joined_field_and_passes() {
    let (dir, db) = db_with(Some(Gap::default()));
    let out = dir.path().join("o");
    let r = verify(&db, CAMPAIGN, &out);
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let rows = std::fs::read_to_string(out.join("records.jsonl")).unwrap();
    let trial: Value = rows
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .find(|v| v["record_type"] == "trial")
        .expect("trial record");
    for (k, v) in [
        ("campaign_id", "camp-1"),
        ("case_id", "case-7"),
        ("arm", "assisted"),
        ("manifest_hash", "mh-abc"),
        ("routing_setting", "decision_assisted"),
        ("exec_model", "second"),
        ("fallback_reason", "rate_limit"),
        ("classifier_kind", "jev"),
        ("classifier_snapshot", "snap-1"),
        ("prompt_version", "pv1"),
        ("question_schema_version", "qs1"),
        ("provider_model_version", "pm-2026"),
        ("grader_verdict", "pass"),
    ] {
        assert_eq!(trial[k], v, "{k}");
    }
    assert_eq!(trial["repetition"], 2);
    assert_eq!(trial["proposed_first"]["model_id"], "first");
    assert_eq!(trial["proposed_candidates"].as_array().unwrap().len(), 2);
    assert_eq!(trial["run_consumption"]["input_tokens"]["sum"], 100.0);
    // Jev の分類費用は task の最初の run に直接帰属、CoS は共有オーバーヘッドで別欄。
    assert_eq!(
        trial["task_level_consumption"]["list_cost_usd"]["sum"],
        0.01
    );
    let summary: Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("summary.json")).unwrap()).unwrap();
    assert_eq!(summary["shared_overhead"]["list_cost_usd"]["sum"], 0.25);
    assert_eq!(summary["total"]["list_cost_usd"]["sum"], 0.51);
    assert_eq!(
        summary["by_attribution_rule"]["cos_turn_shared_overhead"],
        1
    );
    assert!(summary["run_wall_sum_ms"]["sum"].is_number());
    assert!(summary["total"]["cash_cost_usd"]["sum"].is_null());
}

#[test]
fn empty_campaign_fails_the_connection_check() {
    let (dir, db) = db_with(None);
    let r = verify(&db, CAMPAIGN, &dir.path().join("o"));
    assert!(!r.status.success());
    assert!(String::from_utf8_lossy(&r.stderr).contains("export is empty"));
    // 存在しない campaign も同じ。
    let (dir, db) = db_with(Some(Gap::default()));
    assert!(!verify(&db, "other", &dir.path().join("o")).status.success());
}

#[test]
fn missing_required_fields_or_unmetered_consumption_fail() {
    for (gap, needle) in [
        (
            Gap {
                no_case: true,
                ..Gap::default()
            },
            "case_id is missing",
        ),
        (
            Gap {
                no_grader: true,
                ..Gap::default()
            },
            "grader_verdict is missing",
        ),
        (
            Gap {
                no_usage: true,
                ..Gap::default()
            },
            "no consumption was recorded",
        ),
    ] {
        let (dir, db) = db_with(Some(gap));
        // grade 行の無い試行は grader_verdict が欠測のまま（既定値で埋めない）。
        let r = if gap.no_grader {
            let g = grades_file_for(dir.path(), "mh-abc", "other-case", true);
            verify_with(&db, CAMPAIGN, &dir.path().join("o"), Some(&g))
        } else {
            verify(&db, CAMPAIGN, &dir.path().join("o"))
        };
        assert!(!r.status.success(), "{needle}");
        let err = String::from_utf8_lossy(&r.stderr);
        assert!(err.contains(needle), "{needle}: {err}");
    }
}

fn export_records(gap: Gap) -> (Vec<Value>, Value) {
    let (dir, db) = db_with(Some(gap));
    let out = dir.path().join("o");
    let _ = verify(&db, CAMPAIGN, &out);
    let rows = std::fs::read_to_string(out.join("records.jsonl")).unwrap();
    let recs = rows
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .collect();
    let summary =
        serde_json::from_str(&std::fs::read_to_string(out.join("summary.json")).unwrap()).unwrap();
    (recs, summary)
}

#[test]
fn export_keeps_raw_run_and_call_keys_and_states() {
    let (recs, summary) = export_records(Gap::default());
    let usage = |k: &str| {
        recs.iter()
            .find(|v| v["record_type"] == "usage" && v["dedup_key"] == k)
            .unwrap_or_else(|| panic!("usage {k}"))
    };
    let run = usage("run:r1:proxy");
    assert_eq!(run["run_id"], "r1");
    assert_eq!(run["usage_state"], "observed");
    assert_eq!(run["manifest_hash"], "mh-abc");
    assert_eq!(usage("decision:jev:wu-1:1")["usage_key"]["kind"], "call");
    assert_eq!(
        usage("decision:cos:sess:1")["usage_state"],
        "shared_unattributable"
    );
    let trial = recs.iter().find(|v| v["record_type"] == "trial").unwrap();
    assert_eq!(trial["consumption_state"], "observed");
    assert_eq!(
        summary["integrity"]["conflicts"].as_array().unwrap().len(),
        0
    );
    assert_eq!(summary["integrity"]["usage_without_run"], 0);
}

#[test]
fn unobserved_usage_row_is_missing_not_zero() {
    let (recs, _) = export_records(Gap {
        no_usage: true,
        ..Gap::default()
    });
    let trial = recs.iter().find(|v| v["record_type"] == "trial").unwrap();
    assert_eq!(trial["consumption_state"], "missing");
    let u = recs.iter().find(|v| v["record_type"] == "usage").unwrap();
    assert_eq!(u["usage_state"], "missing");
    assert!(u["input_tokens"].is_null());
}

/// B の run が worker 2 本と reviewer 1 本あっても、試行の grade 1 行が全 run に付く。reviewer 相当（routing 判断なし）の
/// run は proposed_first が無くても接続確認に通る（下の試験）。
#[test]
fn grade_row_joins_every_run_of_the_trial() {
    let (dir, db) = db_with(Some(Gap::default()));
    let out = dir.path().join("o");
    let g = grades_file(dir.path(), "mh-abc", false);
    let r = verify_with(&db, CAMPAIGN, &out, Some(&g));
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let rows = std::fs::read_to_string(out.join("records.jsonl")).unwrap();
    let verdicts: Vec<Value> = rows
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|v| v["record_type"] == "trial")
        .map(|v| v["grader_verdict"].clone())
        .collect();
    assert!(!verdicts.is_empty());
    assert!(verdicts.iter().all(|v| v == "fail"), "{verdicts:?}");
}

#[test]
fn grade_with_foreign_manifest_hash_is_rejected() {
    let (dir, db) = db_with(Some(Gap::default()));
    let g = grades_file(dir.path(), "other-hash", true);
    let r = verify_with(&db, CAMPAIGN, &dir.path().join("o"), Some(&g));
    assert!(!r.status.success());
    assert!(String::from_utf8_lossy(&r.stderr).contains("other-hash"));
}

#[test]
fn verify_connected_requires_grades() {
    let (dir, db) = db_with(Some(Gap::default()));
    let r = verify_with(&db, CAMPAIGN, &dir.path().join("o"), None);
    assert!(!r.status.success());
    assert!(String::from_utf8_lossy(&r.stderr).contains("--grades"));
}

/// reviewer run は routing_decided を持たず推奨候補が無い。仕様上欠けてよいので --verify-connected は通る。
#[test]
fn run_without_routing_decision_may_lack_proposed_first() {
    let (dir, db) = db_with(Some(Gap::default()));
    let store = SqliteStore::open(&db).unwrap();
    let now = OffsetDateTime::UNIX_EPOCH;
    let p = DecisionLogProposal {
        run_id: "rv".into(),
        campaign_id: Some(CAMPAIGN.into()),
        arm: Some("assisted".into()),
        case_id: Some("case-7".into()),
        repetition: Some(2),
        manifest_hash: Some("mh-abc".into()),
        task_id: Some("t1".into()),
        ..Default::default()
    };
    store.decision_log_record_proposal(&p, now).unwrap();
    store
        .decision_log_record_execution(
            &DecisionLogExecution {
                run_id: "rv".into(),
                exec_source_id: Some("s".into()),
                exec_model: Some("m".into()),
                ..Default::default()
            },
            now,
        )
        .unwrap();
    store
        .decision_log_record_outcome(
            &DecisionLogOutcome {
                run_id: "rv".into(),
                run_end: Some(RunEndLabel::Completed),
                wall_ms: Some(10),
                ..Default::default()
            },
            now,
        )
        .unwrap();
    let mut u = DecisionLogUsage::new(
        UsageKey::Run {
            run_id: "rv".into(),
            usage_source: "proxy".into(),
        },
        UsageRole::Reviewer,
    );
    u.run_id = Some("rv".into());
    u.campaign_id = Some(CAMPAIGN.into());
    u.input_tokens = Some(10);
    store.decision_log_record_usage(&u, now).unwrap();
    drop(store);
    let g = grades_file(dir.path(), "mh-abc", true);
    let r = verify_with(&db, CAMPAIGN, &dir.path().join("o"), Some(&g));
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
}
