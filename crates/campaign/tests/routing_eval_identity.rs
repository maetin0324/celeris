//! 集計の識別: 別 manifest・別 split（dev 以外）の trial を同じ cell に集計しない。
//! 失敗（error）は合否の分母から外れるが、数は必ず出る（campaign 側は分母に残す: celeris-campaign）。
use std::collections::BTreeMap;

use celeris_campaign::aggregate::dev_trials;
use routing_eval::{
    aggregate::{Trial, build},
    manifest::{Manifest, Problem},
};
use serde_json::json;
use task_core::routing_evidence::EvidenceKey;

fn problem(id: &str, split: &str) -> Problem {
    Problem {
        id: id.into(),
        task_id: "t".into(),
        work_unit_id: "w".into(),
        created_at: "".into(),
        genre: "coding".into(),
        kind: "task".into(),
        command: "check".into(),
        prompt: "Fixture".into(),
        split: split.into(),
        stratum: "coding:task".into(),
        subset_12: false,
        head_commit: None,
        base_commit: None,
        base_commit_from_parent: false,
        features_sha256: "".into(),
        features: json!({}),
        sha256: "".into(),
    }
}

fn manifest(problems: Vec<Problem>, sha: &str) -> Manifest {
    Manifest {
        schema: "x".into(),
        version: "1".into(),
        cutoff: "".into(),
        seed: 0,
        problems,
        refill_rejected: Vec::new(),
        sha256: sha.into(),
    }
}

fn trial(problem_id: &str, passed: bool, eval_status: Option<&str>) -> Trial {
    Trial {
        decision_id: format!("d-{problem_id}"),
        problem_id: problem_id.into(),
        trial_no: 1,
        key: EvidenceKey {
            lane: String::new(),
            genre: "coding".into(),
            harness_id: "claude_code".into(),
            harness_version: "1".into(),
            source_id: "s".into(),
            model_id: "m".into(),
        },
        passed,
        timed_out: false,
        wall_secs: None,
        usd_list: None,
        quota: BTreeMap::new(),
        status: "passed".into(),
        eval_status: eval_status.map(str::to_owned),
    }
}

#[test]
fn held_out_trials_do_not_join_the_dev_cell() {
    let m = manifest(vec![problem("d1", "dev"), problem("h1", "held-out")], "m1");
    let all = [trial("d1", true, None), trial("h1", false, None)];
    let kept: Vec<Trial> = dev_trials(&m, &all).into_iter().cloned().collect();
    let (agg, _) = build(&m, &kept);
    assert_eq!(agg.rows.len(), 1);
    assert_eq!((agg.rows[0].n, agg.rows[0].pass), (1, 1));
    assert_eq!(agg.excluded_total(), 0);
}

#[test]
fn trials_of_another_manifest_are_not_aggregated() {
    let other = manifest(vec![problem("x1", "dev")], "m2");
    let m = manifest(vec![problem("d1", "dev")], "m1");
    let all = [trial("d1", true, None), trial("x1", true, None)];
    let kept: Vec<Trial> = dev_trials(&m, &all).into_iter().cloned().collect();
    let (agg, _) = build(&m, &kept);
    assert_eq!(agg.manifest_sha256, "m1");
    assert_ne!(agg.manifest_sha256, other.sha256);
    assert_eq!(agg.rows[0].n, 1);
    let only_x = [trial("x1", true, None)];
    let kept: Vec<Trial> = dev_trials(&m, &only_x).into_iter().cloned().collect();
    let (empty, _) = build(&m, &kept);
    assert!(empty.rows.is_empty());
}

#[test]
fn failures_are_never_silently_dropped() {
    let m = manifest(vec![problem("d1", "dev"), problem("d2", "dev")], "m1");
    let trials = [trial("d1", true, None), trial("d2", false, Some("error"))];
    let (agg, _) = build(&m, &trials);
    // 根拠表は error を分母から外すが、除外数は cell ごとに必ず載る。
    assert_eq!(agg.rows[0].n, 1);
    assert_eq!(agg.excluded_total(), 1);
}
