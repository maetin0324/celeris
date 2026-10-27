//! CLI coverage for `campaign export`: 一時 DB の fixture から出力し、行数・欠測 null・二重計上なし・
//! 別 campaign の除外・決定的出力・秘密を含まないことを確かめる。
mod common;

use serde_json::Value;
use std::process::{Command, Output};
use task_core::SqliteStore;
use task_core::decision_log::{
    CandidateRef, DecisionLogExecution, DecisionLogIntervention, DecisionLogOutcome,
    DecisionLogProposal, DecisionLogUsage, InterventionActorKind, InterventionKind, UsageKey,
    UsageRole,
};
use time::OffsetDateTime;

const SECRET: &str = "sk-SECRETSECRETSECRET1234";

fn ctl(args: &[&str]) -> Output {
    Command::new(common::celerisctl_path())
        .args(args)
        .env_remove("CELERIS_DB")
        .env_remove("CELERIS_CONFIG")
        .env_remove("CELERIS_RUN_DB")
        .output()
        .expect("spawn celerisctl")
}

fn cand(m: &str) -> CandidateRef {
    CandidateRef {
        source_id: "s".into(),
        model_id: m.into(),
        harness_id: "h".into(),
    }
}

fn seed(store: &SqliteStore, run_id: &str, campaign: &str) {
    let now = OffsetDateTime::UNIX_EPOCH;
    let p = DecisionLogProposal {
        run_id: run_id.into(),
        campaign_id: Some(campaign.into()),
        task_id: Some(format!("t-{run_id}")),
        proposed_first: Some(cand("first")),
        proposed_candidates: vec![cand("first"), cand("second")],
        ..Default::default()
    };
    store.decision_log_record_proposal(&p, now).unwrap();
    let e = DecisionLogExecution {
        run_id: run_id.into(),
        exec_source_id: Some("s".into()),
        exec_model: Some("second".into()),
        // 秘密らしい値が自由文欄に混ざっても出力に出ない。
        fallback_reason: Some(format!("rate_limit key {SECRET}")),
        ..Default::default()
    };
    store.decision_log_record_execution(&e, now).unwrap();
    let o = DecisionLogOutcome {
        run_id: run_id.into(),
        run_end: Some(task_core::decision_log::RunEndLabel::Failed),
        wall_ms: Some(1000),
        ..Default::default()
    };
    store.decision_log_record_outcome(&o, now).unwrap();
    let mut u = DecisionLogUsage::new(
        UsageKey::Run {
            run_id: run_id.into(),
            usage_source: "proxy".into(),
        },
        UsageRole::Worker,
    );
    u.run_id = Some(run_id.into());
    u.campaign_id = Some(campaign.into());
    u.input_tokens = Some(100);
    store.decision_log_record_usage(&u, now).unwrap();
}

fn export(db: &std::path::Path, campaign: &str, out: &std::path::Path) -> (String, Value) {
    let r = ctl(&[
        "--db",
        db.to_str().unwrap(),
        "campaign",
        "export",
        campaign,
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let rows = std::fs::read_to_string(out.join("records.jsonl")).unwrap();
    let summary: Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("summary.json")).unwrap()).unwrap();
    (rows, summary)
}

#[test]
fn campaign_export_is_deterministic_deduped_scoped_and_secret_free() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("fixture.db");
    {
        let store = SqliteStore::open(&db).unwrap();
        seed(&store, "r1", "c1");
        seed(&store, "r1", "c1"); // 再処理
        seed(&store, "r2", "c2"); // 別 campaign
        store
            .decision_log_record_intervention(&DecisionLogIntervention {
                intervention_id: "i1".into(),
                kind: InterventionKind::ResearchJudgment,
                actor_kind: InterventionActorKind::Human,
                actor_id: Some("alice".into()),
                task_id: Some("t-r1".into()),
                campaign_id: Some("c1".into()),
                at: "2026-10-15T00:00:00Z".into(),
                wall_ms: None,
                source_event: None,
            })
            .unwrap();
    }
    let (rows1, summary1) = export(&db, "c1", &dir.path().join("o1"));
    let (rows2, summary2) = export(&db, "c1", &dir.path().join("o2"));
    assert_eq!(rows1, rows2, "records.jsonl is deterministic");
    assert_eq!(summary1, summary2);
    let raw_summary = std::fs::read_to_string(dir.path().join("o1/summary.json")).unwrap();
    assert_eq!(
        raw_summary,
        std::fs::read_to_string(dir.path().join("o2/summary.json")).unwrap()
    );

    // 行数: decision/run/outcome/usage 各 1 + intervention 1 以内（r1 の二重記録・c2 は数えない）。
    let parsed: Vec<Value> = rows1
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let count = |t: &str| parsed.iter().filter(|r| r["record_type"] == t).count();
    assert_eq!(count("usage"), 1, "no double counting");
    assert_eq!(count("intervention"), 1);
    assert!(
        parsed.iter().all(|r| !r.to_string().contains("r2")),
        "other campaign excluded"
    );

    // 欠測は 0 でなく null / n_missing。
    let w = &summary1["total"];
    assert_eq!(w["input_tokens"]["sum"], 100.0);
    assert_eq!(w["input_tokens"]["n_observed"], 1);
    assert!(w["output_tokens"]["sum"].is_null());
    assert_eq!(w["output_tokens"]["n_missing"], 1);
    assert!(
        w["cash_cost_usd"]["sum"].is_null(),
        "cash is never substituted by list price"
    );

    // 秘密を含まない。
    assert!(!rows1.contains(SECRET) && !raw_summary.contains(SECRET));
    assert!(rows1.contains("[redacted]"));
}
