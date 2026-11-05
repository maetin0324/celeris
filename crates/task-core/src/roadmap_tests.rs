use super::*;

fn spec(id: &str, deps: &[&str]) -> RoadmapOutcomeSpec {
    RoadmapOutcomeSpec {
        id: id.into(),
        title: format!("{id} の状態"),
        acceptance: vec![format!("{id} を確かめる")],
        priority: 1,
        depends_on: deps.iter().map(|d| (*d).to_owned()).collect(),
        dropped: false,
        checks: Vec::new(),
    }
}

fn input(outcomes: Vec<RoadmapOutcomeSpec>) -> RoadmapProposalInput {
    RoadmapProposalInput {
        base_version: None,
        change_reason: "人の方針".into(),
        outcomes,
    }
}

#[test]
fn project_roadmap_validate_shape_matches_request_block_rules() {
    assert!(validate_proposal(None, &input(vec![spec("o1", &[]), spec("o2", &["o1"])])).is_ok());
    let dup = validate_proposal(None, &input(vec![spec("o1", &[]), spec("o1", &[])])).unwrap_err();
    assert_eq!(dup.code, "roadmap_invalid");
    let cyc =
        validate_proposal(None, &input(vec![spec("a", &["b"]), spec("b", &["a"])])).unwrap_err();
    assert!(cyc.detail.contains("cycle"), "{cyc:?}");
    let unknown = validate_proposal(None, &input(vec![spec("a", &["zz"])])).unwrap_err();
    assert!(unknown.detail.contains("unknown"), "{unknown:?}");
    let mut empty = spec("a", &[]);
    empty.acceptance.clear();
    assert!(validate_proposal(None, &input(vec![empty])).is_err());
    let mut blank = input(vec![spec("a", &[])]);
    blank.change_reason = "  ".into();
    assert!(validate_proposal(None, &blank).is_err());
}

#[test]
fn project_roadmap_validate_stable_ids_cannot_disappear() {
    let prev = vec![spec("o1", &[]), spec("o2", &["o1"])];
    let removed = validate_proposal(Some(&prev), &input(vec![spec("o1", &[])])).unwrap_err();
    assert_eq!(removed.code, "roadmap_outcome_id_removed");
    let mut dropped = spec("o2", &["o1"]);
    dropped.dropped = true;
    assert!(validate_proposal(Some(&prev), &input(vec![spec("o1", &[]), dropped.clone()])).is_ok());
    let same = validate_proposal(Some(&prev), &input(prev.clone())).unwrap_err();
    assert_eq!(same.code, "roadmap_no_change");
    // 取り下げた Outcome に依存する Outcome は置けない。
    let mut d1 = spec("o1", &[]);
    d1.dropped = true;
    let bad = validate_proposal(Some(&prev), &input(vec![d1, spec("o2", &["o1"])])).unwrap_err();
    assert!(bad.detail.contains("dropped"), "{bad:?}");
}

#[test]
fn project_roadmap_view_status_and_root_links_come_from_records() {
    let mut o2 = spec("o2", &[]);
    o2.dropped = true;
    let v = RoadmapVersion {
        project_id: "p".into(),
        version: 2,
        status: RoadmapStatus::Approved,
        base_version: Some(1),
        request_sha256: "h".into(),
        change_reason: "r".into(),
        outcomes: vec![spec("o1", &[]), o2, spec("o3", &[])],
        proposed_by: "cos:t".into(),
        proposed_at: "2026-11-04T00:00:00Z".into(),
        decided_by: Some("human:r".into()),
        decided_at: Some("2026-11-04T00:00:01Z".into()),
        decision_note: None,
        superseded_at: None,
    };
    let ev = |id, kind: &str, status, root: Option<&str>| OutcomeEvent {
        id,
        outcome_id: "o1".into(),
        kind: kind.into(),
        status,
        root_task_id: root.map(str::to_owned),
        roadmap_version: 1,
        evidence: root.map(|r| vec![format!("task:{r}")]).unwrap_or_default(),
        reason: String::new(),
        actor: "human:r".into(),
        created_at: "2026-11-04T00:00:02Z".into(),
    };
    let events = vec![
        ev(1, "root_linked", None, Some("r2")),
        ev(2, "status", Some(OutcomeStatus::Reached), Some("r2")),
    ];
    let launched = vec![("r1".to_owned(), vec!["o1".to_owned()])];
    let view = build_view(&v, &events, &launched);
    assert_eq!(view.outcomes[0].status, OutcomeStatus::Reached);
    assert_eq!(view.outcomes[0].root_task_ids, vec!["r1", "r2"]);
    assert_eq!(view.outcomes[0].evidence, vec!["task:r2"]);
    assert_eq!(view.outcomes[1].status, OutcomeStatus::Dropped);
    assert_eq!(view.outcomes[2].status, OutcomeStatus::Open);
    let snap = snapshot_outcomes(&v);
    assert_eq!(
        snap.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(),
        vec!["o1", "o3"]
    );
    assert!(snap.iter().all(|o| o.source == ROADMAP_SOURCE));
}
