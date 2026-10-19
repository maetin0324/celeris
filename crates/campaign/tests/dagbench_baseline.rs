//! baseline: 固定 scenario の完走・外部 ledger との整合・state からの再開。論理時計だけで決定的。
use std::path::{Path, PathBuf};

use celeris_campaign::dagbench::{
    baseline::{Baseline, BaselineConfig, Outcome, Status, load_scenario},
    ext::*,
};

fn scenarios() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/dagbench/scenarios.json")
}

fn ext_cfg(faults: Vec<FaultSpec>) -> ExtConfig {
    ExtConfig {
        poll_secs: 30,
        job_slots: 2,
        faults,
    }
}

fn bcfg() -> BaselineConfig {
    BaselineConfig {
        poll_secs: 30,
        ext_job_secs: 330,
    }
}

fn open(dir: &Path, scenario: &str, faults: Vec<FaultSpec>) -> (Baseline, ExtSim) {
    let sc = load_scenario(&scenarios(), scenario).unwrap();
    let b = Baseline::open(sc, bcfg(), &dir.join("state.json")).unwrap();
    let e = ExtSim::open(&dir.join("ext.jsonl"), ext_cfg(faults)).unwrap();
    (b, e)
}

fn assert_ledger(dir: &Path, jobs: usize) {
    let entries = read_ledger(&dir.join("ext.jsonl")).unwrap();
    let c = count(&entries);
    assert_eq!(c.logical_jobs, jobs);
    assert_eq!(
        c.physical_launches, jobs,
        "one physical launch per logical job"
    );
    assert_eq!(c.consumed, jobs, "each job consumed exactly once");
    assert!(entries.iter().all(|e| e.dataset == DATASET));
}

#[test]
fn k2_mini_completes_and_matches_the_external_ledger() {
    let dir = tempfile::tempdir().unwrap();
    let (mut b, mut e) = open(dir.path(), "k2-mini", vec![]);
    assert_eq!(b.run(&mut e).unwrap(), Outcome::Complete);
    // 570 (survey+impl+verify) + 60 pre + 330 job + 30 post + 120 + 120 + 90
    assert_eq!(b.makespan(), Some(1320));
    assert!(b.state().nodes.values().all(|n| n.status == Status::Done));
    assert_ledger(dir.path(), 2);
}

#[test]
fn k2_wide_queues_on_the_external_slots() {
    let dir = tempfile::tempdir().unwrap();
    let (mut b, mut e) = open(dir.path(), "k2-wide", vec![]);
    assert_eq!(b.run(&mut e).unwrap(), Outcome::Complete);
    assert_ledger(dir.path(), 4);
}

/// 保存回数 k の直後に止め（制御 process 停止）、同じ state / ledger で開き直して完走する。
fn resume_at_every_save_point(faults: Vec<FaultSpec>) {
    let total = {
        let dir = tempfile::tempdir().unwrap();
        let (mut b, mut e) = open(dir.path(), "k2-mini", faults.clone());
        b.run(&mut e).unwrap();
        b.saves()
    };
    assert!(total > 5);
    // 保存ごとの fsync が遅い fs でも終わるよう、停止点は間引く（投入前後を含む十分な密度）。
    for k in (1..=total).step_by(3) {
        let dir = tempfile::tempdir().unwrap();
        {
            let (mut b, mut e) = open(dir.path(), "k2-mini", faults.clone());
            b.stop_after_saves(k);
            if b.run(&mut e).unwrap() == Outcome::Complete {
                continue;
            }
        }
        let (mut b, mut e) = open(dir.path(), "k2-mini", faults.clone());
        assert_eq!(b.run(&mut e).unwrap(), Outcome::Complete, "stop at {k}");
        assert!(b.state().nodes.values().all(|n| n.status == Status::Done));
        assert_ledger(dir.path(), 2);
    }
}

#[test]
fn resume_from_state_completes_from_every_stop_point() {
    resume_at_every_save_point(vec![]);
}

#[test]
fn resume_with_lost_response_does_not_double_launch() {
    resume_at_every_save_point(vec![FaultSpec {
        fault_id: "b1".into(),
        kind: FaultKind::DropResponse,
        event: "external_submit_accepted".into(),
        seed: 0,
        occurrences: 1,
    }]);
}

#[test]
fn duplicate_notices_are_ignored_and_consumed_once() {
    let dir = tempfile::tempdir().unwrap();
    let f = FaultSpec {
        fault_id: "c1".into(),
        kind: FaultKind::DuplicateNotice { extra: 2 },
        event: "external_job_completed".into(),
        seed: 0,
        occurrences: 1,
    };
    let (mut b, mut e) = open(dir.path(), "k2-mini", vec![f]);
    assert_eq!(b.run(&mut e).unwrap(), Outcome::Complete);
    assert_ledger(dir.path(), 2);
}

#[test]
fn corrupt_state_is_an_error_not_a_silent_restart() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("state.json"), "{not json").unwrap();
    let sc = load_scenario(&scenarios(), "k2-mini").unwrap();
    assert!(Baseline::open(sc, bcfg(), &dir.path().join("state.json")).is_err());
}
