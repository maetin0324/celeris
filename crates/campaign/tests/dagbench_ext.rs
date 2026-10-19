//! 外部 job stub: 応答喪失・通知重複・ledger 再読込。論理時計と event 到達だけで決定的（sleep なし）。
use celeris_campaign::dagbench::ext::*;

fn cfg(faults: Vec<FaultSpec>) -> ExtConfig {
    ExtConfig {
        poll_secs: 30,
        job_slots: 4,
        faults,
    }
}

fn fault(id: &str, kind: FaultKind, event: &str, seed: u64, occ: u64) -> FaultSpec {
    FaultSpec {
        fault_id: id.into(),
        kind,
        event: event.into(),
        seed,
        occurrences: occ,
    }
}

#[test]
fn lost_response_launches_exactly_once_and_lookup_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("ext.jsonl");
    // seed 4, occurrences 3 -> nth = 2: 2 件目の submit で応答を落とす。
    let f = fault(
        "b1",
        FaultKind::DropResponse,
        "external_submit_accepted",
        4,
        3,
    );
    assert_eq!(f.nth(), 2);
    let mut sim = ExtSim::open(&p, cfg(vec![f])).unwrap();
    assert!(sim.submit("j1", 60).is_ok());
    assert_eq!(sim.submit("j2", 60), Err(SubmitError::ResponseLost));
    // 制御側は冪等キーで再照合し、再投入しても既存の起動に寄る。
    let Lookup::Known { physical_id, .. } = sim.lookup("j2") else {
        panic!("accepted job must be found");
    };
    let again = sim.submit("j2", 60).unwrap();
    assert!(again.deduped);
    assert_eq!(again.physical_id, physical_id);

    let entries = read_ledger(&p).unwrap();
    let c = count(&entries);
    assert_eq!(c.physical_launches, 2);
    assert_eq!(c.logical_jobs, 2);
    let fired: Vec<_> = entries
        .iter()
        .filter(|e| e.event == "fault_fired")
        .collect();
    assert_eq!(fired.len(), 1);
    let d = fired[0].detail.as_ref().unwrap();
    assert_eq!((d["seed"].as_u64(), d["nth"].as_u64()), (Some(4), Some(2)));
    assert_eq!(fired[0].logical_job_id.as_deref(), Some("j2"));
    assert!(d["event_seq"].as_u64().unwrap() > 0);
}

#[test]
fn duplicate_completion_notice_yields_one_consumption() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("ext.jsonl");
    let f = fault(
        "c1",
        FaultKind::DuplicateNotice { extra: 2 },
        "external_job_completed",
        0,
        1,
    );
    let mut sim = ExtSim::open(&p, cfg(vec![f])).unwrap();
    sim.submit("j1", 60).unwrap();
    assert!(sim.advance(59).unwrap().is_empty());
    let mut notices = sim.advance(60).unwrap();
    notices.extend(sim.advance(90).unwrap());
    notices.extend(sim.advance(120).unwrap());
    assert_eq!(notices.len(), 3);
    let mut consumed = 0;
    for n in &notices {
        if sim.record_consumed(&n.logical_job_id).unwrap() {
            consumed += 1;
        }
    }
    assert_eq!(consumed, 1);
    let entries = read_ledger(&p).unwrap();
    let c = count(&entries);
    assert_eq!((c.consumed, c.notices_sent, c.physical_launches), (1, 3, 1));
    assert_eq!(
        entries.iter().filter(|e| e.event == "fault_fired").count(),
        1
    );
}

#[test]
fn reopening_the_ledger_restores_state_and_does_not_refire() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("ext.jsonl");
    let f = fault(
        "b1",
        FaultKind::DropResponse,
        "external_submit_accepted",
        0,
        1,
    );
    {
        let mut sim = ExtSim::open(&p, cfg(vec![f.clone()])).unwrap();
        assert_eq!(sim.submit("j1", 30), Err(SubmitError::ResponseLost));
        sim.submit("j2", 30).unwrap();
        sim.advance(30).unwrap();
        sim.record_consumed("j1").unwrap();
    } // 別の寿命: drop して開き直す。
    let before = read_ledger(&p).unwrap();
    let mut sim = ExtSim::open(&p, cfg(vec![f])).unwrap();
    assert_eq!(sim.now(), 30);
    assert_eq!(
        sim.lookup("j1"),
        Lookup::Known {
            physical_id: "phys-1".into(),
            state: JobState::Completed
        }
    );
    assert_eq!(sim.lookup("nope"), Lookup::Unknown);
    // 消費済みは復元され、再消費は記録されない。発火済み fault は再発火しない。
    assert!(!sim.record_consumed("j1").unwrap());
    assert!(sim.record_consumed("j2").unwrap());
    sim.submit("j3", 30).unwrap();
    let after = read_ledger(&p).unwrap();
    assert!(after.len() > before.len());
    assert_eq!(after[..before.len()], before[..]);
    let seqs: Vec<_> = after.iter().map(|e| e.seq).collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(after.iter().filter(|e| e.event == "fault_fired").count(), 1);
    assert_eq!(count(&after).consumed, 2);
}

#[test]
fn slot_limit_rejects_without_launching() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("ext.jsonl");
    let mut sim = ExtSim::open(
        &p,
        ExtConfig {
            poll_secs: 30,
            job_slots: 1,
            faults: vec![],
        },
    )
    .unwrap();
    sim.submit("a", 30).unwrap();
    assert_eq!(sim.submit("b", 30), Err(SubmitError::NoSlot));
    assert_eq!(count(&read_ledger(&p).unwrap()).physical_launches, 1);
}
